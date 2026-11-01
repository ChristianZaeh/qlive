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
use qlive_core::Toolchain;
use qlive_kernel::Kernel;
use qlive_quarto::engine;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

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

/// Boot the kernel with Quarto's own setup, so figures match Quarto's.
pub fn start_session(tools: &Toolchain, target: &Path) -> Result<Session> {
    let python = tools.python.clone().ok_or_else(|| {
        anyhow!("no usable Python 3 found (the Microsoft Store stub does not count); set QLIVE_PYTHON")
    })?;
    let dir = target
        .parent()
        .ok_or_else(|| anyhow!("document has no parent directory"))?;

    let started = Instant::now();
    let mut kernel = Kernel::start(&python, dir)?;
    let setup = qlive_quarto::inspect::kernel_setup(&tools.quarto, target, "html")?;
    kernel.setup(&setup.script, &setup.args, &setup.env)?;
    println!(
        "qlive: kernel ready with Quarto's setup in {:.0} ms (paid once)",
        started.elapsed().as_secs_f64() * 1000.0
    );

    Ok(Session {
        kernel,
        cache: HashMap::new(),
        holder: Holder::new(),
        analysis: HashMap::new(),
    })
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

    let (mut executed, mut kernel_ms) = (0usize, 0.0f64);
    let mut ran = Vec::new();
    for step in &plan.steps {
        match step {
            Step::Forget(names) => {
                session.kernel.forget(names)?;
                deps::forget(&mut session.holder, names);
            }
            Step::Run(k) => {
                let (d, i) = code[*k];
                let source = &docs[d][i].source;
                let outputs = if engine::cell_options(source).eval {
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
                session.cache.insert(plan.sigs[*k].clone(), outputs);
                deps::record(&mut session.holder, &infos[*k], &plan.sigs[*k]);
                if let Some(why) = &plan.run[*k] {
                    ran.push(format!("cell {} `{}`: {why}", k + 1, first_line(source)));
                }
            }
        }
    }

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
