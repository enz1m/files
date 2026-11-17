// The scripted transport pauses at an exact chunk boundary without a scheduling race.

use super::*;

// A temp directory that removes itself, so the suite leaves no litter.
pub(super) struct TempDir(pub(super) PathBuf);

impl TempDir {
    pub(super) fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "files-app-1253-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        Self(base)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
