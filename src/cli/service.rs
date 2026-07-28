//! Background-service management: run the Lific server under the OS service
//! manager so it survives terminal close, logout, and reboot.
//!
//! `lific init` calls [`install`] automatically (the README's 60-second setup
//! must end with a server that's still alive tomorrow), and `lific service
//! <install|uninstall|status|stop|restart>` exposes the same machinery for
//! management.
//!
//! Two managers are supported, detected at runtime:
//!
//! - **systemd (user unit)** on Linux: `~/.config/systemd/user/lific.service`,
//!   enabled via `systemctl --user enable --now`. `loginctl enable-linger` is
//!   attempted (best-effort) so the unit keeps running after logout.
//! - **launchd (LaunchAgent)** on macOS: `~/Library/LaunchAgents/dev.lific.plist`,
//!   loaded via `launchctl bootstrap gui/<uid>` (falling back to the legacy
//!   `launchctl load -w` on older systems).
//!
//! Everything that *generates* file content is a pure function of a
//! [`ServicePlan`] so it can be unit-tested without touching systemctl.
//! Process-spawning lives in thin wrappers that the commands share.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The service name / launchd label. One service per user by design: Lific's
/// target persona runs a single personal instance.
pub const SYSTEMD_UNIT_NAME: &str = "lific.service";
pub const LAUNCHD_LABEL: &str = "dev.lific";

/// Which service manager runs this platform's user services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    SystemdUser,
    Launchd,
}

impl Manager {
    pub fn label(&self) -> &'static str {
        match self {
            Manager::SystemdUser => "systemd (user unit)",
            Manager::Launchd => "launchd (LaunchAgent)",
        }
    }
}

/// Everything needed to render a service definition. All paths must be
/// absolute — service managers do not inherit the caller's cwd.
#[derive(Debug, Clone)]
pub struct ServicePlan {
    /// Absolute path to the lific binary to exec.
    pub exe: PathBuf,
    /// Absolute path to the lific.toml this instance runs from.
    pub config: PathBuf,
    /// Absolute working directory (relative `database.path`/`backup.dir`
    /// values in the config resolve against this).
    pub workdir: PathBuf,
}

impl ServicePlan {
    /// Build a plan anchored at the config file, using the currently running
    /// binary as the exec target. The service's working directory is the
    /// config file's own directory, so a relative `database.path` resolves
    /// beside the lific.toml the unit points at — no matter where `lific
    /// init` / `lific service install` ran from (LIF-292: the old
    /// caller-supplied-workdir form let `--config` be silently ignored and
    /// baked `./lific.toml` from the invocation cwd into the unit).
    pub fn for_config_file(config: &Path) -> Result<Self, String> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("cannot resolve the lific binary path: {e}"))?;
        let config = config
            .canonicalize()
            .map_err(|e| format!("cannot resolve config path {}: {e}", config.display()))?;
        let workdir = config
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| {
                format!(
                    "config path {} has no parent directory",
                    config.display()
                )
            })?
            .to_path_buf();
        Ok(Self {
            exe,
            config,
            workdir,
        })
    }
}

/// Detect the available service manager. Returns None in environments without
/// one (containers, WSL without systemd, unusual inits) — callers fall back to
/// foreground `lific start`.
pub fn detect() -> Option<Manager> {
    if cfg!(target_os = "macos") {
        // launchd is the init on every supported macOS.
        return Some(Manager::Launchd);
    }
    if cfg!(target_os = "linux") {
        // A user manager needs a session bus; `systemctl --user` failing (or
        // systemctl missing) means we can't manage user units here.
        let ok = Command::new("systemctl")
            .args(["--user", "show", "--property=Version"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            return Some(Manager::SystemdUser);
        }
    }
    None
}

/// Where the service definition file lives for this user.
pub fn definition_path(manager: Manager) -> Result<PathBuf, String> {
    // `dirs::home_dir()` rather than a raw `HOME` read: it is what the rest of
    // the crate uses (config.rs, credentials.rs, connect), it falls back to the
    // passwd entry when `HOME` is unset (which happens in some service and cron
    // contexts, where "HOME is not set" was a confusing way to fail), and it
    // resolves on Windows too, where a raw `HOME` read always came up empty.
    let home = dirs::home_dir().ok_or_else(|| "cannot determine your home directory".to_string())?;
    Ok(match manager {
        Manager::SystemdUser => home
            .join(".config")
            .join("systemd")
            .join("user")
            .join(SYSTEMD_UNIT_NAME),
        Manager::Launchd => home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist")),
    })
}

/// Render the systemd user unit. Paths are systemd-quoted so spaces survive.
pub fn systemd_unit(plan: &ServicePlan) -> String {
    format!(
        r#"# Managed by `lific init` / `lific service install`.
[Unit]
Description=Lific issue tracker
After=network.target

[Service]
ExecStart={exe} start --config {config}
WorkingDirectory={workdir}
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
"#,
        exe = systemd_quote(&plan.exe),
        config = systemd_quote(&plan.config),
        workdir = plan.workdir.display(),
    )
}

/// Quote a path for a systemd ExecStart line (double quotes, escape embedded
/// quotes/backslashes). systemd's quoting rules accept this for paths.
fn systemd_quote(p: &Path) -> String {
    let s = p.display().to_string();
    if s.contains(' ') || s.contains('"') || s.contains('\\') {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s
    }
}

/// Render the launchd LaunchAgent plist.
pub fn launchd_plist(plan: &ServicePlan) -> String {
    let log = plan.workdir.join("lific.log");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>start</string>
        <string>--config</string>
        <string>{config}</string>
    </array>
    <key>WorkingDirectory</key>
    <string>{workdir}</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
</dict>
</plist>
"#,
        label = LAUNCHD_LABEL,
        exe = xml_escape(&plan.exe.display().to_string()),
        config = xml_escape(&plan.config.display().to_string()),
        workdir = xml_escape(&plan.workdir.display().to_string()),
        log = xml_escape(&log.display().to_string()),
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The launchd domain every `launchctl` invocation below is addressed to:
/// `gui/<uid>`, the current user's GUI session.
///
/// This is the crate's only use of `libc` outside the `#[cfg(unix)]` SIGPIPE
/// setup in `main.rs`, and `libc` is declared as a unix-only dependency. The
/// non-unix arm exists so the crate still *typechecks* on Windows, where
/// `detect()` never yields `Manager::Launchd` and none of this can run
/// (reported as GitHub #20: v2.4.0 failed to compile on Windows because five
/// `libc::getuid()` call sites were spread through this module unguarded).
///
/// It returns an error rather than `unreachable!()` because `Manager` is a
/// public enum: a caller can construct `Launchd` on any platform, and a
/// wrong-platform request deserves a message, not a panic.
#[cfg(unix)]
fn launchd_domain() -> Result<String, String> {
    // POSIX guarantees getuid() always succeeds and sets no errno.
    Ok(format!("gui/{}", unsafe { libc::getuid() }))
}

#[cfg(not(unix))]
fn launchd_domain() -> Result<String, String> {
    Err("launchd services are only available on macOS".to_string())
}

/// Load (or reload) the LaunchAgent at `path` into the user's GUI domain.
///
/// Shared by [`install`] and [`restart`], which perform an identical dance:
/// boot out any already-loaded copy, bootstrap the new one, and fall back to
/// the legacy `launchctl load -w` on macOS versions without `bootstrap`.
fn launchd_bootstrap(path: &Path) -> Result<(), String> {
    let domain = launchd_domain()?;
    let _ = Command::new("launchctl")
        .args(["bootout", &format!("{domain}/{LAUNCHD_LABEL}")])
        .output();
    let boot = Command::new("launchctl")
        .args(["bootstrap", &domain, &path.display().to_string()])
        .output()
        .map_err(|e| format!("launchctl failed to run: {e}"))?;
    if !boot.status.success() {
        run_ok("launchctl", &["load", "-w", &path.display().to_string()])?;
    }
    Ok(())
}

/// Outcome of an install, for both human and JSON output.
#[derive(Debug, serde::Serialize)]
pub struct InstallReport {
    pub manager: String,
    pub definition: String,
    pub enabled: bool,
    /// Linux only: whether lingering was enabled so the service survives
    /// logout. None when not applicable/attempted.
    pub linger: Option<bool>,
}

/// Write the service definition and start it now + on boot.
pub fn install(manager: Manager, plan: &ServicePlan) -> Result<InstallReport, String> {
    let path = definition_path(manager)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    match manager {
        Manager::SystemdUser => {
            std::fs::write(&path, systemd_unit(plan))
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            run_ok("systemctl", &["--user", "daemon-reload"])?;
            run_ok(
                "systemctl",
                &["--user", "enable", "--now", SYSTEMD_UNIT_NAME],
            )?;
            // Best-effort: without lingering, user units stop at logout, which
            // defeats the point on a personal box. May prompt for auth or be
            // refused; that's fine — report, don't fail.
            let linger = Command::new("loginctl")
                .arg("enable-linger")
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            Ok(InstallReport {
                manager: manager.label().into(),
                definition: path.display().to_string(),
                enabled: true,
                linger: Some(linger),
            })
        }
        Manager::Launchd => {
            std::fs::write(&path, launchd_plist(plan))
                .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
            launchd_bootstrap(&path)?;
            Ok(InstallReport {
                manager: manager.label().into(),
                definition: path.display().to_string(),
                enabled: true,
                linger: None,
            })
        }
    }
}

/// Stop the service and remove its definition.
pub fn uninstall(manager: Manager) -> Result<String, String> {
    let path = definition_path(manager)?;
    match manager {
        Manager::SystemdUser => {
            // disable --now both stops and removes boot wiring; ignore failure
            // (unit may already be gone) but surface file-removal errors.
            let _ = Command::new("systemctl")
                .args(["--user", "disable", "--now", SYSTEMD_UNIT_NAME])
                .output();
            if path.exists() {
                std::fs::remove_file(&path)
                    .map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
            }
            let _ = Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .output();
        }
        Manager::Launchd => {
            let domain = launchd_domain()?;
            let _ = Command::new("launchctl")
                .args(["bootout", &format!("{domain}/{LAUNCHD_LABEL}")])
                .output();
            if path.exists() {
                std::fs::remove_file(&path)
                    .map_err(|e| format!("cannot remove {}: {e}", path.display()))?;
            }
        }
    }
    Ok(path.display().to_string())
}

#[derive(Debug, serde::Serialize)]
pub struct StatusReport {
    pub manager: String,
    pub installed: bool,
    pub active: bool,
    pub definition: String,
}

/// Is the service installed and currently running?
pub fn status(manager: Manager) -> Result<StatusReport, String> {
    let path = definition_path(manager)?;
    let active = match manager {
        Manager::SystemdUser => Command::new("systemctl")
            .args(["--user", "is-active", "--quiet", SYSTEMD_UNIT_NAME])
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        Manager::Launchd => Command::new("launchctl")
            .args(["print", &format!("{}/{LAUNCHD_LABEL}", launchd_domain()?)])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false),
    };
    Ok(StatusReport {
        manager: manager.label().into(),
        installed: path.exists(),
        active,
        definition: path.display().to_string(),
    })
}

/// Stop (without uninstalling).
pub fn stop(manager: Manager) -> Result<(), String> {
    match manager {
        Manager::SystemdUser => run_ok("systemctl", &["--user", "stop", SYSTEMD_UNIT_NAME]),
        Manager::Launchd => {
            let domain = launchd_domain()?;
            run_ok(
                "launchctl",
                &["bootout", &format!("{domain}/{LAUNCHD_LABEL}")],
            )
        }
    }
}

/// Restart (or start, if stopped).
pub fn restart(manager: Manager) -> Result<(), String> {
    match manager {
        Manager::SystemdUser => run_ok("systemctl", &["--user", "restart", SYSTEMD_UNIT_NAME]),
        Manager::Launchd => launchd_bootstrap(&definition_path(manager)?),
    }
}

/// The hint we print for reading service logs on this platform.
pub fn logs_hint(manager: Manager) -> String {
    match manager {
        Manager::SystemdUser => format!("journalctl --user -u {SYSTEMD_UNIT_NAME} -f"),
        Manager::Launchd => "tail -f lific.log (in the instance directory)".into(),
    }
}

fn run_ok(cmd: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .map_err(|e| format!("{cmd} failed to run: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "`{cmd} {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> ServicePlan {
        ServicePlan {
            exe: PathBuf::from("/home/u/.cargo/bin/lific"),
            config: PathBuf::from("/home/u/tracker/lific.toml"),
            workdir: PathBuf::from("/home/u/tracker"),
        }
    }

    #[test]
    fn systemd_unit_execs_start_with_absolute_config() {
        let unit = systemd_unit(&plan());
        assert!(
            unit.contains(
                "ExecStart=/home/u/.cargo/bin/lific start --config /home/u/tracker/lific.toml"
            ),
            "unit was:\n{unit}"
        );
        assert!(unit.contains("WorkingDirectory=/home/u/tracker"));
        assert!(unit.contains("WantedBy=default.target"));
        assert!(unit.contains("Restart=on-failure"));
    }

    #[test]
    fn systemd_unit_quotes_paths_with_spaces() {
        let mut p = plan();
        p.exe = PathBuf::from("/home/u/my tools/lific");
        p.config = PathBuf::from("/home/u/my tracker/lific.toml");
        let unit = systemd_unit(&p);
        assert!(
            unit.contains(
                r#"ExecStart="/home/u/my tools/lific" start --config "/home/u/my tracker/lific.toml""#
            ),
            "unit was:\n{unit}"
        );
    }

    #[test]
    fn launchd_plist_contains_program_arguments_and_label() {
        let plist = launchd_plist(&plan());
        assert!(plist.contains("<string>dev.lific</string>"));
        assert!(plist.contains("<string>/home/u/.cargo/bin/lific</string>"));
        assert!(plist.contains("<string>start</string>"));
        assert!(plist.contains("<string>--config</string>"));
        assert!(plist.contains("<string>/home/u/tracker/lific.toml</string>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains("<key>WorkingDirectory</key>"));
    }

    #[test]
    fn launchd_plist_escapes_xml_special_chars() {
        let mut p = plan();
        p.workdir = PathBuf::from("/home/u/a&b");
        let plist = launchd_plist(&p);
        assert!(plist.contains("/home/u/a&amp;b"));
        assert!(!plist.contains("<string>/home/u/a&b</string>"));
    }

    // LIF-292: the plan must anchor to the config file itself, so `--config
    // /elsewhere/lific.toml` produces a unit that points at /elsewhere — not
    // at a lific.toml in whatever directory the command happened to run in.
    #[test]
    fn for_config_file_anchors_workdir_to_the_config_dir() {
        let dir = std::env::temp_dir().join(format!("lific_svc_plan_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("lific.toml");
        std::fs::write(&config, "").unwrap();

        let plan = ServicePlan::for_config_file(&config).unwrap();
        let canon_dir = dir.canonicalize().unwrap();
        assert_eq!(plan.workdir, canon_dir);
        assert_eq!(plan.config, canon_dir.join("lific.toml"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn for_config_file_errors_on_missing_config() {
        let err = ServicePlan::for_config_file(Path::new(
            "/tmp/nonexistent_lific_dir_12345/lific.toml",
        ))
        .unwrap_err();
        assert!(err.contains("cannot resolve config path"), "got: {err}");
    }

    // The five launchctl call sites used to inline `gui/{uid}` themselves; they
    // now share this one helper, so its shape is what keeps `launchctl` able to
    // address the agent. (The non-unix arm is what makes the crate compile on
    // Windows at all - GitHub #20 - and is covered by CI's windows job.)
    #[cfg(unix)]
    #[test]
    fn launchd_domain_addresses_the_current_users_gui_session() {
        let domain = launchd_domain().unwrap();
        let uid = domain
            .strip_prefix("gui/")
            .unwrap_or_else(|| panic!("expected gui/<uid>, got: {domain}"));
        assert!(
            !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()),
            "expected gui/<uid>, got: {domain}"
        );
    }

    #[test]
    fn definition_paths_are_per_user() {
        // HOME is set in test environments; just sanity-check the shape.
        let systemd = definition_path(Manager::SystemdUser).unwrap();
        assert!(systemd.ends_with(".config/systemd/user/lific.service"));
        let launchd = definition_path(Manager::Launchd).unwrap();
        assert!(launchd.ends_with("Library/LaunchAgents/dev.lific.plist"));
    }
}
