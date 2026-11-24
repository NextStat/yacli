use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use unicode_normalization::UnicodeNormalization;

use crate::error::{Result, YacliError};

pub fn enabled() -> bool {
    std::env::var("YACLI_DEVCAT_MODE").as_deref() == Ok("1")
}

pub fn validate() -> Result<()> {
    if !enabled() {
        return Ok(());
    }
    let client = std::env::var("YACLI_DEVCAT_OAUTH_CLIENT_ID")
        .map_err(|_| YacliError::Config("DevCat MCP requires owned OAuth client ID".into()))?;
    if client.trim().is_empty() || client == crate::oauth::DEFAULT_YACLI_CLIENT_ID {
        return Err(YacliError::Config(
            "DevCat MCP requires a non-upstream OAuth client ID".into(),
        ));
    }
    let account = std::env::var("YACLI_DEVCAT_ACCOUNT")
        .map_err(|_| YacliError::Config("DevCat MCP requires a default account name".into()))?;
    if account.trim().is_empty() {
        return Err(YacliError::Config(
            "DevCat default account name is empty".into(),
        ));
    }
    let _ = allowed_accounts(&account)?;
    let roots = std::env::var("YACLI_DEVCAT_DISK_ROOTS")
        .map_err(|_| YacliError::Config("DevCat MCP requires disk roots".into()))?;
    let parsed: Vec<_> = roots
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect();
    if parsed.is_empty() {
        return Err(YacliError::Config("DevCat Disk roots are empty".into()));
    }
    for root in parsed {
        clean_path(root)?;
    }
    let known = [
        "mail.read",
        "mail.send",
        "mail.mutate",
        "mail.delete",
        "disk.read",
        "disk.write",
        "disk.delete",
        "disk.publish",
    ];
    for cap in capabilities() {
        if !known.contains(&cap.as_str()) {
            return Err(YacliError::Config(format!(
                "unknown DevCat capability: {cap}"
            )));
        }
    }
    Ok(())
}

fn allowed_accounts(default_account: &str) -> Result<BTreeSet<String>> {
    let configured = std::env::var("YACLI_DEVCAT_ALLOWED_ACCOUNTS").ok();
    merge_allowed_accounts(default_account, configured.as_deref())
}

/// Public metadata only: discovery must not fetch secrets from the vault.
pub fn account_schema() -> Result<serde_json::Value> {
    let default = std::env::var("YACLI_DEVCAT_ACCOUNT")
        .map_err(|_| YacliError::Config("DevCat default account unavailable".into()))?;
    let allowed = allowed_accounts(&default)?;
    let store = crate::account_store::AccountStore::load()?;
    for name in &allowed {
        store.get_account(name)?;
    }
    let services: Vec<_> = allowed
        .iter()
        .map(|name| {
            let account = store.file.accounts.get(name);
            let state = |enabled: bool, configured: bool| {
                if !configured {
                    "NOT_CONFIGURED"
                } else if enabled {
                    "configured"
                } else {
                    "disabled"
                }
            };
            match account {
                Some(a) => format!(
                    "{name}: mail={}, disk={}",
                    state(a.mail.enabled, a.mail.credential_ref.is_some()),
                    state(a.disk.enabled, a.disk.credential_ref.is_some())
                ),
                None => format!("{name}: account metadata unavailable"),
            }
        })
        .collect();
    Ok(serde_json::json!({
        "type": "string", "enum": allowed, "default": default,
        "description": format!("Exact account identifier; omitted account defaults to {default}. {}. Configured describes metadata, not current provider authorization.", services.join("; "))
    }))
}

fn merge_allowed_accounts(
    default_account: &str,
    configured: Option<&str>,
) -> Result<BTreeSet<String>> {
    let default_account = default_account.trim();
    if default_account.is_empty() {
        return Err(YacliError::Config(
            "DevCat default account name is empty".into(),
        ));
    }
    let mut accounts = BTreeSet::from([default_account.to_string()]);
    if let Some(configured) = configured {
        for account in configured
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            accounts.insert(account.to_string());
        }
    }
    Ok(accounts)
}

fn capabilities() -> BTreeSet<String> {
    std::env::var("YACLI_DEVCAT_CAPABILITIES")
        .unwrap_or_else(|_| "mail.read,disk.read".into())
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn has_capability(capability: &str) -> bool {
    enabled() && capabilities().contains(capability)
}

fn required_capabilities(name: &str) -> Option<&'static [&'static str]> {
    match name {
        "yacli.mail.folders" | "yacli.mail.list" | "yacli.mail.search" | "yacli.mail.read" => {
            Some(&["mail.read"])
        }
        "yacli.mail.send" | "yacli.mail.send_published_link" => Some(&["mail.send"]),
        "yacli.mail.folder.create"
        | "yacli.mail.copy"
        | "yacli.mail.move"
        | "yacli.mail.flags"
        | "yacli.mail.draft.create" => Some(&["mail.mutate"]),
        "yacli.contacts.list" | "yacli.contacts.search" | "yacli.contacts.export" => {
            Some(&["contacts.read"])
        }
        "yacli.contacts.create"
        | "yacli.contacts.update"
        | "yacli.contacts.delete"
        | "yacli.contacts.import" => Some(&["contacts.write"]),
        "yacli.disk.info"
        | "yacli.disk.list"
        | "yacli.disk.read"
        | "yacli.disk.permissions"
        | "yacli.disk.download"
        | "yacli.disk.trash.list" => Some(&["disk.read"]),
        "yacli.disk.mkdir"
        | "yacli.disk.upload"
        | "yacli.disk.copy"
        | "yacli.disk.move"
        | "yacli.disk.trash.restore" => Some(&["disk.write"]),
        "yacli.disk.trash.move" => Some(&["disk.delete"]),
        "yacli.disk.publish" | "yacli.disk.unpublish" => Some(&["disk.publish"]),
        "yacli.disk.upload_link" => Some(&["disk.write", "disk.publish"]),
        "yacli.mail.send_link" => Some(&["mail.send", "disk.write", "disk.publish"]),
        _ => None,
    }
}

pub fn tool_allowed(name: &str) -> bool {
    if !enabled() {
        return true;
    }
    required_capabilities(name)
        .is_some_and(|required| required.iter().all(|cap| capabilities().contains(*cap)))
}

pub fn check_tool(name: &str, arguments: &serde_json::Value) -> Result<()> {
    if !enabled() {
        return Ok(());
    }
    if !tool_allowed(name) {
        return Err(YacliError::Auth(format!(
            "MCP capability denied for {name}"
        )));
    }
    if let Ok(default_account) = std::env::var("YACLI_DEVCAT_ACCOUNT") {
        let allowed = allowed_accounts(&default_account)?;
        if arguments.get("account").is_some_and(|v| !v.is_string()) {
            return Err(YacliError::Validation(
                "account must be an exact string identifier".into(),
            ));
        }
        if arguments
            .get("account")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|requested| !allowed.contains(requested))
        {
            return Err(YacliError::Auth("MCP account denied".into()));
        }
    }
    if name.starts_with("yacli.disk.")
        && !matches!(
            name,
            "yacli.disk.info"
                | "yacli.disk.trash.list"
                | "yacli.disk.copy"
                | "yacli.disk.move"
                | "yacli.disk.trash.restore"
        )
        && arguments
            .get("path")
            .and_then(serde_json::Value::as_str)
            .is_none()
    {
        return Err(YacliError::Validation(
            "DevCat Disk tool requires explicit path".into(),
        ));
    }
    if name == "yacli.mail.send_link"
        && arguments
            .get("disk_path")
            .and_then(serde_json::Value::as_str)
            .is_none()
    {
        return Err(YacliError::Validation(
            "DevCat send_link requires disk_path".into(),
        ));
    }
    for field in ["path", "disk_path", "from_path", "to_path"] {
        if let Some(path) = arguments.get(field).and_then(serde_json::Value::as_str) {
            if name.starts_with("yacli.disk.") || name == "yacli.mail.send_link" {
                if name == "yacli.disk.trash.list" {
                    crate::disk::validate_private_trash_path(path)?;
                } else {
                    check_disk_path(path)?;
                }
            }
        }
    }
    if name == "yacli.disk.trash.restore" {
        crate::disk::validate_private_trash_path(
            arguments
                .get("trash_path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| YacliError::Validation("Параметр trash_path обязателен".into()))?,
        )?;
        if let Some(name) = arguments
            .get("restore_name")
            .and_then(serde_json::Value::as_str)
        {
            crate::disk::validate_trash_restore_name(name)?;
        }
    }
    if matches!(name, "yacli.disk.download" | "yacli.mail.attachment.export")
        && let Some(path) = arguments
            .get("output_path")
            .and_then(serde_json::Value::as_str)
    {
        check_local_download(path)?;
    }
    for field in ["source", "source_path"] {
        if let Some(path) = arguments.get(field).and_then(serde_json::Value::as_str) {
            check_local_upload(path)?;
        }
    }
    if let Some(paths) = arguments
        .get("attachments")
        .and_then(serde_json::Value::as_array)
    {
        for path in paths {
            check_local_upload(path.as_str().ok_or_else(|| {
                YacliError::Validation("attachment path must be a string".into())
            })?)?;
        }
    }
    if is_risky(name)
        && arguments
            .get("dry_run")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    {
        let supplied = arguments
            .get("review_sha256")
            .and_then(serde_json::Value::as_str);
        let expected = review_hash(name, arguments)?;
        if supplied != Some(expected.as_str()) || !consume_review(&expected) {
            return Err(YacliError::Auth(format!(
                "{name} requires review_sha256 from its dry run"
            )));
        }
    }
    Ok(())
}

fn reviews() -> &'static Mutex<HashMap<String, Instant>> {
    static REVIEWS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    REVIEWS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn record_review(digest: String) {
    if let Ok(mut reviews) = reviews().lock() {
        reviews.retain(|_, expiry| *expiry > Instant::now());
        reviews.insert(digest, Instant::now() + Duration::from_secs(300));
    }
}

fn consume_review(digest: &str) -> bool {
    reviews()
        .lock()
        .ok()
        .and_then(|mut reviews| reviews.remove(digest))
        .is_some_and(|expiry| expiry > Instant::now())
}

pub fn is_risky(name: &str) -> bool {
    matches!(
        name,
        "yacli.mail.send"
            | "yacli.mail.send_link"
            | "yacli.mail.send_published_link"
            | "yacli.mail.folder.create"
            | "yacli.mail.copy"
            | "yacli.mail.move"
            | "yacli.mail.flags"
            | "yacli.mail.draft.create"
            | "yacli.contacts.create"
            | "yacli.contacts.update"
            | "yacli.contacts.delete"
            | "yacli.contacts.import"
            | "yacli.disk.mkdir"
            | "yacli.disk.upload"
            | "yacli.disk.publish"
            | "yacli.disk.unpublish"
            | "yacli.disk.upload_link"
            | "yacli.disk.download"
            | "yacli.disk.copy"
            | "yacli.disk.move"
            | "yacli.disk.trash.move"
            | "yacli.disk.trash.restore"
    )
}

pub fn review_hash(name: &str, arguments: &serde_json::Value) -> Result<String> {
    let mut args = arguments
        .as_object()
        .cloned()
        .ok_or_else(|| YacliError::Validation("tool arguments must be an object".into()))?;
    args.remove("dry_run");
    args.remove("review_sha256");
    let bytes = serde_json::to_vec(&(name, &args))
        .map_err(|err| YacliError::Serialization(err.to_string()))?;
    let mut digest = Sha256::new();
    digest.update(bytes);
    for field in ["source", "source_path"] {
        if let Some(path) = args.get(field).and_then(serde_json::Value::as_str) {
            hash_file(path, &mut digest)?;
        }
    }
    if let Some(paths) = args
        .get("attachments")
        .and_then(serde_json::Value::as_array)
    {
        for path in paths {
            hash_file(
                path.as_str().ok_or_else(|| {
                    YacliError::Validation("attachment path must be a string".into())
                })?,
                &mut digest,
            )?;
        }
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn hash_file(path: &str, digest: &mut Sha256) -> Result<()> {
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0_u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    Ok(())
}

fn check_local_upload(path: &str) -> Result<()> {
    let roots = std::env::var("YACLI_DEVCAT_UPLOAD_ROOTS")
        .map_err(|_| YacliError::Auth("local upload roots are not configured".into()))?;
    let source = std::fs::canonicalize(path)?;
    if !source.is_file() {
        return Err(YacliError::Validation(
            "upload source must be a file".into(),
        ));
    }
    if roots
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .any(|root| {
            std::fs::canonicalize(Path::new(root)).is_ok_and(|root| source.starts_with(root))
        })
    {
        Ok(())
    } else {
        Err(YacliError::Auth(
            "upload source is outside allowed roots".into(),
        ))
    }
}

fn check_local_download(path: &str) -> Result<()> {
    let roots = std::env::var("YACLI_DEVCAT_DOWNLOAD_ROOTS")
        .map_err(|_| YacliError::Auth("Не настроены разрешенные папки для скачивания".into()))?;
    validate_local_download_path(path, &roots)
}

fn validate_local_download_path(path: &str, roots: &str) -> Result<()> {
    let output = Path::new(path);
    let file_name = output
        .file_name()
        .ok_or_else(|| YacliError::Validation("Для сохранения скачивания укажите файл".into()))?;
    if file_name.is_empty() || file_name == "." || file_name == ".." {
        return Err(YacliError::Validation(
            "Имя файла для скачивания недопустимо".into(),
        ));
    }
    let parent = output.parent().unwrap_or_else(|| Path::new("."));
    let canonical_parent = std::fs::canonicalize(parent)?;
    match std::fs::symlink_metadata(output) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(YacliError::Validation(
                "Путь сохранения скачивания не должен быть символической ссылкой".into(),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if roots
        .split(',')
        .map(str::trim)
        .filter(|root| !root.is_empty())
        .any(|root| {
            std::fs::canonicalize(Path::new(root))
                .is_ok_and(|root| canonical_parent.starts_with(root))
        })
    {
        Ok(())
    } else {
        Err(YacliError::Auth(
            "Папка сохранения находится вне разрешенных корневых папок".into(),
        ))
    }
}

pub fn check_disk_path(path: &str) -> Result<()> {
    if !enabled() {
        return Ok(());
    }
    let roots = std::env::var("YACLI_DEVCAT_DISK_ROOTS")
        .map_err(|_| YacliError::Config("YACLI_DEVCAT_DISK_ROOTS is required".into()))?;
    let target = clean_path(path)?;
    if roots
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .any(|root| {
            clean_path(root).is_ok_and(|allowed| {
                allowed == "disk:/" || target == allowed || target.starts_with(&(allowed + "/"))
            })
        })
    {
        Ok(())
    } else {
        Err(YacliError::Auth(
            "Disk path is outside allowed roots".into(),
        ))
    }
}

fn clean_path(path: &str) -> Result<String> {
    let Some(tail) = path.strip_prefix("disk:/") else {
        return Err(YacliError::Validation(
            "Disk path must start with disk:/".into(),
        ));
    };
    if path.contains('%')
        || path.contains('\\')
        || path.chars().any(char::is_control)
        || path.chars().any(|ch| {
            matches!(
                ch,
                '\u{2215}' | '\u{2044}' | '\u{29f8}' | '\u{ff0f}' | '\u{fe68}'
            )
        })
        || path.nfkc().collect::<String>() != path
    {
        return Err(YacliError::Validation(
            "Disk path contains forbidden encoding or separator".into(),
        ));
    }
    let parts: Vec<&str> = tail.split('/').collect();
    if parts
        .iter()
        .any(|part| *part == "." || *part == ".." || part.is_empty() && !tail.is_empty())
    {
        return Err(YacliError::Validation(
            "Disk path contains unsafe segment".into(),
        ));
    }
    Ok(format!("disk:/{}", tail.trim_end_matches('/')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_allowlist_keeps_default_and_exact_additional_accounts() {
        let accounts = merge_allowed_accounts(
            "dedts",
            Some("dedts, ykkareliadom, dvabobra2014, ykkareliadom"),
        )
        .unwrap();
        assert_eq!(
            accounts,
            BTreeSet::from([
                "dedts".to_string(),
                "dvabobra2014".to_string(),
                "ykkareliadom".to_string(),
            ])
        );
        assert!(!accounts.contains("other"));
        assert!(merge_allowed_accounts("   ", Some("dedts")).is_err());
    }

    #[test]
    fn contacts_tools_require_only_the_contacts_read_capability() {
        assert_eq!(
            required_capabilities("yacli.contacts.list"),
            Some(["contacts.read"].as_slice())
        );
        assert_eq!(
            required_capabilities("yacli.contacts.search"),
            Some(["contacts.read"].as_slice())
        );
    }

    #[test]
    fn contact_writes_require_contacts_write_and_are_review_gated() {
        for name in [
            "yacli.contacts.create",
            "yacli.contacts.update",
            "yacli.contacts.delete",
        ] {
            assert_eq!(
                required_capabilities(name),
                Some(["contacts.write"].as_slice())
            );
            assert!(is_risky(name));
        }
    }

    #[test]
    fn mail_mutations_require_capability_and_review() {
        for name in [
            "yacli.mail.folder.create",
            "yacli.mail.copy",
            "yacli.mail.move",
            "yacli.mail.flags",
            "yacli.mail.draft.create",
        ] {
            assert_eq!(
                required_capabilities(name),
                Some(["mail.mutate"].as_slice())
            );
            assert!(is_risky(name));
        }
    }

    #[test]
    fn disk_resource_mutations_use_scoped_capabilities_and_review() {
        for name in [
            "yacli.disk.copy",
            "yacli.disk.move",
            "yacli.disk.trash.restore",
        ] {
            assert_eq!(required_capabilities(name), Some(["disk.write"].as_slice()));
            assert!(is_risky(name));
        }
        assert_eq!(
            required_capabilities("yacli.disk.trash.move"),
            Some(["disk.delete"].as_slice())
        );
        assert!(is_risky("yacli.disk.trash.move"));
        assert_eq!(
            required_capabilities("yacli.disk.trash.list"),
            Some(["disk.read"].as_slice())
        );
        assert!(!is_risky("yacli.disk.trash.list"));
        assert_eq!(
            required_capabilities("yacli.disk.download"),
            Some(["disk.read"].as_slice())
        );
        assert!(is_risky("yacli.disk.download"));
        assert_eq!(
            required_capabilities("yacli.disk.permissions"),
            Some(["disk.read"].as_slice())
        );
        assert!(!is_risky("yacli.disk.permissions"));
    }

    #[test]
    fn local_download_outputs_stay_in_configured_roots_and_reject_symlinks() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let inside_output = root.path().join("download.bin");
        assert!(
            validate_local_download_path(
                inside_output.to_str().expect("path"),
                root.path().to_str().expect("root path")
            )
            .is_ok()
        );
        let outside_output = outside.path().join("download.bin");
        assert!(
            validate_local_download_path(
                outside_output.to_str().expect("path"),
                root.path().to_str().expect("root path")
            )
            .is_err()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let link = root.path().join("link.bin");
            symlink(outside.path().join("target.bin"), &link).expect("symlink");
            assert!(
                validate_local_download_path(
                    link.to_str().expect("path"),
                    root.path().to_str().expect("root path")
                )
                .is_err()
            );
        }
    }

    #[test]
    fn rejects_traversal_forms() {
        for path in [
            "disk:/safe/../private",
            "disk:/safe/%2e%2e/private",
            "disk:/safe\\..\\private",
            "disk:/safe//private",
            "app:/safe",
            "disk:/safe/．．/private",
            "disk:/safe∕../private",
        ] {
            assert!(clean_path(path).is_err(), "{path}");
        }
        assert_eq!(clean_path("disk:/safe/file").unwrap(), "disk:/safe/file");
    }

    #[test]
    fn review_hash_ignores_only_review_fields() {
        let one = serde_json::json!({"path":"disk:/safe/file", "dry_run":true});
        let two =
            serde_json::json!({"path":"disk:/safe/file", "dry_run":false, "review_sha256":"x"});
        let changed = serde_json::json!({"path":"disk:/safe/other"});
        assert_eq!(
            review_hash("yacli.disk.publish", &one).unwrap(),
            review_hash("yacli.disk.publish", &two).unwrap()
        );
        assert_ne!(
            review_hash("yacli.disk.publish", &one).unwrap(),
            review_hash("yacli.disk.publish", &changed).unwrap()
        );
    }

    #[test]
    fn review_can_be_consumed_only_once() {
        let digest = review_hash(
            "yacli.disk.publish",
            &serde_json::json!({"path":"disk:/safe/file"}),
        )
        .unwrap();
        record_review(digest.clone());
        assert!(consume_review(&digest));
        assert!(!consume_review(&digest));
    }

    #[test]
    fn expired_reviews_and_changed_upload_bytes_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.txt");
        std::fs::write(&source, "before").unwrap();
        let args = serde_json::json!({"account":"a", "source":source, "path":"disk:/safe/test"});
        let before = review_hash("yacli.disk.upload", &args).unwrap();
        std::fs::write(&source, "after").unwrap();
        assert_ne!(before, review_hash("yacli.disk.upload", &args).unwrap());
        reviews()
            .lock()
            .unwrap()
            .insert(before.clone(), Instant::now() - Duration::from_secs(1));
        assert!(!consume_review(&before));
    }
}
