//! `lific member <action>`: project membership from the CLI.
//!
//! LIF-290: with authorization enforcement on, this is how an operator grants
//! a new user access to existing projects without touching the web UI.

use crate::cli::{MemberAction, term, ui};
use crate::config::Config;
use crate::db;
use crate::error;

pub fn run(
    cfg: &Config,
    action: MemberAction,
    json_flag: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let json = term::wants_json(json_flag);
    let pool = db::open(&cfg.database.path)?;

    match action {
        MemberAction::List { project } => {
            let conn = pool.read()?;
            let pid = db::queries::resolve_project_identifier(&conn, &project)?;
            let members = db::queries::members::list_members_with_users(&conn, pid)?;

            if json {
                let out: Vec<_> = members
                    .iter()
                    .map(|m| {
                        serde_json::json!({
                            "username": m.username,
                            "display_name": m.display_name,
                            "role": m.role.as_str(),
                            "since": m.created_at,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else if members.is_empty() {
                println!(
                    "No members on '{project}'. Grant access with `lific member add \
                     --project {project} --user <name>`."
                );
            } else {
                println!("{} member(s) of {}:", members.len(), project);
                for m in &members {
                    println!("  {} | {} | since {}", m.username, m.role, m.created_at);
                }
            }
        }
        MemberAction::Add {
            project,
            user,
            role,
            all,
        } => {
            let conn = pool.write()?;
            let u = db::queries::users::get_user_by_username(&conn, &user)?;

            if all {
                // Grant on every project; existing memberships are
                // skipped, never overwritten (a role change is
                // `member role`'s explicit job).
                let mut granted: Vec<String> = Vec::new();
                let mut skipped: Vec<String> = Vec::new();
                for p in db::queries::list_projects(&conn)? {
                    match db::queries::members::add_member(&conn, p.id, u.id, &role) {
                        Ok(_) => granted.push(p.identifier),
                        Err(error::LificError::Conflict(_)) => skipped.push(p.identifier),
                        Err(e) => return Err(e.into()),
                    }
                }
                if json {
                    let out = serde_json::json!({
                        "user": u.username,
                        "role": role,
                        "granted": granted,
                        "already_member": skipped,
                    });
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    ui::step(format!(
                        "Granted '{}' {role} access to {} project(s){}",
                        u.username,
                        granted.len(),
                        if skipped.is_empty() {
                            String::new()
                        } else {
                            format!(" ({} already a member: {})", skipped.len(), skipped.join(", "))
                        }
                    ));
                }
            } else {
                // clap guarantees project is present when --all is absent.
                let ident = project.expect("clap: --project required unless --all");
                let pid = db::queries::resolve_project_identifier(&conn, &ident)?;
                let member = db::queries::members::add_member(&conn, pid, u.id, &role)?;
                if json {
                    let out = serde_json::json!({
                        "project": ident,
                        "user": u.username,
                        "role": member.role.as_str(),
                    });
                    println!("{}", serde_json::to_string_pretty(&out)?);
                } else {
                    ui::step(format!(
                        "Added '{}' to {ident} as {}",
                        u.username,
                        member.role
                    ));
                }
            }
        }
        MemberAction::Role {
            project,
            user,
            role,
        } => {
            let conn = pool.write()?;
            let u = db::queries::users::get_user_by_username(&conn, &user)?;
            let pid = db::queries::resolve_project_identifier(&conn, &project)?;
            let member = db::queries::members::change_role(&conn, pid, u.id, &role)?;
            if json {
                let out = serde_json::json!({
                    "project": project,
                    "user": u.username,
                    "role": member.role.as_str(),
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                ui::step(format!(
                    "'{}' is now {} on {project}",
                    u.username, member.role
                ));
            }
        }
        MemberAction::Remove { project, user } => {
            let conn = pool.write()?;
            let u = db::queries::users::get_user_by_username(&conn, &user)?;
            let pid = db::queries::resolve_project_identifier(&conn, &project)?;
            db::queries::members::remove_member_guarded(&conn, pid, u.id)?;
            if json {
                let out = serde_json::json!({
                    "project": project,
                    "user": u.username,
                    "removed": true,
                });
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                ui::step(format!("Removed '{}' from {project}", u.username));
            }
        }
    }
    Ok(())
}
