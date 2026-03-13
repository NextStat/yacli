use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn yacli() -> Command {
    let mut command = Command::cargo_bin("yacli").expect("binary exists");
    command.env("YACLI_SECRET_BACKEND", "file");
    command
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent directory created");
    }
    fs::write(path, content).expect("file written");
}

fn prepend_path(dir: &Path) -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    format!("{}:{}", dir.display(), current)
}

fn make_executable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("permissions");
    }
}

fn write_logging_command(path: &Path, log_path: &Path, fail_get: bool) {
    let fail_get_block = if fail_get {
        "if [ \"$1\" = \"mcp\" ] && [ \"$2\" = \"get\" ]; then exit 1; fi\n"
    } else {
        ""
    };
    write_file(
        path,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"{}\"\n{}exit 0\n",
            log_path.display(),
            fail_get_block
        ),
    );
    make_executable(path);
}

fn zed_settings_path(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Zed/settings.json")
    } else {
        home.join(".config/zed/settings.json")
    }
}

fn claude_desktop_config_path(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Claude/claude_desktop_config.json")
    } else if cfg!(target_os = "windows") {
        home.join("AppData/Roaming/Claude/claude_desktop_config.json")
    } else {
        home.join(".config/Claude/claude_desktop_config.json")
    }
}

fn windsurf_config_path(home: &Path) -> PathBuf {
    home.join(".codeium/mcp_config.json")
}

fn windsurf_legacy_config_path(home: &Path) -> PathBuf {
    home.join(".codeium/windsurf/mcp_config.json")
}

#[test]
fn top_level_help_exposes_mcp_command() {
    yacli()
        .args(["--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("mcp"))
        .stdout(predicate::str::contains("Запустить MCP сервер"));
}

#[test]
fn mcp_help_describes_server_mode() {
    yacli()
        .args(["mcp", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("yacli mcp"))
        .stdout(predicate::str::contains("stdio"))
        .stdout(predicate::str::contains("http"))
        .stdout(predicate::str::contains("listen"))
        .stdout(predicate::str::contains("install"));
}

#[test]
fn mcp_install_help_lists_target_clients() {
    yacli()
        .args(["mcp", "install", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("stdio"))
        .stdout(predicate::str::contains("http"))
        .stdout(predicate::str::contains("url"))
        .stdout(predicate::str::contains("cursor"))
        .stdout(predicate::str::contains("codex"))
        .stdout(predicate::str::contains("claude"))
        .stdout(predicate::str::contains("claude-desktop"))
        .stdout(predicate::str::contains("zed"))
        .stdout(predicate::str::contains("warp"))
        .stdout(predicate::str::contains("windsurf"))
        .stdout(predicate::str::contains("gemini"))
        .stdout(predicate::str::contains("antigravity"));
}

#[test]
fn mcp_install_updates_cursor_config_idempotently() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let cursor_path = home.join(".cursor/mcp.json");
    write_file(
        &cursor_path,
        r#"{
  "mcpServers": {
    "existing": {
      "command": "node",
      "args": ["server.js"]
    }
  }
}"#,
    );

    let output = yacli()
        .env("HOME", home)
        .args(["mcp", "install", "--client", "cursor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(report["operation"], "mcp.install");
    assert_eq!(report["items"][0]["client"], "cursor");
    assert_eq!(report["items"][0]["status"], "installed");
    let backup_path = report["items"][0]["backup_path"]
        .as_str()
        .expect("backup path");
    assert!(Path::new(backup_path).exists());

    let installed: Value =
        serde_json::from_slice(&fs::read(&cursor_path).expect("cursor config")).expect("json");
    assert_eq!(installed["mcpServers"]["existing"]["command"], "node");
    assert_eq!(installed["mcpServers"]["yacli"]["args"][0], "mcp");

    let second = yacli()
        .env("HOME", home)
        .args(["mcp", "install", "--client", "cursor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&second).expect("valid json");
    assert_eq!(report["items"][0]["status"], "already_installed");
}

#[test]
fn mcp_install_updates_zed_settings() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let settings_path = zed_settings_path(home);
    write_file(
        &settings_path,
        r#"{
  "theme": "Andromeda",
  "context_servers": {
    "existing": {
      "command": "node",
      "args": ["server.js"]
    }
  }
}"#,
    );

    yacli()
        .env("HOME", home)
        .args(["mcp", "install", "--client", "zed"])
        .assert()
        .success();

    let installed: Value =
        serde_json::from_slice(&fs::read(&settings_path).expect("zed settings")).expect("json");
    assert_eq!(installed["context_servers"]["existing"]["command"], "node");
    assert_eq!(installed["context_servers"]["yacli"]["args"][0], "mcp");
}

#[test]
fn mcp_install_invokes_codex_native_cli() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin directory");
    let log_path = home.join("codex.log");
    let script_path = bin_dir.join("codex");
    write_logging_command(&script_path, &log_path, true);

    yacli()
        .env("HOME", home)
        .env("PATH", prepend_path(&bin_dir))
        .args(["mcp", "install", "--client", "codex"])
        .assert()
        .success();

    let log = fs::read_to_string(&log_path).expect("log");
    assert!(log.contains("mcp"));
    assert!(log.contains("get"));
    assert!(log.contains("add"));
    assert!(log.contains("yacli"));
}

#[test]
fn mcp_install_invokes_codex_native_http_cli() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin directory");
    let log_path = home.join("codex-http.log");
    let script_path = bin_dir.join("codex");
    write_logging_command(&script_path, &log_path, true);

    yacli()
        .env("HOME", home)
        .env("PATH", prepend_path(&bin_dir))
        .args([
            "mcp",
            "install",
            "--client",
            "codex",
            "--transport",
            "http",
            "--url",
            "http://127.0.0.1:8787/mcp",
        ])
        .assert()
        .success();

    let log = fs::read_to_string(&log_path).expect("log");
    assert!(log.contains("--url"));
    assert!(log.contains("http://127.0.0.1:8787/mcp"));
}

#[test]
fn mcp_install_invokes_claude_native_cli() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin directory");
    let log_path = home.join("claude.log");
    let script_path = bin_dir.join("claude");
    write_logging_command(&script_path, &log_path, true);

    yacli()
        .env("HOME", home)
        .env("PATH", prepend_path(&bin_dir))
        .args(["mcp", "install", "--client", "claude"])
        .assert()
        .success();

    let log = fs::read_to_string(&log_path).expect("log");
    assert!(log.contains("mcp"));
    assert!(log.contains("get"));
    assert!(log.contains("add"));
    assert!(log.contains("--scope"));
    assert!(log.contains("user"));
}

#[test]
fn mcp_install_writes_claude_desktop_config() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let desktop_path = claude_desktop_config_path(home);
    write_file(
        &desktop_path,
        r#"{
  "mcpServers": {
    "existing": {
      "command": "node",
      "args": ["desktop.js"]
    }
  }
}"#,
    );

    let output = yacli()
        .env("HOME", home)
        .args(["mcp", "install", "--client", "claude-desktop"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("json report");
    assert_eq!(report["items"][0]["client"], "claude-desktop");
    assert_eq!(report["items"][0]["status"], "installed");
    assert!(report["items"][0]["skills_count"].is_null());

    let installed: Value =
        serde_json::from_slice(&fs::read(&desktop_path).expect("desktop config")).expect("json");
    assert_eq!(installed["mcpServers"]["existing"]["command"], "node");
    assert_eq!(installed["mcpServers"]["yacli"]["args"][0], "mcp");
}

#[test]
fn mcp_install_claude_also_installs_claude_desktop() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin directory");
    let log_path = home.join("claude.log");
    write_logging_command(&bin_dir.join("claude"), &log_path, true);
    write_file(
        &claude_desktop_config_path(home),
        "{\n  \"mcpServers\": {}\n}\n",
    );

    let output = yacli()
        .env("HOME", home)
        .env("PATH", prepend_path(&bin_dir))
        .args(["mcp", "install", "--client", "claude"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("json report");
    let items = report["items"].as_array().expect("items");
    assert!(items.iter().any(|item| item["client"] == "claude"));
    assert!(items.iter().any(|item| item["client"] == "claude-desktop"));

    let desktop_config: Value =
        serde_json::from_slice(&fs::read(claude_desktop_config_path(home)).expect("desktop"))
            .expect("desktop json");
    assert_eq!(desktop_config["mcpServers"]["yacli"]["args"][0], "mcp");
}

#[test]
fn mcp_install_invokes_claude_native_http_cli() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin directory");
    let log_path = home.join("claude-http.log");
    let script_path = bin_dir.join("claude");
    write_logging_command(&script_path, &log_path, true);

    yacli()
        .env("HOME", home)
        .env("PATH", prepend_path(&bin_dir))
        .args([
            "mcp",
            "install",
            "--client",
            "claude",
            "--transport",
            "http",
            "--url",
            "http://127.0.0.1:8787/mcp",
        ])
        .assert()
        .success();

    let log = fs::read_to_string(&log_path).expect("log");
    assert!(log.contains("--transport"));
    assert!(log.contains("http"));
    assert!(log.contains("http://127.0.0.1:8787/mcp"));
}

#[test]
fn mcp_install_invokes_gemini_native_cli() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin directory");
    let log_path = home.join("gemini.log");
    let script_path = bin_dir.join("gemini");
    write_logging_command(&script_path, &log_path, false);

    yacli()
        .env("HOME", home)
        .env("PATH", prepend_path(&bin_dir))
        .env("YACLI_CONFIG_DIR", "/tmp/yacli-config")
        .args(["mcp", "install", "--client", "gemini"])
        .assert()
        .success();

    let log = fs::read_to_string(&log_path).expect("log");
    assert!(log.contains("mcp"));
    assert!(log.contains("add"));
    assert!(log.contains("-s"));
    assert!(log.contains("user"));
    assert!(log.contains("-e"));
    assert!(log.contains("YACLI_CONFIG_DIR=/tmp/yacli-config"));
    assert!(log.contains("yacli"));
}

#[test]
fn mcp_install_invokes_gemini_native_http_cli() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin directory");
    let log_path = home.join("gemini-http.log");
    let script_path = bin_dir.join("gemini");
    write_logging_command(&script_path, &log_path, false);

    yacli()
        .env("HOME", home)
        .env("PATH", prepend_path(&bin_dir))
        .args([
            "mcp",
            "install",
            "--client",
            "gemini",
            "--transport",
            "http",
            "--url",
            "http://127.0.0.1:8787/mcp",
        ])
        .assert()
        .success();

    let log = fs::read_to_string(&log_path).expect("log");
    assert!(log.contains("--transport"));
    assert!(log.contains("http"));
    assert!(log.contains("http://127.0.0.1:8787/mcp"));
}

#[test]
fn mcp_install_invokes_antigravity_native_cli() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin directory");
    let log_path = home.join("antigravity.log");
    let script_path = bin_dir.join("antigravity");
    write_logging_command(&script_path, &log_path, false);

    yacli()
        .env("HOME", home)
        .env("PATH", prepend_path(&bin_dir))
        .args(["mcp", "install", "--client", "antigravity"])
        .assert()
        .success();

    let log = fs::read_to_string(&log_path).expect("log");
    assert!(log.contains("--add-mcp"));
    assert!(log.contains("yacli"));
    assert!(log.contains("\"args\":[\"mcp\"]"));
}

#[test]
fn mcp_install_updates_windsurf_config_idempotently() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let config_path = windsurf_config_path(home);
    write_file(
        &config_path,
        r#"{
  "mcpServers": {
    "existing": {
      "command": "node",
      "args": ["server.js"]
    }
  }
}"#,
    );

    let output = yacli()
        .env("HOME", home)
        .args(["mcp", "install", "--client", "windsurf"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("json report");
    assert_eq!(report["items"][0]["client"], "windsurf");
    assert_eq!(report["items"][0]["status"], "installed");
    assert!(report["items"][0]["backup_path"].as_str().is_some());

    let installed: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("windsurf config")).expect("json");
    assert_eq!(installed["mcpServers"]["existing"]["command"], "node");
    assert_eq!(installed["mcpServers"]["yacli"]["args"][0], "mcp");

    let second = yacli()
        .env("HOME", home)
        .args(["mcp", "install", "--client", "windsurf"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&second).expect("json report");
    assert_eq!(report["items"][0]["status"], "already_installed");
}

#[test]
fn mcp_install_migrates_windsurf_legacy_config_to_current_path() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let legacy_path = windsurf_legacy_config_path(home);
    let current_path = windsurf_config_path(home);
    write_file(
        &legacy_path,
        r#"{
  "mcpServers": {
    "existing": {
      "command": "node",
      "args": ["server.js"]
    }
  }
}"#,
    );

    let output = yacli()
        .env("HOME", home)
        .args(["mcp", "install", "--client", "windsurf"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("json report");
    assert_eq!(report["items"][0]["status"], "installed");
    assert_eq!(
        report["items"][0]["path"],
        current_path.display().to_string()
    );
    let backup_path = report["items"][0]["backup_path"]
        .as_str()
        .expect("backup path");
    assert!(backup_path.contains(".codeium/windsurf/mcp_config."));
    assert!(Path::new(backup_path).exists());

    let installed: Value =
        serde_json::from_slice(&fs::read(&current_path).expect("windsurf config")).expect("json");
    assert_eq!(installed["mcpServers"]["existing"]["command"], "node");
    assert_eq!(installed["mcpServers"]["yacli"]["args"][0], "mcp");
}

#[test]
fn mcp_install_updates_warp_config_and_marks_experimental() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let config_path = home.join(".warp/mcp_settings.json");
    write_file(
        &config_path,
        r#"{
  "mcpServers": {
    "existing": {
      "command": "node",
      "args": ["server.js"]
    }
  }
}"#,
    );

    let output = yacli()
        .env("HOME", home)
        .args(["mcp", "install", "--client", "warp"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("json report");
    assert_eq!(report["items"][0]["client"], "warp");
    assert_eq!(report["items"][0]["experimental"], true);
    assert_eq!(report["items"][0]["status"], "installed");
    assert!(report["items"][0]["backup_path"].as_str().is_some());

    let installed: Value =
        serde_json::from_slice(&fs::read(&config_path).expect("warp config")).expect("json");
    assert_eq!(installed["mcpServers"]["existing"]["command"], "node");
    assert_eq!(installed["mcpServers"]["yacli"]["args"][0], "mcp");
}

#[test]
fn mcp_install_skips_unverified_http_registration_for_cursor() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let cursor_path = home.join(".cursor/mcp.json");
    write_file(&cursor_path, "{}");

    let output = yacli()
        .env("HOME", home)
        .args([
            "mcp",
            "install",
            "--client",
            "cursor",
            "--transport",
            "http",
            "--url",
            "http://127.0.0.1:8787/mcp",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("json report");
    assert_eq!(report["items"][0]["client"], "cursor");
    assert_eq!(report["items"][0]["status"], "skipped");
}

#[test]
fn mcp_install_fails_nonzero_when_detected_client_rejects_install() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin directory");
    let log_path = home.join("codex-fail.log");
    let script_path = bin_dir.join("codex");
    write_file(
        &script_path,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"{}\"\nif [ \"$1\" = \"mcp\" ] && [ \"$2\" = \"get\" ]; then exit 1; fi\nexit 2\n",
            log_path.display()
        ),
    );
    make_executable(&script_path);

    let output = yacli()
        .env("HOME", home)
        .env("PATH", prepend_path(&bin_dir))
        .args(["mcp", "install", "--client", "codex"])
        .assert()
        .failure()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("json report");
    assert_eq!(report["ok"], false);
    assert_eq!(report["items"][0]["client"], "codex");
    assert_eq!(report["items"][0]["status"], "failed");
}

#[test]
fn mcp_install_writes_skills_to_cursor_directory() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();

    let cursor_dir = home.join(".cursor");
    fs::create_dir_all(&cursor_dir).expect("cursor dir");
    write_file(&cursor_dir.join("mcp.json"), "{}");

    let output = yacli()
        .env("HOME", home)
        .args(["mcp", "install", "--client", "cursor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("valid json");
    let item = &report["items"][0];

    assert!(item["skills_count"].as_u64().unwrap() > 0);
    let skills_path = item["skills_path"].as_str().expect("skills_path");
    assert!(skills_path.contains(".cursor/skills"));

    let skills_dir = home.join(".cursor/skills");
    assert!(skills_dir.join("yacli-shared/SKILL.md").exists());
    assert!(skills_dir.join("yacli-mail/SKILL.md").exists());
    assert!(skills_dir.join("yacli-calendar/SKILL.md").exists());
    assert!(skills_dir.join("yacli-disk/SKILL.md").exists());
    assert!(skills_dir.join("yacli-daily-briefing/SKILL.md").exists());
    assert!(skills_dir.join("yacli-find-and-read/SKILL.md").exists());
    assert!(
        skills_dir
            .join("yacli-reply-with-context/SKILL.md")
            .exists()
    );

    let shared = fs::read_to_string(skills_dir.join("yacli-shared/SKILL.md")).expect("read");
    assert!(shared.starts_with("---\n"));
    assert!(shared.contains("name: yacli-shared"));
}

#[test]
fn mcp_install_writes_skills_to_claude_directory() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");

    let log_path = home.join("claude.log");
    write_logging_command(&bin_dir.join("claude"), &log_path, true);

    let output = yacli()
        .env("HOME", home)
        .env("PATH", prepend_path(&bin_dir))
        .args(["mcp", "install", "--client", "claude"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("valid json");
    let item = &report["items"][0];

    assert!(item["skills_count"].as_u64().unwrap() > 0);
    let skills_path = item["skills_path"].as_str().expect("skills_path");
    assert!(skills_path.contains(".claude/skills"));

    let skills_dir = home.join(".claude/skills");
    assert!(skills_dir.join("yacli-shared/SKILL.md").exists());
    assert!(skills_dir.join("yacli-mail/SKILL.md").exists());
}

#[test]
fn mcp_install_skips_skills_for_zed() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();

    let zed_dir = zed_settings_path(home);
    fs::create_dir_all(zed_dir.parent().expect("parent")).expect("zed dir");
    write_file(&zed_dir, "{}");

    let output = yacli()
        .env("HOME", home)
        .args(["mcp", "install", "--client", "zed"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("valid json");
    let item = &report["items"][0];

    assert!(item.get("skills_path").is_none() || item["skills_path"].is_null());
    assert!(item.get("skills_count").is_none() || item["skills_count"].is_null());
}

#[test]
fn mcp_install_skills_are_idempotent() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();

    let cursor_dir = home.join(".cursor");
    fs::create_dir_all(&cursor_dir).expect("cursor dir");
    write_file(&cursor_dir.join("mcp.json"), "{}");

    yacli()
        .env("HOME", home)
        .args(["mcp", "install", "--client", "cursor"])
        .assert()
        .success();

    let output = yacli()
        .env("HOME", home)
        .args(["mcp", "install", "--client", "cursor"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(report["ok"], true);
    assert!(report["items"][0]["skills_count"].as_u64().unwrap() > 0);
}

#[test]
fn mcp_install_writes_skills_to_agents_directory_for_codex() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path();
    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");

    let log_path = home.join("codex.log");
    write_logging_command(&bin_dir.join("codex"), &log_path, true);

    let output = yacli()
        .env("HOME", home)
        .env("PATH", prepend_path(&bin_dir))
        .args(["mcp", "install", "--client", "codex"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: Value = serde_json::from_slice(&output).expect("valid json");
    let item = &report["items"][0];

    let skills_path = item["skills_path"].as_str().expect("skills_path");
    assert!(skills_path.contains(".agents/skills"));

    let skills_dir = home.join(".agents/skills");
    assert!(skills_dir.join("yacli-shared/SKILL.md").exists());
}
