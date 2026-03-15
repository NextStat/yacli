use std::path::PathBuf;

use serde::Serialize;

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
pub struct MailSendLinkReview {
    pub upload: DiskUploadReview,
    pub mail_review: MailSendReview,
    pub link_placeholder: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct MailSendLinkResult {
    pub upload: UploadedFile,
    pub resource: DiskResource,
    pub sent: SentMail,
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

pub fn send_link_via_mail(
    disk_base_url: &str,
    access_token: &str,
    smtp_host: &str,
    smtp_port: u16,
    auth: MailSessionAuth,
    request: &MailSendLinkRequest,
) -> Result<MailSendLinkResult> {
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

    let send_request = MailSendRequest {
        to: request.to.clone(),
        cc: request.cc.clone(),
        bcc: request.bcc.clone(),
        subject: request.subject.clone(),
        text: compose_text_body(request.text.as_deref(), request.html.as_deref(), public_url),
        html: compose_html_body(request.text.as_deref(), request.html.as_deref(), public_url),
        attachments: Vec::new(),
        thread_headers: None,
    };

    let sent = send_mail_message(smtp_host, smtp_port, auth, send_request)
        .map_err(|err| wrap_partial_failure(err, &resource))?;

    Ok(MailSendLinkResult {
        upload,
        resource,
        sent,
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

fn wrap_partial_failure(err: YacliError, resource: &DiskResource) -> YacliError {
    let message = format!(
        "mail send-link already uploaded and published `{}`; public_url: {}; sending email then failed: {}",
        resource.path,
        resource.public_url.as_deref().unwrap_or("-"),
        err
    );
    match err {
        YacliError::AccountExists(_) => YacliError::AccountExists(message),
        YacliError::AccountNotFound(_) => YacliError::AccountNotFound(message),
        YacliError::CurrentAccountMissing => YacliError::Config(message),
        YacliError::Validation(_) => YacliError::Validation(message),
        YacliError::Config(_) => YacliError::Config(message),
        YacliError::Io(_) => YacliError::Io(message),
        YacliError::Serialization(_) => YacliError::Serialization(message),
        YacliError::Network(_) => YacliError::Network(message),
        YacliError::Api(_) => YacliError::Api(message),
        YacliError::Auth(_) => YacliError::Auth(message),
        YacliError::OutputExists(_) => YacliError::OutputExists(message),
        YacliError::UnsupportedOperation(_) => YacliError::UnsupportedOperation(message),
        YacliError::Integrity(_) => YacliError::Integrity(message),
    }
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
    fn wrap_partial_failure_keeps_public_url_context() {
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
        let err = wrap_partial_failure(YacliError::Network("smtp failed".to_string()), &resource);
        assert!(
            err.to_string()
                .contains("public_url: https://disk.yandex.ru/i/report")
        );
    }
}
