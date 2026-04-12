use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
};

pub const COMPONENT_NAME: &str = "tpx-store";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentStore {
    blobs_dir: PathBuf,
}

impl ContentStore {
    pub fn new() -> Result<Self> {
        Ok(Self::with_home(resolve_tpx_home()?))
    }

    pub fn with_home<P: AsRef<Path>>(home: P) -> Self {
        Self {
            blobs_dir: home.as_ref().join("store").join("blobs"),
        }
    }

    pub const fn component_name(&self) -> &'static str {
        COMPONENT_NAME
    }

    pub fn put(&self, data: &[u8]) -> Result<String> {
        fs::create_dir_all(&self.blobs_dir).with_context(|| {
            format!(
                "failed to create blob directory {}",
                self.blobs_dir.display()
            )
        })?;

        let digest = sha256_hex(data);
        let blob_path = self.blob_path(&digest)?;

        if blob_path.exists() {
            return Ok(digest);
        }

        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&blob_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => return Ok(digest),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to create blob {}", blob_path.display()));
            }
        };

        if let Err(error) = file.write_all(data).and_then(|_| file.sync_all()) {
            let _ = fs::remove_file(&blob_path);

            return Err(error)
                .with_context(|| format!("failed to persist blob {}", blob_path.display()));
        }

        Ok(digest)
    }

    pub fn get(&self, digest: &str) -> Result<Vec<u8>> {
        let blob_path = self.blob_path(digest)?;

        fs::read(&blob_path).with_context(|| format!("failed to read blob {}", blob_path.display()))
    }

    pub fn exists(&self, digest: &str) -> bool {
        self.blob_path(digest)
            .map(|blob_path| blob_path.is_file())
            .unwrap_or(false)
    }

    fn blob_path(&self, digest: &str) -> Result<PathBuf> {
        Ok(self.blobs_dir.join(normalize_digest(digest)?))
    }
}

pub fn put(data: &[u8]) -> Result<String> {
    ContentStore::new()?.put(data)
}

pub fn get(digest: &str) -> Result<Vec<u8>> {
    ContentStore::new()?.get(digest)
}

pub fn exists(digest: &str) -> bool {
    ContentStore::new()
        .map(|store| store.exists(digest))
        .unwrap_or(false)
}

fn resolve_tpx_home() -> Result<PathBuf> {
    if let Some(home) = env::var_os("TPX_HOME") {
        return Ok(PathBuf::from(home));
    }

    if let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE")) {
        return Ok(PathBuf::from(home).join(".tpx"));
    }

    Err(anyhow!("failed to resolve TPX home directory"))
}

fn normalize_digest(digest: &str) -> Result<String> {
    let trimmed = digest.trim();
    let hex = trimmed.strip_prefix("sha256:").unwrap_or(trimmed);

    if hex.len() != 64 || !hex.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        bail!("invalid sha256 digest '{digest}'");
    }

    Ok(hex.to_ascii_lowercase())
}

fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    let mut output = String::with_capacity(digest.len() * 2);

    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn exposes_component_name() {
        let store = temp_store();

        assert_eq!(store.component_name(), "tpx-store");

        remove_temp_store(&store);
    }

    #[test]
    fn put_and_get_round_trip_data() {
        let store = temp_store();
        let data = b"kubectl";

        let digest = store.put(data).expect("blob should be written");

        assert_eq!(digest.len(), 64);
        assert!(store.exists(&digest));
        assert_eq!(store.get(&digest).expect("blob should be readable"), data);

        remove_temp_store(&store);
    }

    #[test]
    fn deduplicates_identical_content() {
        let store = temp_store();

        let first_digest = store
            .put(b"same-bytes")
            .expect("first write should succeed");
        let second_digest = store
            .put(b"same-bytes")
            .expect("second write should succeed");

        assert_eq!(first_digest, second_digest);
        assert_eq!(blob_file_count(&store), 1);

        remove_temp_store(&store);
    }

    fn temp_store() -> ContentStore {
        let unique_suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after UNIX epoch")
            .as_nanos();
        let home = env::temp_dir().join(format!("tpx-store-{}-{unique_suffix}", process::id()));

        ContentStore::with_home(home)
    }

    fn blob_file_count(store: &ContentStore) -> usize {
        if !store.blobs_dir.exists() {
            return 0;
        }

        fs::read_dir(&store.blobs_dir)
            .expect("blob directory should be readable")
            .count()
    }

    fn remove_temp_store(store: &ContentStore) {
        let home = store
            .blobs_dir
            .parent()
            .and_then(Path::parent)
            .map(PathBuf::from)
            .expect("temporary home should be derivable from blob path");

        if home.exists() {
            fs::remove_dir_all(home).expect("temporary store directory should be removed");
        }
    }
}
