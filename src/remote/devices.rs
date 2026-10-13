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

    fn save(&self, devices: &[Device]) -> anyhow::Result<()> {
        super::pki::write_private(&self.path, serde_json::to_string_pretty(devices)?.as_bytes())?;
        *self.cache.lock().unwrap() = (None, Vec::new());
        Ok(())
    }

    /// A name for a new phone that no paired one has yet.
    pub fn free_name(&self, wanted: &str) -> String {
        let base: String = wanted.chars().filter(|c| !c.is_control()).take(40).collect::<String>().trim().to_string();
        let base = if base.is_empty() { "phone".to_string() } else { base };
        let taken = self.list();
        let mut name = base.clone();
        let mut n = 2;
        while taken.iter().any(|d| d.name == name) {
            name = format!("{}-{}", base, n);
            n += 1;
        }
        name
    }

    pub fn add(&self, name: &str, sha256: &str) -> anyhow::Result<()> {
        let mut all = self.list();
        all.push(Device {
            name: name.to_string(),
            sha256: sha256.to_string(),
            paired_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        });
        self.save(&all)
    }

    /// Forget a phone. Returns whether it was paired.
    pub fn remove(&self, name: &str) -> anyhow::Result<bool> {
        let mut all = self.list();
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
        assert_eq!(r.free_name("  "), "phone");
        r.add("pixel", "aa").unwrap();
        assert_eq!(r.free_name("pixel"), "pixel-2");
        r.add("pixel-2", "bb").unwrap();
        assert_eq!(r.lookup("bb").as_deref(), Some("pixel-2"));
        // A second registry, as `forkdeck remote revoke` in another process would use.
        assert!(Registry::new(&dir).remove("pixel-2").unwrap());
        assert!(!Registry::new(&dir).remove("pixel-2").unwrap());
        assert_eq!(r.lookup("bb"), None);
        assert_eq!(r.lookup("aa").as_deref(), Some("pixel"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
