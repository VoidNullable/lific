use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub identifier: String,
    pub description: String,
    pub emoji: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateProject {
    pub name: String,
    pub identifier: String,
    #[serde(default)]
    pub description: String,
    pub emoji: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateProject {
    pub name: Option<String>,
    pub identifier: Option<String>,
    pub description: Option<String>,
    pub emoji: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: i64,
    pub project_id: i64,
    pub sequence: i64,
    /// Computed: "{project.identifier}-{sequence}"
    pub identifier: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: String,
    pub module_id: Option<i64>,
    pub sort_order: f64,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Labels attached to this issue (populated on read)
    #[serde(default)]
    pub labels: Vec<String>,
    /// Relations (populated on read for get_issue)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relates_to: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateIssue {
    pub project_id: i64,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default = "default_priority")]
    pub priority: String,
    pub module_id: Option<i64>,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateIssue {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub module_id: Option<i64>,
    pub sort_order: Option<f64>,
    pub start_date: Option<String>,
    pub target_date: Option<String>,
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct ListIssuesQuery {
    pub project_id: Option<i64>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub module_id: Option<i64>,
    pub label: Option<String>,
    pub workable: Option<bool>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

fn default_status() -> String {
    "backlog".to_string()
}

fn default_priority() -> String {
    "none".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Module {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub description: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateModule {
    pub project_id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_module_status")]
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateModule {
    pub name: Option<String>,
    pub description: Option<String>,
    pub status: Option<String>,
}

fn default_module_status() -> String {
    "active".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub color: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateLabel {
    pub project_id: i64,
    pub name: String,
    #[serde(default = "default_label_color")]
    pub color: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateLabel {
    pub name: Option<String>,
    pub color: Option<String>,
}

fn default_label_color() -> String {
    "#6B7280".to_string()
}

#[derive(Debug, Deserialize)]
pub struct UpdateFolder {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page {
    pub id: i64,
    pub project_id: Option<i64>,
    pub sequence: Option<i64>,
    /// Computed: "{project.identifier}-DOC-{sequence}"
    pub identifier: String,
    pub folder_id: Option<i64>,
    pub title: String,
    pub content: String,
    pub sort_order: f64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreatePage {
    pub project_id: Option<i64>,
    pub folder_id: Option<i64>,
    pub title: String,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdatePage {
    pub title: Option<String>,
    pub content: Option<String>,
    pub folder_id: Option<i64>,
    pub sort_order: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Folder {
    pub id: i64,
    pub project_id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub sort_order: f64,
}

#[derive(Debug, Deserialize)]
pub struct CreateFolder {
    pub project_id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
}

// ── Users & Sessions ─────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub email: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub display_name: String,
    pub is_admin: bool,
    pub is_bot: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateUser {
    pub username: String,
    pub email: String,
    pub password: String,
    pub display_name: Option<String>,
    #[serde(default)]
    pub is_admin: bool,
    #[serde(default)]
    pub is_bot: bool,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    /// Accepts either username or email
    pub identity: String,
    pub password: String,
}

/// Lightweight user identity extracted from auth middleware.
/// Inserted into request extensions after token resolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub is_admin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub user_id: i64,
    pub expires_at: String,
    pub created_at: String,
}

// ── API Key (user-facing) ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserApiKey {
    pub id: i64,
    pub name: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub revoked: bool,
}

// ── Comments ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: i64,
    pub issue_id: i64,
    pub user_id: i64,
    /// Author username (joined from users table on read)
    pub author: String,
    /// Author display name (joined from users table on read)
    pub author_display_name: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateComment {
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateComment {
    pub content: String,
}

// ── Search ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    pub project_id: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub result_type: String,
    pub id: i64,
    pub identifier: Option<String>,
    pub title: String,
    pub snippet: String,
    pub project_id: Option<i64>,
}
