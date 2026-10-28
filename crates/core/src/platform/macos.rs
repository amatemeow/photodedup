//! macOS: Apple Silicon's P-core/E-core split via `sysctlbyname`, and
//! per-thread QoS class via `pthread_set_qos_class_self_np`. See
//! `throttle.rs`'s module docs for why QoS (not just a thread-count cap)
//! is the thing that actually determines core-type placement here, and
//! `PlatformScheduler::configure_child_process`'s doc comment for why
//! setting QoS on the Rust thread alone was measured to do nothing for
//! this engine's actual hot path (ffmpeg/ffprobe child processes).
//!
//! `SchedulingHint::Background` -> `QOS_CLASS_BACKGROUND`,
//! `SchedulingHint::Utility` -> `QOS_CLASS_UTILITY`. Both measured via
//! `sudo powermetrics` on the reference 8P+4E machine, not assumed:
//! `Utility` alone left the P-core cluster at ~75% utilization under real
//! pipeline load (it lowers priority, but the scheduler still spills
//! overflow work onto P-cores under sustained demand); `Background`
//! actually held the line (P-cores parked at 0MHz, E-cluster saturated,
//! ~8x lower package power) -- at a real, substantial wall-clock cost,
//! which is why it's reserved for `ScanIntensity::Economy` rather than
//! applied everywhere.

use super::{PlatformScheduler, SchedulingHint};
use std::os::unix::process::CommandExt;
use std::process::Command;

pub struct MacOsScheduler;

impl MacOsScheduler {
    pub fn new() -> Self {
        MacOsScheduler
    }
}

fn qos_class(hint: SchedulingHint) -> libc::qos_class_t {
    match hint {
        SchedulingHint::Background => libc::qos_class_t::QOS_CLASS_BACKGROUND,
        SchedulingHint::Utility => libc::qos_class_t::QOS_CLASS_UTILITY,
    }
}

impl PlatformScheduler for MacOsScheduler {
    fn efficiency_core_count(&self) -> Option<usize> {
        // Darwin convention: perflevel0 = performance cores, perflevel1 =
        // efficiency cores. Returns `None` on Intel Macs (no perflevel1)
        // rather than guessing.
        sysctl_u32("hw.perflevel1.physicalcpu").map(|n| n as usize)
    }

    fn set_current_thread_hint(&self, hint: SchedulingHint) {
        // SAFETY: affects only the calling thread's own scheduling class.
        // A nonzero return (e.g. an overriding QoS already set) is
        // ignored -- best-effort hint, never load-bearing.
        unsafe {
            libc::pthread_set_qos_class_self_np(qos_class(hint), 0);
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
        // `pre_exec` runs in the CHILD after fork(), before exec() --
        // the standard way to carry a scheduling attribute across a
        // boundary that doesn't propagate it by default. QoS class, set
        // here on the not-yet-replaced child process/thread, survives
        // the subsequent exec() (it's a kernel-tracked task/thread policy
        // attribute, not part of the process image exec() replaces) --
        // unlike setting it on the *parent* Rust thread, which has no
        // effect on the child at all.
        //
        // SAFETY: `pre_exec` requires the closure be async-signal-safe
        // (no heap allocation, no locks). `pthread_set_qos_class_self_np`
        // is a direct, allocation-free syscall-backed libSystem call, so
        // this satisfies that constraint. A nonzero return is ignored --
        // same best-effort contract as every other hint in this impl.
        let qc = qos_class(hint);
        unsafe {
            cmd.pre_exec(move || {
                libc::pthread_set_qos_class_self_np(qc, 0);
                Ok(())
            });
        }
    }
}

fn sysctl_u32(name: &str) -> Option<u32> {
    let cname = std::ffi::CString::new(name).ok()?;
    let mut value: i32 = 0;
    let mut size = std::mem::size_of::<i32>();
    // SAFETY: standard sysctlbyname usage -- a correctly-sized output
    // buffer and length, no concurrent access to `value`/`size`.
    let ret = unsafe {
        libc::sysctlbyname(
            cname.as_ptr(),
            &mut value as *mut _ as *mut core::ffi::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (ret == 0 && value > 0).then_some(value as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn efficiency_core_count_matches_sysctl_binary_on_this_machine() {
        // Cross-check against the `sysctl` binary's own output rather
        // than hardcoding an expected number -- portable across whatever
        // Apple Silicon variant (or Intel Mac, where both sides should
        // agree on "none") runs the test.
        let out = std::process::Command::new("sysctl")
            .arg("-n")
            .arg("hw.perflevel1.physicalcpu")
            .output();
        let Ok(out) = out else { return };
        let stdout = String::from_utf8_lossy(&out.stdout);
        let expected = stdout.trim().parse::<usize>().ok();
        assert_eq!(MacOsScheduler::new().efficiency_core_count(), expected);
    }

    #[test]
    fn scheduling_hints_do_not_panic() {
        // These are best-effort OS calls; the only contract to verify
        // here is that calling them is safe and doesn't crash the
        // process, including when run from a test harness thread.
        let s = MacOsScheduler::new();
        s.set_current_thread_hint(SchedulingHint::Background);
        s.set_current_thread_hint(SchedulingHint::Utility);
        s.lower_process_priority();
    }

    #[test]
    fn configured_child_process_still_runs_correctly_for_both_hints() {
        // Doesn't (and can't, without powermetrics) verify *where* the
        // child actually gets scheduled -- but does verify the pre_exec
        // hook doesn't break the exec itself, which a mistake here
        // (e.g. an unsafe pre_exec closure panicking or deadlocking)
        // absolutely could, for both hint variants.
        let s = MacOsScheduler::new();
        for hint in [SchedulingHint::Background, SchedulingHint::Utility] {
            let mut cmd = Command::new("true");
            s.configure_child_process(&mut cmd, hint);
            let status = cmd.status().expect("child process failed to run");
            assert!(status.success());
        }
    }
}
