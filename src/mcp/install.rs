use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use chrono::Utc;
use serde::Serialize;
use serde_json::{Map, Value, json};
use tempfile::NamedTempFile;

use crate::cli::{McpClientArg, McpTransportArg, OutputFormat};
use crate::error::{Result, YacliError};
use crate::output::RenderedOutput;

use super::skills;

const SERVER_NAME: &str = "yacli";

#[derive(Clone, Debug)]
struct ServerRegistration {
    transport: McpTransportArg,
    command: Option<String>,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    url: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum InstallClient {
    Antigravity,
    Claude,
    Codex,
    Cursor,
    Gemini,
    Warp,
    Windsurf,
    Zed,
}

#[derive(Debug, Serialize)]
struct InstallItem {
    client: &'static str,
    status: &'static str,
    mechanism: &'static str,
    experimental: bool,
    detected: bool,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backup_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skills_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skills_count: Option<usize>,
}

pub fn execute_install(
    format: OutputFormat,
    selected: Vec<McpClientArg>,
    transport: McpTransportArg,
    url: Option<String>,
) -> Result<RenderedOutput> {
    let registration = canonical_registration(transport, url)?;
    let clients = resolve_clients(selected);
    let mut items = Vec::with_capacity(clients.len());
    let mut had_failure = false;

    for client in clients {
        let mut item = match install_client(client, &registration) {
            Ok(item) => item,
            Err(err) => {
                had_failure = true;
                InstallItem {
                    client: client.label(),
                    status: "failed",
                    mechanism: client.mechanism(),
                    experimental: client.experimental(),
                    detected: true,
                    detail: err.to_string(),
                    path: client.primary_path().map(|path| path.display().to_string()),
                    backup_path: None,
                    skills_path: None,
                    skills_count: None,
                }
            }
        };

        if item.status == "failed" {
            had_failure = true;
        }
        install_client_skills(client, &mut item);
        items.push(item);
    }

    let json = json!({
        "ok": !had_failure,
        "operation": "mcp.install",
        "server": {
            "name": SERVER_NAME,
            "transport": registration.transport.label(),
            "command": registration.command,
            "args": registration.args,
            "env": registration.env,
            "url": registration.url,
        },
        "items": items,
    });
    let server_args = json["server"]["args"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let item_values = json["items"].as_array().cloned().unwrap_or_default();
    let table = render_install_table(
        SERVER_NAME,
        json["server"]["transport"].as_str().unwrap_or_default(),
        json["server"]["command"].as_str().unwrap_or_default(),
        &server_args,
        json["server"]["url"].as_str(),
        &item_values,
    );

    Ok(RenderedOutput {
        format,
        json,
        table,
        exit_code: if had_failure { 1 } else { 0 },
    })
}

fn install_client_skills(client: InstallClient, item: &mut InstallItem) {
    if !item.detected || item.status == "skipped" {
        return;
    }

    let home = match home_dir() {
        Ok(h) => h,
        Err(_) => return,
    };

    let skills_dir = match client.skills_dir(&home) {
        Some(dir) => dir,
        None => return,
    };

    match skills::install_skills(&skills_dir) {
        Ok(count) => {
            item.skills_path = Some(skills_dir.display().to_string());
            item.skills_count = Some(count);
        }
        Err(_) => {
            item.skills_path = Some(skills_dir.display().to_string());
            item.skills_count = Some(0);
        }
    }
}

fn resolve_clients(selected: Vec<McpClientArg>) -> Vec<InstallClient> {
    let mut clients = BTreeSet::new();
    if selected.is_empty() {
        clients.extend([
            InstallClient::Claude,
            InstallClient::Codex,
            InstallClient::Gemini,
            InstallClient::Warp,
            InstallClient::Zed,
            InstallClient::Cursor,
            InstallClient::Antigravity,
            InstallClient::Windsurf,
        ]);
    } else {
        for client in selected {
            clients.insert(InstallClient::from(client));
        }
    }

    clients.into_iter().collect()
}

fn install_client(client: InstallClient, registration: &ServerRegistration) -> Result<InstallItem> {
    if registration.transport == McpTransportArg::Http && !client.supports_http_transport() {
        return Ok(skipped_item(
            client,
            format!(
                "{} does not have a verified native HTTP registration flow yet",
                client.label()
            ),
        ));
    }

    match client {
        InstallClient::Claude => install_native_get_add(
            client,
            "claude",
            &["mcp", "get", SERVER_NAME],
            &build_claude_add_args(registration),
        ),
        InstallClient::Codex => install_native_get_add(
            client,
            "codex",
            &["mcp", "get", SERVER_NAME],
            &build_codex_add_args(registration),
        ),
        InstallClient::Gemini => {
            install_native_add_only(client, "gemini", &build_gemini_add_args(registration))
        }
        InstallClient::Antigravity => install_native_add_only(
            client,
            "antigravity",
            &build_electron_add_args(registration),
        ),
        InstallClient::Cursor => install_json_file(
            client,
            &cursor_config_path()?,
            &[],
            "mcpServers",
            registration,
        ),
        InstallClient::Windsurf => install_json_file(
            client,
            &windsurf_config_path()?,
            &windsurf_legacy_config_paths()?,
            "mcpServers",
            registration,
        ),
        InstallClient::Warp => install_json_file(
            client,
            &warp_config_path()?,
            &[],
            "mcpServers",
            registration,
        ),
        InstallClient::Zed => install_json_file(
            client,
            &zed_settings_path()?,
            &[],
            "context_servers",
            registration,
        ),
    }
}

fn canonical_registration(
    transport: McpTransportArg,
    url: Option<String>,
) -> Result<ServerRegistration> {
    if transport == McpTransportArg::Http {
        let resolved_url = url.unwrap_or_else(|| "http://127.0.0.1:8787/mcp".to_string());
        url::Url::parse(&resolved_url).map_err(|err| {
            YacliError::Validation(format!("invalid MCP HTTP URL `{resolved_url}`: {err}"))
        })?;
        return Ok(ServerRegistration {
            transport,
            command: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            url: Some(resolved_url),
        });
    }

    let current = env::current_exe().map_err(|err| {
        YacliError::Io(format!(
            "failed to resolve current executable for MCP install: {err}"
        ))
    })?;
    let command = current
        .canonicalize()
        .unwrap_or(current)
        .display()
        .to_string();
    let mut env_vars = BTreeMap::new();
    if let Ok(value) = env::var("YACLI_CONFIG_DIR")
        && !value.trim().is_empty()
    {
        env_vars.insert("YACLI_CONFIG_DIR".to_string(), value);
    }

    Ok(ServerRegistration {
        transport,
        command: Some(command),
        args: vec!["mcp".to_string()],
        env: env_vars,
        url: None,
    })
}

fn install_native_get_add(
    client: InstallClient,
    executable: &str,
    get_args: &[&str],
    add_args: &[String],
) -> Result<InstallItem> {
    let Some(binary) = resolve_executable(executable) else {
        return Ok(skipped_item(
            client,
            format!("{executable} is not available on PATH"),
        ));
    };

    let get_status = ProcessCommand::new(&binary)
        .args(get_args)
        .status()
        .map_err(|err| YacliError::Io(format!("failed to run {executable}: {err}")))?;
    if get_status.success() {
        return Ok(InstallItem {
            client: client.label(),
            status: "already_installed",
            mechanism: client.mechanism(),
            experimental: client.experimental(),
            detected: true,
            detail: format!("{executable} already has a `{SERVER_NAME}` MCP server entry"),
            path: None,
            backup_path: None,
            skills_path: None,
            skills_count: None,
        });
    }

    let status = ProcessCommand::new(&binary)
        .args(add_args)
        .status()
        .map_err(|err| YacliError::Io(format!("failed to run {executable}: {err}")))?;
    if !status.success() {
        return Err(YacliError::Config(format!(
            "{executable} refused to add the `{SERVER_NAME}` MCP server"
        )));
    }

    Ok(InstallItem {
        client: client.label(),
        status: "installed",
        mechanism: client.mechanism(),
        experimental: client.experimental(),
        detected: true,
        detail: format!("{executable} registered `{SERVER_NAME}` using its native MCP CLI"),
        path: None,
        backup_path: None,
        skills_path: None,
        skills_count: None,
    })
}

fn install_native_add_only(
    client: InstallClient,
    executable: &str,
    add_args: &[String],
) -> Result<InstallItem> {
    let Some(binary) = resolve_executable(executable) else {
        return Ok(skipped_item(
            client,
            format!("{executable} is not available on PATH"),
        ));
    };

    let status = ProcessCommand::new(&binary)
        .args(add_args)
        .status()
        .map_err(|err| YacliError::Io(format!("failed to run {executable}: {err}")))?;
    if !status.success() {
        return Err(YacliError::Config(format!(
            "{executable} refused to add the `{SERVER_NAME}` MCP server"
        )));
    }

    Ok(InstallItem {
        client: client.label(),
        status: "installed",
        mechanism: client.mechanism(),
        experimental: client.experimental(),
        detected: true,
        detail: format!("{executable} accepted the `{SERVER_NAME}` MCP registration"),
        path: None,
        backup_path: None,
        skills_path: None,
        skills_count: None,
    })
}

fn install_json_file(
    client: InstallClient,
    path: &Path,
    fallback_paths: &[PathBuf],
    top_level_key: &str,
    registration: &ServerRegistration,
) -> Result<InstallItem> {
    if !client_detected(client) {
        return Ok(skipped_item(
            client,
            format!("{} is not installed locally", client.label()),
        ));
    }

    let source_path =
        existing_config_path(path, fallback_paths).unwrap_or_else(|| path.to_path_buf());
    let (mut root, existed) = load_json_object(&source_path)?;
    let container = ensure_object(
        root.as_object_mut().ok_or_else(|| {
            YacliError::Serialization(format!(
                "{} config root must be a JSON object",
                client.label()
            ))
        })?,
        top_level_key,
    )?;

    let desired = json_server_entry(registration);
    let requires_write = source_path != path
        || !matches!(container.get(SERVER_NAME), Some(current) if *current == desired);
    let status = if requires_write {
        container.insert(SERVER_NAME.to_string(), desired);
        "installed"
    } else {
        "already_installed"
    };

    let mut backup_path = None;
    if status == "installed" {
        if existed {
            backup_path = Some(write_backup(&source_path)?);
        } else if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_json_atomic(path, &root)?;
    }

    Ok(InstallItem {
        client: client.label(),
        status,
        mechanism: client.mechanism(),
        experimental: client.experimental(),
        detected: true,
        detail: format!(
            "{} {} `{}` in {}",
            if status == "installed" {
                "updated"
            } else {
                "already contains"
            },
            top_level_key,
            SERVER_NAME,
            path.display()
        ),
        path: Some(path.display().to_string()),
        backup_path,
        skills_path: None,
        skills_count: None,
    })
}

fn resolve_executable(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let pathext = env::var("PATHEXT").ok();
    resolve_executable_in_paths(name, env::split_paths(&path), pathext.as_deref())
}

fn load_json_object(path: &Path) -> Result<(Value, bool)> {
    if !path.exists() {
        return Ok((Value::Object(Map::new()), false));
    }

    let raw = fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok((Value::Object(Map::new()), true));
    }

    let value = serde_json::from_str::<Value>(&raw).map_err(|err| {
        YacliError::Serialization(format!("invalid JSON in {}: {err}", path.display()))
    })?;
    Ok((value, true))
}

fn ensure_object<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
) -> Result<&'a mut Map<String, Value>> {
    if !object.contains_key(key) {
        object.insert(key.to_string(), Value::Object(Map::new()));
    }

    object
        .get_mut(key)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| YacliError::Serialization(format!("`{key}` must be a JSON object")))
}

fn json_server_entry(registration: &ServerRegistration) -> Value {
    let mut entry = Map::new();
    match registration.transport {
        McpTransportArg::Stdio => {
            if let Some(command) = &registration.command {
                entry.insert("command".to_string(), Value::String(command.clone()));
            }
            entry.insert(
                "args".to_string(),
                Value::Array(
                    registration
                        .args
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            );
            if !registration.env.is_empty() {
                let env = registration
                    .env
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::String(value.clone())))
                    .collect();
                entry.insert("env".to_string(), Value::Object(env));
            }
        }
        McpTransportArg::Http => {
            if let Some(url) = &registration.url {
                entry.insert("url".to_string(), Value::String(url.clone()));
            }
        }
    }

    Value::Object(entry)
}

fn write_backup(path: &Path) -> Result<String> {
    let timestamp = Utc::now().format("%Y%m%d%H%M%S");
    let backup = path.with_extension(format!("{timestamp}.bak"));
    fs::copy(path, &backup)?;
    Ok(backup.display().to_string())
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| YacliError::Io(format!("{} has no parent directory", path.display())))?;
    fs::create_dir_all(parent)?;

    let mut temp = NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(temp.as_file_mut(), value)
        .map_err(|err| YacliError::Serialization(err.to_string()))?;
    temp.as_file_mut().write_all(b"\n")?;
    temp.persist(path)
        .map_err(|err| YacliError::Io(err.to_string()))?;
    Ok(())
}

fn skipped_item(client: InstallClient, detail: String) -> InstallItem {
    InstallItem {
        client: client.label(),
        status: "skipped",
        mechanism: client.mechanism(),
        experimental: client.experimental(),
        detected: false,
        detail,
        path: client.primary_path().map(|path| path.display().to_string()),
        backup_path: None,
        skills_path: None,
        skills_count: None,
    }
}

fn build_codex_add_args(registration: &ServerRegistration) -> Vec<String> {
    match registration.transport {
        McpTransportArg::Stdio => {
            let mut args = vec![
                "mcp".to_string(),
                "add".to_string(),
                SERVER_NAME.to_string(),
            ];
            for (key, value) in &registration.env {
                args.push("--env".to_string());
                args.push(format!("{key}={value}"));
            }
            args.push("--".to_string());
            args.push(registration.command.clone().unwrap_or_default());
            args.extend(registration.args.clone());
            args
        }
        McpTransportArg::Http => vec![
            "mcp".to_string(),
            "add".to_string(),
            SERVER_NAME.to_string(),
            "--url".to_string(),
            registration.url.clone().unwrap_or_default(),
        ],
    }
}

fn build_claude_add_args(registration: &ServerRegistration) -> Vec<String> {
    match registration.transport {
        McpTransportArg::Stdio => {
            let mut args = vec![
                "mcp".to_string(),
                "add".to_string(),
                "--scope".to_string(),
                "user".to_string(),
            ];
            for (key, value) in &registration.env {
                args.push("-e".to_string());
                args.push(format!("{key}={value}"));
            }
            args.push(SERVER_NAME.to_string());
            args.push("--".to_string());
            args.push(registration.command.clone().unwrap_or_default());
            args.extend(registration.args.clone());
            args
        }
        McpTransportArg::Http => vec![
            "mcp".to_string(),
            "add".to_string(),
            SERVER_NAME.to_string(),
            "--transport".to_string(),
            "http".to_string(),
            registration.url.clone().unwrap_or_default(),
            "--scope".to_string(),
            "user".to_string(),
        ],
    }
}

fn build_gemini_add_args(registration: &ServerRegistration) -> Vec<String> {
    let mut args = vec![
        "mcp".to_string(),
        "add".to_string(),
        "-s".to_string(),
        "user".to_string(),
    ];
    match registration.transport {
        McpTransportArg::Stdio => {
            args.push(SERVER_NAME.to_string());
            for (key, value) in &registration.env {
                args.push("-e".to_string());
                args.push(format!("{key}={value}"));
            }
            args.push("--".to_string());
            args.push(registration.command.clone().unwrap_or_default());
            args.extend(registration.args.clone());
        }
        McpTransportArg::Http => {
            args.push("--transport".to_string());
            args.push("http".to_string());
            args.push(SERVER_NAME.to_string());
            args.push(registration.url.clone().unwrap_or_default());
        }
    }
    args
}

fn build_electron_add_args(registration: &ServerRegistration) -> Vec<String> {
    let payload = json!({
        "name": SERVER_NAME,
        "command": registration.command.clone().unwrap_or_default(),
        "args": registration.args,
        "env": registration.env,
    });
    vec!["--add-mcp".to_string(), payload.to_string()]
}

fn client_detected(client: InstallClient) -> bool {
    let home = match home_dir() {
        Ok(home) => home,
        Err(_) => return false,
    };
    match client {
        InstallClient::Cursor => {
            home.join(".cursor").exists()
                || home.join("Library/Application Support/Cursor").exists()
        }
        InstallClient::Windsurf => {
            home.join(".codeium").exists()
                || home.join(".codeium/windsurf").exists()
                || home.join(".windsurf").exists()
                || home.join("Library/Application Support/Windsurf").exists()
        }
        InstallClient::Warp => {
            home.join(".warp").exists()
                || home
                    .join("Library/Group Containers/2BBY89MBSN.dev.warp/Library/Application Support/dev.warp.Warp-Stable")
                    .exists()
        }
        InstallClient::Zed => {
            home.join(".config/zed").exists()
                || home.join("Library/Application Support/Zed").exists()
        }
        _ => true,
    }
}

fn home_dir() -> Result<PathBuf> {
    if let Some(home) = env::var_os("HOME") {
        return Ok(PathBuf::from(home));
    }
    dirs::home_dir().ok_or_else(|| YacliError::Config("failed to resolve HOME".to_string()))
}

fn cursor_config_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".cursor/mcp.json"))
}

fn windsurf_config_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".codeium/mcp_config.json"))
}

fn windsurf_legacy_config_paths() -> Result<Vec<PathBuf>> {
    Ok(vec![home_dir()?.join(".codeium/windsurf/mcp_config.json")])
}

fn warp_config_path() -> Result<PathBuf> {
    Ok(home_dir()?.join(".warp/mcp_settings.json"))
}

fn zed_settings_path() -> Result<PathBuf> {
    let home = home_dir()?;
    if cfg!(target_os = "macos") {
        Ok(home.join("Library/Application Support/Zed/settings.json"))
    } else {
        Ok(home.join(".config/zed/settings.json"))
    }
}

fn render_install_table(
    server_name: &str,
    transport: &str,
    command: &str,
    args: &[Value],
    url: Option<&str>,
    items: &[Value],
) -> String {
    let mut lines = vec![
        format!("SERVER\t{server_name}"),
        format!("TRANSPORT\t{transport}"),
        format!("COMMAND\t{command}"),
        format!(
            "ARGS\t{}",
            args.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        format!("URL\t{}", url.unwrap_or_default()),
        "CLIENT\tSTATUS\tMECHANISM\tEXPERIMENTAL\tSKILLS\tDETAIL".to_string(),
    ];

    for item in items {
        lines.push(format!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            item["client"].as_str().unwrap_or_default(),
            item["status"].as_str().unwrap_or_default(),
            item["mechanism"].as_str().unwrap_or_default(),
            item["experimental"].as_bool().unwrap_or(false),
            item["skills_count"]
                .as_u64()
                .map_or("-".to_string(), |n| n.to_string()),
            item["detail"].as_str().unwrap_or_default(),
        ));
    }

    lines.join("\n")
}

impl InstallClient {
    fn label(self) -> &'static str {
        match self {
            Self::Antigravity => "antigravity",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::Gemini => "gemini",
            Self::Warp => "warp",
            Self::Windsurf => "windsurf",
            Self::Zed => "zed",
        }
    }

    fn mechanism(self) -> &'static str {
        match self {
            Self::Claude | Self::Codex | Self::Gemini | Self::Antigravity => "native_cli",
            Self::Cursor | Self::Windsurf | Self::Zed => "json_file",
            Self::Warp => "json_file",
        }
    }

    fn experimental(self) -> bool {
        matches!(self, Self::Warp | Self::Antigravity)
    }

    fn supports_http_transport(self) -> bool {
        matches!(self, Self::Claude | Self::Codex | Self::Gemini)
    }

    fn primary_path(self) -> Option<PathBuf> {
        match self {
            Self::Cursor => cursor_config_path().ok(),
            Self::Windsurf => windsurf_config_path().ok(),
            Self::Warp => warp_config_path().ok(),
            Self::Zed => zed_settings_path().ok(),
            _ => None,
        }
    }

    fn skills_dir(self, home: &Path) -> Option<PathBuf> {
        match self {
            Self::Claude => Some(home.join(".claude/skills")),
            Self::Codex => Some(home.join(".agents/skills")),
            Self::Gemini => Some(home.join(".agents/skills")),
            Self::Cursor => Some(home.join(".cursor/skills")),
            Self::Windsurf => Some(home.join(".codeium/windsurf/skills")),
            Self::Warp => Some(home.join(".agents/skills")),
            Self::Antigravity => Some(home.join(".gemini/antigravity/skills")),
            Self::Zed => None,
        }
    }
}

impl From<McpClientArg> for InstallClient {
    fn from(value: McpClientArg) -> Self {
        match value {
            McpClientArg::Claude => Self::Claude,
            McpClientArg::Codex => Self::Codex,
            McpClientArg::Gemini => Self::Gemini,
            McpClientArg::Warp => Self::Warp,
            McpClientArg::Zed => Self::Zed,
            McpClientArg::Cursor => Self::Cursor,
            McpClientArg::Antigravity => Self::Antigravity,
            McpClientArg::Windsurf => Self::Windsurf,
        }
    }
}

fn existing_config_path(primary: &Path, fallback_paths: &[PathBuf]) -> Option<PathBuf> {
    if primary.exists() {
        return Some(primary.to_path_buf());
    }

    fallback_paths
        .iter()
        .find(|candidate| candidate.exists())
        .cloned()
}

fn resolve_executable_in_paths(
    name: &str,
    paths: impl Iterator<Item = PathBuf>,
    pathext: Option<&str>,
) -> Option<PathBuf> {
    let candidates = executable_candidates(name, pathext);
    for dir in paths {
        for candidate in &candidates {
            let path = dir.join(candidate);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn executable_candidates(name: &str, pathext: Option<&str>) -> Vec<String> {
    let mut candidates = vec![name.to_string()];
    if Path::new(name).extension().is_some() {
        return candidates;
    }

    if let Some(pathext) = pathext {
        for ext in pathext.split(';') {
            let ext = ext.trim();
            if ext.is_empty() {
                continue;
            }
            let normalized = if ext.starts_with('.') {
                ext.to_string()
            } else {
                format!(".{ext}")
            };
            let candidate = format!("{name}{normalized}");
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }

    candidates
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn executable_candidates_expand_pathext_for_windows_style_bins() {
        let temp = tempdir().expect("tempdir");
        let bin = temp.path().join("bin");
        fs::create_dir_all(&bin).expect("bin directory");
        let candidate = bin.join("codex.CMD");
        fs::write(&candidate, "@echo off\r\n").expect("command file");

        let resolved = resolve_executable_in_paths(
            "codex",
            std::iter::once(bin.clone()),
            Some(".COM;.EXE;.BAT;.CMD"),
        )
        .expect("resolved executable");

        assert_eq!(resolved, candidate);
    }

    #[test]
    fn skills_dir_returns_correct_path_per_client() {
        let home = Path::new("/home/user");

        assert_eq!(
            InstallClient::Claude.skills_dir(home),
            Some(PathBuf::from("/home/user/.claude/skills"))
        );
        assert_eq!(
            InstallClient::Codex.skills_dir(home),
            Some(PathBuf::from("/home/user/.agents/skills"))
        );
        assert_eq!(
            InstallClient::Gemini.skills_dir(home),
            Some(PathBuf::from("/home/user/.agents/skills"))
        );
        assert_eq!(
            InstallClient::Cursor.skills_dir(home),
            Some(PathBuf::from("/home/user/.cursor/skills"))
        );
        assert_eq!(
            InstallClient::Windsurf.skills_dir(home),
            Some(PathBuf::from("/home/user/.codeium/windsurf/skills"))
        );
        assert_eq!(
            InstallClient::Warp.skills_dir(home),
            Some(PathBuf::from("/home/user/.agents/skills"))
        );
        assert_eq!(
            InstallClient::Antigravity.skills_dir(home),
            Some(PathBuf::from("/home/user/.gemini/antigravity/skills"))
        );
        assert_eq!(InstallClient::Zed.skills_dir(home), None);
    }

    #[test]
    fn gemini_add_args_include_env_overrides() {
        let mut env = BTreeMap::new();
        env.insert(
            "YACLI_CONFIG_DIR".to_string(),
            "/tmp/yacli-config".to_string(),
        );
        let registration = ServerRegistration {
            transport: McpTransportArg::Stdio,
            command: Some("/usr/local/bin/yacli".to_string()),
            args: vec!["mcp".to_string()],
            env,
            url: None,
        };

        let args = build_gemini_add_args(&registration);

        assert!(
            args.windows(2)
                .any(|pair| { pair[0] == "-e" && pair[1] == "YACLI_CONFIG_DIR=/tmp/yacli-config" })
        );
    }
}
