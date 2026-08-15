//! `lific key <action>`: mint, list, revoke, rotate, and assign API keys.

use crate::auth;
use crate::cli::{KeyAction, term, ui};
use crate::config::Config;
use crate::db;

pub fn run(
    cfg: &Config,
    action: KeyAction,
    json_flag: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = term::wants_json(json_flag);
    let pool = db::open(&cfg.database.path)?;
    let manager = auth::create_key_manager().map_err(|e| format!("key manager init failed: {e}"))?;

    match action {
        KeyAction::Create {
            name,
            user,
            expires,
        } => {
            let key =
                auth::create_api_key_with_expiry(&pool, &manager, &name, expires.as_deref())?;

            // If --user was provided, assign the key to that user
            let assigned = if let Some(ref username) = user {
                let conn = pool.read()?;
                let u = db::queries::users::get_user_by_username(&conn, username)?;
                drop(conn);
                let conn = pool.write()?;
                db::queries::users::assign_key_to_user(&conn, &name, u.id)?;
                Some(username.clone())
            } else {
                None
            };

            if json {
                let out = serde_json::json!({
                    "name": name,
                    "key": key,
                    "user": assigned,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                let title = if let Some(ref username) = assigned {
                    format!("API key '{name}' created (assigned to {username}) — save it now, it will not be shown again")
                } else {
                    format!("API key '{name}' created — save it now, it will not be shown again")
                };
                ui::note(
                    title,
                    format!("{key}\n\nUse it as: Authorization: Bearer <key>"),
                );
            }
        }
        KeyAction::List => {
            let keys = auth::list_api_keys(&pool)?;
            if json {
                let out: Vec<_> = keys
                    .iter()
                    .map(|k| {
                        serde_json::json!({
                            "name": k.name,
                            "revoked": k.revoked,
                            "created_at": k.created_at,
                            "expires_at": k.expires_at,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else if keys.is_empty() {
                println!("No API keys configured.");
            } else {
                println!("{} API key(s):", keys.len());
                for k in &keys {
                    let status = if k.revoked { "REVOKED" } else { "active" };
                    let expiry = k.expires_at.as_deref().unwrap_or("never");
                    println!(
                        "  {} | {} | created {} | expires {}",
                        k.name, status, k.created_at, expiry
                    );
                }
            }
        }
        KeyAction::Revoke { name } => {
            auth::revoke_api_key(&pool, &name)?;
            if json {
                let out = serde_json::json!({ "revoked": name });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                ui::step(format!("Revoked key '{name}'"));
            }
        }
        KeyAction::Rotate { name } => {
            let key = auth::rotate_api_key(&pool, &manager, &name)?;
            if json {
                let out = serde_json::json!({ "name": name, "key": key });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                ui::note(
                    format!("Key '{name}' rotated — save it now, it will not be shown again"),
                    &key,
                );
            }
        }
        KeyAction::Assign { name, user } => {
            let conn = pool.read()?;
            let u = db::queries::users::get_user_by_username(&conn, &user)?;
            drop(conn);
            let conn = pool.write()?;
            db::queries::users::assign_key_to_user(&conn, &name, u.id)?;
            if json {
                let out = serde_json::json!({ "name": name, "user": user });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                ui::step(format!("Assigned key '{name}' to user '{user}'"));
            }
        }
    }
    Ok(())
}
