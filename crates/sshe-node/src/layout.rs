//! Files derived from one config path, so separate instances stay self-contained.
use std::path::{Path, PathBuf};

pub(crate) struct Layout {
    pub(crate) config: PathBuf,
    /// Identity written by `init`; the config may point elsewhere.
    pub(crate) default_identity: PathBuf,
    /// Daemon IPC socket; its directory must be private.
    pub(crate) socket: PathBuf,
}

impl Layout {
    pub(crate) fn new(config: &Path) -> Self {
        Self {
            config: config.to_path_buf(),
            default_identity: config.with_extension("key"),
            socket: config.with_extension("sock"),
        }
    }
    pub(crate) fn dir(&self) -> &Path {
        self.config.parent().unwrap_or(Path::new("."))
    }
}

/// The lock sits beside the identity it guards, wherever the config points it.
pub(crate) fn identity_lock(identity: &Path) -> PathBuf {
    identity.with_extension("lock")
}
