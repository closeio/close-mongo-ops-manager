//! Persistent configuration at `~/.config/close-mongo-ops-manager/config.json`,
//! compatible with the file written by the Python version.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::theme;

const APP_DIR: &str = "close-mongo-ops-manager";
const FILE_NAME: &str = "config.json";

/// Reads and writes the configuration file.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    /// The store at the default location: `$XDG_CONFIG_HOME` or `~/.config`.
    pub fn default_location() -> Option<Self> {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(Self::at(base.join(APP_DIR).join(FILE_NAME)))
    }

    /// A store backed by `path`.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The saved theme name, if any.
    pub fn load_theme(&self) -> io::Result<Option<String>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let data = self.read()?;
        Ok(data
            .get("theme")
            .and_then(|t| t.get("current_theme"))
            .and_then(Value::as_str)
            .map(str::to_owned))
    }

    /// Saves the theme, keeping every other key of the file.
    pub fn save_theme(&self, name: &str) -> io::Result<()> {
        let mut data = if self.path.exists() {
            self.read()?
        } else {
            Map::new()
        };
        let available: Vec<&str> = theme::all().iter().map(|t| t.name).collect();
        data.insert(
            "theme".to_owned(),
            json!({ "current_theme": name, "available_themes": available }),
        );
        self.write(&Value::Object(data))
    }

    fn read(&self) -> io::Result<Map<String, Value>> {
        let text = fs::read_to_string(&self.path)?;
        match serde_json::from_str(&text) {
            Ok(Value::Object(map)) => Ok(map),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "configuration is not a JSON object",
            )),
            Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
        }
    }

    /// Writes atomically: to a temporary file next to the target, then renamed.
    /// A symlinked file (e.g. managed with dotfiles) is written through: its
    /// target is replaced, not the link. The replaced file keeps its
    /// permissions; a new one gets the default mode (0666 minus the umask).
    fn write(&self, value: &Value) -> io::Result<()> {
        let path = self.resolve_target()?;
        let dir = match path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };
        let mut text = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
        text.push('\n');
        let mut builder = tempfile::Builder::new();
        builder.prefix(".config.json.");
        // Temporary files are owner-only by default.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(fs::Permissions::from_mode(0o666));
        }
        let mut file = builder.tempfile_in(dir)?;
        if let Ok(metadata) = fs::metadata(&path) {
            file.as_file().set_permissions(metadata.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        file.persist(&path).map(drop).map_err(|e| e.error)
    }

    /// The file to replace: `path`, or the file it links to. A dangling link
    /// is followed (at most 8 hops) so that the write creates its target, and
    /// the target's directory is created when missing.
    fn resolve_target(&self) -> io::Result<PathBuf> {
        if let Ok(target) = fs::canonicalize(&self.path) {
            return Ok(target);
        }
        let mut path = self.path.clone();
        for _ in 0..8 {
            let Ok(link) = fs::read_link(&path) else {
                break;
            };
            path = match path.parent() {
                Some(dir) if !dir.as_os_str().is_empty() => dir.join(link),
                _ => link,
            };
        }
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::at(dir.path().join("nested").join("config.json"));
        (dir, store)
    }

    #[test]
    fn missing_file_has_no_theme() {
        let (_dir, store) = store();
        assert_eq!(store.load_theme().unwrap(), None);
    }

    #[test]
    fn save_and_load_round_trip() {
        let (_dir, store) = store();
        store.save_theme("nord").unwrap();
        assert_eq!(store.load_theme().unwrap().as_deref(), Some("nord"));
        let text = fs::read_to_string(store.path()).unwrap();
        assert!(text.contains("available_themes"));
    }

    #[test]
    fn save_preserves_other_keys() {
        let (_dir, store) = store();
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        fs::write(
            store.path(),
            r#"{"other": {"x": 1}, "theme": {"current_theme": "nord"}}"#,
        )
        .unwrap();
        store.save_theme("dracula").unwrap();
        let value: Value =
            serde_json::from_str(&fs::read_to_string(store.path()).unwrap()).unwrap();
        assert_eq!(value["other"]["x"], 1);
        assert_eq!(value["theme"]["current_theme"], "dracula");
    }

    #[test]
    fn invalid_json_is_an_error_and_is_not_overwritten() {
        let (_dir, store) = store();
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        fs::write(store.path(), "{not json").unwrap();
        assert!(store.load_theme().is_err());
        assert!(store.save_theme("nord").is_err());
        assert_eq!(fs::read_to_string(store.path()).unwrap(), "{not json");
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_a_symlink() {
        let (dir, store) = store();
        let real = dir.path().join("dotfiles-config.json");
        fs::write(&real, r#"{"keep": true}"#).unwrap();
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&real, store.path()).unwrap();
        store.save_theme("gruvbox").unwrap();
        assert!(
            fs::symlink_metadata(store.path())
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let value: Value = serde_json::from_str(&fs::read_to_string(&real).unwrap()).unwrap();
        assert_eq!(value["theme"]["current_theme"], "gruvbox");
        assert_eq!(value["keep"], true);

        // Also when the symlink target does not exist yet.
        let fresh = dir.path().join("fresh-dotfiles.json");
        let linked = ConfigStore::at(dir.path().join("linked.json"));
        std::os::unix::fs::symlink(&fresh, linked.path()).unwrap();
        linked.save_theme("nord").unwrap();
        assert!(
            fs::symlink_metadata(linked.path())
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(linked.load_theme().unwrap().as_deref(), Some("nord"));
        assert!(fresh.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn save_keeps_the_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, store) = store();
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        fs::write(store.path(), "{}").unwrap();
        fs::set_permissions(store.path(), fs::Permissions::from_mode(0o664)).unwrap();
        store.save_theme("nord").unwrap();
        let mode = fs::metadata(store.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o664);
    }

    #[test]
    fn reads_python_written_file() {
        let (_dir, store) = store();
        fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        fs::write(
            store.path(),
            r#"{
  "theme": {
    "current_theme": "close-mongodb",
    "available_themes": ["textual-dark", "nord"]
  }
}"#,
        )
        .unwrap();
        assert_eq!(
            store.load_theme().unwrap().as_deref(),
            Some("close-mongodb")
        );
    }
}
