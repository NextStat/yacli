use std::env;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use reqwest::Url;
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tar::Archive;
use tempfile::tempdir;
use zip::ZipArchive;

use crate::cli::OutputFormat;
use crate::error::{Result, YacliError};
use crate::output::RenderedOutput;

const REPO_SLUG: &str = "NextStat/yacli";
const DEFAULT_VERSION: &str = "latest";
const UPDATE_BASE_URL_ENV: &str = "YACLI_UPDATE_BASE_URL";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArchiveKind {
    TarGz,
    Zip,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ReleaseAsset {
    archive_kind: ArchiveKind,
    binary_name: &'static str,
    file_name: &'static str,
    target: &'static str,
}

#[derive(Debug)]
struct ReleasePlan {
    asset: ReleaseAsset,
    base_url: String,
    requested_version: String,
    resolved_version: Option<String>,
    checksum: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UpdateStatusReport {
    pub operation: String,
    pub status: String,
    pub current_version: String,
    pub target_version: String,
    pub requested_version: String,
    pub asset: String,
    pub target: String,
    pub base_url: String,
}

pub fn execute_update(
    format: OutputFormat,
    version: &str,
    check: bool,
    base_url: Option<&str>,
) -> Result<RenderedOutput> {
    let (current_version, plan) = resolve_release_plan(version, base_url)?;

    if check {
        let report = build_update_report("update.check", &current_version, &plan, None);
        return render_output(format, &report);
    }

    if plan
        .resolved_version
        .as_deref()
        .is_some_and(|version| version == current_version)
    {
        let report = build_update_report(
            "update.apply",
            &current_version,
            &plan,
            Some("already_up_to_date"),
        );
        return render_output(format, &report);
    }

    let client = release_client()?;
    let archive_bytes = download_release_archive(&client, &plan)?;
    verify_checksum(&archive_bytes, &plan.checksum, plan.asset.file_name)?;
    let staged_binary = stage_release_binary(&archive_bytes, &plan.asset)?;
    replace_current_binary(&staged_binary)?;

    let report = build_update_report("update.apply", &current_version, &plan, Some("updated"));
    render_output(format, &report)
}

pub fn check_for_update(version: Option<&str>) -> Result<UpdateStatusReport> {
    let requested_version = version.unwrap_or(DEFAULT_VERSION);
    let (current_version, plan) = resolve_release_plan(requested_version, None)?;
    Ok(build_update_report(
        "update.check",
        &current_version,
        &plan,
        None,
    ))
}

fn release_client() -> Result<Client> {
    Client::builder()
        .user_agent(format!("yacli/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(YacliError::from)
}

fn normalize_version(version: &str) -> String {
    let trimmed = version.trim();
    if trimmed.is_empty() || trimmed == DEFAULT_VERSION {
        DEFAULT_VERSION.to_string()
    } else if trimmed.starts_with('v') {
        trimmed.to_string()
    } else {
        format!("v{trimmed}")
    }
}

fn configured_base_url_override(override_base_url: Option<&str>) -> Option<String> {
    if let Some(base_url) = override_base_url {
        return Some(base_url.trim_end_matches('/').to_string());
    }
    env::var(UPDATE_BASE_URL_ENV)
        .ok()
        .map(|value| value.trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn release_base_url(version: &str, override_base_url: Option<&str>) -> String {
    if let Some(base_url) = configured_base_url_override(override_base_url) {
        return base_url.trim_end_matches('/').to_string();
    }

    if version == DEFAULT_VERSION {
        format!("https://github.com/{REPO_SLUG}/releases/latest/download")
    } else {
        format!("https://github.com/{REPO_SLUG}/releases/download/{version}")
    }
}

fn current_release_asset() -> Result<ReleaseAsset> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => Ok(ReleaseAsset {
            archive_kind: ArchiveKind::TarGz,
            binary_name: "yacli",
            file_name: "yacli-aarch64-apple-darwin.tar.gz",
            target: "aarch64-apple-darwin",
        }),
        ("macos", "x86_64") => Ok(ReleaseAsset {
            archive_kind: ArchiveKind::TarGz,
            binary_name: "yacli",
            file_name: "yacli-x86_64-apple-darwin.tar.gz",
            target: "x86_64-apple-darwin",
        }),
        ("linux", "aarch64") => Ok(ReleaseAsset {
            archive_kind: ArchiveKind::TarGz,
            binary_name: "yacli",
            file_name: "yacli-aarch64-unknown-linux-gnu.tar.gz",
            target: "aarch64-unknown-linux-gnu",
        }),
        ("linux", "x86_64") => Ok(ReleaseAsset {
            archive_kind: ArchiveKind::TarGz,
            binary_name: "yacli",
            file_name: "yacli-x86_64-unknown-linux-gnu.tar.gz",
            target: "x86_64-unknown-linux-gnu",
        }),
        ("windows", "x86_64") => Ok(ReleaseAsset {
            archive_kind: ArchiveKind::Zip,
            binary_name: "yacli.exe",
            file_name: "yacli-x86_64-pc-windows-msvc.zip",
            target: "x86_64-pc-windows-msvc",
        }),
        (os, arch) => Err(YacliError::UnsupportedOperation(format!(
            "auto-update is not published for target {arch}-{os}"
        ))),
    }
}

fn fetch_checksums(client: &Client, base_url: &str) -> Result<(String, Option<String>)> {
    let response = client
        .get(format!("{base_url}/SHA256SUMS"))
        .send()
        .map_err(YacliError::from)?;
    let final_url = response.url().clone();
    let response = response.error_for_status().map_err(YacliError::from)?;
    let body = response.text().map_err(YacliError::from)?;
    Ok((body, version_from_download_url(&final_url)))
}

fn version_from_download_url(url: &Url) -> Option<String> {
    let segments = url.path_segments()?.collect::<Vec<_>>();
    let download_index = segments.iter().position(|segment| *segment == "download")?;
    let tag = segments.get(download_index + 1)?.strip_prefix('v')?;
    Some(tag.to_string())
}

fn checksum_for_asset(checksums: &str, asset_name: &str) -> Result<String> {
    for line in checksums.lines() {
        let mut parts = line.split_whitespace();
        let Some(sum) = parts.next() else {
            continue;
        };
        let Some(name) = parts.next() else {
            continue;
        };
        if name == asset_name {
            return Ok(sum.to_string());
        }
    }

    Err(YacliError::Integrity(format!(
        "missing checksum for release asset {asset_name}"
    )))
}

fn download_release_archive(client: &Client, plan: &ReleasePlan) -> Result<Vec<u8>> {
    let response = client
        .get(format!("{}/{}", plan.base_url, plan.asset.file_name))
        .send()
        .map_err(YacliError::from)?;
    let response = response.error_for_status().map_err(YacliError::from)?;
    response
        .bytes()
        .map(|bytes| bytes.to_vec())
        .map_err(YacliError::from)
}

fn resolve_release_plan(version: &str, base_url: Option<&str>) -> Result<(String, ReleasePlan)> {
    let client = release_client()?;
    let requested_version = normalize_version(version);
    let asset = current_release_asset()?;
    let base_url = release_base_url(&requested_version, base_url);
    let (checksums, resolved_version) = fetch_checksums(&client, &base_url)?;
    let checksum = checksum_for_asset(&checksums, asset.file_name)?;
    let current_version = env!("CARGO_PKG_VERSION").to_string();

    Ok((
        current_version,
        ReleasePlan {
            asset,
            base_url,
            requested_version,
            resolved_version,
            checksum,
        },
    ))
}

fn verify_checksum(bytes: &[u8], expected: &str, asset_name: &str) -> Result<()> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual == expected {
        Ok(())
    } else {
        Err(YacliError::Integrity(format!(
            "checksum mismatch for {asset_name}: expected {expected}, got {actual}"
        )))
    }
}

fn stage_release_binary(bytes: &[u8], asset: &ReleaseAsset) -> Result<PathBuf> {
    let tmp_dir = tempdir().map_err(|err| {
        YacliError::Io(format!(
            "failed to create temporary update directory: {err}"
        ))
    })?;
    let extract_root = tmp_dir.path().join("extract");
    fs::create_dir_all(&extract_root)
        .map_err(|err| YacliError::Io(format!("failed to create extraction directory: {err}")))?;

    match asset.archive_kind {
        ArchiveKind::TarGz => extract_tar_gz(bytes, &extract_root)?,
        ArchiveKind::Zip => extract_zip(bytes, &extract_root)?,
    }

    let extracted_binary = find_binary(&extract_root, asset.binary_name)?;
    let staged_binary = tmp_dir.path().join(asset.binary_name);
    fs::copy(&extracted_binary, &staged_binary).map_err(|err| {
        YacliError::Io(format!(
            "failed to stage extracted binary {}: {err}",
            extracted_binary.display()
        ))
    })?;
    make_executable_if_needed(&staged_binary)?;
    let persisted_dir = tmp_dir.keep();

    Ok(persisted_dir.join(asset.binary_name))
}

fn extract_tar_gz(bytes: &[u8], destination: &Path) -> Result<()> {
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = Archive::new(decoder);
    archive
        .unpack(destination)
        .map_err(|err| YacliError::Io(format!("failed to unpack release archive: {err}")))
}

fn extract_zip(bytes: &[u8], destination: &Path) -> Result<()> {
    let reader = Cursor::new(bytes);
    let mut archive = ZipArchive::new(reader)
        .map_err(|err| YacliError::Io(format!("failed to open release zip: {err}")))?;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|err| YacliError::Io(format!("failed to read release zip entry: {err}")))?;
        let Some(name) = entry.enclosed_name().map(|name| name.to_path_buf()) else {
            continue;
        };
        let output_path = destination.join(name);
        if entry.name().ends_with('/') {
            fs::create_dir_all(&output_path).map_err(|err| {
                YacliError::Io(format!(
                    "failed to create extracted directory {}: {err}",
                    output_path.display()
                ))
            })?;
            continue;
        }

        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                YacliError::Io(format!(
                    "failed to create extracted directory {}: {err}",
                    parent.display()
                ))
            })?;
        }

        let mut output = fs::File::create(&output_path).map_err(|err| {
            YacliError::Io(format!(
                "failed to create extracted file {}: {err}",
                output_path.display()
            ))
        })?;
        let mut buffer = Vec::new();
        entry.read_to_end(&mut buffer).map_err(|err| {
            YacliError::Io(format!(
                "failed to read extracted file {}: {err}",
                output_path.display()
            ))
        })?;
        output.write_all(&buffer).map_err(|err| {
            YacliError::Io(format!(
                "failed to write extracted file {}: {err}",
                output_path.display()
            ))
        })?;

        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&output_path, fs::Permissions::from_mode(mode)).map_err(|err| {
                YacliError::Io(format!(
                    "failed to restore permissions for {}: {err}",
                    output_path.display()
                ))
            })?;
        }
    }

    Ok(())
}

fn find_binary(root: &Path, binary_name: &str) -> Result<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in fs::read_dir(&path).map_err(|err| {
            YacliError::Io(format!(
                "failed to read extracted directory {}: {err}",
                path.display()
            ))
        })? {
            let entry = entry.map_err(|err| {
                YacliError::Io(format!(
                    "failed to read extracted directory entry in {}: {err}",
                    path.display()
                ))
            })?;
            let entry_path = entry.path();
            if entry
                .file_type()
                .map_err(|err| {
                    YacliError::Io(format!(
                        "failed to inspect extracted path {}: {err}",
                        entry_path.display()
                    ))
                })?
                .is_dir()
            {
                stack.push(entry_path);
                continue;
            }
            if entry_path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == binary_name)
            {
                return Ok(entry_path);
            }
        }
    }

    Err(YacliError::Integrity(format!(
        "release archive does not contain executable {binary_name}"
    )))
}

fn make_executable_if_needed(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|err| {
                YacliError::Io(format!(
                    "failed to read metadata for {}: {err}",
                    path.display()
                ))
            })?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).map_err(|err| {
            YacliError::Io(format!(
                "failed to set executable permissions for {}: {err}",
                path.display()
            ))
        })?;
    }

    Ok(())
}

fn replace_current_binary(staged_binary: &Path) -> Result<()> {
    self_replace::self_replace(staged_binary).map_err(|err| {
        YacliError::Io(format!(
            "failed to replace current executable with {}: {err}",
            staged_binary.display()
        ))
    })
}

fn build_update_report(
    operation: &str,
    current_version: &str,
    plan: &ReleasePlan,
    explicit_status: Option<&str>,
) -> UpdateStatusReport {
    let status = explicit_status.unwrap_or_else(|| match plan.resolved_version.as_deref() {
        Some(version) if version == current_version => "already_up_to_date",
        Some(_) => "update_available",
        None => "check_complete",
    });
    let target_version = plan
        .resolved_version
        .clone()
        .unwrap_or_else(|| plan.requested_version.trim_start_matches('v').to_string());
    UpdateStatusReport {
        operation: operation.to_string(),
        status: status.to_string(),
        current_version: current_version.to_string(),
        target_version,
        requested_version: plan.requested_version.clone(),
        asset: plan.asset.file_name.to_string(),
        target: plan.asset.target.to_string(),
        base_url: plan.base_url.clone(),
    }
}

fn render_output(format: OutputFormat, report: &UpdateStatusReport) -> Result<RenderedOutput> {
    let json = json!({
        "operation": report.operation,
        "status": report.status,
        "current_version": report.current_version,
        "target_version": report.target_version,
        "requested_version": report.requested_version,
        "asset": report.asset,
        "target": report.target,
        "base_url": report.base_url,
    });
    let table = [
        format!("operation\t{}", report.operation),
        format!("status\t{}", report.status),
        format!("current_version\t{}", report.current_version),
        format!("target_version\t{}", report.target_version),
        format!("asset\t{}", report.asset),
        format!("target\t{}", report.target),
        format!("base_url\t{}", report.base_url),
    ]
    .join("\n");

    Ok(RenderedOutput {
        format,
        json,
        table,
        exit_code: 0,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use std::io::Write;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tar::Builder;
    use tempfile::tempdir;
    use zip::write::SimpleFileOptions;

    use super::*;

    static UPDATE_BASE_URL_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn normalize_version_accepts_latest_and_prefixed_tags() {
        assert_eq!(normalize_version("latest"), "latest");
        assert_eq!(normalize_version("0.1.32"), "v0.1.32");
        assert_eq!(normalize_version("v0.1.32"), "v0.1.32");
        assert_eq!(normalize_version("  "), "latest");
    }

    #[test]
    fn configured_base_url_override_prefers_explicit_value_over_env() {
        let _guard = UPDATE_BASE_URL_ENV_LOCK.lock().expect("env lock");
        unsafe {
            env::set_var(UPDATE_BASE_URL_ENV, "https://mirror.example.test/releases");
        }
        assert_eq!(
            configured_base_url_override(Some("https://override.example.test/download")).as_deref(),
            Some("https://override.example.test/download")
        );
        unsafe {
            env::remove_var(UPDATE_BASE_URL_ENV);
        }
    }

    #[test]
    fn version_from_download_url_extracts_tag() {
        let url =
            Url::parse("https://github.com/NextStat/yacli/releases/download/v0.2.0/SHA256SUMS")
                .expect("url");
        assert_eq!(version_from_download_url(&url).as_deref(), Some("0.2.0"));
    }

    #[test]
    fn checksum_for_asset_reads_sha256sums() {
        let checksums = "abc123  yacli-aarch64-apple-darwin.tar.gz\nxyz789  other.tar.gz\n";
        assert_eq!(
            checksum_for_asset(checksums, "yacli-aarch64-apple-darwin.tar.gz").expect("checksum"),
            "abc123"
        );
    }

    #[test]
    fn extract_tar_gz_stage_finds_unix_binary() {
        let bytes = build_tar_gz_asset("yacli", b"unix-binary");
        let asset = ReleaseAsset {
            archive_kind: ArchiveKind::TarGz,
            binary_name: "yacli",
            file_name: "yacli-x86_64-unknown-linux-gnu.tar.gz",
            target: "x86_64-unknown-linux-gnu",
        };

        let binary = stage_release_binary(&bytes, &asset).expect("staged binary");
        assert_eq!(fs::read(&binary).expect("binary bytes"), b"unix-binary");
    }

    #[test]
    fn extract_zip_stage_finds_windows_binary() {
        let bytes = build_zip_asset("yacli.exe", b"windows-binary");
        let asset = ReleaseAsset {
            archive_kind: ArchiveKind::Zip,
            binary_name: "yacli.exe",
            file_name: "yacli-x86_64-pc-windows-msvc.zip",
            target: "x86_64-pc-windows-msvc",
        };

        let binary = stage_release_binary(&bytes, &asset).expect("staged binary");
        assert_eq!(fs::read(&binary).expect("binary bytes"), b"windows-binary");
    }

    fn build_tar_gz_asset(binary_name: &str, contents: &[u8]) -> Vec<u8> {
        let mut archive = Vec::new();
        {
            let encoder = GzEncoder::new(&mut archive, Compression::default());
            let mut builder = Builder::new(encoder);
            let path = format!("yacli-test/{binary_name}");
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, path, Cursor::new(contents))
                .expect("append tar entry");
            builder.finish().expect("finish tar");
        }
        archive
    }

    fn build_zip_asset(binary_name: &str, contents: &[u8]) -> Vec<u8> {
        let temp = tempdir().expect("tempdir");
        let archive_path = temp.path().join("release.zip");
        let file = fs::File::create(&archive_path).expect("zip file");
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(
            format!("yacli-test/{binary_name}"),
            SimpleFileOptions::default(),
        )
        .expect("start file");
        zip.write_all(contents).expect("write zip file");
        zip.finish().expect("finish zip");
        fs::read(&archive_path).expect("zip bytes")
    }
}
