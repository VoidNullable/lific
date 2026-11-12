//! `PUT /api/uploads/{token}`: the receiving end of an MCP upload ticket.
//!
//! An HTTP MCP client cannot hand Lific a local file, and base64 in tool
//! arguments makes the model write out every byte. `upload_attachment` with a
//! `size` and no content issues a ticket instead: a secret URL that accepts one
//! `PUT` of exactly that many bytes, for a few minutes, and creates the
//! attachment as the agent that asked for it. The token in the URL is the only
//! credential, so this route sits outside the auth middleware, and the ticket
//! is used up before its body is read.
//!
//! The shapes follow the experimental `io.modelcontextprotocol/files`
//! extension's `UploadTarget` (`url`, `method`, `credentials: "omit"`,
//! `expiresAt`), so adopting the extension later changes how the target is
//! delivered, not what it is.

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::put,
};
use rand::{RngCore, rngs::OsRng};
use sha2::{Digest, Sha256};

use super::attachments::{
    NewAttachment, UploadResponse, attachment_markdown, authorize_link, declared_mime_for_filename,
    store_upload,
};
use crate::db::{DbPool, models::AuthUser, queries};
use crate::error::LificError;
use crate::realtime::RealtimeHub;
use crate::storage::{self, AttachmentStore};

/// How long a ticket stays usable.
pub(crate) const TICKET_TTL_SECONDS: i64 = 10 * 60;

/// The path a ticket's URL points at, relative to the server's origin.
pub(crate) fn upload_path(token: &str) -> String {
    format!("/api/uploads/{token}")
}

/// A fresh ticket token: 256 bits from the OS, URL-safe.
pub(crate) fn new_token() -> Result<String, LificError> {
    use base64::Engine;
    let mut entropy = [0u8; 32];
    OsRng
        .try_fill_bytes(&mut entropy)
        .map_err(|e| LificError::Internal(format!("token generation failed: {e}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(entropy))
}

/// What the database keeps instead of the token.
pub(crate) fn hash_token(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

#[derive(Clone)]
struct TicketState {
    db: DbPool,
    store: AttachmentStore,
    realtime: RealtimeHub,
}

/// The upload response, plus the Markdown an agent embeds.
#[derive(serde::Serialize)]
struct TicketUploadResponse {
    #[serde(flatten)]
    upload: UploadResponse,
    markdown: String,
}

pub(crate) fn router(db: DbPool, store: AttachmentStore, realtime: RealtimeHub) -> Router {
    Router::new()
        .route("/api/uploads/{token}", put(upload_with_ticket))
        .with_state(TicketState {
            db,
            store,
            realtime,
        })
}

async fn upload_with_ticket(
    State(state): State<TicketState>,
    Path(token): Path<String>,
    body: Body,
) -> Result<Response, LificError> {
    let hash = hash_token(&token);
    let ticket = state
        .db
        .transaction(|conn| queries::upload_tickets::claim(conn, &hash))?
        .ok_or_else(|| {
            LificError::NotFound("upload ticket not found, already used, or expired".into())
        })?;

    let expected = usize::try_from(ticket.size_bytes).unwrap_or(0);
    let bytes = axum::body::to_bytes(body, expected).await.map_err(|_| {
        LificError::PayloadTooLarge(format!(
            "this ticket accepts exactly {expected} bytes; the upload was larger"
        ))
    })?;
    if bytes.len() != expected {
        return Err(LificError::BadRequest(format!(
            "this ticket accepts exactly {expected} bytes; the upload had {}",
            bytes.len()
        )));
    }

    // The ticket's issuer, as they are now: a deactivated account or a lost
    // role stops the upload even though the ticket was valid when issued.
    let user = {
        let conn = state.db.read()?;
        queries::users::get_user_by_id(&conn, ticket.user_id)?
    };
    if !user.is_active {
        return Err(LificError::Forbidden(
            "the account that requested this upload is deactivated".into(),
        ));
    }
    let identity = Some(crate::resolve_caller::ResolvedIdentity {
        user: AuthUser {
            id: user.id,
            username: user.username,
            display_name: user.display_name,
            is_admin: user.is_admin,
        },
        transport: crate::actor::Transport::Mcp,
    });
    if let Some((entity, entity_id)) = ticket.link {
        authorize_link(&state.db, &identity, entity, entity_id)?;
    }

    let mime = storage::sniff_and_validate(&bytes, declared_mime_for_filename(&ticket.filename))?;
    if !storage::ALLOWED_MIMES.contains(&mime.as_str()) {
        return Err(LificError::BadRequest(format!(
            "rejected: '{mime}' is not an allowed file type"
        )));
    }

    let actor = crate::actor::ActorCtx {
        user_id: Some(user.id),
        transport: crate::actor::Transport::Mcp,
    };
    let upload = NewAttachment {
        filename: &ticket.filename,
        mime: &mime,
        bytes: &bytes,
        uploader: user.id,
        alt_text: ticket.alt_text.as_deref(),
        link: ticket.link,
    };
    let (attachment, event) = crate::actor::scope(actor, async {
        store_upload(&state.db, &state.store, &identity, &upload)
    })
    .await?;
    if let Some(event) = event {
        state.realtime.send(event);
    }

    let label = attachment
        .alt_text
        .clone()
        .unwrap_or_else(|| attachment.filename.clone());
    let markdown = attachment_markdown(&label, &attachment.mime, attachment.id);
    Ok((
        StatusCode::CREATED,
        Json(TicketUploadResponse {
            upload: UploadResponse::from(attachment),
            markdown,
        }),
    )
        .into_response())
}
