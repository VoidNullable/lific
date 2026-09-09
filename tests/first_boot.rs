//! Process-level contract for `lific start --init-if-missing` (LIF-468).
//!
//! The unit tests cover the guards; this covers the thing an operator
//! actually gets: an image whose first boot leaves a working, authenticated
//! instance behind, and a plain `start` that still refuses on the same empty
//! volume.
//!
//! Every wait here is bounded, and the server child is owned by a guard that
//! kills it on drop, so a failing assertion never leaks a listening process.

use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Longest we ever wait for the server to answer.
const BOOT_TIMEOUT: Duration = Duration::from_secs(30);

const ADMIN_NAME: &str = "Container Admin";
const ADMIN_PASSWORD: &str = "first-boot-test-password";
/// `derive_username` slugifies the display name.
const ADMIN_USERNAME: &str = "container-admin";

/// Kills the server on drop, including while a test is unwinding.
struct ServerChild(Child);

impl Drop for ServerChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A free TCP port on loopback. Bound and released, so this races with any
/// other process that grabs it in between; nothing else in the suite listens.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind loopback")
        .local_addr()
        .expect("local addr")
        .port()
}

/// A config file naming `db_path`, with auth required (the default, restated
/// so the test asserts against something the file actually says).
fn write_config(dir: &Path, db_path: &Path, port: u16) -> PathBuf {
    let config_path = dir.join("lific.toml");
    let mut file = std::fs::File::create(&config_path).expect("create config");
    write!(
        file,
        "[server]\nhost = \"127.0.0.1\"\nport = {port}\n\n\
         [database]\npath = {db}\n\n\
         [backup]\nenabled = false\n\n\
         [auth]\nrequired = true\nallow_signup = false\n",
        db = toml_string(db_path)
    )
    .expect("write config");
    config_path
}

fn toml_string(path: &Path) -> String {
    format!("\"{}\"", path.display().to_string().replace('\\', "\\\\"))
}

fn lific(config: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lific"));
    // Never let the developer's own instance, token, or URL leak in.
    for name in [
        "LIFIC_URL",
        "LIFIC_API_KEY",
        "LIFIC_TOKEN",
        "LIFIC_INIT_ADMIN_NAME",
        "LIFIC_INIT_ADMIN_PASSWORD",
        "RUST_LOG",
    ] {
        command.env_remove(name);
    }
    command.env("NO_COLOR", "1");
    command.arg("--config").arg(config);
    command
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("build client")
}

enum Boot {
    Healthy,
    /// The child gave up on its own. Carries its stderr, so the caller can
    /// tell "the port was taken" from a real failure.
    Exited(String),
    TimedOut,
}

/// Poll `/api/health` until it answers 200, or give up. Never hangs, and
/// notices a child that died instead of waiting out the full timeout on a
/// process that will never answer.
fn wait_healthy(client: &reqwest::blocking::Client, base: &str, child: &mut Child) -> Boot {
    let deadline = Instant::now() + BOOT_TIMEOUT;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            let mut stderr = String::new();
            if let Some(pipe) = child.stderr.as_mut() {
                use std::io::Read;
                let _ = pipe.read_to_string(&mut stderr);
            }
            return Boot::Exited(stderr);
        }
        if let Ok(response) = client.get(format!("{base}/api/health")).send()
            && response.status().is_success()
        {
            return Boot::Healthy;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Boot::TimedOut
}

/// Start the server on a free port and wait for it to answer.
///
/// `free_port` releases the port before the server claims it, so another
/// process on the machine can take it in between. That is a flake, not a
/// failure: retry on a fresh port a couple of times before believing it.
fn boot_server(
    dir: &Path,
    db_path: &Path,
    client: &reqwest::blocking::Client,
) -> (ServerChild, u16) {
    let mut last = String::new();
    for attempt in 1..=3 {
        let port = free_port();
        let config = write_config(dir, db_path, port);
        let mut server = ServerChild(
            lific(&config)
                .args(["start", "--init-if-missing"])
                .env("LIFIC_INIT_ADMIN_NAME", ADMIN_NAME)
                .env("LIFIC_INIT_ADMIN_PASSWORD", ADMIN_PASSWORD)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn server"),
        );
        match wait_healthy(client, &format!("http://127.0.0.1:{port}"), &mut server.0) {
            Boot::Healthy => return (server, port),
            Boot::Exited(stderr) if stderr.contains("in use") || stderr.contains("EADDRINUSE") => {
                last = format!("attempt {attempt}: port {port} was taken");
            }
            Boot::Exited(stderr) => panic!("server exited on attempt {attempt}: {stderr}"),
            Boot::TimedOut => panic!("no answer within {BOOT_TIMEOUT:?} on attempt {attempt}"),
        }
    }
    panic!("could not find a free port in 3 attempts ({last})");
}

#[test]
fn init_if_missing_creates_an_authenticated_instance_with_the_env_admin() {
    let dir = tempfile::tempdir().expect("temp dir");
    // A fresh, writable, empty "volume": the mount point exists, the database
    // inside it does not.
    let data = dir.path().join("data");
    std::fs::create_dir(&data).expect("create data dir");
    let db_path = data.join("lific.db");
    let config = write_config(dir.path(), &db_path, free_port());

    // Plain `start` on the same fresh volume must still refuse.
    let refused = lific(&config)
        .arg("start")
        .output()
        .expect("run plain start");
    assert!(
        !refused.status.success(),
        "plain start must not create a database"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("no Lific instance found"),
        "unexpected stderr: {stderr}"
    );
    assert!(!db_path.exists(), "the refusal created a database");

    let client = client();
    let (server, port) = boot_server(dir.path(), &db_path, &client);
    let base = format!("http://127.0.0.1:{port}");
    assert!(db_path.exists(), "first boot did not create the database");

    // The admin exists, signup is closed, and auto-login (which would imply
    // auth is off) is not on. An open instance with no users hands admin to
    // whoever loads the page first.
    let instance: serde_json::Value = client
        .get(format!("{base}/api/instance"))
        .send()
        .expect("GET /api/instance")
        .json()
        .expect("instance json");
    assert_eq!(instance["has_users"], true, "no admin was created");
    assert_eq!(instance["allow_signup"], false, "signup was left open");
    assert_eq!(
        instance["web_auto_login"], false,
        "auth must stay required on first boot"
    );

    // Anonymous access is refused: the flag does not weaken auth.
    let anonymous = client
        .get(format!("{base}/api/projects"))
        .send()
        .expect("GET /api/projects");
    assert_eq!(anonymous.status().as_u16(), 401, "anonymous read allowed");

    // The env credentials really do sign in.
    let login = client
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({
            "identity": ADMIN_USERNAME,
            "password": ADMIN_PASSWORD,
        }))
        .send()
        .expect("POST /api/auth/login");
    assert!(
        login.status().is_success(),
        "login failed: {}",
        login.status()
    );

    // A wrong password is still a wrong password.
    let bad = client
        .post(format!("{base}/api/auth/login"))
        .json(&serde_json::json!({
            "identity": ADMIN_USERNAME,
            "password": "not-the-password",
        }))
        .send()
        .expect("POST /api/auth/login");
    assert!(!bad.status().is_success(), "wrong password was accepted");

    drop(server);
}

#[test]
fn init_if_missing_refuses_to_create_a_database_with_no_admin_credentials() {
    let dir = tempfile::tempdir().expect("temp dir");
    let data = dir.path().join("data");
    std::fs::create_dir(&data).expect("create data dir");
    let db_path = data.join("lific.db");
    let config = write_config(dir.path(), &db_path, free_port());

    let output = lific(&config)
        .args(["start", "--init-if-missing"])
        .output()
        .expect("run start");

    assert!(
        !output.status.success(),
        "a database with no admin must not be created"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("LIFIC_INIT_ADMIN_NAME"), "{stderr}");
    assert!(stderr.contains("LIFIC_INIT_ADMIN_PASSWORD"), "{stderr}");
    assert!(!db_path.exists(), "the refusal created a database");
}

#[test]
fn init_if_missing_refuses_half_a_pair_of_admin_variables() {
    let dir = tempfile::tempdir().expect("temp dir");
    let data = dir.path().join("data");
    std::fs::create_dir(&data).expect("create data dir");
    let db_path = data.join("lific.db");
    let config = write_config(dir.path(), &db_path, free_port());

    let output = lific(&config)
        .args(["start", "--init-if-missing"])
        .env("LIFIC_INIT_ADMIN_NAME", ADMIN_NAME)
        .output()
        .expect("run start");

    assert!(!output.status.success(), "half a pair must refuse");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("LIFIC_INIT_ADMIN_PASSWORD"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        !db_path.exists(),
        "a refused first boot must leave no database behind"
    );
}

#[test]
fn init_if_missing_refuses_a_missing_mount_point() {
    let dir = tempfile::tempdir().expect("temp dir");
    // The volume was never mounted: the parent directory does not exist.
    let data = dir.path().join("data");
    let db_path = data.join("lific.db");
    let config = write_config(dir.path(), &db_path, free_port());

    let output = lific(&config)
        .args(["start", "--init-if-missing"])
        .output()
        .expect("run start");

    assert!(!output.status.success(), "missing mount point must refuse");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not exist"),
        "unexpected stderr: {stderr}"
    );
    assert!(!data.exists(), "the mount point must not be created");
}
