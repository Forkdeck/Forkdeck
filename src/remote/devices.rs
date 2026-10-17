//! Paired phones, by the SHA-256 of the certificate each was given. Removing an entry is
//! how a phone is revoked: the file is re-read whenever it changes, so `forkdeck remote
//! revoke` takes effect on a running forkdeck within seconds.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Device {
    pub name: String,
    pub sha256: String,
    pub paired_at: String,
}

pub struct Registry {
    path: PathBuf,
    cache: Mutex<(Option<SystemTime>, Vec<Device>)>,
}

impl Registry {
    pub fn new(dir: &Path) -> Registry {
        Registry { path: dir.join("devices.json"), cache: Mutex::new((None, Vec::new())) }
    }

    pub fn list(&self) -> Vec<Device> {
        let mtime = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        let mut c = self.cache.lock().unwrap();
        if mtime != c.0 || mtime.is_none() {
            c.1 = std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
            c.0 = mtime;
        }
        c.1.clone()
    }

    /// The name of the phone holding the certificate with this fingerprint, while it is paired.
    pub fn lookup(&self, sha256: &str) -> Option<String> {
        self.list().into_iter().find(|d| d.sha256 == sha256).map(|d| d.name)
    }

    /// Every change reads, changes and writes the file under one lock shared with other
    /// processes, so a pairing cannot undo a revocation made meanwhile, nor two pairings lose one.
    fn locked(&self) -> anyhow::Result<(crate::proc::FileLock, Vec<Device>)> {
        let lock = crate::proc::FileLock::wait(&self.path.with_extension("lock"), std::time::Duration::from_secs(10)).map_err(anyhow::Error::msg)?;
        let all = match std::fs::read(&self.path) {
            Ok(b) => serde_json::from_slice(&b)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        Ok((lock, all))
    }

    fn save(&self, devices: &[Device]) -> anyhow::Result<()> {
        super::pki::write_private(&self.path, serde_json::to_string_pretty(devices)?.as_bytes())?;
        *self.cache.lock().unwrap() = (None, Vec::new());
        Ok(())
    }

    /// A name for a new phone that none of `taken` has yet.
    fn free_name(wanted: &str, taken: &[Device]) -> String {
        let base: String = wanted.chars().filter(|c| !c.is_control()).take(40).collect::<String>().trim().to_string();
        let base = if base.is_empty() { "phone".to_string() } else { base };
        let mut name = base.clone();
        let mut n = 2;
        while taken.iter().any(|d| d.name == name) {
            name = format!("{}-{}", base, n);
            n += 1;
        }
        name
    }

    /// Pair a phone: pick a name no paired phone has, `sign` its certificate for that name, and
    /// record it, all under the lock. Returns the name and the certificate.
    pub fn pair(&self, wanted: &str, sign: impl FnOnce(&str) -> anyhow::Result<Vec<u8>>) -> anyhow::Result<(String, Vec<u8>)> {
        let (_lock, mut all) = self.locked()?;
        let name = Self::free_name(wanted, &all);
        let cert = sign(&name)?;
        all.push(Device {
            name: name.clone(),
            sha256: super::pki::sha256_hex(&cert),
            paired_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        });
        self.save(&all)?;
        Ok((name, cert))
    }

    /// Forget a phone. Returns whether it was paired.
    pub fn remove(&self, name: &str) -> anyhow::Result<bool> {
        let (_lock, mut all) = self.locked()?;
        let before = all.len();
        all.retain(|d| d.name != name);
        if all.len() == before {
            return Ok(false);
        }
        self.save(&all)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_unique_and_revoking_forgets_the_certificate() {
        let dir = std::env::temp_dir().join(format!("forkdeck-devices-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = Registry::new(&dir);
        assert_eq!(Registry::free_name("  ", &[]), "phone");
        let (name, a) = r.pair("pixel", |_| Ok(b"aa".to_vec())).unwrap();
        assert_eq!(name, "pixel");
        let (name, b) = r.pair("pixel", |n| Ok(n.as_bytes().to_vec())).unwrap();
        assert_eq!((name.as_str(), b.as_slice()), ("pixel-2", b"pixel-2".as_slice()), "signed for the name it is recorded under");
        let (aa, bb) = (super::super::pki::sha256_hex(&a), super::super::pki::sha256_hex(&b));
        assert_eq!(r.lookup(&bb).as_deref(), Some("pixel-2"));
        // A second registry, as `forkdeck remote revoke` in another process would use.
        assert!(Registry::new(&dir).remove("pixel-2").unwrap());
        assert!(!Registry::new(&dir).remove("pixel-2").unwrap());
        assert_eq!(r.lookup(&bb), None);
        assert_eq!(r.lookup(&aa).as_deref(), Some("pixel"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
