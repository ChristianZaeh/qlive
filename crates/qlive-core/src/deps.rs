//! Which cells must run: a dependency graph instead of "everything after the edit".
//!
//! The cumulative-prefix rule re-runs every cell after an edit, across every later
//! document. It is sound and coarse — measured, adding a cell nothing uses re-ran every
//! cell of the next chapter. Here Python's `ast`/`symtable` (in the kernel worker) says
//! which names each cell defines and reads, and two things decide what runs:
//!
//! **Signatures.** A cell's signature hashes its own source together with the
//! signatures of the cells it actually reads each name from — the last definer of that
//! name before it. A Merkle hash over the dependency graph: it changes exactly when the
//! cell's source or anything it transitively depends on changes. Outputs are cached by
//! signature, so a cell whose signature is cached is reused, wherever it moved to.
//!
//! **The kernel's actual state.** The kernel is one namespace that has run cells in
//! some order, so for every name the planner tracks *which* signature's value the
//! kernel holds. Before a cell runs, each name it reads must hold the value of its
//! last definer; if the kernel holds another — cell 1 `x = 1`, cell 3 `x = 100`, and
//! cell 2 reads `x` — that definer re-runs first. This covers mutation too: re-running
//! `data.append(2)` first restores `data` from its definer instead of appending twice.
//! Nothing is rebound eagerly, so cells that all rebind `fig, ax` do not drag each
//! other along. A name only a deleted cell defined is deleted from the kernel before
//! its reader runs, so the reader fails as it would in a fresh run.
//!
//! Cells the analysis cannot see through — `exec`, `eval`, `globals()`, star imports,
//! IPython magics — are *opaque*: they count as defining every name, their signature
//! covers every cell before them, and every cell after a running opaque cell runs.
//!
//! Assumed: cells are deterministic, and dependencies go through Python names.
//! Mutation through function arguments (`np.random.shuffle(data)`) and dependencies
//! through files are invisible to name analysis.

use std::collections::{BTreeMap, BTreeSet};

/// What the analysis knows about one code cell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CellInfo {
    /// Hash of the cell's own source.
    pub hash: String,
    pub defines: BTreeSet<String>,
    pub uses: BTreeSet<String>,
    /// Cannot be analysed: depends on and defines everything.
    pub opaque: bool,
}

/// For each name bound in the kernel, the signature of the value it holds.
/// [`UNKNOWN`] after an opaque cell ran.
pub type Holder = BTreeMap<String, String>;

/// Holder value after an opaque cell: never equal to a real signature.
pub const UNKNOWN: &str = "?";

/// Why a cell runs. Printed, so a surprising re-run can be explained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// No cached output for its source with these inputs, and none of its inputs is
    /// re-running: new, or its own source changed.
    Changed,
    /// Reads `name` from cell `from`, which runs in this pass.
    Reads { name: String, from: usize },
    /// The kernel holds another cell's `name`; re-run so cell `for_cell` reads this one's.
    Restores { name: String, for_cell: usize },
    /// Reads `name`, which no cell before it defines any more.
    Forgotten { name: String },
    /// Opaque, or after an opaque cell that runs.
    Opaque,
}

impl std::fmt::Display for Why {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Why::Changed => write!(f, "changed"),
            Why::Reads { name, from } => write!(f, "reads `{name}` from cell {}", from + 1),
            Why::Restores { name, for_cell } => {
                write!(f, "restores `{name}` for cell {}", for_cell + 1)
            }
            Why::Forgotten { name } => write!(f, "`{name}` is no longer defined"),
            Why::Opaque => write!(f, "opaque code (exec, magics, globals) or after it"),
        }
    }
}

/// One thing to do to the kernel, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Delete these names from the kernel's namespace.
    Forget(Vec<String>),
    /// Run cell `j`.
    Run(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// Per cell: its signature, the cache key for its outputs.
    pub sigs: Vec<String>,
    /// Per cell: `Some(reason)` if it runs; `None` to reuse the outputs cached under
    /// its signature.
    pub run: Vec<Option<Why>>,
    pub steps: Vec<Step>,
}

/// Plan a pass over `cells`. `cached` says whether outputs exist for a signature;
/// `holder` is the kernel's current state.
pub fn plan(cells: &[CellInfo], cached: impl Fn(&str) -> bool, holder: &Holder) -> Plan {
    let n = cells.len();
    let sigs = signatures(cells);
    let mut run: Vec<Option<Why>> = (0..n)
        .map(|j| if cached(&sigs[j]) { None } else { Some(Why::Changed) })
        .collect();

    // Simulate the pass against the kernel's state. When a cell would read the wrong
    // value, mark its definer to run and simulate again; each round adds a cell, so
    // this ends.
    let steps = loop {
        let mut state = holder.clone();
        let mut steps = Vec::new();
        let mut opaque_ran = false;
        let mut restore: Option<(usize, String, usize)> = None;

        for j in 0..n {
            if run[j].is_none() && opaque_ran {
                run[j] = Some(Why::Opaque);
            }
            if run[j].is_none() {
                continue;
            }
            let mut forget = Vec::new();
            for name in &cells[j].uses {
                match last_definer_before(cells, j, name) {
                    None => {
                        if state.contains_key(name) {
                            forget.push(name.clone());
                        }
                    }
                    Some(i) => {
                        if state.get(name) != Some(&sigs[i]) && run[i].is_none() {
                            restore = Some((i, name.clone(), j));
                            break;
                        }
                    }
                }
            }
            if restore.is_some() {
                break;
            }
            if !forget.is_empty() {
                for name in &forget {
                    state.remove(name);
                }
                steps.push(Step::Forget(forget));
            }
            steps.push(Step::Run(j));
            record(&mut state, &cells[j], &sigs[j]);
            if cells[j].opaque {
                opaque_ran = true;
            }
        }

        match restore {
            Some((i, name, for_cell)) => run[i] = Some(Why::Restores { name, for_cell }),
            None => break steps,
        }
    };

    explain(cells, holder, &mut run);
    Plan { sigs, run, steps }
}

/// Update the kernel state after running `cell` with signature `sig`.
pub fn record(state: &mut Holder, cell: &CellInfo, sig: &str) {
    if cell.opaque {
        for value in state.values_mut() {
            *value = UNKNOWN.to_string();
        }
    }
    for name in &cell.defines {
        state.insert(name.clone(), sig.to_string());
    }
}

/// Apply a `Step::Forget` to the kernel state.
pub fn forget(state: &mut Holder, names: &[String]) {
    for name in names {
        state.remove(name);
    }
}

/// The Merkle signatures, in document order.
pub fn signatures(cells: &[CellInfo]) -> Vec<String> {
    let mut sigs: Vec<String> = Vec::with_capacity(cells.len());
    for (j, cell) in cells.iter().enumerate() {
        let mut material = String::new();
        material.push_str(&cell.hash);
        if cell.opaque {
            // Depends on everything before it.
            material.push_str("\0opaque");
            for sig in &sigs {
                material.push('\0');
                material.push_str(sig);
            }
        } else {
            for name in &cell.uses {
                material.push('\0');
                material.push_str(name);
                material.push('=');
                match last_definer_before(cells, j, name) {
                    Some(i) => material.push_str(&sigs[i]),
                    None => material.push('-'),
                }
            }
        }
        sigs.push(blake3::hash(material.as_bytes()).to_hex().to_string());
    }
    sigs
}

/// The cell whose value of `name` cell `j` reads in a fresh run. An opaque cell may
/// define anything, so it counts.
fn last_definer_before(cells: &[CellInfo], j: usize, name: &str) -> Option<usize> {
    (0..j).rev().find(|&i| cells[i].opaque || cells[i].defines.contains(name))
}

/// Replace the generic `Changed` with the most useful reason for the log.
fn explain(cells: &[CellInfo], holder: &Holder, run: &mut [Option<Why>]) {
    for j in 0..cells.len() {
        if run[j] != Some(Why::Changed) {
            continue;
        }
        if cells[j].opaque {
            run[j] = Some(Why::Opaque);
            continue;
        }
        for name in &cells[j].uses {
            match last_definer_before(cells, j, name) {
                Some(i) if run[i].is_some() => {
                    run[j] = Some(Why::Reads { name: name.clone(), from: i });
                    break;
                }
                None if holder.contains_key(name) => {
                    run[j] = Some(Why::Forgotten { name: name.clone() });
                    break;
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(src: &str, defines: &[&str], uses: &[&str]) -> CellInfo {
        CellInfo {
            hash: src.to_string(),
            defines: defines.iter().map(|s| s.to_string()).collect(),
            uses: uses.iter().map(|s| s.to_string()).collect(),
            opaque: false,
        }
    }

    fn opaque(src: &str) -> CellInfo {
        CellInfo {
            hash: src.to_string(),
            opaque: true,
            ..Default::default()
        }
    }

    /// The kernel after running `cells` from cold, and the signatures it cached.
    fn ran(cells: &[CellInfo]) -> (Holder, BTreeSet<String>) {
        let sigs = signatures(cells);
        let mut holder = Holder::new();
        for (cell, sig) in cells.iter().zip(&sigs) {
            record(&mut holder, cell, sig);
        }
        (holder, sigs.into_iter().collect())
    }

    /// Plan `new` against a kernel that has run `prev`.
    fn plan_after(prev: &[CellInfo], new: &[CellInfo]) -> Plan {
        let (holder, cache) = ran(prev);
        plan(new, |s| cache.contains(s), &holder)
    }

    fn runs(plan: &Plan) -> Vec<usize> {
        plan.run.iter().enumerate().filter_map(|(i, w)| w.as_ref().map(|_| i)).collect()
    }

    #[test]
    fn nothing_changed_runs_nothing() {
        let cells = vec![cell("a", &["x"], &[]), cell("b", &["y"], &["x"])];
        assert!(runs(&plan_after(&cells, &cells)).is_empty());
    }

    #[test]
    fn a_cold_kernel_runs_everything_in_order() {
        let cells = vec![cell("a", &["x"], &[]), cell("b", &["y"], &["x"])];
        let p = plan(&cells, |_| false, &Holder::new());
        assert_eq!(p.steps, vec![Step::Run(0), Step::Run(1)]);
    }

    #[test]
    fn readers_of_a_changed_cell_run_and_unrelated_cells_do_not() {
        let prev = vec![
            cell("data = load()", &["data"], &["load"]),
            cell("other = 1", &["other"], &[]),
            cell("print(sum(data))", &[], &["print", "sum", "data"]),
        ];
        let mut new = prev.clone();
        new[0] = cell("data = load(2)", &["data"], &["load"]);
        let p = plan_after(&prev, &new);
        assert_eq!(runs(&p), vec![0, 2]);
        assert_eq!(p.run[2], Some(Why::Reads { name: "data".into(), from: 0 }));
    }

    #[test]
    fn an_unrelated_new_cell_runs_alone() {
        // The measured over-invalidation: a cell nothing uses re-ran the next chapter.
        let prev = vec![
            cell("data = [1, 4, 9]", &["data"], &[]),
            cell("print(sum(data))", &[], &["print", "sum", "data"]),
            cell("plot(data)", &[], &["plot", "data"]),
        ];
        let mut new = prev.clone();
        new.insert(1, cell("unrelated = 'x'", &["unrelated"], &[]));
        assert_eq!(runs(&plan_after(&prev, &new)), vec![1]);
    }

    #[test]
    fn an_edited_late_cell_runs_alone() {
        let prev = vec![cell("data = slow()", &["data"], &["slow"]), cell("plot(data)", &[], &["plot", "data"])];
        let mut new = prev.clone();
        new[1] = cell("plot(data, 'o')", &[], &["plot", "data"]);
        assert_eq!(runs(&plan_after(&prev, &new)), vec![1], "the slow cell stays cached");
    }

    #[test]
    fn hidden_state_restores_the_value_the_cell_should_read() {
        // x = 1 / y = x + 1 / x = 100. Running cell 2 alone would read x = 100.
        let prev = vec![
            cell("x = 1", &["x"], &[]),
            cell("y = x + 1", &["y"], &["x"]),
            cell("x = 100", &["x"], &[]),
        ];
        let mut new = prev.clone();
        new[1] = cell("y = x + 2", &["y"], &["x"]);
        let p = plan_after(&prev, &new);
        assert_eq!(runs(&p), vec![0, 1], "restore x, then run the edit; cell 3 is not needed");
        assert_eq!(p.run[0], Some(Why::Restores { name: "x".into(), for_cell: 1 }));
        assert_eq!(p.steps, vec![Step::Run(0), Step::Run(1)]);
    }

    #[test]
    fn a_later_reader_then_restores_the_later_binding() {
        // After the pass above the kernel holds x = 1; a reader after cell 3 needs 100.
        let cells = vec![
            cell("x = 1", &["x"], &[]),
            cell("y = x + 2", &["y"], &["x"]),
            cell("x = 100", &["x"], &[]),
        ];
        let sigs = signatures(&cells);
        let mut holder = Holder::new();
        record(&mut holder, &cells[0], &sigs[0]);
        record(&mut holder, &cells[2], &sigs[2]);
        record(&mut holder, &cells[0], &sigs[0]); // the restore
        record(&mut holder, &cells[1], &sigs[1]);
        let mut new = cells.clone();
        new.push(cell("print(x)", &[], &["print", "x"]));
        let cache: BTreeSet<String> = sigs.into_iter().collect();
        let p = plan(&new, |s| cache.contains(s), &holder);
        assert_eq!(runs(&p), vec![2, 3]);
        assert_eq!(p.run[2], Some(Why::Restores { name: "x".into(), for_cell: 3 }));
    }

    #[test]
    fn rerunning_a_mutation_restores_its_input_instead_of_applying_it_twice() {
        // data = [1] / data.append(2) (reads and defines data) / print(data)
        let prev = vec![
            cell("data = [1]", &["data"], &[]),
            cell("data.append(2)", &["data"], &["data"]),
            cell("print(data)", &[], &["print", "data"]),
        ];
        let mut new = prev.clone();
        new[1] = cell("data.append(3)", &["data"], &["data"]);
        let p = plan_after(&prev, &new);
        assert_eq!(runs(&p), vec![0, 1, 2]);
        assert_eq!(p.run[0], Some(Why::Restores { name: "data".into(), for_cell: 1 }));
        assert_eq!(p.run[2], Some(Why::Reads { name: "data".into(), from: 1 }));
    }

    #[test]
    fn cells_that_all_rebind_fig_and_ax_do_not_drag_each_other_along() {
        let prev = vec![
            cell("fig, ax = plt.subplots(); ax.plot(a)", &["fig", "ax"], &["plt", "a"]),
            cell("fig, ax = plt.subplots(); ax.plot(b)", &["fig", "ax"], &["plt", "b"]),
        ];
        let mut new = prev.clone();
        new[0] = cell("fig, ax = plt.subplots(); ax.plot(a, 'o')", &["fig", "ax"], &["plt", "a"]);
        assert_eq!(runs(&plan_after(&prev, &new)), vec![0]);
    }

    #[test]
    fn an_opaque_cell_runs_after_any_change_and_takes_everything_after_with_it() {
        let prev = vec![cell("a = 1", &["a"], &[]), opaque("exec(code)"), cell("b = 2", &["b"], &[])];
        let mut new = prev.clone();
        new[0] = cell("a = 2", &["a"], &[]);
        let p = plan_after(&prev, &new);
        assert_eq!(runs(&p), vec![0, 1, 2]);
        assert_eq!(p.run[1], Some(Why::Opaque));
        assert_eq!(p.run[2], Some(Why::Opaque));
    }

    #[test]
    fn an_opaque_cell_before_the_edit_is_left_alone() {
        let prev = vec![opaque("%matplotlib inline"), cell("a = 1", &["a"], &[])];
        let mut new = prev.clone();
        new[1] = cell("a = 2", &["a"], &[]);
        assert_eq!(runs(&plan_after(&prev, &new)), vec![1]);
    }

    #[test]
    fn deleting_a_definer_forgets_the_name_before_its_reader_runs() {
        let prev = vec![
            cell("x = 1", &["x"], &[]),
            cell("print(x)", &[], &["print", "x"]),
            cell("z = 3", &["z"], &[]),
        ];
        let new = vec![prev[1].clone(), prev[2].clone()];
        let p = plan_after(&prev, &new);
        assert_eq!(runs(&p), vec![0]);
        assert_eq!(p.steps, vec![Step::Forget(vec!["x".into()]), Step::Run(0)]);
        assert_eq!(p.run[0], Some(Why::Forgotten { name: "x".into() }));
    }

    #[test]
    fn deleting_a_later_redefinition_costs_nothing_until_someone_reads_it() {
        let prev = vec![cell("x = 1", &["x"], &[]), cell("x = 2", &["x"], &[])];
        let new = vec![prev[0].clone()];
        assert!(runs(&plan_after(&prev, &new)).is_empty());
    }

    #[test]
    fn inserting_a_definer_reruns_its_readers() {
        let prev = vec![cell("a = 1", &["a"], &[]), cell("print(y)", &[], &["print", "y"])];
        let new = vec![prev[0].clone(), cell("y = a + 1", &["y"], &["a"]), prev[1].clone()];
        assert_eq!(runs(&plan_after(&prev, &new)), vec![1, 2]);
    }

    #[test]
    fn transitive_readers_all_run_and_independent_cells_do_not() {
        let prev = vec![
            cell("a = 1", &["a"], &[]),
            cell("b = a + 1", &["b"], &["a"]),
            cell("c = b + 1", &["c"], &["b"]),
            cell("d = 7", &["d"], &[]),
        ];
        let mut new = prev.clone();
        new[0] = cell("a = 5", &["a"], &[]);
        assert_eq!(runs(&plan_after(&prev, &new)), vec![0, 1, 2]);
    }

    #[test]
    fn moving_independent_cells_reuses_their_outputs() {
        let prev = vec![cell("a = 1", &["a"], &[]), cell("b = 2", &["b"], &[])];
        let new = vec![prev[1].clone(), prev[0].clone()];
        assert!(runs(&plan_after(&prev, &new)).is_empty());
    }

    #[test]
    fn builtins_never_force_a_rerun() {
        let prev = vec![cell("print(len([1]))", &[], &["print", "len"])];
        assert!(runs(&plan_after(&prev, &prev)).is_empty());
    }
}
