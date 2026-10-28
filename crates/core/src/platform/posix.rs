//! Non-macOS Unix (primarily Linux). No OS-exposed P-core/E-core
//! distinction or QoS-class equivalent at this API level (some ARM Linux
//! SoCs have big.LITTLE cores, but there's no portable libc call for it --
//! that would need parsing `/sys/devices/system/cpu/cpu*/cpufreq/` or
//! similar, which is a real roadmap item, not implemented here yet; see
//! `/ROADMAP.md`). `setpriority()`/nice value is the one lever this impl
//! has, so `SchedulingHint::Background` vs `Utility` map to different nice
//! values rather than different mechanisms entirely -- a weaker
//! distinction than macOS's QoS classes, but still a real, honest one.

use super::{PlatformScheduler, SchedulingHint};
use std::os::unix::process::CommandExt;
use std::process::Command;

pub struct PosixScheduler;

impl PosixScheduler {
    pub fn new() -> Self {
        PosixScheduler
    }
}

fn nice_value(hint: SchedulingHint) -> i32 {
    match hint {
        // 19 is the maximum (lowest-priority) nice value on Linux --
        // mirrors macOS's QOS_CLASS_BACKGROUND being the strongest
        // available deprioritization, not an arbitrary number.
        SchedulingHint::Background => 19,
        SchedulingHint::Utility => 10,
    }
}

impl PlatformScheduler for PosixScheduler {
    fn efficiency_core_count(&self) -> Option<usize> {
        None // see module docs -- a real big.LITTLE detector is a roadmap item
    }

    fn set_current_thread_hint(&self, hint: SchedulingHint) {
        // No per-thread QoS-equivalent lever on plain POSIX; the nice
        // value is process-wide, so this sets the same thing
        // `lower_process_priority` does, just at the requested strength.
        //
        // SAFETY: setpriority(PRIO_PROCESS, 0, _) is a pure scheduling
        // hint affecting the calling process ("pid 0" = self).
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, nice_value(hint));
        }
    }

    fn lower_process_priority(&self) {
        // SAFETY: setpriority(PRIO_PROCESS, 0, _) is a pure scheduling
        // hint affecting the calling process ("pid 0" = self). A failure
        // (e.g. a sandbox denying it) is intentionally ignored.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, 10);
        }
    }

    fn configure_child_process(&self, cmd: &mut Command, hint: SchedulingHint) {
        // A nice value set on the parent is in fact inherited across
        // fork() on Linux (unlike macOS QoS, which is why that platform
        // needs this method at all -- see the trait's doc comment).
        // Setting it again explicitly here, in the child via `pre_exec`,
        // costs nothing and removes any doubt rather than relying on
        // implicit inheritance.
        //
        // SAFETY: `pre_exec` requires an async-signal-safe closure;
        // `setpriority` is a direct syscall, no allocation. A nonzero
        // return is ignored -- best-effort, matching every other hint.
        let n = nice_value(hint);
        unsafe {
            cmd.pre_exec(move || {
                libc::setpriority(libc::PRIO_PROCESS, 0, n);
                Ok(())
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheduling_hints_do_not_panic() {
        let s = PosixScheduler::new();
        s.set_current_thread_hint(SchedulingHint::Background);
        s.set_current_thread_hint(SchedulingHint::Utility);
        s.lower_process_priority();
        assert_eq!(s.efficiency_core_count(), None);
    }

    #[test]
    fn configured_child_process_still_runs_correctly() {
        let s = PosixScheduler::new();
        let mut cmd = Command::new("true");
        s.configure_child_process(&mut cmd, SchedulingHint::Background);
        let status = cmd.status().expect("child process failed to run");
        assert!(status.success());
    }

    #[test]
    fn background_is_a_higher_nice_value_than_utility() {
        // The one thing we can actually assert about the distinction on
        // this platform: Background must be *more* deprioritized.
        assert!(nice_value(SchedulingHint::Background) > nice_value(SchedulingHint::Utility));
    }
}
