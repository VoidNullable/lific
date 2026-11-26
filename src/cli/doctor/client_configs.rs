use super::{Check, Status};
use crate::cli::connect::{
    PATH_TOKEN_GUIDANCE,
    clients::{self, Format, PathBase, Scope},
    production_base, writer,
};
use crate::config::Config;

pub(super) fn check(cfg: &Config) -> Vec<Check> {
    match production_base() {
        Ok(base) => check_at(&base, cfg),
        Err(_) => vec![Check::new(
            "client_config",
            Status::Warn,
            "could not locate client configuration directories",
        )],
    }
}

fn check_at(base: &PathBase, cfg: &Config) -> Vec<Check> {
    let mut checks = Vec::new();
    let local_base = super::connect_base(cfg);
    let origins = [cfg.server.public_url.as_deref(), Some(local_base.as_str())]
        .into_iter()
        .flatten()
        .filter_map(|url| reqwest::Url::parse(url).ok())
        .collect::<Vec<_>>();
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
            if let Some(status) = removed_server_url_status(&value, server_map, &origins) {
                let diagnosis = if status == Status::Fail {
                    "uses a removed Lific MCP URL."
                } else {
                    "has a possible legacy MCP URL; if this entry points at Lific, update its connection."
                };
                checks.push(Check::new(
                    "client_config",
                    status,
                    format!(
                        "{} configuration at {} {diagnosis} {PATH_TOKEN_GUIDANCE}",
                        client.display,
                        path.display()
                    ),
                ));
            }
        }
    }
    checks
}

fn removed_server_url_status(
    value: &serde_json::Value,
    server_map: &str,
    origins: &[reqwest::Url],
) -> Option<Status> {
    match value {
        serde_json::Value::Object(values) => values
            .iter()
            .filter_map(|(name, value)| {
                if name == server_map {
                    value
                        .as_object()?
                        .iter()
                        .filter_map(|(key, entry)| {
                            let named_lific = key.to_ascii_lowercase().contains("lific")
                                || entry
                                    .get("name")
                                    .and_then(|name| name.as_str())
                                    .is_some_and(|name| {
                                        name.to_ascii_lowercase().contains("lific")
                                    });
                            entry_url_status(entry, named_lific, origins)
                        })
                        .max_by_key(|status| *status == Status::Fail)
                } else {
                    removed_server_url_status(value, server_map, origins)
                }
            })
            .max_by_key(|status| *status == Status::Fail),
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(|value| removed_server_url_status(value, server_map, origins))
            .max_by_key(|status| *status == Status::Fail),
        _ => None,
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

fn entry_url_status(
    value: &serde_json::Value,
    named_lific: bool,
    origins: &[reqwest::Url],
) -> Option<Status> {
    match value {
        serde_json::Value::String(value) => {
            let url = reqwest::Url::parse(value.trim()).ok()?;
            if !matches!(url.scheme(), "http" | "https") {
                return None;
            }
            let path = urlencoding::decode(url.path()).ok()?;
            let token = path.strip_prefix("/mcp/")?;
            if token.is_empty() || token.contains('/') {
                return None;
            }
            Some(
                if named_lific || origins.iter().any(|origin| origin.origin() == url.origin()) {
                    Status::Fail
                } else {
                    Status::Warn
                },
            )
        }
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(|value| entry_url_status(value, named_lific, origins))
            .max_by_key(|status| *status == Status::Fail),
        serde_json::Value::Object(values) => values
            .values()
            .filter_map(|value| entry_url_status(value, named_lific, origins))
            .max_by_key(|status| *status == Status::Fail),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_url_severity_requires_lific_identity_or_a_matching_origin() {
        let dir = tempfile::tempdir().unwrap();
        let base = PathBase {
            home: dir.path().join("home"),
            project: dir.path().join("project"),
            os: clients::Os::Linux,
            appdata: None,
        };
        std::fs::create_dir_all(&base.project).unwrap();
        let mut cfg = Config::default();
        cfg.server.public_url = Some("https://lific.example.test/prefix".into());
        for (key, name, url, expected) in [
            (
                "zapier",
                "",
                "https://mcp.zapier.com/api/mcp/s/SECRET/mcp",
                None,
            ),
            (
                "My-LIFIC",
                "",
                "https://foreign.example/mcp/SECRET",
                Some(Status::Fail),
            ),
            (
                "custom",
                "My LiFiC server",
                "https://foreign.example/mcp/SECRET",
                Some(Status::Fail),
            ),
            (
                "custom",
                "",
                "https://foreign.example/mcp/SECRET",
                Some(Status::Warn),
            ),
            (
                "custom",
                "",
                "https://lific.example.test/mcp/SECRET",
                Some(Status::Fail),
            ),
            (
                "custom",
                "",
                "http://127.0.0.1:3456/mcp/SECRET",
                Some(Status::Fail),
            ),
            (
                "custom",
                "",
                "http://127.0.0.1:9999/mcp/SECRET",
                Some(Status::Warn),
            ),
            (
                "custom",
                "",
                "http://lific.example.test/mcp/SECRET",
                Some(Status::Warn),
            ),
            (
                "custom",
                "",
                "https://lific.example.test.attacker.test/mcp/SECRET",
                Some(Status::Warn),
            ),
        ] {
            let value = serde_json::json!({"mcpServers": {key: {"name": name, "url": url}}});
            std::fs::write(base.project.join(".mcp.json"), value.to_string()).unwrap();
            let checks = check_at(&base, &cfg);
            assert_eq!(
                checks.first().map(|check| check.status),
                expected,
                "{key}: {checks:?}"
            );
            assert_eq!(checks.len(), usize::from(expected.is_some()));
            for check in checks {
                assert!(!check.detail.contains("SECRET"));
                if expected == Some(Status::Warn) {
                    assert!(check.detail.contains("if this entry points at Lific"));
                }
            }
        }
    }

    #[test]
    fn scans_nested_client_projects_but_not_unrelated_settings() {
        assert_eq!(
            removed_server_url_status(
                &serde_json::json!({
                    "documentation": "https://docs.example/mcp/tools",
                    "mcpServers": {"lific": {"url": "https://example.test/mcp"}}
                }),
                "mcpServers",
                &[]
            ),
            None
        );
        assert_eq!(
            removed_server_url_status(
                &serde_json::json!({
                    "projects": {"/checkout": {"mcpServers": {
                        "custom-name": {"url": "https://example.test/mcp/old-token"}
                    }}}
                }),
                "mcpServers",
                &[]
            ),
            Some(Status::Warn)
        );
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
        let checks = check_at(&base, &Config::default());
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
            assert!(check_at(&base, &Config::default()).is_empty());
        }
    }
}
