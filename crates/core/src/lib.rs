//! photodedup-core: the detection/classification engine, with no I/O
//! policy of its own (no path to a UI, no knowledge of HTTP) — see the
//! architecture plan's crate layout. `crates/cli` and (from M2) `crates/
//! server` are the two consumers.
//!
//! Status: M0 complete (see `/SPIKES.md`, `/ROADMAP.md`). `hash` and
//! `provenance` are real, tested modules ported from validated spikes.
//! `scan`, `cluster`, `search`, `apply` are M1 work, not yet started.

pub mod hash;
pub mod provenance;
