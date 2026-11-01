//! The `pandoc lua` worker — M1, not implemented yet.
//!
//! `pandoc lua` (verified present in pandoc 3.6.3) is a persistent Lua interpreter
//! with the full `pandoc.*` API: `pandoc.read`, `pandoc.write`, filter walking. A
//! long-lived worker speaking a line protocol over stdin/stdout gives us two things
//! `pandoc server` cannot:
//!
//! 1. ~3.9 ms per block instead of 6–9 ms, because there is no HTTP hop; and
//! 2. **Quarto's own Lua filters**, loaded from `<quarto>/share/filters`, which is
//!    the only way output can match `quarto render` byte for byte.
//!
//! Planned protocol — one JSON object per line, in and out:
//!
//! ```text
//! -> {"id":7,"op":"render","from":"markdown","to":"html","text":"…"}
//! <- {"id":7,"ok":true,"out":"<p>…</p>","ms":3.9}
//! ```
//!
//! The worker script holds the parsed document between calls so that a tier-B
//! whole-document pass does not re-read unchanged blocks.
//!
//! The hard part is not the protocol. Quarto's filters expect Quarto's runtime: the
//! `quarto` global, crossref state, `PANDOC_WRITER_OPTIONS`, and the params Quarto
//! passes through its filter-params environment variable. That environment has to be
//! stubbed, it is undocumented, and it moves between Quarto versions. Hence:
//!
//! * pin one Quarto version per qlive release;
//! * escalate to a real `quarto render` whenever the stub cannot satisfy a filter;
//! * gate every commit on a corpus test that diffs qlive's HTML against
//!   `quarto render`'s. Fast and approximately right is worse than slow.
