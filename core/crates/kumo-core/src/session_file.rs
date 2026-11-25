use std::fs;
use std::io::Write;
use std::path::PathBuf;

use crate::auth::{AuthSession, SessionStore};
use crate::error::CoreError;

pub struct FileSessionStore {
    path: PathBuf,
}

impl FileSessionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        let home = std::env::home_dir().unwrap_or_default();
        home.join(".config").join("kumowatch").join("session.json")
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl SessionStore for FileSessionStore {
    fn load(&self) -> Option<AuthSession> {
        let text = fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn save(&self, session: &AuthSession) -> crate::error::Result<()> {
        let storage = |e: std::io::Error| CoreError::Storage(e.to_string());
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir).map_err(storage)?;
        }
        // Unique per save: kumo-mcp processes sharing this file may refresh the token at once.
        static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = self
            .path
            .with_extension(format!("{}-{n}.tmp", std::process::id()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp).map_err(storage)?;
        let written = file
            .write_all(serde_json::to_string_pretty(session)?.as_bytes())
            .and_then(|()| fs::rename(&tmp, &self.path));
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written.map_err(storage)
    }

    fn clear(&self) -> crate::error::Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CoreError::Storage(e.to_string())),
        }
    }
}
