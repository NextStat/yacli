use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value, json};

use crate::disk::{
    DiskResource, DiskUploadReview, PrivateDiskPublishRequest, PrivateDiskUploadRequest,
    UploadedFile, publish_private_resource, review_private_upload, upload_private_resource,
};
use crate::error::{Result, YacliError};
use crate::mail::{
    MailSendRequest, MailSendReview, MailSessionAuth, SentMail, review_mail_submission,
    send_mail_message,
};

const PUBLIC_LINK_PLACEHOLDER: &str = "<публичная ссылка на файл>";

#[derive(Clone, Debug)]
pub struct MailSendLinkRequest {
    pub source: PathBuf,
    pub disk_path: String,
    pub overwrite: bool,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MailSendPublishedLinkRequest {
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub bcc: Vec<String>,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
    pub public_url: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MailSendLinkReview {
    pub upload: DiskUploadReview,
    pub mail_review: MailSendReview,
    pub link_placeholder: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MailSendPublishedLinkReview {
    pub mail_review: MailSendReview,
    pub public_url: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MailSendLinkResult {
    pub upload: UploadedFile,
    pub resource: DiskResource,
    pub sent: SentMail,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MailSendLinkOutcome {
    Sent { result: MailSendLinkResult },
    Partial { partial: MailSendLinkPartialFailure },
}

#[derive(Clone, Debug, Serialize)]
pub struct MailSendLinkPartialFailure {
    pub upload: UploadedFile,
    pub resource: DiskResource,
    pub failed_stage: String,
    pub error: MailSendLinkPartialError,
    pub recovery: MailSendLinkRecovery,
}

#[derive(Clone, Debug, Serialize)]
pub struct MailSendLinkPartialError {
    pub code: &'static str,
    pub message: String,
    pub exit_code: i32,
}

#[derive(Clone, Debug, Serialize)]
pub struct MailSendLinkRecovery {
    pub share_public_link: SharePublicLinkRecovery,
    pub retry_mail_step: RetryMailStepRecovery,
    pub cleanup_public_link: CleanupPublicLinkRecovery,
}

#[derive(Clone, Debug, Serialize)]
pub struct SharePublicLinkRecovery {
    pub public_url: String,
    pub public_key: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RetryMailStepRecovery {
    pub tool: String,
    pub command: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct CleanupPublicLinkRecovery {
    pub tool: String,
    pub command: String,
    pub arguments: Value,
}

pub fn review_mail_send_link(
    auth: MailSessionAuth,
    request: &MailSendLinkRequest,
) -> Result<MailSendLinkReview> {
    let upload = review_private_upload(&PrivateDiskUploadRequest {
        source: request.source.clone(),
        path: request.disk_path.clone(),
        overwrite: request.overwrite,
    })?;
    let mail_review = review_mail_submission(
        auth,
        MailSendRequest {
            to: request.to.clone(),
            cc: request.cc.clone(),
            bcc: request.bcc.clone(),
            subject: request.subject.clone(),
            text: compose_text_body(
                request.text.as_deref(),
                request.html.as_deref(),
                PUBLIC_LINK_PLACEHOLDER,
            ),
            html: compose_html_body(
                request.text.as_deref(),
                request.html.as_deref(),
                PUBLIC_LINK_PLACEHOLDER,
            ),
            attachments: Vec::new(),
            thread_headers: None,
        },
    )?;

    Ok(MailSendLinkReview {
        upload,
        mail_review,
        link_placeholder: PUBLIC_LINK_PLACEHOLDER.to_string(),
    })
}

pub fn review_mail_send_published_link(
    auth: MailSessionAuth,
    request: &MailSendPublishedLinkRequest,
) -> Result<MailSendPublishedLinkReview> {
    let mail_review = review_mail_submission(
        auth,
        MailSendRequest {
            to: request.to.clone(),
            cc: request.cc.clone(),
            bcc: request.bcc.clone(),
            subject: request.subject.clone(),
            text: compose_text_body(
                request.text.as_deref(),
                request.html.as_deref(),
                &request.public_url,
            ),
            html: compose_html_body(
                request.text.as_deref(),
                request.html.as_deref(),
                &request.public_url,
            ),
            attachments: Vec::new(),
            thread_headers: None,
        },
    )?;

    Ok(MailSendPublishedLinkReview {
        mail_review,
        public_url: request.public_url.clone(),
    })
}

pub fn send_published_link_via_mail(
    smtp_host: &str,
    smtp_port: u16,
    auth: MailSessionAuth,
    request: &MailSendPublishedLinkRequest,
) -> Result<SentMail> {
    let send_request = MailSendRequest {
        to: request.to.clone(),
        cc: request.cc.clone(),
        bcc: request.bcc.clone(),
        subject: request.subject.clone(),
        text: compose_text_body(
            request.text.as_deref(),
            request.html.as_deref(),
            &request.public_url,
        ),
        html: compose_html_body(
            request.text.as_deref(),
            request.html.as_deref(),
            &request.public_url,
        ),
        attachments: Vec::new(),
        thread_headers: None,
    };

    send_mail_message(smtp_host, smtp_port, auth, send_request)
}

pub fn send_link_via_mail(
    disk_base_url: &str,
    access_token: &str,
    smtp_host: &str,
    smtp_port: u16,
    auth: MailSessionAuth,
    request: &MailSendLinkRequest,
) -> Result<MailSendLinkOutcome> {
    let (_, upload) = upload_private_resource(
        disk_base_url,
        access_token,
        &PrivateDiskUploadRequest {
            source: request.source.clone(),
            path: request.disk_path.clone(),
            overwrite: request.overwrite,
        },
    )?;
    let resource = publish_private_resource(
        disk_base_url,
        access_token,
        &PrivateDiskPublishRequest {
            path: request.disk_path.clone(),
        },
    )?;
    let public_url = resource.public_url.as_deref().ok_or_else(|| {
        YacliError::Api(format!(
            "Yandex Disk did not return `public_url` after publishing `{}`",
            resource.path
        ))
    })?;

    let send_request = MailSendPublishedLinkRequest {
        to: request.to.clone(),
        cc: request.cc.clone(),
        bcc: request.bcc.clone(),
        subject: request.subject.clone(),
        text: request.text.clone(),
        html: request.html.clone(),
        public_url: public_url.to_string(),
    };

    let sent = match send_published_link_via_mail(smtp_host, smtp_port, auth, &send_request) {
        Ok(sent) => sent,
        Err(err) => {
            return Ok(MailSendLinkOutcome::Partial {
                partial: partial_failure(&upload, &resource, &send_request, err),
            });
        }
    };

    Ok(MailSendLinkOutcome::Sent {
        result: MailSendLinkResult {
            upload,
            resource,
            sent,
        },
    })
}

fn compose_text_body(text: Option<&str>, html: Option<&str>, public_url: &str) -> Option<String> {
    match (text, html) {
        (Some(text), _) => Some(format!("{text}\n\nСсылка на файл: {public_url}")),
        (None, Some(_)) => None,
        (None, None) => Some(format!("Отправляю ссылку на файл:\n{public_url}")),
    }
}

fn compose_html_body(text: Option<&str>, html: Option<&str>, public_url: &str) -> Option<String> {
    let link_html = format!(
        "<p>Ссылка на файл: <a href=\"{0}\">{0}</a></p>",
        escape_html(public_url)
    );
    match (text, html) {
        (_, Some(html)) => Some(format!("{html}\n{link_html}")),
        (Some(text), None) => Some(format!(
            "<p>{}</p>\n{}",
            escape_html(text).replace('\n', "<br>\n"),
            link_html
        )),
        (None, None) => None,
    }
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn partial_failure(
    upload: &UploadedFile,
    resource: &DiskResource,
    request: &MailSendPublishedLinkRequest,
    err: YacliError,
) -> MailSendLinkPartialFailure {
    let public_url = resource.public_url.as_deref().unwrap_or("-").to_string();
    MailSendLinkPartialFailure {
        upload: upload.clone(),
        resource: resource.clone(),
        failed_stage: "mail_send".to_string(),
        error: MailSendLinkPartialError {
            code: err.code(),
            message: err.to_string(),
            exit_code: err.exit_code(),
        },
        recovery: MailSendLinkRecovery {
            share_public_link: SharePublicLinkRecovery {
                public_url: public_url.clone(),
                public_key: resource.public_key.clone(),
            },
            retry_mail_step: RetryMailStepRecovery {
                tool: "yacli.mail.send_published_link".to_string(),
                command: send_published_link_command(request, true),
                arguments: json!({
                    "to": request.to.first().cloned().unwrap_or_default(),
                    "cc": request.cc,
                    "bcc": request.bcc,
                    "subject": request.subject,
                    "text": request.text,
                    "html": request.html,
                    "public_url": request.public_url,
                    "dry_run": true,
                }),
            },
            cleanup_public_link: CleanupPublicLinkRecovery {
                tool: "yacli.disk.unpublish".to_string(),
                command: format!(
                    "yacli disk unpublish {} --dry-run",
                    shell_quote(&resource.path)
                ),
                arguments: json!({
                    "path": resource.path,
                    "dry_run": true,
                }),
            },
        },
    }
}

pub fn send_published_link_command(
    request: &MailSendPublishedLinkRequest,
    dry_run: bool,
) -> String {
    let mut command = format!(
        "yacli mail send-published-link {} {}",
        shell_quote(request.to.first().map(String::as_str).unwrap_or("")),
        shell_quote(&request.subject)
    );
    if let Some(text) = request.text.as_deref() {
        command.push(' ');
        command.push_str(&shell_quote(text));
    }
    if !request.cc.is_empty() {
        for cc in &request.cc {
            command.push_str(" --cc ");
            command.push_str(&shell_quote(cc));
        }
    }
    if !request.bcc.is_empty() {
        for bcc in &request.bcc {
            command.push_str(" --bcc ");
            command.push_str(&shell_quote(bcc));
        }
    }
    if let Some(html) = request.html.as_deref() {
        command.push_str(" --html ");
        command.push_str(&shell_quote(html));
    }
    command.push_str(" --public-url ");
    command.push_str(&shell_quote(&request.public_url));
    if dry_run {
        command.push_str(" --dry-run");
    }
    command
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '-' | '.' | ':' | '@'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mail::MailSessionAuth;
    use tempfile::tempdir;

    #[test]
    fn review_mail_send_link_uses_placeholder_link_without_network() {
        let temp = tempdir().expect("tempdir");
        let source = temp.path().join("report.txt");
        std::fs::write(&source, "report body").expect("source");

        let review = review_mail_send_link(
            MailSessionAuth::OauthXoauth2 {
                account: "me@yandex.ru".to_string(),
                access_token: "token".to_string(),
            },
            &MailSendLinkRequest {
                source,
                disk_path: "disk:/docs/report.txt".to_string(),
                overwrite: false,
                to: vec!["person@example.com".to_string()],
                cc: Vec::new(),
                bcc: Vec::new(),
                subject: "Отчёт".to_string(),
                text: Some("Отправляю ссылку".to_string()),
                html: None,
            },
        )
        .expect("review");

        assert_eq!(review.upload.remote_path, "disk:/docs/report.txt");
        assert_eq!(review.link_placeholder, PUBLIC_LINK_PLACEHOLDER);
        assert_eq!(review.mail_review.sent.subject, "Отчёт");
        assert_eq!(review.mail_review.attachment_count, 0);
    }

    #[test]
    fn partial_failure_keeps_public_url_context_and_recovery_actions() {
        let resource = DiskResource {
            name: "report.txt".to_string(),
            path: "disk:/docs/report.txt".to_string(),
            resource_type: "file".to_string(),
            mime_type: None,
            size: None,
            created: None,
            modified: None,
            md5: None,
            revision: None,
            public_url: Some("https://disk.yandex.ru/i/report".to_string()),
            public_key: Some("key".to_string()),
            children: None,
        };
        let upload = UploadedFile {
            source_path: "/tmp/report.txt".to_string(),
            remote_path: "disk:/docs/report.txt".to_string(),
            overwrite: false,
            bytes_written: 12,
            sha256: "abc".to_string(),
            attempts: 1,
            elapsed_ms: 10,
        };
        let partial = partial_failure(
            &upload,
            &resource,
            &MailSendPublishedLinkRequest {
                to: vec!["person@example.com".to_string()],
                cc: Vec::new(),
                bcc: Vec::new(),
                subject: "Отчёт".to_string(),
                text: Some("Отправляю ссылку".to_string()),
                html: None,
                public_url: "https://disk.yandex.ru/i/report".to_string(),
            },
            YacliError::Network("smtp failed".to_string()),
        );
        assert_eq!(partial.error.code, "NETWORK_ERROR");
        assert_eq!(
            partial.recovery.share_public_link.public_url,
            "https://disk.yandex.ru/i/report"
        );
        assert_eq!(
            partial.recovery.cleanup_public_link.tool,
            "yacli.disk.unpublish"
        );
    }
}
