use assert_cmd::cargo::cargo_bin;
use reqwest::blocking::{Client, Response};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::Duration;

const APP_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";

struct TestHttpServer {
    child: Child,
    addr: String,
}

impl TestHttpServer {
    fn spawn() -> Self {
        Self::spawn_with_args_envs(&[], &[])
    }

    fn spawn_with_envs(envs: &[(&str, &str)]) -> Self {
        Self::spawn_with_args_envs(&[], envs)
    }

    fn spawn_with_args_envs(args: &[&str], envs: &[(&str, &str)]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().expect("local addr").to_string();
        drop(listener);

        let mut command = Command::new(cargo_bin("yacli"));
        command
            .args(["mcp", "--transport", "http", "--listen", &addr])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for (key, value) in envs {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("spawn yacli mcp http");

        for _ in 0..200 {
            if let Some(status) = child.try_wait().expect("poll child status") {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    let _ = pipe.read_to_string(&mut stderr);
                }
                panic!("HTTP MCP server exited early with {status}: {stderr}");
            }
            if TcpStream::connect(&addr).is_ok() {
                return Self { child, addr };
            }
            thread::sleep(Duration::from_millis(50));
        }

        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        let _ = child.kill();
        let _ = child.wait();
        panic!("HTTP MCP server did not start at {addr}. stderr: {stderr}");
    }

    fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }
}

impl Drop for TestHttpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .expect("http client")
}

fn post_json(
    client: &Client,
    url: &str,
    body: Value,
    session_id: Option<&str>,
    origin: Option<&str>,
) -> Response {
    let mut request = client.post(url).json(&body);
    if let Some(session_id) = session_id {
        request = request.header("Mcp-Session-Id", session_id);
    }
    if let Some(origin) = origin {
        request = request.header("Origin", origin);
    }
    request.send().expect("http response")
}

fn open_sse(client: &Client, url: &str, session_id: &str) -> Response {
    client
        .get(url)
        .header("Accept", "text/event-stream")
        .header("Mcp-Session-Id", session_id)
        .send()
        .expect("sse response")
}

fn read_sse_json_message(reader: &mut BufReader<Response>) -> Value {
    let mut payload = String::new();

    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).expect("sse line");
        assert!(bytes > 0, "sse stream closed unexpectedly");
        if line == "\n" || line == "\r\n" {
            if !payload.is_empty() {
                return serde_json::from_str(&payload).expect("sse json payload");
            }
            continue;
        }
        if let Some(data) = line.strip_prefix("data: ") {
            payload.push_str(data.trim_end());
        }
    }
}

fn write_accounts_file(config_dir: &std::path::Path, content: &str) {
    std::fs::create_dir_all(config_dir).expect("config dir");
    std::fs::write(config_dir.join("accounts.toml"), content).expect("accounts file");
}

#[test]
fn mcp_http_initialize_returns_session_header_and_supports_follow_up_requests() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );

    assert!(initialize.status().is_success());
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();
    let payload: Value = initialize.json().expect("initialize json");
    assert_eq!(payload["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(
        payload["result"]["capabilities"]["resources"]["subscribe"],
        true
    );

    let tools_list = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );

    assert!(tools_list.status().is_success());
    let payload: Value = tools_list.json().expect("tools/list json");
    let tools = payload["result"]["tools"].as_array().expect("tools array");
    let account_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.account.current")
        .expect("account tool");
    assert_eq!(
        account_tool["_meta"]["ui"]["resourceUri"],
        "ui://yacli/dashboard"
    );
    let update_tool = tools
        .iter()
        .find(|tool| tool["name"] == "yacli.update.check")
        .expect("update tool");
    assert_eq!(
        update_tool["_meta"]["ui"]["resourceUri"],
        "ui://yacli/dashboard"
    );
}

#[test]
fn mcp_http_rejects_follow_up_requests_without_session_header() {
    let server = TestHttpServer::spawn();
    let client = client();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        None,
        None,
    );

    assert_eq!(response.status().as_u16(), 400);
    let payload: Value = response.json().expect("error json");
    assert_eq!(payload["ok"], false);
    assert!(
        payload["error"]
            .as_str()
            .expect("error text")
            .contains("Mcp-Session-Id")
    );
}

#[test]
fn mcp_http_rejects_non_local_origins() {
    let server = TestHttpServer::spawn();
    let client = client();

    let response = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        Some("https://example.com"),
    );

    assert_eq!(response.status().as_u16(), 403);
}

#[test]
fn mcp_http_protected_tools_require_bearer_token_when_configured() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@yandex.ru"
default = true

[accounts.personal.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.personal.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.personal.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );
    let config_dir = temp.path().display().to_string();
    let server = TestHttpServer::spawn_with_envs(&[
        ("YACLI_CONFIG_DIR", &config_dir),
        ("YACLI_MCP_HTTP_BEARER_TOKEN", "secret-token"),
        ("YACLI_MCP_HTTP_AUTH_ISSUER", "https://auth.example.test"),
    ]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let unauthorized = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.auth.status",
                "arguments": {}
            }
        }),
        Some(&session_id),
        None,
    );
    let expected_metadata_url = format!(
        "http://{}/.well-known/oauth-protected-resource/mcp",
        server.addr
    );
    assert_eq!(unauthorized.status().as_u16(), 401);
    assert_eq!(
        unauthorized
            .headers()
            .get("www-authenticate")
            .expect("auth challenge")
            .to_str()
            .expect("header text"),
        format!(
            "Bearer error=\"invalid_token\", resource_metadata=\"{}\", scope=\"yacli.auth.read\"",
            expected_metadata_url
        )
    );
    let unauthorized_payload: Value = unauthorized.json().expect("unauthorized json");
    assert_eq!(unauthorized_payload["error"], "invalid_token");
    assert_eq!(
        unauthorized_payload["resource_metadata"],
        expected_metadata_url
    );
    assert_eq!(unauthorized_payload["scope"], "yacli.auth.read");

    let authorized = client
        .post(server.url())
        .header("Mcp-Session-Id", &session_id)
        .header("Authorization", "Bearer secret-token")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.auth.status",
                "arguments": {}
            }
        }))
        .send()
        .expect("authorized response");

    assert!(authorized.status().is_success());
    let payload: Value = authorized.json().expect("authorized json");
    assert_eq!(
        payload["result"]["structuredContent"]["account"],
        "personal"
    );
}

#[test]
fn mcp_http_local_bearer_auth_works_without_auth_issuer() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@yandex.ru"
default = true

[accounts.personal.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465
"#,
    );
    let config_dir = temp.path().display().to_string();
    let server = TestHttpServer::spawn_with_envs(&[
        ("YACLI_CONFIG_DIR", &config_dir),
        ("YACLI_MCP_HTTP_BEARER_TOKEN", "secret-token"),
    ]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    assert!(initialize.status().is_success());
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let unauthorized = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "yacli.auth.status",
                "arguments": {}
            }
        }),
        Some(&session_id),
        None,
    );
    assert_eq!(unauthorized.status().as_u16(), 401);
    assert_eq!(
        unauthorized
            .headers()
            .get("www-authenticate")
            .expect("auth challenge")
            .to_str()
            .expect("header text"),
        "Bearer error=\"invalid_token\", scope=\"yacli.auth.read\""
    );
    let unauthorized_payload: Value = unauthorized.json().expect("unauthorized json");
    assert_eq!(unauthorized_payload["error"], "invalid_token");
    assert!(unauthorized_payload.get("resource_metadata").is_none());
    assert_eq!(unauthorized_payload["scope"], "yacli.auth.read");

    let metadata = client
        .get(format!(
            "http://{}/.well-known/oauth-protected-resource/mcp",
            server.addr
        ))
        .send()
        .expect("metadata response");
    assert_eq!(metadata.status().as_u16(), 404);

    let authorized = client
        .post(server.url())
        .header("Mcp-Session-Id", &session_id)
        .header("Authorization", "Bearer secret-token")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "yacli.auth.status",
                "arguments": {}
            }
        }))
        .send()
        .expect("authorized response");

    assert!(authorized.status().is_success());
}

#[test]
fn mcp_http_exposes_protected_resource_metadata_when_auth_is_enabled() {
    let server = TestHttpServer::spawn_with_envs(&[
        ("YACLI_MCP_HTTP_BEARER_TOKEN", "secret-token"),
        ("YACLI_MCP_HTTP_AUTH_ISSUER", "https://auth.example.test"),
    ]);
    let client = client();

    let response = client
        .get(format!(
            "http://{}/.well-known/oauth-protected-resource/mcp",
            server.addr
        ))
        .send()
        .expect("metadata response");

    assert!(response.status().is_success());
    let payload: Value = response.json().expect("metadata json");
    assert_eq!(payload["resource"], format!("http://{}/mcp", server.addr));
    assert_eq!(
        payload["authorization_servers"][0],
        "https://auth.example.test"
    );
    assert_eq!(payload["scopes_supported"][0], "yacli.auth.read");
    assert_eq!(payload["bearer_methods_supported"][0], "header");
}

#[test]
fn mcp_http_auth_discovery_uses_public_url_when_provided() {
    let server = TestHttpServer::spawn_with_args_envs(
        &["--public-url", "https://mcp.example.test/mcp"],
        &[
            ("YACLI_MCP_HTTP_BEARER_TOKEN", "secret-token"),
            ("YACLI_MCP_HTTP_AUTH_ISSUER", "https://auth.example.test"),
        ],
    );
    let client = client();

    let response = client
        .get(format!(
            "http://{}/.well-known/oauth-protected-resource/mcp",
            server.addr
        ))
        .send()
        .expect("metadata response");

    assert!(response.status().is_success());
    let payload: Value = response.json().expect("metadata json");
    assert_eq!(payload["resource"], "https://mcp.example.test/mcp");
}

#[test]
fn mcp_http_accepts_batch_requests_after_initialize() {
    let server = TestHttpServer::spawn();
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let batch = client
        .post(server.url())
        .header("Mcp-Session-Id", &session_id)
        .json(&json!([
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "ping",
                "params": {}
            },
            {
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/list",
                "params": {}
            }
        ]))
        .send()
        .expect("batch response");

    assert!(batch.status().is_success());
    let payload: Value = batch.json().expect("batch json");
    let responses = payload.as_array().expect("batch array");
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["id"], 2);
    assert_eq!(responses[1]["id"], 3);
}

#[test]
fn mcp_http_lists_resource_templates_and_reads_templated_resources() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@yandex.ru"
default = true

[accounts.personal.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.personal.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.personal.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );
    let config_dir = temp.path().display().to_string();
    let server = TestHttpServer::spawn_with_envs(&[("YACLI_CONFIG_DIR", &config_dir)]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let templates = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "resources/templates/list",
            "params": {}
        }),
        Some(&session_id),
        None,
    );
    assert!(templates.status().is_success());
    let templates_payload: Value = templates.json().expect("templates json");
    let resource_templates = templates_payload["result"]["resourceTemplates"]
        .as_array()
        .expect("templates array");
    assert!(
        resource_templates
            .iter()
            .any(|template| template["uriTemplate"] == "resource://yacli/account/{account}")
    );

    let account_resource = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "resources/read",
            "params": {
                "uri": "resource://yacli/account/personal"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(account_resource.status().is_success());
    let payload: Value = account_resource.json().expect("resource json");
    let contents = payload["result"]["contents"]
        .as_array()
        .expect("contents array");
    assert_eq!(contents[0]["mimeType"], "application/json");
    let account_payload: Value =
        serde_json::from_str(contents[0]["text"].as_str().expect("resource text"))
            .expect("account payload");
    assert_eq!(account_payload["account"], "personal");
    assert_eq!(account_payload["services"]["calendar"]["enabled"], true);
}

#[test]
fn mcp_http_sse_stream_receives_resource_update_notifications() {
    let temp = tempfile::tempdir().expect("tempdir");
    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "me@yandex.ru"
default = true

[accounts.personal.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.personal.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.personal.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );
    let config_dir = temp.path().display().to_string();
    let server = TestHttpServer::spawn_with_envs(&[("YACLI_CONFIG_DIR", &config_dir)]);
    let client = client();

    let initialize = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "extensions": {
                        "io.modelcontextprotocol/ui": {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE]
                        }
                    }
                },
                "clientInfo": { "name": "http-test", "version": "0.1.0" }
            }
        }),
        None,
        None,
    );
    let session_id = initialize
        .headers()
        .get("Mcp-Session-Id")
        .expect("session header")
        .to_str()
        .expect("header string")
        .to_string();

    let sse = open_sse(&client, &server.url(), &session_id);
    assert!(sse.status().is_success());
    let mut reader = BufReader::new(sse);

    let subscribe = post_json(
        &client,
        &server.url(),
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "resources/subscribe",
            "params": {
                "uri": "resource://yacli/account/personal"
            }
        }),
        Some(&session_id),
        None,
    );
    assert!(subscribe.status().is_success());

    write_accounts_file(
        temp.path(),
        r#"
version = 1

[accounts.personal]
email = "updated@yandex.ru"
default = true

[accounts.personal.mail]
enabled = true
auth_mode = "oauth_xoauth2"
imap_host = "imap.yandex.com"
imap_port = 993
smtp_host = "smtp.yandex.com"
smtp_port = 465

[accounts.personal.calendar]
enabled = true
auth_mode = "app_password"
caldav_base_url = "https://caldav.yandex.ru"

[accounts.personal.disk]
enabled = true
auth_mode = "oauth"
rest_base_url = "https://cloud-api.yandex.net"
"#,
    );

    let notification = read_sse_json_message(&mut reader);
    assert_eq!(notification["method"], "notifications/resources/updated");
    assert_eq!(
        notification["params"]["uri"],
        "resource://yacli/account/personal"
    );
}
