use super::{Check, Status};
use crate::cli::connect::{
    PATH_TOKEN_GUIDANCE,
    clients::{self, Format, PathBase, Scope},
    is_path_token_url, production_base, writer,
};

pub(super) fn check() -> Vec<Check> {
    match production_base() {
        Ok(base) => check_at(&base),
        Err(_) => vec![Check::new(
            "client_config",
            Status::Warn,
            "could not locate client configuration directories",
        )],
    }
}

fn check_at(base: &PathBase) -> Vec<Check> {
    let mut checks = Vec::new();
    let mut visited = std::collections::HashSet::new();
    for client in clients::all_clients() {
        for scope in [Scope::Global, Scope::Project] {
            let Some(path) = client.path_for(base, scope) else {
                continue;
            };
            if !visited.insert(path.clone()) {
                continue;
            }
            let contents = match writer::config_contents(&path) {
                Ok(Some(contents)) => contents,
                Ok(None) => continue,
                Err(_) => {
                    checks.push(Check::new(
                        "client_config",
                        Status::Warn,
                        format!(
                            "could not read {} configuration at {}",
                            client.display,
                            path.display()
                        ),
                    ));
                    continue;
                }
            };
            // Parse errors can include source lines containing secrets.
            let value = match parse(&contents, client.format) {
                Some(value) => value,
                None => {
                    checks.push(Check::new(
                        "client_config",
                        Status::Warn,
                        format!(
                            "could not inspect {} configuration at {}; check MCP URLs manually",
                            client.display,
                            path.display()
                        ),
                    ));
                    continue;
                }
            };
            let entry = client.compile(&clients::ServerConfig::oauth_remote(
                "https://example.test/mcp",
            ));
            let server_map = entry.top_key.split('.').next().unwrap();
            if has_removed_server_url(&value, server_map) {
                checks.push(Check::new(
                    "client_config",
                    Status::Fail,
                    format!(
                        "{} configuration at {} uses a removed MCP URL. {PATH_TOKEN_GUIDANCE}",
                        client.display,
                        path.display()
                    ),
                ));
            }
        }
    }
    checks
}

fn has_removed_server_url(value: &serde_json::Value, server_map: &str) -> bool {
    match value {
        serde_json::Value::Object(values) => values.iter().any(|(name, value)| {
            if name == server_map {
                has_path_token(value)
            } else {
                has_removed_server_url(value, server_map)
            }
        }),
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| has_removed_server_url(value, server_map)),
        _ => false,
    }
}

fn parse(contents: &str, format: Format) -> Option<serde_json::Value> {
    match format {
        Format::Json => serde_json::from_str(contents).ok(),
        Format::Toml => toml::from_str::<toml::Value>(contents)
            .ok()
            .and_then(|value| serde_json::to_value(value).ok()),
        Format::Yaml => serde_yaml::from_str(contents).ok(),
    }
}

fn has_path_token(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(value) => is_path_token_url(value),
        serde_json::Value::Array(values) => values.iter().any(has_path_token),
        serde_json::Value::Object(values) => values.values().any(has_path_token),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_nested_client_projects_but_not_unrelated_settings() {
        assert!(!has_removed_server_url(
            &serde_json::json!({
                "documentation": "https://docs.example/mcp/tools",
                "mcpServers": {"lific": {"url": "https://example.test/mcp"}}
            }),
            "mcpServers"
        ));
        assert!(has_removed_server_url(
            &serde_json::json!({
                "projects": {"/checkout": {"mcpServers": {
                    "custom-name": {"url": "https://example.test/mcp/old-token"}
                }}}
            }),
            "mcpServers"
        ));
    }

    #[test]
    fn detects_old_urls_in_every_client_format_without_disclosing_the_secret() {
        let dir = tempfile::tempdir().unwrap();
        let base = PathBase {
            home: dir.path().join("home"),
            project: dir.path().join("project"),
            os: clients::Os::Linux,
            appdata: None,
        };
        for client in clients::all_clients() {
            for scope in [Scope::Global, Scope::Project] {
                let Some(path) = client.path_for(&base, scope) else {
                    continue;
                };
                let entry = client.compile(&clients::ServerConfig::remote(
                    "https://example.test/mcp/SECRET_PATH_TOKEN",
                    "header-secret",
                ));
                writer::write(&path, client.format, &entry).unwrap();
            }
        }
        let checks = check_at(&base);
        let expected = clients::all_clients()
            .iter()
            .map(|client| {
                [Scope::Global, Scope::Project]
                    .into_iter()
                    .filter(|scope| client.path_for(&base, *scope).is_some())
                    .count()
            })
            .sum::<usize>();
        assert_eq!(checks.len(), expected);
        for check in checks {
            assert_eq!(check.status, Status::Fail, "{check:?}");
            assert!(check.detail.contains("Authorization"));
            assert!(check.detail.contains("OAuth"));
            assert!(!check.detail.contains("SECRET_PATH_TOKEN"));
            assert!(!check.detail.contains("header-secret"));
        }
    }

    #[test]
    fn accepts_header_oauth_and_stdio_client_configs() {
        let dir = tempfile::tempdir().unwrap();
        let base = PathBase {
            home: dir.path().join("home"),
            project: dir.path().join("project"),
            os: clients::Os::Linux,
            appdata: None,
        };
        for config in [
            clients::ServerConfig::remote("https://example.test/mcp", "header-secret"),
            clients::ServerConfig::oauth_remote("https://example.test/mcp"),
            clients::ServerConfig::stdio("/instance/lific.db"),
        ] {
            for client in clients::all_clients() {
                let Some(path) = client.path_for(&base, Scope::Global) else {
                    continue;
                };
                writer::write(&path, client.format, &client.compile(&config)).unwrap();
            }
            assert!(check_at(&base).is_empty());
        }
    }
}
