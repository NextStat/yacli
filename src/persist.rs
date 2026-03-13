use std::fs;
use std::io::Write;
use std::path::Path;

use tempfile::NamedTempFile;

use crate::error::{Result, YacliError};

pub fn write_config_file(path: &Path, contents: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| YacliError::Io(format!("{} has no parent directory", path.display())))?;
    fs::create_dir_all(parent)?;
    harden_config_dir(parent)?;

    let mut temp = NamedTempFile::new_in(parent)?;
    temp.as_file_mut().write_all(contents.as_bytes())?;
    temp.as_file_mut().flush()?;
    temp.as_file_mut().sync_all()?;
    harden_file_permissions(temp.path())?;
    temp.persist(path)
        .map_err(|err| YacliError::Io(err.to_string()))?;
    harden_file_permissions(path)?;
    sync_dir(parent)?;
    Ok(())
}

fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let dir = fs::File::open(path)?;
        dir.sync_all()?;
    }

    #[cfg(not(unix))]
    let _ = path;

    Ok(())
}

fn harden_config_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }

    Ok(())
}

fn harden_file_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use tempfile::tempdir;

    use super::*;

    static PERSIST_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn write_config_file_replaces_contents_atomically() {
        let _guard = PERSIST_TEST_LOCK.lock().expect("persist test lock");
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("config").join("settings.toml");

        write_config_file(&path, "version = 1\n").expect("initial save");
        write_config_file(&path, "version = 2\n").expect("replacement save");

        assert_eq!(
            fs::read_to_string(&path).expect("saved config"),
            "version = 2\n"
        );
    }

    #[test]
    fn write_config_file_creates_parent_directory() {
        let _guard = PERSIST_TEST_LOCK.lock().expect("persist test lock");
        let temp = tempdir().expect("tempdir");
        let path = temp.path().join("nested").join("yacli").join("config.toml");

        write_config_file(&path, "enabled = true\n").expect("save");

        assert!(path.exists());
        assert!(path.parent().expect("parent").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_config_file_hardens_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = PERSIST_TEST_LOCK.lock().expect("persist test lock");
        let temp = tempdir().expect("tempdir");
        let dir = temp.path().join("secure");
        let path = dir.join("credentials.toml");

        write_config_file(&path, "secret = true\n").expect("save");

        let dir_mode = fs::metadata(&dir)
            .expect("dir metadata")
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(&path)
            .expect("file metadata")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }
}
