use std::collections::{BTreeMap, BTreeSet, HashMap, hash_map::DefaultHasher};
use std::convert::Infallible;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Write};
use std::path::PathBuf;
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
use tokio::sync::{Mutex, broadcast, mpsc as tokio_mpsc, oneshot};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};
use url::Url;

use crate::account_store::AccountStore;
use crate::activity_store::{ActivityStore, NewActivityEntry, record_activity};
use crate::activity_undo::{apply_activity_undo, calendar_create_undo, disk_publish_undo};
use crate::commands::{apply_safe_doctor_remediation, doctor_safe_remediation_activity_entry};
use crate::credential_store::CredentialStore;
use crate::disk_link::{DiskUploadLinkRequest, review_disk_upload_link, upload_link_to_disk};
use crate::doctor::doctor_payload;
use crate::error::{Result, YacliError};
use crate::goal_router::goal_route_payload;
use crate::home::home_payload;
use crate::mail_link::{MailSendLinkRequest, review_mail_send_link, send_link_via_mail};
use crate::next_actions::next_actions_payload;
use crate::onboarding::onboarding_resource_payload;
use crate::runtime_context::{
    auth_state, resolve_calendar_private_context, resolve_disk_private_context,
    resolve_mail_private_context,
};
use crate::update::check_for_update;
use crate::workflows;
use crate::{
    calendar::{
        CalendarCreateRequest, CalendarEventsRequest, calendar_create_request_from_invites,
        create_calendar_event, delete_calendar_event, list_calendar_events, list_calendars,
        parse_event_window, review_calendar_event_creation,
    },
    disk::{
        DownloadedFile, PrivateDiskDownloadRequest, PrivateDiskListRequest,
        PrivateDiskMkdirRequest, PrivateDiskPublishRequest, PrivateDiskUnpublishRequest,
        PrivateDiskUploadRequest, create_private_directory, download_private_resource,
        fetch_disk_info, fetch_private_resource, publish_private_resource, review_private_publish,
        review_private_unpublish, review_private_upload, unpublish_private_resource,
        upload_private_resource,
    },
    mail::{
        MailAttachmentExportRequest, MailAttachmentSelector, MailInviteInspectRequest,
        export_mail_attachment, inspect_mail_invite, list_mail_folders, list_mail_messages,
        load_mail_attachments, read_mail_message, review_mail_submission, search_mail_messages,
        send_mail_message,
    },
};
use chrono::{DateTime, Utc};

use super::{prompts, skills};

const MCP_PROTOCOL_VERSION: &str = "2025-11-25";
const APP_RESOURCE_URI: &str = "ui://yacli/dashboard";
const APP_RESOURCE_URI_TEMPLATE: &str =
    "ui://yacli/dashboard{?account,section,resource,tool,skill,prompt,workflow,activity,goal}";
const APP_RESOURCE_MIME_TYPE: &str = "text/html;profile=mcp-app";
const APP_EXTENSION_ID: &str = "io.modelcontextprotocol/ui";
const APP_ACCOUNT_QUERY_PARAM: &str = "account";
const APP_SECTION_QUERY_PARAM: &str = "section";
const APP_RESOURCE_QUERY_PARAM: &str = "resource";
const APP_TOOL_QUERY_PARAM: &str = "tool";
const APP_SKILL_QUERY_PARAM: &str = "skill";
const APP_PROMPT_QUERY_PARAM: &str = "prompt";
const APP_WORKFLOW_QUERY_PARAM: &str = "workflow";
const APP_ACTIVITY_QUERY_PARAM: &str = "activity";
const APP_GOAL_QUERY_PARAM: &str = "goal";
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
const DASHBOARD_SECTION_HOME: &str = "home";
const DASHBOARD_SECTION_WORKFLOWS: &str = "workflows";
const DASHBOARD_SECTION_GOAL: &str = "goal";
const DASHBOARD_SECTION_PROMPTS: &str = "prompts";
const DASHBOARD_SECTION_RESOURCES: &str = "resources";
const DASHBOARD_SECTION_AUTH: &str = "auth";
const DASHBOARD_SECTION_ACTIVITY: &str = "activity";
const DASHBOARD_RESOURCE_ACCOUNT: &str = "account";
const DASHBOARD_RESOURCE_AUTH: &str = "auth";
const DASHBOARD_RESOURCE_SKILLS: &str = "skills";
const DASHBOARD_RESOURCE_SKILL: &str = "skill";
const DASHBOARD_TOOL_APP_SNAPSHOT: &str = "yacli.app.snapshot";
const DASHBOARD_TOOL_ACCOUNT_LIST: &str = "yacli.account.list";
const DASHBOARD_TOOL_ACCOUNT_CURRENT: &str = "yacli.account.current";
const DASHBOARD_TOOL_AUTH_STATUS: &str = "yacli.auth.status";
const DASHBOARD_TOOL_UPDATE_CHECK: &str = "yacli.update.check";
const DASHBOARD_TOOL_GOAL_ROUTE: &str = "yacli.goal.route";
const DASHBOARD_TOOL_DOCTOR_APPLY_SAFE: &str = "yacli.doctor.apply_safe";

struct DashboardResourceState {
    default_account: Option<String>,
    preferred_section: Option<String>,
    preferred_resource: Option<String>,
    preferred_tool: Option<String>,
    preferred_skill: Option<String>,
    preferred_prompt: Option<String>,
    preferred_workflow: Option<String>,
    preferred_activity: Option<String>,
    preferred_goal: Option<String>,
}

struct MailForwardToolRequest {
    folder: String,
    uid: u64,
    to: String,
    cc: Vec<String>,
    bcc: Vec<String>,
    text: Option<String>,
    html: Option<String>,
    max_source_bytes: u64,
}

struct MailSendToolRequest {
    to: String,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: String,
    text: Option<String>,
    html: Option<String>,
    attachment_paths: Vec<String>,
    dry_run: bool,
}

struct MailSendLinkToolRequest {
    to: String,
    cc: Vec<String>,
    bcc: Vec<String>,
    subject: String,
    text: Option<String>,
    html: Option<String>,
    source_path: String,
    disk_path: String,
    overwrite: bool,
    dry_run: bool,
}

struct MailAttachmentExportToolRequest {
    folder: String,
    uid: u64,
    selector: MailAttachmentSelector,
    output: String,
    force: bool,
    max_bytes: u64,
}

struct MailInviteInspectToolRequest {
    folder: String,
    uid: u64,
    selector: MailAttachmentSelector,
    max_bytes: u64,
}

struct MailInviteCreateEventToolRequest {
    folder: String,
    uid: u64,
    selector: MailAttachmentSelector,
    calendar: String,
    event_index: usize,
    max_bytes: u64,
}

struct CalendarCreateToolRequest {
    calendar: String,
    summary: String,
    start: String,
    end: String,
    description: Option<String>,
    location: Option<String>,
    dry_run: bool,
}

struct DiskUploadToolRequest {
    source: String,
    path: String,
    overwrite: bool,
    dry_run: bool,
}

struct DiskUploadLinkToolRequest {
    source: String,
    path: String,
    overwrite: bool,
    dry_run: bool,
}

struct DiskDownloadToolRequest {
    path: String,
    output_path: String,
    force: bool,
}

struct DiskPublishToolRequest {
    path: String,
    dry_run: bool,
}

struct DiskUnpublishToolRequest {
    path: String,
    dry_run: bool,
}

struct SessionState {
    initialized: bool,
    ui_enabled: bool,
    supports_resource_subscriptions: bool,
    supports_roots_requests: bool,
    roots_list_changed_supported: bool,
    roots_dirty: bool,
    cached_roots: Option<Vec<Value>>,
    next_outbound_request_id: u64,
    resource_subscriptions: BTreeSet<String>,
    stdio_message_format: StdioMessageFormat,
}

impl SessionState {
    fn stdio() -> Self {
        Self {
            initialized: false,
            ui_enabled: false,
            supports_resource_subscriptions: true,
            supports_roots_requests: false,
            roots_list_changed_supported: false,
            roots_dirty: false,
            cached_roots: None,
            next_outbound_request_id: 1,
            resource_subscriptions: BTreeSet::new(),
            stdio_message_format: StdioMessageFormat::ContentLength,
        }
    }

    fn http() -> Self {
        Self {
            initialized: false,
            ui_enabled: false,
            supports_resource_subscriptions: true,
            supports_roots_requests: false,
            roots_list_changed_supported: false,
            roots_dirty: false,
            cached_roots: None,
            next_outbound_request_id: 1,
            resource_subscriptions: BTreeSet::new(),
            stdio_message_format: StdioMessageFormat::ContentLength,
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
    pending_client_requests: HashMap<u64, oneshot::Sender<Value>>,
}

impl HttpSession {
    fn new() -> Self {
        let (notifications, _) = broadcast::channel(64);
        Self {
            state: SessionState::http(),
            poller: ResourceSubscriptionPoller::default(),
            notifications,
            pending_client_requests: HashMap::new(),
        }
    }
}

enum InputEvent {
    Message(Value, StdioMessageFormat),
    Eof,
    Error(YacliError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StdioMessageFormat {
    ContentLength,
    JsonLine,
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
                Ok(Some((message, format))) => {
                    if tx.send(InputEvent::Message(message, format)).is_err() {
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
            Ok(InputEvent::Message(message, format)) => {
                session.stdio_message_format = format;
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

                let response = match execute_stdio_request(
                    method,
                    params,
                    &mut session,
                    &mut writer,
                    &rx,
                    &mut poller,
                ) {
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
                write_message(&mut writer, &response, session.stdio_message_format)?;
            }
            Ok(InputEvent::Eof) => break,
            Ok(InputEvent::Error(err)) => return Err(err),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        for notification in poller.collect_notifications(&session.resource_subscriptions)? {
            write_message(&mut writer, &notification, session.stdio_message_format)?;
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

    if messages.iter().any(is_client_response_message) {
        if !messages.iter().all(is_client_response_message) {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "JSON-RPC responses cannot be mixed with requests in the same HTTP payload",
                None,
            );
        }
        return handle_http_client_responses(&state, &headers, messages).await;
    }

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

    if method == "tools/call"
        && requested_tool_name(messages[0].get("params").unwrap_or(&Value::Null))
            == Some("yacli.roots.list")
    {
        if messages.len() != 1 {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "yacli.roots.list must be called as a single JSON-RPC request over HTTP",
                Some(&session_id),
            );
        }
        return handle_http_roots_tool_call(
            state,
            headers,
            session_id,
            messages.into_iter().next().unwrap_or(Value::Null),
        )
        .await;
    }

    let mut responses = Vec::new();
    {
        let mut sessions = state.sessions.lock().await;
        let session = sessions
            .entry(session_id.clone())
            .or_insert_with(HttpSession::new);
        for message in messages {
            match tokio::task::block_in_place(|| {
                execute_message(message, &mut session.state, Some(&mut session.poller))
            }) {
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

async fn handle_http_client_responses(
    state: &HttpAppState,
    headers: &HeaderMap,
    messages: Vec<Value>,
) -> Response {
    let session_id = match header_value(headers, MCP_SESSION_HEADER) {
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
    let Some(session) = sessions.get_mut(&session_id) else {
        return http_error_response(
            StatusCode::BAD_REQUEST,
            "unknown Mcp-Session-Id session",
            None,
        );
    };

    for message in messages {
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "HTTP client response requires numeric JSON-RPC `id`",
                Some(&session_id),
            );
        };
        let Some(sender) = session.pending_client_requests.remove(&id) else {
            return http_error_response(
                StatusCode::BAD_REQUEST,
                "unknown pending HTTP client request id",
                Some(&session_id),
            );
        };
        let _ = sender.send(message);
    }

    http_empty_response(StatusCode::ACCEPTED, Some(&session_id))
}

async fn handle_http_roots_tool_call(
    state: HttpAppState,
    headers: HeaderMap,
    session_id: String,
    message: Value,
) -> Response {
    if !request_accepts_sse(&headers) {
        return http_error_response(
            StatusCode::NOT_ACCEPTABLE,
            "yacli.roots.list over HTTP requires Accept: text/event-stream",
            Some(&session_id),
        );
    }

    let original_id = message.get("id").cloned().unwrap_or(Value::Null);
    let mut sessions = state.sessions.lock().await;
    let Some(session) = sessions.get_mut(&session_id) else {
        return http_error_response(
            StatusCode::BAD_REQUEST,
            "unknown Mcp-Session-Id session",
            Some(&session_id),
        );
    };
    if !session.state.initialized {
        return http_error_response(
            StatusCode::BAD_REQUEST,
            "MCP session is not initialized; call initialize first",
            Some(&session_id),
        );
    }
    if !session.state.supports_roots_requests {
        let payload = jsonrpc_error_payload(
            original_id,
            &YacliError::UnsupportedOperation(
                "yacli.roots.list requires client roots capability".to_string(),
            ),
        );
        return http_json_response(StatusCode::OK, payload, Some(&session_id));
    }

    let outbound_request_id = session.state.next_outbound_request_id;
    session.state.next_outbound_request_id += 1;
    let ui_enabled = session.state.ui_enabled;
    let (roots_response_tx, roots_response_rx) = oneshot::channel();
    session
        .pending_client_requests
        .insert(outbound_request_id, roots_response_tx);
    drop(sessions);

    let (event_tx, event_rx) = tokio_mpsc::channel::<Value>(4);
    let state_for_task = state.clone();
    let session_id_for_task = session_id.clone();
    tokio::spawn(async move {
        let roots_request = json!({
            "jsonrpc": "2.0",
            "id": outbound_request_id,
            "method": "roots/list"
        });
        if event_tx.send(roots_request).await.is_err() {
            let mut sessions = state_for_task.sessions.lock().await;
            if let Some(session) = sessions.get_mut(&session_id_for_task) {
                session.pending_client_requests.remove(&outbound_request_id);
            }
            return;
        }

        let final_payload =
            match tokio::time::timeout(Duration::from_secs(30), roots_response_rx).await {
                Ok(Ok(client_response)) => match parse_roots_list_response(&client_response)
                    .and_then(|roots| {
                        let response = roots_tool_response(roots.clone(), ui_enabled)?;
                        Ok((roots, response))
                    }) {
                    Ok((roots, response)) => {
                        let mut sessions = state_for_task.sessions.lock().await;
                        if let Some(session) = sessions.get_mut(&session_id_for_task) {
                            session.state.cached_roots = Some(roots);
                            session.state.roots_dirty = false;
                        }
                        json!({
                            "jsonrpc": "2.0",
                            "id": original_id,
                            "result": response,
                        })
                    }
                    Err(err) => jsonrpc_error_payload(original_id, &err),
                },
                Ok(Err(_)) => jsonrpc_error_payload(
                    original_id,
                    &YacliError::Io(
                        "HTTP client closed roots/list response channel before replying"
                            .to_string(),
                    ),
                ),
                Err(_) => {
                    let mut sessions = state_for_task.sessions.lock().await;
                    if let Some(session) = sessions.get_mut(&session_id_for_task) {
                        session.pending_client_requests.remove(&outbound_request_id);
                    }
                    jsonrpc_error_payload(
                        original_id,
                        &YacliError::Io(
                            "timed out waiting for HTTP client roots/list response".to_string(),
                        ),
                    )
                }
            };

        let _ = event_tx.send(final_payload).await;
    });

    let stream =
        ReceiverStream::new(event_rx).filter_map(|payload| match serde_json::to_string(&payload) {
            Ok(data) => Some(Ok::<Event, Infallible>(
                Event::default().event("message").data(data),
            )),
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
    match method {
        "notifications/initialized" => {
            session.initialized = true;
        }
        "notifications/roots/list_changed" => {
            session.roots_dirty = true;
            session.cached_roots = None;
        }
        _ => {}
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
            session.roots_list_changed_supported = client_supports_roots_list_changed(&params);
            session.supports_roots_requests = client_supports_roots(&params);
            session.roots_dirty = session.supports_roots_requests;
            session.cached_roots = None;
            session.initialized = true;

            Ok(json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "completions": {},
                    "prompts": { "listChanged": false },
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
        "completion/complete" => prompts::complete(params),
        "prompts/list" => Ok(json!({ "prompts": prompts::prompt_definitions() })),
        "prompts/get" => prompts::get_prompt(params),
        "tools/list" => Ok(json!({
            "tools": tool_definitions(session.ui_enabled, session.supports_roots_requests)
        })),
        "tools/call" => {
            if requested_tool_name(&params) == Some("yacli.roots.list") {
                return Err(YacliError::UnsupportedOperation(
                    "yacli.roots.list requires stdio transport with client roots capability"
                        .to_string(),
                ));
            }
            call_tool(params, session.ui_enabled)
        }
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

fn tool_definitions(ui_enabled: bool, roots_enabled: bool) -> Vec<Value> {
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

    if roots_enabled {
        tools.push(tool(
            "yacli.roots.list",
            "Request the current MCP client filesystem roots for this session.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            Some(MODEL_AND_APP_VISIBILITY),
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
            "yacli.activity.undo",
            "Undo one reversible activity entry by ID.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string" }
                },
                "required": ["id"],
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
            DASHBOARD_TOOL_GOAL_ROUTE,
            "Route a natural-language goal into the best yacli workflow, prompt and MCP tool-path.",
            json!({
                "type": "object",
                "properties": {
                    "goal": { "type": "string" },
                    "account": { "type": "string" }
                },
                "required": ["goal"],
                "additionalProperties": false
            }),
            Some(MODEL_AND_APP_VISIBILITY),
            ui_enabled,
        ),
        tool(
            DASHBOARD_TOOL_DOCTOR_APPLY_SAFE,
            "Apply only safe local remediation steps from the current doctor and goal context, then return the refreshed doctor state.",
            json!({
                "type": "object",
                "properties": {
                    "goal": { "type": "string" },
                    "account": { "type": "string" }
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
            "yacli.mail.send",
            "Send one mail message.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "to": { "type": "string" },
                    "cc": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "bcc": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "subject": { "type": "string" },
                    "text": { "type": "string" },
                    "html": { "type": "string" },
                    "attachments": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "dry_run": { "type": "boolean" }
                },
                "required": ["to", "subject"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.mail.send_link",
            "Upload one local file to Yandex Disk, publish it, and send the public link by email.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "to": { "type": "string" },
                    "cc": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "bcc": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "subject": { "type": "string" },
                    "text": { "type": "string" },
                    "html": { "type": "string" },
                    "source_path": { "type": "string" },
                    "disk_path": { "type": "string" },
                    "overwrite": { "type": "boolean" },
                    "dry_run": { "type": "boolean" }
                },
                "required": ["to", "subject", "source_path", "disk_path"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.mail.reply",
            "Reply to one message by UID.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder": { "type": "string" },
                    "uid": { "type": "integer", "minimum": 1 },
                    "text": { "type": "string" },
                    "html": { "type": "string" },
                    "cc": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "required": ["uid"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.mail.forward",
            "Forward one message by UID.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder": { "type": "string" },
                    "uid": { "type": "integer", "minimum": 1 },
                    "to": { "type": "string" },
                    "text": { "type": "string" },
                    "html": { "type": "string" },
                    "cc": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "bcc": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "max_source_bytes": { "type": "integer", "minimum": 1 }
                },
                "required": ["uid", "to"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.mail.attachment.export",
            "Export one mail attachment to a local file on the MCP server host.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder": { "type": "string" },
                    "uid": { "type": "integer", "minimum": 1 },
                    "index": { "type": "integer", "minimum": 1 },
                    "name": { "type": "string" },
                    "output_path": { "type": "string" },
                    "force": { "type": "boolean" },
                    "max_bytes": { "type": "integer", "minimum": 1 }
                },
                "required": ["uid", "output_path"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.mail.invite.inspect",
            "Inspect one calendar invite attachment and return parsed VEVENT fields.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder": { "type": "string" },
                    "uid": { "type": "integer", "minimum": 1 },
                    "index": { "type": "integer", "minimum": 1 },
                    "name": { "type": "string" },
                    "max_bytes": { "type": "integer", "minimum": 1 }
                },
                "required": ["uid"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.mail.invite.create_event",
            "Create one calendar event from a VEVENT in a mail invite attachment.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "folder": { "type": "string" },
                    "uid": { "type": "integer", "minimum": 1 },
                    "index": { "type": "integer", "minimum": 1 },
                    "name": { "type": "string" },
                    "calendar": { "type": "string" },
                    "event_index": { "type": "integer", "minimum": 1 },
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
            "yacli.calendar.create",
            "Create one calendar event.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "calendar": { "type": "string" },
                    "summary": { "type": "string" },
                    "start": { "type": "string" },
                    "end": { "type": "string" },
                    "description": { "type": "string" },
                    "location": { "type": "string" },
                    "dry_run": { "type": "boolean" }
                },
                "required": ["summary", "start", "end"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.calendar.delete",
            "Delete one calendar event by UID.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "calendar": { "type": "string" },
                    "uid": { "type": "string" }
                },
                "required": ["uid"],
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
        tool(
            "yacli.disk.mkdir",
            "Create one Yandex Disk directory.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "path": { "type": "string" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.disk.upload",
            "Upload one local file to Yandex Disk.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "source": { "type": "string" },
                    "path": { "type": "string" },
                    "overwrite": { "type": "boolean" },
                    "dry_run": { "type": "boolean" }
                },
                "required": ["source", "path"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.disk.upload_link",
            "Upload one local file to Yandex Disk and immediately publish a public link.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "source": { "type": "string" },
                    "path": { "type": "string" },
                    "overwrite": { "type": "boolean" },
                    "dry_run": { "type": "boolean" }
                },
                "required": ["source", "path"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.disk.download",
            "Download one private Yandex Disk file to a local path on the MCP server host.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "path": { "type": "string" },
                    "output_path": { "type": "string" },
                    "force": { "type": "boolean" }
                },
                "required": ["path", "output_path"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.disk.publish",
            "Publish one private Yandex Disk resource and return its public URL.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "path": { "type": "string" },
                    "dry_run": { "type": "boolean" }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            None,
            ui_enabled,
        ),
        tool(
            "yacli.disk.unpublish",
            "Revoke the public URL of one private Yandex Disk resource.",
            json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string" },
                    "path": { "type": "string" },
                    "dry_run": { "type": "boolean" }
                },
                "required": ["path"],
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
        DASHBOARD_TOOL_GOAL_ROUTE => goal_route_payload(
            required_string(&arguments, "goal")?,
            arguments.get("account").and_then(Value::as_str),
        )?,
        DASHBOARD_TOOL_DOCTOR_APPLY_SAFE => {
            let structured = apply_safe_doctor_remediation(
                arguments.get("account").and_then(Value::as_str),
                arguments.get("goal").and_then(Value::as_str),
            )?;
            if let Some(entry) = doctor_safe_remediation_activity_entry(
                "mcp",
                &structured,
                arguments.get("account").and_then(Value::as_str),
                arguments.get("goal").and_then(Value::as_str),
            ) {
                record_activity_mcp(entry);
            }
            structured
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
        "yacli.mail.send" => mail_send(
            arguments.get("account").and_then(Value::as_str),
            MailSendToolRequest {
                to: required_string(&arguments, "to")?.to_string(),
                cc: string_list(&arguments, "cc")?,
                bcc: string_list(&arguments, "bcc")?,
                subject: required_string(&arguments, "subject")?.to_string(),
                text: optional_string_owned(&arguments, "text"),
                html: optional_string_owned(&arguments, "html"),
                attachment_paths: string_list(&arguments, "attachments")?,
                dry_run: optional_bool(&arguments, "dry_run").unwrap_or(false),
            },
        )?,
        "yacli.mail.send_link" => mail_send_link(
            arguments.get("account").and_then(Value::as_str),
            MailSendLinkToolRequest {
                to: required_string(&arguments, "to")?.to_string(),
                cc: string_list(&arguments, "cc")?,
                bcc: string_list(&arguments, "bcc")?,
                subject: required_string(&arguments, "subject")?.to_string(),
                text: optional_string_owned(&arguments, "text"),
                html: optional_string_owned(&arguments, "html"),
                source_path: required_string(&arguments, "source_path")?.to_string(),
                disk_path: required_string(&arguments, "disk_path")?.to_string(),
                overwrite: optional_bool(&arguments, "overwrite").unwrap_or(false),
                dry_run: optional_bool(&arguments, "dry_run").unwrap_or(false),
            },
        )?,
        "yacli.mail.reply" => mail_reply(
            arguments.get("account").and_then(Value::as_str),
            optional_string(&arguments, "folder").unwrap_or("INBOX"),
            required_u64(&arguments, "uid")?,
            string_list(&arguments, "cc")?,
            optional_string_owned(&arguments, "text"),
            optional_string_owned(&arguments, "html"),
        )?,
        "yacli.mail.forward" => mail_forward(
            arguments.get("account").and_then(Value::as_str),
            MailForwardToolRequest {
                folder: optional_string(&arguments, "folder")
                    .unwrap_or("INBOX")
                    .to_string(),
                uid: required_u64(&arguments, "uid")?,
                to: required_string(&arguments, "to")?.to_string(),
                cc: string_list(&arguments, "cc")?,
                bcc: string_list(&arguments, "bcc")?,
                text: optional_string_owned(&arguments, "text"),
                html: optional_string_owned(&arguments, "html"),
                max_source_bytes: optional_u64(&arguments, "max_source_bytes")
                    .unwrap_or(15 * 1024 * 1024),
            },
        )?,
        "yacli.mail.attachment.export" => mail_attachment_export(
            arguments.get("account").and_then(Value::as_str),
            MailAttachmentExportToolRequest {
                folder: optional_string(&arguments, "folder")
                    .unwrap_or("INBOX")
                    .to_string(),
                uid: required_u64(&arguments, "uid")?,
                selector: mail_attachment_selector(&arguments)?,
                output: required_string(&arguments, "output_path")?.to_string(),
                force: optional_bool(&arguments, "force").unwrap_or(false),
                max_bytes: optional_u64(&arguments, "max_bytes").unwrap_or(15 * 1024 * 1024),
            },
        )?,
        "yacli.mail.invite.inspect" => mail_invite_inspect(
            arguments.get("account").and_then(Value::as_str),
            MailInviteInspectToolRequest {
                folder: optional_string(&arguments, "folder")
                    .unwrap_or("INBOX")
                    .to_string(),
                uid: required_u64(&arguments, "uid")?,
                selector: mail_attachment_selector_with_context(
                    &arguments,
                    "yacli.mail.invite.inspect",
                )?,
                max_bytes: optional_u64(&arguments, "max_bytes").unwrap_or(15 * 1024 * 1024),
            },
        )?,
        "yacli.mail.invite.create_event" => mail_invite_create_event(
            arguments.get("account").and_then(Value::as_str),
            MailInviteCreateEventToolRequest {
                folder: optional_string(&arguments, "folder")
                    .unwrap_or("INBOX")
                    .to_string(),
                uid: required_u64(&arguments, "uid")?,
                selector: mail_attachment_selector_with_context(
                    &arguments,
                    "yacli.mail.invite.create_event",
                )?,
                calendar: optional_string(&arguments, "calendar")
                    .unwrap_or("default")
                    .to_string(),
                event_index: optional_usize(&arguments, "event_index").unwrap_or(1),
                max_bytes: optional_u64(&arguments, "max_bytes").unwrap_or(15 * 1024 * 1024),
            },
        )?,
        "yacli.activity.undo" => activity_undo(required_string(&arguments, "id")?)?,
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
        "yacli.calendar.create" => calendar_create(
            arguments.get("account").and_then(Value::as_str),
            CalendarCreateToolRequest {
                calendar: optional_string(&arguments, "calendar")
                    .unwrap_or("default")
                    .to_string(),
                summary: required_string(&arguments, "summary")?.to_string(),
                start: required_string(&arguments, "start")?.to_string(),
                end: required_string(&arguments, "end")?.to_string(),
                description: optional_string_owned(&arguments, "description"),
                location: optional_string_owned(&arguments, "location"),
                dry_run: optional_bool(&arguments, "dry_run").unwrap_or(false),
            },
        )?,
        "yacli.calendar.delete" => calendar_delete(
            arguments.get("account").and_then(Value::as_str),
            optional_string(&arguments, "calendar").unwrap_or("default"),
            required_string(&arguments, "uid")?,
        )?,
        "yacli.disk.info" => disk_info(arguments.get("account").and_then(Value::as_str))?,
        "yacli.disk.list" => disk_list(
            arguments.get("account").and_then(Value::as_str),
            optional_string(&arguments, "path").unwrap_or("disk:/"),
            optional_usize(&arguments, "limit").unwrap_or(100),
            optional_u64(&arguments, "offset").unwrap_or(0),
        )?,
        "yacli.disk.mkdir" => disk_mkdir(
            arguments.get("account").and_then(Value::as_str),
            required_string(&arguments, "path")?,
        )?,
        "yacli.disk.upload" => disk_upload(
            arguments.get("account").and_then(Value::as_str),
            DiskUploadToolRequest {
                source: required_string(&arguments, "source")?.to_string(),
                path: required_string(&arguments, "path")?.to_string(),
                overwrite: optional_bool(&arguments, "overwrite").unwrap_or(false),
                dry_run: optional_bool(&arguments, "dry_run").unwrap_or(false),
            },
        )?,
        "yacli.disk.upload_link" => disk_upload_link(
            arguments.get("account").and_then(Value::as_str),
            DiskUploadLinkToolRequest {
                source: required_string(&arguments, "source")?.to_string(),
                path: required_string(&arguments, "path")?.to_string(),
                overwrite: optional_bool(&arguments, "overwrite").unwrap_or(false),
                dry_run: optional_bool(&arguments, "dry_run").unwrap_or(false),
            },
        )?,
        "yacli.disk.download" => disk_download(
            arguments.get("account").and_then(Value::as_str),
            DiskDownloadToolRequest {
                path: required_string(&arguments, "path")?.to_string(),
                output_path: required_string(&arguments, "output_path")?.to_string(),
                force: optional_bool(&arguments, "force").unwrap_or(false),
            },
        )?,
        "yacli.disk.publish" => disk_publish(
            arguments.get("account").and_then(Value::as_str),
            DiskPublishToolRequest {
                path: required_string(&arguments, "path")?.to_string(),
                dry_run: optional_bool(&arguments, "dry_run").unwrap_or(false),
            },
        )?,
        "yacli.disk.unpublish" => disk_unpublish(
            arguments.get("account").and_then(Value::as_str),
            DiskUnpublishToolRequest {
                path: required_string(&arguments, "path")?.to_string(),
                dry_run: optional_bool(&arguments, "dry_run").unwrap_or(false),
            },
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
    let mut resources = vec![
        json!({
            "uri": "resource://yacli/getting-started",
            "name": "yacli MCP Getting Started",
            "description": "Text guide for the stable yacli MCP surface",
            "mimeType": "text/markdown"
        }),
        json!({
            "uri": "resource://yacli/home",
            "name": "yacli Home",
            "description": "Canonical unified home summary for account, onboarding, doctor, workflows, activity and next actions",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "resource://yacli/onboarding",
            "name": "yacli Onboarding",
            "description": "Live onboarding checklist driven by account, auth posture and optional goal context",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "resource://yacli/doctor",
            "name": "yacli Doctor",
            "description": "Health-check for config, secret backend, service readiness, workflows and optional goal context",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "resource://yacli/next-actions",
            "name": "yacli Next Actions",
            "description": "Ranked next steps with the highest product payoff and optional goal-aware prioritization",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "resource://yacli/skills",
            "name": "yacli Embedded Skills",
            "description": "Catalog of embedded yacli SKILL.md workflows mirrored into MCP resources",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "resource://yacli/workflows",
            "name": "yacli Workflow Hub",
            "description": "Catalog of cross-service workflows exposed as one canonical MCP resource",
            "mimeType": "application/json"
        }),
        json!({
            "uri": "resource://yacli/activity",
            "name": "yacli Activity Log",
            "description": "Catalog of recent successful write-actions with replay commands",
            "mimeType": "application/json"
        }),
    ];
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
            "uriTemplate": "resource://yacli/home/{account}",
            "name": "yacli Home Resource",
            "description": "Read one canonical yacli home summary for a configured account",
            "mimeType": "application/json"
        }),
        json!({
            "uriTemplate": "resource://yacli/home{?goal}",
            "name": "yacli Goal-aware Home Resource",
            "description": "Read one canonical yacli home summary prioritized for a natural-language goal",
            "mimeType": "application/json"
        }),
        json!({
            "uriTemplate": "resource://yacli/home/{account}{?goal}",
            "name": "yacli Goal-aware Account Home Resource",
            "description": "Read one canonical yacli home summary for a configured account and natural-language goal",
            "mimeType": "application/json"
        }),
        json!({
            "uriTemplate": "resource://yacli/onboarding{?goal}",
            "name": "yacli Goal-aware Onboarding Resource",
            "description": "Read onboarding checklist focused on a natural-language goal",
            "mimeType": "application/json"
        }),
        json!({
            "uriTemplate": "resource://yacli/doctor{?goal}",
            "name": "yacli Goal-aware Doctor Resource",
            "description": "Read health-check focused on a natural-language goal",
            "mimeType": "application/json"
        }),
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
        json!({
            "uriTemplate": "resource://yacli/skill/{skill}",
            "name": "yacli Embedded Skill Resource",
            "description": "Read one embedded yacli SKILL.md workflow as Markdown",
            "mimeType": "text/markdown"
        }),
        json!({
            "uriTemplate": "resource://yacli/workflow/{workflow}",
            "name": "yacli Workflow Detail Resource",
            "description": "Read one canonical yacli workflow with CLI steps and MCP links",
            "mimeType": "application/json"
        }),
        json!({
            "uriTemplate": "resource://yacli/activity/{activity}",
            "name": "yacli Activity Detail Resource",
            "description": "Read one successful write-action with its replay command",
            "mimeType": "application/json"
        }),
        json!({
            "uriTemplate": "resource://yacli/next-actions{?goal}",
            "name": "yacli Goal-aware Next Actions Resource",
            "description": "Read ranked next steps prioritized for a natural-language goal",
            "mimeType": "application/json"
        }),
        json!({
            "uriTemplate": "resource://yacli/next-actions/{account}{?goal}",
            "name": "yacli Goal-aware Account Next Actions Resource",
            "description": "Read ranked next steps for a configured account and natural-language goal",
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

fn home_resource_contents(uri: &str) -> Result<Vec<Value>> {
    let (account_name, goal) = home_resource_request(uri)?;
    json_resource_contents(uri, home_payload(account_name.as_deref(), goal.as_deref())?)
}

fn next_actions_resource_contents(uri: &str) -> Result<Vec<Value>> {
    let (account_name, goal) = next_actions_resource_request(uri)?;
    json_resource_contents(
        uri,
        next_actions_payload(account_name.as_deref(), goal.as_deref())?,
    )
}

fn onboarding_resource_contents(uri: &str) -> Result<Vec<Value>> {
    let goal = onboarding_resource_request(uri)?;
    json_resource_contents(uri, onboarding_resource_payload(goal.as_deref())?)
}

fn doctor_resource_contents(uri: &str) -> Result<Vec<Value>> {
    let goal = doctor_resource_request(uri)?;
    json_resource_contents(uri, doctor_payload(None, goal.as_deref())?)
}

fn skills_catalog_resource() -> Value {
    let items = skills::skill_names()
        .into_iter()
        .map(|name| {
            json!({
                "name": name,
                "prompt": skills::skill_prompt_name(name),
                "uri": format!("resource://yacli/skill/{name}"),
                "description": skills::skill_description(name).unwrap_or_default(),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "count": items.len(),
        "items": items,
    })
}

fn skill_resource_contents(uri: &str) -> Result<Vec<Value>> {
    let skill_name = templated_account_name(uri, "skill")?;
    let content = skills::skill_content(&skill_name).ok_or_else(|| {
        YacliError::UnsupportedOperation(format!("unknown embedded skill resource: {uri}"))
    })?;
    Ok(vec![json!({
        "uri": uri,
        "mimeType": "text/markdown",
        "text": content,
    })])
}

fn workflow_resource_contents(uri: &str) -> Result<Vec<Value>> {
    let workflow_id = templated_account_name(uri, "workflow")?;
    let payload = workflows::workflow_resource_detail(&workflow_id).ok_or_else(|| {
        YacliError::UnsupportedOperation(format!("unknown workflow resource: {uri}"))
    })?;
    json_resource_contents(uri, payload)
}

fn activity_catalog_resource() -> Result<Value> {
    let store = ActivityStore::load()?;
    let items = store
        .entries()
        .iter()
        .take(50)
        .map(|entry| {
            json!({
                "id": entry.id,
                "occurred_at": entry.occurred_at,
                "source": entry.source,
                "operation": entry.operation,
                "account": entry.account,
                "summary": entry.summary,
                "replay_command": entry.replay_command,
                "uri": format!("resource://yacli/activity/{}", entry.id),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "count": items.len(),
        "items": items,
    }))
}

fn activity_resource_contents(uri: &str) -> Result<Vec<Value>> {
    let activity_id = templated_account_name(uri, "activity")?;
    let store = ActivityStore::load()?;
    let payload = store
        .find(&activity_id)
        .map(|entry| {
            json!({
                "id": entry.id,
                "occurred_at": entry.occurred_at,
                "source": entry.source,
                "operation": entry.operation,
                "account": entry.account,
                "summary": entry.summary,
                "replay_command": entry.replay_command,
            })
        })
        .ok_or_else(|| YacliError::Validation(format!("unknown activity entry: {activity_id}")))?;
    json_resource_contents(uri, payload)
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

fn activity_undo(id: &str) -> Result<Value> {
    let store = ActivityStore::load()?;
    let entry = store.find(id).cloned().ok_or_else(|| {
        YacliError::Validation(format!("activity undo: запись `{id}` не найдена"))
    })?;
    let applied = apply_activity_undo(&entry)?;
    record_activity_mcp(NewActivityEntry {
        source: "mcp".to_string(),
        operation: "activity.undo".to_string(),
        account: applied.account.clone(),
        summary: applied.summary.clone(),
        replay_command: applied.replay_command.clone(),
        undo: None,
    });
    Ok(json!({
        "entry": entry,
        "undo": applied,
    }))
}

fn mail_send(account: Option<&str>, request: MailSendToolRequest) -> Result<Value> {
    let (resolved_account, auth, context) = resolve_mail_private_context(account)?;
    let attachments = load_mail_attachments(
        &request
            .attachment_paths
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>(),
    )?;
    let send_request = crate::mail::MailSendRequest {
        to: vec![request.to],
        cc: request.cc,
        bcc: request.bcc,
        subject: request.subject,
        text: request.text,
        html: request.html,
        attachments,
        thread_headers: None,
    };

    if request.dry_run {
        let review = review_mail_submission(auth, send_request)?;
        Ok(json!({
            "account": resolved_account,
            "attachments": request.attachment_paths,
            "dry_run": true,
            "review": review,
            "smtp": {
                "host": context.smtp_host,
                "port": context.smtp_port
            }
        }))
    } else {
        let sent = send_mail_message(&context.smtp_host, context.smtp_port, auth, send_request)?;
        let mut replay = format!(
            "yacli mail send {} {} {} --dry-run",
            shell_quote(sent.to.first().map(String::as_str).unwrap_or("")),
            shell_quote(&sent.subject),
            shell_quote("<текст письма>")
        );
        record_activity_mcp(NewActivityEntry {
            source: "mcp".to_string(),
            operation: "mail.send".to_string(),
            account: resolved_account.clone(),
            summary: format!(
                "Отправлено письмо {}: {}",
                sent.to.first().cloned().unwrap_or_else(|| "-".to_string()),
                sent.subject
            ),
            replay_command: std::mem::take(&mut replay),
            undo: None,
        });
        Ok(json!({
            "account": resolved_account,
            "attachments": request.attachment_paths,
            "sent": sent,
        }))
    }
}

fn mail_send_link(account: Option<&str>, request: MailSendLinkToolRequest) -> Result<Value> {
    let (resolved_account, disk_base_url, access_token) = resolve_disk_private_context(account)?;
    let (_, auth, context) = resolve_mail_private_context(account)?;
    let send_link_request = MailSendLinkRequest {
        source: PathBuf::from(&request.source_path),
        disk_path: request.disk_path,
        overwrite: request.overwrite,
        to: vec![request.to],
        cc: request.cc,
        bcc: request.bcc,
        subject: request.subject,
        text: request.text,
        html: request.html,
    };

    if request.dry_run {
        let review = review_mail_send_link(auth, &send_link_request)?;
        Ok(json!({
            "account": resolved_account,
            "dry_run": true,
            "review": review,
            "disk": {
                "base_url": disk_base_url,
            },
            "smtp": {
                "host": context.smtp_host,
                "port": context.smtp_port
            }
        }))
    } else {
        let result = send_link_via_mail(
            &disk_base_url,
            &access_token,
            &context.smtp_host,
            context.smtp_port,
            auth,
            &send_link_request,
        )?;
        let mut replay = format!(
            "yacli mail send-link {} {} {} --source {} --path {} --dry-run",
            shell_quote(result.sent.to.first().map(String::as_str).unwrap_or("")),
            shell_quote(&result.sent.subject),
            shell_quote("<текст письма>"),
            shell_quote(&request.source_path),
            shell_quote(&result.resource.path)
        );
        if request.overwrite {
            replay.push_str(" --overwrite");
        }
        record_activity_mcp(NewActivityEntry {
            source: "mcp".to_string(),
            operation: "mail.send_link".to_string(),
            account: resolved_account.clone(),
            summary: format!(
                "Отправлена публичная ссылка на файл: {} -> {}",
                result.resource.path,
                result.resource.public_url.as_deref().unwrap_or("-")
            ),
            replay_command: replay,
            undo: None,
        });
        Ok(json!({
            "account": resolved_account,
            "result": result,
        }))
    }
}

fn mail_reply(
    account: Option<&str>,
    folder: &str,
    uid: u64,
    cc: Vec<String>,
    text: Option<String>,
    html: Option<String>,
) -> Result<Value> {
    let (resolved_account, auth, context) = resolve_mail_private_context(account)?;
    let reply = crate::mail::reply_to_mail_message(
        &context.imap_host,
        context.imap_port,
        &context.smtp_host,
        context.smtp_port,
        auth,
        folder,
        crate::mail::MailReplyRequest {
            uid,
            cc,
            text,
            html,
        },
    )?;
    Ok(json!({
        "account": resolved_account,
        "folder": folder,
        "reply": reply,
    }))
}

fn mail_forward(account: Option<&str>, request: MailForwardToolRequest) -> Result<Value> {
    let (resolved_account, auth, context) = resolve_mail_private_context(account)?;
    let forward = crate::mail::forward_mail_message(
        &context.imap_host,
        context.imap_port,
        &context.smtp_host,
        context.smtp_port,
        auth,
        &request.folder,
        crate::mail::MailForwardRequest {
            uid: request.uid,
            to: vec![request.to],
            cc: request.cc,
            bcc: request.bcc,
            text: request.text,
            html: request.html,
            max_source_bytes: request.max_source_bytes,
        },
    )?;
    Ok(json!({
        "account": resolved_account,
        "folder": request.folder,
        "forward": forward,
    }))
}

fn mail_attachment_export(
    account: Option<&str>,
    request: MailAttachmentExportToolRequest,
) -> Result<Value> {
    let (resolved_account, auth, context) = resolve_mail_private_context(account)?;
    let attachment = export_mail_attachment(
        &context.imap_host,
        context.imap_port,
        auth,
        &request.folder,
        MailAttachmentExportRequest {
            uid: request.uid,
            selector: request.selector,
            output: PathBuf::from(&request.output),
            force: request.force,
            max_bytes: request.max_bytes,
        },
    )?;
    Ok(json!({
        "account": resolved_account,
        "folder": request.folder,
        "attachment": attachment,
    }))
}

fn mail_invite_inspect(
    account: Option<&str>,
    request: MailInviteInspectToolRequest,
) -> Result<Value> {
    let (resolved_account, auth, context) = resolve_mail_private_context(account)?;
    let invite = inspect_mail_invite(
        &context.imap_host,
        context.imap_port,
        auth,
        &request.folder,
        MailInviteInspectRequest {
            uid: request.uid,
            selector: request.selector,
            max_bytes: request.max_bytes,
        },
    )?;
    Ok(json!({
        "account": resolved_account,
        "folder": request.folder,
        "invite": invite,
    }))
}

fn mail_invite_create_event(
    account: Option<&str>,
    request: MailInviteCreateEventToolRequest,
) -> Result<Value> {
    if request.event_index == 0 {
        return Err(YacliError::Validation(
            "yacli.mail.invite.create_event `event_index` must be a positive integer".to_string(),
        ));
    }
    let (resolved_account, auth, mail_context) = resolve_mail_private_context(account)?;
    let replay_selector = request.selector.clone();
    let inspected = inspect_mail_invite(
        &mail_context.imap_host,
        mail_context.imap_port,
        auth,
        &request.folder,
        MailInviteInspectRequest {
            uid: request.uid,
            selector: request.selector,
            max_bytes: request.max_bytes,
        },
    )?;
    let (create_request, selected_invite) = calendar_create_request_from_invites(
        &request.calendar,
        &inspected.invites,
        request.event_index,
        "yacli.mail.invite.create_event",
    )?;
    let (_, app_password, calendar_context) =
        resolve_calendar_private_context(Some(&resolved_account))?;
    let (calendar, event) = create_calendar_event(
        &calendar_context.caldav_base_url,
        &calendar_context.email,
        &app_password,
        create_request,
    )?;
    let mut replay = format!(
        "yacli mail invite create-event {} --folder {} --calendar {} --event-index {}",
        request.uid,
        shell_quote(&request.folder),
        shell_quote(&calendar.id),
        request.event_index
    );
    match &replay_selector {
        MailAttachmentSelector::Index(index) => replay.push_str(&format!(" --index {}", index)),
        MailAttachmentSelector::Filename(name) => {
            replay.push_str(&format!(" --name {}", shell_quote(name)));
        }
    }
    record_activity_mcp(NewActivityEntry {
        source: "mcp".to_string(),
        operation: "mail.invite.create_event".to_string(),
        account: resolved_account.clone(),
        summary: format!(
            "Создано событие из приглашения письма {}: {}",
            request.uid,
            event.summary.as_deref().unwrap_or("-")
        ),
        replay_command: replay,
        undo: None,
    });

    Ok(json!({
        "account": resolved_account,
        "folder": request.folder,
        "attachment": inspected,
        "selected_invite": selected_invite,
        "calendar": calendar,
        "event": event,
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

fn calendar_create(account: Option<&str>, request: CalendarCreateToolRequest) -> Result<Value> {
    let (resolved_account, app_password, context) = resolve_calendar_private_context(account)?;
    let dry_run = request.dry_run;
    let create_request = CalendarCreateRequest {
        calendar: request.calendar,
        summary: request.summary,
        start: request.start,
        end: request.end,
        description: request.description,
        location: request.location,
    };
    if dry_run {
        let (calendar, review) = review_calendar_event_creation(
            &context.caldav_base_url,
            &context.email,
            &app_password,
            create_request,
        )?;
        Ok(json!({
            "account": resolved_account,
            "calendar": calendar,
            "dry_run": true,
            "review": review,
        }))
    } else {
        let (calendar, event) = create_calendar_event(
            &context.caldav_base_url,
            &context.email,
            &app_password,
            create_request,
        )?;
        let mut replay = format!(
            "yacli calendar create {} {} {}",
            shell_quote(event.summary.as_deref().unwrap_or("")),
            shell_quote(event.start.as_deref().unwrap_or("")),
            shell_quote(event.end.as_deref().unwrap_or(""))
        );
        if calendar.id != "default" {
            replay.push_str(&format!(" --calendar {}", shell_quote(&calendar.id)));
        }
        if let Some(location) = event.location.as_deref() {
            replay.push_str(&format!(" --location {}", shell_quote(location)));
        }
        if let Some(description) = event.description.as_deref() {
            replay.push_str(&format!(" --description {}", shell_quote(description)));
        }
        replay.push_str(" --dry-run");
        record_activity_mcp(NewActivityEntry {
            source: "mcp".to_string(),
            operation: "calendar.create".to_string(),
            account: resolved_account.clone(),
            summary: format!(
                "Создано событие в календаре {}: {}",
                calendar.name,
                event.summary.as_deref().unwrap_or("-")
            ),
            replay_command: replay,
            undo: calendar_create_undo(&calendar, &event),
        });
        Ok(json!({
            "account": resolved_account,
            "calendar": calendar,
            "event": event,
        }))
    }
}

fn calendar_delete(account: Option<&str>, calendar: &str, uid: &str) -> Result<Value> {
    let (resolved_account, app_password, context) = resolve_calendar_private_context(account)?;
    let (calendar, deleted_event) = delete_calendar_event(
        &context.caldav_base_url,
        &context.email,
        &app_password,
        calendar,
        uid,
    )?;
    Ok(json!({
        "account": resolved_account,
        "calendar": calendar,
        "deleted_event": deleted_event,
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

fn disk_mkdir(account: Option<&str>, path: &str) -> Result<Value> {
    let (resolved_account, base_url, access_token) = resolve_disk_private_context(account)?;
    let resource = create_private_directory(
        &base_url,
        &access_token,
        &PrivateDiskMkdirRequest {
            path: path.to_string(),
        },
    )?;
    record_activity_mcp(NewActivityEntry {
        source: "mcp".to_string(),
        operation: "disk.mkdir".to_string(),
        account: resolved_account.clone(),
        summary: format!("Создана папка на Диске: {}", resource.path),
        replay_command: format!("yacli disk mkdir {}", shell_quote(&resource.path)),
        undo: None,
    });
    Ok(json!({
        "account": resolved_account,
        "path": path,
        "resource": resource,
    }))
}

fn disk_upload(account: Option<&str>, request: DiskUploadToolRequest) -> Result<Value> {
    let (resolved_account, base_url, access_token) = resolve_disk_private_context(account)?;
    let dry_run = request.dry_run;
    let upload_request = PrivateDiskUploadRequest {
        source: PathBuf::from(request.source),
        path: request.path,
        overwrite: request.overwrite,
    };
    if dry_run {
        let review = review_private_upload(&upload_request)?;
        Ok(json!({
            "account": resolved_account,
            "path": upload_request.path,
            "dry_run": true,
            "upload": review,
        }))
    } else {
        let (resource, upload) =
            upload_private_resource(&base_url, &access_token, &upload_request)?;
        let mut replay = format!(
            "yacli disk upload {} {}",
            shell_quote(&upload.source_path),
            shell_quote(&upload.remote_path)
        );
        if upload.overwrite {
            replay.push_str(" --overwrite");
        }
        replay.push_str(" --dry-run");
        record_activity_mcp(NewActivityEntry {
            source: "mcp".to_string(),
            operation: "disk.upload".to_string(),
            account: resolved_account.clone(),
            summary: format!("Загружен файл на Диск: {}", upload.remote_path),
            replay_command: replay,
            undo: None,
        });
        Ok(json!({
            "account": resolved_account,
            "path": upload_request.path,
            "resource": resource,
            "upload": upload,
        }))
    }
}

fn disk_upload_link(account: Option<&str>, request: DiskUploadLinkToolRequest) -> Result<Value> {
    let (resolved_account, base_url, access_token) = resolve_disk_private_context(account)?;
    let upload_link_request = DiskUploadLinkRequest {
        source: PathBuf::from(&request.source),
        disk_path: request.path.clone(),
        overwrite: request.overwrite,
    };
    if request.dry_run {
        let review = review_disk_upload_link(&upload_link_request)?;
        Ok(json!({
            "account": resolved_account,
            "path": request.path,
            "dry_run": true,
            "review": review,
        }))
    } else {
        let result = upload_link_to_disk(&base_url, &access_token, &upload_link_request)?;
        record_activity_mcp(NewActivityEntry {
            source: "mcp".to_string(),
            operation: "disk.upload_link".to_string(),
            account: resolved_account.clone(),
            summary: format!(
                "Загружен и опубликован ресурс на Диске: {}",
                result
                    .resource
                    .public_url
                    .as_deref()
                    .unwrap_or(&result.resource.path)
            ),
            replay_command: format!(
                "yacli disk upload-link --source {} --path {} --dry-run",
                shell_quote(&result.upload.source_path),
                shell_quote(&result.resource.path)
            ),
            undo: None,
        });
        Ok(json!({
            "account": resolved_account,
            "path": request.path,
            "result": result,
        }))
    }
}

fn disk_download(account: Option<&str>, request: DiskDownloadToolRequest) -> Result<Value> {
    let (resolved_account, base_url, access_token) = resolve_disk_private_context(account)?;
    let download_request = PrivateDiskDownloadRequest {
        path: request.path,
        output: PathBuf::from(request.output_path),
        force: request.force,
    };
    let (resource, download) =
        download_private_resource(&base_url, &access_token, &download_request)?;

    let mut replay = format!(
        "yacli disk download {} --output {}",
        shell_quote(&resource.path),
        shell_quote(&download.output_path)
    );
    if request.force {
        replay.push_str(" --force");
    }
    record_activity_mcp(NewActivityEntry {
        source: "mcp".to_string(),
        operation: "disk.download".to_string(),
        account: resolved_account.clone(),
        summary: mcp_transfer_activity_summary("Скачан файл с Диска", &resource.path, &download),
        replay_command: replay,
        undo: None,
    });

    Ok(json!({
        "account": resolved_account,
        "path": download_request.path,
        "resource": resource,
        "download": download,
    }))
}

fn disk_publish(account: Option<&str>, request: DiskPublishToolRequest) -> Result<Value> {
    let (resolved_account, base_url, access_token) = resolve_disk_private_context(account)?;
    let publish_request = PrivateDiskPublishRequest {
        path: request.path.clone(),
    };
    if request.dry_run {
        let review = review_private_publish(&base_url, &access_token, &publish_request)?;
        Ok(json!({
            "account": resolved_account,
            "path": request.path,
            "dry_run": true,
            "review": review,
        }))
    } else {
        let resource = publish_private_resource(&base_url, &access_token, &publish_request)?;
        record_activity_mcp(NewActivityEntry {
            source: "mcp".to_string(),
            operation: "disk.publish".to_string(),
            account: resolved_account.clone(),
            summary: format!(
                "Опубликован ресурс на Диске: {}",
                resource.public_url.as_deref().unwrap_or(&resource.path)
            ),
            replay_command: format!(
                "yacli disk publish {} --dry-run",
                shell_quote(&resource.path)
            ),
            undo: Some(disk_publish_undo(&resource)),
        });
        Ok(json!({
            "account": resolved_account,
            "path": request.path,
            "resource": resource,
        }))
    }
}

fn disk_unpublish(account: Option<&str>, request: DiskUnpublishToolRequest) -> Result<Value> {
    let (resolved_account, base_url, access_token) = resolve_disk_private_context(account)?;
    let unpublish_request = PrivateDiskUnpublishRequest {
        path: request.path.clone(),
    };
    if request.dry_run {
        let review = review_private_unpublish(&base_url, &access_token, &unpublish_request)?;
        Ok(json!({
            "account": resolved_account,
            "path": request.path,
            "dry_run": true,
            "review": review,
        }))
    } else {
        let result = unpublish_private_resource(&base_url, &access_token, &unpublish_request)?;
        record_activity_mcp(NewActivityEntry {
            source: "mcp".to_string(),
            operation: "disk.unpublish".to_string(),
            account: resolved_account.clone(),
            summary: format!(
                "Отозвана публичная ссылка на Диске: {}",
                result
                    .revoked_public_url
                    .as_deref()
                    .unwrap_or(&result.resource.path)
            ),
            replay_command: format!(
                "yacli disk unpublish {} --dry-run",
                shell_quote(&result.resource.path)
            ),
            undo: None,
        });
        Ok(json!({
            "account": resolved_account,
            "path": request.path,
            "result": result,
        }))
    }
}

fn record_activity_mcp(entry: NewActivityEntry) {
    if let Err(err) = record_activity(entry) {
        eprintln!(
            "{}",
            json!({
                "ok": false,
                "warning": "activity_log_unavailable",
                "message": format!("failed to record activity entry: {err}"),
            })
        );
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn mcp_transfer_activity_summary(prefix: &str, target: &str, download: &DownloadedFile) -> String {
    let mut details = vec![
        format!("попыток: {}", download.attempts),
        format!("время: {} ms", download.elapsed_ms),
    ];
    if download.resumed_from_bytes > 0 {
        details.push(format!("resume: {} B", download.resumed_from_bytes));
    }
    format!("{prefix}: {target} ({})", details.join(", "))
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

fn optional_string_owned(value: &Value, key: &str) -> Option<String> {
    optional_string(value, key).map(ToString::to_string)
}

fn mail_attachment_selector(value: &Value) -> Result<MailAttachmentSelector> {
    mail_attachment_selector_with_context(value, "yacli.mail.attachment.export")
}

fn mail_attachment_selector_with_context(
    value: &Value,
    tool_name: &str,
) -> Result<MailAttachmentSelector> {
    let index = optional_usize(value, "index");
    let name = optional_string(value, "name");

    match (index, name) {
        (Some(index), None) => Ok(MailAttachmentSelector::Index(index)),
        (None, Some(name)) => Ok(MailAttachmentSelector::Filename(name.to_string())),
        (Some(_), Some(_)) => Err(YacliError::Validation(format!(
            "{tool_name} accepts either `index` or `name`, not both"
        ))),
        (None, None) => Err(YacliError::Validation(format!(
            "{tool_name} requires `index` or `name`"
        ))),
    }
}

fn string_list(value: &Value, key: &str) -> Result<Vec<String>> {
    let Some(values) = value.get(key) else {
        return Ok(Vec::new());
    };

    let array = values
        .as_array()
        .ok_or_else(|| YacliError::Validation(format!("`{key}` must be an array of strings")))?;

    array
        .iter()
        .map(|item| {
            item.as_str().map(ToString::to_string).ok_or_else(|| {
                YacliError::Validation(format!("`{key}` must be an array of strings"))
            })
        })
        .collect()
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

fn optional_bool(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
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
        "preferredSkill": bootstrap_state.preferred_skill,
        "preferredPrompt": bootstrap_state.preferred_prompt,
        "preferredWorkflow": bootstrap_state.preferred_workflow,
        "preferredActivity": bootstrap_state.preferred_activity,
        "preferredGoal": bootstrap_state.preferred_goal,
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
        home_uri if is_home_resource_uri(home_uri) => home_resource_contents(home_uri),
        onboarding_uri if is_onboarding_resource_uri(onboarding_uri) => {
            onboarding_resource_contents(onboarding_uri)
        }
        doctor_uri if is_doctor_resource_uri(doctor_uri) => doctor_resource_contents(doctor_uri),
        next_actions_uri if is_next_actions_resource_uri(next_actions_uri) => {
            next_actions_resource_contents(next_actions_uri)
        }
        "resource://yacli/skills" => json_resource_contents(uri, skills_catalog_resource()),
        "resource://yacli/workflows" => {
            json_resource_contents(uri, workflows::workflow_resource_catalog())
        }
        "resource://yacli/activity" => json_resource_contents(uri, activity_catalog_resource()?),
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
        skill_uri if skill_uri.starts_with("resource://yacli/skill/") => {
            skill_resource_contents(skill_uri)
        }
        workflow_uri if workflow_uri.starts_with("resource://yacli/workflow/") => {
            workflow_resource_contents(workflow_uri)
        }
        activity_uri if activity_uri.starts_with("resource://yacli/activity/") => {
            activity_resource_contents(activity_uri)
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

fn is_home_resource_uri(uri: &str) -> bool {
    resource_request(uri, "home").is_ok()
}

fn is_next_actions_resource_uri(uri: &str) -> bool {
    resource_request(uri, "next-actions").is_ok()
}

fn is_onboarding_resource_uri(uri: &str) -> bool {
    goal_query_resource_request(uri, "onboarding").is_ok()
}

fn is_doctor_resource_uri(uri: &str) -> bool {
    goal_query_resource_request(uri, "doctor").is_ok()
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

fn home_resource_request(uri: &str) -> Result<(Option<String>, Option<String>)> {
    resource_request(uri, "home")
}

fn next_actions_resource_request(uri: &str) -> Result<(Option<String>, Option<String>)> {
    resource_request(uri, "next-actions")
}

fn onboarding_resource_request(uri: &str) -> Result<Option<String>> {
    goal_query_resource_request(uri, "onboarding")
}

fn doctor_resource_request(uri: &str) -> Result<Option<String>> {
    goal_query_resource_request(uri, "doctor")
}

fn resource_request(uri: &str, namespace: &str) -> Result<(Option<String>, Option<String>)> {
    let parsed = Url::parse(uri)
        .map_err(|err| YacliError::Validation(format!("invalid resource URI `{uri}`: {err}")))?;
    let segments = parsed
        .path_segments()
        .ok_or_else(|| YacliError::Validation(format!("invalid resource URI `{uri}`")))?;
    let parts = segments.collect::<Vec<_>>();
    if parsed.scheme() != "resource" || parsed.host_str() != Some("yacli") {
        return Err(YacliError::UnsupportedOperation(format!(
            "unknown MCP resource: {uri}"
        )));
    }
    let goal = parsed
        .query_pairs()
        .find(|(key, _)| key == "goal")
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty());

    match parts.as_slice() {
        [only] if *only == namespace => Ok((None, goal)),
        [first, account] if *first == namespace && !account.trim().is_empty() => {
            Ok((Some(account.to_string()), goal))
        }
        _ => Err(YacliError::UnsupportedOperation(format!(
            "unknown MCP resource: {uri}"
        ))),
    }
}

fn goal_query_resource_request(uri: &str, namespace: &str) -> Result<Option<String>> {
    let parsed = Url::parse(uri)
        .map_err(|err| YacliError::Validation(format!("invalid resource URI `{uri}`: {err}")))?;
    let segments = parsed
        .path_segments()
        .ok_or_else(|| YacliError::Validation(format!("invalid resource URI `{uri}`")))?;
    let parts = segments.collect::<Vec<_>>();
    if parsed.scheme() != "resource"
        || parsed.host_str() != Some("yacli")
        || parts.as_slice() != [namespace]
    {
        return Err(YacliError::UnsupportedOperation(format!(
            "unknown MCP resource: {uri}"
        )));
    }

    Ok(parsed
        .query_pairs()
        .find(|(key, _)| key == "goal")
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| !value.is_empty()))
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
    let mut skill = None;
    let mut prompt = None;
    let mut workflow = None;
    let mut activity = None;
    let mut goal = None;
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
                    DASHBOARD_SECTION_HOME
                        | DASHBOARD_SECTION_TOOLS
                        | DASHBOARD_SECTION_WORKFLOWS
                        | DASHBOARD_SECTION_GOAL
                        | DASHBOARD_SECTION_PROMPTS
                        | DASHBOARD_SECTION_RESOURCES
                        | DASHBOARD_SECTION_AUTH
                        | DASHBOARD_SECTION_ACTIVITY
                ) {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_SECTION_QUERY_PARAM}` must be one of: {DASHBOARD_SECTION_HOME}, {DASHBOARD_SECTION_TOOLS}, {DASHBOARD_SECTION_WORKFLOWS}, {DASHBOARD_SECTION_GOAL}, {DASHBOARD_SECTION_PROMPTS}, {DASHBOARD_SECTION_RESOURCES}, {DASHBOARD_SECTION_AUTH}, {DASHBOARD_SECTION_ACTIVITY}"
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
                    DASHBOARD_RESOURCE_ACCOUNT
                        | DASHBOARD_RESOURCE_AUTH
                        | DASHBOARD_RESOURCE_SKILLS
                        | DASHBOARD_RESOURCE_SKILL
                ) {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_RESOURCE_QUERY_PARAM}` must be one of: {DASHBOARD_RESOURCE_ACCOUNT}, {DASHBOARD_RESOURCE_AUTH}, {DASHBOARD_RESOURCE_SKILLS}, {DASHBOARD_RESOURCE_SKILL}"
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
            APP_SKILL_QUERY_PARAM => {
                if value.trim().is_empty() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_SKILL_QUERY_PARAM}` cannot be empty"
                    )));
                }
                if skill.replace(value.into_owned()).is_some() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_SKILL_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            APP_PROMPT_QUERY_PARAM => {
                let value = value.into_owned();
                if !prompts::prompt_names().contains(&value.as_str()) {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_PROMPT_QUERY_PARAM}` must be one of the embedded MCP prompts"
                    )));
                }
                if prompt.replace(value).is_some() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_PROMPT_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            APP_WORKFLOW_QUERY_PARAM => {
                let value = value.into_owned();
                if workflows::workflow_definition(&value).is_none() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_WORKFLOW_QUERY_PARAM}` must be one of the canonical yacli workflows"
                    )));
                }
                if workflow.replace(value).is_some() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_WORKFLOW_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            APP_ACTIVITY_QUERY_PARAM => {
                if value.trim().is_empty() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_ACTIVITY_QUERY_PARAM}` cannot be empty"
                    )));
                }
                if activity.replace(value.into_owned()).is_some() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_ACTIVITY_QUERY_PARAM}` cannot appear more than once"
                    )));
                }
            }
            APP_GOAL_QUERY_PARAM => {
                if value.trim().is_empty() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_GOAL_QUERY_PARAM}` cannot be empty"
                    )));
                }
                if goal.replace(value.into_owned()).is_some() {
                    return Err(YacliError::Validation(format!(
                        "dashboard resource query `{APP_GOAL_QUERY_PARAM}` cannot appear more than once"
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

    match resource.as_deref() {
        Some(DASHBOARD_RESOURCE_SKILL) if skill.is_none() => {
            return Err(YacliError::Validation(format!(
                "dashboard resource query `{APP_SKILL_QUERY_PARAM}` is required when `{APP_RESOURCE_QUERY_PARAM}=skill`"
            )));
        }
        Some(DASHBOARD_RESOURCE_ACCOUNT | DASHBOARD_RESOURCE_AUTH | DASHBOARD_RESOURCE_SKILLS)
            if skill.is_some() =>
        {
            return Err(YacliError::Validation(format!(
                "dashboard resource query `{APP_SKILL_QUERY_PARAM}` is only valid when `{APP_RESOURCE_QUERY_PARAM}=skill`"
            )));
        }
        None if skill.is_some() => {
            return Err(YacliError::Validation(format!(
                "dashboard resource query `{APP_SKILL_QUERY_PARAM}` requires `{APP_RESOURCE_QUERY_PARAM}=skill`"
            )));
        }
        _ => {}
    }

    if section.is_none() {
        if resource.is_some() {
            section = Some(DASHBOARD_SECTION_RESOURCES.to_string());
        } else if tool.is_some() {
            section = Some(DASHBOARD_SECTION_TOOLS.to_string());
        } else if workflow.is_some() {
            section = Some(DASHBOARD_SECTION_WORKFLOWS.to_string());
        } else if goal.is_some() {
            section = Some(DASHBOARD_SECTION_GOAL.to_string());
        } else if prompt.is_some() {
            section = Some(DASHBOARD_SECTION_PROMPTS.to_string());
        } else if activity.is_some() {
            section = Some(DASHBOARD_SECTION_ACTIVITY.to_string());
        }
    }

    if prompt.is_some() && section.as_deref() != Some(DASHBOARD_SECTION_PROMPTS) {
        return Err(YacliError::Validation(format!(
            "dashboard resource query `{APP_PROMPT_QUERY_PARAM}` requires `{APP_SECTION_QUERY_PARAM}=prompts`"
        )));
    }

    if workflow.is_some() && section.as_deref() != Some(DASHBOARD_SECTION_WORKFLOWS) {
        return Err(YacliError::Validation(format!(
            "dashboard resource query `{APP_WORKFLOW_QUERY_PARAM}` requires `{APP_SECTION_QUERY_PARAM}=workflows`"
        )));
    }

    if goal.is_some()
        && section.as_deref() != Some(DASHBOARD_SECTION_GOAL)
        && section.as_deref() != Some(DASHBOARD_SECTION_HOME)
    {
        return Err(YacliError::Validation(format!(
            "dashboard resource query `{APP_GOAL_QUERY_PARAM}` requires `{APP_SECTION_QUERY_PARAM}=goal` or `{APP_SECTION_QUERY_PARAM}=home`"
        )));
    }

    if activity.is_some() && section.as_deref() != Some(DASHBOARD_SECTION_ACTIVITY) {
        return Err(YacliError::Validation(format!(
            "dashboard resource query `{APP_ACTIVITY_QUERY_PARAM}` requires `{APP_SECTION_QUERY_PARAM}=activity`"
        )));
    }

    Ok(DashboardResourceState {
        default_account: account,
        preferred_section: section,
        preferred_resource: resource,
        preferred_tool: tool,
        preferred_skill: skill,
        preferred_prompt: prompt,
        preferred_workflow: workflow,
        preferred_activity: activity,
        preferred_goal: goal,
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
        "yacli.roots.list"
        | "yacli.account.list"
        | "yacli.account.current"
        | "yacli.auth.status"
        | DASHBOARD_TOOL_GOAL_ROUTE
        | DASHBOARD_TOOL_DOCTOR_APPLY_SAFE
        | DASHBOARD_TOOL_UPDATE_CHECK => Some(MODEL_AND_APP_VISIBILITY),
        _ => None,
    }
}

fn jsonrpc_error_payload(id: Value, err: &YacliError) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": error_code(err),
            "message": err.to_string(),
            "data": err.as_json(),
        }
    })
}

fn execute_stdio_request(
    method: &str,
    params: Value,
    session: &mut SessionState,
    writer: &mut dyn Write,
    rx: &mpsc::Receiver<InputEvent>,
    poller: &mut ResourceSubscriptionPoller,
) -> Result<Value> {
    if method == "tools/call" && requested_tool_name(&params) == Some("yacli.roots.list") {
        return roots_list_tool_stdio(session, writer, rx, poller);
    }

    handle_request(method, params, session, Some(poller))
}

fn roots_list_tool_stdio(
    session: &mut SessionState,
    writer: &mut dyn Write,
    rx: &mpsc::Receiver<InputEvent>,
    poller: &mut ResourceSubscriptionPoller,
) -> Result<Value> {
    if !session.supports_roots_requests {
        return Err(YacliError::UnsupportedOperation(
            "yacli.roots.list requires stdio transport with client roots capability".to_string(),
        ));
    }

    if let Some(cached_roots) = session.cached_roots.clone()
        && !session.roots_dirty
    {
        return roots_tool_response(cached_roots, session.ui_enabled);
    }

    let request_id = session.next_outbound_request_id;
    session.next_outbound_request_id += 1;
    let request = json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "roots/list"
    });
    write_message(writer, &request, session.stdio_message_format)?;

    loop {
        match rx.recv_timeout(RESOURCE_POLL_INTERVAL) {
            Ok(InputEvent::Message(message, format)) => {
                session.stdio_message_format = format;

                if message.get("id") == Some(&json!(request_id)) && message.get("method").is_none()
                {
                    if let Some(error) = message.get("error") {
                        let code = error.get("code").and_then(Value::as_i64).unwrap_or(-32000);
                        let message = error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("roots/list failed");
                        let err = if code == -32601 {
                            YacliError::UnsupportedOperation(message.to_string())
                        } else {
                            YacliError::Validation(message.to_string())
                        };
                        return Err(err);
                    }

                    let roots = parse_roots_list_response(&message)?;
                    session.cached_roots = Some(roots.clone());
                    session.roots_dirty = false;
                    return roots_tool_response(roots, session.ui_enabled);
                }

                if message.get("method").is_none() {
                    continue;
                }

                let nested_method =
                    message
                        .get("method")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            YacliError::Serialization("missing JSON-RPC method".to_string())
                        })?;
                let nested_params = message.get("params").cloned().unwrap_or(Value::Null);

                if message.get("id").is_none() {
                    handle_notification(nested_method, session);
                    continue;
                }

                let nested_id = message
                    .get("id")
                    .cloned()
                    .ok_or_else(|| YacliError::Serialization("missing JSON-RPC id".to_string()))?;
                let response = match execute_stdio_request(
                    nested_method,
                    nested_params,
                    session,
                    writer,
                    rx,
                    poller,
                ) {
                    Ok(result) => json!({
                        "jsonrpc": "2.0",
                        "id": nested_id,
                        "result": result,
                    }),
                    Err(err) => json!({
                        "jsonrpc": "2.0",
                        "id": nested_id,
                        "error": {
                            "code": error_code(&err),
                            "message": err.to_string(),
                            "data": err.as_json(),
                        }
                    }),
                };
                write_message(writer, &response, session.stdio_message_format)?;
            }
            Ok(InputEvent::Eof) => {
                return Err(YacliError::Io(
                    "stdio stream closed while waiting for roots/list response".to_string(),
                ));
            }
            Ok(InputEvent::Error(err)) => return Err(err),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(YacliError::Io(
                    "stdio reader disconnected while waiting for roots/list response".to_string(),
                ));
            }
        }

        for notification in poller.collect_notifications(&session.resource_subscriptions)? {
            write_message(writer, &notification, session.stdio_message_format)?;
        }
    }
}

fn roots_tool_response(roots: Vec<Value>, ui_enabled: bool) -> Result<Value> {
    let structured = json!({
        "roots": roots,
    });
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
    if ui_enabled && let Some(visibility) = tool_visibility("yacli.roots.list") {
        response.insert("_meta".to_string(), app_meta(visibility));
    }
    Ok(Value::Object(response))
}

fn parse_roots_list_response(message: &Value) -> Result<Vec<Value>> {
    let roots = message
        .get("result")
        .and_then(|result| result.get("roots"))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            YacliError::Serialization("roots/list response missing `result.roots`".to_string())
        })?;

    roots.iter().map(validate_root).collect::<Result<Vec<_>>>()
}

fn validate_root(root: &Value) -> Result<Value> {
    let uri = root
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| YacliError::Validation("roots/list item requires `uri`".to_string()))?;
    if !uri.starts_with("file://") {
        return Err(YacliError::Validation(format!(
            "roots/list item `uri` must be a file:// URI, got `{uri}`"
        )));
    }

    let mut normalized = Map::new();
    normalized.insert("uri".to_string(), Value::String(uri.to_string()));
    if let Some(name) = root.get("name").and_then(Value::as_str) {
        normalized.insert("name".to_string(), Value::String(name.to_string()));
    }
    Ok(Value::Object(normalized))
}

fn requested_tool_name(params: &Value) -> Option<&str> {
    params.get("name").and_then(Value::as_str)
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

fn client_supports_roots(params: &Value) -> bool {
    params
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("roots"))
        .is_some()
}

fn client_supports_roots_list_changed(params: &Value) -> bool {
    params
        .get("capabilities")
        .and_then(|capabilities| capabilities.get("roots"))
        .and_then(|roots| roots.get("listChanged"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
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

fn is_client_response_message(message: &Value) -> bool {
    message.get("method").is_none()
        && message.get("id").is_some()
        && (message.get("result").is_some() || message.get("error").is_some())
}

fn request_accepts_sse(headers: &HeaderMap) -> bool {
    header_value(headers, "Accept")
        .map(|accept| accept.contains("text/event-stream"))
        .unwrap_or(false)
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

fn read_message(reader: &mut dyn BufRead) -> Result<Option<(Value, StdioMessageFormat)>> {
    let mut content_length = None::<usize>;
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            return Ok(None);
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }

        if content_length.is_none() && matches!(trimmed.as_bytes().first(), Some(b'{' | b'[')) {
            let payload = serde_json::from_str::<Value>(trimmed).map_err(|err| {
                YacliError::Serialization(format!("invalid JSON-RPC payload: {err}"))
            })?;
            return Ok(Some((payload, StdioMessageFormat::JsonLine)));
        }

        if let Some(value) = trimmed
            .split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("Content-Length"))
            .map(|(_, value)| value)
        {
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
        .map(|value| Some((value, StdioMessageFormat::ContentLength)))
        .map_err(|err| YacliError::Serialization(format!("invalid JSON-RPC payload: {err}")))
}

fn write_message(
    writer: &mut dyn Write,
    payload: &Value,
    format: StdioMessageFormat,
) -> Result<()> {
    let encoded =
        serde_json::to_vec(payload).map_err(|err| YacliError::Serialization(err.to_string()))?;
    match format {
        StdioMessageFormat::ContentLength => {
            write!(writer, "Content-Length: {}\r\n\r\n", encoded.len())?;
            writer.write_all(&encoded)?;
        }
        StdioMessageFormat::JsonLine => {
            writer.write_all(&encoded)?;
            writer.write_all(b"\n")?;
        }
    }
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
