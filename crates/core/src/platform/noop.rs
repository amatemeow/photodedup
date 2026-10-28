//! Windows and any other non-Unix target: no scheduling hints implemented
//! yet. The Windows equivalent (`SetPriorityClass(...,
//! BELOW_NORMAL_PRIORITY_CLASS)`, possibly `THREAD_MODE_BACKGROUND_BEGIN`
//! for the per-thread hint) is a roadmap item -- see `/ROADMAP.md` --
//! deliberately not added speculatively alongside a `windows-sys`
//! dependency with no Windows target to verify it against yet. Every
//! caller goes through the `PlatformScheduler` trait, so filling this in
//! later touches only this one file.

use super::{PlatformScheduler, SchedulingHint};

pub struct NoopScheduler;

impl NoopScheduler {
    pub fn new() -> Self {
        NoopScheduler
    }
}

impl PlatformScheduler for NoopScheduler {
    fn efficiency_core_count(&self) -> Option<usize> {
        None
    }

    fn set_current_thread_hint(&self, _hint: SchedulingHint) {}

    fn lower_process_priority(&self) {}

    fn configure_child_process(&self, _cmd: &mut std::process::Command, _hint: SchedulingHint) {}
}
