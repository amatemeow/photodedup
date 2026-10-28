//! OS-specific scheduling knobs behind one interface, selected once at
//! startup -- not scattered `#[cfg(target_os = ...)]` blocks at every call
//! site. Each platform gets its own module with one implementation of
//! `PlatformScheduler`; `platform()` resolves the right one for the
//! current build target exactly once (a `OnceLock`-backed singleton, the
//! same shape as a DI "bean") and every caller goes through that one
//! object via the trait, never the concrete type.
//!
//! Why this exists as its own seam: `throttle.rs`'s whole job is picking
//! the right process/thread scheduling hints for the OS it's running on
//! (see its module docs for *why* that matters -- it's not a performance
//! nicety, it's what keeps a background scan from sounding like a jet
//! engine). That logic was originally a handful of free functions each
//! wearing their own `#[cfg(...)]`, which works but means every new
//! platform concern adds another scattered conditional. One trait with
//! one impl per platform scales better and is easier to test in isolation
//! (a platform's impl can be unit-tested on its own OS; the trait boundary
//! means `throttle.rs` itself needs no `#[cfg]` at all).

use std::sync::OnceLock;

/// How strongly to bias scheduling away from performance cores. Two
/// distinct levels, not a single on/off flag, because **they measurably
/// behave differently, not just "more vs less" of the same thing**: on
/// this project's 8P+4E reference machine (`sudo powermetrics`, see
/// `/ROADMAP.md` and the session's memory notes for the full before/after),
/// `Utility` lowered priority but the scheduler still spilled real work
/// onto P-cores under load (P-core cluster stayed at ~75% utilization);
/// `Background` actually held the line (P-cores parked at 0MHz, E-cluster
/// saturated at ~98%, ~8x lower package power). Which one a tier should
/// use is a genuine product decision, not a tuning knob -- see
/// `throttle::ScanIntensity`'s doc comments for which tier gets which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchedulingHint {
    /// Strongest E-core bias; most willing to let work wait under
    /// contention. Measured ~8x lower CPU package power than `Utility` on
    /// the reference machine, at a real, substantial wall-clock cost.
    Background,
    /// Lower priority than default, but the scheduler may still place
    /// work on performance cores under sustained demand -- "polite, not
    /// invisible."
    Utility,
}

/// OS-level scheduling hints this crate needs. Every method is
/// best-effort: a platform that can't do something returns a sensible
/// default (`None`, or simply does nothing) rather than erroring --
/// scheduling hints are never load-bearing for correctness, only for
/// being a good citizen on the user's machine.
pub trait PlatformScheduler: Send + Sync {
    /// Count of "efficiency" or background-tier cores, if this platform
    /// exposes that distinction separately from total logical core count.
    /// `None` means no such distinction is available (most platforms).
    fn efficiency_core_count(&self) -> Option<usize>;

    /// Biases the *calling thread* toward low-power/background scheduling
    /// at the given strength. Must be called from the thread it should
    /// affect (this is a per-thread hint on platforms that support it,
    /// e.g. macOS QoS classes -- it does not propagate to other threads).
    fn set_current_thread_hint(&self, hint: SchedulingHint);

    /// Lowers this *process's* overall scheduling priority.
    fn lower_process_priority(&self);

    /// Configures a `Command` so the **child process** it spawns also gets
    /// the given scheduling hint. This exists as its own method, separate
    /// from `set_current_thread_hint`, because of a real bug found via
    /// direct measurement (`sudo powermetrics` showed P-cores busy while
    /// E-cores sat idle, even with the calling Rust thread's QoS correctly
    /// lowered): **thread-level QoS does not propagate across `fork`+
    /// `exec` into a new process image.** Almost all of this engine's
    /// actual CPU work happens inside `ffmpeg`/`ffprobe` child processes,
    /// not the orchestrating Rust threads -- so without this, every
    /// scheduling hint in this trait was affecting threads that mostly
    /// just block on `Command::output()`, while the real hot loop ran
    /// under whatever default scheduling a freshly spawned process gets
    /// (P-core-biased). Call before `.output()`/`.spawn()` on every
    /// `Command` that launches ffmpeg or ffprobe.
    fn configure_child_process(&self, cmd: &mut std::process::Command, hint: SchedulingHint);
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos::MacOsScheduler as PlatformImpl;

#[cfg(all(unix, not(target_os = "macos")))]
mod posix;
#[cfg(all(unix, not(target_os = "macos")))]
use posix::PosixScheduler as PlatformImpl;

#[cfg(not(unix))]
mod noop;
#[cfg(not(unix))]
use noop::NoopScheduler as PlatformImpl;

/// The resolved scheduler for this process. Selected once (`OnceLock`),
/// then shared -- every call site uses the same instance through the
/// trait, never the concrete per-platform type.
pub fn platform() -> &'static dyn PlatformScheduler {
    static INSTANCE: OnceLock<PlatformImpl> = OnceLock::new();
    INSTANCE.get_or_init(PlatformImpl::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_resolves_to_the_same_instance_every_call() {
        // Not a pointer-identity check (trait objects from a OnceLock can
        // reasonably have the same address anyway) -- just confirms this
        // doesn't panic and returns something usable from multiple calls,
        // which is the actual "selected once" contract that matters.
        let a = platform().efficiency_core_count();
        let b = platform().efficiency_core_count();
        assert_eq!(a, b);
    }
}
