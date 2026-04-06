use axum::extract::{Json, Path, Query, State};

use crate::db::{DbPool, models::*};
use crate::error::LificError;

use super::{with_read, with_write};

#[derive(serde::Deserialize)]
pub(super) struct PageQuery {
    project_id: Option<i64>,
    folder_id: Option<i64>,
}

pub(super) async fn list_pages_handler(
    State(db): State<DbPool>,
    Query(q): Query<PageQuery>,
) -> Result<Json<Vec<Page>>, LificError> {
    with_read(&db, |conn| {
        crate::db::queries::list_pages(conn, q.project_id, q.folder_id)
    })
    .map(Json)
}

pub(super) async fn get_page(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<Page>, LificError> {
    with_read(&db, |conn| crate::db::queries::get_page(conn, id)).map(Json)
}

pub(super) async fn create_page(
    State(db): State<DbPool>,
    Json(input): Json<CreatePage>,
) -> Result<Json<Page>, LificError> {
    with_write(&db, |conn| crate::db::queries::create_page(conn, &input)).map(Json)
}

pub(super) async fn update_page(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
    Json(input): Json<UpdatePage>,
) -> Result<Json<Page>, LificError> {
    with_write(&db, |conn| {
        crate::db::queries::update_page(conn, id, &input)
    })
    .map(Json)
}

pub(super) async fn delete_page_handler(
    State(db): State<DbPool>,
    Path(id): Path<i64>,
) -> Result<Json<serde_json::Value>, LificError> {
    with_write(&db, |conn| crate::db::queries::delete_page(conn, id))?;
    Ok(Json(serde_json::json!({"deleted": true})))
}
