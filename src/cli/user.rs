//! `lific user <action>`: create and manage local user accounts.

use crate::cli::{UserAction, term, ui};
use crate::config::Config;
use crate::db;

pub fn run(
    cfg: &Config,
    action: UserAction,
    json_flag: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = term::wants_json(json_flag);
    let pool = db::open(&cfg.database.path)?;

    match action {
        UserAction::Create {
            username,
            email,
            password,
            admin,
            bot,
        } => {
            // Prompt for password if not provided. On a TTY use a
            // masked prompt (the old prompt echoed the password in
            // plaintext); piped stdin keeps the read-a-line behavior
            // so scripts can feed it.
            let pw = match password {
                Some(p) => p,
                None if term::stdin_is_tty() => cliclack::password("Password").interact()?,
                None => {
                    let mut buf = String::new();
                    std::io::stdin().read_line(&mut buf)?;
                    buf.trim().to_string()
                }
            };

            let conn = pool.write()?;
            // LIF-261: seed the settings row NOW, before this user
            // exists, so a CLI-first admin creation (`lific user create
            // --admin` before any `lific start`) still counts the DB as
            // fresh and gets authz_enforced on by default. `ensure` is a
            // no-op once the row exists, so this never overrides a prior
            // seed or an admin's later choice.
            db::queries::settings::ensure(&conn, cfg.auth.allow_signup)?;
            let user = db::queries::users::create_user(
                &conn,
                &db::models::CreateUser {
                    username: username.clone(),
                    email: email.clone(),
                    password: pw,
                    display_name: None,
                    is_admin: admin,
                    is_bot: bot,
                },
            )?;

            if json {
                let out = serde_json::json!({
                    "username": user.username,
                    "email": user.email,
                    "display_name": user.display_name,
                    "is_admin": user.is_admin,
                    "is_bot": user.is_bot,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                let role = if user.is_admin { " (admin)" } else { "" };
                ui::step(format!(
                    "User created: {}{role} {}",
                    user.username,
                    ui::dim(format!("({})", user.email))
                ));
            }
        }
        UserAction::List => {
            let conn = pool.read()?;
            let users = db::queries::users::list_users(&conn)?;

            if json {
                let out: Vec<_> = users
                    .iter()
                    .map(|u| {
                        serde_json::json!({
                            "id": u.id,
                            "username": u.username,
                            "email": u.email,
                            "is_admin": u.is_admin,
                            "is_bot": u.is_bot,
                            "created_at": u.created_at,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else if users.is_empty() {
                println!("No users.");
            } else {
                println!("{} user(s):", users.len());
                for u in &users {
                    let flags = match (u.is_admin, u.is_bot) {
                        (true, true) => " [admin, bot]",
                        (true, false) => " [admin]",
                        (false, true) => " [bot]",
                        (false, false) => "",
                    };
                    println!(
                        "  {} | {} | {}{} | created {}",
                        u.id, u.username, u.email, flags, u.created_at
                    );
                }
            }
        }
        UserAction::SetPassword { username, password } => {
            // Same prompt behavior as `user create`: masked prompt on
            // a TTY, read-a-line for piped stdin.
            let pw = match password {
                Some(p) => p,
                None if term::stdin_is_tty() => cliclack::password("New password").interact()?,
                None => {
                    let mut buf = String::new();
                    std::io::stdin().read_line(&mut buf)?;
                    buf.trim().to_string()
                }
            };

            let conn = pool.write()?;
            let user = db::queries::users::get_user_by_username(&conn, &username)?;
            // An operator reset is a recovery action, so it carries the same
            // blast radius as the account holder changing their own password:
            // update the hash and run the full lockdown in one savepoint, so a
            // failure anywhere leaves the old password and the old credentials
            // both intact rather than a half-locked account.
            //
            // There is no realtime hub in the direct-DB CLI to push a
            // revocation from. Other processes notice on their own: web
            // sockets revalidate on their periodic tick, and a long-running
            // stdio MCP session revalidates its token on every tool call.
            db::queries::savepoint(&conn, "cli_set_password", || {
                db::queries::users::update_password(&conn, user.id, &pw)?;
                db::queries::users::lock_down_account(&conn, user.id)
            })?;

            if json {
                let out = serde_json::json!({
                    "username": user.username,
                    "password_set": true,
                    "sessions_cleared": true,
                    "credentials_revoked": true,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                ui::step(format!(
                    "Password updated for '{}' {}",
                    user.username,
                    ui::dim(
                        "(all sessions signed out; API keys, OAuth sessions and connected tools \
                         owned by this account revoked; they must be reconnected)"
                    )
                ));
            }
        }
        UserAction::Promote { username } => {
            let conn = pool.write()?;
            db::queries::users::set_admin(&conn, &username, true)?;
            if json {
                let out = serde_json::json!({ "promoted": username });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                ui::step(format!("Promoted '{username}' to admin."));
            }
        }
        UserAction::Demote { username } => {
            let conn = pool.write()?;
            db::queries::users::set_admin(&conn, &username, false)?;
            if json {
                let out = serde_json::json!({ "demoted": username });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                ui::step(format!("Demoted '{username}' from admin."));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    /// Drive the real `lific user` entrypoint against a scratch database.
    fn run_user(dir: &tempfile::TempDir, action: UserAction) {
        let mut cfg = Config::default();
        cfg.database.path = dir.path().join("lific.db");
        run(&cfg, action, true).expect("user command");
    }

    /// `lific user set-password` is the operator's recovery door, so it carries
    /// the same blast radius as the account holder changing their own password:
    /// sessions, keys and OAuth tokens for the user *and* the tools they own,
    /// all in one write with the new password hash.
    #[test]
    fn set_password_locks_the_account_down() {
        let dir = tempfile::tempdir().expect("scratch dir");
        run_user(
            &dir,
            UserAction::Create {
                username: "blake".into(),
                email: "blake@test.local".into(),
                password: Some("original password".into()),
                admin: true,
                bot: false,
            },
        );

        let pool = db::open(&dir.path().join("lific.db")).expect("open db");
        let (user_id, bot_id) = {
            let conn = pool.write().unwrap();
            let user = db::queries::users::get_user_by_username(&conn, "blake").unwrap();
            let bot = db::queries::users::create_bot_user(
                &conn,
                user.id,
                "opencode-blake",
                "OpenCode",
                Some("opencode"),
            )
            .unwrap();
            db::queries::users::create_session(&conn, user.id, None).unwrap();
            conn.execute(
                "INSERT INTO api_keys (name, key_hash, user_id) VALUES
                 ('blake-key', 'hash-human', ?1),
                 ('bot-key', 'hash-bot', ?2),
                 ('operator', 'hash-operator', NULL)",
                params![user.id, bot.id],
            )
            .unwrap();
            (user.id, bot.id)
        };
        drop(pool);

        run_user(
            &dir,
            UserAction::SetPassword {
                username: "blake".into(),
                password: Some("reset by operator".into()),
            },
        );

        let pool = db::open(&dir.path().join("lific.db")).expect("reopen db");
        let conn = pool.read().unwrap();
        let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };

        assert_eq!(count("SELECT COUNT(*) FROM sessions"), 0);
        assert_eq!(
            count("SELECT COUNT(*) FROM api_keys WHERE revoked = 0 AND user_id IS NOT NULL"),
            0,
            "the human's key and the owned bot's key are both revoked"
        );
        assert_eq!(
            count("SELECT revoked FROM api_keys WHERE name = 'operator'"),
            0,
            "the unbound operator key names nobody and survives"
        );

        // The bot identity survives so the tool shows as disconnected and can
        // be reconnected, rather than disappearing.
        assert!(db::queries::users::get_user_by_id(&conn, bot_id).is_ok());

        let user = db::queries::users::get_user_by_id(&conn, user_id).unwrap();
        assert!(
            db::queries::users::verify_password("reset by operator", &user.password_hash).unwrap()
        );
    }
}
