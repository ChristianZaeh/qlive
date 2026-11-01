//! One Jupyter kernel for a chain of documents, and the dependency planner that
//! decides which of their cells run. Shared by `qlive live` and `qlive cells`.
//!
//! Each pass: expand includes, split every document into cells, analyse any cell
//! whose source is new (Python's `ast`, in the worker — never executed), plan with
//! [`qlive_core::deps`], then forget and run exactly what the plan says. Outputs are
//! cached by the cell's dependency signature; the kernel's state — which signature's
//! value each name holds — is tracked across passes so hidden state is caught.

use anyhow::{anyhow, Result};
use qlive_core::cells::{self, Cell, CellOutputs};
use qlive_core::deps::{self, CellInfo, Holder, Step};
use qlive_core::{frontmatter, Toolchain};
use qlive_kernel::Kernel;
use qlive_quarto::engine;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use serde_json::Value;
use std::time::{Duration, Instant};

/// Documents whose cells share one kernel, in execution order. The last one is the
/// document being previewed. With a master file this is just the master: its
/// includes are expanded into it, in order.
#[derive(Clone)]
pub struct Chain {
    pub docs: Vec<PathBuf>,
}

impl Chain {
    pub fn target(&self) -> &Path {
        self.docs.last().expect("a chain always has its target")
    }
}

pub struct Session {
    pub kernel: Kernel,
    /// Outputs by dependency signature.
    cache: HashMap<String, CellOutputs>,
    /// Which signature's value the kernel holds, per name.
    holder: Holder,
    /// Analysis by source hash, so unchanged cells are not re-parsed.
    analysis: HashMap<String, CellInfo>,
    /// Hash of Quarto's kernel setup: a kept kernel set up differently is restarted.
    setup: String,
    /// The kernel lives in a daemon, which keeps this bookkeeping alongside it.
    persistent: bool,
}

/// Where the kernel runs.
#[derive(Clone, Copy)]
pub enum KernelMode {
    /// Owned by this process, gone when it exits.
    Private,
    /// In a daemon that outlives qlive, so the next start finds its variables.
    /// `fresh` replaces a running one.
    Persistent { fresh: bool },
}

/// Minutes without a client before a kernel daemon exits: `QLIVE_KERNEL_IDLE_MINUTES`.
fn idle() -> Duration {
    let minutes = std::env::var("QLIVE_KERNEL_IDLE_MINUTES").ok().and_then(|m| m.parse().ok()).unwrap_or(120u64);
    Duration::from_secs(minutes * 60)
}

/// Daemon state files live here; `qlive kernels` lists them.
pub fn kernels_dir() -> Option<PathBuf> {
    Some(crate::restart::base()?.join("kernels"))
}

/// One daemon per Python, worker protocol and chain of documents.
fn daemon_state(python: &Path, chain: &Chain) -> Result<PathBuf> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(python.to_string_lossy().as_bytes());
    hasher.update(qlive_kernel::worker_version().as_bytes());
    for doc in &chain.docs {
        hasher.update(b"\0");
        hasher.update(doc.to_string_lossy().as_bytes());
    }
    let dir = kernels_dir().ok_or_else(|| anyhow!("no cache directory"))?;
    Ok(dir.join(format!("{}.json", &hasher.finalize().to_hex()[..24])))
}

/// Quarto's kernel setup depends on the same inputs as the restart cache, so it is
/// kept there: a restart that finds its kernel running needs no `quarto inspect`.
fn kernel_setup(tools: &Toolchain, target: &Path) -> Result<qlive_quarto::inspect::KernelSetup> {
    let expanded = read_expanded(target)?;
    let front = frontmatter::split(&expanded.text).0;
    let mut metadata = Vec::new();
    for file in &expanded.files {
        if let Some(f) = frontmatter::split(&std::fs::read_to_string(file)?).0 { metadata.push((file.clone(), f.to_owned())); }
    }
    let key = crate::restart::key(tools, target, "kernel-setup", front, &metadata)?;
    if let Some(value) = crate::restart::load_value(&key) {
        if let (Some(script), false, false) = (value["script"].as_str(), value["args"].is_null(), value["env"].is_null()) {
            if Path::new(script).is_file() {
                return Ok(qlive_quarto::inspect::KernelSetup { script: script.into(), args: value["args"].clone(), env: value["env"].clone() });
            }
        }
    }
    let setup = qlive_quarto::inspect::kernel_setup(&tools.quarto, target, "html")?;
    let value = serde_json::json!({"script": setup.script, "args": setup.args, "env": setup.env});
    if let Err(e) = crate::restart::store_value(&key, &value) { println!("qlive: kernel setup not cached: {e}"); }
    Ok(setup)
}

fn setup_hash(setup: &qlive_quarto::inspect::KernelSetup) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(serde_json::json!({"script": setup.script, "args": setup.args, "env": setup.env}).to_string().as_bytes());
    hasher.update(&std::fs::read(&setup.script).unwrap_or_default());
    hasher.finalize().to_hex()[..24].to_owned()
}

impl Session {
    fn state(&self) -> Value {
        serde_json::json!({"setup": self.setup, "cache": self.cache, "holder": self.holder, "analysis": self.analysis})
    }

    /// Adopt what the daemon kept. `false` when it is missing, from another setup, or
    /// was invalidated by a pass that never finished.
    fn restore(&mut self, state: &Value) -> bool {
        if state["setup"].as_str() != Some(self.setup.as_str()) { return false; }
        let parts = (
            serde_json::from_value(state["cache"].clone()),
            serde_json::from_value(state["holder"].clone()),
            serde_json::from_value(state["analysis"].clone()),
        );
        let (Ok(cache), Ok(holder), Ok(analysis)) = parts else { return false };
        (self.cache, self.holder, self.analysis) = (cache, holder, analysis);
        true
    }

    /// Whether the kernel outlives this process (a daemon could not be reached otherwise).
    pub fn is_persistent(&self) -> bool {
        self.persistent
    }

    fn save(&mut self) -> Result<()> {
        if self.persistent { let state = self.state(); self.kernel.stash(&state)?; }
        Ok(())
    }
}

/// One pass over the chain.
pub struct Pass {
    /// The target document's cells.
    pub cells: Vec<Cell>,
    /// Indexed like `cells`.
    pub outputs: Vec<Option<CellOutputs>>,
    /// Cells run in this pass, whose figures must be (re)written.
    pub fresh: Vec<bool>,
    pub executed: usize,
    pub reused: usize,
    pub kernel_ms: f64,
    /// One line per cell that ran, saying why.
    pub ran: Vec<String>,
}

/// Boot the kernel with Quarto's own setup, so figures match Quarto's. A persistent
/// kernel still running from an earlier start is reused with its variables and
/// bookkeeping, so no cell runs again unless it changed.
pub fn start_session(tools: &Toolchain, chain: &Chain, mode: KernelMode) -> Result<Session> {
    let target = chain.target();
    let python = tools.python.clone().ok_or_else(|| {
        anyhow!("no usable Python 3 found (the Microsoft Store stub does not count); set QLIVE_PYTHON")
    })?;
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("document has no parent directory"))?;

    let started = Instant::now();
    // Booting ipykernel and asking Quarto for its setup are independent and each
    // takes about a second or more; overlap them.
    let (kernel, setup) = std::thread::scope(|scope| {
        let setup = scope.spawn(|| kernel_setup(tools, target));
        let kernel = match mode {
            KernelMode::Private => Kernel::start(&python, dir).map(|k| (k, false)),
            KernelMode::Persistent { fresh } => daemon_state(&python, chain).and_then(|state| {
                if fresh { let _ = Kernel::stop_daemon(&state); }
                Kernel::persistent(&python, dir, &state, &target.display().to_string(), idle())
            }).or_else(|e| {
                println!("qlive: kernel daemon unavailable ({e:#}); using a kernel that ends with qlive");
                Kernel::start(&python, dir).map(|k| (k, false))
            }),
        };
        (kernel, setup.join().unwrap_or_else(|_| Err(anyhow!("quarto inspect panicked"))))
    });
    let ((kernel, reused), setup) = (kernel?, setup?);
    let persistent = matches!(mode, KernelMode::Persistent { .. }) && kernel.is_daemon();
    let mut session = Session {
        kernel,
        cache: HashMap::new(),
        holder: Holder::new(),
        analysis: HashMap::new(),
        setup: setup_hash(&setup),
        persistent,
    };
    if reused {
        let state = session.kernel.unstash()?;
        if session.restore(&state) {
            println!(
                "qlive: kept kernel reused in {:.0} ms: {} outputs, {} variables still defined",
                started.elapsed().as_secs_f64() * 1000.0, session.cache.len(), session.holder.len()
            );
            return Ok(session);
        }
        println!("qlive: kept kernel had another setup or an unfinished pass; restarting it");
        session.kernel.restart()?;
    }
    session.kernel.setup(&setup.script, &setup.args, &setup.env)?;
    session.save()?;
    println!(
        "qlive: kernel ready with Quarto's setup in {:.0} ms{}",
        started.elapsed().as_secs_f64() * 1000.0,
        if session.persistent { " (kept running after qlive exits)" } else { " (paid once)" }
    );
    Ok(session)
}

/// Run exactly the cells the dependency planner says, and collect the target's
/// outputs.
pub fn run_chain(session: &mut Session, chain: &Chain) -> Result<Pass> {
    let mut docs = Vec::with_capacity(chain.docs.len());
    for doc in &chain.docs {
        docs.push(cells::from_markdown(&read_expanded(doc)?.text));
    }
    // Keys only give cells a stable id in the rendered page; the plan does not use them.
    cells::chain_keys(&mut docs);

    let code: Vec<(usize, usize)> = docs
        .iter()
        .enumerate()
        .flat_map(|(d, cs)| cs.iter().enumerate().filter(|(_, c)| c.is_code()).map(move |(i, _)| (d, i)))
        .collect();
    let sources: Vec<&str> = code.iter().map(|&(d, i)| docs[d][i].source.as_str()).collect();
    let infos = analyse(session, &sources)?;

    let plan = deps::plan(&infos, |sig| session.cache.contains_key(sig), &session.holder);
    let inputs = cell_inputs(&infos);
    let mut failed: Vec<bool> = plan.sigs.iter().map(|sig| session.cache.get(sig).is_some_and(has_error)).collect();

    let (mut executed, mut kernel_ms) = (0usize, 0.0f64);
    let mut ran = Vec::new();
    // From here the kernel's variables may move ahead of the bookkeeping. Drop the
    // kept copy until the pass completes, so a qlive killed mid-pass leaves a kernel
    // the next start restarts instead of trusts.
    if session.persistent && !plan.steps.is_empty() { session.kernel.stash(&Value::Null)?; }
    for step in &plan.steps {
        match step {
            Step::Forget(names) => {
                session.kernel.forget(names)?;
                deps::forget(&mut session.holder, names);
            }
            Step::Run(k) => {
                let (d, i) = code[*k];
                let source = &docs[d][i].source;
                let blocked = inputs[*k].iter().copied().find(|i| failed[*i]);
                let outputs = if let Some(upstream) = blocked {
                    CellOutputs {
                        outputs: vec![serde_json::json!({"output_type": "error", "ename": "DependencyError",
                            "evalue": format!("Blocked: cell {} failed. Fix that cell first.", upstream + 1), "traceback": []})],
                        execution_count: None,
                    }
                } else if engine::cell_options(source).eval {
                    let result = session.kernel.run(source)?;
                    kernel_ms += result.ms;
                    executed += 1;
                    CellOutputs {
                        outputs: result.outputs,
                        execution_count: result.execution_count,
                    }
                } else {
                    CellOutputs::default()
                };
                failed[*k] = has_error(&outputs);
                session.cache.insert(plan.sigs[*k].clone(), outputs);
                if failed[*k] {
                    // A Python exception may leave old or partially updated bindings.
                    // None of a failed cell's promised outputs is usable downstream.
                    let names: Vec<_> = if infos[*k].opaque {
                        session.holder.keys().chain(infos[*k].defines.iter()).cloned().collect()
                    } else { infos[*k].defines.iter().cloned().collect() };
                    session.kernel.forget(&names)?;
                    deps::forget(&mut session.holder, &names);
                } else {
                    deps::record(&mut session.holder, &infos[*k], &plan.sigs[*k]);
                }
                if let Some(why) = &plan.run[*k] {
                    let status = if blocked.is_some() { "blocked" } else if failed[*k] { "failed" } else { "ran" };
                    ran.push(format!("cell {} `{}`: {status}, {why}", k + 1, first_line(source)));
                }
            }
        }
    }

    session.save()?;

    let target = docs.len() - 1;
    let mut outputs = vec![None; docs[target].len()];
    let mut fresh = vec![false; docs[target].len()];
    for (k, &(d, i)) in code.iter().enumerate() {
        if d == target {
            outputs[i] = session.cache.get(&plan.sigs[k]).cloned();
            fresh[i] = plan.run[k].is_some();
        }
    }

    Ok(Pass {
        cells: docs.pop().unwrap_or_default(),
        outputs,
        fresh,
        executed,
        reused: plan.run.iter().filter(|r| r.is_none()).count(),
        kernel_ms,
        ran,
    })
}

fn has_error(outputs: &CellOutputs) -> bool {
    outputs.outputs.iter().any(|output| output["output_type"] == "error")
}

/// Match the planner's last-writer dependencies, including conservative opaque cells.
fn cell_inputs(infos: &[CellInfo]) -> Vec<Vec<usize>> {
    let mut writers = HashMap::<String, usize>::new();
    let mut opaque = None;
    let mut inputs = Vec::new();
    for (i, info) in infos.iter().enumerate() {
        let mut reads = if info.opaque { (0..i).collect::<Vec<_>>() } else {
            info.uses.iter().filter_map(|name| writers.get(name).copied().or(opaque)).collect()
        };
        if let Some(previous) = opaque { reads.push(previous); }
        reads.sort_unstable();
        reads.dedup();
        inputs.push(reads);
        if info.opaque { opaque = Some(i); writers.clear(); }
        for name in &info.defines { writers.insert(name.clone(), i); }
    }
    inputs
}

/// What each cell defines and reads. Cached by source; only new sources are parsed.
fn analyse(session: &mut Session, sources: &[&str]) -> Result<Vec<CellInfo>> {
    let hashes: Vec<String> = sources
        .iter()
        .map(|s| blake3::hash(s.as_bytes()).to_hex().to_string())
        .collect();

    let missing: Vec<usize> = (0..sources.len())
        .filter(|&k| !session.analysis.contains_key(&hashes[k]))
        .collect();
    if !missing.is_empty() {
        let codes: Vec<String> = missing.iter().map(|&k| sources[k].to_string()).collect();
        let found = session.kernel.analyze(&codes)?;
        for (&k, a) in missing.iter().zip(found) {
            let mut info = CellInfo {
                hash: hashes[k].clone(),
                defines: a.defines.into_iter().collect(),
                uses: a.uses.into_iter().collect(),
                opaque: a.opaque,
            };
            let options = engine::cell_options(sources[k]);
            if !options.eval {
                // Never runs, so it binds nothing and reads nothing.
                info.defines.clear();
                info.uses.clear();
                info.opaque = false;
            }
            // `#| depends: a, b` adds reads the analysis cannot see: a file another
            // cell writes, or mutation through a function argument.
            info.uses.extend(depends_option(sources[k]));
            session.analysis.insert(hashes[k].clone(), info);
        }
    }

    Ok(hashes.iter().map(|h| session.analysis[h].clone()).collect())
}

/// `#| depends: data, model` -> ["data", "model"].
pub fn depends_option(source: &str) -> Vec<String> {
    source
        .lines()
        .take_while(|l| l.trim_start().starts_with("#|"))
        .filter_map(|l| l.trim_start().trim_start_matches("#|").trim().strip_prefix("depends:"))
        .flat_map(|names| names.split([',', ' ']).map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect::<Vec<_>>())
        .collect()
}

/// A document's text with every `{{< include >}}` expanded, as Quarto feeds it on.
pub fn read_expanded(doc: &Path) -> Result<qlive_core::includes::Expanded> {
    let source = std::fs::read_to_string(doc)?;
    qlive_core::includes::expand(&source, doc)
}

fn first_line(source: &str) -> String {
    let line = source
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim_start().starts_with("#|"))
        .unwrap_or("")
        .trim();
    if line.chars().count() > 40 {
        format!("{}…", line.chars().take(40).collect::<String>())
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_depends_option() {
        assert_eq!(depends_option("#| depends: data, model\nx = 1"), vec!["data", "model"]);
        assert_eq!(depends_option("#| echo: false\n#| depends: a b\nx"), vec!["a", "b"]);
        assert!(depends_option("x = 1\n#| depends: late").is_empty(), "options only at the top");
    }
}
