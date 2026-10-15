//! The Windows background service.
//!
//! Windows has no per-user service manager that can also supervise. Task
//! Scheduler can, but Group Policy commonly forbids standard users from
//! creating tasks, and an interactive task still opens a console window. A
//! real Windows service needs administrator rights. So the job is split:
//!
//! - **Starting at logon** is a `Lific` value under the per-user
//!   `HKCU\...\CurrentVersion\Run` key, written with the registry API (no
//!   admin, no COM, no PowerShell, no reg.exe). It shows up in Task Manager's
//!   Startup apps, where the user can see and disable it. The value runs
//!   `lific service run`, which starts the supervisor and exits at once.
//! - **Staying up** is Lific's own supervisor (`lific service supervise`). It
//!   owns a named mutex for its whole life (the single source of truth for
//!   "running"), keeps `lific start` alive with a restart budget, puts it in a
//!   kill-on-close job so a dead supervisor cannot orphan the server, and
//!   appends both processes' output to `lific.log` in the instance directory.
//!
//! The supervised server is told where to signal readiness and where to wait
//! for a stop request through two environment variables. A `lific start` run
//! by hand has neither, so stopping the service never touches it.
//!
//! The binary's manifest (build.rs) sets `consoleAllocationPolicy=detached`,
//! so on Windows 11 24H2 and later no console window appears at logon. Older
//! Windows briefly shows one while `service run` hands off.

// The pure helpers below are only called from Windows code outside tests.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The per-user autostart key and the value Lific owns in it.
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const RUN_VALUE: &str = "Lific";

/// Windows documents a 260-character limit for Run and RunOnce commands.
pub const MAX_RUN_COMMAND: usize = 260;

/// Set by the supervisor on the `lific start` it runs, naming the event the
/// server waits on for a graceful stop and the event it sets once listening.
pub const STOP_EVENT_ENV: &str = "LIFIC_SERVICE_STOP_EVENT";
pub const READY_EVENT_ENV: &str = "LIFIC_SERVICE_READY_EVENT";

pub const LOG_FILE: &str = "lific.log";
const LOG_ROTATE_BYTES: u64 = 10 * 1024 * 1024;

/// Restart a crashing server at most this many times per window, then give
/// up and say so in the log (systemd's StartLimitBurst, in miniature).
const RESTART_LIMIT: usize = 5;
const RESTART_WINDOW: Duration = Duration::from_secs(60);
const RESTART_DELAY: Duration = Duration::from_secs(2);
/// How long a stop request waits for a graceful shutdown before killing.
const GRACEFUL_STOP: Duration = Duration::from_secs(10);
/// How long `stop` waits for the supervisor itself to be gone.
const STOP_TIMEOUT: Duration = Duration::from_secs(15);
/// How long `install`/`restart` wait for a new supervisor to take its lock.
const START_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(not(windows))]
const NOT_WINDOWS: &str = "the Windows startup service is only available on Windows";

/// Human-readable location of the startup entry, used as the "definition".
pub fn definition_display() -> String {
    format!(r"HKCU\{RUN_KEY}\{RUN_VALUE}")
}

/// Render the Run value's command line. Windows paths cannot contain `"`, so
/// plain quoting is exact and [`parse_run_command`] can invert it.
pub fn run_command(exe: &Path, config: &Path) -> Result<String, String> {
    let exe = exe.display().to_string();
    let config = config.display().to_string();
    if exe.contains('"') || config.contains('"') {
        return Err("service paths must not contain double quotes".into());
    }
    let command = format!("\"{exe}\" --config \"{config}\" service run");
    let length = command.chars().count();
    if length > MAX_RUN_COMMAND {
        return Err(format!(
            "the startup command would be {length} characters, and Windows runs at most \
             {MAX_RUN_COMMAND} from the Run key. Move lific.exe or the instance to a shorter path."
        ));
    }
    Ok(command)
}

/// Recover `(exe, config)` from a command written by [`run_command`]. Anything
/// else (a hand-edited or foreign value) is `None`.
pub fn parse_run_command(command: &str) -> Option<(PathBuf, PathBuf)> {
    let rest = command.strip_prefix('"')?;
    let (exe, rest) = rest.split_once('"')?;
    let rest = rest.strip_prefix(" --config \"")?;
    let (config, rest) = rest.split_once('"')?;
    (rest == " service run" && !exe.is_empty() && !config.is_empty())
        .then(|| (PathBuf::from(exe), PathBuf::from(config)))
}

/// Sliding-window limit on server starts.
#[derive(Debug)]
pub struct RestartBudget {
    starts: VecDeque<Instant>,
    limit: usize,
    window: Duration,
}

impl RestartBudget {
    pub fn new(limit: usize, window: Duration) -> Self {
        Self {
            starts: VecDeque::with_capacity(limit),
            limit,
            window,
        }
    }

    /// Record a start at `now` if the budget allows one.
    pub fn allow(&mut self, now: Instant) -> bool {
        while self
            .starts
            .front()
            .is_some_and(|&start| now.saturating_duration_since(start) >= self.window)
        {
            self.starts.pop_front();
        }
        if self.starts.len() >= self.limit {
            return false;
        }
        self.starts.push_back(now);
        true
    }
}

/// Kernel object names for one service instance. `Local\` is the logon
/// session's namespace: no privilege is needed, and another user's session
/// cannot see or squat the names.
#[derive(Debug, Clone)]
pub struct Names {
    pub mutex: String,
    pub stop: String,
    pub ready: String,
    pub control: String,
}

impl Names {
    pub fn service() -> Self {
        Self::with_base("dev.lific.service")
    }

    pub fn with_base(base: &str) -> Self {
        Self {
            mutex: format!(r"Local\{base}"),
            stop: format!(r"Local\{base}.stop"),
            ready: format!(r"Local\{base}.ready"),
            control: format!(r"Local\{base}.control"),
        }
    }
}

/// How a supervisor run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Another supervisor already holds the lock; this one did nothing.
    AlreadyRunning,
    /// A stop request arrived.
    Stopped,
    /// The server exited with status 0, which is not a failure to restart.
    ExitedCleanly,
    /// The server kept crashing and the restart budget ran out.
    GaveUp,
}

// ── Public operations (Windows) ──────────────────────────────

#[cfg(windows)]
pub use sys::{notify_ready, stop_requested};

#[cfg(windows)]
pub fn install(exe: &Path, config: &Path) -> Result<(), String> {
    let command = run_command(exe, config)?;
    let names = Names::service();
    let _control = sys::control_lock(&names)?;
    sys::stop_supervisor(&names, STOP_TIMEOUT)?;
    sys::write_run_value(RUN_VALUE, &command)?;
    sys::start_supervisor(exe, config, &names, true)
}

#[cfg(windows)]
pub fn uninstall() -> Result<(), String> {
    let names = Names::service();
    let _control = sys::control_lock(&names)?;
    sys::stop_supervisor(&names, STOP_TIMEOUT)?;
    sys::delete_run_value(RUN_VALUE)
}

/// `(installed, active)`. Active means the supervisor is alive *and* its
/// server has bound its port, so a different process answering on that port
/// cannot pass for the service.
#[cfg(windows)]
pub fn status() -> Result<(bool, bool), String> {
    let names = Names::service();
    let installed = sys::read_run_value(RUN_VALUE)?.is_some();
    let active = sys::mutex_held(&names.mutex) && sys::event_is_set(&names.ready);
    Ok((installed, active))
}

#[cfg(windows)]
pub fn stop() -> Result<(), String> {
    let names = Names::service();
    let _control = sys::control_lock(&names)?;
    sys::stop_supervisor(&names, STOP_TIMEOUT).map(|_| ())
}

/// Restart from the installed entry, so the service keeps the binary and
/// config it was installed with, whatever directory this runs from.
#[cfg(windows)]
pub fn restart() -> Result<(), String> {
    let command = sys::read_run_value(RUN_VALUE)?.ok_or_else(|| {
        "the service is not installed. Install it: lific service install".to_string()
    })?;
    let (exe, config) = parse_run_command(&command).ok_or_else(|| {
        format!(
            "{} was not written by lific; reinstall with `lific service install`",
            definition_display()
        )
    })?;
    let names = Names::service();
    let _control = sys::control_lock(&names)?;
    sys::stop_supervisor(&names, STOP_TIMEOUT)?;
    sys::start_supervisor(&exe, &config, &names, true)
}

/// `lific service run`, the logon entry point: start the supervisor and exit
/// immediately. On Windows before 24H2 this process owns the console window
/// the user may glimpse, so it must not wait for anything.
#[cfg(windows)]
pub fn launch(config: &Path) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot resolve the lific binary path: {e}"))?;
    sys::start_supervisor(&exe, config, &Names::service(), false)
}

/// `lific service supervise`: keep `lific start` running until stopped.
#[cfg(windows)]
pub fn supervise_service(config: &Path) -> Result<Outcome, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("cannot resolve the lific binary path: {e}"))?;
    let workdir = config
        .parent()
        .ok_or_else(|| format!("config path {} has no parent directory", config.display()))?;
    let config = config.to_path_buf();
    sys::supervise(
        || {
            let mut command = std::process::Command::new(&exe);
            command.arg("--config").arg(&config).arg("start");
            command
        },
        workdir,
        &workdir.join(LOG_FILE),
        &Names::service(),
        RestartBudget::new(RESTART_LIMIT, RESTART_WINDOW),
    )
}

// ── Public operations (elsewhere) ────────────────────────────

/// The command the installed autostart entry runs, if any. Doctor (and any
/// other local diagnostic) parses it with [`parse_run_command`] to learn
/// which config this machine's instance runs — the install-time witness.
#[cfg(windows)]
pub fn installed_run_command() -> Result<Option<String>, String> {
    sys::read_run_value(RUN_VALUE)
}

#[cfg(not(windows))]
pub fn installed_run_command() -> Result<Option<String>, String> {
    Err(NOT_WINDOWS.into())
}

#[cfg(not(windows))]
pub fn install(_exe: &Path, _config: &Path) -> Result<(), String> {
    Err(NOT_WINDOWS.into())
}

#[cfg(not(windows))]
pub fn uninstall() -> Result<(), String> {
    Err(NOT_WINDOWS.into())
}

#[cfg(not(windows))]
pub fn status() -> Result<(bool, bool), String> {
    Err(NOT_WINDOWS.into())
}

#[cfg(not(windows))]
pub fn stop() -> Result<(), String> {
    Err(NOT_WINDOWS.into())
}

#[cfg(not(windows))]
pub fn restart() -> Result<(), String> {
    Err(NOT_WINDOWS.into())
}

#[cfg(not(windows))]
pub fn launch(_config: &Path) -> Result<(), String> {
    Err(NOT_WINDOWS.into())
}

#[cfg(not(windows))]
pub fn supervise_service(_config: &Path) -> Result<Outcome, String> {
    Err(NOT_WINDOWS.into())
}

/// Signal the supervisor that the server is listening. No-op off Windows.
#[cfg(not(windows))]
pub fn notify_ready() {}

/// Resolve when the supervisor asks the server to stop. Never, off Windows.
#[cfg(not(windows))]
pub async fn stop_requested() {
    std::future::pending::<()>().await;
}

// ── Win32 ────────────────────────────────────────────────────

#[cfg(windows)]
mod sys {
    use super::{
        GRACEFUL_STOP, LOG_ROTATE_BYTES, Names, Outcome, READY_EVENT_ENV, RESTART_DELAY, RUN_KEY,
        RestartBudget, START_TIMEOUT, STOP_EVENT_ENV,
    };
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE,
        WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateEventW,
        CreateMutexW, CreateProcessW, EVENT_MODIFY_STATE, INFINITE, MUTEX_ALL_ACCESS, OpenEventW,
        OpenMutexW, PROCESS_INFORMATION, ReleaseMutex, ResetEvent, STARTUPINFOW,
        SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn os_error(code: u32) -> std::io::Error {
        std::io::Error::from_raw_os_error(code as i32)
    }

    fn last_error() -> std::io::Error {
        os_error(unsafe { GetLastError() })
    }

    fn millis(duration: Duration) -> u32 {
        u32::try_from(duration.as_millis()).unwrap_or(INFINITE - 1)
    }

    /// An owned kernel handle, closed on drop.
    pub struct Handle(HANDLE);

    // SAFETY: kernel handles are process-wide values, usable from any thread.
    unsafe impl Send for Handle {}

    impl Handle {
        fn new(raw: HANDLE) -> Option<Self> {
            (!raw.is_null()).then_some(Self(raw))
        }

        fn raw(&self) -> HANDLE {
            self.0
        }

        #[cfg(test)]
        pub fn raw_for_test(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    // ── Registry ──

    pub fn write_run_value(name: &str, command: &str) -> Result<(), String> {
        let key = wide(RUN_KEY);
        let value = wide(name);
        let data = wide(command);
        let bytes = u32::try_from(data.len() * 2).map_err(|_| "startup command is too long")?;
        let status = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                bytes,
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(format!(
                r"cannot write the startup entry HKCU\{RUN_KEY}\{name}: {}",
                os_error(status)
            ))
        }
    }

    pub fn read_run_value(name: &str) -> Result<Option<String>, String> {
        let key = wide(RUN_KEY);
        let value = wide(name);
        let mut size: u32 = 0;
        let read = |buffer: *mut core::ffi::c_void, size: &mut u32| unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer,
                size,
            )
        };
        let status = read(std::ptr::null_mut(), &mut size);
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != 0 {
            return Err(format!(
                r"cannot read the startup entry HKCU\{RUN_KEY}\{name}: {}",
                os_error(status)
            ));
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2)];
        let status = read(buffer.as_mut_ptr().cast(), &mut size);
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status != 0 {
            return Err(format!(
                r"cannot read the startup entry HKCU\{RUN_KEY}\{name}: {}",
                os_error(status)
            ));
        }
        let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Ok(Some(String::from_utf16_lossy(&buffer[..end])))
    }

    pub fn delete_run_value(name: &str) -> Result<(), String> {
        let key = wide(RUN_KEY);
        let value = wide(name);
        let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
        if status == 0 || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!(
                r"cannot remove the startup entry HKCU\{RUN_KEY}\{name}: {}",
                os_error(status)
            ))
        }
    }

    // ── Mutex and events ──

    /// A named mutex this thread owns; released on drop.
    pub struct OwnedMutex(Handle);

    impl Drop for OwnedMutex {
        fn drop(&mut self) {
            unsafe {
                ReleaseMutex(self.0.raw());
            }
        }
    }

    /// Take ownership of `name`, waiting up to `wait`. `None` means another
    /// live thread holds it. An abandoned mutex (its owner died) is acquired.
    pub fn acquire_mutex(name: &str, wait: Duration) -> Result<Option<OwnedMutex>, String> {
        let wide_name = wide(name);
        let handle = Handle::new(unsafe { CreateMutexW(std::ptr::null(), 0, wide_name.as_ptr()) })
            .ok_or_else(|| format!("cannot create {name}: {}", last_error()))?;
        match unsafe { WaitForSingleObject(handle.raw(), millis(wait)) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(OwnedMutex(handle))),
            WAIT_TIMEOUT => Ok(None),
            _ => Err(format!("cannot wait for {name}: {}", last_error())),
        }
    }

    /// Is a live process holding `name`? Trying to take it is the only honest
    /// test: the mutex object outlives a dead owner while any handle is open.
    pub fn mutex_held(name: &str) -> bool {
        let wide_name = wide(name);
        let Some(handle) =
            Handle::new(unsafe { OpenMutexW(MUTEX_ALL_ACCESS, 0, wide_name.as_ptr()) })
        else {
            return false;
        };
        match unsafe { WaitForSingleObject(handle.raw(), 0) } {
            WAIT_TIMEOUT => true,
            WAIT_OBJECT_0 | WAIT_ABANDONED => {
                unsafe {
                    ReleaseMutex(handle.raw());
                }
                false
            }
            _ => false,
        }
    }

    /// Serializes install/stop/restart/uninstall across CLI invocations.
    pub fn control_lock(names: &Names) -> Result<OwnedMutex, String> {
        acquire_mutex(&names.control, Duration::from_secs(30))?.ok_or_else(|| {
            "another `lific service` command is still running; try again in a moment".into()
        })
    }

    /// A named manual-reset event.
    pub struct Event(Handle);

    impl Event {
        /// Create the event, or open it if it already exists.
        pub fn create(name: &str) -> Result<Self, String> {
            let wide_name = wide(name);
            Handle::new(unsafe { CreateEventW(std::ptr::null(), 1, 0, wide_name.as_ptr()) })
                .map(Self)
                .ok_or_else(|| format!("cannot create {name}: {}", last_error()))
        }

        pub fn open(name: &str) -> Option<Self> {
            let wide_name = wide(name);
            Handle::new(unsafe {
                OpenEventW(
                    SYNCHRONIZATION_SYNCHRONIZE | EVENT_MODIFY_STATE,
                    0,
                    wide_name.as_ptr(),
                )
            })
            .map(Self)
        }

        pub fn set(&self) {
            unsafe {
                SetEvent(self.0.raw());
            }
        }

        pub fn reset(&self) {
            unsafe {
                ResetEvent(self.0.raw());
            }
        }

        /// Wait up to `timeout` for the event; `true` if it is set.
        pub fn wait(&self, timeout: Duration) -> bool {
            unsafe { WaitForSingleObject(self.0.raw(), millis(timeout)) == WAIT_OBJECT_0 }
        }
    }

    pub fn event_is_set(name: &str) -> bool {
        Event::open(name).is_some_and(|event| event.wait(Duration::ZERO))
    }

    /// Called by `lific start` once its listener is bound.
    pub fn notify_ready() {
        if let Ok(name) = std::env::var(READY_EVENT_ENV)
            && let Some(event) = Event::open(&name)
        {
            event.set();
        }
    }

    /// Resolves when the supervisor that started this server asks it to
    /// stop. Pending forever for a server nobody supervises. The wait runs on
    /// a detached OS thread so it can never hold up runtime shutdown.
    pub async fn stop_requested() {
        let event = std::env::var(STOP_EVENT_ENV)
            .ok()
            .and_then(|name| Event::open(&name));
        let Some(event) = event else {
            return std::future::pending::<()>().await;
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            event.wait(Duration::from_millis(u64::from(INFINITE)));
            let _ = tx.send(());
        });
        if rx.await.is_err() {
            std::future::pending::<()>().await;
        }
    }

    // ── Job object ──

    /// A job whose processes all die when its last handle closes, including
    /// when the supervisor holding it is killed.
    pub struct Job(Handle);

    impl Job {
        pub fn kill_on_close() -> Result<Self, String> {
            let handle =
                Handle::new(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })
                    .ok_or_else(|| format!("cannot create a job object: {}", last_error()))?;
            // SAFETY: a plain C struct for which all-zero is a valid value.
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = unsafe {
                SetInformationJobObject(
                    handle.raw(),
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&info).cast(),
                    u32::try_from(std::mem::size_of_val(&info)).unwrap_or(u32::MAX),
                )
            };
            if ok == 0 {
                return Err(format!("cannot configure the job object: {}", last_error()));
            }
            Ok(Self(handle))
        }

        pub fn assign(&self, child: &Child) -> Result<(), String> {
            let ok =
                unsafe { AssignProcessToJobObject(self.0.raw(), child.as_raw_handle().cast()) };
            if ok == 0 {
                Err(format!(
                    "cannot place the server in a job object: {}",
                    last_error()
                ))
            } else {
                Ok(())
            }
        }
    }

    // ── Processes ──

    /// Start a process with no console window and no inherited handles,
    /// outside the caller's job when that job allows it.
    ///
    /// `CreateProcessW` directly, because `std::process::Command` always
    /// inherits every inheritable handle. A caller that captures lific's
    /// output (PowerShell `$out = lific service install`, an agent harness)
    /// hands lific an inheritable pipe, and a supervisor holding that pipe
    /// would keep the caller waiting for end-of-file for as long as the
    /// service runs.
    pub fn spawn_uninherited(
        application: &str,
        command_line: &str,
        workdir: &Path,
    ) -> Result<Handle, u32> {
        let application = wide(application);
        let directory = wide(&workdir.display().to_string());
        let attempt = |flags: u32| -> Result<Handle, u32> {
            // CreateProcessW may write into the command line buffer.
            let mut command_line = wide(command_line);
            // SAFETY: plain C structs for which all-zero is a valid value.
            let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
            startup.cb = u32::try_from(std::mem::size_of::<STARTUPINFOW>()).unwrap_or(u32::MAX);
            let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
            let ok = unsafe {
                CreateProcessW(
                    application.as_ptr(),
                    command_line.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    flags,
                    std::ptr::null(),
                    directory.as_ptr(),
                    &startup,
                    &mut info,
                )
            };
            if ok == 0 {
                return Err(unsafe { GetLastError() });
            }
            drop(Handle::new(info.hThread));
            Handle::new(info.hProcess).ok_or(0)
        };
        let base = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
        attempt(base | CREATE_BREAKAWAY_FROM_JOB).or_else(|error| {
            if error == ERROR_ACCESS_DENIED {
                attempt(base)
            } else {
                Err(error)
            }
        })
    }

    fn spawn_supervisor(exe: &Path, config: &Path, workdir: &Path) -> Result<(), String> {
        let (Some(exe_text), Some(config_text)) = (exe.to_str(), config.to_str()) else {
            return Err("service paths must be valid Unicode".into());
        };
        if exe_text.contains('"') || config_text.contains('"') {
            return Err("service paths must not contain double quotes".into());
        }
        let command_line = format!("\"{exe_text}\" --config \"{config_text}\" service supervise");
        spawn_uninherited(exe_text, &command_line, workdir)
            .map(drop)
            .map_err(|error| format!("cannot start {}: {}", exe.display(), os_error(error)))
    }

    pub fn start_supervisor(
        exe: &Path,
        config: &Path,
        names: &Names,
        wait: bool,
    ) -> Result<(), String> {
        let workdir = config
            .parent()
            .ok_or_else(|| format!("config path {} has no parent directory", config.display()))?;
        spawn_supervisor(exe, config, workdir)?;
        if !wait {
            return Ok(());
        }
        let deadline = Instant::now() + START_TIMEOUT;
        while Instant::now() < deadline {
            if mutex_held(&names.mutex) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(format!(
            "the service supervisor did not start within {}s; see {} in {}",
            START_TIMEOUT.as_secs(),
            super::LOG_FILE,
            workdir.display()
        ))
    }

    /// Ask a running supervisor to stop and wait until it is gone. `false`
    /// when nothing was running.
    pub fn stop_supervisor(names: &Names, timeout: Duration) -> Result<bool, String> {
        if !mutex_held(&names.mutex) {
            return Ok(false);
        }
        Event::create(&names.stop)?.set();
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !mutex_held(&names.mutex) {
                return Ok(true);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(format!(
            "the service did not stop within {}s",
            timeout.as_secs()
        ))
    }

    fn open_log(path: &Path) -> Result<File, String> {
        if std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_ROTATE_BYTES) {
            let mut rotated = path.as_os_str().to_owned();
            rotated.push(".1");
            let _ = std::fs::rename(path, rotated);
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))
    }

    /// One `write` per line: the server appends to the same file, and a
    /// formatted `writeln!` on an unbuffered file is several writes that its
    /// output can land between.
    fn log_line(log: &mut File, message: &str) {
        let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = log.write_all(format!("{now} lific service: {message}\n").as_bytes());
    }

    /// The supervisor loop. `make_child` builds the server command; the loop
    /// adds its environment, stdio and flags.
    pub fn supervise(
        mut make_child: impl FnMut() -> Command,
        workdir: &Path,
        log_path: &Path,
        names: &Names,
        mut budget: RestartBudget,
    ) -> Result<Outcome, String> {
        let Some(_lock) = acquire_mutex(&names.mutex, Duration::ZERO)? else {
            return Ok(Outcome::AlreadyRunning);
        };
        let stop = Event::create(&names.stop)?;
        stop.reset();
        let ready = Event::create(&names.ready)?;
        ready.reset();
        let job = Job::kill_on_close()?;
        let mut log = open_log(log_path)?;
        log_line(&mut log, "supervisor started");

        loop {
            if !budget.allow(Instant::now()) {
                log_line(
                    &mut log,
                    "the server keeps exiting with an error; giving up. Fix the cause above, then run `lific service restart`",
                );
                return Ok(Outcome::GaveUp);
            }
            let stdout = log
                .try_clone()
                .map_err(|e| format!("cannot share the log: {e}"))?;
            let stderr = log
                .try_clone()
                .map_err(|e| format!("cannot share the log: {e}"))?;
            let mut command = make_child();
            command
                .current_dir(workdir)
                .env(STOP_EVENT_ENV, &names.stop)
                .env(READY_EVENT_ENV, &names.ready)
                .env("NO_COLOR", "1")
                .stdin(Stdio::null())
                .stdout(stdout)
                .stderr(stderr)
                .creation_flags(CREATE_NO_WINDOW);
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(e) => {
                    log_line(&mut log, &format!("cannot start the server: {e}"));
                    if stop.wait(RESTART_DELAY) {
                        return Ok(Outcome::Stopped);
                    }
                    continue;
                }
            };
            if let Err(e) = job.assign(&child) {
                log_line(&mut log, &e);
            }
            log_line(&mut log, &format!("server started (pid {})", child.id()));

            let handles = [stop.0.raw(), child.as_raw_handle().cast()];
            let woke = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
            if woke == WAIT_OBJECT_0 {
                log_line(&mut log, "stop requested");
                let graceful = unsafe {
                    WaitForSingleObject(child.as_raw_handle().cast(), millis(GRACEFUL_STOP))
                } == WAIT_OBJECT_0;
                if !graceful {
                    log_line(&mut log, "server did not shut down in time; ending it");
                    let _ = child.kill();
                }
                let _ = child.wait();
                ready.reset();
                log_line(&mut log, "stopped");
                return Ok(Outcome::Stopped);
            }

            let status = child
                .wait()
                .map_err(|e| format!("cannot reap the server: {e}"))?;
            ready.reset();
            if status.success() {
                log_line(&mut log, "server exited cleanly; supervisor exiting");
                return Ok(Outcome::ExitedCleanly);
            }
            log_line(
                &mut log,
                &format!(
                    "server exited ({status}); restarting in {}s",
                    RESTART_DELAY.as_secs()
                ),
            );
            if stop.wait(RESTART_DELAY) {
                log_line(&mut log, "stopped");
                return Ok(Outcome::Stopped);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_command_round_trips_paths_with_spaces() {
        let exe = Path::new(r"C:\Program Files\lific\lific.exe");
        let config = Path::new(r"C:\Users\Zorro Q\AppData\Roaming\lific\lific.toml");
        let command = run_command(exe, config).unwrap();
        assert_eq!(
            command,
            r#""C:\Program Files\lific\lific.exe" --config "C:\Users\Zorro Q\AppData\Roaming\lific\lific.toml" service run"#
        );
        assert_eq!(
            parse_run_command(&command),
            Some((exe.to_path_buf(), config.to_path_buf()))
        );
    }

    #[test]
    fn run_command_refuses_what_windows_would_not_run() {
        let long = format!(r"C:\{}\lific.toml", "d".repeat(MAX_RUN_COMMAND));
        let err = run_command(Path::new(r"C:\lific.exe"), Path::new(&long)).unwrap_err();
        assert!(err.contains("at most 260"), "got: {err}");
        assert!(run_command(Path::new(r#"C:\a"b\lific.exe"#), Path::new(r"C:\x.toml")).is_err());
    }

    #[test]
    fn foreign_run_values_are_not_parsed() {
        for value in [
            "",
            r"C:\lific.exe start",
            r#""C:\lific.exe" start"#,
            r#""C:\lific.exe" --config "C:\x.toml" start"#,
            r#""C:\lific.exe" --config "C:\x.toml" service run --extra"#,
            r#""" --config "C:\x.toml" service run"#,
        ] {
            assert_eq!(parse_run_command(value), None, "parsed {value:?}");
        }
    }

    #[test]
    fn restart_budget_allows_a_burst_then_recovers_after_the_window() {
        let mut budget = RestartBudget::new(3, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(budget.allow(t0));
        assert!(budget.allow(t0 + Duration::from_secs(1)));
        assert!(budget.allow(t0 + Duration::from_secs(2)));
        assert!(!budget.allow(t0 + Duration::from_secs(3)));
        // The first start ages out of the window, freeing one slot, and the
        // other two are still inside it half a second later.
        assert!(budget.allow(t0 + Duration::from_secs(60)));
        assert!(!budget.allow(t0 + Duration::from_millis(60_500)));
    }

    #[test]
    fn object_names_live_in_the_session_namespace() {
        let names = Names::service();
        for name in [&names.mutex, &names.stop, &names.ready, &names.control] {
            assert!(name.starts_with(r"Local\dev.lific.service"), "{name}");
        }
        assert_eq!(
            definition_display(),
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Lific"
        );
    }

    #[cfg(windows)]
    mod windows_only {
        use super::super::sys;
        use super::super::*;
        use std::process::Command;

        fn unique(tag: &str) -> String {
            format!("dev.lific.test.{tag}.{}", std::process::id())
        }

        #[test]
        fn run_value_round_trips_through_the_registry() {
            let name = unique("runvalue");
            assert_eq!(sys::read_run_value(&name).unwrap(), None);
            let command =
                run_command(Path::new(r"C:\t\lific.exe"), Path::new(r"C:\t\lific.toml")).unwrap();
            sys::write_run_value(&name, &command).unwrap();
            assert_eq!(
                sys::read_run_value(&name).unwrap().as_deref(),
                Some(command.as_str())
            );
            sys::delete_run_value(&name).unwrap();
            assert_eq!(sys::read_run_value(&name).unwrap(), None);
            // Deleting what is already gone is not an error.
            sys::delete_run_value(&name).unwrap();
        }

        #[test]
        fn a_held_mutex_reads_as_running_and_a_released_one_does_not() {
            let names = Names::with_base(&unique("mutex"));
            assert!(!sys::mutex_held(&names.mutex));
            let (held_tx, held_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
            let mutex = names.mutex.clone();
            let owner = std::thread::spawn(move || {
                let _lock = sys::acquire_mutex(&mutex, Duration::ZERO).unwrap().unwrap();
                held_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            held_rx.recv().unwrap();
            assert!(sys::mutex_held(&names.mutex));
            // Probing must not disturb the owner.
            assert!(sys::mutex_held(&names.mutex));
            release_tx.send(()).unwrap();
            owner.join().unwrap();
            assert!(!sys::mutex_held(&names.mutex));
        }

        fn cmd_exit(code: u8) -> Command {
            let mut command = Command::new("cmd.exe");
            command.args(["/C", &format!("exit {code}")]);
            command
        }

        #[test]
        fn supervisor_gives_up_on_a_server_that_keeps_crashing() {
            let dir = tempfile::tempdir().unwrap();
            let names = Names::with_base(&unique("crash"));
            let log = dir.path().join(LOG_FILE);
            let outcome = sys::supervise(
                || cmd_exit(3),
                dir.path(),
                &log,
                &names,
                RestartBudget::new(2, Duration::from_secs(60)),
            )
            .unwrap();
            assert_eq!(outcome, Outcome::GaveUp);
            let text = std::fs::read_to_string(&log).unwrap();
            assert_eq!(text.matches("server started").count(), 2, "log:\n{text}");
            assert!(text.contains("giving up"), "log:\n{text}");
            assert!(!sys::mutex_held(&names.mutex));
        }

        #[test]
        fn supervisor_treats_a_clean_exit_as_done() {
            let dir = tempfile::tempdir().unwrap();
            let names = Names::with_base(&unique("clean"));
            let outcome = sys::supervise(
                || cmd_exit(0),
                dir.path(),
                &dir.path().join(LOG_FILE),
                &names,
                RestartBudget::new(5, Duration::from_secs(60)),
            )
            .unwrap();
            assert_eq!(outcome, Outcome::ExitedCleanly);
        }

        #[test]
        fn stop_request_ends_a_running_server_and_releases_the_lock() {
            let dir = tempfile::tempdir().unwrap();
            let names = Names::with_base(&unique("stop"));
            let log = dir.path().join(LOG_FILE);
            let workdir = dir.path().to_path_buf();
            let thread_names = names.clone();
            let thread_log = log.clone();
            let supervisor = std::thread::spawn(move || {
                sys::supervise(
                    || {
                        // A long-lived stand-in for `lific start`.
                        let mut command = Command::new("cmd.exe");
                        command.args(["/C", "ping -n 60 127.0.0.1 >NUL"]);
                        command
                    },
                    &workdir,
                    &thread_log,
                    &thread_names,
                    RestartBudget::new(5, Duration::from_secs(60)),
                )
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            while !sys::mutex_held(&names.mutex) {
                assert!(Instant::now() < deadline, "supervisor never took its lock");
                std::thread::sleep(Duration::from_millis(50));
            }
            assert!(sys::stop_supervisor(&names, Duration::from_secs(20)).unwrap());
            assert_eq!(supervisor.join().unwrap().unwrap(), Outcome::Stopped);
            assert!(!sys::mutex_held(&names.mutex));
            assert!(!sys::stop_supervisor(&names, Duration::from_secs(1)).unwrap());
            let text = std::fs::read_to_string(&log).unwrap();
            assert!(text.contains("stop requested"), "log:\n{text}");
        }

        /// Whether the write end of an inheritable pipe is still held open
        /// by someone after this process closes its copy.
        fn pipe_outlives_our_copy(spawn: impl FnOnce()) -> bool {
            use std::io::Read;
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};
            let (mut reader, writer) = std::io::pipe().unwrap();
            let raw = writer.as_raw_handle().cast();
            assert_ne!(
                unsafe { SetHandleInformation(raw, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) },
                0
            );
            spawn();
            drop(writer);
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut sink = Vec::new();
                let _ = reader.read_to_end(&mut sink);
                let _ = tx.send(());
            });
            rx.recv_timeout(Duration::from_secs(3)).is_err()
        }

        fn ping() -> String {
            let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            format!(r"{root}\System32\PING.EXE")
        }

        #[test]
        fn the_supervisor_spawn_leaks_no_handles_to_its_child() {
            let ping = ping();
            let dir = std::env::temp_dir();
            let mut child = None;
            let leaked = pipe_outlives_our_copy(|| {
                child = Some(
                    sys::spawn_uninherited(&ping, &format!("\"{ping}\" -n 30 127.0.0.1"), &dir)
                        .unwrap(),
                );
            });
            if let Some(child) = child {
                unsafe {
                    windows_sys::Win32::System::Threading::TerminateProcess(child.raw_for_test(), 1)
                };
            }
            assert!(!leaked, "the detached child kept the caller's pipe open");

            // Control: std's Command inherits it, which is why it is not used.
            let mut std_child = None;
            let std_leaked = pipe_outlives_our_copy(|| {
                std_child = Some(
                    Command::new(&ping)
                        .args(["-n", "30", "127.0.0.1"])
                        .stdout(std::process::Stdio::null())
                        .spawn()
                        .unwrap(),
                );
            });
            if let Some(mut c) = std_child {
                let _ = c.kill();
                let _ = c.wait();
            }
            assert!(
                std_leaked,
                "control failed: std Command no longer inherits handles"
            );
        }

        #[test]
        fn second_supervisor_defers_to_the_first() {
            let dir = tempfile::tempdir().unwrap();
            let names = Names::with_base(&unique("second"));
            let _lock = sys::acquire_mutex(&names.mutex, Duration::ZERO)
                .unwrap()
                .unwrap();
            let names_for_thread = names;
            let path = dir.path().to_path_buf();
            // Mutex ownership is per thread, so the contender runs on another.
            let outcome = std::thread::spawn(move || {
                sys::supervise(
                    || cmd_exit(0),
                    &path,
                    &path.join(LOG_FILE),
                    &names_for_thread,
                    RestartBudget::new(5, Duration::from_secs(60)),
                )
            })
            .join()
            .unwrap()
            .unwrap();
            assert_eq!(outcome, Outcome::AlreadyRunning);
        }
    }
}
