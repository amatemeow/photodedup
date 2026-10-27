# photodedup

A cross-platform (macOS/Windows/Linux), self-contained, open-source app for
finding and reviewing duplicate photos/videos in a folder — point it at a
location, get a reviewable report, choose what to keep and what to
quarantine (never delete).

Grew out of a working single-purpose MVP (see `SPIKES.md` for what carried
over and what was validated). Architecture, licensing posture, and
milestones: see `ROADMAP.md` for deferred work, `SPIKES.md` for the M0
feasibility findings that shaped the dependency list.

**Status: M0 complete.** `crates/core` has real, tested `hash` and
`provenance` modules. Scan/clustering/apply (M1) not yet started.

## License

Apache-2.0 — see `LICENSE`. Third-party dependency licenses are enforced by
`cargo deny check` (config: `deny.toml`) and listed in
`THIRD-PARTY-LICENSES.md` once `cargo-about` is wired into CI.

## Building

```
cargo build
cargo test            # fast, no external dataset needed
cargo deny check       # license/advisory/source hygiene gate
```
