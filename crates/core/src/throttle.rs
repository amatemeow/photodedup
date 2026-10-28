//! How hard the engine is allowed to push the machine. This is not a
//! performance knob first -- it's a product requirement: a desktop app a
//! person runs while doing other things must not become the reason their
//! fans spin up and their laptop gets hot. Observed failures during
//! development, in order, drove this module's design -- not hypotheticals:
//!
//! 1. An unthrottled run used `rayon`'s ambient global pool (every logical
//!    core, default OS priority) and pegged every core on a 12-core
//!    machine.
//! 2. Capping the pool to half the logical cores still ran hot, because
//!    **core count was never the real lever on Apple Silicon** -- which
//!    core *type* the work lands on is. A thread-count cap or
//!    `setpriority()` nice value alone doesn't influence that placement;
//!    only the thread's QoS class does.
//! 3. Setting `QOS_CLASS_UTILITY` on every worker thread *and* every
//!    ffmpeg/ffprobe child process (see `platform.rs`'s doc comment on
//!    why the child-process half of that matters) still left one P-core
//!    cluster at ~75% utilization under real load, measured via `sudo
//!    powermetrics`: `Utility` lowers priority but doesn't hard-confine to
//!    efficiency cores -- under sustained demand the scheduler spills
//!    overflow onto P-cores anyway.
//! 4. `QOS_CLASS_BACKGROUND` measurably held the line (P-cores parked at
//!    0MHz, ~8x lower package power) -- but at a real, substantial
//!    wall-clock cost, confirmed by letting a `Default`-worker-count run
//!    sit at that setting: 25+ minutes of CPU time with no end in sight,
//!    versus ~4 minutes at `Utility`. That tradeoff is too severe to be
//!    the *default* experience, so `Background` and `Utility` are now two
//!    distinct, deliberately-chosen tiers (`SchedulingHint` in
//!    `platform.rs`) rather than one "background or not" toggle -- see
//!    each `ScanIntensity` variant's doc comment for which gets which.
//!
//! All OS-specific scheduling knowledge lives behind `platform::platform()`
//! (a `PlatformScheduler` trait, one implementation per OS, resolved once)
//! -- this module contains no `#[cfg(...)]` of its own; it only decides
//! *how many* workers and *which* hint, then asks the platform layer to
//! carry it out.
//!
//! Tier names (`Efficient`/`Default`/`Performance`) deliberately match the
//! naming convention most apps already use for this exact tradeoff (macOS
//! Low Power/Automatic/High Power, browser "Energy Saver" modes, etc.) --
//! this is meant to be a recognizable choice in a settings UI later (M2+:
//! a persistent option in the app's settings, not a one-off per-scan
//! flag), not a label a user has to learn from scratch.

use crate::platform::{platform, SchedulingHint};
use std::num::NonZeroUsize;

#[derive(Debug, Clone, Copy)]
pub enum ScanIntensity {
    /// The most conservative tier: half of `Default`'s worker count
    /// (minimum 1), `SchedulingHint::Background` applied -- the strongest
    /// available E-core bias. For a user who explicitly wants the scan to
    /// be as unobtrusive as possible and is fine with it taking
    /// substantially longer in exchange (measured ~8x lower CPU package
    /// power than `Default`, at real wall-clock cost -- see module docs).
    Efficient,
    /// Targets the machine's efficiency-core count (Apple Silicon) with
    /// `SchedulingHint::Utility` applied -- lower priority than normal,
    /// but still allowed to use performance cores when it genuinely needs
    /// to, rather than being strictly confined to E-cores. Falls back to
    /// half the logical cores where efficiency-core detection isn't
    /// available. **The default** -- noticeably more polite than no
    /// throttling at all, without `Efficient`'s severe slowdown.
    Default,
    /// All logical cores, normal priority, no scheduling downgrade at
    /// all. For an explicit "scan faster" choice when the user isn't
    /// using the machine for anything else right now.
    Performance,
    /// An explicit worker count, for tests and power-user tuning. No
    /// automatic scheduling hint is applied -- an explicit worker count
    /// is itself an explicit choice, so this tier doesn't layer a second,
    /// implicit one on top.
    Custom(NonZeroUsize),
}

impl Default for ScanIntensity {
    fn default() -> Self {
        ScanIntensity::Default
    }
}

/// Parses the same spellings the CLI's `--mode` flag and the server's scan
/// request accept: `efficient`, `default`, `performance`, or a positive
/// integer worker count.
impl std::str::FromStr for ScanIntensity {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "efficient" => Ok(ScanIntensity::Efficient),
            "default" => Ok(ScanIntensity::Default),
            "performance" => Ok(ScanIntensity::Performance),
            n => n
                .parse::<usize>()
                .ok()
                .and_then(NonZeroUsize::new)
                .map(ScanIntensity::Custom)
                .ok_or_else(|| format!("invalid mode '{n}': expected efficient, default, performance, or a positive integer")),
        }
    }
}

impl ScanIntensity {
    pub fn worker_count(&self) -> usize {
        match self {
            ScanIntensity::Efficient => (default_tier_workers() / 2).max(1),
            ScanIntensity::Default => default_tier_workers(),
            ScanIntensity::Performance => available_parallelism(),
            ScanIntensity::Custom(n) => n.get(),
        }
    }

    /// Which scheduling hint (if any) this tier applies -- to worker
    /// threads via `build_pool`, and to every ffmpeg/ffprobe child process
    /// via `platform().configure_child_process` (called from
    /// `decode.rs`/`metadata.rs`/`video_match.rs`). `None` for
    /// `Performance` (chosen for speed) and `Custom` (an explicit
    /// power-user override).
    pub fn scheduling_hint(&self) -> Option<SchedulingHint> {
        match self {
            ScanIntensity::Efficient => Some(SchedulingHint::Background),
            ScanIntensity::Default => Some(SchedulingHint::Utility),
            ScanIntensity::Performance => None,
            ScanIntensity::Custom(_) => None,
        }
    }
}

fn default_tier_workers() -> usize {
    platform()
        .efficiency_core_count()
        .unwrap_or_else(|| (available_parallelism() / 2).max(1))
}

fn available_parallelism() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

/// Builds a scoped rayon thread pool sized per `ScanIntensity`. Deliberately
/// never the ambient global pool (`rayon::prelude`'s `par_iter` uses that
/// by default, sized to every logical core with zero throttle) -- callers
/// must run parallel work inside `pool.install(|| ...)`.
///
/// When `intensity.scheduling_hint()` is `Some`, every worker thread gets
/// that hint at thread start -- it's per-thread (not inherited from the
/// process), so it has to happen inside the pool itself via rayon's
/// `start_handler`.
pub fn build_pool(intensity: ScanIntensity) -> rayon::ThreadPool {
    let hint = intensity.scheduling_hint();
    rayon::ThreadPoolBuilder::new()
        .num_threads(intensity.worker_count())
        .start_handler(move |_| {
            if let Some(hint) = hint {
                platform().set_current_thread_hint(hint);
            }
        })
        .build()
        .expect("failed to build worker thread pool")
}

/// Lowers this process's overall OS scheduling priority -- complementary
/// to `build_pool`'s per-thread hints, and (on platforms with no QoS-class
/// equivalent, e.g. Linux) one of the only levers available at all. Safe
/// to call regardless of intensity; a process-wide nice value doesn't
/// meaningfully hurt a `Performance` scan and does no harm.
pub fn lower_process_priority() {
    platform().lower_process_priority();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn efficient_is_never_more_than_default() {
        assert!(ScanIntensity::Efficient.worker_count() <= ScanIntensity::Default.worker_count());
    }

    #[test]
    fn default_never_exceeds_performance() {
        assert!(ScanIntensity::Default.worker_count() <= ScanIntensity::Performance.worker_count());
    }

    #[test]
    fn every_tier_is_at_least_one_worker() {
        assert!(ScanIntensity::Efficient.worker_count() >= 1);
        assert!(ScanIntensity::Default.worker_count() >= 1);
        assert!(ScanIntensity::Performance.worker_count() >= 1);
    }

    #[test]
    fn performance_uses_every_logical_core() {
        assert_eq!(ScanIntensity::Performance.worker_count(), available_parallelism());
    }

    #[test]
    fn custom_worker_count_is_respected() {
        let n = NonZeroUsize::new(3).unwrap();
        assert_eq!(ScanIntensity::Custom(n).worker_count(), 3);
    }

    #[test]
    fn default_trait_impl_matches_default_variant() {
        assert_eq!(
            ScanIntensity::default().worker_count(),
            ScanIntensity::Default.worker_count()
        );
    }

    #[test]
    fn each_tier_gets_the_right_hint() {
        assert_eq!(ScanIntensity::Efficient.scheduling_hint(), Some(SchedulingHint::Background));
        assert_eq!(ScanIntensity::Default.scheduling_hint(), Some(SchedulingHint::Utility));
        assert_eq!(ScanIntensity::Performance.scheduling_hint(), None);
        assert_eq!(ScanIntensity::Custom(NonZeroUsize::new(2).unwrap()).scheduling_hint(), None);
    }

    #[test]
    fn built_pool_actually_works() {
        let pool = build_pool(ScanIntensity::Custom(NonZeroUsize::new(2).unwrap()));
        let sum: i32 = pool.install(|| (1..=4).sum());
        assert_eq!(sum, 10);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn default_targets_efficiency_cores_not_half_of_all_cores_on_apple_silicon() {
        // The specific regression this module exists to prevent: a plain
        // "half of logical cores" heuristic would pick ~6 on a 12-core
        // (8P+4E) Mac, which the scheduler still places on P-cores by
        // default. `Default` must come out to the E-core count (4 on the
        // reference machine), not 6 -- and `Efficient` must be half of that.
        if let Some(e_cores) = platform().efficiency_core_count() {
            assert_eq!(ScanIntensity::Default.worker_count(), e_cores);
            assert_eq!(ScanIntensity::Efficient.worker_count(), (e_cores / 2).max(1));
        }
    }
}
