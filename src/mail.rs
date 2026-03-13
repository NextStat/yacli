use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::Utc;
use mailparse::{DispositionType, MailAddr, MailHeaderMap, addrparse};
use mime_guess::MimeGuess;
use native_tls::TlsConnector;
use rand::random;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::calendar::{CalendarInvite, parse_calendar_invites};
use crate::error::{Result, YacliError};

const IMAP_TIMEOUT_SECS: u64 = 20;
const SMTP_TIMEOUT_SECS: u64 = 20;

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct MailFolder {
    pub name: String,
    pub raw_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct MailMessageSummary {
    pub uid: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct MailAttachmentSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
    pub inline: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MailAttachmentPart {
    filename: Option<String>,
    mime_type: String,
    content_id: Option<String>,
    inline: bool,
    content: Vec<u8>,
}

impl MailAttachmentPart {
    fn summary(&self) -> MailAttachmentSummary {
        MailAttachmentSummary {
            filename: self.filename.clone(),
            mime_type: self.mime_type.clone(),
            content_id: self.content_id.clone(),
            inline: self.inline,
        }
    }

    fn payload(&self) -> MailAttachmentPayload {
        MailAttachmentPayload {
            filename: self.filename.clone(),
            mime_type: self.mime_type.clone(),
            content_id: self.content_id.clone(),
            inline: self.inline,
            content: self.content.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct MailMessage {
    pub uid: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_body: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MailAttachmentSummary>,
    #[serde(skip_serializing)]
    raw_attachments: Vec<MailAttachmentPart>,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct SentMail {
    pub from: String,
    pub to: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cc: Vec<String>,
    pub bcc_count: usize,
    pub subject: String,
    pub message_id: String,
    pub body_kind: String,
}

#[derive(Clone, Debug)]
pub struct MailThreadHeaders {
    pub in_reply_to: String,
    pub references: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct MailSendRequest {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub attachments: Vec<MailAttachmentPayload>,
    pub thread_headers: Option<MailThreadHeaders>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MailAttachmentSelector {
    Index(usize),
    Filename(String),
}

#[derive(Clone, Debug)]
pub struct MailAttachmentExportRequest {
    pub uid: u64,
    pub selector: MailAttachmentSelector,
    pub output: PathBuf,
    pub force: bool,
    pub max_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct MailInviteInspectRequest {
    pub uid: u64,
    pub selector: MailAttachmentSelector,
    pub max_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MailAttachmentPayload {
    pub filename: Option<String>,
    pub mime_type: String,
    pub content_id: Option<String>,
    pub inline: bool,
    pub content: Vec<u8>,
}

#[derive(Clone, Debug)]
pub enum MailSessionAuth {
    OauthXoauth2 {
        account: String,
        access_token: String,
    },
    AppPassword {
        account: String,
        app_password: String,
    },
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct RepliedMail {
    pub original_uid: u64,
    pub recipient: String,
    pub original_subject: String,
    pub original_message_id: String,
    pub sent: SentMail,
}

#[derive(Clone, Debug)]
pub struct MailReplyRequest {
    pub uid: u64,
    pub cc: Vec<String>,
    pub text: Option<String>,
    pub html: Option<String>,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct ForwardedMail {
    pub original_uid: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MailAttachmentSummary>,
    pub sent: SentMail,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct ExportedMailAttachment {
    pub message_uid: u64,
    pub attachment_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
    pub inline: bool,
    pub output_path: String,
    pub bytes_written: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct InspectedMailInvite {
    pub message_uid: u64,
    pub attachment_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_id: Option<String>,
    pub inline: bool,
    pub invites: Vec<CalendarInvite>,
}

#[derive(Clone, Debug)]
pub struct MailForwardRequest {
    pub uid: u64,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub text: Option<String>,
    pub html: Option<String>,
    pub max_source_bytes: u64,
}

struct NormalizedOutgoingBody {
    text: Option<String>,
    html: Option<String>,
    body_kind: String,
}

struct NormalizedForwardRequest {
    uid: u64,
    to: Vec<String>,
    cc: Vec<String>,
    bcc: Vec<String>,
    intro_text: Option<String>,
    intro_html: Option<String>,
    max_source_bytes: u64,
}

struct ForwardBody {
    text: String,
    html: Option<String>,
}

struct OutgoingMessage<'a> {
    from: &'a str,
    to: &'a [String],
    cc: &'a [String],
    subject: &'a str,
    text: Option<&'a str>,
    html: Option<&'a str>,
    attachments: &'a [MailAttachmentPayload],
    message_id: &'a str,
    thread_headers: Option<&'a MailThreadHeaders>,
}

pub fn list_mail_folders(
    imap_host: &str,
    imap_port: u16,
    auth: MailSessionAuth,
) -> Result<Vec<MailFolder>> {
    let mut session = open_authenticated_session(imap_host, imap_port, auth)?;
    let folders = session.list_folders()?;
    let _ = session.logout();
    Ok(folders)
}

pub fn list_mail_messages(
    imap_host: &str,
    imap_port: u16,
    auth: MailSessionAuth,
    mailbox_name: &str,
    limit: usize,
) -> Result<Vec<MailMessageSummary>> {
    validate_mail_summary_limit("mail list", limit)?;

    let mut session = open_authenticated_session(imap_host, imap_port, auth)?;

    let encoded_mailbox = encode_modified_utf7(mailbox_name);
    session.select_mailbox(&encoded_mailbox)?;
    let uids = session.search_all_uids()?;
    if uids.is_empty() {
        let _ = session.logout();
        return Ok(Vec::new());
    }

    let selected_uids = uids.into_iter().rev().take(limit).collect::<Vec<_>>();
    let messages = session.fetch_message_summaries(&selected_uids)?;
    let _ = session.logout();
    Ok(messages)
}

pub fn search_mail_messages(
    imap_host: &str,
    imap_port: u16,
    auth: MailSessionAuth,
    mailbox_name: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<MailMessageSummary>> {
    validate_mail_summary_limit("mail search", limit)?;
    let query = normalize_search_query(query)?;

    let mut session = open_authenticated_session(imap_host, imap_port, auth)?;

    let encoded_mailbox = encode_modified_utf7(mailbox_name);
    session.select_mailbox(&encoded_mailbox)?;
    let uids = session.search_uids_by_text(&query)?;
    if uids.is_empty() {
        let _ = session.logout();
        return Ok(Vec::new());
    }

    let selected_uids = uids.into_iter().rev().take(limit).collect::<Vec<_>>();
    let messages = session.fetch_message_summaries(&selected_uids)?;
    let _ = session.logout();
    Ok(messages)
}

pub fn read_mail_message(
    imap_host: &str,
    imap_port: u16,
    auth: MailSessionAuth,
    mailbox_name: &str,
    uid: u64,
    max_bytes: u64,
) -> Result<MailMessage> {
    if uid == 0 {
        return Err(YacliError::Validation(
            "mail read <id> must be greater than zero".to_string(),
        ));
    }
    if max_bytes == 0 {
        return Err(YacliError::Validation(
            "mail read --max-bytes must be greater than zero".to_string(),
        ));
    }

    let mut session = open_authenticated_session(imap_host, imap_port, auth)?;
    let encoded_mailbox = encode_modified_utf7(mailbox_name);
    session.select_mailbox(&encoded_mailbox)?;

    let (metadata, headers) = session
        .fetch_message_headers(uid, &["SUBJECT", "FROM", "TO", "CC", "DATE", "MESSAGE-ID"])?;
    if let Some(size) = metadata.size
        && size > max_bytes
    {
        let _ = session.logout();
        return Err(YacliError::UnsupportedOperation(format!(
            "message id {} size {} exceeds --max-bytes {}; rerun `yacli mail read {} --max-bytes {}`",
            uid, size, max_bytes, uid, size
        )));
    }

    let raw_message = session.fetch_message_rfc822(uid)?;
    let message = build_read_message(metadata, &headers, &raw_message)?;
    let _ = session.logout();
    Ok(message)
}

pub fn send_mail_message(
    smtp_host: &str,
    smtp_port: u16,
    auth: MailSessionAuth,
    request: MailSendRequest,
) -> Result<SentMail> {
    let prepared = prepare_mail_submission(auth, request)?;
    let mut session = SmtpSession::connect(smtp_host, smtp_port)?;
    session.read_greeting(smtp_host)?;
    session.ehlo("yacli.nextstat.dev")?;
    match &prepared.auth {
        MailSmtpAuth::OauthXoauth2 {
            account,
            access_token,
        } => session.authenticate_xoauth2(account, access_token)?,
        MailSmtpAuth::AppPassword {
            account,
            app_password,
        } => session.authenticate_plain(account, app_password)?,
    }
    session.send_message(
        &prepared.envelope_from,
        &prepared.envelope_recipients,
        &prepared.message,
    )?;
    let _ = session.quit();
    Ok(prepared.sent)
}

pub fn load_mail_attachments(paths: &[PathBuf]) -> Result<Vec<MailAttachmentPayload>> {
    paths
        .iter()
        .map(|path| load_mail_attachment(path))
        .collect()
}

pub fn export_mail_attachment(
    imap_host: &str,
    imap_port: u16,
    auth: MailSessionAuth,
    mailbox_name: &str,
    request: MailAttachmentExportRequest,
) -> Result<ExportedMailAttachment> {
    validate_mail_attachment_export_request(&request)?;
    let uid = request.uid;
    let selector = request.selector.clone();
    let output = request.output.clone();
    let force = request.force;
    let max_bytes = request.max_bytes;

    let message = read_mail_message(imap_host, imap_port, auth, mailbox_name, uid, max_bytes)?;
    export_attachment_from_message(&message, &selector, &output, force)
}

pub fn inspect_mail_invite(
    imap_host: &str,
    imap_port: u16,
    auth: MailSessionAuth,
    mailbox_name: &str,
    request: MailInviteInspectRequest,
) -> Result<InspectedMailInvite> {
    validate_mail_attachment_read_request(
        request.uid,
        &request.selector,
        request.max_bytes,
        "mail invite inspect",
    )?;
    let message = read_mail_message(
        imap_host,
        imap_port,
        auth,
        mailbox_name,
        request.uid,
        request.max_bytes,
    )?;
    inspect_invite_from_message(&message, &request.selector)
}

pub fn reply_to_mail_message(
    imap_host: &str,
    imap_port: u16,
    smtp_host: &str,
    smtp_port: u16,
    auth: MailSessionAuth,
    mailbox_name: &str,
    request: MailReplyRequest,
) -> Result<RepliedMail> {
    if request.uid == 0 {
        return Err(YacliError::Validation(
            "mail reply <id> must be greater than zero".to_string(),
        ));
    }

    let normalized_body = normalize_outgoing_body(request.text, request.html, "mail reply")?;

    let target = fetch_reply_target(
        imap_host,
        imap_port,
        auth.clone(),
        mailbox_name,
        request.uid,
    )?;
    let sent = send_mail_message(
        smtp_host,
        smtp_port,
        auth,
        MailSendRequest {
            to: vec![target.recipient.clone()],
            cc: request.cc,
            bcc: Vec::new(),
            subject: normalize_reply_subject(&target.subject),
            text: normalized_body.text,
            html: normalized_body.html,
            attachments: Vec::new(),
            thread_headers: Some(MailThreadHeaders {
                in_reply_to: target.message_id.clone(),
                references: target.references.clone(),
            }),
        },
    )?;

    Ok(RepliedMail {
        original_uid: request.uid,
        recipient: target.recipient,
        original_subject: target.subject,
        original_message_id: target.message_id,
        sent,
    })
}

pub fn forward_mail_message(
    imap_host: &str,
    imap_port: u16,
    smtp_host: &str,
    smtp_port: u16,
    auth: MailSessionAuth,
    mailbox_name: &str,
    request: MailForwardRequest,
) -> Result<ForwardedMail> {
    if request.uid == 0 {
        return Err(YacliError::Validation(
            "mail forward <id> must be greater than zero".to_string(),
        ));
    }

    let normalized = normalize_forward_request(request)?;
    let source = read_mail_message(
        imap_host,
        imap_port,
        auth.clone(),
        mailbox_name,
        normalized.uid,
        normalized.max_source_bytes,
    )?;
    let body = build_forward_body(
        &source,
        normalized.intro_text.as_deref(),
        normalized.intro_html.as_deref(),
    );
    let sent = send_mail_message(
        smtp_host,
        smtp_port,
        auth,
        MailSendRequest {
            to: normalized.to,
            cc: normalized.cc,
            bcc: normalized.bcc,
            subject: normalize_forward_subject(source.subject.as_deref()),
            text: Some(body.text),
            html: body.html,
            attachments: source
                .raw_attachments
                .iter()
                .map(MailAttachmentPart::payload)
                .collect(),
            thread_headers: None,
        },
    )?;

    Ok(ForwardedMail {
        original_uid: source.uid,
        original_subject: source.subject,
        original_message_id: source.message_id,
        attachments: source.attachments,
        sent,
    })
}

fn open_authenticated_session(
    imap_host: &str,
    imap_port: u16,
    auth: MailSessionAuth,
) -> Result<ImapSession> {
    let mut session = ImapSession::connect(imap_host, imap_port)?;
    match auth {
        MailSessionAuth::OauthXoauth2 {
            account,
            access_token,
        } => session.authenticate_xoauth2(&account, &access_token)?,
        MailSessionAuth::AppPassword {
            account,
            app_password,
        } => session.login(&account, &app_password)?,
    }
    Ok(session)
}

fn fetch_reply_target(
    imap_host: &str,
    imap_port: u16,
    auth: MailSessionAuth,
    mailbox_name: &str,
    uid: u64,
) -> Result<MailReplyTarget> {
    let mut session = open_authenticated_session(imap_host, imap_port, auth)?;
    let encoded_mailbox = encode_modified_utf7(mailbox_name);
    session.select_mailbox(&encoded_mailbox)?;
    let (_, headers) = session.fetch_message_headers(
        uid,
        &["SUBJECT", "FROM", "REPLY-TO", "MESSAGE-ID", "REFERENCES"],
    )?;
    let _ = session.logout();

    build_reply_target(&headers)
}

#[derive(Debug)]
enum MailSmtpAuth {
    OauthXoauth2 {
        account: String,
        access_token: String,
    },
    AppPassword {
        account: String,
        app_password: String,
    },
}

#[derive(Debug)]
struct PreparedMailSubmission {
    auth: MailSmtpAuth,
    envelope_from: String,
    envelope_recipients: Vec<String>,
    message: String,
    sent: SentMail,
}

struct MailReplyTarget {
    recipient: String,
    subject: String,
    message_id: String,
    references: Vec<String>,
}

struct SmtpSession<S: Read + Write> {
    reader: BufReader<S>,
}

struct SmtpResponse {
    code: u16,
    lines: Vec<String>,
}

struct ImapSession {
    reader: BufReader<native_tls::TlsStream<TcpStream>>,
    next_tag: u32,
}

#[derive(Clone, Copy)]
enum TaggedErrorKind {
    Auth,
    Api,
}

impl SmtpSession<native_tls::TlsStream<TcpStream>> {
    fn connect(smtp_host: &str, smtp_port: u16) -> Result<Self> {
        let address = format!("{smtp_host}:{smtp_port}");
        let tcp = TcpStream::connect(&address).map_err(|err| {
            YacliError::Network(format!("failed to connect to SMTP server {address}: {err}"))
        })?;
        tcp.set_read_timeout(Some(Duration::from_secs(SMTP_TIMEOUT_SECS)))?;
        tcp.set_write_timeout(Some(Duration::from_secs(SMTP_TIMEOUT_SECS)))?;

        let connector = TlsConnector::new().map_err(|err| {
            YacliError::Network(format!("failed to initialize TLS connector: {err}"))
        })?;
        let tls = connector.connect(smtp_host, tcp).map_err(|err| {
            YacliError::Network(format!(
                "failed to establish TLS session with {smtp_host}: {err}"
            ))
        })?;

        Ok(Self::from_stream(tls))
    }
}

impl ImapSession {
    fn connect(imap_host: &str, imap_port: u16) -> Result<Self> {
        let address = format!("{imap_host}:{imap_port}");
        let tcp = TcpStream::connect(&address).map_err(|err| {
            YacliError::Network(format!("failed to connect to IMAP server {address}: {err}"))
        })?;
        tcp.set_read_timeout(Some(Duration::from_secs(IMAP_TIMEOUT_SECS)))?;
        tcp.set_write_timeout(Some(Duration::from_secs(IMAP_TIMEOUT_SECS)))?;

        let connector = TlsConnector::new().map_err(|err| {
            YacliError::Network(format!("failed to initialize TLS connector: {err}"))
        })?;
        let tls = connector.connect(imap_host, tcp).map_err(|err| {
            YacliError::Network(format!(
                "failed to establish TLS session with {imap_host}: {err}"
            ))
        })?;

        let mut session = Self {
            reader: BufReader::new(tls),
            next_tag: 1,
        };
        let greeting = session.read_line()?;
        if !greeting.starts_with("* OK") {
            return Err(YacliError::Api(format!(
                "unexpected IMAP greeting from {imap_host}: {}",
                greeting.trim()
            )));
        }

        Ok(session)
    }

    fn authenticate_xoauth2(&mut self, account: &str, access_token: &str) -> Result<()> {
        let payload = build_xoauth2_payload(account, access_token);
        let (_, tagged) = self.run_command(&format!(
            "AUTHENTICATE XOAUTH2 {}",
            STANDARD.encode(payload)
        ))?;
        parse_tagged_status(
            &self.current_tag(),
            &tagged,
            "IMAP XOAUTH2 authentication",
            TaggedErrorKind::Auth,
        )
    }

    fn login(&mut self, account: &str, app_password: &str) -> Result<()> {
        let quoted_account = quote_imap_string(account);
        let quoted_password = quote_imap_string(app_password);
        let (_, tagged) = self.run_command(&format!("LOGIN {quoted_account} {quoted_password}"))?;
        parse_tagged_status(
            &self.current_tag(),
            &tagged,
            "IMAP LOGIN authentication",
            TaggedErrorKind::Auth,
        )
    }

    fn list_folders(&mut self) -> Result<Vec<MailFolder>> {
        let (lines, tagged) = self.run_command(r#"LIST "" "*""#)?;
        parse_tagged_status(
            &self.current_tag(),
            &tagged,
            "IMAP LIST",
            TaggedErrorKind::Api,
        )?;

        let mut folders = Vec::new();
        for line in lines {
            if line.starts_with("* LIST ") {
                folders.push(parse_list_line(&line)?);
            }
        }

        Ok(folders)
    }

    fn select_mailbox(&mut self, mailbox_name: &str) -> Result<()> {
        let quoted_mailbox = quote_imap_string(mailbox_name);
        let (_, tagged) = self.run_command(&format!("SELECT {quoted_mailbox}"))?;
        parse_tagged_status(
            &self.current_tag(),
            &tagged,
            "IMAP SELECT",
            TaggedErrorKind::Api,
        )
    }

    fn search_all_uids(&mut self) -> Result<Vec<u64>> {
        let (lines, tagged) = self.run_command("UID SEARCH ALL")?;
        parse_tagged_status(
            &self.current_tag(),
            &tagged,
            "IMAP UID SEARCH",
            TaggedErrorKind::Api,
        )?;
        parse_search_uids(&lines)
    }

    fn search_uids_by_text(&mut self, query: &str) -> Result<Vec<u64>> {
        let tag = self.next_tag();
        let literal = query.as_bytes();
        self.write_line(&format!(
            "{tag} UID SEARCH CHARSET UTF-8 TEXT {{{}}}",
            literal.len()
        ))?;

        let continuation = self.read_line()?;
        if !continuation.starts_with('+') {
            return Err(YacliError::Api(format!(
                "IMAP UID SEARCH literal was not accepted: {continuation}"
            )));
        }

        self.write_bytes(literal)?;
        self.write_line("")?;

        let (lines, tagged) = self.read_until_tag(&tag)?;
        parse_tagged_status(&tag, &tagged, "IMAP UID SEARCH", TaggedErrorKind::Api)?;
        parse_search_uids(&lines)
    }

    fn fetch_message_summaries(&mut self, uids: &[u64]) -> Result<Vec<MailMessageSummary>> {
        let query = uids
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let tag = self.next_tag();
        self.write_line(&format!(
            "{tag} UID FETCH {query} (UID FLAGS RFC822.SIZE BODY.PEEK[HEADER.FIELDS (SUBJECT FROM DATE)])"
        ))?;

        let mut messages = Vec::new();
        loop {
            let line = self.read_line()?;
            if line.starts_with(&tag) {
                parse_tagged_status(&tag, &line, "IMAP UID FETCH", TaggedErrorKind::Api)?;
                messages.sort_by_key(|message: &MailMessageSummary| std::cmp::Reverse(message.uid));
                return Ok(messages);
            }

            if line.starts_with("* ") && line.contains(" FETCH (") {
                let metadata = parse_fetch_metadata(&line)?;
                let headers = if let Some(literal_len) = metadata.header_literal_len {
                    let mut literal = vec![0_u8; literal_len];
                    self.reader.read_exact(&mut literal)?;
                    let trailer = self.read_line()?;
                    ensure_fetch_literal_closed(&trailer)?;
                    literal
                } else {
                    Vec::new()
                };
                messages.push(build_message_summary(metadata, &headers)?);
            }
        }
    }

    fn fetch_message_headers(
        &mut self,
        uid: u64,
        header_fields: &[&str],
    ) -> Result<(FetchMetadata, Vec<u8>)> {
        let fields = header_fields.join(" ");
        let tag = self.next_tag();
        self.write_line(&format!(
            "{tag} UID FETCH {uid} (UID FLAGS RFC822.SIZE BODY.PEEK[HEADER.FIELDS ({fields})])"
        ))?;

        loop {
            let line = self.read_line()?;
            if line.starts_with(&tag) {
                parse_tagged_status(&tag, &line, "IMAP UID FETCH", TaggedErrorKind::Api)?;
                return Err(YacliError::Api(format!(
                    "message uid {} not found in folder",
                    uid
                )));
            }

            if line.starts_with("* ") && line.contains(" FETCH (") {
                let metadata = parse_fetch_metadata(&line)?;
                if metadata.uid != uid {
                    return Err(YacliError::Serialization(format!(
                        "IMAP FETCH returned uid {} while requesting uid {}",
                        metadata.uid, uid
                    )));
                }
                let headers = if let Some(literal_len) = metadata.header_literal_len {
                    let mut literal = vec![0_u8; literal_len];
                    self.reader.read_exact(&mut literal)?;
                    let trailer = self.read_line()?;
                    ensure_fetch_literal_closed(&trailer)?;
                    literal
                } else {
                    Vec::new()
                };
                let tagged = self.read_line()?;
                parse_tagged_status(&tag, &tagged, "IMAP UID FETCH", TaggedErrorKind::Api)?;
                return Ok((metadata, headers));
            }
        }
    }

    fn fetch_message_rfc822(&mut self, uid: u64) -> Result<Vec<u8>> {
        let tag = self.next_tag();
        self.write_line(&format!("{tag} UID FETCH {uid} (BODY.PEEK[])"))?;

        loop {
            let line = self.read_line()?;
            if line.starts_with(&tag) {
                parse_tagged_status(&tag, &line, "IMAP UID FETCH", TaggedErrorKind::Api)?;
                return Err(YacliError::Api(format!(
                    "message uid {} not found in folder",
                    uid
                )));
            }

            if line.starts_with("* ") && line.contains(" FETCH (") {
                let literal_len = extract_trailing_literal_len(&line)?.ok_or_else(|| {
                    YacliError::Serialization(format!(
                        "missing RFC822 literal in IMAP FETCH response: {line}"
                    ))
                })?;
                let mut literal = vec![0_u8; literal_len];
                self.reader.read_exact(&mut literal)?;
                let trailer = self.read_line()?;
                ensure_fetch_literal_closed(&trailer)?;
                let tagged = self.read_line()?;
                parse_tagged_status(&tag, &tagged, "IMAP UID FETCH", TaggedErrorKind::Api)?;
                return Ok(literal);
            }
        }
    }

    fn logout(&mut self) -> Result<()> {
        let tag = self.next_tag();
        self.write_line(&format!("{tag} LOGOUT"))?;
        let (_, tagged) = self.read_until_tag(&tag)?;
        parse_tagged_status(&tag, &tagged, "IMAP LOGOUT", TaggedErrorKind::Api)
    }

    fn run_command(&mut self, command: &str) -> Result<(Vec<String>, String)> {
        let tag = self.next_tag();
        self.write_line(&format!("{tag} {command}"))?;
        self.read_until_tag(&tag)
    }

    fn read_until_tag(&mut self, tag: &str) -> Result<(Vec<String>, String)> {
        let mut lines = Vec::new();
        loop {
            let line = self.read_line()?;
            if line.starts_with(tag) {
                return Ok((lines, line));
            }
            lines.push(line);
        }
    }

    fn write_line(&mut self, line: &str) -> Result<()> {
        let stream = self.reader.get_mut();
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\r\n")?;
        stream.flush()?;
        Ok(())
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        let stream = self.reader.get_mut();
        stream.write_all(bytes)?;
        stream.flush()?;
        Ok(())
    }

    fn read_line(&mut self) -> Result<String> {
        let mut line = String::new();
        let read = self.reader.read_line(&mut line)?;
        if read == 0 {
            return Err(YacliError::Network(
                "IMAP server closed the connection unexpectedly".to_string(),
            ));
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }

    fn next_tag(&mut self) -> String {
        let tag = format!("A{:04}", self.next_tag);
        self.next_tag += 1;
        tag
    }

    fn current_tag(&self) -> String {
        format!("A{:04}", self.next_tag.saturating_sub(1))
    }
}

impl<S: Read + Write> SmtpSession<S> {
    fn from_stream(stream: S) -> Self {
        Self {
            reader: BufReader::new(stream),
        }
    }

    fn read_greeting(&mut self, smtp_host: &str) -> Result<()> {
        let response = self.read_response()?;
        ensure_smtp_code(
            &response,
            &[220],
            &format!("SMTP greeting from {smtp_host}"),
        )
    }

    fn ehlo(&mut self, client_name: &str) -> Result<()> {
        self.write_line(&format!("EHLO {client_name}"))?;
        let response = self.read_response()?;
        ensure_smtp_code(&response, &[250], "SMTP EHLO")
    }

    fn authenticate_xoauth2(&mut self, account: &str, access_token: &str) -> Result<()> {
        let payload = build_xoauth2_payload(account, access_token);
        self.write_line(&format!("AUTH XOAUTH2 {}", STANDARD.encode(payload)))?;
        let response = self.read_response()?;
        self.finish_auth_response(response, "SMTP XOAUTH2 authentication")
    }

    fn authenticate_plain(&mut self, account: &str, app_password: &str) -> Result<()> {
        let payload = format!("\u{0}{account}\u{0}{app_password}");
        self.write_line(&format!("AUTH PLAIN {}", STANDARD.encode(payload)))?;
        let response = self.read_response()?;
        self.finish_auth_response(response, "SMTP PLAIN authentication")
    }

    fn finish_auth_response(&mut self, response: SmtpResponse, action: &str) -> Result<()> {
        match response.code {
            235 => Ok(()),
            334 => {
                self.write_line("")?;
                let final_response = self.read_response()?;
                let detail = decode_smtp_auth_challenge(&response)
                    .or_else(|| smtp_response_detail(&final_response))
                    .unwrap_or_else(|| flatten_smtp_response(&final_response));
                Err(YacliError::Auth(format!("{action} failed: {detail}")))
            }
            _ => Err(YacliError::Auth(format!(
                "{action} failed: {}",
                flatten_smtp_response(&response)
            ))),
        }
    }

    fn send_message(
        &mut self,
        envelope_from: &str,
        recipients: &[String],
        message: &str,
    ) -> Result<()> {
        self.write_line(&format!("MAIL FROM:<{envelope_from}>"))?;
        let response = self.read_response()?;
        ensure_smtp_code(&response, &[250], "SMTP MAIL FROM")?;

        for recipient in recipients {
            self.write_line(&format!("RCPT TO:<{recipient}>"))?;
            let response = self.read_response()?;
            ensure_smtp_code(&response, &[250, 251], "SMTP RCPT TO")?;
        }

        self.write_line("DATA")?;
        let response = self.read_response()?;
        ensure_smtp_code(&response, &[354], "SMTP DATA")?;

        self.write_data(message)?;
        let response = self.read_response()?;
        ensure_smtp_code(&response, &[250], "SMTP DATA body")
    }

    fn quit(&mut self) -> Result<()> {
        self.write_line("QUIT")?;
        let response = self.read_response()?;
        ensure_smtp_code(&response, &[221], "SMTP QUIT")
    }

    fn write_data(&mut self, message: &str) -> Result<()> {
        let normalized = message.replace("\r\n", "\n");
        let stream = self.reader.get_mut();
        for line in normalized.split('\n') {
            if line.starts_with('.') {
                stream.write_all(b".")?;
            }
            stream.write_all(line.as_bytes())?;
            stream.write_all(b"\r\n")?;
        }
        stream.write_all(b".\r\n")?;
        stream.flush()?;
        Ok(())
    }

    fn write_line(&mut self, line: &str) -> Result<()> {
        let stream = self.reader.get_mut();
        stream.write_all(line.as_bytes())?;
        stream.write_all(b"\r\n")?;
        stream.flush()?;
        Ok(())
    }

    fn read_response(&mut self) -> Result<SmtpResponse> {
        let mut lines = Vec::new();
        let mut expected_code: Option<u16> = None;
        loop {
            let line = self.read_line()?;
            if line.len() < 3 {
                return Err(YacliError::Serialization(format!(
                    "SMTP response line is too short: {line}"
                )));
            }
            let code = line[..3].parse::<u16>().map_err(|err| {
                YacliError::Serialization(format!("invalid SMTP status code in `{line}`: {err}"))
            })?;
            if let Some(expected) = expected_code {
                if code != expected {
                    return Err(YacliError::Serialization(format!(
                        "SMTP multiline response mixed status codes: expected {expected}, got {code}"
                    )));
                }
            } else {
                expected_code = Some(code);
            }

            let separator = line.as_bytes().get(3).copied().unwrap_or(b' ');
            lines.push(line.clone());
            match separator {
                b' ' => {
                    return Ok(SmtpResponse { code, lines });
                }
                b'-' => continue,
                _ => {
                    return Err(YacliError::Serialization(format!(
                        "SMTP response missing separator after status code: {line}"
                    )));
                }
            }
        }
    }

    fn read_line(&mut self) -> Result<String> {
        let mut line = String::new();
        let read = self.reader.read_line(&mut line)?;
        if read == 0 {
            return Err(YacliError::Network(
                "SMTP server closed the connection unexpectedly".to_string(),
            ));
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }
}

fn build_xoauth2_payload(account: &str, access_token: &str) -> String {
    format!("user={account}\u{1}auth=Bearer {access_token}\u{1}\u{1}")
}

fn validate_mail_summary_limit(action: &str, limit: usize) -> Result<()> {
    if limit == 0 {
        return Err(YacliError::Validation(format!(
            "{action} --limit must be greater than zero"
        )));
    }
    if limit > 100 {
        return Err(YacliError::Validation(format!(
            "{action} --limit must be 100 or less"
        )));
    }

    Ok(())
}

fn normalize_search_query(query: &str) -> Result<String> {
    let query = query.trim();
    if query.is_empty() {
        return Err(YacliError::Validation(
            "mail search text must not be empty".to_string(),
        ));
    }
    if query.contains('\r') || query.contains('\n') {
        return Err(YacliError::Validation(
            "mail search text must not contain CR or LF characters".to_string(),
        ));
    }

    Ok(query.to_string())
}

fn parse_search_uids(lines: &[String]) -> Result<Vec<u64>> {
    let mut uids = Vec::new();
    for line in lines {
        if let Some(rest) = line.strip_prefix("* SEARCH") {
            for token in rest.split_whitespace() {
                let uid = token.parse::<u64>().map_err(|err| {
                    YacliError::Serialization(format!("invalid IMAP SEARCH uid `{token}`: {err}"))
                })?;
                uids.push(uid);
            }
        }
    }

    Ok(uids)
}

fn normalize_reply_subject(subject: &str) -> String {
    let trimmed = subject.trim();
    if trimmed.is_empty() {
        return "Re:".to_string();
    }
    if starts_with_ascii_case(trimmed, "re:") {
        return trimmed.to_string();
    }

    format!("Re: {trimmed}")
}

fn normalize_forward_subject(subject: Option<&str>) -> String {
    let trimmed = subject.unwrap_or_default().trim();
    if trimmed.is_empty() {
        return "Fwd:".to_string();
    }
    if starts_with_ascii_case(trimmed, "fwd:") || starts_with_ascii_case(trimmed, "fw:") {
        return trimmed.to_string();
    }

    format!("Fwd: {trimmed}")
}

fn starts_with_ascii_case(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .map(|candidate| candidate.eq_ignore_ascii_case(prefix))
        .unwrap_or(false)
}

fn build_reply_target(headers: &[u8]) -> Result<MailReplyTarget> {
    let (parsed, _) = mailparse::parse_headers(headers)
        .map_err(|err| YacliError::Serialization(format!("failed to parse mail headers: {err}")))?;

    let reply_to = parsed.get_first_value("Reply-To");
    let from = parsed.get_first_value("From");
    let recipient_header = reply_to.or(from).ok_or_else(|| {
        YacliError::Serialization("reply target has no Reply-To or From header".to_string())
    })?;
    let recipient = extract_primary_recipient(&recipient_header)?;

    let subject = parsed.get_first_value("Subject").unwrap_or_default();
    let message_id = parsed.get_first_value("Message-ID").ok_or_else(|| {
        YacliError::Serialization("reply target has no Message-ID header".to_string())
    })?;
    let references =
        normalize_references_header(parsed.get_first_value("References").as_deref(), &message_id)?;

    Ok(MailReplyTarget {
        recipient,
        subject,
        message_id,
        references,
    })
}

fn extract_primary_recipient(header_value: &str) -> Result<String> {
    let addresses = addrparse(header_value).map_err(|err| {
        YacliError::Serialization(format!("failed to parse reply recipient header: {err}"))
    })?;

    for address in addresses.iter() {
        match address {
            MailAddr::Single(single) => return Ok(single.addr.clone()),
            MailAddr::Group(group) => {
                if let Some(first) = group.addrs.first() {
                    return Ok(first.addr.clone());
                }
            }
        }
    }

    Err(YacliError::Serialization(
        "reply target header does not contain a usable recipient address".to_string(),
    ))
}

fn normalize_references_header(references: Option<&str>, message_id: &str) -> Result<Vec<String>> {
    let message_id = sanitize_header_ascii(message_id, "Message-ID")?;
    let mut values = references
        .unwrap_or_default()
        .split_whitespace()
        .filter(|value| !value.is_empty())
        .map(|value| sanitize_header_ascii(value, "References"))
        .collect::<Result<Vec<_>>>()?;

    if !values.iter().any(|value| value == &message_id) {
        values.push(message_id);
    }

    Ok(values)
}

fn prepare_mail_submission(
    auth: MailSessionAuth,
    request: MailSendRequest,
) -> Result<PreparedMailSubmission> {
    let (from, auth) = match auth {
        MailSessionAuth::OauthXoauth2 {
            account,
            access_token,
        } => (
            account.clone(),
            MailSmtpAuth::OauthXoauth2 {
                account,
                access_token,
            },
        ),
        MailSessionAuth::AppPassword {
            account,
            app_password,
        } => (
            account.clone(),
            MailSmtpAuth::AppPassword {
                account,
                app_password,
            },
        ),
    };

    let to = normalize_recipient_list(request.to, "mail send")?;
    let cc = normalize_recipient_list(request.cc, "mail send --cc")?;
    let bcc = normalize_recipient_list(request.bcc, "mail send --bcc")?;
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err(YacliError::Validation(
            "mail send requires at least one recipient".to_string(),
        ));
    }

    let subject = normalize_subject(request.subject)?;
    let attachments = request.attachments;
    let body = normalize_outgoing_body(request.text, request.html, "mail send")?;
    let message_id = generate_message_id();
    let message = build_outgoing_message(OutgoingMessage {
        from: &from,
        to: &to,
        cc: &cc,
        subject: &subject,
        text: body.text.as_deref(),
        html: body.html.as_deref(),
        attachments: &attachments,
        message_id: &message_id,
        thread_headers: request.thread_headers.as_ref(),
    })?;
    let envelope_recipients = to
        .iter()
        .chain(cc.iter())
        .chain(bcc.iter())
        .cloned()
        .collect::<Vec<_>>();

    Ok(PreparedMailSubmission {
        auth,
        envelope_from: from.clone(),
        envelope_recipients,
        message,
        sent: SentMail {
            from,
            to,
            cc,
            bcc_count: bcc.len(),
            subject,
            message_id,
            body_kind: if attachments.is_empty() {
                body.body_kind
            } else {
                "multipart_mixed".to_string()
            },
        },
    })
}

fn normalize_forward_request(request: MailForwardRequest) -> Result<NormalizedForwardRequest> {
    if request.max_source_bytes == 0 {
        return Err(YacliError::Validation(
            "mail forward --max-source-bytes must be greater than zero".to_string(),
        ));
    }

    let to = normalize_recipient_list(request.to, "mail forward")?;
    let cc = normalize_recipient_list(request.cc, "mail forward --cc")?;
    let bcc = normalize_recipient_list(request.bcc, "mail forward --bcc")?;
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err(YacliError::Validation(
            "mail forward requires at least one recipient".to_string(),
        ));
    }

    Ok(NormalizedForwardRequest {
        uid: request.uid,
        to,
        cc,
        bcc,
        intro_text: normalize_optional_body(request.text, "mail forward text")?,
        intro_html: normalize_optional_body(request.html, "mail forward --html")?,
        max_source_bytes: request.max_source_bytes,
    })
}

fn build_forward_body(
    source: &MailMessage,
    intro_text: Option<&str>,
    intro_html: Option<&str>,
) -> ForwardBody {
    ForwardBody {
        text: build_forward_text_body(source, intro_text),
        html: build_forward_html_body(source, intro_text, intro_html),
    }
}

fn build_forward_text_body(source: &MailMessage, intro_text: Option<&str>) -> String {
    let mut lines = Vec::new();

    if let Some(intro_text) = intro_text {
        lines.push(intro_text.trim_end().to_string());
        lines.push(String::new());
    }

    lines.push("---------- Forwarded message ----------".to_string());
    lines.push(format!(
        "From: {}",
        source.from.as_deref().unwrap_or("(unknown)")
    ));
    if let Some(date) = &source.date {
        lines.push(format!("Date: {date}"));
    }
    lines.push(format!(
        "Subject: {}",
        source.subject.as_deref().unwrap_or("(без темы)")
    ));
    if let Some(to) = &source.to {
        lines.push(format!("To: {to}"));
    }
    if let Some(cc) = &source.cc {
        lines.push(format!("Cc: {cc}"));
    }
    if let Some(message_id) = &source.message_id {
        lines.push(format!("Message-ID: {message_id}"));
    }
    lines.push(String::new());
    lines.push(resolve_forward_text_source(source));

    lines.join("\n")
}

fn build_forward_html_body(
    source: &MailMessage,
    intro_text: Option<&str>,
    intro_html: Option<&str>,
) -> Option<String> {
    if source.html_body.is_none() && intro_html.is_none() {
        return None;
    }

    let mut parts = Vec::new();
    if let Some(intro_html) = intro_html {
        parts.push(intro_html.trim_end().to_string());
    } else if let Some(intro_text) = intro_text {
        parts.push(format!("<p>{}</p>", htmlize_text(intro_text.trim_end())));
    }

    let mut header_lines = vec![
        format!(
            "<strong>From:</strong> {}",
            escape_html(source.from.as_deref().unwrap_or("(unknown)"))
        ),
        format!(
            "<strong>Subject:</strong> {}",
            escape_html(source.subject.as_deref().unwrap_or("(без темы)"))
        ),
    ];
    if let Some(date) = &source.date {
        header_lines.push(format!("<strong>Date:</strong> {}", escape_html(date)));
    }
    if let Some(to) = &source.to {
        header_lines.push(format!("<strong>To:</strong> {}", escape_html(to)));
    }
    if let Some(cc) = &source.cc {
        header_lines.push(format!("<strong>Cc:</strong> {}", escape_html(cc)));
    }
    if let Some(message_id) = &source.message_id {
        header_lines.push(format!(
            "<strong>Message-ID:</strong> {}",
            escape_html(message_id)
        ));
    }

    let original_html = source
        .html_body
        .as_deref()
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            format!(
                "<pre>{}</pre>",
                escape_html(&resolve_forward_text_source(source))
            )
        });

    parts.push(format!(
        "<hr><p><strong>Forwarded message</strong><br>{}</p>{}",
        header_lines.join("<br>"),
        original_html
    ));

    Some(parts.join("\n"))
}

fn resolve_forward_text_source(source: &MailMessage) -> String {
    if let Some(text) = &source.text_body {
        return text.trim_end().to_string();
    }
    if let Some(html) = &source.html_body {
        let text = html_to_text_lossy(html);
        if !text.trim().is_empty() {
            return text;
        }
    }

    "(Исходное письмо не содержит текстового тела.)".to_string()
}

fn html_to_text_lossy(value: &str) -> String {
    let mut output = String::new();
    let mut entity = String::new();
    let mut in_tag = false;

    for ch in value.chars() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
            }
            continue;
        }
        if !entity.is_empty() {
            if ch == ';' {
                output.push_str(match entity.as_str() {
                    "&nbsp" => " ",
                    "&amp" => "&",
                    "&lt" => "<",
                    "&gt" => ">",
                    "&quot" => "\"",
                    "&#39" => "'",
                    _ => entity.as_str(),
                });
                entity.clear();
                continue;
            }
            if ch.is_ascii_alphanumeric() || ch == '#' {
                entity.push(ch);
                continue;
            }
            output.push_str(&entity);
            entity.clear();
        }

        match ch {
            '<' => in_tag = true,
            '&' => entity.push(ch),
            '\r' => {}
            _ => output.push(ch),
        }
    }

    if !entity.is_empty() {
        output.push_str(&entity);
    }

    output
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn htmlize_text(value: &str) -> String {
    escape_html(value).replace('\n', "<br>")
}

fn parse_tagged_status(
    tag: &str,
    line: &str,
    action: &str,
    error_kind: TaggedErrorKind,
) -> Result<()> {
    let Some(rest) = line.strip_prefix(tag) else {
        return Err(YacliError::Api(format!(
            "{action} returned unexpected tagged response: {line}"
        )));
    };
    let mut parts = rest.trim_start().splitn(2, ' ');
    let status = parts.next().unwrap_or_default();
    let detail = parts.next().unwrap_or_default().trim();
    if status.eq_ignore_ascii_case("OK") {
        return Ok(());
    }

    let message = format!(
        "{action} failed: {}",
        if detail.is_empty() { line } else { detail }
    );
    match error_kind {
        TaggedErrorKind::Auth => Err(YacliError::Auth(message)),
        TaggedErrorKind::Api => Err(YacliError::Api(message)),
    }
}

fn ensure_smtp_code(response: &SmtpResponse, expected: &[u16], action: &str) -> Result<()> {
    if expected.contains(&response.code) {
        return Ok(());
    }

    let message = format!("{action} failed: {}", flatten_smtp_response(response));
    Err(YacliError::Api(message))
}

fn flatten_smtp_response(response: &SmtpResponse) -> String {
    response.lines.join(" | ")
}

fn smtp_response_detail(response: &SmtpResponse) -> Option<String> {
    response
        .lines
        .last()
        .and_then(|line| line.get(4..))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn decode_smtp_auth_challenge(response: &SmtpResponse) -> Option<String> {
    let detail = smtp_response_detail(response)?;
    let decoded = STANDARD.decode(detail.as_bytes()).ok()?;
    String::from_utf8(decoded)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

struct FetchMetadata {
    uid: u64,
    flags: Vec<String>,
    size: Option<u64>,
    header_literal_len: Option<usize>,
}

fn parse_fetch_metadata(line: &str) -> Result<FetchMetadata> {
    let uid = extract_u64_after_keyword(line, "UID ")?;
    let size = extract_optional_u64_after_keyword(line, "RFC822.SIZE ")?;
    let flags = extract_flags(line)?;
    let header_literal_len = extract_trailing_literal_len(line)?;

    Ok(FetchMetadata {
        uid,
        flags,
        size,
        header_literal_len,
    })
}

fn build_message_summary(metadata: FetchMetadata, headers: &[u8]) -> Result<MailMessageSummary> {
    let (subject, from, date) = if headers.is_empty() {
        (None, None, None)
    } else {
        let (parsed, _) = mailparse::parse_headers(headers).map_err(|err| {
            YacliError::Serialization(format!("failed to parse mail headers: {err}"))
        })?;
        (
            parsed.get_first_value("Subject"),
            parsed.get_first_value("From"),
            parsed.get_first_value("Date"),
        )
    };

    Ok(MailMessageSummary {
        uid: metadata.uid,
        subject,
        from,
        date,
        flags: metadata.flags,
        size: metadata.size,
    })
}

fn build_read_message(
    metadata: FetchMetadata,
    headers: &[u8],
    raw_message: &[u8],
) -> Result<MailMessage> {
    let (subject, from, to, cc, date, message_id) = if headers.is_empty() {
        (None, None, None, None, None, None)
    } else {
        let (parsed, _) = mailparse::parse_headers(headers).map_err(|err| {
            YacliError::Serialization(format!("failed to parse mail headers: {err}"))
        })?;
        (
            parsed.get_first_value("Subject"),
            parsed.get_first_value("From"),
            parsed.get_first_value("To"),
            parsed.get_first_value("Cc"),
            parsed.get_first_value("Date"),
            parsed.get_first_value("Message-ID"),
        )
    };

    let parsed_mail = mailparse::parse_mail(raw_message).map_err(|err| {
        YacliError::Serialization(format!("failed to parse RFC822 message: {err}"))
    })?;
    let extracted = extract_message_content(&parsed_mail)?;

    Ok(MailMessage {
        uid: metadata.uid,
        subject,
        from,
        to,
        cc,
        date,
        message_id,
        flags: metadata.flags,
        size: metadata.size,
        text_body: extracted.text_body,
        html_body: extracted.html_body,
        attachments: extracted
            .attachment_parts
            .iter()
            .map(MailAttachmentPart::summary)
            .collect(),
        raw_attachments: extracted.attachment_parts,
    })
}

struct ExtractedMessageContent {
    text_body: Option<String>,
    html_body: Option<String>,
    attachment_parts: Vec<MailAttachmentPart>,
}

fn extract_message_content(
    parsed_mail: &mailparse::ParsedMail<'_>,
) -> Result<ExtractedMessageContent> {
    let mut content = ExtractedMessageContent {
        text_body: None,
        html_body: None,
        attachment_parts: Vec::new(),
    };

    for part in parsed_mail.parts() {
        if !part.subparts.is_empty() {
            continue;
        }

        let disposition = part.get_content_disposition();
        let filename = disposition
            .params
            .get("filename")
            .cloned()
            .or_else(|| part.ctype.params.get("name").cloned());
        let content_id = part.headers.get_first_value("Content-ID");
        let inline = disposition.disposition == DispositionType::Inline;
        let mime_type = part.ctype.mimetype.to_lowercase();
        let is_attachment = disposition.disposition == DispositionType::Attachment
            || filename.is_some()
            || mime_type == "text/calendar";

        if is_attachment {
            let payload = part.get_body_raw().map_err(|err| {
                YacliError::Serialization(format!("failed to decode attachment payload: {err}"))
            })?;
            content.attachment_parts.push(MailAttachmentPart {
                filename,
                mime_type,
                content_id,
                inline,
                content: payload,
            });
            continue;
        }

        match mime_type.as_str() {
            "text/plain" if content.text_body.is_none() => {
                content.text_body = Some(part.get_body().map_err(|err| {
                    YacliError::Serialization(format!(
                        "failed to decode text/plain message body: {err}"
                    ))
                })?);
            }
            "text/html" if content.html_body.is_none() => {
                content.html_body = Some(part.get_body().map_err(|err| {
                    YacliError::Serialization(format!(
                        "failed to decode text/html message body: {err}"
                    ))
                })?);
            }
            _ => {}
        }
    }

    if content.text_body.is_none() && content.html_body.is_none() && parsed_mail.subparts.is_empty()
    {
        content.text_body = Some(parsed_mail.get_body().map_err(|err| {
            YacliError::Serialization(format!("failed to decode single-part message body: {err}"))
        })?);
    }

    Ok(content)
}

fn ensure_fetch_literal_closed(trailer: &str) -> Result<()> {
    if trailer.trim() == ")" {
        return Ok(());
    }

    Err(YacliError::Serialization(format!(
        "unexpected IMAP FETCH literal trailer: {trailer}"
    )))
}

fn extract_u64_after_keyword(line: &str, keyword: &str) -> Result<u64> {
    extract_optional_u64_after_keyword(line, keyword)?.ok_or_else(|| {
        YacliError::Serialization(format!(
            "missing `{keyword}` in IMAP FETCH response: {line}"
        ))
    })
}

fn extract_optional_u64_after_keyword(line: &str, keyword: &str) -> Result<Option<u64>> {
    let Some(start) = line.find(keyword) else {
        return Ok(None);
    };
    let rest = &line[start + keyword.len()..];
    let digits = rest
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return Err(YacliError::Serialization(format!(
            "expected digits after `{keyword}` in IMAP FETCH response: {line}"
        )));
    }

    let value = digits.parse::<u64>().map_err(|err| {
        YacliError::Serialization(format!(
            "invalid numeric value after `{keyword}` in IMAP FETCH response: {err}"
        ))
    })?;
    Ok(Some(value))
}

fn extract_flags(line: &str) -> Result<Vec<String>> {
    let Some(start) = line.find("FLAGS (") else {
        return Ok(Vec::new());
    };
    let rest = &line[start + "FLAGS (".len()..];
    let end = rest.find(')').ok_or_else(|| {
        YacliError::Serialization(format!(
            "unterminated FLAGS block in IMAP FETCH response: {line}"
        ))
    })?;
    Ok(rest[..end]
        .split_whitespace()
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect())
}

fn extract_trailing_literal_len(line: &str) -> Result<Option<usize>> {
    let Some(open) = line.rfind('{') else {
        return Ok(None);
    };
    if !line.ends_with('}') {
        return Ok(None);
    }

    let digits = line[open + 1..line.len() - 1].trim();
    let value = digits.parse::<usize>().map_err(|err| {
        YacliError::Serialization(format!(
            "invalid literal size in IMAP FETCH response `{line}`: {err}"
        ))
    })?;
    Ok(Some(value))
}

fn parse_list_line(line: &str) -> Result<MailFolder> {
    let payload = line
        .strip_prefix("* LIST ")
        .ok_or_else(|| YacliError::Serialization(format!("invalid IMAP LIST line: {line}")))?;
    let attrs_end = payload.find(')').ok_or_else(|| {
        YacliError::Serialization(format!("invalid IMAP LIST attributes: {line}"))
    })?;
    let attrs_raw = payload
        .strip_prefix('(')
        .and_then(|value| value.get(..attrs_end.saturating_sub(1)))
        .ok_or_else(|| {
            YacliError::Serialization(format!("invalid IMAP LIST attribute block: {line}"))
        })?;

    let attributes = attrs_raw
        .split_whitespace()
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    let rest = payload
        .get(attrs_end + 1..)
        .ok_or_else(|| YacliError::Serialization(format!("invalid IMAP LIST payload: {line}")))?;
    let (delimiter, rest) = parse_imap_token(rest.trim_start())?;
    let (raw_name, _) = parse_imap_token(rest.trim_start())?;
    let raw_name = raw_name.ok_or_else(|| {
        YacliError::Serialization(format!(
            "IMAP LIST mailbox name is NIL, which is unsupported: {line}"
        ))
    })?;

    Ok(MailFolder {
        name: decode_modified_utf7(&raw_name)?,
        raw_name,
        delimiter,
        attributes,
    })
}

fn parse_imap_token(input: &str) -> Result<(Option<String>, &str)> {
    let trimmed = input.trim_start();
    if trimmed.is_empty() {
        return Err(YacliError::Serialization(
            "unexpected end of IMAP token stream".to_string(),
        ));
    }

    if trimmed.starts_with("NIL") {
        return Ok((None, trimmed.get(3..).unwrap_or("")));
    }

    if let Some(rest) = trimmed.strip_prefix('"') {
        let mut escaped = false;
        let mut value = String::new();
        for (index, ch) in rest.char_indices() {
            if escaped {
                value.push(ch);
                escaped = false;
                continue;
            }

            match ch {
                '\\' => escaped = true,
                '"' => return Ok((Some(value), rest.get(index + 1..).unwrap_or(""))),
                _ => value.push(ch),
            }
        }

        return Err(YacliError::Serialization(
            "unterminated IMAP quoted string".to_string(),
        ));
    }

    let end = trimmed.find(' ').unwrap_or(trimmed.len());
    Ok((
        Some(trimmed[..end].to_string()),
        trimmed.get(end..).unwrap_or(""),
    ))
}

fn decode_modified_utf7(raw_name: &str) -> Result<String> {
    let mut decoded = String::new();
    let mut chars = raw_name.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '&' {
            decoded.push(ch);
            continue;
        }

        if chars.peek() == Some(&'-') {
            chars.next();
            decoded.push('&');
            continue;
        }

        let mut shifted = String::new();
        for next in chars.by_ref() {
            if next == '-' {
                break;
            }
            shifted.push(next);
        }

        let mut base64 = shifted.replace(',', "/");
        while base64.len() % 4 != 0 {
            base64.push('=');
        }

        let bytes = STANDARD.decode(base64.as_bytes()).map_err(|err| {
            YacliError::Serialization(format!("invalid modified UTF-7 folder name: {err}"))
        })?;
        if bytes.len() % 2 != 0 {
            return Err(YacliError::Serialization(
                "modified UTF-7 folder name is not valid UTF-16BE".to_string(),
            ));
        }

        let utf16 = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        let text = String::from_utf16(&utf16).map_err(|err| {
            YacliError::Serialization(format!(
                "invalid UTF-16 in modified UTF-7 folder name: {err}"
            ))
        })?;
        decoded.push_str(&text);
    }

    Ok(decoded)
}

fn encode_modified_utf7(mailbox_name: &str) -> String {
    let mut encoded = String::new();
    let mut shifted = String::new();

    for ch in mailbox_name.chars() {
        if is_direct_imap_char(ch) {
            if !shifted.is_empty() {
                encoded.push_str(&encode_shifted_run(&shifted));
                shifted.clear();
            }
            encoded.push(ch);
            continue;
        }

        if ch == '&' {
            if !shifted.is_empty() {
                encoded.push_str(&encode_shifted_run(&shifted));
                shifted.clear();
            }
            encoded.push_str("&-");
            continue;
        }

        shifted.push(ch);
    }

    if !shifted.is_empty() {
        encoded.push_str(&encode_shifted_run(&shifted));
    }

    encoded
}

fn normalize_recipient_list(values: Vec<String>, flag_name: &str) -> Result<Vec<String>> {
    values
        .into_iter()
        .map(|value| normalize_recipient(&value, flag_name))
        .collect()
}

fn normalize_recipient(value: &str, flag_name: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(YacliError::Validation(format!(
            "{flag_name} must not contain empty recipients"
        )));
    }
    if trimmed.contains(['\r', '\n']) {
        return Err(YacliError::Validation(format!(
            "{flag_name} recipient must not contain CR or LF characters"
        )));
    }
    if trimmed.contains(',') {
        return Err(YacliError::Validation(format!(
            "{flag_name} accepts one email address per flag; repeat the flag for multiple recipients"
        )));
    }
    if !trimmed.is_ascii() {
        return Err(YacliError::Validation(format!(
            "{flag_name} recipient must be ASCII-only email address"
        )));
    }
    if trimmed.contains(char::is_whitespace) {
        return Err(YacliError::Validation(format!(
            "{flag_name} recipient must not contain whitespace"
        )));
    }
    if !trimmed.contains('@') {
        return Err(YacliError::Validation(format!(
            "{flag_name} recipient must contain `@`"
        )));
    }
    Ok(trimmed.to_string())
}

fn normalize_subject(subject: String) -> Result<String> {
    let trimmed = subject.trim();
    if trimmed.is_empty() {
        return Err(YacliError::Validation(
            "mail send subject must not be empty".to_string(),
        ));
    }
    if trimmed.contains(['\r', '\n']) {
        return Err(YacliError::Validation(
            "mail send subject must not contain CR or LF characters".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_body(body: Option<String>, flag_name: &str) -> Result<Option<String>> {
    let Some(body) = body else {
        return Ok(None);
    };
    if body.contains('\0') {
        return Err(YacliError::Validation(format!(
            "{flag_name} must not contain NUL characters"
        )));
    }
    if body.is_empty() {
        return Err(YacliError::Validation(format!(
            "{flag_name} must not be empty"
        )));
    }
    Ok(Some(body))
}

fn normalize_outgoing_body(
    text: Option<String>,
    html: Option<String>,
    command_name: &str,
) -> Result<NormalizedOutgoingBody> {
    let text = normalize_optional_body(text, &format!("{command_name} text"))?;
    let html = normalize_optional_body(html, &format!("{command_name} --html"))?;
    let body_kind = match (text.is_some(), html.is_some()) {
        (true, true) => "multipart_alternative",
        (true, false) => "text",
        (false, true) => "html",
        (false, false) => {
            return Err(YacliError::Validation(format!(
                "{command_name} requires text or --html"
            )));
        }
    }
    .to_string();

    Ok(NormalizedOutgoingBody {
        text,
        html,
        body_kind,
    })
}

fn build_outgoing_message(outgoing: OutgoingMessage<'_>) -> Result<String> {
    let from_header = sanitize_header_ascii(outgoing.from, "From")?;
    let to_header = sanitize_header_ascii(&outgoing.to.join(", "), "To")?;
    let cc_header = if outgoing.cc.is_empty() {
        None
    } else {
        Some(sanitize_header_ascii(&outgoing.cc.join(", "), "Cc")?)
    };
    let subject_header = encode_header_value(outgoing.subject)?;
    let date_header = Utc::now().to_rfc2822();

    let mut headers = vec![
        format!("From: {from_header}"),
        format!("To: {to_header}"),
        format!("Subject: {subject_header}"),
        format!("Date: {date_header}"),
        format!("Message-ID: <{}>", outgoing.message_id),
        "MIME-Version: 1.0".to_string(),
    ];
    if let Some(cc_header) = cc_header {
        headers.push(format!("Cc: {cc_header}"));
    }
    if let Some(thread_headers) = outgoing.thread_headers {
        let in_reply_to = sanitize_header_ascii(&thread_headers.in_reply_to, "In-Reply-To")?;
        headers.push(format!("In-Reply-To: {in_reply_to}"));
        if !thread_headers.references.is_empty() {
            let references =
                sanitize_header_ascii(&thread_headers.references.join(" "), "References")?;
            headers.push(format!("References: {references}"));
        }
    }

    let body = if outgoing.attachments.is_empty() {
        build_root_body(&mut headers, outgoing.text, outgoing.html)?
    } else {
        let boundary = format!("yacli-mix-{:016x}", random::<u64>());
        headers.push(format!(
            "Content-Type: multipart/mixed; boundary=\"{boundary}\""
        ));
        build_multipart_mixed_body(
            &boundary,
            outgoing.text,
            outgoing.html,
            outgoing.attachments,
        )?
    };

    let mut message = headers.join("\r\n");
    message.push_str("\r\n\r\n");
    message.push_str(&body);
    Ok(message)
}

fn build_root_body(
    headers: &mut Vec<String>,
    text: Option<&str>,
    html: Option<&str>,
) -> Result<String> {
    match (text, html) {
        (Some(text), Some(html)) => {
            let boundary = format!("yacli-alt-{:016x}", random::<u64>());
            headers.push(format!(
                "Content-Type: multipart/alternative; boundary=\"{boundary}\""
            ));
            Ok(build_multipart_alternative_body(&boundary, text, html))
        }
        (Some(text), None) => {
            headers.push("Content-Type: text/plain; charset=UTF-8".to_string());
            headers.push("Content-Transfer-Encoding: base64".to_string());
            Ok(encode_bytes_base64(text.as_bytes()))
        }
        (None, Some(html)) => {
            headers.push("Content-Type: text/html; charset=UTF-8".to_string());
            headers.push("Content-Transfer-Encoding: base64".to_string());
            Ok(encode_bytes_base64(html.as_bytes()))
        }
        (None, None) => Err(YacliError::Validation(
            "mail send requires text or --html".to_string(),
        )),
    }
}

fn build_multipart_alternative_body(boundary: &str, text: &str, html: &str) -> String {
    let lines = vec![
        format!("--{boundary}"),
        "Content-Type: text/plain; charset=UTF-8".to_string(),
        "Content-Transfer-Encoding: base64".to_string(),
        String::new(),
        encode_bytes_base64(text.as_bytes()),
        format!("--{boundary}"),
        "Content-Type: text/html; charset=UTF-8".to_string(),
        "Content-Transfer-Encoding: base64".to_string(),
        String::new(),
        encode_bytes_base64(html.as_bytes()),
        format!("--{boundary}--"),
    ];
    lines.join("\r\n")
}

fn build_multipart_mixed_body(
    boundary: &str,
    text: Option<&str>,
    html: Option<&str>,
    attachments: &[MailAttachmentPayload],
) -> Result<String> {
    let mut lines = vec![format!("--{boundary}")];
    lines.extend(build_primary_body_part(text, html)?);

    for attachment in attachments {
        lines.push(format!("--{boundary}"));
        lines.extend(build_attachment_part(attachment)?);
    }
    lines.push(format!("--{boundary}--"));

    Ok(lines.join("\r\n"))
}

fn build_primary_body_part(text: Option<&str>, html: Option<&str>) -> Result<Vec<String>> {
    match (text, html) {
        (Some(text), Some(html)) => {
            let boundary = format!("yacli-alt-{:016x}", random::<u64>());
            Ok(vec![
                format!("Content-Type: multipart/alternative; boundary=\"{boundary}\""),
                String::new(),
                build_multipart_alternative_body(&boundary, text, html),
            ])
        }
        (Some(text), None) => Ok(vec![
            "Content-Type: text/plain; charset=UTF-8".to_string(),
            "Content-Transfer-Encoding: base64".to_string(),
            String::new(),
            encode_bytes_base64(text.as_bytes()),
        ]),
        (None, Some(html)) => Ok(vec![
            "Content-Type: text/html; charset=UTF-8".to_string(),
            "Content-Transfer-Encoding: base64".to_string(),
            String::new(),
            encode_bytes_base64(html.as_bytes()),
        ]),
        (None, None) => Err(YacliError::Validation(
            "mail send requires text or --html".to_string(),
        )),
    }
}

fn build_attachment_part(attachment: &MailAttachmentPayload) -> Result<Vec<String>> {
    let mime_type = sanitize_mime_type(&attachment.mime_type)?;
    let mut content_type = mime_type.clone();
    let mut disposition = if attachment.inline {
        "inline".to_string()
    } else {
        "attachment".to_string()
    };

    if let Some(filename) = &attachment.filename {
        let name_param = build_mime_parameter("name", filename)?;
        let filename_param = build_mime_parameter("filename", filename)?;
        content_type.push_str("; ");
        content_type.push_str(&name_param);
        disposition.push_str("; ");
        disposition.push_str(&filename_param);
    }

    let mut lines = vec![
        format!("Content-Type: {content_type}"),
        "Content-Transfer-Encoding: base64".to_string(),
        format!("Content-Disposition: {disposition}"),
    ];

    if let Some(content_id) = &attachment.content_id {
        lines.push(format!(
            "Content-ID: {}",
            normalize_content_id_header(content_id)?
        ));
    }

    lines.push(String::new());
    lines.push(encode_bytes_base64(&attachment.content));
    Ok(lines)
}

fn encode_bytes_base64(value: &[u8]) -> String {
    wrap_base64(&STANDARD.encode(value), 76)
}

fn sanitize_mime_type(value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok("application/octet-stream".to_string());
    }
    if trimmed.contains(['\r', '\n']) {
        return Err(YacliError::Validation(
            "mail attachment MIME type must not contain CR or LF characters".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

fn normalize_content_id_header(value: &str) -> Result<String> {
    let trimmed = sanitize_header_ascii(value.trim(), "Content-ID")?;
    if trimmed.starts_with('<') && trimmed.ends_with('>') {
        return Ok(trimmed);
    }
    Ok(format!("<{trimmed}>"))
}

fn build_mime_parameter(name: &str, value: &str) -> Result<String> {
    if value.contains(['\r', '\n']) {
        return Err(YacliError::Validation(format!(
            "{name} MIME parameter must not contain CR or LF characters"
        )));
    }

    if value.is_ascii() {
        return Ok(format!(
            r#"{name}="{}""#,
            value.replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }

    Ok(format!(
        "{name}*=UTF-8''{}",
        percent_encode_mime_value(value)
    ))
}

fn percent_encode_mime_value(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| match byte {
            b'0'..=b'9'
            | b'A'..=b'Z'
            | b'a'..=b'z'
            | b'!'
            | b'#'
            | b'$'
            | b'&'
            | b'+'
            | b'-'
            | b'.'
            | b'^'
            | b'_'
            | b'`'
            | b'|'
            | b'~' => (*byte as char).to_string(),
            _ => format!("%{:02X}", byte),
        })
        .collect()
}

fn wrap_base64(value: &str, width: usize) -> String {
    value
        .as_bytes()
        .chunks(width)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default().to_string())
        .collect::<Vec<_>>()
        .join("\r\n")
}

fn encode_header_value(value: &str) -> Result<String> {
    if value.contains(['\r', '\n']) {
        return Err(YacliError::Validation(
            "mail send headers must not contain CR or LF characters".to_string(),
        ));
    }
    if value.is_ascii() {
        return Ok(value.to_string());
    }
    Ok(format!("=?UTF-8?B?{}?=", STANDARD.encode(value.as_bytes())))
}

fn sanitize_header_ascii(value: &str, header_name: &str) -> Result<String> {
    if value.contains(['\r', '\n']) {
        return Err(YacliError::Validation(format!(
            "{header_name} header must not contain CR or LF characters"
        )));
    }
    if !value.is_ascii() {
        return Err(YacliError::Validation(format!(
            "{header_name} header must be ASCII-only in mail send core"
        )));
    }
    Ok(value.to_string())
}

fn generate_message_id() -> String {
    format!(
        "yacli-{}-{:016x}@nextstat.dev",
        Utc::now().timestamp_micros(),
        random::<u64>()
    )
}

fn is_direct_imap_char(ch: char) -> bool {
    ch.is_ascii() && (' '..='~').contains(&ch) && ch != '&'
}

fn encode_shifted_run(value: &str) -> String {
    let utf16 = value
        .encode_utf16()
        .flat_map(u16::to_be_bytes)
        .collect::<Vec<_>>();
    format!(
        "&{}-",
        STANDARD
            .encode(utf16)
            .trim_end_matches('=')
            .replace('/', ",")
    )
}

fn quote_imap_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn validate_mail_attachment_export_request(request: &MailAttachmentExportRequest) -> Result<()> {
    validate_mail_attachment_read_request(
        request.uid,
        &request.selector,
        request.max_bytes,
        "mail attachment export",
    )
}

fn validate_mail_attachment_read_request(
    uid: u64,
    selector: &MailAttachmentSelector,
    max_bytes: u64,
    command_name: &str,
) -> Result<()> {
    if uid == 0 {
        return Err(YacliError::Validation(format!(
            "{command_name} <id> must be greater than zero"
        )));
    }
    if max_bytes == 0 {
        return Err(YacliError::Validation(format!(
            "{command_name} --max-bytes must be greater than zero"
        )));
    }
    match selector {
        MailAttachmentSelector::Index(index) => {
            if *index == 0 {
                return Err(YacliError::Validation(format!(
                    "{command_name} --index must be greater than zero"
                )));
            }
        }
        MailAttachmentSelector::Filename(filename) => {
            if filename.trim().is_empty() {
                return Err(YacliError::Validation(format!(
                    "{command_name} --name must not be empty"
                )));
            }
        }
    }

    Ok(())
}

fn load_mail_attachment(path: &Path) -> Result<MailAttachmentPayload> {
    let metadata = fs::metadata(path).map_err(|err| {
        YacliError::Io(format!(
            "unable to read attachment metadata {}: {err}",
            path.display()
        ))
    })?;
    if metadata.is_dir() {
        return Err(YacliError::UnsupportedOperation(format!(
            "attachment path points to a directory: {}",
            path.display()
        )));
    }

    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            YacliError::Validation(format!(
                "attachment path must end with a valid UTF-8 filename: {}",
                path.display()
            ))
        })?;

    let content = fs::read(path).map_err(|err| {
        YacliError::Io(format!(
            "unable to read attachment file {}: {err}",
            path.display()
        ))
    })?;
    let mime_type = detect_attachment_mime_type(path);

    Ok(MailAttachmentPayload {
        filename: Some(filename.to_string()),
        mime_type,
        content_id: None,
        inline: false,
        content,
    })
}

fn detect_attachment_mime_type(path: &Path) -> String {
    MimeGuess::from_path(path)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_string()
}

fn export_attachment_from_message(
    message: &MailMessage,
    selector: &MailAttachmentSelector,
    output: &Path,
    force: bool,
) -> Result<ExportedMailAttachment> {
    let (attachment_index, attachment) = select_attachment_part(message, selector)?;
    let artifact = write_attachment_to_path(&attachment.content, output, force)?;

    Ok(ExportedMailAttachment {
        message_uid: message.uid,
        attachment_index,
        filename: attachment.filename.clone(),
        mime_type: attachment.mime_type.clone(),
        content_id: attachment.content_id.clone(),
        inline: attachment.inline,
        output_path: artifact.output_path,
        bytes_written: artifact.bytes_written,
        sha256: artifact.sha256,
    })
}

fn inspect_invite_from_message(
    message: &MailMessage,
    selector: &MailAttachmentSelector,
) -> Result<InspectedMailInvite> {
    let (attachment_index, attachment) = select_attachment_part(message, selector)?;
    let is_calendar_payload = attachment.mime_type == "text/calendar"
        || attachment
            .filename
            .as_deref()
            .map(|name| name.to_ascii_lowercase().ends_with(".ics"))
            .unwrap_or(false);
    if !is_calendar_payload {
        return Err(YacliError::UnsupportedOperation(format!(
            "selected attachment is not a calendar invite: mime_type={}",
            attachment.mime_type
        )));
    }

    let raw = std::str::from_utf8(&attachment.content).map_err(|err| {
        YacliError::Serialization(format!("failed to decode calendar invite as UTF-8: {err}"))
    })?;
    let invites = parse_calendar_invites(raw)?;
    if invites.is_empty() {
        return Err(YacliError::UnsupportedOperation(
            "calendar invite did not contain any VEVENT entries".to_string(),
        ));
    }

    Ok(InspectedMailInvite {
        message_uid: message.uid,
        attachment_index,
        filename: attachment.filename.clone(),
        mime_type: attachment.mime_type.clone(),
        content_id: attachment.content_id.clone(),
        inline: attachment.inline,
        invites,
    })
}

fn select_attachment_part<'a>(
    message: &'a MailMessage,
    selector: &MailAttachmentSelector,
) -> Result<(usize, &'a MailAttachmentPart)> {
    if message.raw_attachments.is_empty() {
        return Err(YacliError::UnsupportedOperation(format!(
            "message id {} has no attachments",
            message.uid
        )));
    }

    match selector {
        MailAttachmentSelector::Index(index) => message
            .raw_attachments
            .get(index - 1)
            .map(|attachment| (*index, attachment))
            .ok_or_else(|| {
                YacliError::Validation(format!(
                    "mail attachment export attachment index {} out of range; message has {} attachment(s)",
                    index,
                    message.raw_attachments.len()
                ))
            }),
        MailAttachmentSelector::Filename(filename) => {
            let normalized = filename.trim();
            let mut matches = message
                .raw_attachments
                .iter()
                .enumerate()
                .filter(|(_, attachment)| attachment.filename.as_deref() == Some(normalized));

            let Some((first_index, first_attachment)) = matches.next() else {
                return Err(YacliError::Validation(format!(
                    "mail attachment export attachment not found: {}",
                    normalized
                )));
            };

            if matches.next().is_some() {
                return Err(YacliError::UnsupportedOperation(format!(
                    "mail attachment export attachment name `{}` is ambiguous; use --index",
                    normalized
                )));
            }

            Ok((first_index + 1, first_attachment))
        }
    }
}

fn write_attachment_to_path(contents: &[u8], output: &Path, force: bool) -> Result<ExportedBytes> {
    if output.exists() && !force {
        return Err(YacliError::OutputExists(output.display().to_string()));
    }

    if output.is_dir() {
        return Err(YacliError::UnsupportedOperation(format!(
            "output path points to a directory: {}",
            output.display()
        )));
    }

    let temp_dir = match output.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            fs::create_dir_all(parent)?;
            parent.to_path_buf()
        }
        _ => std::env::current_dir()?,
    };

    let mut temp_file = NamedTempFile::new_in(&temp_dir)?;
    temp_file.write_all(contents)?;
    temp_file.as_file_mut().sync_all()?;

    if force && output.exists() {
        fs::remove_file(output)?;
    }

    temp_file
        .persist(output)
        .map_err(|err| YacliError::Io(err.error.to_string()))?;

    let mut hasher = Sha256::new();
    hasher.update(contents);

    Ok(ExportedBytes {
        output_path: output.display().to_string(),
        bytes_written: contents.len() as u64,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

struct ExportedBytes {
    output_path: String,
    bytes_written: u64,
    sha256: String,
}

#[cfg(test)]
mod tests {
    use super::{
        FetchMetadata, MailAttachmentExportRequest, MailAttachmentPart, MailAttachmentPayload,
        MailAttachmentSelector, MailAttachmentSummary, MailMessage, MailSendRequest,
        MailSessionAuth, MailThreadHeaders, OutgoingMessage, SmtpSession, build_forward_body,
        build_message_summary, build_outgoing_message, build_read_message, build_reply_target,
        build_xoauth2_payload, decode_modified_utf7, encode_modified_utf7,
        export_attachment_from_message, extract_message_content, inspect_invite_from_message,
        load_mail_attachments, normalize_forward_subject, normalize_reply_subject,
        normalize_search_query, parse_fetch_metadata, parse_imap_token, parse_list_line,
        parse_search_uids, prepare_mail_submission, quote_imap_string,
        validate_mail_attachment_export_request, validate_mail_attachment_read_request,
    };
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use std::io::{Cursor, Read, Result as IoResult, Write};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use tempfile::tempdir;

    #[derive(Clone)]
    struct FakeStream {
        reader: Cursor<Vec<u8>>,
        writes: Arc<Mutex<Vec<u8>>>,
    }

    impl FakeStream {
        fn new(script: &str) -> Self {
            Self {
                reader: Cursor::new(script.as_bytes().to_vec()),
                writes: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn written(&self) -> String {
            String::from_utf8(self.writes.lock().expect("lock").clone()).expect("utf8")
        }
    }

    impl Read for FakeStream {
        fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
            self.reader.read(buf)
        }
    }

    impl Write for FakeStream {
        fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
            self.writes.lock().expect("lock").extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> IoResult<()> {
            Ok(())
        }
    }

    #[test]
    fn decode_modified_utf7_decodes_unicode_mailbox_names() {
        let utf16 = "Отправленные"
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect::<Vec<_>>();
        let encoded = format!(
            "&{}-",
            STANDARD
                .encode(utf16)
                .trim_end_matches('=')
                .replace('/', ",")
        );

        let decoded = decode_modified_utf7(&encoded).expect("mailbox decoded");
        assert_eq!(decoded, "Отправленные");
    }

    #[test]
    fn decode_modified_utf7_preserves_literal_ampersand() {
        let decoded = decode_modified_utf7("Support &- Sales").expect("mailbox decoded");
        assert_eq!(decoded, "Support & Sales");
    }

    #[test]
    fn encode_modified_utf7_encodes_unicode_mailbox_names() {
        let encoded = encode_modified_utf7("Отправленные");
        assert_eq!(encoded, "&BB4EQgQ,BEAEMAQyBDsENQQ9BD0ESwQ1-");
    }

    #[test]
    fn parse_list_line_supports_quoted_mailbox_names() {
        let folder = parse_list_line(
            r#"* LIST (\HasNoChildren \Sent) "/" "&BB4EQgQ,BEAEMAQyBDsENQQ9BD0ESwQ1-""#,
        )
        .expect("folder parsed");
        assert_eq!(folder.raw_name, "&BB4EQgQ,BEAEMAQyBDsENQQ9BD0ESwQ1-");
        assert_eq!(folder.name, "Отправленные");
        assert_eq!(folder.delimiter.as_deref(), Some("/"));
        assert_eq!(folder.attributes, vec![r"\HasNoChildren", r"\Sent"]);
    }

    #[test]
    fn parse_imap_token_supports_atoms_and_nil() {
        let (token, rest) = parse_imap_token("INBOX tail").expect("atom token");
        assert_eq!(token.as_deref(), Some("INBOX"));
        assert_eq!(rest.trim_start(), "tail");

        let (token, rest) = parse_imap_token("NIL tail").expect("nil token");
        assert!(token.is_none());
        assert_eq!(rest.trim_start(), "tail");
    }

    #[test]
    fn quote_imap_string_escapes_quotes_and_backslashes() {
        let quoted = quote_imap_string(r#"my"user\name"#);
        assert_eq!(quoted, r#""my\"user\\name""#);
    }

    #[test]
    fn normalize_search_query_rejects_empty_and_multiline_values() {
        let empty = normalize_search_query("   ").expect_err("empty query rejected");
        assert_eq!(
            empty.to_string(),
            "Validation error: mail search text must not be empty"
        );

        let multiline =
            normalize_search_query("hello\nworld").expect_err("multiline query rejected");
        assert_eq!(
            multiline.to_string(),
            "Validation error: mail search text must not contain CR or LF characters"
        );
    }

    #[test]
    fn normalize_search_query_trims_whitespace() {
        let query = normalize_search_query("  Budget  ").expect("query normalized");
        assert_eq!(query, "Budget");
    }

    #[test]
    fn parse_search_uids_extracts_all_matches() {
        let lines = vec![
            "* SEARCH 42 99 1353".to_string(),
            "* OK some unrelated response".to_string(),
        ];
        let uids = parse_search_uids(&lines).expect("uids parsed");
        assert_eq!(uids, vec![42, 99, 1353]);
    }

    #[test]
    fn normalize_reply_subject_preserves_existing_prefix() {
        assert_eq!(normalize_reply_subject("Budget"), "Re: Budget");
        assert_eq!(normalize_reply_subject("Re: Budget"), "Re: Budget");
        assert_eq!(normalize_reply_subject("re: Budget"), "re: Budget");
        assert_eq!(
            normalize_reply_subject("Счёт на оплату"),
            "Re: Счёт на оплату"
        );
        assert_eq!(normalize_reply_subject("   "), "Re:");
    }

    #[test]
    fn normalize_forward_subject_preserves_existing_prefix() {
        assert_eq!(normalize_forward_subject(Some("Budget")), "Fwd: Budget");
        assert_eq!(
            normalize_forward_subject(Some("Fwd: Budget")),
            "Fwd: Budget"
        );
        assert_eq!(normalize_forward_subject(Some("fw: Budget")), "fw: Budget");
        assert_eq!(
            normalize_forward_subject(Some("Счёт на оплату")),
            "Fwd: Счёт на оплату"
        );
        assert_eq!(normalize_forward_subject(Some("   ")), "Fwd:");
        assert_eq!(normalize_forward_subject(None), "Fwd:");
    }

    #[test]
    fn build_reply_target_prefers_reply_to_and_extends_references() {
        let target = build_reply_target(
            b"Subject: Hello\r\nFrom: Sender <sender@example.com>\r\nReply-To: Reply <reply@example.com>\r\nMessage-ID: <msg-1@example>\r\nReferences: <root@example>\r\n\r\n",
        )
        .expect("reply target");

        assert_eq!(target.recipient, "reply@example.com");
        assert_eq!(target.subject, "Hello");
        assert_eq!(target.message_id, "<msg-1@example>");
        assert_eq!(
            target.references,
            vec!["<root@example>".to_string(), "<msg-1@example>".to_string()]
        );
    }

    #[test]
    fn build_forward_body_includes_intro_headers_and_original_content() {
        let forward = build_forward_body(
            &MailMessage {
                uid: 42,
                subject: Some("Budget".to_string()),
                from: Some("sender@example.com".to_string()),
                to: Some("team@example.com".to_string()),
                cc: Some("copy@example.com".to_string()),
                date: Some("Thu, 12 Mar 2026 20:00:00 +0000".to_string()),
                message_id: Some("<msg-42@example>".to_string()),
                flags: vec!["\\Seen".to_string()],
                size: Some(1024),
                text_body: Some("Original body".to_string()),
                html_body: Some("<p>Original <strong>body</strong></p>".to_string()),
                attachments: vec![MailAttachmentSummary {
                    filename: Some("report.pdf".to_string()),
                    mime_type: "application/pdf".to_string(),
                    content_id: None,
                    inline: false,
                }],
                raw_attachments: vec![MailAttachmentPart {
                    filename: Some("report.pdf".to_string()),
                    mime_type: "application/pdf".to_string(),
                    content_id: None,
                    inline: false,
                    content: b"%PDF-1.4".to_vec(),
                }],
            },
            Some("FYI"),
            None,
        );

        assert!(forward.text.contains("FYI"));
        assert!(
            forward
                .text
                .contains("---------- Forwarded message ----------")
        );
        assert!(forward.text.contains("Subject: Budget"));
        assert!(forward.text.contains("Original body"));

        let html = forward.html.expect("html body");
        assert!(html.contains("<strong>Forwarded message</strong>"));
        assert!(html.contains("Original <strong>body</strong>"));
        assert!(!html.contains("Attachments omitted"));
    }

    #[test]
    fn build_xoauth2_payload_formats_bearer_blob() {
        let payload = build_xoauth2_payload("me@yandex.ru", "token-123");
        assert_eq!(
            payload,
            "user=me@yandex.ru\u{1}auth=Bearer token-123\u{1}\u{1}"
        );
    }

    #[test]
    fn prepare_mail_submission_builds_multipart_message() {
        let prepared = prepare_mail_submission(
            MailSessionAuth::OauthXoauth2 {
                account: "me@yandex.ru".to_string(),
                access_token: "token-123".to_string(),
            },
            MailSendRequest {
                to: vec!["person@example.com".to_string()],
                cc: vec!["copy@example.com".to_string()],
                bcc: vec!["blind@example.com".to_string()],
                subject: "Привет".to_string(),
                text: Some("Первая строка".to_string()),
                html: Some("<p>Привет</p>".to_string()),
                attachments: Vec::new(),
                thread_headers: None,
            },
        )
        .expect("prepared");

        assert_eq!(prepared.sent.from, "me@yandex.ru");
        assert_eq!(prepared.sent.to, vec!["person@example.com"]);
        assert_eq!(prepared.sent.cc, vec!["copy@example.com"]);
        assert_eq!(prepared.sent.bcc_count, 1);
        assert_eq!(prepared.sent.body_kind, "multipart_alternative");
        assert!(prepared.message.contains("Subject: =?UTF-8?B?"));
        assert!(
            prepared
                .message
                .contains("Content-Type: multipart/alternative; boundary=\"")
        );
        assert!(prepared.message.contains("From: me@yandex.ru"));
        assert!(prepared.message.contains("To: person@example.com"));
        assert!(prepared.message.contains("Cc: copy@example.com"));
    }

    #[test]
    fn build_outgoing_message_adds_reply_headers() {
        let message = build_outgoing_message(OutgoingMessage {
            from: "me@yandex.ru",
            to: &[String::from("person@example.com")],
            cc: &[],
            subject: "Re: Hello",
            text: Some("Body"),
            html: None,
            attachments: &[],
            message_id: "msg-2@nextstat.dev",
            thread_headers: Some(&MailThreadHeaders {
                in_reply_to: "<parent@example>".to_string(),
                references: vec!["<root@example>".to_string(), "<parent@example>".to_string()],
            }),
        })
        .expect("message");

        assert!(message.contains("In-Reply-To: <parent@example>"));
        assert!(message.contains("References: <root@example> <parent@example>"));
    }

    #[test]
    fn build_outgoing_message_encodes_text_body_as_base64() {
        let message = build_outgoing_message(OutgoingMessage {
            from: "me@yandex.ru",
            to: &[String::from("person@example.com")],
            cc: &[],
            subject: "Hello",
            text: Some("Привет"),
            html: None,
            attachments: &[],
            message_id: "msg-1@nextstat.dev",
            thread_headers: None,
        })
        .expect("message");

        assert!(message.contains("Content-Type: text/plain; charset=UTF-8"));
        assert!(message.contains("Content-Transfer-Encoding: base64"));
        assert!(message.contains("Message-ID: <msg-1@nextstat.dev>"));
        assert!(message.contains(&STANDARD.encode("Привет".as_bytes())));
    }

    #[test]
    fn smtp_session_sends_xoauth2_mail_flow_and_dot_stuffs_body() {
        let fake = FakeStream::new(
            "220 smtp ready\r\n250-smtp.yandex.com\r\n250 AUTH XOAUTH2 PLAIN\r\n235 2.7.0 ok\r\n250 2.1.0 ok\r\n250 2.1.5 ok\r\n354 End data with <CR><LF>.<CR><LF>\r\n250 2.0.0 queued\r\n221 2.0.0 bye\r\n",
        );
        let observer = fake.clone();
        let mut session = SmtpSession::from_stream(fake);
        session.read_greeting("smtp.yandex.com").expect("greeting");
        session.ehlo("yacli.test").expect("ehlo");
        session
            .authenticate_xoauth2("me@yandex.ru", "token-123")
            .expect("auth");
        session
            .send_message(
                "me@yandex.ru",
                &[String::from("person@example.com")],
                "Subject: Hello\r\n\r\n.line one\r\nline two",
            )
            .expect("send");
        session.quit().expect("quit");

        let writes = observer.written();
        assert!(writes.contains("EHLO yacli.test\r\n"));
        assert!(writes.contains("AUTH XOAUTH2 "));
        assert!(writes.contains("MAIL FROM:<me@yandex.ru>\r\n"));
        assert!(writes.contains("RCPT TO:<person@example.com>\r\n"));
        assert!(writes.contains("DATA\r\n"));
        assert!(writes.contains("\r\n..line one\r\nline two\r\n.\r\n"));
        assert!(writes.ends_with("QUIT\r\n"));
    }

    #[test]
    fn prepare_mail_submission_rejects_missing_body() {
        let error = prepare_mail_submission(
            MailSessionAuth::OauthXoauth2 {
                account: "me@yandex.ru".to_string(),
                access_token: "token-123".to_string(),
            },
            MailSendRequest {
                to: vec!["person@example.com".to_string()],
                cc: Vec::new(),
                bcc: Vec::new(),
                subject: "Hello".to_string(),
                text: None,
                html: None,
                attachments: Vec::new(),
                thread_headers: None,
            },
        )
        .expect_err("missing body");

        assert_eq!(
            error.to_string(),
            "Validation error: mail send requires text or --html"
        );
    }

    #[test]
    fn parse_fetch_metadata_extracts_uid_flags_size_and_literal_length() {
        let metadata = parse_fetch_metadata(
            r#"* 42 FETCH (UID 9001 FLAGS (\Seen \Answered) RFC822.SIZE 1234 BODY[HEADER.FIELDS (SUBJECT FROM DATE)] {77}"#,
        )
        .expect("fetch metadata parsed");

        assert_eq!(metadata.uid, 9001);
        assert_eq!(metadata.flags, vec![r"\Seen", r"\Answered"]);
        assert_eq!(metadata.size, Some(1234));
        assert_eq!(metadata.header_literal_len, Some(77));
    }

    #[test]
    fn build_message_summary_extracts_headers() {
        let metadata = FetchMetadata {
            uid: 42,
            flags: vec![r"\Seen".to_string()],
            size: Some(512),
            header_literal_len: Some(79),
        };
        let summary = build_message_summary(
            metadata,
            b"Subject: =?UTF-8?B?0KLQtdGB0YI=?=\r\nFrom: Example <example@yandex.ru>\r\nDate: Thu, 12 Mar 2026 10:00:00 +0300\r\n\r\n",
        )
        .expect("summary built");

        assert_eq!(summary.uid, 42);
        assert_eq!(summary.subject.as_deref(), Some("Тест"));
        assert_eq!(summary.from.as_deref(), Some("Example <example@yandex.ru>"));
        assert_eq!(
            summary.date.as_deref(),
            Some("Thu, 12 Mar 2026 10:00:00 +0300")
        );
        assert_eq!(summary.flags, vec![r"\Seen"]);
        assert_eq!(summary.size, Some(512));
    }

    #[test]
    fn extract_message_content_collects_text_html_and_attachments() {
        let raw = concat!(
            "Content-Type: multipart/mixed; boundary=\"mix\"\r\n",
            "\r\n",
            "--mix\r\n",
            "Content-Type: multipart/alternative; boundary=\"alt\"\r\n",
            "\r\n",
            "--alt\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n",
            "\r\n",
            "plain body\r\n",
            "--alt\r\n",
            "Content-Type: text/html; charset=utf-8\r\n",
            "\r\n",
            "<p>html body</p>\r\n",
            "--alt--\r\n",
            "--mix\r\n",
            "Content-Type: application/pdf; name=\"bill.pdf\"\r\n",
            "Content-Disposition: attachment; filename=\"bill.pdf\"\r\n",
            "Content-Transfer-Encoding: base64\r\n",
            "\r\n",
            "JVBERi0xLjQ=\r\n",
            "--mix--\r\n",
        );
        let parsed = mailparse::parse_mail(raw.as_bytes()).expect("mail parsed");
        let content = extract_message_content(&parsed).expect("content extracted");

        assert_eq!(content.text_body.as_deref(), Some("plain body"));
        assert_eq!(content.html_body.as_deref(), Some("<p>html body</p>"));
        assert_eq!(content.attachment_parts.len(), 1);
        assert_eq!(
            content.attachment_parts[0].filename.as_deref(),
            Some("bill.pdf")
        );
        assert_eq!(content.attachment_parts[0].mime_type, "application/pdf");
        assert!(!content.attachment_parts[0].inline);
        assert_eq!(content.attachment_parts[0].content, b"%PDF-1.4");
    }

    #[test]
    fn build_read_message_extracts_headers_body_and_attachments() {
        let metadata = FetchMetadata {
            uid: 77,
            flags: vec![r"\Seen".to_string()],
            size: Some(1234),
            header_literal_len: Some(115),
        };
        let headers = concat!(
            "Subject: =?UTF-8?B?0KLQtdGB0YI=?=\r\n",
            "From: Example <example@yandex.ru>\r\n",
            "To: You <you@yandex.ru>\r\n",
            "Cc: Team <team@yandex.ru>\r\n",
            "Date: Thu, 12 Mar 2026 10:00:00 +0300\r\n",
            "Message-ID: <id@example>\r\n",
            "\r\n"
        );
        let raw = concat!(
            "Content-Type: multipart/mixed; boundary=\"mix\"\r\n",
            "\r\n",
            "--mix\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n",
            "\r\n",
            "body text\r\n",
            "--mix\r\n",
            "Content-Type: application/pdf; name=\"invoice.pdf\"\r\n",
            "Content-Disposition: attachment; filename=\"invoice.pdf\"\r\n",
            "Content-Transfer-Encoding: base64\r\n",
            "\r\n",
            "JVBERi0xLjQ=\r\n",
            "--mix--\r\n",
        );

        let message = build_read_message(metadata, headers.as_bytes(), raw.as_bytes())
            .expect("message built");

        assert_eq!(message.uid, 77);
        assert_eq!(message.subject.as_deref(), Some("Тест"));
        assert_eq!(message.from.as_deref(), Some("Example <example@yandex.ru>"));
        assert_eq!(message.to.as_deref(), Some("You <you@yandex.ru>"));
        assert_eq!(message.cc.as_deref(), Some("Team <team@yandex.ru>"));
        assert_eq!(
            message.date.as_deref(),
            Some("Thu, 12 Mar 2026 10:00:00 +0300")
        );
        assert_eq!(message.message_id.as_deref(), Some("<id@example>"));
        assert_eq!(message.text_body.as_deref(), Some("body text"));
        assert_eq!(message.attachments.len(), 1);
        assert_eq!(
            message.attachments[0].filename.as_deref(),
            Some("invoice.pdf")
        );
        assert_eq!(message.raw_attachments.len(), 1);
        assert_eq!(message.raw_attachments[0].content, b"%PDF-1.4");
    }

    #[test]
    fn prepare_mail_submission_builds_multipart_mixed_message_with_attachments() {
        let prepared = prepare_mail_submission(
            MailSessionAuth::OauthXoauth2 {
                account: "me@yandex.ru".to_string(),
                access_token: "token-123".to_string(),
            },
            MailSendRequest {
                to: vec!["person@example.com".to_string()],
                cc: Vec::new(),
                bcc: Vec::new(),
                subject: "Forward".to_string(),
                text: Some("Body".to_string()),
                html: Some("<p>Body</p>".to_string()),
                attachments: vec![MailAttachmentPayload {
                    filename: Some("файл.pdf".to_string()),
                    mime_type: "application/pdf".to_string(),
                    content_id: Some("cid-report".to_string()),
                    inline: false,
                    content: b"%PDF-1.4".to_vec(),
                }],
                thread_headers: None,
            },
        )
        .expect("prepared");

        assert_eq!(prepared.sent.body_kind, "multipart_mixed");
        assert!(
            prepared
                .message
                .contains("Content-Type: multipart/mixed; boundary=\"")
        );
        assert!(
            prepared
                .message
                .contains("Content-Type: multipart/alternative; boundary=\"")
        );
        assert!(prepared.message.contains("Content-Type: application/pdf; "));
        assert!(
            prepared
                .message
                .contains("Content-Disposition: attachment; ")
        );
        assert!(
            prepared
                .message
                .contains("filename*=UTF-8''%D1%84%D0%B0%D0%B9%D0%BB.pdf")
        );
        assert!(prepared.message.contains("Content-ID: <cid-report>"));
        assert!(prepared.message.contains(&STANDARD.encode(b"%PDF-1.4")));
    }

    #[test]
    fn validate_attachment_export_request_rejects_zero_index() {
        let error = validate_mail_attachment_export_request(&MailAttachmentExportRequest {
            uid: 42,
            selector: MailAttachmentSelector::Index(0),
            output: PathBuf::from("invoice.pdf"),
            force: false,
            max_bytes: 1024,
        })
        .expect_err("zero index rejected");

        assert_eq!(
            error.to_string(),
            "Validation error: mail attachment export --index must be greater than zero"
        );
    }

    #[test]
    fn validate_invite_inspect_request_rejects_zero_index() {
        let error = validate_mail_attachment_read_request(
            42,
            &MailAttachmentSelector::Index(0),
            1024,
            "mail invite inspect",
        )
        .expect_err("zero index rejected");

        assert_eq!(
            error.to_string(),
            "Validation error: mail invite inspect --index must be greater than zero"
        );
    }

    #[test]
    fn export_attachment_from_message_writes_selected_attachment_by_index() {
        let temp = tempdir().expect("tempdir");
        let output = temp.path().join("invoice.pdf");
        let message = MailMessage {
            uid: 42,
            subject: Some("Invoice".to_string()),
            from: Some("sender@example.com".to_string()),
            to: None,
            cc: None,
            date: None,
            message_id: None,
            flags: Vec::new(),
            size: Some(512),
            text_body: Some("Body".to_string()),
            html_body: None,
            attachments: vec![MailAttachmentSummary {
                filename: Some("invoice.pdf".to_string()),
                mime_type: "application/pdf".to_string(),
                content_id: None,
                inline: false,
            }],
            raw_attachments: vec![MailAttachmentPart {
                filename: Some("invoice.pdf".to_string()),
                mime_type: "application/pdf".to_string(),
                content_id: None,
                inline: false,
                content: b"%PDF-1.4".to_vec(),
            }],
        };

        let exported = export_attachment_from_message(
            &message,
            &MailAttachmentSelector::Index(1),
            &output,
            false,
        )
        .expect("attachment exported");

        assert_eq!(exported.message_uid, 42);
        assert_eq!(exported.attachment_index, 1);
        assert_eq!(exported.filename.as_deref(), Some("invoice.pdf"));
        assert_eq!(exported.bytes_written, 8);
        assert_eq!(std::fs::read(&output).expect("exported file"), b"%PDF-1.4");
        assert_eq!(exported.output_path, output.display().to_string());
        assert_eq!(exported.sha256.len(), 64);
    }

    #[test]
    fn export_attachment_from_message_rejects_ambiguous_filename() {
        let temp = tempdir().expect("tempdir");
        let output = temp.path().join("invoice.pdf");
        let message = MailMessage {
            uid: 42,
            subject: Some("Invoice".to_string()),
            from: Some("sender@example.com".to_string()),
            to: None,
            cc: None,
            date: None,
            message_id: None,
            flags: Vec::new(),
            size: Some(1024),
            text_body: Some("Body".to_string()),
            html_body: None,
            attachments: vec![
                MailAttachmentSummary {
                    filename: Some("invoice.pdf".to_string()),
                    mime_type: "application/pdf".to_string(),
                    content_id: None,
                    inline: false,
                },
                MailAttachmentSummary {
                    filename: Some("invoice.pdf".to_string()),
                    mime_type: "application/pdf".to_string(),
                    content_id: None,
                    inline: false,
                },
            ],
            raw_attachments: vec![
                MailAttachmentPart {
                    filename: Some("invoice.pdf".to_string()),
                    mime_type: "application/pdf".to_string(),
                    content_id: None,
                    inline: false,
                    content: b"one".to_vec(),
                },
                MailAttachmentPart {
                    filename: Some("invoice.pdf".to_string()),
                    mime_type: "application/pdf".to_string(),
                    content_id: None,
                    inline: false,
                    content: b"two".to_vec(),
                },
            ],
        };

        let error = export_attachment_from_message(
            &message,
            &MailAttachmentSelector::Filename("invoice.pdf".to_string()),
            &output,
            false,
        )
        .expect_err("ambiguous attachment name rejected");

        assert_eq!(
            error.to_string(),
            "Unsupported operation: mail attachment export attachment name `invoice.pdf` is ambiguous; use --index"
        );
    }

    #[test]
    fn export_attachment_from_message_refuses_to_overwrite_without_force() {
        let temp = tempdir().expect("tempdir");
        let output = temp.path().join("invoice.pdf");
        std::fs::write(&output, b"existing").expect("existing file");
        let message = MailMessage {
            uid: 42,
            subject: Some("Invoice".to_string()),
            from: Some("sender@example.com".to_string()),
            to: None,
            cc: None,
            date: None,
            message_id: None,
            flags: Vec::new(),
            size: Some(512),
            text_body: Some("Body".to_string()),
            html_body: None,
            attachments: vec![MailAttachmentSummary {
                filename: Some("invoice.pdf".to_string()),
                mime_type: "application/pdf".to_string(),
                content_id: None,
                inline: false,
            }],
            raw_attachments: vec![MailAttachmentPart {
                filename: Some("invoice.pdf".to_string()),
                mime_type: "application/pdf".to_string(),
                content_id: None,
                inline: false,
                content: b"%PDF-1.4".to_vec(),
            }],
        };

        let error = export_attachment_from_message(
            &message,
            &MailAttachmentSelector::Index(1),
            &output,
            false,
        )
        .expect_err("overwrite rejected");

        assert_eq!(
            error.to_string(),
            format!("Output already exists: {}", output.display())
        );
        assert_eq!(std::fs::read(&output).expect("existing file"), b"existing");
    }

    #[test]
    fn load_mail_attachments_reads_file_and_infers_mime_type() {
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("note.txt");
        std::fs::write(&path, b"hello world").expect("attachment file");

        let attachments =
            load_mail_attachments(std::slice::from_ref(&path)).expect("attachments loaded");

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].filename.as_deref(), Some("note.txt"));
        assert_eq!(attachments[0].mime_type, "text/plain");
        assert_eq!(attachments[0].content, b"hello world");
        assert!(!attachments[0].inline);
    }

    #[test]
    fn load_mail_attachments_rejects_directory_path() {
        let temp = tempdir().expect("tempdir");
        let error = load_mail_attachments(&[temp.path().to_path_buf()])
            .expect_err("directory attachment rejected");

        assert_eq!(
            error.to_string(),
            format!(
                "Unsupported operation: attachment path points to a directory: {}",
                temp.path().display()
            )
        );
    }

    #[test]
    fn extract_message_content_collects_text_calendar_as_attachment() {
        let raw = concat!(
            "Content-Type: multipart/alternative; boundary=\"alt\"\r\n",
            "\r\n",
            "--alt\r\n",
            "Content-Type: text/plain; charset=utf-8\r\n",
            "\r\n",
            "plain body\r\n",
            "--alt\r\n",
            "Content-Type: text/calendar; charset=utf-8\r\n",
            "\r\n",
            "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:evt-1\r\nSUMMARY:Sync\r\nDTSTART:20260312T090000Z\r\nDTEND:20260312T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
            "--alt--\r\n",
        );
        let parsed = mailparse::parse_mail(raw.as_bytes()).expect("mail parsed");
        let content = extract_message_content(&parsed).expect("content extracted");

        assert_eq!(content.text_body.as_deref(), Some("plain body"));
        assert_eq!(content.attachment_parts.len(), 1);
        assert_eq!(content.attachment_parts[0].mime_type, "text/calendar");
    }

    #[test]
    fn inspect_invite_from_message_reads_calendar_attachment() {
        let message = MailMessage {
            uid: 42,
            subject: Some("Invite".to_string()),
            from: Some("sender@example.com".to_string()),
            to: None,
            cc: None,
            date: None,
            message_id: None,
            flags: Vec::new(),
            size: Some(512),
            text_body: Some("Body".to_string()),
            html_body: None,
            attachments: vec![MailAttachmentSummary {
                filename: Some("invite.ics".to_string()),
                mime_type: "text/calendar".to_string(),
                content_id: None,
                inline: false,
            }],
            raw_attachments: vec![MailAttachmentPart {
                filename: Some("invite.ics".to_string()),
                mime_type: "text/calendar".to_string(),
                content_id: None,
                inline: false,
                content: b"BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:evt-1\r\nSUMMARY:Sync\r\nDTSTART:20260312T090000Z\r\nDTEND:20260312T100000Z\r\nEND:VEVENT\r\nEND:VCALENDAR".to_vec(),
            }],
        };

        let inspected = inspect_invite_from_message(&message, &MailAttachmentSelector::Index(1))
            .expect("invite inspected");

        assert_eq!(inspected.message_uid, 42);
        assert_eq!(inspected.attachment_index, 1);
        assert_eq!(inspected.filename.as_deref(), Some("invite.ics"));
        assert_eq!(inspected.invites.len(), 1);
        assert_eq!(inspected.invites[0].uid.as_deref(), Some("evt-1"));
        assert_eq!(inspected.invites[0].summary.as_deref(), Some("Sync"));
    }
}
