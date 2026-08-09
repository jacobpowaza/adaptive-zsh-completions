use anyhow::{Context, Result};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Envelope<T> {
    created_at: u64,
    expires_at: Option<u64>,
    value: T,
}

#[derive(Debug, Clone, Serialize)]
pub struct CacheStatus {
    pub path: PathBuf,
    pub files: usize,
    pub bytes: u64,
}

impl Cache {
    pub fn new() -> Self {
        let root = std::env::var_os("ADAPTIVE_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                dirs::cache_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("adaptive")
            });
        Self { root }
    }
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn path(&self, namespace: &str, key: &str) -> PathBuf {
        let hash = format!("{:x}", Sha256::digest(key.as_bytes()));
        self.root.join(namespace).join(format!("{hash}.json"))
    }
    pub fn get<T: DeserializeOwned>(&self, namespace: &str, key: &str) -> Result<Option<T>> {
        let path = self.path(namespace, key);
        let raw = match fs::read(&path) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let envelope: Envelope<T> = serde_json::from_slice(&raw)
            .with_context(|| format!("invalid cache {}", path.display()))?;
        if envelope.expires_at.is_some_and(|expiry| expiry < now()) {
            let _ = fs::remove_file(path);
            return Ok(None);
        }
        Ok(Some(envelope.value))
    }
    pub fn put<T: Serialize>(
        &self,
        namespace: &str,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
    ) -> Result<()> {
        let path = self.path(namespace, key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let envelope = Envelope {
            created_at: now(),
            expires_at: ttl.map(|t| now().saturating_add(t.as_secs())),
            value,
        };
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        fs::write(&temp, serde_json::to_vec(&envelope)?)?;
        fs::rename(temp, path)?;
        Ok(())
    }
    pub fn remove_namespace(&self, namespace: &str) -> Result<()> {
        let target = self.root.join(namespace);
        if target.is_dir() {
            fs::remove_dir_all(target)?;
        }
        Ok(())
    }
    pub fn clear(&self) -> Result<()> {
        if self.root.is_dir() {
            fs::remove_dir_all(&self.root)?;
        }
        Ok(())
    }
    pub fn status(&self) -> Result<CacheStatus> {
        let mut files = 0;
        let mut bytes = 0;
        visit(&self.root, &mut |m| {
            files += 1;
            bytes += m.len();
        })?;
        Ok(CacheStatus {
            path: self.root.clone(),
            files,
            bytes,
        })
    }
    pub fn prune(&self, max_age: Duration, max_bytes: u64) -> Result<usize> {
        let mut entries = Vec::new();
        collect(&self.root, &mut entries)?;
        entries.sort_by_key(|(_, m)| m.modified().unwrap_or(UNIX_EPOCH));
        let cutoff = SystemTime::now().checked_sub(max_age).unwrap_or(UNIX_EPOCH);
        let mut total: u64 = entries.iter().map(|(_, m)| m.len()).sum();
        let mut removed = 0;
        for (path, meta) in entries {
            if meta.modified().unwrap_or(UNIX_EPOCH) < cutoff || total > max_bytes {
                total = total.saturating_sub(meta.len());
                fs::remove_file(path)?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn visit(root: &Path, f: &mut impl FnMut(&fs::Metadata)) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for e in fs::read_dir(root)? {
        let e = e?;
        let m = e.metadata()?;
        if m.is_dir() {
            visit(&e.path(), f)?
        } else {
            f(&m)
        }
    }
    Ok(())
}
fn collect(root: &Path, out: &mut Vec<(PathBuf, fs::Metadata)>) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for e in fs::read_dir(root)? {
        let e = e?;
        let m = e.metadata()?;
        if m.is_dir() {
            collect(&e.path(), out)?
        } else {
            out.push((e.path(), m))
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ttl_invalidates_values() {
        let d = tempfile::tempdir().unwrap();
        let c = Cache::at(d.path().into());
        c.put("x", "k", &42, Some(Duration::ZERO)).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        assert_eq!(c.get::<i32>("x", "k").unwrap(), None);
    }
}
