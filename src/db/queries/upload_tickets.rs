//! One-time upload URLs (`upload_tickets`). See migration 069.

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::models::AttachmentEntity;
use crate::error::LificError;

/// What a ticket authorizes: one file of exactly `size_bytes`, uploaded as
/// `user_id`, linked to `link` when present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UploadTicket {
    pub(crate) user_id: i64,
    pub(crate) filename: String,
    pub(crate) size_bytes: i64,
    pub(crate) link: Option<(AttachmentEntity, i64)>,
    pub(crate) alt_text: Option<String>,
}

/// Most unexpired tickets one user may hold at once.
pub(crate) const MAX_OPEN_TICKETS_PER_USER: i64 = 20;

/// Store a ticket valid for `ttl_seconds` and return its expiry as stored
/// (`YYYY-MM-DD HH:MM:SS`, UTC). Expired tickets are swept first.
pub(crate) fn create(
    conn: &Connection,
    token_hash: &str,
    ticket: &UploadTicket,
    ttl_seconds: i64,
) -> Result<String, LificError> {
    conn.execute(
        "DELETE FROM upload_tickets WHERE expires_at <= datetime('now')",
        [],
    )?;
    let open: i64 = conn.query_row(
        "SELECT COUNT(*) FROM upload_tickets WHERE user_id = ?1",
        [ticket.user_id],
        |row| row.get(0),
    )?;
    if open >= MAX_OPEN_TICKETS_PER_USER {
        return Err(LificError::BadRequest(format!(
            "{open} upload tickets are already open; use or let them expire first"
        )));
    }
    let expires_at: String = conn.query_row(
        "INSERT INTO upload_tickets
             (token_hash, user_id, filename, size_bytes, entity_type, entity_id, alt_text, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now', ?8))
         RETURNING expires_at",
        params![
            token_hash,
            ticket.user_id,
            ticket.filename,
            ticket.size_bytes,
            ticket.link.map(|(entity, _)| entity.as_str()),
            ticket.link.map(|(_, id)| id),
            ticket.alt_text,
            format!("{ttl_seconds:+} seconds"),
        ],
        |row| row.get(0),
    )?;
    Ok(expires_at)
}

/// Use up the ticket with this token hash. Deleting is the claim, so a token
/// works once even under concurrent requests. `None` when there is no such
/// ticket or it has expired.
pub(crate) fn claim(
    conn: &Connection,
    token_hash: &str,
) -> Result<Option<UploadTicket>, LificError> {
    let row = conn
        .query_row(
            "DELETE FROM upload_tickets WHERE token_hash = ?1
             RETURNING user_id, filename, size_bytes, entity_type, entity_id, alt_text,
                       expires_at > datetime('now')",
            [token_hash],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, bool>(6)?,
                ))
            },
        )
        .optional()?;
    let Some((user_id, filename, size_bytes, entity_type, entity_id, alt_text, live)) = row else {
        return Ok(None);
    };
    if !live {
        return Ok(None);
    }
    let link = match (entity_type, entity_id) {
        (Some(entity), Some(id)) => Some((
            entity
                .parse::<AttachmentEntity>()
                .map_err(|_| LificError::Internal(format!("bad ticket entity type {entity}")))?,
            id,
        )),
        _ => None,
    };
    Ok(Some(UploadTicket {
        user_id,
        filename,
        size_bytes,
        link,
        alt_text,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn_with_user() -> (crate::db::DbPool, i64) {
        let pool = crate::db::open_memory().unwrap();
        let id = {
            let conn = pool.write().unwrap();
            crate::db::queries::users::create_passwordless_admin(&conn, "Owner")
                .unwrap()
                .id
        };
        (pool, id)
    }

    fn ticket(user_id: i64) -> UploadTicket {
        UploadTicket {
            user_id,
            filename: "a.png".into(),
            size_bytes: 10,
            link: Some((AttachmentEntity::Issue, 4)),
            alt_text: Some("alt".into()),
        }
    }

    #[test]
    fn a_ticket_is_claimed_once() {
        let (pool, user) = conn_with_user();
        let conn = pool.write().unwrap();
        create(&conn, "h1", &ticket(user), 600).unwrap();
        assert_eq!(claim(&conn, "h1").unwrap(), Some(ticket(user)));
        assert_eq!(claim(&conn, "h1").unwrap(), None);
    }

    #[test]
    fn an_expired_ticket_cannot_be_claimed() {
        let (pool, user) = conn_with_user();
        let conn = pool.write().unwrap();
        create(&conn, "h1", &ticket(user), -1).unwrap();
        assert_eq!(claim(&conn, "h1").unwrap(), None);
    }

    #[test]
    fn open_tickets_per_user_are_capped_and_expired_ones_do_not_count() {
        let (pool, user) = conn_with_user();
        let conn = pool.write().unwrap();
        create(&conn, "old", &ticket(user), -1).unwrap();
        for n in 0..MAX_OPEN_TICKETS_PER_USER {
            create(&conn, &format!("h{n}"), &ticket(user), 600).unwrap();
        }
        let error = create(&conn, "one-more", &ticket(user), 600).unwrap_err();
        assert!(error.to_string().contains("already open"), "{error}");
    }
}
