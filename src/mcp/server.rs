use std::collections::{BTreeMap, BTreeSet, HashMap, hash_map::DefaultHasher};
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Write};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header as http_header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rand::{Rng, distr::Alphanumeric};
use serde_json::{Map, Value, json};
use tokio::sync::{Mutex, broadcast};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;
use url::Url;

use crate::account_store::AccountStore;
use crate::credential_store::CredentialStore;
use crate::error::{Result, YacliError};
use crate::runtime_context::{
    auth_state, resolve_calendar_private_context, resolve_disk_private_context,
    resolve_mail_private_context,
};
use crate::update::check_for_update;
use crate::{
    calendar::{CalendarEventsRequest, list_calendar_events, list_calendars, parse_event_window},
    disk::{PrivateDiskListRequest, fetch_disk_info, fetch_private_resource},
    mail::{list_mail_folders, list_mail_messages, read_mail_message, search_mail_messages},
};
use chrono::{DateTime, Utc};

const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const APP_RESOURCE_URI: &str = "ui://yacli/dashboard";
const APP_RESOURCE_URI_TEMPLATE: &str = "ui://yacli/dashboard{?account,section,resource,tool}";
const APP_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";
const APP_EXTENSION_ID: &str = "io.modelcontextprotocol/ui";
const APP_ACCOUNT_QUERY_PARAM: &str = "account";
const APP_SECTION_QUERY_PARAM: &str = "section";
const APP_RESOURCE_QUERY_PARAM: &str = "resource";
const APP_TOOL_QUERY_PARAM: &str = "tool";
const HTTP_MCP_PATH: &str = "/mcp";
const MCP_SESSION_HEADER: &str = "Mcp-Session-Id";
const HTTP_AUTH_TOKEN_ENV: &str = "YACLI_MCP_HTTP_BEARER_TOKEN";
const HTTP_AUTH_ISSUER_ENV: &str = "YACLI_MCP_HTTP_AUTH_ISSUER";
const MODEL_AND_APP_VISIBILITY: &[&str] = &["model", "app"];
const APP_ONLY_VISIBILITY: &[&str] = &["app"];
const RESOURCE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const SSE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const PROTECTED_RESOURCE_METADATA_PATH: &str = "/.well-known/oauth-protected-resource";
const PROTECTED_RESOURCE_MCP_METADATA_PATH: &str = "/.well-known/oauth-protected-resource/mcp";
const DASHBOARD_SECTION_TOOLS: &str = "tools";
const DASHBOARD_SECTION_RESOURCES: &str = "resources";
const DASHBOARD_SECTION_AUTH: &str = "auth";
const DASHBOARD_RESOURCE_ACCOUNT: &str = "account";
const DASHBOARD_RESOURCE_AUTH: &str = "auth";
const DASHBOARD_TOOL_APP_SNAPSHOT: &str = "yacli.app.snapshot";
const DASHBOARD_TOOL_ACCOUNT_LIST: &str = "yacli.account.list";
const DASHBOARD_TOOL_ACCOUNT_CURRENT: &str = "yacli.account.current";
const DASHBOARD_TOOL_AUTH_STATUS: &str = "yacli.auth.status";
const DASHBOARD_TOOL_UPDATE_CHECK: &str = "yacli.update.check";

struct DashboardResourceState {
    default_account: Option<String>,
    preferred_section: Option<String>,
    preferred_resource: Option<String>,
    preferred_tool: Option<String>,
}

struct SessionState {
    initialized: bool,
    ui_enabled: bool,
    supports_resource_subscriptions: bool,
    resource_subscriptions: BTreeSet<String>,
}

impl SessionState {
    fn stdio() -> Self {
        Self {
            initialized: false,
            ui_enabled: false,
            supports_resource_subscriptions: true,
            resource_subscriptions: BTreeSet::new(),
        }
    }

    fn http() -> Self {
        Self {
            initialized: false,
            ui_enabled: false,
            supports_resource_subscriptions: true,
            resource_subscriptions: BTreeSet::new(),
        }
    }
}

#[derive(Default)]
struct ResourceSubscriptionPoller {
    digests: HashMap<String, u64>,
}

impl ResourceSubscriptionPoller {
    fn collect_notifications(&mut self, subscriptions: &BTreeSet<String>) -> Result<Vec<Value>> {
        self.digests
            .retain(|uri, _| subscriptions.contains(uri.as_str()));

        let mut notifications = Vec::new();
        for uri in subscriptions {
            let digest = resource_digest(uri)?;
            match self.digests.insert(uri.clone(), digest) {
                None => {}
                Some(previous) if previous != digest => {
                    notifications.push(resource_updated_notification(uri));
                }
                Some(_) => {}
            }
        }

        Ok(notifications)
    }

    fn track(&mut self, uri: &str) -> Result<()> {
        self.digests.insert(uri.to_string(), resource_digest(uri)?);
        Ok(())
    }

    fn untrack(&mut self, uri: &str) {
        self.digests.remove(uri);
    }
}

struct HttpSession {
    state: SessionState,
    poller: ResourceSubscriptionPoller,
    notifications: broadcast::Sender<Value>,
}

impl HttpSession {
    fn new() -> Self {
        let (notifications, _) = broadcast::channel(64);
        Self {
            state: SessionState::http(),
            poller: ResourceSubscriptionPoller::default(),
            notifications,
        }
    }
}

enum InputEvent {
    Message(Value),
    Eof,
    Error(YacliError),
}

#[derive(Clone)]
struct HttpAppState {
    sessions: Arc<Mutex<HashMap<String, HttpSession>>>,
    auth: HttpAuthConfig,
    auth_discovery: Option<HttpAuthDiscovery>,
}

#[derive(Clone, Default)]
struct HttpAuthConfig {
    bearer_token: Option<String>,
}

#[derive(Clone)]
struct HttpAuthDiscovery {
    authorization_servers: Vec<String>,
    canonical_server_url: String,
    protected_resource_metadata_url: String,
}

pub fn serve_http(listen: &str, public_url: Option<&str>) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| YacliError::Io(format!("failed to start HTTP runtime: {err}")))?;

    runtime.block_on(async move {
        let auth = HttpAuthConfig::from_env();
        let auth_discovery = HttpAuthDiscovery::from_config(listen, public_url, &auth)?;
        let state = HttpAppState {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            auth,
            auth_discovery,
        };
        spawn_http_resource_poller(state.clone());

        let app = Router::new()
            .route(
                PROTECTED_RESOURCE_METADATA_PATH,
                get(handle_http_protected_resource_metadata),
            )
            .route(
                PROTECTED_RESOURCE_MCP_METADATA_PATH,
                get(handle_http_protected_resource_metadata),
            )
            .route(
                HTTP_MCP_PATH,
                post(handle_http_post)
                    .get(handle_http_get)
                    .delete(handle_http_delete)
                    .options(handle_http_options),
            )
            .with_state(state);

        let listener = tokio::net::TcpListener::bind(listen).await.map_err(|err| {
            YacliError::Io(format!("failed to bind MCP HTTP server at {listen}: {err}"))
        })?;
        axum::serve(listener, app)
            .await
            .map_err(|err| YacliError::Io(format!("MCP HTTP server failed: {err}")))
    })
}

pub fn serve_stdio() -> Result<()> {
    let stdout = io::stdout();
    let mut writer = stdout.lock();
    let mut session = SessionState::stdio();
    let mut poller = ResourceSubscriptionPoller::default();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());

        loop {
            match read_message(&mut reader) {
                Ok(Some(message)) => {
                    if tx.send(InputEvent::Message(message)).is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = tx.send(InputEvent::Eof);
                    break;
                }
                Err(err) => {
                    let _ = tx.send(InputEvent::Error(err));
                    break;
                }
            }
        }
    });

    loop {
        match rx.recv_timeout(RESOURCE_POLL_INTERVAL) {
            Ok(InputEvent::Message(message)) => {
                if message.get("method").is_none() {
                    continue;
                }
                let method = message
                    .get("method")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        YacliError::Serialization("missing JSON-RPC method".to_string())
                    })?;
                let params = message.get("params").cloned().unwrap_or(Value::Null);

                if message.get("id").is_none() {
                    handle_notification(method, &mut session);
                    continue;
                }

                let id = message
                    .get("id")
                    .cloned()
                    .ok_or_else(|| YacliError::Serialization("missing JSON-RPC id".to_string()))?;

                let response = match handle_request(method, params, &mut session, Some(&mut poller))
                {
                    Ok(result) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result,
                    }),
                    Err(err) => json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": error_code(&err),
                            "message": err.to_string(),
                            "data": err.as_json(),
                        }
                    }),
                };
                write_message(&mut writer, &response)?;
            }
            Ok(InputEvent::Eof) => break,
            Ok(InputEvent::Error(err)) => return Err(err),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        for notification in poller.collect_notifications(&session.resource_subscriptions)? {
            write_message(&mut writer, &notification)?;
        }
    }

    Ok(())
}

async fn handle_http_post(
    State(state): State<HttpAppState>,
    headers: HeaderMap,
    Json(message): Json<Value>,
) -> Response {
    if let Err(err) = request_origin_allowed(&headers) {
        return http_error_response(StatusCode::FORBIDDEN, &err.to_string(), None);
    }

    let messages = match normalize_messages(message) {
        Ok(messages) => messages,
        Err(err) => return http_error_response(StatusCode::BAD_REQUEST, &err.to_string(), None),
    };

    if needs_http_auth(&messages) && !state.auth.authorized(&headers) {
        return http_unauthorized_response(
            required_scopes(&messages),
            state.auth_discovery.as_ref(),
        );
    }

    let method = match messages[0].get("method").and_then(Value::as_str) {
        Some(method) => method,
        None => {
            return http_error_response(StatusCode::BAD_REQUEST, "missing JSON-RPC method", None);
        }
    };

    let session_header = header_value(&headers, MCP_SESSION_HEADER);
    let session_id = if method == "initialize" {
        session_header.unwrap_or_else(new_session_id)
    } else if let Some(session_id) = session_header {
        session_id
    } else {
        return http_error_response(
            StatusCode::BAD_REQUEST,
            "missing Mcp-Session-Id header; call initialize first",
            None,
        );
    };

    let mut responses = Vec::new();
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions
            .entry(session_id.clone())
            .or_insert_with(HttpSession::new);
        for message in messages {
            match execute_message(message, &mut session.state, Some(&mut session.poller)) {
                Ok(Some(response)) => responses.push(response),
                Ok(None) => {}
                Err(err) => responses.push(json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": {
                        "code": error_code(&err),
                        "message": err.to_string(),
                        "data": err.as_json(),
                    }
                })),
            }
        }
    }

    if responses.is_empty() {
        return http_empty_response(StatusCode::ACCEPTED, Some(&session_id));
    }

    let payload = if responses.len() == 1 {
        responses.into_iter().next().unwrap_or(Value::Null)
    } else {
        Value::Array(responses)
    };
    http_json_response(StatusCode::OK, payload, Some(&session_id))
}

async fn handle_http_get(State(state): State<HttpAppState>, headers: HeaderMap) -> Response {
    if let Err(err) = request_origin_allowed(&headers) {
        return http_error_response(StatusCode::FORBIDDEN, &err.to_string(), None);
    }

    let session_id = match header_value(&headers, MCP_SESSION_HEADER) {
        Some(session_id) => session_id,
        None => {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "missing Mcp-Session-Id header; call initialize first",
                None,
            );
        }
    };
    let accept = header_value(&headers, "Accept").unwrap_or_default();
    if !accept.contains("text/event-stream") {
        return http_error_response(
            StatusCode::NOT_ACCEPTABLE,
            "SSE stream requires Accept: text/event-stream",
            None,
        );
    }

    let receiver = {
        let mut sessions = state.sessions.lock().await;
        let Some(session) = sessions.get_mut(&session_id) else {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "unknown Mcp-Session-Id session",
                None,
            );
        };
        if !session.state.initialized {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "MCP session is not initialized; call initialize first",
                None,
            );
        }
        session.notifications.subscribe()
    };

    let stream = BroadcastStream::new(receiver).filter_map(|result| match result {
        Ok(payload) => match serde_json::to_string(&payload) {
            Ok(data) => Some(Ok::<Event, Infallible>(
                Event::default().event("message").data(data),
            )),
            Err(_) => None,
        },
        Err(_) => None,
    });
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(SSE_KEEPALIVE_INTERVAL)
                .text("keepalive"),
        )
        .into_response();
    insert_common_http_headers(response.headers_mut(), Some(&session_id));
    response.headers_mut().insert(
        http_header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache"),
    );
    response
}

async fn handle_http_delete(State(state): State<HttpAppState>, headers: HeaderMap) -> Response {
    if let Err(err) = request_origin_allowed(&headers) {
        return http_error_response(StatusCode::FORBIDDEN, &err.to_string(), None);
    }

    let session_id = match header_value(&headers, MCP_SESSION_HEADER) {
        Some(session_id) => session_id,
        None => {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "missing Mcp-Session-Id header; call initialize first",
                None,
            );
        }
    };

    let mut sessions = state.sessions.lock().await;
    sessions.remove(&session_id);
    http_empty_response(StatusCode::NO_CONTENT, Some(&session_id))
}

async fn handle_http_options(headers: HeaderMap) -> Response {
    if let Err(err) = request_origin_allowed(&headers) {
        return http_error_response(StatusCode::FORBIDDEN, &err.to_string(), None);
    }
    http_empty_response(StatusCode::NO_CONTENT, None)
}

async fn handle_http_protected_resource_metadata(State(state): State<HttpAppState>) -> Response {
    let Some(auth_discovery) = state.auth_discovery.as_ref() else {
        return http_error_response(
            StatusCode::NOT_FOUND,
            "HTTP auth discovery is not configured",
            None,
        );
    };

    http_json_response(
        StatusCode::OK,
        json!({
            "resource": auth_discovery.canonical_server_url,
            "authorization_servers": auth_discovery.authorization_servers,
            "scopes_supported": [
                "yacli.auth.read",
                "yacli.mail.read",
                "yacli.calendar.read",
                "yacli.disk.read"
            ],
            "bearer_methods_supported": ["header"]
        }),
        None,
    )
}

fn spawn_http_resource_poller(state: HttpAppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(RESOURCE_POLL_INTERVAL).await;

            let mut sessions = state.sessions.lock().await;
            for session in sessions.values_mut() {
                let notifications = match session
                    .poller
                    .collect_notifications(&session.state.resource_subscriptions)
                {
                    Ok(notifications) => notifications,
                    Err(_) => continue,
                };

                for notification in notifications {
                    let _ = session.notifications.send(notification);
                }
            }
        }
    });
}

fn resource_updated_notification(uri: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/resources/updated",
        "params": {
            "uri": uri
        }
    })
}

fn execute_message(
    message: Value,
    session: &mut SessionState,
    poller: Option<&mut ResourceSubscriptionPoller>,
) -> Result<Option<Value>> {
    if message.get("method").is_none() {
        return Ok(None);
    }

    let method = message
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| YacliError::Serialization("missing JSON-RPC method".to_string()))?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    if message.get("id").is_none() {
        handle_notification(method, session);
        return Ok(None);
    }

    let id = message
        .get("id")
        .cloned()
        .ok_or_else(|| YacliError::Serialization("missing JSON-RPC id".to_string()))?;

    let response = match handle_request(method, params, session, poller) {
        Ok(result) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }),
        Err(err) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": error_code(&err),
                "message": err.to_string(),
                "data": err.as_json(),
            }
        }),
    };

    Ok(Some(response))
}

fn handle_notification(method: &str, session: &mut SessionState) {
    if method == "notifications/initialized" {
        session.initialized = true;
    }
}

fn handle_request(
    method: &str,
    params: Value,
    session: &mut SessionState,
    poller: Option<&mut ResourceSubscriptionPoller>,
) -> Result<Value> {
    if method != "initialize" && method != "ping" && !session.initialized {
        return Err(YacliError::Validation(
            "MCP session is not initialized; call `initialize` first".to_string(),
        ));
    }

    match method {
        "initialize" => {
            session.ui_enabled = client_supports_ui(&params);
            session.initialized = true;

            Ok(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": { "listChanged": false },
                    "resources": {
                        "listChanged": false,
                        "subscribe": session.supports_resource_subscriptions
                    },
                    "experimental": {
                        APP_EXTENSION_ID: {
                            "mimeTypes": [APP_RESOURCE_MIME_TYPE],
                            "resourceTemplates": true
                        }
                    }
                },
                "serverInfo": {
                    "name": "yacli",
                    "version": env!("CARGO_PKG_VERSION"),
                }
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions(session.ui_enabled) })),
        "tools/call" => call_tool(params, session.ui_enabled),
        "resources/list" => Ok(json!({ "resources": resource_definitions(session.ui_enabled) })),
        "resources/templates/list" => {
            Ok(json!({ "resourceTemplates": resource_templates(session.ui_enabled) }))
        }
        "resources/subscribe" => subscribe_resource(params, session, poller),
        "resources/unsubscribe" => unsubscribe_resource(params, session, poller),
        "resources/read" => read_resource(params),
        _ => Err(YacliError::UnsupportedOperation(format!(
            "unsupported MCP method: {method}"
        ))),
    }
}

fn tool_definitions(ui_enabled: bool) -> Vec<Value> {
    let mut tools = Vec::new();
    if ui_enabled {
        tools.push(tool(
            "yacli.app.snapshot",
            "Return the current yacli dashboard snapshot for the hosted app.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            Some(APP_ONLY_VISIBILITY),
            ui_enabled,
        ));
    }

    tools.extend([
        tool(
            "yacli.account.list",
            "List configured yacli accounts.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            Some(MODEL_AND_APP_VISIBILITY),
            ui_enabled,
        ),
        tool(
            "yacli.account.current",
            "Return the current yacli account.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            Some(MODEL_AND_APP_VISIBILITY),
            ui_enabled,
        ),
        tool(
            "yacli.auth.status",
            "Return auth status for one account or the current account.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" }
                },
                "additionalProperties": false
            }),
            Some(MODEL_AND_APP_VISIBILITY),
            ui_enabled,
        ),
        tool(
            DASHBOARD_TOOL_UPDATE_CHECK,
            "Check whether a newer published yacli release is available for this target.",
            json!({
                "type": "object",
                "properties": {
                    "version": { "type": "string" }
                },
                "additionalProperties": false
            }),
            Some(MODEL_AND_APP_VISIBILITY),
            ui_enabled,
        ),
        tool(
            "yacli.mail.folders",
            "List folders in the configured mailbox.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" }
                },
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.mail.list",
            "List messages from a folder.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.mail.search",
            "Search messages in a folder.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder": { "type": "string" },
                    "query": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "required": ["query"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.mail.read",
            "Read one message by UID.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder": { "type": "string" },
                    "uid": { "type": "integer", "minimum": 1 },
                    "max_bytes": { "type": "integer", "minimum": 1 }
                },
                "required": ["uid"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.calendar.calendars",
            "List calendars for the selected account.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" }
                },
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.calendar.events",
            "List upcoming calendar events.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "calendar": { "type": "string" },
                    "from": { "type": "string" },
                    "to": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.disk.info",
            "Return Yandex Disk quota information.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" }
                },
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.disk.list",
            "List Yandex Disk resources under a path.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "path": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1 },
                    "offset": { "type": "integer", "minimum": 0 }
                },
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
    ]);

    tools
}

fn tool(
    name: &str,
    description: &str,
    input_schema: Value,
    visibility: Option<&[&str]>,
    ui_enabled: bool,
) -> Value {
    let mut object = Map::new();
    object.insert("name".to_string(), Value::String(name.to_string()));
    object.insert(
        "description".to_string(),
        Value::String(description.to_string()),
    );
    object.insert("inputSchema".to_string(), input_schema);
    if let Some(visibility) = visibility
        && ui_enabled
    {
        object.insert("_meta".to_string(), app_meta(visibility));
    }
    Value::Object(object)
}

fn call_tool(params: Value, ui_enabled: bool) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| YacliError::Validation("tools/call requires `name`".to_string()))?;
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(Value::Object(Map::new()));

    let structured = match name {
        "yacli.app.snapshot" => app_snapshot()?,
        "yacli.account.list" => account_list()?,
        "yacli.account.current" => account_current()?,
        "yacli.auth.status" => auth_status(arguments.get("account").and_then(Value::as_str))?,
        DASHBOARD_TOOL_UPDATE_CHECK => {
            update_check(arguments.get("version").and_then(Value::as_str))?
        }
        "yacli.mail.folders" => mail_folders(arguments.get("account").and_then(Value::as_str))?,
        "yacli.mail.list" => mail_list(
            arguments.get("account").and_then(Value::as_str),
            optional_string(&arguments, "folder").unwrap_or("INBOX"),
            optional_usize(&arguments, "limit").unwrap_or(20),
        )?,
        "yacli.mail.search" => mail_search(
            arguments.get("account").and_then(Value::as_str),
            optional_string(&arguments, "folder").unwrap_or("INBOX"),
            required_string(&arguments, "query")?,
            optional_usize(&arguments, "limit").unwrap_or(20),
        )?,
        "yacli.mail.read" => mail_read(
            arguments.get("account").and_then(Value::as_str),
            optional_string(&arguments, "folder").unwrap_or("INBOX"),
            required_u64(&arguments, "uid")?,
            optional_u64(&arguments, "max_bytes").unwrap_or(15 * 1024 * 1024),
        )?,
        "yacli.calendar.calendars" => {
            calendar_calendars(arguments.get("account").and_then(Value::as_str))?
        }
        "yacli.calendar.events" => calendar_events(
            arguments.get("account").and_then(Value::as_str),
            optional_string(&arguments, "calendar").unwrap_or("default"),
            optional_string(&arguments, "from"),
            optional_string(&arguments, "to"),
            optional_usize(&arguments, "limit").unwrap_or(20),
        )?,
        "yacli.disk.info" => disk_info(arguments.get("account").and_then(Value::as_str))?,
        "yacli.disk.list" => disk_list(
            arguments.get("account").and_then(Value::as_str),
            optional_string(&arguments, "path").unwrap_or("disk:/"),
            optional_usize(&arguments, "limit").unwrap_or(100),
            optional_u64(&arguments, "offset").unwrap_or(0),
        )?,
        _ => {
            return Err(YacliError::UnsupportedOperation(format!(
                "unsupported MCP tool: {name}"
            )));
        }
    };

    let text = serde_json::to_string_pretty(&structured)
        .map_err(|err| YacliError::Serialization(err.to_string()))?;

    let mut response = Map::new();
    response.insert("structuredContent".to_string(), structured);
    response.insert(
        "content".to_string(),
        json!([
            {
                "type": "text",
                "text": text
            }
        ]),
    );
    if ui_enabled && let Some(visibility) = tool_visibility(name) {
        response.insert("_meta".to_string(), app_meta(visibility));
    }

    Ok(Value::Object(response))
}

fn resource_definitions(ui_enabled: bool) -> Vec<Value> {
    let mut resources = vec![json!({
        "uri": "resource://yacli/getting-started",
        "name": "yacli MCP Getting Started",
        "description": "Text guide for the stable yacli MCP surface",
        "mimeType": "text/markdown"
    })];
    if ui_enabled {
        resources.push(json!({
            "uri": APP_RESOURCE_URI,
            "name": "yacli MCP Dashboard",
            "description": "Minimal MCP Apps-compatible HTML surface for yacli",
            "mimeType": APP_RESOURCE_MIME_TYPE,
            "_meta": app_resource_meta()
        }));
    }
    resources
}

fn resource_templates(_ui_enabled: bool) -> Vec<Value> {
    let mut templates = vec![
        json!({
            "uriTemplate": "resource://yacli/account/{account}",
            "name": "yacli Account Resource",
            "description": "Read one configured yacli account summary as JSON",
            "mimeType": "application/json"
        }),
        json!({
            "uriTemplate": "resource://yacli/auth/{account}",
            "name": "yacli Auth Resource",
            "description": "Read auth posture for one configured yacli account as JSON",
            "mimeType": "application/json"
        }),
    ];

    if _ui_enabled {
        templates.push(json!({
            "uriTemplate": APP_RESOURCE_URI_TEMPLATE,
            "name": "yacli MCP Dashboard",
            "description": "Load the hosted yacli dashboard with an optional restored account alias",
            "mimeType": APP_RESOURCE_MIME_TYPE,
            "_meta": app_resource_meta()
        }));
    }

    templates
}

fn read_resource(params: Value) -> Result<Value> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| YacliError::Validation("resources/read requires `uri`".to_string()))?;

    let contents = resource_contents(uri)?;

    Ok(json!({ "contents": contents }))
}

fn subscribe_resource(
    params: Value,
    session: &mut SessionState,
    poller: Option<&mut ResourceSubscriptionPoller>,
) -> Result<Value> {
    if !session.supports_resource_subscriptions {
        return Err(YacliError::UnsupportedOperation(
            "resource subscriptions are not available on this transport".to_string(),
        ));
    }

    let uri = resource_uri_param(&params, "resources/subscribe")?;
    validate_subscribable_resource_uri(uri)?;
    session.resource_subscriptions.insert(uri.to_string());
    if let Some(poller) = poller {
        poller.track(uri)?;
    }
    Ok(json!({}))
}

fn unsubscribe_resource(
    params: Value,
    session: &mut SessionState,
    poller: Option<&mut ResourceSubscriptionPoller>,
) -> Result<Value> {
    if !session.supports_resource_subscriptions {
        return Err(YacliError::UnsupportedOperation(
            "resource subscriptions are not available on this transport".to_string(),
        ));
    }

    let uri = resource_uri_param(&params, "resources/unsubscribe")?;
    session.resource_subscriptions.remove(uri);
    if let Some(poller) = poller {
        poller.untrack(uri);
    }
    Ok(json!({}))
}

fn account_list() -> Result<Value> {
    let store = AccountStore::load()?;
    let items: Vec<_> = store
        .summaries()
        .into_iter()
        .map(|(name, account)| {
            json!({
                "name": name,
                "email": account.email,
                "current": store.is_current_account(&name),
                "services": {
                    "mail": account.mail.enabled,
                    "calendar": account.calendar.enabled,
                    "disk": account.disk.enabled,
                }
            })
        })
        .collect();
    Ok(json!({ "items": items }))
}

fn account_current() -> Result<Value> {
    let store = AccountStore::load()?;
    let name = store.current_account_name()?;
    let account = store.get_account(&name)?;
    Ok(json!({
        "account": name,
        "email": account.email,
    }))
}

fn auth_status(account: Option<&str>) -> Result<Value> {
    let account_store = AccountStore::load()?;
    let name = account_store.resolved_account_name(account)?;
    let account = account_store.get_account(&name)?;
    let credential_store = CredentialStore::load()?;

    Ok(json!({
        "account": name,
        "email": account.email,
        "services": BTreeMap::from([
            ("mail", auth_state(&credential_store, &name, account.mail.credential_ref.as_deref(), "mail")),
            ("calendar", auth_state(&credential_store, &name, account.calendar.credential_ref.as_deref(), "calendar")),
            ("disk", auth_state(&credential_store, &name, account.disk.credential_ref.as_deref(), "disk")),
        ])
    }))
}

fn update_check(version: Option<&str>) -> Result<Value> {
    let report = check_for_update(version)?;
    Ok(json!({
        "operation": report.operation,
        "status": report.status,
        "currentVersion": report.current_version,
        "targetVersion": report.target_version,
        "requestedVersion": report.requested_version,
        "asset": report.asset,
        "target": report.target,
        "baseUrl": report.base_url,
    }))
}

fn account_resource(uri: &str) -> Result<Value> {
    let account_name = templated_account_name(uri, "account")?;
    let account_store = AccountStore::load()?;
    let account = account_store.get_account(&account_name)?;

    Ok(json!({
        "account": account_name,
        "email": account.email,
        "current": account_store.is_current_account(&account_name),
        "services": {
            "mail": {
                "enabled": account.mail.enabled,
                "authMode": account.mail.auth_mode,
                "credentialRef": account.mail.credential_ref,
                "imapHost": account.mail.imap_host,
                "imapPort": account.mail.imap_port,
            },
            "calendar": {
                "enabled": account.calendar.enabled,
                "authMode": account.calendar.auth_mode,
                "credentialRef": account.calendar.credential_ref,
                "caldavBaseUrl": account.calendar.caldav_base_url,
            },
            "disk": {
                "enabled": account.disk.enabled,
                "authMode": account.disk.auth_mode,
                "credentialRef": account.disk.credential_ref,
                "restBaseUrl": account.disk.rest_base_url,
            }
        }
    }))
}

fn auth_resource(uri: &str) -> Result<Value> {
    let account_name = templated_account_name(uri, "auth")?;
    auth_status(Some(&account_name))
}

fn app_snapshot() -> Result<Value> {
    let account_store = AccountStore::load()?;
    let credential_store = CredentialStore::load()?;
    let current_account = account_store.current_account_name().ok();
    let auth_discovery = app_auth_discovery();

    let accounts = account_store
        .summaries()
        .into_iter()
        .map(|(name, account)| {
            json!({
                "name": name,
                "email": account.email,
                "current": current_account.as_deref() == Some(name.as_str()),
                "services": {
                    "mail": {
                        "enabled": account.mail.enabled,
                        "authMode": account.mail.auth_mode,
                        "auth": auth_state(
                            &credential_store,
                            &name,
                            account.mail.credential_ref.as_deref(),
                            "mail",
                        )
                    },
                    "calendar": {
                        "enabled": account.calendar.enabled,
                        "authMode": account.calendar.auth_mode,
                        "auth": auth_state(
                            &credential_store,
                            &name,
                            account.calendar.credential_ref.as_deref(),
                            "calendar",
                        )
                    },
                    "disk": {
                        "enabled": account.disk.enabled,
                        "authMode": account.disk.auth_mode,
                        "auth": auth_state(
                            &credential_store,
                            &name,
                            account.disk.credential_ref.as_deref(),
                            "disk",
                        )
                    }
                }
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "generatedAt": Utc::now().to_rfc3339(),
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "appResourceUri": APP_RESOURCE_URI,
        "accountCount": accounts.len(),
        "currentAccount": current_account,
        "authDiscovery": auth_discovery,
        "accounts": accounts,
    }))
}

fn mail_folders(account: Option<&str>) -> Result<Value> {
    let (resolved_account, auth, context) = resolve_mail_private_context(account)?;
    let folders = list_mail_folders(&context.imap_host, context.imap_port, auth)?;
    Ok(json!({
        "account": resolved_account,
        "items": folders,
    }))
}

fn mail_list(account: Option<&str>, folder: &str, limit: usize) -> Result<Value> {
    let (resolved_account, auth, context) = resolve_mail_private_context(account)?;
    let messages = list_mail_messages(&context.imap_host, context.imap_port, auth, folder, limit)?;
    Ok(json!({
        "account": resolved_account,
        "folder": folder,
        "items": messages,
    }))
}

fn mail_search(account: Option<&str>, folder: &str, query: &str, limit: usize) -> Result<Value> {
    let (resolved_account, auth, context) = resolve_mail_private_context(account)?;
    let messages = search_mail_messages(
        &context.imap_host,
        context.imap_port,
        auth,
        folder,
        query,
        limit,
    )?;
    Ok(json!({
        "account": resolved_account,
        "folder": folder,
        "query": query,
        "items": messages,
    }))
}

fn mail_read(account: Option<&str>, folder: &str, uid: u64, max_bytes: u64) -> Result<Value> {
    let (resolved_account, auth, context) = resolve_mail_private_context(account)?;
    let message = read_mail_message(
        &context.imap_host,
        context.imap_port,
        auth,
        folder,
        uid,
        max_bytes,
    )?;
    Ok(json!({
        "account": resolved_account,
        "folder": folder,
        "item": message,
    }))
}

fn calendar_calendars(account: Option<&str>) -> Result<Value> {
    let (resolved_account, app_password, context) = resolve_calendar_private_context(account)?;
    let calendars = list_calendars(&context.caldav_base_url, &context.email, &app_password)?;
    Ok(json!({
        "account": resolved_account,
        "items": calendars,
    }))
}

fn calendar_events(
    account: Option<&str>,
    calendar: &str,
    from: Option<&str>,
    to: Option<&str>,
    limit: usize,
) -> Result<Value> {
    let (resolved_account, app_password, context) = resolve_calendar_private_context(account)?;
    let window = parse_event_window(from, to, limit)?;
    let request = CalendarEventsRequest {
        calendar: calendar.to_string(),
        from: parse_rfc3339(&window.from)?,
        to: parse_rfc3339(&window.to)?,
        limit,
    };
    let (calendar, window, items) = list_calendar_events(
        &context.caldav_base_url,
        &context.email,
        &app_password,
        request,
    )?;
    Ok(json!({
        "account": resolved_account,
        "calendar": calendar,
        "window": window,
        "items": items,
    }))
}

fn disk_info(account: Option<&str>) -> Result<Value> {
    let (resolved_account, base_url, access_token) = resolve_disk_private_context(account)?;
    let info = fetch_disk_info(&base_url, &access_token)?;
    Ok(json!({
        "account": resolved_account,
        "info": info,
    }))
}

fn disk_list(account: Option<&str>, path: &str, limit: usize, offset: u64) -> Result<Value> {
    let (resolved_account, base_url, access_token) = resolve_disk_private_context(account)?;
    let resource = fetch_private_resource(
        &base_url,
        &access_token,
        &PrivateDiskListRequest {
            path: path.to_string(),
            limit,
            offset,
        },
    )?;
    Ok(json!({
        "account": resolved_account,
        "resource": resource,
    }))
}

fn required_string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| YacliError::Validation(format!("`{key}` is required")))
}

fn required_u64(value: &Value, key: &str) -> Result<u64> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| YacliError::Validation(format!("`{key}` must be a positive integer")))
}

fn optional_string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn optional_usize(value: &Value, key: &str) -> Option<usize> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map(|number| number as usize)
}

fn optional_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn parse_rfc3339(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|err| {
            YacliError::Validation(format!("invalid RFC3339 timestamp `{value}`: {err}"))
        })
}

fn app_html(uri: &str) -> Result<String> {
    let bootstrap_state = parse_dashboard_resource_state(uri)?;
    let bootstrap = serde_json::to_string(&json!({
        "resourceUri": uri,
        "defaultAccount": bootstrap_state.default_account,
        "preferredSection": bootstrap_state.preferred_section,
        "preferredResource": bootstrap_state.preferred_resource,
        "preferredTool": bootstrap_state.preferred_tool,
    }))
    .map_err(|err| YacliError::Serialization(err.to_string()))?;

    Ok(include_str!("dashboard_app.html")
        .replace("__YACLI_VERSION__", env!("CARGO_PKG_VERSION"))
        .replace("__YACLI_BOOTSTRAP__", &bootstrap))
}

fn resource_uri_param<'a>(params: &'a Value, method: &str) -> Result<&'a str> {
    params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| YacliError::Validation(format!("{method} requires `uri`")))
}

fn validate_subscribable_resource_uri(uri: &str) -> Result<()> {
    if !is_subscribable_resource_uri(uri) {
        return Err(YacliError::UnsupportedOperation(format!(
            "resource does not support subscriptions: {uri}"
        )));
    }

    let _ = resource_digest(uri)?;
    Ok(())
}

fn resource_contents(uri: &str) -> Result<Vec<Value>> {
    match uri {
        "resource://yacli/getting-started" => Ok(vec![json!({
            "uri": uri,
            "mimeType": "text/markdown",
            "text": "# yacli MCP\n\nStable read-only tools are available for accounts, auth status, mail, calendar, and disk.\n\nApps-ready clients can also load `ui://yacli/dashboard`."
        })]),
        dashboard_uri if is_dashboard_resource_uri(dashboard_uri) => Ok(vec![json!({
            "uri": dashboard_uri,
            "mimeType": APP_RESOURCE_MIME_TYPE,
            "text": app_html(dashboard_uri)?,
            "_meta": app_resource_meta()
        })]),
        account_uri if account_uri.starts_with("resource://yacli/account/") => {
            json_resource_contents(account_uri, account_resource(account_uri)?)
        }
        auth_uri if auth_uri.starts_with("resource://yacli/auth/") => {
            json_resource_contents(auth_uri, auth_resource(auth_uri)?)
        }
        _ => Err(YacliError::UnsupportedOperation(format!(
            "unknown MCP resource: {uri}"
        ))),
    }
}

fn is_dashboard_resource_uri(uri: &str) -> bool {
    parse_dashboard_resource_state(uri).is_ok()
}

fn is_subscribable_resource_uri(uri: &str) -> bool {
    uri.starts_with("resource://yacli/account/") || uri.starts_with("resource://yacli/auth/")
}

fn templated_account_name(uri: &str, namespace: &str) -> Result<String> {
    let parsed = Url::parse(uri)
        .map_err(|err| YacliError::Validation(format!("invalid resource URI `{uri}`: {err}")))?;
    let segments = parsed
        .path_segments()
        .ok_or_else(|| YacliError::Validation(format!("invalid resource URI `{uri}`")))?;
    let parts = segments.collect::<Vec<_>>();
    if parsed.scheme() != "resource"
        || parsed.host_str() != Some("yacli")
        || parts.len() != 2
        || parts[0] != namespace
        || parts[1].trim().is_empty()
    {
        return Err(YacliError::UnsupportedOperation(format!(
            "unknown MCP resource: {uri}"
        )));
    }

    Ok(parts[1].to_string())
}

fn parse_dashboard_resource_state(uri: &str) -> Result<DashboardResourceState> {
    let parsed = Url::parse(uri)
        .map_err(|err| YacliError::Validation(format!("invalid resource URI `{uri}`: {err}")))?;

    if parsed.scheme() != "ui"
        || parsed.host_str() != Some("yacli")
        || parsed.path() != "/dashboard"
    {
        return Err(YacliError::UnsupportedOperation(format!(
            "unknown MCP resource: {uri}"
        )));
    }

    let mut account = None;
    let mut section = None;
    let mut resource = None;
    let mut tool = None;
    for (key, value) in parsed.query_pairs() {
        match key.as_ref() {
            APP_ACCOUNT_QUERY_PARAM => {
                if value.trim().is_empty() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_ACCOUNT_QUERY_PARAM}` cannot be empty"
                    )));
                }
                if account.replace(value.into_owned()).is_some() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_ACCOUNT_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            APP_SECTION_QUERY_PARAM => {
                let value = value.into_owned();
                if !matches!(
                    value.as_str(),
                    DASHBOARD_SECTION_TOOLS | DASHBOARD_SECTION_RESOURCES | DASHBOARD_SECTION_AUTH
                ) {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_SECTION_QUERY_PARAM}` must be one of: {DASHBOARD_SECTION_TOOLS}, {DASHBOARD_SECTION_RESOURCES}, {DASHBOARD_SECTION_AUTH}"
                    )));
                }
                if section.replace(value).is_some() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_SECTION_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            APP_RESOURCE_QUERY_PARAM => {
                let value = value.into_owned();
                if !matches!(
                    value.as_str(),
                    DASHBOARD_RESOURCE_ACCOUNT | DASHBOARD_RESOURCE_AUTH
                ) {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_RESOURCE_QUERY_PARAM}` must be one of: {DASHBOARD_RESOURCE_ACCOUNT}, {DASHBOARD_RESOURCE_AUTH}"
                    )));
                }
                if resource.replace(value).is_some() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_RESOURCE_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            APP_TOOL_QUERY_PARAM => {
                let value = value.into_owned();
                if !matches!(
                    value.as_str(),
                    DASHBOARD_TOOL_APP_SNAPSHOT
                        | DASHBOARD_TOOL_ACCOUNT_LIST
                        | DASHBOARD_TOOL_ACCOUNT_CURRENT
                        | DASHBOARD_TOOL_AUTH_STATUS
                ) {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_TOOL_QUERY_PARAM}` must be one of: {DASHBOARD_TOOL_APP_SNAPSHOT}, {DASHBOARD_TOOL_ACCOUNT_LIST}, {DASHBOARD_TOOL_ACCOUNT_CURRENT}, {DASHBOARD_TOOL_AUTH_STATUS}"
                    )));
                }
                if tool.replace(value).is_some() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_TOOL_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            _ => {
                return Err(YacliError::UnsupportedOperation(format!(
                    "unknown MCP resource: {uri}"
                )));
            }
        }
    }

    if section.is_none() {
        if resource.is_some() {
            section = Some(DASHBOARD_SECTION_RESOURCES.to_string());
        } else if tool.is_some() {
            section = Some(DASHBOARD_SECTION_TOOLS.to_string());
        }
    }

    Ok(DashboardResourceState {
        default_account: account,
        preferred_section: section,
        preferred_resource: resource,
        preferred_tool: tool,
    })
}

fn json_resource_contents(uri: &str, payload: Value) -> Result<Vec<Value>> {
    let text = serde_json::to_string_pretty(&payload)
        .map_err(|err| YacliError::Serialization(err.to_string()))?;
    Ok(vec![json!({
        "uri": uri,
        "mimeType": "application/json",
        "text": text,
    })])
}

fn app_auth_discovery() -> Value {
    let auth = HttpAuthConfig::from_env();
    match HttpAuthDiscovery::from_config("127.0.0.1:8787", None, &auth) {
        Ok(Some(discovery)) => json!({
            "enabled": true,
            "authorizationServers": discovery.authorization_servers,
            "resourceMetadataUrl": discovery.protected_resource_metadata_url,
            "serverUrl": discovery.canonical_server_url,
        }),
        _ => json!({
            "enabled": false
        }),
    }
}

fn resource_digest(uri: &str) -> Result<u64> {
    let mut hasher = DefaultHasher::new();
    match resource_contents(uri) {
        Ok(contents) => {
            "ok".hash(&mut hasher);
            let encoded = serde_json::to_vec(&contents)
                .map_err(|err| YacliError::Serialization(err.to_string()))?;
            encoded.hash(&mut hasher);
        }
        Err(err) => {
            "err".hash(&mut hasher);
            err.code().hash(&mut hasher);
            err.to_string().hash(&mut hasher);
        }
    }
    Ok(hasher.finish())
}

fn tool_visibility(tool_name: &str) -> Option<&'static [&'static str]> {
    match tool_name {
        "yacli.app.snapshot" => Some(APP_ONLY_VISIBILITY),
        "yacli.account.list"
        | "yacli.account.current"
        | "yacli.auth.status"
        | DASHBOARD_TOOL_UPDATE_CHECK => Some(MODEL_AND_APP_VISIBILITY),
        _ => None,
    }
}

fn app_meta(visibility: &[&str]) -> Value {
    json!({
        "ui": {
            "resourceUri": APP_RESOURCE_URI,
            "visibility": visibility
        }
    })
}

fn app_resource_meta() -> Value {
    json!({
        "ui": {
            "prefersBorder": true,
            "csp": {
                "connectDomains": [],
                "resourceDomains": [],
                "frameDomains": []
            }
        }
    })
}

fn client_supports_ui(params: &Value) -> bool {
    capability_mime_types(params, "extensions")
        .into_iter()
        .chain(capability_mime_types(params, "experimental"))
        .any(|mime_type| mime_type == APP_RESOURCE_MIME_TYPE)
}

fn capability_mime_types<'a>(params: &'a Value, branch: &str) -> Vec<&'a str> {
    params
        .get("capabilities")
        .and_then(|capabilities| capabilities.get(branch))
        .and_then(|branch| branch.get(APP_EXTENSION_ID))
        .and_then(|ui| ui.get("mimeTypes"))
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn normalize_messages(payload: Value) -> Result<Vec<Value>> {
    match payload {
        Value::Array(messages) => {
            if messages.is_empty() {
                return Err(YacliError::Validation(
                    "JSON-RPC batch payload must not be empty".to_string(),
                ));
            }
            Ok(messages)
        }
        single => Ok(vec![single]),
    }
}

fn needs_http_auth(messages: &[Value]) -> bool {
    messages.iter().any(message_requires_http_auth)
}

fn message_requires_http_auth(message: &Value) -> bool {
    message.get("method").and_then(Value::as_str) == Some("tools/call")
        && message
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str)
            .map(tool_requires_http_auth)
            .unwrap_or(false)
}

fn tool_requires_http_auth(tool_name: &str) -> bool {
    matches!(tool_name, "yacli.auth.status")
        || tool_name.starts_with("yacli.mail.")
        || tool_name.starts_with("yacli.calendar.")
        || tool_name.starts_with("yacli.disk.")
}

fn read_message(reader: &mut dyn BufRead) -> Result<Option<Value>> {
    let mut content_length = None::<usize>;
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(None);
        }
        if line == "\r\n" || line == "\n" {
            break;
        }

        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            let parsed = value.trim().parse::<usize>().map_err(|err| {
                YacliError::Serialization(format!("invalid Content-Length header: {err}"))
            })?;
            content_length = Some(parsed);
        }
    }

    let length = content_length
        .ok_or_else(|| YacliError::Serialization("missing Content-Length header".to_string()))?;
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice::<Value>(&payload)
        .map(Some)
        .map_err(|err| YacliError::Serialization(format!("invalid JSON-RPC payload: {err}")))
}

fn write_message(writer: &mut dyn Write, payload: &Value) -> Result<()> {
    let encoded =
        serde_json::to_vec(payload).map_err(|err| YacliError::Serialization(err.to_string()))?;
    write!(writer, "Content-Length: {}\r\n\r\n", encoded.len())?;
    writer.write_all(&encoded)?;
    writer.flush()?;
    Ok(())
}

fn error_code(err: &YacliError) -> i64 {
    match err {
        YacliError::Validation(_) => -32602,
        YacliError::UnsupportedOperation(_) => -32601,
        _ => -32000,
    }
}

fn new_session_id() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(24)
        .map(char::from)
        .collect()
}

fn request_origin_allowed(headers: &HeaderMap) -> Result<()> {
    let Some(origin) = header_value(headers, "Origin") else {
        return Ok(());
    };

    let parsed = Url::parse(&origin).map_err(|err| {
        YacliError::Validation(format!("invalid Origin header `{origin}`: {err}"))
    })?;
    let Some(host) = parsed.host_str() else {
        return Err(YacliError::Validation(
            "origin is not allowed for local MCP HTTP transport".to_string(),
        ));
    };
    if matches!(host, "localhost" | "127.0.0.1" | "::1") {
        Ok(())
    } else {
        Err(YacliError::Validation(
            "origin is not allowed for local MCP HTTP transport".to_string(),
        ))
    }
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|header| header.to_str().ok())
        .map(str::to_string)
}

impl HttpAuthConfig {
    fn from_env() -> Self {
        let bearer_token = std::env::var(HTTP_AUTH_TOKEN_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Self { bearer_token }
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        let Some(expected) = &self.bearer_token else {
            return true;
        };
        let Some(authorization) = header_value(headers, "Authorization") else {
            return false;
        };
        authorization
            .strip_prefix("Bearer ")
            .map(|token| token == expected)
            .unwrap_or(false)
    }
}

impl HttpAuthDiscovery {
    fn from_config(
        listen: &str,
        public_url: Option<&str>,
        auth: &HttpAuthConfig,
    ) -> Result<Option<Self>> {
        if auth.bearer_token.is_none() {
            return Ok(None);
        }

        let authorization_server = std::env::var(HTTP_AUTH_ISSUER_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let Some(authorization_server) = authorization_server else {
            return Ok(None);
        };

        let canonical_server_url = canonical_server_url(listen, public_url)?;
        let protected_resource_metadata_url = format!(
            "{}{}",
            canonical_base_url(&canonical_server_url)?,
            PROTECTED_RESOURCE_MCP_METADATA_PATH
        );

        Ok(Some(Self {
            authorization_servers: vec![authorization_server],
            canonical_server_url,
            protected_resource_metadata_url,
        }))
    }
}

fn http_json_response(status: StatusCode, payload: Value, session_id: Option<&str>) -> Response {
    let mut response = (status, Json(payload)).into_response();
    insert_common_http_headers(response.headers_mut(), session_id);
    response
}

fn http_error_response(status: StatusCode, message: &str, session_id: Option<&str>) -> Response {
    http_json_response(
        status,
        json!({
            "ok": false,
            "error": message,
        }),
        session_id,
    )
}

fn http_unauthorized_response(
    scopes: Vec<String>,
    auth_discovery: Option<&HttpAuthDiscovery>,
) -> Response {
    let mut payload = json!({
        "ok": false,
        "error": "invalid_token",
        "error_description": "protected MCP tool requires Authorization: Bearer",
    });
    if let Some(auth_discovery) = auth_discovery {
        payload["resource_metadata"] =
            Value::String(auth_discovery.protected_resource_metadata_url.clone());
    }
    if !scopes.is_empty() {
        payload["scope"] = Value::String(scopes.join(" "));
    }

    let mut response = http_json_response(StatusCode::UNAUTHORIZED, payload, None);
    let mut challenge = String::from("Bearer error=\"invalid_token\"");
    if let Some(auth_discovery) = auth_discovery {
        challenge.push_str(&format!(
            ", resource_metadata=\"{}\"",
            auth_discovery.protected_resource_metadata_url
        ));
    }
    if !scopes.is_empty() {
        challenge.push_str(&format!(", scope=\"{}\"", scopes.join(" ")));
    }
    response.headers_mut().insert(
        http_header::WWW_AUTHENTICATE,
        HeaderValue::from_str(&challenge)
            .unwrap_or_else(|_| HeaderValue::from_static("Bearer error=\"invalid_token\"")),
    );
    response
}

fn http_empty_response(status: StatusCode, session_id: Option<&str>) -> Response {
    let mut response = status.into_response();
    insert_common_http_headers(response.headers_mut(), session_id);
    response
}

fn insert_common_http_headers(headers: &mut HeaderMap, session_id: Option<&str>) {
    headers.insert(
        http_header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type, Mcp-Session-Id, Origin, Authorization, Accept"),
    );
    headers.insert(
        http_header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, DELETE, OPTIONS"),
    );
    if let Some(session_id) = session_id
        && let Ok(value) = HeaderValue::from_str(session_id)
    {
        headers.insert(HeaderName::from_static("mcp-session-id"), value);
    }
}

fn required_scopes(messages: &[Value]) -> Vec<String> {
    let mut scopes = BTreeSet::new();
    for message in messages {
        if message.get("method").and_then(Value::as_str) != Some("tools/call") {
            continue;
        }
        let Some(name) = message
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if let Some(scope) = tool_scope(name) {
            scopes.insert(scope.to_string());
        }
    }
    scopes.into_iter().collect()
}

fn tool_scope(tool_name: &str) -> Option<&'static str> {
    match tool_name {
        "yacli.auth.status" => Some("yacli.auth.read"),
        name if name.starts_with("yacli.mail.") => Some("yacli.mail.read"),
        name if name.starts_with("yacli.calendar.") => Some("yacli.calendar.read"),
        name if name.starts_with("yacli.disk.") => Some("yacli.disk.read"),
        _ => None,
    }
}

fn canonical_server_url(listen: &str, public_url: Option<&str>) -> Result<String> {
    let raw = public_url
        .map(str::to_string)
        .unwrap_or_else(|| format!("http://{listen}{HTTP_MCP_PATH}"));
    let parsed = Url::parse(&raw)
        .map_err(|err| YacliError::Config(format!("invalid public MCP URL `{raw}`: {err}")))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(YacliError::Config(format!(
            "public MCP URL must use http or https: {raw}"
        )));
    }
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

fn canonical_base_url(server_url: &str) -> Result<String> {
    let mut parsed = Url::parse(server_url).map_err(|err| {
        YacliError::Config(format!("invalid public MCP URL `{server_url}`: {err}"))
    })?;
    parsed.set_path("");
    parsed.set_query(None);
    parsed.set_fragment(None);
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}
