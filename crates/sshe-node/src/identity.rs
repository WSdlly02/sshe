//! Persistent endpoint keys and exclusive identity ownership.
use crate::{Error, Result, layout::identity_lock};
use iroh::SecretKey;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

pub(crate) fn initialize(path: &Path) -> Result<SecretKey> {
    let dir = path
        .parent()
        .ok_or_else(|| Error::Invalid("identity parent missing".into()))?;
    let key = if path.exists() {
        load_key(path)?
    } else {
        let key = SecretKey::generate();
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        file.write_all(&key.to_bytes())?;
        file.as_file().sync_all()?;
        file.persist_noclobber(path)
            .map_err(|e| Error::Io(e.error))?;
        key
    };
    Ok(key)
}

pub(crate) fn load_key(path: &Path) -> Result<SecretKey> {
    let meta = fs::metadata(path)?;
    if meta.permissions().mode() & 0o077 != 0 {
        return Err(Error::Invalid(
            "identity must be private (chmod 600)".into(),
        ));
    }
    let bytes: [u8; 32] = fs::read(path)?
        .try_into()
        .map_err(|_| Error::Invalid("invalid identity length; refusing to replace key".into()))?;
    Ok(SecretKey::from_bytes(&bytes))
}

/// Held for as long as an Endpoint uses this identity.
pub(crate) fn lock(identity: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(identity_lock(identity))?;
    file.try_lock().map_err(Error::IdentityBusy)?;
    Ok(file)
}
