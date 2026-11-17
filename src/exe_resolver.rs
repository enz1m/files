// Immutable absolute search directories prevent later PATH changes from changing helper authority.

use std::path::{Path, PathBuf};

fn is_admitted_path_component(component: &Path) -> bool {
    let s = component.as_os_str();
    !s.is_empty() && component.is_absolute()
}

// The directories searched for an executable, in order.
//
// An empty list resolves nothing, which is how tests make a helper absent
// without changing the process `PATH`. Empty and relative entries are dropped.
#[derive(Debug, Clone, Default)]
pub(crate) struct SearchPath {
    dirs: Vec<PathBuf>,
}

impl SearchPath {
    // A search path over explicit directories; empty and relative entries are
    // dropped. `vec![]` models a missing helper.
    pub(crate) fn new(dirs: Vec<PathBuf>) -> Self {
        Self {
            dirs: dirs
                .into_iter()
                .filter(|d| is_admitted_path_component(d))
                .collect(),
        }
    }

    // Snapshots `$PATH`, dropping empty and relative components. Production
    // calls this once at boot.
    pub(crate) fn from_env() -> Self {
        let raw: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        Self::new(raw)
    }

    // The first executable file named `program` on the search path.
    pub(crate) fn resolve(&self, program: &str) -> Option<PathBuf> {
        self.dirs
            .iter()
            .map(|dir| dir.join(program))
            .find(|candidate| is_executable_file(candidate))
    }

    // Resolves `program` to a [`ResolvedHelper`], or `None` when not found.
    pub(crate) fn resolve_authority(&self, program: &str) -> Option<ResolvedHelper> {
        self.resolve(program).map(|path| ResolvedHelper {
            program: program.to_owned(),
            path,
        })
    }
}

// An executable resolved once to an absolute path, the sole spawn authority;
// the program name is kept for diagnostics.
#[derive(Debug)]
pub(crate) struct ResolvedHelper {
    program: String,
    path: PathBuf,
}

impl ResolvedHelper {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Clone for ResolvedHelper {
    fn clone(&self) -> Self {
        Self {
            program: self.program.clone(),
            path: self.path.clone(),
        }
    }
}

// Whether `p` exists and is a regular file with an execute bit.
pub(crate) fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(p) {
        Ok(md) => md.is_file() && md.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}
