# qlive

Instant Quarto preview. Quarto keeps the cold path — templates, themes, Lua filters,
bibliography, formats. qlive owns the hot loop: warm engines, block-level diffing, and
patches pushed to the browser instead of page reloads.

**Status: M0 scaffold. Compiles clean; 7 unit tests pass.** The tokio-linked targets
need the MSVC toolchain on Windows — see *Building* below.

## Why

Measured on the development machine (Quarto 1.8.27, pandoc 3.6.3, typst 0.13.0,
Windows 11, 283-line document):

| path | cost |
|---|---:|
| `quarto preview` save → browser reload | 1040 ms |
| `quarto render --to html` one shot | 1906 ms |
| `pandoc.exe` one-shot | 74 ms |
| warm pandoc, whole document | ~16 ms |
| warm pandoc, **one block** | **0.52 ms** |
| `pandoc server` HTTP, one block | 1.2 ms |
| `typst watch` rebuild, as typst reports it | **3.1 ms** |
| any Python cell, on top of the above | +4300 ms |

Almost all of every Quarto save is process startup and pipeline setup; the actual
rendering of a mid-size document is ~16 ms, and a single block is half a millisecond.
That gap — roughly 2000× — is the project. A document with Python cells is a separate
problem: a flat ~4.3 s of jupyter engine regardless of what the cells do, which is
what the kernel pool in M5 addresses.

`qlive bench` reproduces this table wherever it runs, so the premise stays a claim you
can check rather than one you have to trust.

**Run it on an idle machine.** Every number here is CPU-bound and measured by wall
clock. A run taken next to a game and a browser came out 2–16× worse across the board
(55 s for the cold `quarto render`, 10.8 s for a one-shot typst compile), which says
nothing about either tool.

### M0 result

`qlive bench`, 283-line fixture, 121 blocks, 20 warm iterations, **idle machine**:

| step | ms |
|---|---:|
| quarto render --to html (cold, Deno included) | 1905.8 |
| pandoc one-shot, whole doc (process start) | 74.1 |
| pandoc server, whole doc | 23.5 |
| pandoc server, one block | **1.2** |
| typst compile, one-shot (process start) | 253.0 |
| typst watch, edit -> new PDF (end to end) | 132.5 |
| &nbsp;&nbsp;of which typst reports as compile | **3.1** |

> A one-word edit dirties 1 of 121 blocks.

That last line is the premise, now measured rather than asserted: the hot path has one
block to re-render, at ~1 ms, in place of the ~1.9 s cold render it replaces. The
132 ms end-to-end against typst's own 3.1 ms is file-watch latency, which is exactly
the argument for M4 using typst as a library instead of a subprocess.

For scale, `quarto preview` save → browser reload on the same idle machine is
**1040 ms** (1046 / 1038 / 1042 over three edits). That is the number to beat.

The same bench on a machine running a game gave 14270 ms for the cold render and
4.9–10.9 ms per block — 3–7× worse throughout. The ratios survived; nothing else did.

One finding worth keeping: **`typst watch` sometimes misses an edit outright.** Twice
in four runs the process stayed alive, its last line a successful startup compile, and
the output never moved for a full minute. `TypstWatch::time_edit` therefore re-writes
the source and waits again rather than trusting a single event. A preview built on a
file watcher inherits this; one built on typst-as-a-library does not.

## Commands

```
qlive doctor                  which quarto / pandoc / typst were found, and versions
qlive bench [--doc F]         print the latency table for this machine
qlive preview F [--port N]    cold render once, then re-render on save
```

`qlive preview` is deliberately dumb in M0: it still runs a full `quarto render` per
save, but the server, the socket and the browser tab survive, so nothing restarts and
the page keeps its scroll position. That alone removes the Deno tax from every save.

## Layout

```
crates/qlive-core      block splitter, hashing, dirty-block counting, tool discovery
crates/qlive-pandoc    warm pandoc: `pandoc server` now, `pandoc lua` worker at M1
crates/qlive-quarto    cold renders, output paths, shell extraction (M2)
crates/qlive-typst     typst subprocess now, typst-as-library at M4
crates/qlive-server    axum static server + /qlive/ws patch channel + client.js
crates/qlive-cli       the `qlive` binary
```

## Building

```
cargo test --workspace
```

```
cargo run -p qlive-cli -- doctor
```

```
cargo run -p qlive-cli --release -- bench
```

Dependency versions are pinned to majors that match the APIs used here: `axum 0.7`
(`Message::Text(String)`, `/*path` wildcard routes), `notify 6`
(`recommended_watcher`), `ureq 2` (`send_string`). If you bump either major, expect
the server and watcher to need edits.

### Windows: use the MSVC toolchain

**`stable-x86_64-pc-windows-gnu` as rustup ships it cannot build this project.** The
findings, so nobody repeats them:

* With no linker at all, rustc silently picks up Git Bash's `/usr/bin/link.exe` —
  which is GNU coreutils' hard-link tool — and reports
  `link: extra operand ... Try 'link --help'` from random build scripts.
* The windows-gnu toolchain's `self-contained` directory holds `dlltool.exe`, `ld.exe`
  and `x86_64-w64-mingw32-gcc.exe` but **no `as.exe`**, so `dlltool` fails with a bare
  `CreateProcess` error when `windows-sys` asks for import libraries.
* Pointing rustc at LLVM's dlltool instead (`-Cdlltool=…/llvm-dlltool.exe`, created by
  copying the multicall `llvm-ar.exe`) makes everything *compile* — and produces
  binaries that die at startup with `STATUS_ACCESS_VIOLATION`. Verified with a
  five-line tokio program, so it is the import libraries, not this code.

Status on that toolchain: `cargo check --workspace --all-targets` is clean, and the
crates without tokio in their tree pass their tests (`qlive-core` 6/6, `qlive-quarto`
1/1). Anything linking tokio cannot run.

So, on Windows:

```
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

```
rustup default stable-x86_64-pc-windows-msvc
```

MSVC's `link.exe` handles raw-dylib imports itself, so no `dlltool` is involved. It is
also what M5 will need, since the ZeroMQ crates want a C compiler. The alternative, if
you would rather stay on GNU, is a real mingw-w64 binutils (MSYS2), which provides a
working `as` and `dlltool`.

## Roadmap

| # | Scope | Gate |
|---|---|---|
| **M0** | this scaffold: bench + cold render + watch + reload | a save costs one `quarto render`, not a `quarto preview` restart |
| **M1** | `pandoc lua` worker, whole-document warm passes | p50 under 200 ms on a 300-line doc |
| **M2** | block rendering + DOM patching, `data-qlive-id` | p50 under 25 ms; HTML identical to `quarto render` on a 50-doc corpus |
| **M3** | VS Code buffer streaming, no save needed | keystroke → pixel under 50 ms p95 |
| **M4** | typst as a library, per-page SVG patches | under 30 ms per edit |
| **M5** | Jupyter kernel pool + per-chunk cache | prose edit = 0 executions |

The hard part is not speed. Quarto's Lua filter runtime is undocumented and
version-coupled, so M1 onwards must pin a Quarto version and gate every commit on a
corpus test that diffs qlive's HTML against `quarto render`'s. Fast and approximately
right is worse than slow.
