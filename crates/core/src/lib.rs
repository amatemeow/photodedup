//! photodedup-core: the detection/classification engine, with no I/O
//! policy of its own (no path to a UI, no knowledge of HTTP) — see the
//! architecture plan's crate layout. `crates/cli` and (from M2) `crates/
//! server` are the two consumers.
//!
//! Status: M0 complete (see `/SPIKES.md`, `/ROADMAP.md`). M1 (scan ->
//! classify -> cluster -> keeper pipeline, SQLite persistence) built and
//! parity-tested against the reference dataset.

pub mod apply;
pub mod classify;
pub mod cluster;
pub mod decode;
pub mod exacthash;
pub mod fsguard;
pub mod grouping;
pub mod hash;
pub mod keeper;
pub mod metadata;
pub mod model;
pub mod pipeline;
pub mod platform;
pub mod provenance;
pub mod resolve;
pub mod scan;
pub mod search;
pub mod store;
pub mod throttle;
pub mod thresholds;
pub mod thumbnail;
pub mod tools;
pub mod video_match;
