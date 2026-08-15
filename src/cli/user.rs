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
            db::queries::users::update_password(&conn, user.id, &pw)?;
            // LIF-205 semantics: any password change signs out every
            // existing session — a reset must not leave a possibly
            // hijacked session alive.
            db::queries::users::delete_all_sessions(&conn, user.id)?;

            if json {
                let out = serde_json::json!({
                    "username": user.username,
                    "password_set": true,
                    "sessions_cleared": true,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                ui::step(format!(
                    "Password updated for '{}' {}",
                    user.username,
                    ui::dim("(all sessions signed out)")
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
