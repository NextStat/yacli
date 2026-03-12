use assert_cmd::Command;
use base64::Engine;
use mockito::{Matcher, Server};
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use tempfile::tempdir;

fn yacli() -> Command {
    Command::cargo_bin("yacli").expect("binary exists")
}

fn write_accounts_file(config_dir: &std::path::Path, content: &str) {
    fs::write(config_dir.join("accounts.toml"), content).expect("accounts file written");
}

fn write_credentials_file(config_dir: &std::path::Path, content: &str) {
    fs::write(config_dir.join("credentials.toml"), content).expect("credentials file written");
}

fn write_mock_account_with_refs(
    config_dir: &std::path::Path,
    disk_base_url: &str,
    mail_auth_mode: &str,
    mail_credential_ref: Option<&str>,
    disk_credential_ref: Option<&str>,
) {
    let mail_credential_ref = mail_credential_ref
        .map(|value| format!("credential_ref = \"{value}\"\n"))
        .unwrap_or_default();
    let disk_credential_ref = disk_credential_ref
        .map(|value| format!("credential_ref = \"{value}\"\n"))
        .unwrap_or_default();

    write_accounts_file(
        config_dir,
        &format!(
            r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "{}"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465
{}

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "{}"
{}"#,
            mail_auth_mode, mail_credential_ref, disk_base_url, disk_credential_ref
        ),
    );
}

fn write_mock_account_with_calendar_refs(
    config_dir: &std::path::Path,
    calendar_base_url: &str,
    calendar_credential_ref: Option<&str>,
) {
    let calendar_credential_ref = calendar_credential_ref
        .map(|value| format!("credential_ref = \"{value}\"\n"))
        .unwrap_or_default();

    write_accounts_file(
        config_dir,
        &format!(
            r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "{}"
{}

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
            calendar_base_url, calendar_credential_ref
        ),
    );
}

fn write_mock_account(
    config_dir: &std::path::Path,
    disk_base_url: &str,
    disk_credential_ref: Option<&str>,
) {
    write_mock_account_with_refs(
        config_dir,
        disk_base_url,
        "oauth_xoauth2",
        None,
        disk_credential_ref,
    );
}

fn basic_auth_header(account: &str, app_password: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{account}:{app_password}"))
    )
}

#[test]
fn account_add_and_list_work() {
    let temp = tempdir().expect("tempdir");

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "account",
            "add",
            "personal",
            "me@yandex.ru",
            "--use",
            "--mail-credential-ref",
            "env:YACLI_MAIL_SECRET",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"operation\":\"account.add\""));

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["account", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    let items = value["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "personal");
    assert_eq!(items[0]["current"], true);
}

#[test]
fn simple_add_sets_current_account_and_derived_name() {
    let temp = tempdir().expect("tempdir");

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["add", "me@yandex.ru"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"operation\":\"account.add\""));

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["whoami"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["account"], "me");
    assert_eq!(value["email"], "me@yandex.ru");
}

#[test]
fn simple_add_respects_manual_account_name() {
    let temp = tempdir().expect("tempdir");

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["add", "me@yandex.ru", "personal"])
        .assert()
        .success();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["whoami"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["account"], "personal");
    assert_eq!(value["email"], "me@yandex.ru");
}

#[test]
fn top_level_help_hides_agent_guide_command() {
    yacli()
        .args(["--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("add"))
        .stdout(predicate::str::contains("login"))
        .stdout(predicate::str::contains("mail"))
        .stdout(predicate::str::contains("calendar"))
        .stdout(predicate::str::contains("disk"))
        .stdout(predicate::str::contains("guide").not());
}

#[test]
fn mail_read_help_uses_positional_uid() {
    yacli()
        .args(["mail", "read", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: yacli mail read [OPTIONS] <UID>"))
        .stdout(predicate::str::contains("--uid").not())
        .stdout(predicate::str::contains("--folder <FOLDER>        [default: INBOX]"));
}

#[test]
fn guide_lists_stable_commands_and_workflows() {
    let output = yacli()
        .args(["guide"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "guide.show");
    assert_eq!(value["topic"], "all");
    assert_eq!(value["version"], "0.1.22");

    let commands = value["commands"].as_array().expect("commands array");
    assert!(commands.iter().any(|entry| entry["path"] == "add"));
    assert!(commands.iter().any(|entry| entry["path"] == "accounts"));
    assert!(commands.iter().any(|entry| entry["path"] == "use"));
    assert!(commands.iter().any(|entry| entry["path"] == "whoami"));
    assert!(commands.iter().any(|entry| entry["path"] == "status"));
    assert!(commands.iter().any(|entry| entry["path"] == "login"));
    assert!(commands.iter().any(|entry| entry["path"] == "login calendar"));
    assert!(commands.iter().any(|entry| entry["path"] == "logout"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail read"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail search"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail reply"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail forward"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail send"));
    assert!(commands.iter().any(|entry| entry["path"] == "disk list"));
    assert!(commands.iter().any(|entry| entry["path"] == "disk mkdir"));
    assert!(commands.iter().any(|entry| entry["path"] == "disk upload"));
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "disk public download")
    );
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "calendar events")
    );

    let workflows = value["workflows"].as_array().expect("workflows array");
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_read_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_search_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_reply_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_forward_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "multi_account_mail_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_send_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "disk_browse_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "disk_write_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "calendar_read_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "calendar_write_flow")
    );
}

#[test]
fn guide_topic_mail_filters_to_mail_commands() {
    let output = yacli()
        .args(["guide", "--topic", "mail"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["topic"], "mail");

    let commands = value["commands"].as_array().expect("commands array");
    assert!(!commands.is_empty());
    assert!(commands.iter().all(|entry| entry["topic"] == "mail"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail folders"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail search"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail reply"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail forward"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail read"));
    assert!(commands.iter().any(|entry| entry["path"] == "mail send"));

    let workflows = value["workflows"].as_array().expect("workflows array");
    assert!(workflows.iter().all(|entry| entry["topic"] == "mail"));
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_search_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_reply_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_forward_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "mail_send_flow")
    );
}

#[test]
fn guide_topic_auth_filters_to_simple_login_commands() {
    let output = yacli()
        .args(["guide", "--topic", "auth"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["topic"], "auth");

    let commands = value["commands"].as_array().expect("commands array");
    assert!(!commands.is_empty());
    assert!(commands.iter().all(|entry| entry["topic"] == "auth"));
    assert!(commands.iter().any(|entry| entry["path"] == "status"));
    assert!(commands.iter().any(|entry| entry["path"] == "login"));
    assert!(commands.iter().any(|entry| entry["path"] == "login calendar"));
    assert!(commands.iter().any(|entry| entry["path"] == "logout"));
}

#[test]
fn guide_topic_calendar_filters_to_calendar_commands() {
    let output = yacli()
        .args(["guide", "--topic", "calendar"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["topic"], "calendar");

    let commands = value["commands"].as_array().expect("commands array");
    assert!(!commands.is_empty());
    assert!(commands.iter().all(|entry| entry["topic"] == "calendar"));
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "calendar calendars")
    );
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "calendar events")
    );
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "calendar create")
    );
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "calendar delete")
    );

    let workflows = value["workflows"].as_array().expect("workflows array");
    assert!(workflows.iter().all(|entry| entry["topic"] == "calendar"));
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "calendar_read_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "calendar_write_flow")
    );
}

#[test]
fn guide_topic_disk_filters_to_disk_commands() {
    let output = yacli()
        .args(["guide", "--topic", "disk"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["topic"], "disk");

    let commands = value["commands"].as_array().expect("commands array");
    assert!(!commands.is_empty());
    assert!(commands.iter().all(|entry| entry["topic"] == "disk"));
    assert!(commands.iter().any(|entry| entry["path"] == "disk list"));
    assert!(commands.iter().any(|entry| entry["path"] == "disk mkdir"));
    assert!(commands.iter().any(|entry| entry["path"] == "disk upload"));
    assert!(commands.iter().any(|entry| entry["path"] == "disk info"));
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "disk public show")
    );
    assert!(
        commands
            .iter()
            .any(|entry| entry["path"] == "disk public download")
    );

    let workflows = value["workflows"].as_array().expect("workflows array");
    assert!(workflows.iter().all(|entry| entry["topic"] == "disk"));
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "disk_browse_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "disk_write_flow")
    );
    assert!(
        workflows
            .iter()
            .any(|entry| entry["id"] == "disk_private_flow")
    );
}

#[test]
fn account_use_switches_current_account() {
    let temp = tempdir().expect("tempdir");

    for name in ["personal", "work"] {
        yacli()
            .env("YACLI_CONFIG_DIR", temp.path())
            .args(["account", "add", name, "me@yandex.ru"])
            .assert()
            .success();
    }

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["account", "use", "work"])
        .assert()
        .success();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["account", "current"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["account"], "work");
}

#[test]
fn account_list_marks_single_account_as_current_without_default_flag() {
    let temp = tempdir().expect("tempdir");

    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.single]
email = "single@yandex.ru"
default = false

[accounts.single.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.single.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.single.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );

    let list_output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["account", "list"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let list: Value = serde_json::from_slice(&list_output).expect("valid json");
    assert_eq!(list["items"][0]["name"], "single");
    assert_eq!(list["items"][0]["current"], true);

    let current_output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["account", "current"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let current: Value = serde_json::from_slice(&current_output).expect("valid json");
    assert_eq!(current["account"], "single");
    assert_eq!(current["email"], "single@yandex.ru");
}

#[test]
fn validate_reports_invalid_account() {
    let temp = tempdir().expect("tempdir");

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["account", "add", "broken", "not-an-email"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("VALIDATION_ERROR"));
}

#[test]
fn auth_status_reports_missing_and_present_env_refs() {
    let temp = tempdir().expect("tempdir");

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "account",
            "add",
            "personal",
            "me@yandex.ru",
            "--use",
            "--mail-credential-ref",
            "env:YACLI_MAIL_SECRET",
            "--calendar-credential-ref",
            "env:YACLI_CALENDAR_SECRET",
        ])
        .assert()
        .success();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .env("YACLI_MAIL_SECRET", "ready")
        .args(["auth", "status"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["services"]["mail"]["credential_state"], "env_present");
    assert_eq!(
        value["services"]["calendar"]["credential_state"],
        "env_missing"
    );
    assert_eq!(
        value["services"]["disk"]["credential_state"],
        "not_configured"
    );
}

#[test]
fn auth_login_stores_disk_token_and_updates_account_ref() {
    let temp = tempdir().expect("tempdir");
    let mut oauth = Server::new();

    write_mock_account(temp.path(), "https://cloud-api.yandex.net", None);

    let _token = oauth
        .mock("POST", "/token")
        .match_body(Matcher::Regex(
            "grant_type=authorization_code&code=confirm-123&client_id=client-123&code_verifier=.+"
                .to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "access_token": "access-123",
  "refresh_token": "refresh-123",
  "token_type": "bearer",
  "expires_in": 3600,
  "scope": "cloud_api:disk.app_folder cloud_api:disk.info cloud_api:disk.read cloud_api:disk.write"
}"#,
        )
        .create();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .env("YACLI_OAUTH_BASE_URL", oauth.url())
        .args([
            "auth",
            "login",
            "--account",
            "mock",
            "--service",
            "disk",
            "--client-id",
            "client-123",
            "--code",
            "confirm-123",
            "--login-hint",
            "me@yandex.ru",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "auth.login");
    assert_eq!(value["service"], "disk");
    assert_eq!(value["credential_ref"], "store:disk");
    let authorization_url = value["authorization"]["authorization_url"]
        .as_str()
        .expect("authorization url");
    assert!(authorization_url.contains("/authorize?"));
    assert!(authorization_url.contains("client_id=client-123"));
    assert!(
        authorization_url
            .contains("redirect_uri=https%3A%2F%2Foauth.yandex.ru%2Fverification_code")
    );
    assert!(authorization_url.contains("code_challenge="));
    assert!(authorization_url.contains("code_challenge_method=S256"));
    assert!(authorization_url.contains("login_hint=me%40yandex.ru"));

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(accounts.contains("credential_ref = \"store:disk\""));

    let credentials =
        fs::read_to_string(temp.path().join("credentials.toml")).expect("credentials");
    assert!(credentials.contains("kind = \"oauth_pkce\""));
    assert!(credentials.contains("access_token = \"access-123\""));
    assert!(credentials.contains("client_id = \"client-123\""));
    assert!(!credentials.contains("refresh_token"));
    assert!(!credentials.contains("client_secret_env"));
}

#[test]
fn simple_login_uses_builtin_client_and_connects_mail_and_disk() {
    let temp = tempdir().expect("tempdir");
    let mut oauth = Server::new();

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        None,
        None,
    );

    let _token = oauth
        .mock("POST", "/token")
        .match_body(Matcher::Regex(
            "grant_type=authorization_code&code=confirm-123&client_id=babbe3ab2e254d5abee427890e2a5a8f&code_verifier=.+".to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "access_token": "access-123",
  "token_type": "bearer",
  "expires_in": 3600,
  "scope": "mail:imap_full mail:smtp cloud_api:disk.app_folder cloud_api:disk.info cloud_api:disk.read cloud_api:disk.write"
}"#,
        )
        .create();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .env("YACLI_OAUTH_BASE_URL", oauth.url())
        .args(["login", "--code", "confirm-123"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "auth.login");
    assert_eq!(value["client_id_source"], "built_in");
    assert_eq!(value["services"][0], "mail");
    assert_eq!(value["services"][1], "disk");
    let authorization_url = value["authorization"]["authorization_url"]
        .as_str()
        .expect("authorization url");
    assert!(authorization_url.contains("client_id=babbe3ab2e254d5abee427890e2a5a8f"));
    assert!(authorization_url.contains("mail%3Aimap_full"));
    assert!(authorization_url.contains("cloud_api%3Adisk.read"));

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(accounts.contains("[accounts.mock.mail]"));
    assert!(accounts.contains("[accounts.mock.disk]"));
    assert!(accounts.contains("credential_ref = \"store:mail\""));
    assert!(accounts.contains("credential_ref = \"store:disk\""));
}

#[test]
fn auth_login_stores_mail_token_and_updates_account_ref() {
    let temp = tempdir().expect("tempdir");
    let mut oauth = Server::new();

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        None,
        None,
    );

    let _token = oauth
        .mock("POST", "/token")
        .match_body(Matcher::Regex(
            "grant_type=authorization_code&code=mail-confirm-123&client_id=client-123&code_verifier=.+"
                .to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "access_token": "mail-access-123",
  "token_type": "bearer",
  "expires_in": 3600,
  "scope": "mail:imap_full mail:smtp"
}"#,
        )
        .create();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .env("YACLI_OAUTH_BASE_URL", oauth.url())
        .args([
            "auth",
            "login",
            "--account",
            "mock",
            "--service",
            "mail",
            "--client-id",
            "client-123",
            "--code",
            "mail-confirm-123",
            "--login-hint",
            "me@yandex.ru",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "auth.login");
    assert_eq!(value["service"], "mail");
    assert_eq!(value["credential_ref"], "store:mail");
    let authorization_url = value["authorization"]["authorization_url"]
        .as_str()
        .expect("authorization url");
    assert!(authorization_url.contains("scope=mail%3Aimap_full+mail%3Asmtp"));

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(accounts.contains("credential_ref = \"store:mail\""));

    let credentials =
        fs::read_to_string(temp.path().join("credentials.toml")).expect("credentials");
    assert!(credentials.contains("[accounts.mock.services.mail]"));
    assert!(credentials.contains("access_token = \"mail-access-123\""));
}

#[test]
fn simple_login_with_app_password_defaults_to_calendar() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_calendar_refs(temp.path(), "https://caldav.yandex.ru", None);

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["login", "--app-password", "calendar-secret"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "auth.login");
    assert_eq!(value["service"], "calendar");
    assert_eq!(value["credential_ref"], "store:calendar");
    assert_eq!(value["mode"], "app_password_store");
}

#[test]
fn auth_login_sets_calendar_env_ref_without_touching_credentials_store() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_calendar_refs(temp.path(), "https://caldav.yandex.ru", None);

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "auth",
            "login",
            "--account",
            "mock",
            "--service",
            "calendar",
            "--env-var",
            "YACLI_CALENDAR_APP_PASSWORD",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "auth.login");
    assert_eq!(value["service"], "calendar");
    assert_eq!(value["credential_ref"], "env:YACLI_CALENDAR_APP_PASSWORD");
    assert_eq!(value["mode"], "app_password_env");

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(accounts.contains("credential_ref = \"env:YACLI_CALENDAR_APP_PASSWORD\""));
    assert!(!temp.path().join("credentials.toml").exists());
}

#[test]
fn auth_login_stores_calendar_app_password_and_updates_account_ref() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_calendar_refs(temp.path(), "https://caldav.yandex.ru", None);

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "auth",
            "login",
            "--account",
            "mock",
            "--service",
            "calendar",
            "--app-password",
            "calendar-secret",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "auth.login");
    assert_eq!(value["service"], "calendar");
    assert_eq!(value["credential_ref"], "store:calendar");
    assert_eq!(value["mode"], "app_password_store");

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(accounts.contains("credential_ref = \"store:calendar\""));

    let credentials =
        fs::read_to_string(temp.path().join("credentials.toml")).expect("credentials");
    assert!(credentials.contains("[accounts.mock.services.calendar]"));
    assert!(credentials.contains("kind = \"app_password\""));
    assert!(credentials.contains("secret = \"calendar-secret\""));
}

#[test]
fn mail_oauth_tokens_are_isolated_per_account() {
    let temp = tempdir().expect("tempdir");
    let mut oauth = Server::new();

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["account", "add", "personal", "personal@yandex.ru", "--use"])
        .assert()
        .success();

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["account", "add", "work", "work@company.ru"])
        .assert()
        .success();

    let _token_personal = oauth
        .mock("POST", "/token")
        .match_body(Matcher::Regex(
            "grant_type=authorization_code&code=personal-code&client_id=client-123&code_verifier=.+"
                .to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "access_token": "mail-access-personal",
  "token_type": "bearer",
  "expires_in": 3600,
  "scope": "mail:imap_full mail:smtp"
}"#,
        )
        .create();

    let _token_work = oauth
        .mock("POST", "/token")
        .match_body(Matcher::Regex(
            "grant_type=authorization_code&code=work-code&client_id=client-123&code_verifier=.+"
                .to_string(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "access_token": "mail-access-work",
  "token_type": "bearer",
  "expires_in": 3600,
  "scope": "mail:imap_full mail:smtp"
}"#,
        )
        .create();

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .env("YACLI_OAUTH_BASE_URL", oauth.url())
        .args([
            "auth",
            "login",
            "--account",
            "personal",
            "--service",
            "mail",
            "--client-id",
            "client-123",
            "--code",
            "personal-code",
        ])
        .assert()
        .success();

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .env("YACLI_OAUTH_BASE_URL", oauth.url())
        .args([
            "auth",
            "login",
            "--account",
            "work",
            "--service",
            "mail",
            "--client-id",
            "client-123",
            "--code",
            "work-code",
        ])
        .assert()
        .success();

    let credentials =
        fs::read_to_string(temp.path().join("credentials.toml")).expect("credentials");
    assert!(credentials.contains("[accounts.personal.services.mail]"));
    assert!(credentials.contains("[accounts.work.services.mail]"));
    assert!(credentials.contains("access_token = \"mail-access-personal\""));
    assert!(credentials.contains("access_token = \"mail-access-work\""));

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(accounts.contains("[accounts.personal.mail]"));
    assert!(accounts.contains("[accounts.work.mail]"));
    assert!(accounts.contains("credential_ref = \"store:mail\""));
}

#[test]
fn mail_logout_only_removes_target_account_token() {
    let temp = tempdir().expect("tempdir");

    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "personal@yandex.ru"
default = true

[accounts.personal.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465
credential_ref = "store:mail"

[accounts.personal.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.personal.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"

[accounts.work]
email = "work@company.ru"
default = false

[accounts.work.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465
credential_ref = "store:mail"

[accounts.work.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.work.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.personal.services.mail]
kind = "oauth_pkce"
access_token = "mail-access-personal"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"

[accounts.work.services.mail]
kind = "oauth_pkce"
access_token = "mail-access-work"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "auth",
            "logout",
            "--account",
            "personal",
            "--service",
            "mail",
        ])
        .assert()
        .success();

    let credentials =
        fs::read_to_string(temp.path().join("credentials.toml")).expect("credentials");
    assert!(!credentials.contains("[accounts.personal.services.mail]"));
    assert!(credentials.contains("[accounts.work.services.mail]"));
    assert!(credentials.contains("access_token = \"mail-access-work\""));

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(accounts.contains("[accounts.personal.mail]"));
    assert!(accounts.contains("[accounts.work.mail]"));
    assert_eq!(
        accounts.matches("credential_ref = \"store:mail\"").count(),
        1
    );
}

#[test]
fn auth_login_rejects_mail_oauth_when_account_uses_app_password() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "app_password",
        Some("env:YACLI_MAIL_APP_PASSWORD"),
        None,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "auth",
            "login",
            "--account",
            "mock",
            "--service",
            "mail",
            "--client-id",
            "client-123",
            "--code",
            "mail-confirm-123",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "\"code\":\"UNSUPPORTED_OPERATION\"",
        ))
        .stderr(predicate::str::contains(
            "mail OAuth login requires account.mail.auth_mode=oauth_xoauth2",
        ));
}

#[test]
fn auth_status_reports_store_present_for_saved_oauth_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_account(
        temp.path(),
        "https://cloud-api.yandex.net",
        Some("store:disk"),
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "access-123"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.read"]
client_id = "client-123"
"#,
    );

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["auth", "status", "--account", "mock"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(
        value["services"]["disk"]["credential_state"],
        "store_present"
    );
    assert_eq!(value["services"]["disk"]["credential_ref"], "store:disk");
}

#[test]
fn auth_status_reports_store_present_for_saved_calendar_app_password() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_calendar_refs(
        temp.path(),
        "https://caldav.yandex.ru",
        Some("store:calendar"),
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.calendar]
kind = "app_password"
secret = "calendar-secret"
"#,
    );

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["auth", "status", "--account", "mock"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(
        value["services"]["calendar"]["credential_state"],
        "store_present"
    );
    assert_eq!(
        value["services"]["calendar"]["credential_ref"],
        "store:calendar"
    );
}

#[test]
fn auth_logout_removes_stored_mail_token_and_clears_account_ref() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        Some("store:disk"),
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-access-123"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-access-123"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.read"]
client_id = "client-123"
"#,
    );

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["auth", "logout", "--account", "mock", "--service", "mail"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["service"], "mail");
    assert_eq!(value["removed"], true);
    assert_eq!(value["cleared_account_ref"], true);

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(!accounts.contains("credential_ref = \"store:mail\""));
    assert!(accounts.contains("credential_ref = \"store:disk\""));

    let credentials =
        fs::read_to_string(temp.path().join("credentials.toml")).expect("credentials");
    assert!(!credentials.contains("[accounts.mock.services.mail]"));
    assert!(credentials.contains("[accounts.mock.services.disk]"));
}

#[test]
fn auth_logout_removes_stored_disk_token_and_clears_account_ref() {
    let temp = tempdir().expect("tempdir");

    write_mock_account(
        temp.path(),
        "https://cloud-api.yandex.net",
        Some("store:disk"),
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "access-123"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.read"]
client_id = "client-123"
"#,
    );

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["auth", "logout", "--account", "mock", "--service", "disk"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "auth.logout");
    assert_eq!(value["removed"], true);
    assert_eq!(value["cleared_account_ref"], true);

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(!accounts.contains("credential_ref = \"store:disk\""));

    let credentials =
        fs::read_to_string(temp.path().join("credentials.toml")).expect("credentials");
    assert!(!credentials.contains("[accounts.mock.services.disk]"));
}

#[test]
fn auth_logout_clears_calendar_env_ref_without_credentials_store() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_calendar_refs(
        temp.path(),
        "https://caldav.yandex.ru",
        Some("env:YACLI_CALENDAR_APP_PASSWORD"),
    );

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "auth",
            "logout",
            "--account",
            "mock",
            "--service",
            "calendar",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["service"], "calendar");
    assert_eq!(value["removed"], false);
    assert_eq!(value["cleared_account_ref"], true);

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(!accounts.contains("credential_ref = \"env:YACLI_CALENDAR_APP_PASSWORD\""));
    assert!(!temp.path().join("credentials.toml").exists());
}

#[test]
fn auth_logout_removes_stored_calendar_secret_and_clears_account_ref() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_calendar_refs(
        temp.path(),
        "https://caldav.yandex.ru",
        Some("store:calendar"),
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.calendar]
kind = "app_password"
secret = "calendar-secret"
"#,
    );

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "auth",
            "logout",
            "--account",
            "mock",
            "--service",
            "calendar",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["service"], "calendar");
    assert_eq!(value["removed"], true);
    assert_eq!(value["cleared_account_ref"], true);

    let accounts = fs::read_to_string(temp.path().join("accounts.toml")).expect("accounts");
    assert!(!accounts.contains("credential_ref = \"store:calendar\""));

    let credentials =
        fs::read_to_string(temp.path().join("credentials.toml")).expect("credentials");
    assert!(!credentials.contains("[accounts.mock.services.calendar]"));
}

#[test]
fn mail_folders_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "expired-token"
token_type = "bearer"
expires_at_epoch_secs = 1
scope = ["mail:imap_full"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mail", "folders", "--account", "mock"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""))
        .stderr(predicate::str::contains("stored OAuth token expired"));
}

#[test]
fn mail_list_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "expired-token"
token_type = "bearer"
expires_at_epoch_secs = 1
scope = ["mail:imap_full"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mail", "list", "--account", "mock"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""))
        .stderr(predicate::str::contains("stored OAuth token expired"));
}

#[test]
fn mail_read_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "expired-token"
token_type = "bearer"
expires_at_epoch_secs = 1
scope = ["mail:imap_full"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mail", "read", "--account", "mock", "42"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""))
        .stderr(predicate::str::contains("stored OAuth token expired"));
}

#[test]
fn mail_list_rejects_zero_limit() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mail", "list", "--account", "mock", "--limit", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "mail list --limit must be greater than zero",
        ));
}

#[test]
fn mail_search_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "expired-token"
token_type = "bearer"
expires_at_epoch_secs = 1
scope = ["mail:imap_full"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mail", "search", "--account", "mock", "--query", "Budget"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""))
        .stderr(predicate::str::contains("stored OAuth token expired"));
}

#[test]
fn mail_search_rejects_zero_limit() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "search",
            "--account",
            "mock",
            "--query",
            "Budget",
            "--limit",
            "0",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "mail search --limit must be greater than zero",
        ));
}

#[test]
fn mail_search_rejects_empty_query() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mail", "search", "--account", "mock", "--query", "   "])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "mail search --query must not be empty",
        ));
}

#[test]
fn mail_reply_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "expired-token"
token_type = "bearer"
expires_at_epoch_secs = 1
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "reply",
            "--account",
            "mock",
            "42",
            "--text",
            "Принято",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""))
        .stderr(predicate::str::contains("stored OAuth token expired"));
}

#[test]
fn mail_reply_rejects_zero_uid() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "reply",
            "--account",
            "mock",
            "0",
            "--text",
            "Принято",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "mail reply --uid must be greater than zero",
        ));
}

#[test]
fn mail_reply_rejects_missing_body_before_network() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mail", "reply", "--account", "mock", "42"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "mail reply requires --text, --html, or both",
        ));
}

#[test]
fn mail_forward_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "expired-token"
token_type = "bearer"
expires_at_epoch_secs = 1
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "forward",
            "--account",
            "mock",
            "42",
            "--to",
            "person@example.com",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""))
        .stderr(predicate::str::contains("stored OAuth token expired"));
}

#[test]
fn mail_forward_rejects_zero_uid() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "forward",
            "--account",
            "mock",
            "0",
            "--to",
            "person@example.com",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "mail forward --uid must be greater than zero",
        ));
}

#[test]
fn mail_forward_rejects_zero_max_source_bytes_before_network() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "forward",
            "--account",
            "mock",
            "42",
            "--to",
            "person@example.com",
            "--max-source-bytes",
            "0",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "mail forward --max-source-bytes must be greater than zero",
        ));
}

#[test]
fn mail_forward_rejects_invalid_recipient_before_network() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full", "mail:smtp"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "forward",
            "--account",
            "mock",
            "42",
            "--to",
            "broken-recipient",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "mail forward --to recipient must contain `@`",
        ));
}

#[test]
fn mail_read_rejects_zero_uid() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["mail", "read", "--account", "mock", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "mail read --uid must be greater than zero",
        ));
}

#[test]
fn mail_read_rejects_zero_max_bytes() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_refs(
        temp.path(),
        "https://cloud-api.yandex.net",
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:imap_full"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "read",
            "--account",
            "mock",
            "42",
            "--max-bytes",
            "0",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "mail read --max-bytes must be greater than zero",
        ));
}

#[test]
fn mail_send_rejects_missing_body_before_network() {
    let temp = tempdir().expect("tempdir");
    let disk = Server::new();

    write_mock_account_with_refs(
        temp.path(),
        &disk.url(),
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:smtp", "mail:imap_full"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "send",
            "--account",
            "mock",
            "--to",
            "person@example.com",
            "--subject",
            "Hello",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "mail send requires --text, --html, or both",
        ));
}

#[test]
fn mail_send_rejects_invalid_recipient_before_network() {
    let temp = tempdir().expect("tempdir");
    let disk = Server::new();

    write_mock_account_with_refs(
        temp.path(),
        &disk.url(),
        "oauth_xoauth2",
        Some("store:mail"),
        None,
    );
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.mail]
kind = "oauth_pkce"
access_token = "mail-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["mail:smtp", "mail:imap_full"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "mail",
            "send",
            "--account",
            "mock",
            "--to",
            "broken-recipient",
            "--subject",
            "Hello",
            "--text",
            "Body",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "mail send --to recipient must contain `@`",
        ));
}

#[test]
fn calendar_calendars_lists_collections_via_caldav() {
    let temp = tempdir().expect("tempdir");
    let mut caldav = Server::new();

    write_mock_account_with_calendar_refs(temp.path(), &caldav.url(), Some("store:calendar"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.calendar]
kind = "app_password"
secret = "calendar-secret"
"#,
    );

    let auth = basic_auth_header("me@yandex.ru", "calendar-secret");

    let _principal = caldav
        .mock("PROPFIND", "/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "0")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/</d:href>
    <d:propstat>
      <d:prop>
        <d:current-user-principal>
          <d:href>/principals/users/me@yandex.ru/</d:href>
        </d:current-user-principal>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _home = caldav
        .mock("PROPFIND", "/principals/users/me@yandex.ru/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "0")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/principals/users/me@yandex.ru/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>me@yandex.ru</d:displayname>
        <c:calendar-home-set>
          <d:href>/calendars/me@yandex.ru/</d:href>
        </c:calendar-home-set>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _collections = caldav
        .mock("PROPFIND", "/calendars/me@yandex.ru/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "1")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/me@yandex.ru/</d:href>
    <d:propstat>
      <d:prop>
        <d:resourcetype><d:collection/></d:resourcetype>
      </d:prop>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/me@yandex.ru/default/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Личный</d:displayname>
        <c:calendar-description>Основной календарь</c:calendar-description>
        <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      </d:prop>
    </d:propstat>
  </d:response>
  <d:response>
    <d:href>/calendars/me@yandex.ru/team/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Команда</d:displayname>
        <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["calendar", "calendars", "--account", "mock"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "calendar.calendars");
    assert_eq!(value["calendars"].as_array().expect("array").len(), 2);
    assert_eq!(value["calendars"][0]["id"], "default");
    assert_eq!(value["calendars"][0]["name"], "Личный");
    assert_eq!(value["calendars"][0]["description"], "Основной календарь");
}

#[test]
fn calendar_events_lists_items_from_selected_calendar() {
    let temp = tempdir().expect("tempdir");
    let mut caldav = Server::new();

    write_mock_account_with_calendar_refs(temp.path(), &caldav.url(), Some("store:calendar"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.calendar]
kind = "app_password"
secret = "calendar-secret"
"#,
    );

    let auth = basic_auth_header("me@yandex.ru", "calendar-secret");

    let _principal = caldav
        .mock("PROPFIND", "/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "0")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/</d:href>
    <d:propstat>
      <d:prop>
        <d:current-user-principal>
          <d:href>/principals/users/me@yandex.ru/</d:href>
        </d:current-user-principal>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _home = caldav
        .mock("PROPFIND", "/principals/users/me@yandex.ru/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "0")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/principals/users/me@yandex.ru/</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-home-set>
          <d:href>/calendars/me@yandex.ru/</d:href>
        </c:calendar-home-set>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _collections = caldav
        .mock("PROPFIND", "/calendars/me@yandex.ru/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "1")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/me@yandex.ru/default/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Личный</d:displayname>
        <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _report = caldav
        .mock("REPORT", "/calendars/me@yandex.ru/default/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "1")
        .match_body(Matcher::Regex(
            "time-range start=\"20260312T000000Z\" end=\"20260319T000000Z\"".to_string(),
        ))
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/me@yandex.ru/default/event-1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"evt-1"</d:getetag>
        <c:calendar-data><![CDATA[BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:event-1
SUMMARY:Синк команды
DTSTART:20260312T090000Z
DTEND:20260312T100000Z
LOCATION:Meet
STATUS:CONFIRMED
END:VEVENT
END:VCALENDAR]]></c:calendar-data>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "calendar",
            "events",
            "--account",
            "mock",
            "--calendar",
            "default",
            "--from",
            "2026-03-12",
            "--to",
            "2026-03-19",
            "--limit",
            "10",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "calendar.events");
    assert_eq!(value["calendar"]["id"], "default");
    assert_eq!(value["window"]["from"], "2026-03-12T00:00:00Z");
    assert_eq!(value["window"]["to"], "2026-03-19T00:00:00Z");
    assert_eq!(value["events"].as_array().expect("events").len(), 1);
    assert_eq!(value["events"][0]["uid"], "event-1");
    assert_eq!(value["events"][0]["summary"], "Синк команды");
    assert_eq!(value["events"][0]["start"], "2026-03-12T09:00:00Z");
    assert_eq!(value["events"][0]["location"], "Meet");
}

#[test]
fn calendar_create_writes_event_via_caldav_put() {
    let temp = tempdir().expect("tempdir");
    let mut caldav = Server::new();

    write_mock_account_with_calendar_refs(temp.path(), &caldav.url(), Some("store:calendar"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.calendar]
kind = "app_password"
secret = "calendar-secret"
"#,
    );

    let auth = basic_auth_header("me@yandex.ru", "calendar-secret");

    let _principal = caldav
        .mock("PROPFIND", "/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "0")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/</d:href>
    <d:propstat>
      <d:prop>
        <d:current-user-principal>
          <d:href>/principals/users/me@yandex.ru/</d:href>
        </d:current-user-principal>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _home = caldav
        .mock("PROPFIND", "/principals/users/me@yandex.ru/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "0")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/principals/users/me@yandex.ru/</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-home-set>
          <d:href>/calendars/me@yandex.ru/</d:href>
        </c:calendar-home-set>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _collections = caldav
        .mock("PROPFIND", "/calendars/me@yandex.ru/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "1")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/me@yandex.ru/default/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Личный</d:displayname>
        <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _put = caldav
        .mock(
            "PUT",
            Matcher::Regex("/calendars/me@yandex\\.ru/default/[^/]+\\.ics".to_string()),
        )
        .match_header("authorization", auth.as_str())
        .match_header("content-type", "text/calendar; charset=utf-8")
        .match_header("if-none-match", "*")
        .match_body(Matcher::AllOf(vec![
            Matcher::Regex("BEGIN:VEVENT".to_string()),
            Matcher::Regex("SUMMARY:Синк команды".to_string()),
            Matcher::Regex("DTSTART:20260312T090000Z".to_string()),
            Matcher::Regex("DTEND:20260312T100000Z".to_string()),
            Matcher::Regex("DESCRIPTION:Первая строка\\\\nвторая".to_string()),
            Matcher::Regex("LOCATION:Meet".to_string()),
        ]))
        .with_status(201)
        .with_header("etag", "\"new-evt\"")
        .create();

    let _report = caldav
        .mock("REPORT", "/calendars/me@yandex.ru/default/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "1")
        .match_body(Matcher::Regex("prop-filter name=\"UID\"".to_string()))
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body_from_request(|request| {
            let body = request
                .utf8_lossy_body()
                .expect("request body available")
                .into_owned();
            let uid = body
                .split("<c:text-match collation=\"i;octet\">")
                .nth(1)
                .and_then(|value| value.split("</c:text-match>").next())
                .expect("uid filter present");
            format!(
                r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/me@yandex.ru/default/canonical-event.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"new-evt"</d:getetag>
        <c:calendar-data><![CDATA[BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:{uid}
SUMMARY:Синк команды
DESCRIPTION:Первая строка\nвторая
DTSTART:20260312T090000Z
DTEND:20260312T100000Z
LOCATION:Meet
STATUS:CONFIRMED
END:VEVENT
END:VCALENDAR]]></c:calendar-data>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#
            )
            .into_bytes()
        })
        .create();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "calendar",
            "create",
            "--account",
            "mock",
            "--calendar",
            "default",
            "--summary",
            "Синк команды",
            "--start",
            "2026-03-12T09:00:00Z",
            "--end",
            "2026-03-12T10:00:00Z",
            "--description",
            "Первая строка\nвторая",
            "--location",
            "Meet",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "calendar.create");
    assert_eq!(value["calendar"]["id"], "default");
    assert_eq!(value["event"]["summary"], "Синк команды");
    assert_eq!(value["event"]["start"], "2026-03-12T09:00:00Z");
    assert_eq!(value["event"]["end"], "2026-03-12T10:00:00Z");
    assert_eq!(value["event"]["etag"], "\"new-evt\"");
    assert!(
        value["event"]["uid"]
            .as_str()
            .expect("uid")
            .starts_with("yacli-")
    );
    assert_eq!(
        value["event"]["href"],
        "/calendars/me@yandex.ru/default/canonical-event.ics"
    );
}

#[test]
fn calendar_delete_removes_event_by_uid_via_caldav_delete() {
    let temp = tempdir().expect("tempdir");
    let mut caldav = Server::new();

    write_mock_account_with_calendar_refs(temp.path(), &caldav.url(), Some("store:calendar"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.calendar]
kind = "app_password"
secret = "calendar-secret"
"#,
    );

    let auth = basic_auth_header("me@yandex.ru", "calendar-secret");

    let _principal = caldav
        .mock("PROPFIND", "/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "0")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:">
  <d:response>
    <d:href>/</d:href>
    <d:propstat>
      <d:prop>
        <d:current-user-principal>
          <d:href>/principals/users/me@yandex.ru/</d:href>
        </d:current-user-principal>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _home = caldav
        .mock("PROPFIND", "/principals/users/me@yandex.ru/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "0")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/principals/users/me@yandex.ru/</d:href>
    <d:propstat>
      <d:prop>
        <c:calendar-home-set>
          <d:href>/calendars/me@yandex.ru/</d:href>
        </c:calendar-home-set>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _collections = caldav
        .mock("PROPFIND", "/calendars/me@yandex.ru/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "1")
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/me@yandex.ru/default/</d:href>
    <d:propstat>
      <d:prop>
        <d:displayname>Личный</d:displayname>
        <d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _report = caldav
        .mock("REPORT", "/calendars/me@yandex.ru/default/")
        .match_header("authorization", auth.as_str())
        .match_header("depth", "1")
        .match_body(Matcher::Regex(
            "text-match collation=\"i;octet\">event-1<".to_string(),
        ))
        .with_status(207)
        .with_header("content-type", "application/xml; charset=utf-8")
        .with_body(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/calendars/me@yandex.ru/default/event-1.ics</d:href>
    <d:propstat>
      <d:prop>
        <d:getetag>"evt-1"</d:getetag>
        <c:calendar-data><![CDATA[BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:event-1
SUMMARY:Удаляемое событие
DTSTART:20260312T090000Z
DTEND:20260312T100000Z
END:VEVENT
END:VCALENDAR]]></c:calendar-data>
      </d:prop>
    </d:propstat>
  </d:response>
</d:multistatus>"#,
        )
        .create();

    let _delete = caldav
        .mock("DELETE", "/calendars/me@yandex.ru/default/event-1.ics")
        .match_header("authorization", auth.as_str())
        .match_header("if-match", "\"evt-1\"")
        .with_status(204)
        .create();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "calendar",
            "delete",
            "--account",
            "mock",
            "--calendar",
            "default",
            "--uid",
            "event-1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "calendar.delete");
    assert_eq!(value["calendar"]["id"], "default");
    assert_eq!(value["deleted_event"]["uid"], "event-1");
    assert_eq!(value["deleted_event"]["summary"], "Удаляемое событие");
    assert_eq!(
        value["deleted_event"]["href"],
        "/calendars/me@yandex.ru/default/event-1.ics"
    );
}

#[test]
fn calendar_events_rejects_zero_limit() {
    let temp = tempdir().expect("tempdir");

    write_mock_account_with_calendar_refs(
        temp.path(),
        "https://caldav.yandex.ru",
        Some("env:YACLI_CALENDAR_APP_PASSWORD"),
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "calendar",
            "events",
            "--account",
            "mock",
            "--calendar",
            "default",
            "--limit",
            "0",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "calendar events --limit must be greater than zero",
        ));
}

#[test]
fn disk_info_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");
    let disk = Server::new();

    write_mock_account(temp.path(), &disk.url(), Some("store:disk"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "expired-token"
token_type = "bearer"
expires_at_epoch_secs = 1
scope = ["cloud_api:disk.read"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["disk", "info", "--account", "mock"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""))
        .stderr(predicate::str::contains("stored OAuth token expired"));
}

#[test]
fn disk_list_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");
    let disk = Server::new();

    write_mock_account(temp.path(), &disk.url(), Some("store:disk"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "expired-token"
token_type = "bearer"
expires_at_epoch_secs = 1
scope = ["cloud_api:disk.read"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["disk", "list", "--account", "mock"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""))
        .stderr(predicate::str::contains("stored OAuth token expired"));
}

#[test]
fn disk_list_rejects_zero_limit_before_network() {
    let temp = tempdir().expect("tempdir");
    let disk = Server::new();

    write_mock_account(temp.path(), &disk.url(), Some("store:disk"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.read"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["disk", "list", "--account", "mock", "--limit", "0"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "disk list --limit must be greater than zero",
        ));
}

#[test]
fn disk_list_returns_private_resource_with_children() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();

    write_mock_account(temp.path(), &server.url(), Some("store:disk"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.read"]
client_id = "client-123"
"#,
    );

    let _mock = server
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs".into()),
            Matcher::UrlEncoded("limit".into(), "2".into()),
            Matcher::UrlEncoded("offset".into(), "1".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "docs",
  "path": "disk:/docs",
  "type": "dir",
  "created": "2026-03-12T20:00:00+00:00",
  "modified": "2026-03-12T20:05:00+00:00",
  "revision": 17,
  "_embedded": {
    "limit": 2,
    "offset": 1,
    "total": 3,
    "items": [
      {
        "name": "report.pdf",
        "path": "disk:/docs/report.pdf",
        "type": "file",
        "size": 42,
        "mime_type": "application/pdf",
        "modified": "2026-03-12T20:04:00+00:00",
        "revision": 9
      },
      {
        "name": "notes",
        "path": "disk:/docs/notes",
        "type": "dir",
        "modified": "2026-03-12T20:03:00+00:00",
        "revision": 8
      }
    ]
  }
}"#,
        )
        .create();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "disk",
            "list",
            "--account",
            "mock",
            "--path",
            "disk:/docs",
            "--limit",
            "2",
            "--offset",
            "1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "disk.list");
    assert_eq!(value["path"], "disk:/docs");
    assert_eq!(value["limit"], 2);
    assert_eq!(value["offset"], 1);
    assert_eq!(value["resource"]["name"], "docs");
    assert_eq!(value["resource"]["resource_type"], "dir");
    assert_eq!(value["resource"]["children"]["total"], 3);
    assert_eq!(
        value["resource"]["children"]["items"][0]["name"],
        "report.pdf"
    );
    assert_eq!(
        value["resource"]["children"]["items"][1]["resource_type"],
        "dir"
    );
}

#[test]
fn disk_mkdir_rejects_expired_stored_token() {
    let temp = tempdir().expect("tempdir");
    let disk = Server::new();

    write_mock_account(temp.path(), &disk.url(), Some("store:disk"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "expired-token"
token_type = "bearer"
expires_at_epoch_secs = 1
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["disk", "mkdir", "--account", "mock", "--path", "disk:/docs"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"AUTH_ERROR\""))
        .stderr(predicate::str::contains("stored OAuth token expired"));
}

#[test]
fn disk_mkdir_rejects_empty_path_before_network() {
    let temp = tempdir().expect("tempdir");
    let disk = Server::new();

    write_mock_account(temp.path(), &disk.url(), Some("store:disk"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args(["disk", "mkdir", "--account", "mock", "--path", ""])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "disk mkdir --path must not be empty",
        ));
}

#[test]
fn disk_mkdir_creates_directory_and_returns_metadata() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();

    write_mock_account(temp.path(), &server.url(), Some("store:disk"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    let _mkdir = server
        .mock("PUT", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::UrlEncoded("path".into(), "disk:/docs/new-folder".into()))
        .with_status(201)
        .create();

    let _metadata = server
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/new-folder".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "new-folder",
  "path": "disk:/docs/new-folder",
  "type": "dir",
  "created": "2026-03-12T21:00:00+00:00",
  "modified": "2026-03-12T21:00:00+00:00",
  "revision": 18,
  "_embedded": {
    "limit": 100,
    "offset": 0,
    "total": 0,
    "items": []
  }
}"#,
        )
        .create();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "disk",
            "mkdir",
            "--account",
            "mock",
            "--path",
            "disk:/docs/new-folder",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "disk.mkdir");
    assert_eq!(value["path"], "disk:/docs/new-folder");
    assert_eq!(value["resource"]["resource_type"], "dir");
    assert_eq!(value["resource"]["children"]["total"], 0);
}

#[test]
fn disk_upload_rejects_zero_length_file_before_network() {
    let temp = tempdir().expect("tempdir");
    let disk = Server::new();
    let source_path = temp.path().join("empty.txt");
    fs::write(&source_path, []).expect("empty file");

    write_mock_account(temp.path(), &disk.url(), Some("store:disk"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "disk",
            "upload",
            "--account",
            "mock",
            "--source",
            source_path.to_str().expect("utf8 path"),
            "--path",
            "disk:/docs/empty.txt",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"VALIDATION_ERROR\""))
        .stderr(predicate::str::contains(
            "disk upload --source file must not be empty",
        ));
}

#[test]
fn disk_upload_writes_file_and_returns_metadata() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();
    let source_path = temp.path().join("note.txt");
    fs::write(&source_path, b"hello disk").expect("source file");

    write_mock_account(temp.path(), &server.url(), Some("store:disk"));
    write_credentials_file(
        temp.path(),
        r#"
version = 1

[accounts.mock.services.disk]
kind = "oauth_pkce"
access_token = "disk-token"
token_type = "bearer"
expires_at_epoch_secs = 4102444800
scope = ["cloud_api:disk.write"]
client_id = "client-123"
"#,
    );

    let _ticket = server
        .mock("GET", "/v1/disk/resources/upload")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/note.txt".into()),
            Matcher::UrlEncoded("overwrite".into(), "false".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
  "href": "{}/upload-target/note.txt",
  "method": "PUT",
  "templated": false
}}"#,
            server.url()
        ))
        .create();

    let _upload = server
        .mock("PUT", "/upload-target/note.txt")
        .match_body(Matcher::Exact("hello disk".to_string()))
        .with_status(201)
        .create();

    let _metadata = server
        .mock("GET", "/v1/disk/resources")
        .match_header("authorization", "OAuth disk-token")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded("path".into(), "disk:/docs/note.txt".into()),
            Matcher::UrlEncoded("limit".into(), "100".into()),
            Matcher::UrlEncoded("offset".into(), "0".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "note.txt",
  "path": "disk:/docs/note.txt",
  "type": "file",
  "size": 10,
  "mime_type": "text/plain",
  "created": "2026-03-12T21:00:00+00:00",
  "modified": "2026-03-12T21:00:01+00:00",
  "md5": "5eb63bbbe01eeed093cb22bb8f5acdc3",
  "revision": 19
}"#,
        )
        .create();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "disk",
            "upload",
            "--account",
            "mock",
            "--source",
            source_path.to_str().expect("utf8 path"),
            "--path",
            "disk:/docs/note.txt",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "disk.upload");
    assert_eq!(value["path"], "disk:/docs/note.txt");
    assert_eq!(value["upload"]["bytes_written"], 10);
    assert_eq!(
        value["upload"]["source_path"].as_str(),
        Some(source_path.to_str().expect("utf8 path"))
    );
    assert_eq!(value["resource"]["resource_type"], "file");
    assert_eq!(value["resource"]["size"], 10);
}

#[test]
fn disk_public_show_returns_resource_metadata() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();

    write_accounts_file(
        temp.path(),
        &format!(
            r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "{}"
"#,
            server.url()
        ),
    );

    let _mock = server
        .mock("GET", "/v1/disk/public/resources")
        .match_query(Matcher::AllOf(vec![
            Matcher::UrlEncoded(
                "public_key".into(),
                "https://disk.yandex.ru/i/example".into(),
            ),
            Matcher::UrlEncoded("path".into(), "/docs".into()),
        ]))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "name": "docs",
  "path": "/docs",
  "type": "dir",
  "public_url": "https://disk.yandex.ru/d/example",
  "public_key": "public-key-value",
  "_embedded": {
    "limit": 20,
    "offset": 0,
    "total": 1,
    "items": [
      {
        "name": "guide.pdf",
        "path": "/docs/guide.pdf",
        "type": "file",
        "size": 42,
        "mime_type": "application/pdf",
        "public_url": "https://disk.yandex.ru/i/example",
        "public_key": "child-key"
      }
    ]
  }
}"#,
        )
        .create();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "disk",
            "public",
            "show",
            "--account",
            "mock",
            "--public-key",
            "https://disk.yandex.ru/i/example",
            "--path",
            "/docs",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "disk.public.show");
    assert_eq!(value["resource"]["name"], "docs");
    assert_eq!(value["resource"]["resource_type"], "dir");
    assert_eq!(value["resource"]["children"]["total"], 1);
    assert_eq!(
        value["resource"]["children"]["items"][0]["name"],
        "guide.pdf"
    );
}

#[test]
fn disk_public_show_surfaces_provider_errors() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();

    write_accounts_file(
        temp.path(),
        &format!(
            r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "{}"
"#,
            server.url()
        ),
    );

    let _mock = server
        .mock("GET", "/v1/disk/public/resources")
        .match_query(Matcher::UrlEncoded(
            "public_key".into(),
            "missing-key".into(),
        ))
        .with_status(404)
        .with_header("content-type", "application/json")
        .with_body(
            r#"{
  "message": "Resource not found.",
  "error": "DiskNotFoundError"
}"#,
        )
        .create();

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "disk",
            "public",
            "show",
            "--account",
            "mock",
            "--public-key",
            "missing-key",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"API_ERROR\""))
        .stderr(predicate::str::contains("DiskNotFoundError"))
        .stderr(predicate::str::contains("Resource not found."));
}

#[test]
fn disk_public_download_writes_file_and_reports_checksum() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();
    let output_path = temp.path().join("downloads").join("guide.pdf");

    write_accounts_file(
        temp.path(),
        &format!(
            r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "{}"
"#,
            server.url()
        ),
    );

    let body = b"%PDF-1.4\nmock pdf\n";
    let _metadata = server
        .mock("GET", "/v1/disk/public/resources")
        .match_query(Matcher::UrlEncoded(
            "public_key".into(),
            "https://disk.yandex.ru/i/example".into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
  "name": "guide.pdf",
  "path": "/guide.pdf",
  "type": "file",
  "size": {},
  "mime_type": "application/pdf",
  "public_url": "https://disk.yandex.ru/i/example",
  "public_key": "public-key-value",
  "file": "{}/download/guide.pdf"
}}"#,
            body.len(),
            server.url()
        ))
        .create();

    let _download = server
        .mock("GET", "/download/guide.pdf")
        .with_status(200)
        .with_header("content-type", "application/pdf")
        .with_body(body.as_slice())
        .create();

    let output = yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "disk",
            "public",
            "download",
            "--account",
            "mock",
            "--public-key",
            "https://disk.yandex.ru/i/example",
            "--output",
            output_path.to_str().expect("utf8 path"),
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("valid json");
    assert_eq!(value["operation"], "disk.public.download");
    assert_eq!(value["resource"]["name"], "guide.pdf");
    assert_eq!(
        value["download"]["bytes_written"].as_u64(),
        Some(body.len() as u64)
    );
    assert_eq!(
        value["download"]["output_path"].as_str(),
        Some(output_path.to_str().expect("utf8 path"))
    );
    assert_eq!(
        fs::read(&output_path).expect("downloaded file"),
        body.as_slice()
    );
    assert_eq!(
        value["download"]["sha256"].as_str().expect("sha256").len(),
        64
    );
}

#[test]
fn disk_public_download_refuses_to_overwrite_without_force() {
    let temp = tempdir().expect("tempdir");
    let mut server = Server::new();
    let output_path = temp.path().join("guide.pdf");
    fs::write(&output_path, b"existing").expect("existing file");

    write_accounts_file(
        temp.path(),
        &format!(
            r#"
version = 1

[accounts.mock]
email = "me@yandex.ru"
default = true

[accounts.mock.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.mock.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.mock.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "{}"
"#,
            server.url()
        ),
    );

    let _metadata = server
        .mock("GET", "/v1/disk/public/resources")
        .match_query(Matcher::UrlEncoded(
            "public_key".into(),
            "https://disk.yandex.ru/i/example".into(),
        ))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(format!(
            r#"{{
  "name": "guide.pdf",
  "path": "/guide.pdf",
  "type": "file",
  "size": 8,
  "mime_type": "application/pdf",
  "public_url": "https://disk.yandex.ru/i/example",
  "public_key": "public-key-value",
  "file": "{}/download/guide.pdf"
}}"#,
            server.url()
        ))
        .create();

    yacli()
        .env("YACLI_CONFIG_DIR", temp.path())
        .args([
            "disk",
            "public",
            "download",
            "--account",
            "mock",
            "--public-key",
            "https://disk.yandex.ru/i/example",
            "--output",
            output_path.to_str().expect("utf8 path"),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("\"code\":\"OUTPUT_EXISTS\""));

    assert_eq!(fs::read(&output_path).expect("existing file"), b"existing");
}
