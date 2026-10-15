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
//! - **Windows (per-user startup)**: a `Lific` value under the user's `Run`
//!   registry key starts a small supervisor that Lific runs itself. See
//!   [`windows`] for why this is not Task Scheduler.
//!
//! Everything that *generates* file content is a pure function of a
//! [`ServicePlan`] so it can be unit-tested without touching systemctl.
//! Process-spawning lives in thin wrappers that the commands share.

use std::path::{Path, PathBuf};
use std::process::Command;

pub mod windows;

/// The service name / launchd label. One service per user by design: Lific's
/// target persona runs a single personal instance.
pub const SYSTEMD_UNIT_NAME: &str = "lific.service";
pub const LAUNCHD_LABEL: &str = "dev.lific";

/// Which service manager runs this platform's user services.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    SystemdUser,
    Launchd,
    WindowsUser,
}

impl Manager {
    pub fn label(&self) -> &'static str {
        match self {
            Manager::SystemdUser => "systemd (user unit)",
            Manager::Launchd => "launchd (LaunchAgent)",
            Manager::WindowsUser => "Windows (per-user startup)",
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
            .map(strip_verbatim)
            .map_err(|e| format!("cannot resolve config path {}: {e}", config.display()))?;
        let exe = strip_verbatim(exe);
        let workdir = config
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| format!("config path {} has no parent directory", config.display()))?
            .to_path_buf();
        reject_control_paths([exe.as_path(), config.as_path(), workdir.as_path()])?;
        Ok(Self {
            exe,
            config,
            workdir,
        })
    }
}

/// `canonicalize` on Windows returns verbatim paths (`\\?\C:\...`). Those
/// are not accepted everywhere a path is (a process's working directory, for
/// one) and they spend characters of the Run entry's 260-character budget, so
/// drop the prefix when the plain form names the same file. No-op elsewhere.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path;
    };
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{share}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

fn reject_control_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Result<(), String> {
    if paths.into_iter().any(path_contains_control) {
        return Err("service paths must not contain control characters".into());
    }
    Ok(())
}

fn path_contains_control(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.to_str().is_none() || path.as_os_str().as_bytes().iter().any(u8::is_ascii_control)
    }
    #[cfg(not(unix))]
    {
        path.to_str()
            .is_none_or(|value| value.chars().any(char::is_control))
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
    if cfg!(windows) {
        // The user's Run key and Lific's own supervisor need nothing that
        // can be missing on a supported Windows.
        return Some(Manager::WindowsUser);
    }
    if cfg!(target_os = "linux") {
        // A user manager needs a session bus; `systemctl --user` failing (or
        // systemctl missing) means we can't manage user units here.
        let ok = Command::new("systemctl")
            .args(["--user", "show", "--property=Version"])
            .output()
            .is_ok_and(|o| o.status.success());
        if ok {
            return Some(Manager::SystemdUser);
        }
    }
    None
}

/// The message shown when [`detect`] finds no service manager. Stated
/// per-platform so the copy never lists managers the running platform cannot
/// have: the Windows per-user startup (whose Run-key entry needs nothing
/// optional) made the old single "systemd on Linux, or launchd on macOS"
/// string stale — a Windows user reading it would conclude Lific does not
/// support them at all. On Windows [`detect`] never returns None, so that
/// branch is defensive only.
pub fn missing_manager_message() -> String {
    if cfg!(windows) {
        "no supported service manager found on Windows (unexpected: the per-user \
         startup entry needs nothing optional — please report this)"
            .to_string()
    } else if cfg!(target_os = "macos") {
        "no supported service manager found (macOS manages user services with \
         launchd, which should always be present — please report this)"
            .to_string()
    } else if cfg!(target_os = "linux") {
        "no supported service manager found (Linux needs a systemd user session: \
         `systemctl --user` must work). Run the server in the foreground with \
         `lific start` instead"
            .to_string()
    } else {
        "no supported service manager found on this platform (supported: systemd \
         user sessions on Linux, launchd on macOS, per-user startup on Windows). \
         Run the server in the foreground with `lific start` instead"
            .to_string()
    }
}

/// The config file of the lific server running on this machine right now,
/// read from its own command line — the live witness. It is exactly what is
/// serving, whatever the port, with or without a service.
///
/// A process is a witness only when it belongs to this user, and — on Linux —
/// its own executable is this same binary. `argv[0]` is never taken on its own:
/// the Linux scan reads the kernel's exe link, and the macOS scan requires the
/// process's uid to match. `None` therefore means "no lific server found", which
/// includes "a process claimed to be lific but is not". The caller falls back to
/// the install-time snapshot ([`installed_config`]) and then to the default
/// candidates.
pub fn running_config() -> Option<PathBuf> {
    // Only a `lific start` process is the server witness: any lific
    // subcommand may carry `--config` (a concurrent `lific doctor` does),
    // and a witness for "the running server" must not pick one up.
    find_own_processes()
        .iter()
        .filter(|args| args.get(1).is_some_and(|sub| sub == "start"))
        .find_map(|args| config_from_cmdline(args))
}

/// The config file the installed background service runs, if a service is
/// installed — the install-time witness. The definition is a snapshot: it is
/// the authority on what was *set up* to run, not on what is running now
/// (the operator may have moved the config since), and it is the only
/// witness left when the server is down — which is when doctor is run most.
pub fn installed_config() -> Option<PathBuf> {
    let mgr = detect()?;
    let path = definition_path(mgr).ok()?;
    let config = match mgr {
        Manager::SystemdUser => config_from_systemd_unit(&std::fs::read_to_string(path).ok()?)?,
        Manager::Launchd => config_from_plist(&std::fs::read_to_string(path).ok()?)?,
        Manager::WindowsUser => {
            let command = windows::installed_run_command().ok()??;
            windows::parse_run_command(&command).map(|(_, config)| config)?
        }
    };
    Some(config)
}

/// Whether a process's own executable is this program. The basename — not the
/// resolved path — is compared, so a dev build and an installed release of the
/// same program both count. A forged `argv[0]` never enters this decision: the
/// exe link is read from the kernel, not from the process's arguments.
//
// Only the Linux scan calls this; the macOS scan gates on uid instead and the
// Windows scan has no live witness. The allow keeps the helper compiled for its
// unit test on every platform.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn exe_is_ours(exe: &Path, me: &Path) -> bool {
    exe.file_name()
        .is_some_and(|name| name == me.file_name().unwrap_or_default())
}

/// Command lines (`argv` vectors) of *other* processes running this same
/// lific binary. The per-OS scan is the platform seam; the `--config` parse
/// ([`config_from_cmdline`]) is shared.
#[cfg(target_os = "linux")]
fn find_own_processes() -> Vec<Vec<String>> {
    let me = match std::env::current_exe() {
        Ok(me) => me,
        Err(_) => return Vec::new(),
    };
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return found;
    };
    for entry in entries.flatten() {
        // Numeric directory names only: one per process. Skip our own pid —
        // `lific doctor` itself is a lific process, but holds no --config.
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.bytes().all(|b| b.is_ascii_digit()) || name == std::process::id().to_string() {
            continue;
        }
        // Trust the process's own executable, never its argv: argv[0] is
        // caller-chosen and this file is world-readable, so a basename match
        // there would let any local user forge a witness. `read_link` on
        // `/proc/<pid>/exe` is subject to the kernel's ptrace permission check,
        // so a process owned by another user is refused outright.
        //
        // A binary replaced while it runs reads " (deleted)" from the exe link
        // and drops out of the witness set. That is deliberate, not a bug:
        // doctor falls back to the installed-definition witness.
        let Ok(exe) = std::fs::read_link(entry.path().join("exe")) else {
            continue;
        };
        if !exe_is_ours(&exe, &me) {
            continue;
        }
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args: Vec<String> = cmdline
            .split(|b| *b == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        if args.len() > 1 {
            found.push(args);
        }
    }
    found
}

/// One `ps -axo uid=,command=` line, parsed into this program's argv when the
/// process belongs to `uid` and its command names this program. Pure, so the
/// uid gate is testable without `ps` and on every platform.
//
// Only the macOS scan calls this; the allow keeps it compiled for its unit test
// elsewhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn ps_witness_line(line: &str, uid: &str, me: Option<&Path>) -> Option<Vec<String>> {
    let (owner, rest) = line.trim_start().split_once(|c: char| c.is_whitespace())?;
    if owner != uid {
        return None;
    }
    let args: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
    let arg0 = args.first()?;
    let is_lific =
        arg0.ends_with("/lific") || me.is_some_and(|me| arg0.as_str() == me.to_string_lossy());
    is_lific.then_some(args)
}

#[cfg(target_os = "macos")]
fn find_own_processes() -> Vec<Vec<String>> {
    // The uid column gates on ownership before argv is even considered: `ps`
    // lists every user's processes, and argv is caller-chosen, so without it
    // any local user could forge a witness. (A same-uid process can still forge
    // one, but a same-uid process already owns this user's keyring.)
    let Ok(output) = Command::new("ps").arg("-axo").arg("uid=,command=").output() else {
        return Vec::new();
    };
    let me = std::env::current_exe().ok();
    // SAFETY: POSIX getuid() always succeeds and sets no errno.
    let uid = unsafe { libc::getuid() }.to_string();
    output
        .stdout
        .split(|b| *b == b'\n')
        .filter_map(|line| {
            ps_witness_line(&String::from_utf8_lossy(line), &uid, me.as_deref())
        })
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn find_own_processes() -> Vec<Vec<String>> {
    // Windows: the Run key ([`installed_config`]) covers every way Lific
    // starts itself there; a live scan would need PowerShell and adds no
    // witness the Run key lacks.
    Vec::new()
}

/// The `--config` argument of a `lific start` command line. Pure so tests
/// exercise the same parse the process scan feeds.
fn config_from_cmdline(args: &[String]) -> Option<PathBuf> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--config" {
            return iter.next().map(PathBuf::from);
        }
        if let Some(value) = arg.strip_prefix("--config=") {
            return Some(PathBuf::from(value));
        }
    }
    None
}

/// The `--config` path in a systemd unit's `ExecStart` line, as
/// [`systemd_unit`] writes it (space-separated, [`systemd_quote`] escaping).
/// Pure: the text format knowledge stays testable without a unit file.
fn config_from_systemd_unit(contents: &str) -> Option<PathBuf> {
    let exec_start = contents
        .lines()
        .find(|line| line.starts_with("ExecStart="))?
        .strip_prefix("ExecStart=")?;
    let mut tokens = systemd_tokens(exec_start);
    while let Some(token) = tokens.next() {
        if token == "--config" {
            return tokens.next().map(PathBuf::from);
        }
        if let Some(value) = token.strip_prefix("--config=") {
            return Some(PathBuf::from(value));
        }
    }
    None
}

/// Split a systemd command line into unquoted tokens, inverting
/// [`systemd_quote`]'s escaping (`\\`, `\"`, `$$`, `%%` inside quotes).
fn systemd_tokens(line: &str) -> impl Iterator<Item = String> + '_ {
    let mut chars = line.chars().peekable();
    std::iter::from_fn(move || {
        // Skip leading whitespace.
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        chars.peek()?;
        let quoted = chars.peek() == Some(&'"');
        if quoted {
            chars.next();
        }
        let mut token = String::new();
        loop {
            match chars.next() {
                None => break,
                Some('"') if quoted => break,
                Some('\\') if quoted => match chars.next() {
                    Some('"') => token.push('"'),
                    Some('\\') => token.push('\\'),
                    // Not an escape we wrote; keep both characters.
                    Some(other) => {
                        token.push('\\');
                        token.push(other);
                    }
                    None => break,
                },
                Some(c) if !quoted && c.is_whitespace() => break,
                Some(c) => token.push(c),
            }
        }
        // Unquote the doubled escapes systemd uses for $ and %.
        let token = token.replace("$$", "$").replace("%%", "%");
        Some(token)
    })
}

/// The `--config` path in a launchd plist's `ProgramArguments`, as
/// [`launchd_plist`] writes it (`--config` string element followed by the
/// xml-escaped path). Pure for the same reason as the unit parser.
fn config_from_plist(contents: &str) -> Option<PathBuf> {
    let mut strings = contents.lines().filter_map(|line| {
        let line = line.trim();
        line.strip_prefix("<string>")?.strip_suffix("</string>")
    });
    while let Some(value) = strings.next() {
        if value == "--config" {
            return strings.next().map(|escaped| {
                PathBuf::from(
                    escaped
                        .replace("&lt;", "<")
                        .replace("&gt;", ">")
                        .replace("&amp;", "&"),
                )
            });
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
    let home =
        dirs::home_dir().ok_or_else(|| "cannot determine your home directory".to_string())?;
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
        // A registry value rather than a file; this is its display form.
        Manager::WindowsUser => PathBuf::from(windows::definition_display()),
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
UMask=0077
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
"#,
        exe = systemd_quote(&plan.exe, true),
        config = systemd_quote(&plan.config, true),
        workdir = systemd_quote(&plan.workdir, false),
    )
}

/// Quote a path for a systemd ExecStart line (double quotes, escape embedded
/// quotes/backslashes). systemd's quoting rules accept this for paths.
fn systemd_quote(p: &Path, escape_dollar: bool) -> String {
    let s = p.display().to_string();
    let mut escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    if escape_dollar {
        escaped = escaped.replace('$', "$$");
    }
    if s.contains(' ') || s.contains('"') || s.contains('\\') {
        format!("\"{escaped}\"")
    } else {
        escaped
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
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The launchd domain every `launchctl` invocation below is addressed to:
/// `gui/<uid>`, the current user's GUI session.
///
/// Like the macOS process scan, this reaches for `libc` only for `getuid()`,
/// and `libc` is declared as a unix-only dependency. The
/// non-unix arm exists so the crate still *typechecks* on Windows, where
/// `detect()` never yields `Manager::Launchd` and none of this can run
/// (reported as GitHub #20: v2.4.0 failed to compile on Windows because five
/// `libc::getuid()` call sites were spread through this module unguarded).
///
/// It returns an error rather than `unreachable!()` because `Manager` is a
/// public enum: a caller can construct `Launchd` on any platform, and a
/// wrong-platform request deserves a message, not a panic.
#[cfg(unix)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the non-unix implementation can fail and callers are platform-neutral"
)]
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
    if manager != Manager::WindowsUser
        && let Some(dir) = path.parent()
    {
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
                .is_ok_and(|s| s.success());
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
        Manager::WindowsUser => {
            windows::install(&plan.exe, &plan.config)?;
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
        Manager::WindowsUser => windows::uninstall()?,
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
    let (installed, active) = match manager {
        Manager::SystemdUser => (
            path.exists(),
            Command::new("systemctl")
                .args(["--user", "is-active", "--quiet", SYSTEMD_UNIT_NAME])
                .status()
                .is_ok_and(|s| s.success()),
        ),
        Manager::Launchd => (
            path.exists(),
            Command::new("launchctl")
                .args(["print", &format!("{}/{LAUNCHD_LABEL}", launchd_domain()?)])
                .output()
                .is_ok_and(|o| o.status.success()),
        ),
        // Running means the supervisor is alive and its server has bound the
        // port, so another program on the port cannot pass for the service.
        Manager::WindowsUser => windows::status()?,
    };
    Ok(StatusReport {
        manager: manager.label().into(),
        installed,
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
        Manager::WindowsUser => windows::stop(),
    }
}

/// Restart (or start, if stopped).
pub fn restart(manager: Manager) -> Result<(), String> {
    match manager {
        Manager::SystemdUser => run_ok("systemctl", &["--user", "restart", SYSTEMD_UNIT_NAME]),
        Manager::Launchd => launchd_bootstrap(&definition_path(manager)?),
        Manager::WindowsUser => windows::restart(),
    }
}

/// The hint we print for reading service logs on this platform.
pub fn logs_hint(manager: Manager) -> String {
    match manager {
        Manager::SystemdUser => format!("journalctl --user -u {SYSTEMD_UNIT_NAME} -f"),
        Manager::Launchd => "tail -f lific.log (in the instance directory)".into(),
        Manager::WindowsUser => "Get-Content -Wait lific.log (in the instance directory)".into(),
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
        assert!(unit.contains("UMask=0077"));
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
    fn systemd_unit_escapes_expansion_characters() {
        let mut p = plan();
        p.exe = PathBuf::from("/home/u/bin/$lific%bin");
        p.config = PathBuf::from("/home/u/config/$env%file.toml");
        p.workdir = PathBuf::from("/home/u/$data%dir");
        let unit = systemd_unit(&p);
        assert!(
            unit.contains("/home/u/bin/$$lific%%bin"),
            "unit was:\n{unit}"
        );
        assert!(
            unit.contains("/home/u/config/$$env%%file.toml"),
            "unit was:\n{unit}"
        );
        assert!(unit.contains("/home/u/$data%%dir"), "unit was:\n{unit}");
    }

    #[test]
    fn config_path_with_control_character_is_rejected() {
        // Validate the path before touching the filesystem: Windows cannot
        // create a filename containing a newline in order to reach the same
        // check through `canonicalize`.
        let config = PathBuf::from(format!("lific_svc_control_{}\n", std::process::id()));
        let error = reject_control_paths([config.as_path()]).unwrap_err();
        assert!(error.contains("control characters"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_service_path_is_rejected() {
        use std::os::unix::ffi::OsStringExt;

        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![
            b'/', b't', b'm', b'p', b'/', 0xff,
        ]));
        assert!(path_contains_control(&path));
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

    // ── Instance discovery: parsing what install wrote back ────

    // The discovery parsers must invert the generators exactly; each test
    // round-trips a generated definition so a future generator change that
    // the parser cannot read fails here, not silently in doctor.

    #[test]
    fn systemd_unit_config_round_trips() {
        let unit = systemd_unit(&plan());
        assert_eq!(
            config_from_systemd_unit(&unit),
            Some(plan().config),
            "unit was:\n{unit}"
        );
    }

    #[test]
    fn systemd_unit_config_round_trips_quoted_and_escaped_paths() {
        let mut p = plan();
        p.config = PathBuf::from("/home/u/my tracker/li\"fic$.toml");
        let unit = systemd_unit(&p);
        assert_eq!(
            config_from_systemd_unit(&unit),
            Some(p.config),
            "unit was:\n{unit}"
        );
    }

    #[test]
    fn systemd_unit_without_config_argument_parses_to_none() {
        assert_eq!(
            config_from_systemd_unit("[Service]\nExecStart=/usr/bin/lific start\n"),
            None
        );
        assert_eq!(config_from_systemd_unit("[Unit]\nDescription=x\n"), None);
    }

    #[test]
    fn launchd_plist_config_round_trips() {
        let plist = launchd_plist(&plan());
        assert_eq!(config_from_plist(&plist), Some(plan().config));
    }

    #[test]
    fn launchd_plist_config_round_trips_xml_escaped_paths() {
        let mut p = plan();
        p.config = PathBuf::from("/home/u/a&b<c>.toml");
        let plist = launchd_plist(&p);
        assert_eq!(config_from_plist(&plist), Some(p.config));
    }

    #[test]
    fn cmdline_config_parses_both_argument_forms() {
        let plain: Vec<String> = ["lific", "start", "--config", "/x/y.toml"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            config_from_cmdline(&plain),
            Some(PathBuf::from("/x/y.toml"))
        );

        let inline: Vec<String> = ["lific", "start", "--config=/x/y.toml"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            config_from_cmdline(&inline),
            Some(PathBuf::from("/x/y.toml"))
        );

        let none: Vec<String> = ["lific", "doctor"].iter().map(|s| s.to_string()).collect();
        assert_eq!(config_from_cmdline(&none), None);
    }

    // LIF-292: the plan must anchor to the config file itself, so `--config
    // /elsewhere/lific.toml` produces a unit that points at /elsewhere — not
    // at a lific.toml in whatever directory the command happened to run in.
    #[test]
    fn for_config_file_anchors_workdir_to_the_config_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let config = dir.join("lific.toml");
        std::fs::write(&config, "").unwrap();

        let plan = ServicePlan::for_config_file(&config).unwrap();
        // The plan drops Windows' verbatim `\\?\` prefix; compare like with like.
        let canon_dir = strip_verbatim(dir.canonicalize().unwrap());
        assert_eq!(plan.workdir, canon_dir);
        assert_eq!(plan.config, canon_dir.join("lific.toml"));
    }

    #[test]
    fn for_config_file_errors_on_missing_config() {
        let err =
            ServicePlan::for_config_file(Path::new("/tmp/nonexistent_lific_dir_12345/lific.toml"))
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
        let windows = definition_path(Manager::WindowsUser).unwrap();
        assert!(windows.display().to_string().starts_with(r"HKCU\"));
    }

    #[test]
    fn verbatim_windows_paths_lose_their_prefix_only_when_safe() {
        for (input, want) in [
            (r"\\?\C:\Users\me\lific.toml", r"C:\Users\me\lific.toml"),
            (
                r"\\?\UNC\server\share\lific.toml",
                r"\\server\share\lific.toml",
            ),
            // A device path has no plain equivalent and stays as it is.
            (r"\\?\Volume{0}\lific.toml", r"\\?\Volume{0}\lific.toml"),
            (r"C:\already\plain.toml", r"C:\already\plain.toml"),
            ("/home/u/lific.toml", "/home/u/lific.toml"),
        ] {
            assert_eq!(strip_verbatim(PathBuf::from(input)), PathBuf::from(want));
        }
    }

    #[test]
    fn detect_matches_the_platform_manager() {
        if cfg!(windows) {
            assert_eq!(detect(), Some(Manager::WindowsUser));
        } else if cfg!(target_os = "macos") {
            assert_eq!(detect(), Some(Manager::Launchd));
        } else {
            assert_ne!(detect(), Some(Manager::WindowsUser));
        }
    }

    /// The no-manager message names the managers that actually apply to the
    /// running platform, and never one that cannot (a Windows reader must not
    /// be told only about systemd and launchd). Also: every variant names the
    /// foreground fallback where one exists.
    #[test]
    fn missing_manager_message_matches_the_platform() {
        let msg = missing_manager_message();
        assert!(msg.starts_with("no supported service manager found"));
        if cfg!(target_os = "linux") {
            assert!(
                msg.contains("systemd"),
                "linux copy must name systemd: {msg}"
            );
            assert!(msg.contains("lific start"));
        } else if cfg!(target_os = "macos") {
            assert!(
                msg.contains("launchd"),
                "macos copy must name launchd: {msg}"
            );
        } else if cfg!(windows) {
            assert!(
                !msg.contains("systemd"),
                "windows copy must not name systemd: {msg}"
            );
        }
    }

    // ── Witness trust: the process's own exe, never its argv ────

    /// A process counts as this program by its executable's basename, never by
    /// its argv: argv[0] is caller-chosen and `/proc/<pid>/cmdline` is
    /// world-readable, so a basename match there proves nothing. The basename
    /// (not the resolved path) keeps a dev build and an installed release of
    /// the same program both visible.
    #[test]
    fn a_process_is_ours_by_its_executable_not_its_argv() {
        let me = Path::new("/usr/local/bin/lific");
        assert!(exe_is_ours(Path::new("/usr/local/bin/lific"), me));
        // A different build of the same program, elsewhere on disk.
        assert!(exe_is_ours(Path::new("/home/u/target/debug/lific"), me));
        // Not this program, whatever the argv claimed.
        assert!(!exe_is_ours(Path::new("/usr/bin/sleep"), me));
        assert!(!exe_is_ours(
            Path::new("/tmp/lific"),
            Path::new("/usr/local/bin/lific2")
        ));
    }

    /// The macOS scan's uid gate: another user's process is refused before its
    /// argv is read, so a forged command name never becomes a witness.
    #[test]
    fn ps_witness_line_gates_on_uid_before_argv() {
        let me = Path::new("/home/u/.cargo/bin/lific");
        let line = "  501 /home/u/.cargo/bin/lific start --config /home/u/lific.toml";

        assert_eq!(
            ps_witness_line(line, "501", Some(me)),
            Some(vec![
                "/home/u/.cargo/bin/lific".to_string(),
                "start".to_string(),
                "--config".to_string(),
                "/home/u/lific.toml".to_string(),
            ])
        );
        // Another user's process, even one named lific, is refused.
        assert_eq!(ps_witness_line(line, "502", Some(me)), None);
        // A same-user process that is not this program is not a witness.
        assert_eq!(
            ps_witness_line("  501 /usr/bin/sleep 10", "501", Some(me)),
            None
        );
        // A blank line yields nothing.
        assert_eq!(ps_witness_line("", "501", Some(me)), None);
    }

    /// A forged `argv[0]` must not become a witness. Under the old
    /// basename-of-argv matching this child was adopted; the exe gate reads
    /// the kernel's own record of the executable, which the child cannot
    /// forge. Cross-uid forgery is the same rule but cannot be staged
    /// in-process (spawning another user needs privilege).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_forged_argv0_is_not_a_witness() {
        use std::os::unix::process::CommandExt;

        let me = std::env::current_exe().expect("test binary path");
        let forged = format!("/tmp/{}", me.file_name().unwrap().to_string_lossy());
        let mut child = Command::new("sleep")
            .arg0(&forged)
            .arg("10")
            .spawn()
            .expect("spawn the forged-argv child");
        // Let it exec so `/proc/<pid>/exe` and `cmdline` are settled.
        std::thread::sleep(std::time::Duration::from_millis(300));

        let found = find_own_processes();
        let _ = child.kill();
        let _ = child.wait();

        assert!(
            !found.iter().any(|args| args.first() == Some(&forged)),
            "a process whose argv[0] was forged must not be a witness; found: {found:?}"
        );
    }
}
