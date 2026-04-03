use std::collections::HashMap;
use std::path::Path;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::db::DbPool;
use crate::error::LificError;

#[derive(Debug, Deserialize, Serialize)]
pub struct PlaneExport {
    projects: Vec<PlaneProject>,
    states: Vec<PlaneState>,
    modules: Vec<PlaneModule>,
    labels: Vec<PlaneLabel>,
    issues: Vec<PlaneIssue>,
    issue_labels: Option<Vec<PlaneIssueLabel>>,
    module_issues: Option<Vec<PlaneModuleIssue>>,
    relations: Option<Vec<PlaneRelation>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlaneProject {
    id: String,
    name: String,
    identifier: String,
    description: Option<String>,
    emoji: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlaneState {
    id: String,
    #[allow(dead_code)]
    name: String,
    group: String,
    #[allow(dead_code)]
    project_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlaneModule {
    id: String,
    name: String,
    description: Option<String>,
    status: Option<String>,
    project_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlaneLabel {
    id: String,
    name: String,
    color: Option<String>,
    project_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlaneIssue {
    id: String,
    name: String,
    description_stripped: Option<String>,
    state_id: Option<String>,
    priority: Option<String>,
    #[allow(dead_code)]
    sequence_id: Option<i64>,
    sort_order: Option<f64>,
    start_date: Option<String>,
    target_date: Option<String>,
    project_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlaneIssueLabel {
    issue_id: String,
    label_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlaneModuleIssue {
    issue_id: String,
    module_id: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlaneRelation {
    issue_id: String,
    related_issue_id: String,
    relation_type: String,
}

fn map_state_group(group: &str) -> &str {
    match group {
        "backlog" => "backlog",
        "unstarted" => "todo",
        "started" => "active",
        "completed" => "done",
        "cancelled" => "cancelled",
        _ => "backlog",
    }
}

fn map_relation_type(plane_type: &str) -> Option<&str> {
    match plane_type {
        "blocked_by" => Some("blocks"), // reversed: if A blocked_by B, then B blocks A
        "relates_to" => Some("relates_to"),
        "duplicate" => Some("duplicate"),
        _ => None,
    }
}

pub fn run_import(
    pool: &DbPool,
    export_path: &Path,
    skip_identifiers: &[String],
) -> Result<(), LificError> {
    let raw = std::fs::read_to_string(export_path)
        .map_err(|e| LificError::Internal(format!("failed to read export file: {e}")))?;
    let export: PlaneExport = serde_json::from_str(&raw)
        .map_err(|e| LificError::Internal(format!("failed to parse export: {e}")))?;

    info!(
        projects = export.projects.len(),
        issues = export.issues.len(),
        modules = export.modules.len(),
        labels = export.labels.len(),
        "loaded Plane export"
    );

    // Build state → status mapping
    let state_map: HashMap<String, String> = export
        .states
        .iter()
        .map(|s| (s.id.clone(), map_state_group(&s.group).to_string()))
        .collect();

    // Build issue → module mapping from module_issues junction
    let issue_module_map: HashMap<String, String> = export
        .module_issues
        .as_ref()
        .map(|mi| {
            mi.iter()
                .map(|m| (m.issue_id.clone(), m.module_id.clone()))
                .collect()
        })
        .unwrap_or_default();

    // Build issue → label IDs mapping
    let mut issue_label_map: HashMap<String, Vec<String>> = HashMap::new();
    if let Some(ref ils) = export.issue_labels {
        for il in ils {
            issue_label_map
                .entry(il.issue_id.clone())
                .or_default()
                .push(il.label_id.clone());
        }
    }

    for project in &export.projects {
        if skip_identifiers.contains(&project.identifier) {
            info!(identifier = %project.identifier, "skipping");
            continue;
        }

        let conn = pool.write()?;

        // Skip if already exists
        let existing: Option<i64> = conn
            .query_row(
                "SELECT id FROM projects WHERE identifier = ?1",
                params![project.identifier],
                |row| row.get(0),
            )
            .ok();

        if existing.is_some() {
            warn!(identifier = %project.identifier, "already exists, skipping");
            drop(conn);
            continue;
        }

        conn.execute(
            "INSERT INTO projects (name, identifier, description, emoji) VALUES (?1, ?2, ?3, ?4)",
            params![
                project.name,
                project.identifier,
                project.description.as_deref().unwrap_or(""),
                project.emoji,
            ],
        )?;
        let lific_project_id = conn.last_insert_rowid();
        drop(conn);

        info!(identifier = %project.identifier, name = %project.name, "importing project");

        // Import modules for this project
        let project_modules: Vec<&PlaneModule> = export
            .modules
            .iter()
            .filter(|m| m.project_id == project.id)
            .collect();
        let mut module_id_map: HashMap<String, i64> = HashMap::new();
        {
            let conn = pool.write()?;
            for module in &project_modules {
                let status = match module.status.as_deref() {
                    Some("backlog") => "backlog",
                    Some("planned") => "planned",
                    Some("in-progress") => "active",
                    Some("paused") => "paused",
                    Some("completed") => "done",
                    Some("cancelled") => "cancelled",
                    _ => "active",
                };
                conn.execute(
                    "INSERT INTO modules (project_id, name, description, status) VALUES (?1, ?2, ?3, ?4)",
                    params![lific_project_id, module.name, module.description.as_deref().unwrap_or(""), status],
                )?;
                module_id_map.insert(module.id.clone(), conn.last_insert_rowid());
            }
        }

        // Import labels for this project
        let project_labels: Vec<&PlaneLabel> = export
            .labels
            .iter()
            .filter(|l| l.project_id == project.id)
            .collect();
        let mut label_id_map: HashMap<String, i64> = HashMap::new();
        {
            let conn = pool.write()?;
            for label in &project_labels {
                conn.execute(
                    "INSERT OR IGNORE INTO labels (project_id, name, color) VALUES (?1, ?2, ?3)",
                    params![
                        lific_project_id,
                        label.name,
                        label.color.as_deref().unwrap_or("#6B7280")
                    ],
                )?;
                label_id_map.insert(label.id.clone(), conn.last_insert_rowid());
            }
        }

        // Import issues for this project
        let project_issues: Vec<&PlaneIssue> = export
            .issues
            .iter()
            .filter(|i| i.project_id == project.id)
            .collect();
        let mut issue_id_map: HashMap<String, i64> = HashMap::new();
        {
            let conn = pool.write()?;
            let mut seq = 0i64;
            for issue in &project_issues {
                seq += 1;
                let status = issue
                    .state_id
                    .as_ref()
                    .and_then(|sid| state_map.get(sid))
                    .map(|s| s.as_str())
                    .unwrap_or("backlog");
                let priority = issue.priority.as_deref().unwrap_or("none");
                let module_id = issue_module_map
                    .get(&issue.id)
                    .and_then(|mid| module_id_map.get(mid))
                    .copied();

                conn.execute(
                    "INSERT INTO issues (project_id, sequence, title, description, status, priority, module_id, sort_order, start_date, target_date)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        lific_project_id,
                        seq,
                        issue.name,
                        issue.description_stripped.as_deref().unwrap_or(""),
                        status,
                        priority,
                        module_id,
                        issue.sort_order.unwrap_or(0.0),
                        issue.start_date,
                        issue.target_date,
                    ],
                )?;
                let lific_issue_id = conn.last_insert_rowid();
                issue_id_map.insert(issue.id.clone(), lific_issue_id);

                if let Some(label_ids) = issue_label_map.get(&issue.id) {
                    for lid in label_ids {
                        if let Some(&lific_label_id) = label_id_map.get(lid) {
                            conn.execute(
                                "INSERT OR IGNORE INTO issue_labels (issue_id, label_id) VALUES (?1, ?2)",
                                params![lific_issue_id, lific_label_id],
                            )?;
                        }
                    }
                }
            }
        }

        info!(
            modules = project_modules.len(),
            labels = project_labels.len(),
            issues = project_issues.len(),
            "imported"
        );
    }

    // Import relations (cross-project aware)
    if let Some(ref relations) = export.relations {
        let conn = pool.write()?;
        let mut count = 0;
        // Build a global issue ID map across all projects
        // We already have per-project maps, but need a combined one
        // Actually we need to rebuild since issue_id_map was per-project and dropped
        // Let's just look up by sequence in the DB

        // Rebuild full plane UUID → lific ID map
        let mut full_issue_map: HashMap<String, i64> = HashMap::new();
        for project in &export.projects {
            if skip_identifiers.contains(&project.identifier) {
                continue;
            }
            let project_issues: Vec<&PlaneIssue> = export
                .issues
                .iter()
                .filter(|i| i.project_id == project.id)
                .collect();
            for (idx, issue) in project_issues.iter().enumerate() {
                // Look up by project identifier + sequence
                let seq = (idx + 1) as i64;
                if let Ok(lific_id) = conn.query_row(
                    "SELECT i.id FROM issues i JOIN projects p ON p.id = i.project_id WHERE p.identifier = ?1 AND i.sequence = ?2",
                    params![project.identifier, seq],
                    |row| row.get::<_, i64>(0),
                ) {
                    full_issue_map.insert(issue.id.clone(), lific_id);
                }
            }
        }

        for rel in relations {
            let lific_type = match map_relation_type(&rel.relation_type) {
                Some(t) => t,
                None => continue,
            };

            let (source, target) = if rel.relation_type == "blocked_by" {
                // Reverse: if issue_id is blocked_by related_issue_id,
                // then related_issue_id blocks issue_id
                (&rel.related_issue_id, &rel.issue_id)
            } else {
                (&rel.issue_id, &rel.related_issue_id)
            };

            if let (Some(&src), Some(&tgt)) =
                (full_issue_map.get(source), full_issue_map.get(target))
            {
                conn.execute(
                    "INSERT OR IGNORE INTO issue_relations (source_id, target_id, relation_type) VALUES (?1, ?2, ?3)",
                    params![src, tgt, lific_type],
                )?;
                count += 1;
            }
        }
        if count > 0 {
            info!(count, "imported relations");
        }
    }

    info!("Plane import complete");
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
struct PaginatedResponse<T> {
    results: Vec<T>,
    next_cursor: Option<String>,
    next_page_results: Option<bool>,
}

/// Fetch all Plane data via the REST API and import it.
pub async fn run_api_import(
    pool: &DbPool,
    base_url: &str,
    api_key: &str,
    workspace: &str,
    skip_identifiers: &[String],
) -> Result<(), LificError> {
    let client = reqwest::Client::builder()
        .default_headers({
            let mut h = reqwest::header::HeaderMap::new();
            h.insert(
                "x-api-key",
                api_key
                    .parse()
                    .map_err(|e| LificError::Internal(format!("{e}")))?,
            );
            h
        })
        .build()
        .map_err(|e| LificError::Internal(e.to_string()))?;

    let base = base_url.trim_end_matches('/');
    let ws = workspace;

    info!("fetching data from Plane API...");

    // Fetch projects
    let projects: Vec<PlaneProject> =
        fetch_paginated(&client, &format!("{base}/api/v1/workspaces/{ws}/projects/")).await?;
    info!(count = projects.len(), "fetched projects");

    let mut all_states = Vec::new();
    let mut all_modules = Vec::new();
    let mut all_labels = Vec::new();
    let mut all_issues = Vec::new();
    let all_issue_labels = Vec::new();
    let mut all_module_issues = Vec::new();
    let mut all_relations = Vec::new();

    for project in &projects {
        if skip_identifiers.contains(&project.identifier) {
            info!(identifier = %project.identifier, "skipping");
            continue;
        }
        info!(identifier = %project.identifier, "fetching project data");
        let pid = &project.id;

        // States
        let states: Vec<PlaneState> = fetch_paginated(
            &client,
            &format!("{base}/api/v1/workspaces/{ws}/projects/{pid}/states/"),
        )
        .await?;
        all_states.extend(states);

        // Modules
        let modules: Vec<PlaneModule> = fetch_paginated(
            &client,
            &format!("{base}/api/v1/workspaces/{ws}/projects/{pid}/modules/"),
        )
        .await?;

        // Module issues (for each module)
        for module in &modules {
            let mi: Vec<PlaneModuleIssue> = match fetch_paginated::<PlaneModuleIssue>(
                &client,
                &format!(
                    "{base}/api/v1/workspaces/{ws}/projects/{pid}/modules/{}/issues/",
                    module.id
                ),
            )
            .await
            {
                Ok(items) => items,
                Err(_) => {
                    // Module issues endpoint might return different format, try raw
                    match fetch_raw_array(
                        &client,
                        &format!(
                            "{base}/api/v1/workspaces/{ws}/projects/{pid}/modules/{}/issues/",
                            module.id
                        ),
                    )
                    .await
                    {
                        Ok(values) => values
                            .iter()
                            .filter_map(|v| {
                                Some(PlaneModuleIssue {
                                    issue_id: v.get("issue").or(v.get("id"))?.as_str()?.to_string(),
                                    module_id: module.id.clone(),
                                })
                            })
                            .collect(),
                        Err(_) => vec![],
                    }
                }
            };
            all_module_issues.extend(mi);
        }
        all_modules.extend(modules);

        // Labels
        let labels: Vec<PlaneLabel> = fetch_paginated(
            &client,
            &format!("{base}/api/v1/workspaces/{ws}/projects/{pid}/labels/"),
        )
        .await?;
        all_labels.extend(labels);

        // Issues
        let issues: Vec<PlaneIssue> = fetch_paginated(
            &client,
            &format!("{base}/api/v1/workspaces/{ws}/projects/{pid}/issues/"),
        )
        .await?;

        // Issue labels (from issue expand or separate junction)
        // Plane issues have a `labels` field with UUIDs
        for _issue in &issues {
            // Try to get labels from issue list endpoint with expand
            // For now, fetch relations per issue
        }

        // Relations (per issue, with delay to avoid rate limits)
        for issue in &issues {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if let Ok(resp) = client
                .get(format!(
                    "{base}/api/v1/workspaces/{ws}/projects/{pid}/issues/{}/relations/",
                    issue.id
                ))
                .send()
                .await
                && let Ok(text) = resp.text().await
                && let Ok(rels) = serde_json::from_str::<serde_json::Value>(&text)
            {
                if let Some(blocking) = rels.get("blocking").and_then(|v| v.as_array()) {
                    for r in blocking {
                        if let Some(rid) = r.get("id").and_then(|v| v.as_str()) {
                            all_relations.push(PlaneRelation {
                                issue_id: issue.id.clone(),
                                related_issue_id: rid.to_string(),
                                relation_type: "blocked_by".to_string(),
                            });
                        }
                    }
                }
                if let Some(relates) = rels.get("relates_to").and_then(|v| v.as_array()) {
                    for r in relates {
                        if let Some(rid) = r.get("id").and_then(|v| v.as_str()) {
                            all_relations.push(PlaneRelation {
                                issue_id: issue.id.clone(),
                                related_issue_id: rid.to_string(),
                                relation_type: "relates_to".to_string(),
                            });
                        }
                    }
                }
            }
        }

        all_issues.extend(issues);
    }

    info!(
        issues = all_issues.len(),
        modules = all_modules.len(),
        labels = all_labels.len(),
        relations = all_relations.len(),
        "fetched all data, importing..."
    );

    // Build the PlaneExport and use the existing import logic
    let export = PlaneExport {
        projects,
        states: all_states,
        modules: all_modules,
        labels: all_labels,
        issues: all_issues,
        issue_labels: if all_issue_labels.is_empty() {
            None
        } else {
            Some(all_issue_labels)
        },
        module_issues: if all_module_issues.is_empty() {
            None
        } else {
            Some(all_module_issues)
        },
        relations: if all_relations.is_empty() {
            None
        } else {
            Some(all_relations)
        },
    };

    import_export(pool, &export, skip_identifiers)
}

/// Fetch all pages of a paginated Plane API endpoint.
async fn fetch_paginated<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<T>, LificError> {
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut req = client.get(url).query(&[("per_page", "100")]);
        if let Some(ref c) = cursor {
            req = req.query(&[("cursor", c.as_str())]);
        }

        // Small delay between pages to avoid rate limits
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let text = req
            .send()
            .await
            .map_err(|e| LificError::Internal(format!("API request failed: {e}")))?
            .text()
            .await
            .map_err(|e| LificError::Internal(format!("read response: {e}")))?;

        match serde_json::from_str::<PaginatedResponse<T>>(&text) {
            Ok(resp) => {
                let has_more = resp.next_page_results.unwrap_or(false);
                all.extend(resp.results);
                if !has_more {
                    break;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    cursor = v
                        .get("next_cursor")
                        .and_then(|c| c.as_str())
                        .map(|s| s.to_string());
                    if cursor.is_none() {
                        break;
                    }
                } else {
                    break;
                }
            }
            Err(_) => {
                // Try as plain array (some endpoints don't paginate)
                if let Ok(items) = serde_json::from_str::<Vec<T>>(&text) {
                    all.extend(items);
                }
                break;
            }
        }
    }

    Ok(all)
}

/// Fetch raw JSON array for endpoints with non-standard response format.
async fn fetch_raw_array(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<serde_json::Value>, LificError> {
    let text = client
        .get(url)
        .query(&[("per_page", "200")])
        .send()
        .await
        .map_err(|e| LificError::Internal(e.to_string()))?
        .text()
        .await
        .map_err(|e| LificError::Internal(e.to_string()))?;

    if let Ok(paginated) = serde_json::from_str::<PaginatedResponse<serde_json::Value>>(&text) {
        Ok(paginated.results)
    } else if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&text) {
        Ok(arr)
    } else {
        Ok(vec![])
    }
}

/// Shared import logic used by both file-based and API-based import.
fn import_export(
    pool: &DbPool,
    export: &PlaneExport,
    skip_identifiers: &[String],
) -> Result<(), LificError> {
    // This is just the existing run_import logic extracted.
    // For now, serialize to JSON and call run_import via the file path.
    // TODO: refactor to share the core logic directly.
    let json = serde_json::to_string(export)
        .map_err(|e| LificError::Internal(format!("serialize export: {e}")))?;
    let tmp = std::env::temp_dir().join("lific_api_import.json");
    std::fs::write(&tmp, &json)
        .map_err(|e| LificError::Internal(format!("write temp file: {e}")))?;
    let result = run_import(pool, &tmp, skip_identifiers);
    let _ = std::fs::remove_file(&tmp);
    result
}
