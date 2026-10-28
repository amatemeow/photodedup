//! Finding ffmpeg and ffprobe, and checking they are usable.
//!
//! The engine runs both as subprocesses (HEIC photos and video frames need
//! them). A packaged app can't rely on `PATH`, so they are looked for, in
//! order: an explicit override in the environment, next to the running
//! executable (where the app bundle puts its sidecar copies), then `PATH`.
//!
//! This module also holds the licence gate for anything we *ship*: a
//! redistributed ffmpeg must be an LGPL build (no `--enable-gpl`, no
//! `--enable-nonfree`). It is only a gate for bundled binaries; a user's own
//! ffmpeg on their own machine is their business.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;

const EXE_SUFFIX: &str = std::env::consts::EXE_SUFFIX;

/// Where to find `name` (`ffmpeg` or `ffprobe`), as something that can be
/// handed to `Command::new`. Falls back to the bare name, which means "look
/// on `PATH`" and fails later, visibly, if it isn't there.
pub fn resolve(name: &str) -> String {
    let env_var = format!("PHOTODEDUP_{}", name.to_uppercase());
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    resolve_in(name, std::env::var_os(&env_var).map(PathBuf::from), exe_dir.as_deref())
}

/// `resolve` with its two inputs passed in, so the order is testable.
pub fn resolve_in(name: &str, env_override: Option<PathBuf>, exe_dir: Option<&Path>) -> String {
    if let Some(p) = env_override.filter(|p| p.is_file()) {
        return p.to_string_lossy().into_owned();
    }
    if let Some(dir) = exe_dir {
        let beside = dir.join(format!("{name}{EXE_SUFFIX}"));
        if beside.is_file() {
            return beside.to_string_lossy().into_owned();
        }
    }
    name.to_string()
}

pub fn ffmpeg() -> String {
    resolve("ffmpeg")
}

pub fn ffprobe() -> String {
    resolve("ffprobe")
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolStatus {
    pub name: &'static str,
    pub found: bool,
    /// The first line of `-version`, e.g. `ffmpeg version 7.1`.
    pub version: Option<String>,
}

/// Runs `<tool> -version` to see whether it exists and works.
pub fn check(name: &'static str) -> ToolStatus {
    let out = Command::new(resolve(name)).arg("-version").output();
    match out {
        Ok(o) if o.status.success() => ToolStatus {
            name,
            found: true,
            version: String::from_utf8_lossy(&o.stdout).lines().next().map(str::to_string),
        },
        _ => ToolStatus { name, found: false, version: None },
    }
}

/// The reason a binary may NOT be redistributed, or `None` if its build
/// configuration is LGPL-compatible. `-version` prints a `configuration:`
/// line listing the flags it was built with.
pub fn redistribution_problem(version_output: &str) -> Option<String> {
    let config = version_output.lines().find(|l| l.trim_start().starts_with("configuration:"));
    let Some(config) = config else {
        return Some("no build configuration reported, so its licence can't be checked".into());
    };
    let flags: Vec<&str> = config.split_whitespace().collect();
    // `--enable-version3` alone only makes it LGPLv3, which is fine; GPL
    // components and non-free ones are the blockers.
    ["--enable-gpl", "--enable-nonfree"].into_iter().find(|bad| flags.contains(bad)).map(|bad| format!("built with {bad}"))
}

/// Runs a specific binary's `-version` and applies `redistribution_problem`.
pub fn check_redistributable(binary: &Path) -> Result<(), String> {
    let out = Command::new(binary).arg("-version").output().map_err(|e| format!("could not run {}: {e}", binary.display()))?;
    if !out.status.success() {
        return Err(format!("{} -version failed", binary.display()));
    }
    match redistribution_problem(&String::from_utf8_lossy(&out.stdout)) {
        None => Ok(()),
        Some(why) => Err(format!("{}: {why}", binary.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_override_wins_then_the_folder_beside_the_app_then_path() {
        let dir = tempfile::tempdir().unwrap();
        let beside = dir.path().join(format!("ffmpeg{EXE_SUFFIX}"));
        let custom = dir.path().join("my-ffmpeg");
        std::fs::write(&beside, b"x").unwrap();
        std::fs::write(&custom, b"x").unwrap();

        assert_eq!(resolve_in("ffmpeg", Some(custom.clone()), Some(dir.path())), custom.to_string_lossy());
        assert_eq!(resolve_in("ffmpeg", None, Some(dir.path())), beside.to_string_lossy());
        assert_eq!(resolve_in("ffprobe", None, Some(dir.path())), "ffprobe", "nothing beside the app: fall back to PATH");
        assert_eq!(resolve_in("ffmpeg", None, None), "ffmpeg");
    }

    #[test]
    fn an_override_pointing_at_nothing_is_ignored_not_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope");
        assert_eq!(resolve_in("ffmpeg", Some(missing), Some(dir.path())), "ffmpeg");
    }

    #[test]
    fn a_missing_tool_is_reported_as_not_found_rather_than_crashing() {
        let status = check("photodedup-no-such-tool");
        assert!(!status.found && status.version.is_none());
        assert!(check_redistributable(Path::new("/definitely/not/here/ffmpeg")).is_err());
    }

    const LGPL: &str = "ffmpeg version 7.1\nbuilt with clang\nconfiguration: --prefix=/x --enable-version3 --disable-gpl --enable-shared\n";
    const GPL: &str = "ffmpeg version 9.0.2\nconfiguration: --prefix=/x --enable-version3 --enable-gpl --enable-libx264\n";
    const NONFREE: &str = "ffmpeg version 7\nconfiguration: --enable-nonfree --enable-libfdk-aac\n";

    #[test]
    fn only_lgpl_builds_may_be_redistributed() {
        assert_eq!(redistribution_problem(LGPL), None);
        assert!(redistribution_problem(GPL).unwrap().contains("--enable-gpl"));
        assert!(redistribution_problem(NONFREE).unwrap().contains("--enable-nonfree"));
    }

    #[test]
    fn a_build_that_does_not_say_how_it_was_configured_is_refused() {
        assert!(redistribution_problem("ffmpeg version 7\nnothing useful here\n").is_some());
        assert!(redistribution_problem("").is_some());
    }

    #[test]
    fn disabling_gpl_is_not_mistaken_for_enabling_it() {
        assert_eq!(redistribution_problem("configuration: --disable-gpl\n"), None);
    }
}
