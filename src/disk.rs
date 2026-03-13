use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use url::Url;

use crate::error::{Result, YacliError};

pub const DEFAULT_DISK_BASE_URL: &str = "https://cloud-api.yandex.net";

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
}

#[derive(Clone, Debug, Serialize)]
pub struct UploadedFile {
    pub source_path: String,
    pub remote_path: String,
    pub bytes_written: u64,
    pub sha256: String,
    pub overwrite: bool,
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
    let client = build_http_client()?;
    let resource = fetch_public_resource_with_client(
        &client,
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

    let artifact = download_to_path(
        &client,
        &download_url,
        &request.output,
        request.force,
        resource.size,
    )?;

    Ok((resource, artifact))
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
            "disk list --limit must be greater than zero".to_string(),
        ));
    }
    if request.path.trim().is_empty() {
        return Err(YacliError::Validation(
            "disk list --path must not be empty".to_string(),
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
            "disk mkdir --path must not be empty".to_string(),
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

pub fn upload_private_resource(
    base_url: &str,
    access_token: &str,
    request: &PrivateDiskUploadRequest,
) -> Result<(DiskResource, UploadedFile)> {
    let remote_path = request.path.trim();
    if remote_path.is_empty() {
        return Err(YacliError::Validation(
            "disk upload --path must not be empty".to_string(),
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

    let upload_response = client
        .put(&ticket.href)
        .header("Content-Type", "application/octet-stream")
        .body(reqwest::blocking::Body::new(fs::File::open(
            &request.source,
        )?))
        .send()?;
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
        },
    ))
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

    let temp_dir = match output.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => {
            fs::create_dir_all(parent)?;
            parent.to_path_buf()
        }
        _ => std::env::current_dir()?,
    };

    let response = client.get(download_url).send()?;
    let status = response.status();
    if !status.is_success() {
        return Err(YacliError::Api(format!(
            "Yandex Disk public download failed with status {}",
            status.as_u16()
        )));
    }

    let mut response = response;
    let mut temp_file = NamedTempFile::new_in(&temp_dir)?;
    let mut hasher = Sha256::new();
    let mut bytes_written = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let read = response.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        temp_file.write_all(&buffer[..read])?;
        hasher.update(&buffer[..read]);
        bytes_written += read as u64;
    }

    if let Some(expected_size) = expected_size
        && bytes_written != expected_size
    {
        return Err(YacliError::Integrity(format!(
            "downloaded byte count mismatch: expected {}, got {}",
            expected_size, bytes_written
        )));
    }

    temp_file.as_file_mut().sync_all()?;

    if force && output.exists() {
        fs::remove_file(output)?;
    }

    temp_file
        .persist(output)
        .map_err(|err| YacliError::Io(err.error.to_string()))?;

    Ok(DownloadedFile {
        output_path: output.display().to_string(),
        bytes_written,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn build_http_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("yacli/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(Into::into)
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
            YacliError::Validation(format!(
                "disk upload --source file not found: {}",
                path.display()
            ))
        } else {
            YacliError::Io(err.to_string())
        }
    })?;

    if !metadata.is_file() {
        return Err(YacliError::Validation(format!(
            "disk upload --source must point to a file: {}",
            path.display()
        )));
    }

    if metadata.len() == 0 {
        return Err(YacliError::Validation(
            "disk upload --source file must not be empty".to_string(),
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

struct UploadSourceMeta {
    bytes_written: u64,
    sha256: String,
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
                    })
                    .collect(),
            }),
        }
    }
}
