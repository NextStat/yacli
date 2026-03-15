use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::time::Instant;
use std::{env, str::FromStr};

use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::error::{Result, YacliError};

pub const DEFAULT_DISK_BASE_URL: &str = "https://cloud-api.yandex.net";
const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 20;
const DISK_UPLOAD_TIMEOUT_ENV: &str = "YACLI_DISK_UPLOAD_TIMEOUT_SECS";
const DISK_DOWNLOAD_TIMEOUT_ENV: &str = "YACLI_DISK_DOWNLOAD_TIMEOUT_SECS";
const TRANSFER_PROGRESS_INTERVAL_MS: u128 = 250;
const TRANSFER_RETRY_ATTEMPTS: usize = 3;
const TRANSFER_RETRY_BACKOFF_MS: u64 = 400;
type TransferProgressCallback = Arc<Mutex<Box<dyn FnMut(TransferProgress) + Send>>>;

#[derive(Clone, Debug)]
pub struct TransferProgress {
    pub transferred_bytes: u64,
    pub total_bytes: Option<u64>,
    pub bytes_per_second: f64,
    pub finished: bool,
}

#[derive(Debug)]
pub struct PublicDiskRequest {
    pub public_key: String,
    pub path: Option<String>,
}

#[derive(Debug)]
pub struct PublicDownloadRequest {
    pub public_key: String,
    pub path: Option<String>,
    pub output: PathBuf,
    pub force: bool,
}

#[derive(Debug)]
pub struct PrivateDiskDownloadRequest {
    pub path: String,
    pub output: PathBuf,
    pub force: bool,
}

#[derive(Debug)]
pub struct PrivateDiskPublishRequest {
    pub path: String,
}

#[derive(Debug)]
pub struct PrivateDiskUnpublishRequest {
    pub path: String,
}

#[derive(Debug)]
pub struct PrivateDiskListRequest {
    pub path: String,
    pub limit: usize,
    pub offset: u64,
}

#[derive(Debug)]
pub struct PrivateDiskMkdirRequest {
    pub path: String,
}

#[derive(Debug)]
pub struct PrivateDiskUploadRequest {
    pub source: PathBuf,
    pub path: String,
    pub overwrite: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskUploadReview {
    pub source_path: String,
    pub remote_path: String,
    pub bytes_written: u64,
    pub sha256: String,
    pub overwrite: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskPublishReview {
    pub path: String,
    pub resource_name: String,
    pub resource_type: String,
    pub already_public: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_public_key: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskUnpublishReview {
    pub path: String,
    pub resource_name: String,
    pub resource_type: String,
    pub is_public: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_public_key: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PublicResource {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub resource_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<PublicResourceChildren>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PublicResourceChildren {
    pub limit: u64,
    pub offset: u64,
    pub total: u64,
    pub items: Vec<PublicResourceItem>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PublicResourceItem {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub resource_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DownloadedFile {
    pub output_path: String,
    pub bytes_written: u64,
    pub sha256: String,
    pub resumed_from_bytes: u64,
    pub attempts: usize,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct UploadedFile {
    pub source_path: String,
    pub remote_path: String,
    pub bytes_written: u64,
    pub sha256: String,
    pub overwrite: bool,
    pub attempts: usize,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct UnpublishedDiskResource {
    pub resource: DiskResource,
    pub was_public: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_public_key: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskInfo {
    pub total_space: u64,
    pub used_space: u64,
    pub trash_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_file_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paid_max_file_size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<DiskUser>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_paid: Option<bool>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskUser {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskResource {
    pub name: String,
    pub path: String,
    pub resource_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<DiskResourceChildren>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskResourceChildren {
    pub limit: u64,
    pub offset: u64,
    pub total: u64,
    pub items: Vec<DiskResourceItem>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DiskResourceItem {
    pub name: String,
    pub path: String,
    pub resource_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
}

pub fn fetch_public_resource(
    base_url: &str,
    request: &PublicDiskRequest,
) -> Result<PublicResource> {
    let client = build_http_client()?;
    fetch_public_resource_with_client(&client, base_url, request)
}

pub fn download_public_resource(
    base_url: &str,
    request: &PublicDownloadRequest,
) -> Result<(PublicResource, DownloadedFile)> {
    download_public_resource_with_progress(base_url, request, None)
}

pub fn download_public_resource_with_progress(
    base_url: &str,
    request: &PublicDownloadRequest,
    progress: Option<Box<dyn FnMut(TransferProgress) + Send>>,
) -> Result<(PublicResource, DownloadedFile)> {
    let progress = progress.map(|callback| Arc::new(Mutex::new(callback)));
    let metadata_client = build_http_client()?;
    let resource = fetch_public_resource_with_client(
        &metadata_client,
        base_url,
        &PublicDiskRequest {
            public_key: request.public_key.clone(),
            path: request.path.clone(),
        },
    )?;

    if resource.resource_type != "file" {
        return Err(YacliError::UnsupportedOperation(format!(
            "public download requires a file resource, got {}",
            resource.resource_type
        )));
    }

    let download_url = resource.download_url.clone().ok_or_else(|| {
        YacliError::Api(
            "public resource does not expose a download URL; downloads may be disabled".to_string(),
        )
    })?;

    let download_timeout = configured_download_timeout()?;
    let download_client = build_transfer_http_client(download_timeout)?;
    let transfer = retry_transfer(|_| {
        download_to_path(
            &download_client,
            &download_url,
            &request.output,
            request.force,
            resource.size,
            progress.clone(),
        )
    })?;

    Ok((resource, transfer))
}

pub fn fetch_disk_info(base_url: &str, access_token: &str) -> Result<DiskInfo> {
    let client = build_http_client()?;
    let endpoint = disk_info_endpoint(base_url)?;
    let response = client
        .get(endpoint)
        .header("Authorization", format!("OAuth {access_token}"))
        .send()?;
    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(provider_error(status, &body, "private metadata", "info"));
    }

    let raw = serde_json::from_str::<RawDiskInfo>(&body)
        .map_err(|err| YacliError::Serialization(format!("invalid provider response: {err}")))?;
    Ok(raw.into_disk_info())
}

pub fn fetch_private_resource(
    base_url: &str,
    access_token: &str,
    request: &PrivateDiskListRequest,
) -> Result<DiskResource> {
    if request.limit == 0 {
        return Err(YacliError::Validation(
            "disk list --limit должен быть больше нуля".to_string(),
        ));
    }
    if request.path.trim().is_empty() {
        return Err(YacliError::Validation(
            "disk list [PATH] не должен быть пустым".to_string(),
        ));
    }

    let client = build_http_client()?;
    let endpoint = disk_resources_endpoint(base_url)?;
    let response = client
        .get(endpoint)
        .header("Authorization", format!("OAuth {access_token}"))
        .query(&[
            ("path", request.path.as_str()),
            ("limit", &request.limit.to_string()),
            ("offset", &request.offset.to_string()),
        ])
        .send()?;
    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(provider_error(status, &body, "private metadata", "browse"));
    }

    let raw = serde_json::from_str::<RawDiskResource>(&body)
        .map_err(|err| YacliError::Serialization(format!("invalid provider response: {err}")))?;
    Ok(raw.into_disk_resource())
}

pub fn create_private_directory(
    base_url: &str,
    access_token: &str,
    request: &PrivateDiskMkdirRequest,
) -> Result<DiskResource> {
    let path = request.path.trim();
    if path.is_empty() {
        return Err(YacliError::Validation(
            "disk mkdir <PATH> не должен быть пустым".to_string(),
        ));
    }

    let client = build_http_client()?;
    let endpoint = disk_resources_endpoint(base_url)?;
    let response = client
        .put(endpoint)
        .header("Authorization", format!("OAuth {access_token}"))
        .query(&[("path", path)])
        .send()?;
    let status = response.status();
    let body = response.text()?;

    if !matches!(status, StatusCode::CREATED | StatusCode::ACCEPTED) {
        return Err(provider_error(status, &body, "private directory", "create"));
    }

    fetch_private_resource(
        base_url,
        access_token,
        &PrivateDiskListRequest {
            path: path.to_string(),
            limit: 100,
            offset: 0,
        },
    )
}

pub fn publish_private_resource(
    base_url: &str,
    access_token: &str,
    request: &PrivateDiskPublishRequest,
) -> Result<DiskResource> {
    let path = request.path.trim();
    if path.is_empty() {
        return Err(YacliError::Validation(
            "disk publish <PATH> не должен быть пустым".to_string(),
        ));
    }

    let client = build_http_client()?;
    let endpoint = disk_publish_endpoint(base_url)?;
    let response = client
        .put(endpoint)
        .header("Authorization", format!("OAuth {access_token}"))
        .query(&[("path", path)])
        .send()?;
    let status = response.status();
    let body = response.text()?;

    if !matches!(
        status,
        StatusCode::OK | StatusCode::CREATED | StatusCode::ACCEPTED
    ) {
        return Err(provider_error(status, &body, "private resource", "publish"));
    }

    fetch_private_resource(
        base_url,
        access_token,
        &PrivateDiskListRequest {
            path: path.to_string(),
            limit: 100,
            offset: 0,
        },
    )
}

pub fn review_private_publish(
    base_url: &str,
    access_token: &str,
    request: &PrivateDiskPublishRequest,
) -> Result<DiskPublishReview> {
    let path = request.path.trim();
    if path.is_empty() {
        return Err(YacliError::Validation(
            "disk publish <PATH> не должен быть пустым".to_string(),
        ));
    }

    let resource = fetch_private_resource(
        base_url,
        access_token,
        &PrivateDiskListRequest {
            path: path.to_string(),
            limit: 100,
            offset: 0,
        },
    )?;

    Ok(DiskPublishReview {
        path: resource.path.clone(),
        resource_name: resource.name.clone(),
        resource_type: resource.resource_type.clone(),
        already_public: resource.public_url.is_some() || resource.public_key.is_some(),
        current_public_url: resource.public_url.clone(),
        current_public_key: resource.public_key.clone(),
    })
}

pub fn unpublish_private_resource(
    base_url: &str,
    access_token: &str,
    request: &PrivateDiskUnpublishRequest,
) -> Result<UnpublishedDiskResource> {
    let path = request.path.trim();
    if path.is_empty() {
        return Err(YacliError::Validation(
            "disk unpublish <PATH> не должен быть пустым".to_string(),
        ));
    }

    let before = fetch_private_resource(
        base_url,
        access_token,
        &PrivateDiskListRequest {
            path: path.to_string(),
            limit: 100,
            offset: 0,
        },
    )
    .ok();

    let client = build_http_client()?;
    let endpoint = disk_unpublish_endpoint(base_url)?;
    let response = client
        .put(endpoint)
        .header("Authorization", format!("OAuth {access_token}"))
        .query(&[("path", path)])
        .send()?;
    let status = response.status();
    let body = response.text()?;

    if !matches!(
        status,
        StatusCode::OK | StatusCode::CREATED | StatusCode::ACCEPTED
    ) {
        return Err(provider_error(
            status,
            &body,
            "private resource",
            "unpublish",
        ));
    }

    let resource = fetch_private_resource(
        base_url,
        access_token,
        &PrivateDiskListRequest {
            path: path.to_string(),
            limit: 100,
            offset: 0,
        },
    )?;

    Ok(UnpublishedDiskResource {
        was_public: before
            .as_ref()
            .map(|item| item.public_url.is_some() || item.public_key.is_some())
            .unwrap_or(false),
        revoked_public_url: before.as_ref().and_then(|item| item.public_url.clone()),
        revoked_public_key: before.as_ref().and_then(|item| item.public_key.clone()),
        resource,
    })
}

pub fn review_private_unpublish(
    base_url: &str,
    access_token: &str,
    request: &PrivateDiskUnpublishRequest,
) -> Result<DiskUnpublishReview> {
    let path = request.path.trim();
    if path.is_empty() {
        return Err(YacliError::Validation(
            "disk unpublish <PATH> не должен быть пустым".to_string(),
        ));
    }

    let resource = fetch_private_resource(
        base_url,
        access_token,
        &PrivateDiskListRequest {
            path: path.to_string(),
            limit: 100,
            offset: 0,
        },
    )?;

    Ok(DiskUnpublishReview {
        path: resource.path.clone(),
        resource_name: resource.name.clone(),
        resource_type: resource.resource_type.clone(),
        is_public: resource.public_url.is_some() || resource.public_key.is_some(),
        current_public_url: resource.public_url.clone(),
        current_public_key: resource.public_key.clone(),
    })
}

pub fn upload_private_resource(
    base_url: &str,
    access_token: &str,
    request: &PrivateDiskUploadRequest,
) -> Result<(DiskResource, UploadedFile)> {
    upload_private_resource_with_progress(base_url, access_token, request, None)
}

pub fn upload_private_resource_with_progress(
    base_url: &str,
    access_token: &str,
    request: &PrivateDiskUploadRequest,
    progress: Option<Box<dyn FnMut(TransferProgress) + Send>>,
) -> Result<(DiskResource, UploadedFile)> {
    let progress = progress.map(|callback| Arc::new(Mutex::new(callback)));
    retry_transfer(|_| {
        upload_private_resource_attempt(base_url, access_token, request, progress.clone())
    })
}

fn upload_private_resource_attempt(
    base_url: &str,
    access_token: &str,
    request: &PrivateDiskUploadRequest,
    progress: Option<TransferProgressCallback>,
) -> Result<(DiskResource, UploadedFile)> {
    let remote_path = request.path.trim();
    if remote_path.is_empty() {
        return Err(YacliError::Validation(
            "disk upload <PATH> не должен быть пустым".to_string(),
        ));
    }

    let source_meta = analyze_upload_source(&request.source)?;
    let client = build_http_client()?;
    let endpoint = disk_upload_endpoint(base_url)?;
    let response = client
        .get(endpoint)
        .header("Authorization", format!("OAuth {access_token}"))
        .query(&[
            ("path", remote_path),
            (
                "overwrite",
                if request.overwrite { "true" } else { "false" },
            ),
        ])
        .send()?;
    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(provider_error(
            status,
            &body,
            "private file",
            "upload_ticket",
        ));
    }

    let ticket = serde_json::from_str::<RawUploadTicket>(&body)
        .map_err(|err| YacliError::Serialization(format!("invalid provider response: {err}")))?;

    if !ticket.method.eq_ignore_ascii_case("PUT") {
        return Err(YacliError::UnsupportedOperation(format!(
            "Yandex Disk upload ticket requested unsupported method {}",
            ticket.method
        )));
    }

    let upload_timeout = configured_upload_timeout()?;
    let upload_client = build_transfer_http_client(upload_timeout)?;
    let upload_body = ProgressReader::new(fs::File::open(&request.source)?, progress)
        .with_total(Some(source_meta.bytes_written));
    let upload_response = upload_client
        .put(&ticket.href)
        .header("Content-Type", "application/octet-stream")
        .body(reqwest::blocking::Body::new(upload_body))
        .send()
        .map_err(|err| {
            if err.is_timeout() && upload_timeout.is_some() {
                YacliError::Network(format!(
                    "Yandex Disk upload timed out after configured ceiling {}s while sending {}",
                    upload_timeout.unwrap_or_default().as_secs(),
                    request.source.display()
                ))
            } else {
                YacliError::from(err)
            }
        })?;
    let upload_status = upload_response.status();
    let upload_body = upload_response.text()?;

    if !matches!(
        upload_status,
        StatusCode::CREATED | StatusCode::ACCEPTED | StatusCode::OK
    ) {
        return Err(upload_target_error(upload_status, &upload_body));
    }

    let resource = fetch_private_resource(
        base_url,
        access_token,
        &PrivateDiskListRequest {
            path: remote_path.to_string(),
            limit: 100,
            offset: 0,
        },
    )?;

    Ok((
        resource,
        UploadedFile {
            source_path: request.source.display().to_string(),
            remote_path: remote_path.to_string(),
            bytes_written: source_meta.bytes_written,
            sha256: source_meta.sha256,
            overwrite: request.overwrite,
            attempts: 1,
            elapsed_ms: 0,
        },
    ))
}

pub fn review_private_upload(request: &PrivateDiskUploadRequest) -> Result<DiskUploadReview> {
    let remote_path = request.path.trim();
    if remote_path.is_empty() {
        return Err(YacliError::Validation(
            "disk upload <PATH> не должен быть пустым".to_string(),
        ));
    }

    let source_meta = analyze_upload_source(&request.source)?;
    Ok(DiskUploadReview {
        source_path: request.source.display().to_string(),
        remote_path: remote_path.to_string(),
        bytes_written: source_meta.bytes_written,
        sha256: source_meta.sha256,
        overwrite: request.overwrite,
    })
}

pub fn download_private_resource(
    base_url: &str,
    access_token: &str,
    request: &PrivateDiskDownloadRequest,
) -> Result<(DiskResource, DownloadedFile)> {
    download_private_resource_with_progress(base_url, access_token, request, None)
}

pub fn download_private_resource_with_progress(
    base_url: &str,
    access_token: &str,
    request: &PrivateDiskDownloadRequest,
    progress: Option<Box<dyn FnMut(TransferProgress) + Send>>,
) -> Result<(DiskResource, DownloadedFile)> {
    let remote_path = request.path.trim();
    if remote_path.is_empty() {
        return Err(YacliError::Validation(
            "disk download <PATH> не должен быть пустым".to_string(),
        ));
    }

    let resource = fetch_private_resource(
        base_url,
        access_token,
        &PrivateDiskListRequest {
            path: remote_path.to_string(),
            limit: 100,
            offset: 0,
        },
    )?;
    if resource.resource_type != "file" {
        return Err(YacliError::UnsupportedOperation(format!(
            "private download requires a file resource, got {}",
            resource.resource_type
        )));
    }

    let client = build_http_client()?;
    let endpoint = disk_download_endpoint(base_url)?;
    let response = client
        .get(endpoint)
        .header("Authorization", format!("OAuth {access_token}"))
        .query(&[("path", remote_path)])
        .send()?;
    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(provider_error(
            status,
            &body,
            "private file",
            "download_ticket",
        ));
    }

    let ticket = serde_json::from_str::<RawDownloadTicket>(&body)
        .map_err(|err| YacliError::Serialization(format!("invalid provider response: {err}")))?;
    if !ticket.method.eq_ignore_ascii_case("GET") {
        return Err(YacliError::UnsupportedOperation(format!(
            "Yandex Disk download ticket requested unsupported method {}",
            ticket.method
        )));
    }

    let download_timeout = configured_download_timeout()?;
    let download_client = build_transfer_http_client(download_timeout)?;
    let progress = progress.map(|callback| Arc::new(Mutex::new(callback)));
    let artifact = retry_transfer(|_| {
        download_to_path(
            &download_client,
            &ticket.href,
            &request.output,
            request.force,
            resource.size,
            progress.clone(),
        )
    })?;

    Ok((resource, artifact))
}

fn fetch_public_resource_with_client(
    client: &Client,
    base_url: &str,
    request: &PublicDiskRequest,
) -> Result<PublicResource> {
    let endpoint = public_resource_endpoint(base_url)?;
    let mut query = vec![("public_key", request.public_key.as_str())];
    if let Some(path) = request.path.as_deref() {
        query.push(("path", path));
    }

    let response = client.get(endpoint).query(&query).send()?;
    let status = response.status();
    let body = response.text()?;

    if !status.is_success() {
        return Err(provider_error(status, &body, "public metadata", "metadata"));
    }

    let raw = serde_json::from_str::<RawPublicResource>(&body)
        .map_err(|err| YacliError::Serialization(format!("invalid provider response: {err}")))?;
    Ok(raw.into_public_resource())
}

fn download_to_path(
    client: &Client,
    download_url: &str,
    output: &Path,
    force: bool,
    expected_size: Option<u64>,
    progress: Option<TransferProgressCallback>,
) -> Result<DownloadedFile> {
    if output.exists() && !force {
        return Err(YacliError::OutputExists(output.display().to_string()));
    }

    if output.is_dir() {
        return Err(YacliError::UnsupportedOperation(format!(
            "output path points to a directory: {}",
            output.display()
        )));
    }

    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let partial_path = partial_download_path(output);
    let mut resumed_from_bytes = existing_partial_len(&partial_path)?;
    if let Some(expected_size) = expected_size
        && resumed_from_bytes > expected_size
    {
        fs::remove_file(&partial_path)?;
        resumed_from_bytes = 0;
    }

    let mut request = client.get(download_url);
    if resumed_from_bytes > 0 {
        request = request.header("Range", format!("bytes={resumed_from_bytes}-"));
    }

    let response = request.send()?;
    let status = response.status();
    let (mut response, mut bytes_written, mut hasher) = if status == StatusCode::PARTIAL_CONTENT {
        let (existing_bytes, existing_hasher) = analyze_existing_partial(&partial_path)?;
        if existing_bytes != resumed_from_bytes {
            return Err(YacliError::Integrity(format!(
                "partial download size changed during resume: expected {}, got {}",
                resumed_from_bytes, existing_bytes
            )));
        }
        (response, existing_bytes, existing_hasher)
    } else if status.is_success() {
        if resumed_from_bytes > 0 && partial_path.exists() {
            fs::remove_file(&partial_path)?;
            resumed_from_bytes = 0;
        }
        (response, 0, Sha256::new())
    } else if status == StatusCode::RANGE_NOT_SATISFIABLE && resumed_from_bytes > 0 {
        if let Some(expected_size) = expected_size
            && resumed_from_bytes == expected_size
        {
            let (existing_bytes, existing_hasher) = analyze_existing_partial(&partial_path)?;
            finalize_partial_download(&partial_path, output, force)?;
            return Ok(DownloadedFile {
                output_path: output.display().to_string(),
                bytes_written: existing_bytes,
                sha256: format!("{:x}", existing_hasher.finalize()),
                resumed_from_bytes,
                attempts: 1,
                elapsed_ms: 0,
            });
        }
        return Err(YacliError::Api(format!(
            "Yandex Disk download resume failed with status {}",
            status.as_u16()
        )));
    } else {
        return Err(YacliError::Api(format!(
            "Yandex Disk download failed with status {}",
            status.as_u16()
        )));
    };

    let mut partial_file = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(resumed_from_bytes == 0)
        .append(resumed_from_bytes > 0)
        .open(&partial_path)?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut reporter = TransferReporter::new(expected_size, progress);
    if resumed_from_bytes > 0 {
        reporter.record(resumed_from_bytes, false);
    }

    loop {
        let read = response.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        partial_file.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        bytes_written += read as u64;
        reporter.record(bytes_written, false);
    }
    reporter.record(bytes_written, true);

    if let Some(expected_size) = expected_size
        && bytes_written != expected_size
    {
        return Err(YacliError::Integrity(format!(
            "downloaded byte count mismatch: expected {}, got {}",
            expected_size, bytes_written
        )));
    }

    partial_file.sync_all()?;
    finalize_partial_download(&partial_path, output, force)?;

    Ok(DownloadedFile {
        output_path: output.display().to_string(),
        bytes_written,
        sha256: format!("{:x}", hasher.finalize()),
        resumed_from_bytes,
        attempts: 1,
        elapsed_ms: 0,
    })
}

fn retry_transfer<T: TransferStatTarget>(mut op: impl FnMut(usize) -> Result<T>) -> Result<T> {
    let started_at = Instant::now();
    for attempt in 1..=TRANSFER_RETRY_ATTEMPTS {
        match op(attempt) {
            Ok(mut value) => {
                apply_transfer_stats(&mut value, attempt, started_at.elapsed());
                return Ok(value);
            }
            Err(err) => {
                if attempt == TRANSFER_RETRY_ATTEMPTS || !is_retryable_transfer_error(&err) {
                    return Err(err);
                }
                thread::sleep(Duration::from_millis(
                    TRANSFER_RETRY_BACKOFF_MS * attempt as u64,
                ));
            }
        }
    }
    unreachable!("retry loop must return")
}

trait TransferStatTarget {
    fn set_transfer_stats(&mut self, attempts: usize, elapsed: Duration);
}

fn apply_transfer_stats<T: TransferStatTarget>(value: &mut T, attempts: usize, elapsed: Duration) {
    value.set_transfer_stats(attempts, elapsed);
}

fn build_http_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("yacli/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(DEFAULT_HTTP_TIMEOUT_SECS))
        .timeout(Duration::from_secs(DEFAULT_HTTP_TIMEOUT_SECS))
        .build()
        .map_err(Into::into)
}

fn build_transfer_http_client(timeout: Option<Duration>) -> Result<Client> {
    let mut builder = Client::builder()
        .user_agent(format!("yacli/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(DEFAULT_HTTP_TIMEOUT_SECS));
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    builder.build().map_err(Into::into)
}

fn configured_upload_timeout() -> Result<Option<Duration>> {
    parse_transfer_timeout_env(
        DISK_UPLOAD_TIMEOUT_ENV,
        env::var_os(DISK_UPLOAD_TIMEOUT_ENV).as_deref(),
    )
}

fn configured_download_timeout() -> Result<Option<Duration>> {
    parse_transfer_timeout_env(
        DISK_DOWNLOAD_TIMEOUT_ENV,
        env::var_os(DISK_DOWNLOAD_TIMEOUT_ENV).as_deref(),
    )
}

fn parse_transfer_timeout_env(
    env_name: &str,
    raw: Option<&std::ffi::OsStr>,
) -> Result<Option<Duration>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let text = raw.to_string_lossy();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let seconds = u64::from_str(trimmed).map_err(|_| {
        YacliError::Config(format!(
            "{env_name} must be a positive integer number of seconds"
        ))
    })?;
    if seconds == 0 {
        return Ok(None);
    }
    Ok(Some(Duration::from_secs(seconds)))
}

struct TransferReporter {
    total_bytes: Option<u64>,
    callback: Option<TransferProgressCallback>,
    started_at: Instant,
    last_emitted_at: Instant,
}

impl TransferReporter {
    fn new(total_bytes: Option<u64>, callback: Option<TransferProgressCallback>) -> Self {
        let now = Instant::now();
        Self {
            total_bytes,
            callback,
            started_at: now,
            last_emitted_at: now,
        }
    }

    fn record(&mut self, transferred_bytes: u64, finished: bool) {
        let Some(callback) = self.callback.as_ref() else {
            return;
        };
        let now = Instant::now();
        if !finished
            && now.duration_since(self.last_emitted_at).as_millis() < TRANSFER_PROGRESS_INTERVAL_MS
        {
            return;
        }
        self.last_emitted_at = now;
        let elapsed = now.duration_since(self.started_at).as_secs_f64();
        let bytes_per_second = if elapsed > 0.0 {
            transferred_bytes as f64 / elapsed
        } else {
            0.0
        };
        if let Ok(mut callback) = callback.lock() {
            callback(TransferProgress {
                transferred_bytes,
                total_bytes: self.total_bytes,
                bytes_per_second,
                finished,
            });
        }
    }
}

struct ProgressReader<R> {
    inner: R,
    reporter: TransferReporter,
    transferred_bytes: u64,
}

impl<R> ProgressReader<R> {
    fn new(inner: R, callback: Option<TransferProgressCallback>) -> Self {
        Self {
            inner,
            reporter: TransferReporter::new(None, callback),
            transferred_bytes: 0,
        }
    }

    fn with_total(mut self, total_bytes: Option<u64>) -> Self {
        self.reporter.total_bytes = total_bytes;
        self
    }
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        if read == 0 {
            self.reporter.record(self.transferred_bytes, true);
            return Ok(0);
        }
        self.transferred_bytes += read as u64;
        self.reporter.record(self.transferred_bytes, false);
        Ok(read)
    }
}

impl TransferStatTarget for DownloadedFile {
    fn set_transfer_stats(&mut self, attempts: usize, elapsed: Duration) {
        self.attempts = attempts;
        self.elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
    }
}

impl TransferStatTarget for UploadedFile {
    fn set_transfer_stats(&mut self, attempts: usize, elapsed: Duration) {
        self.attempts = attempts;
        self.elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
    }
}

impl TransferStatTarget for (DiskResource, UploadedFile) {
    fn set_transfer_stats(&mut self, attempts: usize, elapsed: Duration) {
        self.1.set_transfer_stats(attempts, elapsed);
    }
}

fn is_retryable_transfer_error(err: &YacliError) -> bool {
    match err {
        YacliError::Network(_) => true,
        YacliError::Io(message) => {
            let lower = message.to_ascii_lowercase();
            lower.contains("timed out")
                || lower.contains("connection reset")
                || lower.contains("connection aborted")
                || lower.contains("unexpected eof")
                || lower.contains("broken pipe")
        }
        YacliError::Api(message) => {
            message.contains("status 500")
                || message.contains("status 502")
                || message.contains("status 503")
                || message.contains("status 504")
        }
        _ => false,
    }
}

fn partial_download_path(output: &Path) -> PathBuf {
    let file_name = output
        .file_name()
        .map(|name| format!("{}.part", name.to_string_lossy()))
        .unwrap_or_else(|| ".download.part".to_string());
    output.with_file_name(file_name)
}

fn existing_partial_len(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(YacliError::UnsupportedOperation(format!(
            "partial download path points to a directory: {}",
            path.display()
        )));
    }
    Ok(metadata.len())
}

fn analyze_existing_partial(path: &Path) -> Result<(u64, Sha256)> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut bytes_written = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes_written += read as u64;
    }
    Ok((bytes_written, hasher))
}

fn finalize_partial_download(partial_path: &Path, output: &Path, force: bool) -> Result<()> {
    if force && output.exists() {
        fs::remove_file(output)?;
    }
    fs::rename(partial_path, output)?;
    Ok(())
}

fn public_resource_endpoint(base_url: &str) -> Result<Url> {
    Url::parse(base_url)
        .map_err(|err| YacliError::Config(format!("invalid disk base URL: {err}")))?
        .join("/v1/disk/public/resources")
        .map_err(|err| YacliError::Config(format!("invalid disk endpoint: {err}")))
}

fn disk_resources_endpoint(base_url: &str) -> Result<Url> {
    Url::parse(base_url)
        .map_err(|err| YacliError::Config(format!("invalid disk base URL: {err}")))?
        .join("/v1/disk/resources")
        .map_err(|err| YacliError::Config(format!("invalid disk endpoint: {err}")))
}

fn disk_upload_endpoint(base_url: &str) -> Result<Url> {
    Url::parse(base_url)
        .map_err(|err| YacliError::Config(format!("invalid disk base URL: {err}")))?
        .join("/v1/disk/resources/upload")
        .map_err(|err| YacliError::Config(format!("invalid disk endpoint: {err}")))
}

fn disk_download_endpoint(base_url: &str) -> Result<Url> {
    Url::parse(base_url)
        .map_err(|err| YacliError::Config(format!("invalid disk base URL: {err}")))?
        .join("/v1/disk/resources/download")
        .map_err(|err| YacliError::Config(format!("invalid disk endpoint: {err}")))
}

fn disk_publish_endpoint(base_url: &str) -> Result<Url> {
    Url::parse(base_url)
        .map_err(|err| YacliError::Config(format!("invalid disk base URL: {err}")))?
        .join("/v1/disk/resources/publish")
        .map_err(|err| YacliError::Config(format!("invalid disk endpoint: {err}")))
}

fn disk_unpublish_endpoint(base_url: &str) -> Result<Url> {
    Url::parse(base_url)
        .map_err(|err| YacliError::Config(format!("invalid disk base URL: {err}")))?
        .join("/v1/disk/resources/unpublish")
        .map_err(|err| YacliError::Config(format!("invalid disk endpoint: {err}")))
}

fn disk_info_endpoint(base_url: &str) -> Result<Url> {
    Url::parse(base_url)
        .map_err(|err| YacliError::Config(format!("invalid disk base URL: {err}")))?
        .join("/v1/disk")
        .map_err(|err| YacliError::Config(format!("invalid disk endpoint: {err}")))
}

fn provider_error(status: StatusCode, body: &str, surface: &str, action: &str) -> YacliError {
    let parsed = serde_json::from_str::<ProviderError>(body).ok();
    let provider_code = parsed
        .as_ref()
        .and_then(|value| value.error.as_deref())
        .unwrap_or("UnknownProviderError");
    let provider_message = parsed
        .as_ref()
        .and_then(|value| value.message.as_deref())
        .unwrap_or("No provider message returned.");

    YacliError::Api(format!(
        "Yandex Disk {surface} {action} request failed with status {} ({}): {}",
        status.as_u16(),
        provider_code,
        provider_message
    ))
}

fn upload_target_error(status: StatusCode, body: &str) -> YacliError {
    if body.trim().is_empty() {
        YacliError::Api(format!(
            "Yandex Disk upload target request failed with status {}",
            status.as_u16()
        ))
    } else {
        YacliError::Api(format!(
            "Yandex Disk upload target request failed with status {}: {}",
            status.as_u16(),
            body.trim()
        ))
    }
}

fn analyze_upload_source(path: &Path) -> Result<UploadSourceMeta> {
    let metadata = fs::metadata(path).map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            YacliError::Validation(format!("disk upload: файл не найден: {}", path.display()))
        } else {
            YacliError::Io(err.to_string())
        }
    })?;

    if !metadata.is_file() {
        return Err(YacliError::Validation(format!(
            "disk upload: путь должен указывать на файл: {}",
            path.display()
        )));
    }

    if metadata.len() == 0 {
        return Err(YacliError::Validation(
            "disk upload: файл не должен быть пустым".to_string(),
        ));
    }

    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut bytes_written = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        hasher.update(&buffer[..read]);
        bytes_written += read as u64;
    }

    Ok(UploadSourceMeta {
        bytes_written,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

#[derive(Debug, Deserialize)]
struct ProviderError {
    error: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawUploadTicket {
    href: String,
    #[serde(default)]
    method: String,
}

#[derive(Debug, Deserialize)]
struct RawDownloadTicket {
    href: String,
    #[serde(default)]
    method: String,
}

struct UploadSourceMeta {
    bytes_written: u64,
    sha256: String,
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::sync::{Arc, Mutex};

    use super::{
        DISK_DOWNLOAD_TIMEOUT_ENV, DISK_UPLOAD_TIMEOUT_ENV, DiskPublishReview, DiskUnpublishReview,
        PrivateDiskUploadRequest, ProgressReader, TransferProgress, is_retryable_transfer_error,
        parse_transfer_timeout_env, review_private_upload,
    };
    use crate::error::YacliError;

    #[test]
    fn review_private_upload_reports_file_metadata_without_network() {
        let temp = tempfile::tempdir().expect("tempdir");
        let source = temp.path().join("note.txt");
        std::fs::write(&source, "hello disk").expect("source");

        let review = review_private_upload(&PrivateDiskUploadRequest {
            source: source.clone(),
            path: "disk:/docs/note.txt".to_string(),
            overwrite: false,
        })
        .expect("review");

        assert_eq!(review.source_path, source.display().to_string());
        assert_eq!(review.remote_path, "disk:/docs/note.txt");
        assert_eq!(review.bytes_written, 10);
        assert!(!review.overwrite);
        assert_eq!(
            review.sha256,
            "7fcfee685f13646152c61ee19a72cda55f4a1e63ad9563cf8f34323fad5453f6"
        );
    }

    #[test]
    fn publish_and_unpublish_review_structs_preserve_public_state() {
        let publish = DiskPublishReview {
            path: "disk:/docs/report.pdf".to_string(),
            resource_name: "report.pdf".to_string(),
            resource_type: "file".to_string(),
            already_public: true,
            current_public_url: Some("https://disk.yandex.ru/i/report".to_string()),
            current_public_key: Some("public-key-report".to_string()),
        };
        assert!(publish.already_public);
        assert_eq!(
            publish.current_public_url.as_deref(),
            Some("https://disk.yandex.ru/i/report")
        );

        let unpublish = DiskUnpublishReview {
            path: "disk:/docs/report.pdf".to_string(),
            resource_name: "report.pdf".to_string(),
            resource_type: "file".to_string(),
            is_public: false,
            current_public_url: None,
            current_public_key: None,
        };
        assert!(!unpublish.is_public);
        assert!(unpublish.current_public_key.is_none());
    }

    #[test]
    fn parse_transfer_timeout_env_returns_none_by_default() {
        assert_eq!(
            parse_transfer_timeout_env(DISK_UPLOAD_TIMEOUT_ENV, None).expect("parse"),
            None
        );
        assert_eq!(
            parse_transfer_timeout_env(DISK_UPLOAD_TIMEOUT_ENV, Some(OsStr::new("")))
                .expect("parse"),
            None
        );
    }

    #[test]
    fn parse_transfer_timeout_env_accepts_positive_seconds() {
        assert_eq!(
            parse_transfer_timeout_env(DISK_UPLOAD_TIMEOUT_ENV, Some(OsStr::new("600")))
                .expect("parse"),
            Some(std::time::Duration::from_secs(600))
        );
        assert_eq!(
            parse_transfer_timeout_env(DISK_DOWNLOAD_TIMEOUT_ENV, Some(OsStr::new(" 45 ")))
                .expect("parse"),
            Some(std::time::Duration::from_secs(45))
        );
    }

    #[test]
    fn parse_transfer_timeout_env_treats_zero_as_unbounded() {
        assert_eq!(
            parse_transfer_timeout_env(DISK_UPLOAD_TIMEOUT_ENV, Some(OsStr::new("0")))
                .expect("parse"),
            None
        );
    }

    #[test]
    fn parse_transfer_timeout_env_rejects_invalid_values() {
        let err = parse_transfer_timeout_env(DISK_UPLOAD_TIMEOUT_ENV, Some(OsStr::new("fast")))
            .expect_err("invalid");
        assert!(
            err.to_string().contains(DISK_UPLOAD_TIMEOUT_ENV),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn progress_reader_emits_finished_progress_snapshot() {
        let updates = Arc::new(Mutex::new(Vec::<TransferProgress>::new()));
        let captured = Arc::clone(&updates);
        let mut reader = ProgressReader::new(
            std::io::Cursor::new(b"hello world".to_vec()),
            Some(Arc::new(Mutex::new(Box::new(move |progress| {
                captured.lock().expect("lock").push(progress);
            })))),
        )
        .with_total(Some(11));

        let mut sink = Vec::new();
        std::io::copy(&mut reader, &mut sink).expect("copy");

        let updates = updates.lock().expect("lock");
        let last = updates.last().expect("progress");
        assert!(last.finished);
        assert_eq!(last.transferred_bytes, 11);
        assert_eq!(last.total_bytes, Some(11));
    }

    #[test]
    fn retryable_transfer_error_accepts_transient_network_and_io_failures() {
        assert!(is_retryable_transfer_error(&YacliError::Network(
            "request timed out".to_string()
        )));
        assert!(is_retryable_transfer_error(&YacliError::Io(
            "connection reset by peer".to_string()
        )));
        assert!(is_retryable_transfer_error(&YacliError::Api(
            "Yandex Disk upload target request failed with status 503".to_string()
        )));
    }

    #[test]
    fn retryable_transfer_error_rejects_validation_and_non_transient_api_failures() {
        assert!(!is_retryable_transfer_error(&YacliError::Validation(
            "disk upload <PATH> не должен быть пустым".to_string()
        )));
        assert!(!is_retryable_transfer_error(&YacliError::Api(
            "Yandex Disk upload target request failed with status 409".to_string()
        )));
    }
}

#[derive(Debug, Deserialize)]
struct RawPublicResource {
    name: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(rename = "type")]
    resource_type: String,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    public_url: Option<String>,
    #[serde(default)]
    public_key: Option<String>,
    #[serde(default, rename = "file")]
    download_url: Option<String>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    modified: Option<String>,
    #[serde(default)]
    md5: Option<String>,
    #[serde(default, rename = "_embedded")]
    embedded: Option<RawPublicResourceChildren>,
}

#[derive(Debug, Deserialize)]
struct RawPublicResourceChildren {
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    items: Vec<RawPublicResourceItem>,
}

#[derive(Debug, Deserialize)]
struct RawPublicResourceItem {
    name: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(rename = "type")]
    resource_type: String,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    public_url: Option<String>,
    #[serde(default)]
    public_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawDiskInfo {
    total_space: u64,
    used_space: u64,
    #[serde(default)]
    trash_size: u64,
    #[serde(default)]
    max_file_size: Option<u64>,
    #[serde(default)]
    paid_max_file_size: Option<u64>,
    #[serde(default)]
    user: Option<RawDiskUser>,
    #[serde(default)]
    is_paid: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RawDiskUser {
    #[serde(default)]
    login: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    uid: Option<String>,
    #[serde(default)]
    country: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawDiskResource {
    name: String,
    path: String,
    #[serde(rename = "type")]
    resource_type: String,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    modified: Option<String>,
    #[serde(default)]
    md5: Option<String>,
    #[serde(default, rename = "revision")]
    revision: Option<u64>,
    #[serde(default)]
    public_url: Option<String>,
    #[serde(default)]
    public_key: Option<String>,
    #[serde(default, rename = "_embedded")]
    embedded: Option<RawDiskResourceChildren>,
}

#[derive(Debug, Deserialize)]
struct RawDiskResourceChildren {
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
    total: Option<u64>,
    #[serde(default)]
    items: Vec<RawDiskResourceItem>,
}

#[derive(Debug, Deserialize)]
struct RawDiskResourceItem {
    name: String,
    path: String,
    #[serde(rename = "type")]
    resource_type: String,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    modified: Option<String>,
    #[serde(default)]
    md5: Option<String>,
    #[serde(default, rename = "revision")]
    revision: Option<u64>,
    #[serde(default)]
    public_url: Option<String>,
    #[serde(default)]
    public_key: Option<String>,
}

impl RawPublicResource {
    fn into_public_resource(self) -> PublicResource {
        PublicResource {
            name: self.name,
            path: self.path,
            resource_type: self.resource_type,
            mime_type: self.mime_type,
            size: self.size,
            public_url: self.public_url,
            public_key: self.public_key,
            download_url: self.download_url,
            created: self.created,
            modified: self.modified,
            md5: self.md5,
            children: self.embedded.map(|embedded| PublicResourceChildren {
                limit: embedded.limit.unwrap_or(0),
                offset: embedded.offset.unwrap_or(0),
                total: embedded.total.unwrap_or(embedded.items.len() as u64),
                items: embedded
                    .items
                    .into_iter()
                    .map(|item| PublicResourceItem {
                        name: item.name,
                        path: item.path,
                        resource_type: item.resource_type,
                        mime_type: item.mime_type,
                        size: item.size,
                        public_url: item.public_url,
                        public_key: item.public_key,
                    })
                    .collect(),
            }),
        }
    }
}

impl RawDiskInfo {
    fn into_disk_info(self) -> DiskInfo {
        DiskInfo {
            total_space: self.total_space,
            used_space: self.used_space,
            trash_size: self.trash_size,
            max_file_size: self.max_file_size,
            paid_max_file_size: self.paid_max_file_size,
            user: self.user.map(|user| DiskUser {
                login: user.login,
                display_name: user.display_name,
                uid: user.uid,
                country: user.country,
            }),
            is_paid: self.is_paid,
        }
    }
}

impl RawDiskResource {
    fn into_disk_resource(self) -> DiskResource {
        DiskResource {
            name: self.name,
            path: self.path,
            resource_type: self.resource_type,
            mime_type: self.mime_type,
            size: self.size,
            created: self.created,
            modified: self.modified,
            md5: self.md5,
            revision: self.revision,
            public_url: self.public_url,
            public_key: self.public_key,
            children: self.embedded.map(|embedded| DiskResourceChildren {
                limit: embedded.limit.unwrap_or(0),
                offset: embedded.offset.unwrap_or(0),
                total: embedded.total.unwrap_or(embedded.items.len() as u64),
                items: embedded
                    .items
                    .into_iter()
                    .map(|item| DiskResourceItem {
                        name: item.name,
                        path: item.path,
                        resource_type: item.resource_type,
                        mime_type: item.mime_type,
                        size: item.size,
                        created: item.created,
                        modified: item.modified,
                        md5: item.md5,
                        revision: item.revision,
                        public_url: item.public_url,
                        public_key: item.public_key,
                    })
                    .collect(),
            }),
        }
    }
}
