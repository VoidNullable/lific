//! First boot for a container: `lific start --init-if-missing` (LIF-468).
//!
//! A container image has no interactive `lific init` step. The image starts,
//! the mounted volume is empty, and `start` refuses because only `init`
//! creates a database. This module is the narrow exception to that rule, and
//! it is deliberately more suspicious than `init` is:
//!
//! - it only ever runs when the operator typed `--init-if-missing`,
//! - it refuses unless the database location was chosen on purpose (an
//!   explicit `--config`/`--db`, or a config file that was actually found),
//!   so the flag can never conjure a stray `lific.db` in whatever directory
//!   the process happened to start in,
//! - it refuses to create the parent directory, because a missing mount point
//!   is the normal shape of a misconfigured volume and silently writing into
//!   the container's ephemeral filesystem loses the operator's data on the
//!   next restart,
//! - it never mints the unbound "default" API key. `init` does that in the
//!   operator's own terminal where they can read it; here stdout is a
//!   container log, which is the wrong place for an operator credential.
//!   Creating the admin is what prevents it: `auth::should_mint_initial_key`
//!   is false the moment a human operator exists.
//!
//! **Creating a database requires both admin environment variables.** An
//! instance that boots with zero users is not a neutral starting state: signup
//! would be open, whoever loads the page first becomes the admin, and
//! `lific start` would mint and print an unbound operator key into the
//! container log. So a fresh database is created only when there is an admin to
//! put in it. An *existing* database ignores the variables, except for the one
//! case below.
//!
//! **The seeding is one immediate transaction, and the admin is its own
//! completion marker.** Creating the database and seeding it cannot be a single
//! atomic step (the file has to exist and be migrated before anything can be
//! written to it), so a crash in between leaves a migrated database with no
//! users. The next `--init-if-missing` start finds that and finishes the job
//! rather than treating "the file exists" as "the instance is set up". A
//! separate marker row would add nothing: it could only ever be written in the
//! same transaction as the admin, so "no marker" and "no human admin" are the
//! same fact. Two starters racing serialize on the immediate transaction, and
//! the loser sees the admin and does nothing.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::config::{Config, ConfigSource};
use crate::db::models::User;
use crate::db::{self, DbPool};
use crate::error::LificError;

/// Display name of the first admin to create on a fresh database.
pub const ADMIN_NAME_ENV: &str = "LIFIC_INIT_ADMIN_NAME";
/// Password of the first admin to create on a fresh database. Read once, moved
/// straight into the hasher, and never logged or echoed.
pub const ADMIN_PASSWORD_ENV: &str = "LIFIC_INIT_ADMIN_PASSWORD";

/// What `--init-if-missing` should do about the configured database path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// A database is already there, so the flag is a no-op and startup
    /// proceeds exactly as a plain `lific start` would.
    AlreadyExists,
    /// Nothing is there yet and every guard passed: create and migrate it.
    Initialize,
}

/// The first admin, as named by the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstAdmin {
    pub name: String,
    pub password: String,
}

/// Decide whether `--init-if-missing` may create the database at `db_path`.
///
/// `source` is where the running configuration came from and `db_flag` is
/// whether `--db` was given; together they answer "did somebody choose this
/// path on purpose?". Pure apart from the filesystem probes, so the guards
/// are testable without a server.
pub fn decide(db_path: &Path, source: ConfigSource, db_flag: bool) -> Result<Decision, String> {
    if db_path.exists() {
        return Ok(Decision::AlreadyExists);
    }

    let shown = crate::config::absolutize(db_path).display().to_string();

    // The built-in default is the bare relative `lific.db`, which resolves
    // against the process cwd. That is precisely the path this flag must
    // never create: it is not a location anybody chose.
    if !db_flag && source == ConfigSource::BuiltInDefault {
        return Err(format!(
            "refusing to initialize {shown}: no config file was found and no --db was given, so \
             the database path is the built-in relative default. Say where the database belongs \
             with --config <file> or --db <file>."
        ));
    }

    let parent = db_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);

    if !parent.is_dir() {
        return Err(format!(
            "refusing to initialize {shown}: its parent directory {} does not exist. Create it, \
             or mount the volume there, before starting.",
            parent.display()
        ));
    }

    // Probe by writing, not by reading mode bits: the mode says nothing about
    // ACLs, a read-only mount, or the uid the container actually runs as, and
    // getting this wrong is a crash loop rather than a message.
    if let Err(error) = probe_writable(&parent) {
        return Err(format!(
            "refusing to initialize {shown}: its parent directory {} is not writable by this \
             process ({error}).",
            parent.display()
        ));
    }

    Ok(Decision::Initialize)
}

/// Can this process create a file in `dir`? The probe file removes itself.
fn probe_writable(dir: &Path) -> std::io::Result<()> {
    tempfile::Builder::new()
        .prefix(".lific-init-probe-")
        .tempfile_in(dir)
        .map(drop)
}

/// Turn the two admin environment variables into a first admin, if both are
/// set. Half a pair is a misconfiguration, not a default: silently ignoring
/// `LIFIC_INIT_ADMIN_PASSWORD` because the name was misspelled would boot an
/// instance whose operator believes it has an admin account and does not.
///
/// Takes the values rather than reading the environment itself, so the rule is
/// testable and so the caller can read each variable exactly once.
pub fn admin_from_env(
    name: Option<String>,
    password: Option<String>,
) -> Result<Option<FirstAdmin>, String> {
    match (name, password) {
        (None, None) => Ok(None),
        (Some(_), None) => Err(format!(
            "{ADMIN_NAME_ENV} is set but {ADMIN_PASSWORD_ENV} is not. Set both to create the \
             first admin, or neither to start without one."
        )),
        (None, Some(_)) => Err(format!(
            "{ADMIN_PASSWORD_ENV} is set but {ADMIN_NAME_ENV} is not. Set both to create the \
             first admin, or neither to start without one."
        )),
        (Some(name), Some(password)) => {
            let name = name.trim().to_owned();
            if name.is_empty() {
                return Err(format!("{ADMIN_NAME_ENV} is set but empty"));
            }
            // Not trimmed: leading and trailing whitespace is part of a
            // password. Only the empty case is refused, which is what
            // `create_first_admin_with_password` rejects anyway.
            if password.is_empty() {
                return Err(format!("{ADMIN_PASSWORD_ENV} is set but empty"));
            }
            Ok(Some(FirstAdmin { name, password }))
        }
    }
}

/// Create the first human admin on a zero-user instance, together with the
/// `web_auto_login` flag that belongs beside it (that one lives in the
/// database, not the config file).
///
/// `password` of `None` means the passwordless operator of login-free mode.
/// Shared by `lific init` and `lific start --init-if-missing` so the two
/// first-run paths can never disagree about what a fresh instance looks like.
pub fn create_first_admin(
    conn: &Connection,
    display_name: &str,
    password: Option<&str>,
    web_auto_login: bool,
) -> Result<User, LificError> {
    db::queries::settings::update(
        conn,
        db::queries::settings::InstanceSettingsPatch {
            web_auto_login: Some(web_auto_login),
            ..Default::default()
        },
    )?;
    match password {
        Some(password) => {
            db::queries::users::create_first_admin_with_password(conn, display_name, password)
        }
        None => db::queries::users::create_passwordless_admin(conn, display_name),
    }
}

/// Why a fresh database will not be created without an operator.
fn admin_required() -> String {
    format!(
        "refusing to create a database without an administrator: set both {ADMIN_NAME_ENV} and \
         {ADMIN_PASSWORD_ENV}. An instance with no users has signup open and hands admin to \
         whoever loads the page first, so first-boot initialization always creates the \
         operator. (An existing database ignores both variables.)"
    )
}

/// Why an incomplete database will not be finished without an operator.
fn incomplete_admin_required(path: &Path) -> String {
    format!(
        "refusing to start: {} exists but has no administrator, which is what an interrupted \
         first boot leaves behind. Set both {ADMIN_NAME_ENV} and {ADMIN_PASSWORD_ENV} and start \
         again to finish setting it up, or create the account by hand with `lific user create \
         --username <name> --email <address> --password <password> --admin`. Starting anyway \
         would serve an instance with open signup that hands admin to its first visitor.",
        crate::config::absolutize(path).display()
    )
}

/// What one bootstrap attempt did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bootstrap {
    /// A human admin was already there: nothing to do.
    AlreadyBootstrapped,
    /// This call created the first admin.
    Created,
}

/// Does this database still need an operator? The question `--init-if-missing`
/// asks of a database that already exists.
///
/// A human *admin*, not merely a human: a database holding only non-admin
/// people is still an instance nobody can administer, and leaving it that way
/// keeps it on the same rails an empty one is on.
pub fn needs_bootstrap(pool: &DbPool) -> Result<bool, LificError> {
    let conn = pool.read()?;
    Ok(!db::queries::users::has_human_admin(&conn)?)
}

/// Seed a migrated database in one immediate transaction: the settings row,
/// then the first admin. Idempotent and safe to race, because the transaction
/// serializes concurrent starters and the second one finds the admin.
///
/// `allow_signup` is forced off regardless of what the config file says. This
/// runs at the exact moment the instance has no administrator, which is when
/// open signup means "the next stranger to find this URL is in charge". An
/// operator who wants signup turns it on afterwards, from an account.
pub fn bootstrap(pool: &DbPool, admin: &FirstAdmin) -> Result<Bootstrap, LificError> {
    pool.transaction(|tx| {
        // The completion marker: the admin row itself, written in this same
        // transaction, so a crash before commit is indistinguishable from
        // never having started and the next boot simply retries.
        if db::queries::users::has_human_admin(tx)? {
            return Ok(Bootstrap::AlreadyBootstrapped);
        }
        db::queries::settings::ensure(tx, false)?;
        db::queries::settings::update(
            tx,
            db::queries::settings::InstanceSettingsPatch {
                allow_signup: Some(false),
                ..Default::default()
            },
        )?;
        // web_auto_login off: an admin with a password exists, and handing a
        // browser session to anyone who loads the page is the opposite of
        // what that means.
        create_first_admin(tx, &admin.name, Some(&admin.password), false)?;
        Ok(Bootstrap::Created)
    })
}

/// Run first-boot initialization for `lific start --init-if-missing`.
pub fn run(
    cfg: &Config,
    source: ConfigSource,
    db_flag: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    run_with(
        cfg,
        source,
        db_flag,
        std::env::var(ADMIN_NAME_ENV).ok(),
        std::env::var(ADMIN_PASSWORD_ENV).ok(),
    )
}

/// [`run`] with the environment passed in, so the rules below are testable
/// without mutating process-global state.
///
/// The credentials are examined **only** when they are going to be used: a
/// database that already has an administrator is healthy, and a stale, half
/// set, or empty pair left over in a compose file or machine config must not
/// be able to stop an instance from booting. They are validated in the two
/// cases where they matter: the database is missing, or it is confirmed to
/// have no administrator (what a crash between "migrate" and "seed" leaves).
/// In both of those an absent pair is fatal, because starting anyway serves
/// an instance with open signup that hands admin to its first visitor.
fn run_with(
    cfg: &Config,
    source: ConfigSource,
    db_flag: bool,
    name: Option<String>,
    password: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let path = &cfg.database.path;

    if decide(path, source, db_flag)? == Decision::Initialize {
        // Validated before anything is created: a refusal must leave no
        // database behind for the next boot to find and treat as set up.
        let admin = admin_from_env(name, password)?.ok_or_else(admin_required)?;
        let pool = db::open(path)?;
        let outcome = bootstrap(&pool, &admin)?;
        // Release the CLI's handles before the server opens the same file.
        drop(pool);
        // stderr rather than `tracing`: the subscriber is installed by
        // `server::run` a moment from now, so an `info!` here would be
        // dropped on the floor. The username is deliberately not logged; the
        // container log is a shared surface and the account name is half of a
        // credential.
        if outcome == Bootstrap::Created {
            eprintln!(
                "lific: created database {} and its first admin from the environment",
                path.display()
            );
        }
        return Ok(());
    }

    let pool = db::open(path)?;
    if !needs_bootstrap(&pool)? {
        // Healthy instance. The environment is not even looked at.
        return Ok(());
    }

    let admin = admin_from_env(name, password)?.ok_or_else(|| incomplete_admin_required(path))?;
    let outcome = bootstrap(&pool, &admin)?;
    drop(pool);
    if outcome == Bootstrap::Created {
        eprintln!(
            "lific: {} had no administrator (an interrupted first boot); created the first admin \
             from the environment",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use clap::Parser;

    fn db_in(dir: &Path) -> PathBuf {
        dir.join("lific.db")
    }

    #[test]
    fn an_existing_database_makes_the_flag_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let path = db_in(dir.path());
        std::fs::write(&path, b"").unwrap();

        // Even from the built-in default, which is otherwise refused: the
        // guard is about creating a database, and there is nothing to create.
        assert_eq!(
            decide(&path, ConfigSource::BuiltInDefault, false),
            Ok(Decision::AlreadyExists)
        );
    }

    #[test]
    fn the_built_in_default_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = db_in(dir.path());

        let error = decide(&path, ConfigSource::BuiltInDefault, false).unwrap_err();
        assert!(
            error.contains("built-in relative default"),
            "unexpected error: {error}"
        );
        assert!(!path.exists(), "the refusal must not create anything");
    }

    #[test]
    fn an_explicit_db_flag_rescues_the_built_in_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            decide(&db_in(dir.path()), ConfigSource::BuiltInDefault, true),
            Ok(Decision::Initialize)
        );
    }

    #[test]
    fn a_found_config_file_is_enough_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        for source in [
            ConfigSource::Explicit,
            ConfigSource::ProjectLocal,
            ConfigSource::User,
            ConfigSource::System,
        ] {
            assert_eq!(
                decide(&db_in(dir.path()), source, false),
                Ok(Decision::Initialize),
                "{source:?} names a real location"
            );
        }
    }

    #[test]
    fn a_missing_parent_directory_is_refused_rather_than_created() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("data");
        let path = db_in(&missing);

        let error = decide(&path, ConfigSource::Explicit, false).unwrap_err();
        assert!(
            error.contains("does not exist"),
            "unexpected error: {error}"
        );
        assert!(!missing.exists(), "the mount point must not be created");
    }

    #[cfg(unix)]
    #[test]
    fn an_unwritable_parent_directory_is_refused() {
        use std::os::unix::fs::PermissionsExt;

        // root ignores the mode bits, so the probe would succeed and the
        // assertion would be meaningless.
        if unsafe { libc::geteuid() } == 0 {
            eprintln!("skipping: running as root, directory permissions are not enforced");
            return;
        }

        let dir = tempfile::tempdir().unwrap();
        let sealed = dir.path().join("read-only");
        std::fs::create_dir(&sealed).unwrap();
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o500)).unwrap();

        let error = decide(&db_in(&sealed), ConfigSource::Explicit, false).unwrap_err();

        // Restore before asserting so the TempDir can still clean itself up.
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            error.contains("is not writable"),
            "unexpected error: {error}"
        );
    }

    fn admin() -> FirstAdmin {
        FirstAdmin {
            name: "Container Admin".into(),
            password: "first-boot-test-password".into(),
        }
    }

    fn human_admins(pool: &DbPool) -> i64 {
        pool.read()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM users WHERE is_bot = 0 AND is_admin = 1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn neither_admin_variable_set_means_no_admin() {
        assert_eq!(admin_from_env(None, None), Ok(None));
    }

    /// The refusal has to name both variables, because the operator who hits
    /// it set neither and has nothing else to go on.
    #[test]
    fn creating_a_database_without_an_admin_is_refused_by_name() {
        let message = admin_required();
        assert!(message.contains(ADMIN_NAME_ENV), "{message}");
        assert!(message.contains(ADMIN_PASSWORD_ENV), "{message}");
    }

    #[test]
    fn bootstrap_creates_a_closed_instance_with_one_admin() {
        let pool = db::open_memory().unwrap();
        assert_eq!(bootstrap(&pool, &admin()).unwrap(), Bootstrap::Created);

        assert_eq!(human_admins(&pool), 1);
        let settings = db::queries::settings::get(&pool.read().unwrap()).unwrap();
        assert!(
            !settings.allow_signup,
            "signup must be closed: an open instance hands admin to the first visitor"
        );
        assert!(
            !settings.web_auto_login,
            "auto-login would bypass the password"
        );
    }

    /// The other half of the CRITICAL: `lific start` mints and PRINTS an
    /// unbound operator key on a genuinely empty instance. Bootstrapping an
    /// admin is what makes that branch unreachable.
    #[test]
    fn a_bootstrapped_instance_never_mints_the_initial_api_key() {
        let pool = db::open_memory().unwrap();
        assert!(
            crate::auth::should_mint_initial_key(&pool),
            "precondition: an empty instance would mint one"
        );

        bootstrap(&pool, &admin()).unwrap();

        assert!(!crate::auth::should_mint_initial_key(&pool));
    }

    /// A crash between `db::open` (migrate) and the seeding transaction leaves
    /// a migrated database with no users. The next start must finish the job,
    /// not treat "the file exists" as "the instance is set up".
    #[test]
    fn an_interrupted_first_boot_is_retried_on_the_next_start() {
        let pool = db::open_memory().unwrap();
        // Exactly the state a rolled-back transaction leaves: schema, no users.
        assert_eq!(human_admins(&pool), 0);

        assert_eq!(bootstrap(&pool, &admin()).unwrap(), Bootstrap::Created);
        assert_eq!(human_admins(&pool), 1);
    }

    /// Two starters racing serialize on the immediate transaction; the loser
    /// sees the admin and does nothing. Sequential here, because that is the
    /// same code path with a deterministic order.
    #[test]
    fn a_second_bootstrap_is_a_no_op_and_never_creates_a_second_admin() {
        let pool = db::open_memory().unwrap();
        assert_eq!(bootstrap(&pool, &admin()).unwrap(), Bootstrap::Created);
        assert_eq!(
            bootstrap(&pool, &admin()).unwrap(),
            Bootstrap::AlreadyBootstrapped
        );
        assert_eq!(human_admins(&pool), 1);
    }

    /// A database holding only non-admin people is still one nobody can
    /// administer, so it counts as unfinished and gets the operator.
    #[test]
    fn a_human_who_is_not_an_admin_does_not_count_as_bootstrapped() {
        let pool = db::open_memory().unwrap();
        {
            let conn = pool.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, \
                 is_bot) VALUES ('member', 'member@local', 'x', 'Member', 0, 0)",
                [],
            )
            .unwrap();
        }

        assert!(needs_bootstrap(&pool).unwrap(), "no admin means unfinished");
        assert_eq!(bootstrap(&pool, &admin()).unwrap(), Bootstrap::Created);
        assert_eq!(human_admins(&pool), 1);
    }

    /// A bot admin is not a human operator, and must not pass for one.
    #[test]
    fn a_bot_admin_does_not_count_as_bootstrapped() {
        let pool = db::open_memory().unwrap();
        {
            let conn = pool.write().unwrap();
            conn.execute(
                "INSERT INTO users (username, email, password_hash, display_name, is_admin, \
                 is_bot) VALUES ('agent', 'agent@local', 'x', 'Agent', 1, 1)",
                [],
            )
            .unwrap();
        }

        assert!(needs_bootstrap(&pool).unwrap());
    }

    // ── run_with: which of the two paths looks at the environment ──

    /// A temp directory holding a real (not in-memory) database, because
    /// `run_with` decides from the path on disk.
    fn instance(dir: &Path) -> Config {
        let mut cfg = Config::default();
        cfg.database.path = db_in(dir);
        cfg
    }

    /// The headline of Sol's first finding: a healthy instance must boot even
    /// with a stale, half-set, or empty pair left in a compose file. Nothing
    /// about the environment may keep a working server down.
    #[test]
    fn a_healthy_database_ignores_the_admin_environment_entirely() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = instance(dir.path());
        let pool = db::open(&cfg.database.path).unwrap();
        bootstrap(&pool, &admin()).unwrap();
        drop(pool);

        for (name, password) in [
            (Some("Someone Else".to_owned()), None),
            (None, Some("stale-password".to_owned())),
            (Some(String::new()), Some(String::new())),
            (None, None),
        ] {
            run_with(&cfg, ConfigSource::Explicit, false, name, password)
                .expect("a healthy instance starts regardless of the environment");
        }

        // And none of that created a second administrator.
        let pool = db::open(&cfg.database.path).unwrap();
        assert_eq!(human_admins(&pool), 1);
    }

    /// The opposite case: confirmed incomplete, so the credentials are the
    /// difference between a usable instance and an open one. Refuse to start.
    #[test]
    fn an_incomplete_database_without_credentials_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = instance(dir.path());
        // Migrated, no users: what a crash between migrate and seed leaves.
        drop(db::open(&cfg.database.path).unwrap());

        let error = run_with(&cfg, ConfigSource::Explicit, false, None, None)
            .expect_err("an instance nobody can administer must not be served");
        let message = error.to_string();
        assert!(message.contains(ADMIN_NAME_ENV), "{message}");
        assert!(message.contains(ADMIN_PASSWORD_ENV), "{message}");
    }

    /// With the credentials there, the same state is finished rather than
    /// refused.
    #[test]
    fn an_incomplete_database_with_credentials_is_seeded_on_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = instance(dir.path());
        drop(db::open(&cfg.database.path).unwrap());

        run_with(
            &cfg,
            ConfigSource::Explicit,
            false,
            Some(admin().name),
            Some(admin().password),
        )
        .expect("the retry finishes the job");

        let pool = db::open(&cfg.database.path).unwrap();
        assert_eq!(human_admins(&pool), 1);
        assert!(!crate::auth::should_mint_initial_key(&pool));
    }

    /// A missing database with no credentials creates nothing at all.
    #[test]
    fn a_missing_database_without_credentials_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = instance(dir.path());

        let error = run_with(&cfg, ConfigSource::Explicit, false, None, None)
            .expect_err("a database is never created without an operator");
        assert!(error.to_string().contains(ADMIN_NAME_ENV));
        assert!(!cfg.database.path.exists(), "nothing may be left behind");
    }

    #[test]
    fn half_a_pair_of_admin_variables_is_refused() {
        let name_only = admin_from_env(Some("Blake".into()), None).unwrap_err();
        assert!(name_only.contains(ADMIN_PASSWORD_ENV), "{name_only}");

        let password_only = admin_from_env(None, Some("hunter2".into())).unwrap_err();
        assert!(password_only.contains(ADMIN_NAME_ENV), "{password_only}");
    }

    #[test]
    fn an_empty_admin_variable_is_refused() {
        assert!(admin_from_env(Some("  ".into()), Some("hunter2".into())).is_err());
        assert!(admin_from_env(Some("Blake".into()), Some(String::new())).is_err());
    }

    #[test]
    fn both_admin_variables_produce_the_first_admin() {
        assert_eq!(
            admin_from_env(Some("  Blake  ".into()), Some(" hunter2 ".into())),
            Ok(Some(FirstAdmin {
                name: "Blake".into(),
                // Whitespace is part of a password, so it survives.
                password: " hunter2 ".into(),
            }))
        );
    }

    #[test]
    fn only_start_with_the_flag_is_exempt_from_the_existing_database_guard() {
        let with_flag = Cli::parse_from(["lific", "start", "--init-if-missing"]);
        assert!(!crate::needs_existing_database(&with_flag.command));

        let without_flag = Cli::parse_from(["lific", "start"]);
        assert!(crate::needs_existing_database(&without_flag.command));

        // The flag belongs to `start` alone; nothing else changes shape.
        let mcp = Cli::parse_from(["lific", "mcp"]);
        assert!(crate::needs_existing_database(&mcp.command));
        assert!(matches!(with_flag.command, Command::Start { .. }));
    }

    #[test]
    fn the_flag_is_not_accepted_by_other_commands() {
        assert!(Cli::try_parse_from(["lific", "mcp", "--init-if-missing"]).is_err());
    }
}
