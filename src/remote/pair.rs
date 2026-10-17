//! Pairing codes and the subnet rule.
//!
//! A code is made by `forkdeck remote pair`, works once, expires, and is only accepted from
//! the subnet it was made for: the phone has to be on the same network as this machine.

use forkdeck_remote_proto::in_subnet;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// One address of one network interface.
#[derive(Clone, Debug, PartialEq)]
pub struct Iface {
    pub name: String,
    pub ip: IpAddr,
    pub prefix: u8,
}

pub fn interfaces() -> Vec<Iface> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|i| {
            let (ip, prefix) = match i.addr {
                if_addrs::IfAddr::V4(a) => (IpAddr::V4(a.ip), a.prefixlen),
                if_addrs::IfAddr::V6(a) => (IpAddr::V6(a.ip), a.prefixlen),
            };
            Iface { name: i.name, ip, prefix }
        })
        .collect()
}

fn canon(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Did `peer` reach us from the subnet of the interface that took the connection?
pub fn same_subnet(local: IpAddr, peer: IpAddr, ifaces: &[Iface]) -> bool {
    let local = canon(local);
    ifaces.iter().any(|i| i.ip == local && in_subnet(canon(peer), i.ip, i.prefix))
}

/// The address a code is made for: the named interface, or the first private IPv4 address.
pub fn pick(ifaces: &[Iface], name: Option<&str>) -> Option<Iface> {
    match name {
        Some(n) => ifaces.iter().find(|i| i.name == n && i.ip.is_ipv4()).or_else(|| ifaces.iter().find(|i| i.name == n)),
        None => ifaces.iter().find(|i| matches!(i.ip, IpAddr::V4(v4) if v4.is_private())),
    }
    .cloned()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Pending {
    token_sha256: String,
    expires: i64,
    ip: IpAddr,
    prefix: u8,
}

fn pending_path(dir: &Path) -> PathBuf {
    dir.join("pending.json")
}

fn load(dir: &Path) -> Vec<Pending> {
    std::fs::read(pending_path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Codes are made and used up under one lock shared with other processes: a code is used once,
/// and making a code never brings back one that was used meanwhile.
fn lock(dir: &Path) -> Result<crate::proc::FileLock, String> {
    crate::proc::FileLock::wait(&dir.join("pending.lock"), std::time::Duration::from_secs(10))
}

fn save(dir: &Path, all: &[Pending]) -> anyhow::Result<()> {
    super::pki::write_private(&pending_path(dir), serde_json::to_string(all)?.as_bytes())
}

fn hash(token: &str) -> String {
    super::pki::sha256_hex(token.as_bytes())
}

/// Make a code for `iface`, valid for `ttl_secs`. Returns the token.
pub fn issue(dir: &Path, iface: &Iface, ttl_secs: u64) -> anyhow::Result<String> {
    let mut raw = [0u8; 16];
    getrandom::getrandom(&mut raw).map_err(|e| anyhow::anyhow!("no randomness: {}", e))?;
    let token: String = raw.iter().map(|b| format!("{:02x}", b)).collect();
    let now = chrono::Utc::now().timestamp();
    let _lock = lock(dir).map_err(anyhow::Error::msg)?;
    let mut all: Vec<Pending> = load(dir).into_iter().filter(|p| p.expires > now).collect();
    all.push(Pending { token_sha256: hash(&token), expires: now + ttl_secs as i64, ip: iface.ip, prefix: iface.prefix });
    save(dir, &all)?;
    Ok(token)
}

/// Use up a code presented by `peer`. A wrong subnet uses it up too: the code was seen
/// somewhere it should not have been.
pub fn redeem(dir: &Path, token: &str, peer: IpAddr) -> Result<(), String> {
    let now = chrono::Utc::now().timestamp();
    let _lock = lock(dir)?;
    let all = load(dir);
    let h = hash(token);
    let found = all.iter().find(|p| p.token_sha256 == h).cloned();
    let rest: Vec<Pending> = all.into_iter().filter(|p| p.token_sha256 != h && p.expires > now).collect();
    save(dir, &rest).map_err(|e| format!("could not update pairing codes: {}", e))?;
    let p = found.ok_or("unknown or already used pairing code")?;
    if p.expires <= now {
        return Err("pairing code expired, make a new one with `forkdeck remote pair`".into());
    }
    if !in_subnet(canon(peer), p.ip, p.prefix) {
        return Err("pairing must come from the same subnet as forkdeck".into());
    }
    Ok(())
}

pub fn qr(text: &str) -> anyhow::Result<String> {
    let code = qrcode::QrCode::new(text.as_bytes())?;
    Ok(code.render::<qrcode::render::unicode::Dense1x2>().quiet_zone(true).build())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn lan() -> Vec<Iface> {
        vec![
            Iface { name: "lo".into(), ip: ip("127.0.0.1"), prefix: 8 },
            Iface { name: "wlan0".into(), ip: ip("192.168.1.20"), prefix: 24 },
            Iface { name: "eth1".into(), ip: ip("10.20.0.5"), prefix: 16 },
        ]
    }

    #[test]
    fn only_the_subnet_of_the_receiving_interface_counts() {
        let ifs = lan();
        assert!(same_subnet(ip("192.168.1.20"), ip("192.168.1.77"), &ifs));
        // Same machine, but the phone came in through the other interface's subnet.
        assert!(!same_subnet(ip("192.168.1.20"), ip("10.20.3.3"), &ifs));
        assert!(same_subnet(ip("10.20.0.5"), ip("10.20.3.3"), &ifs));
        // Routed from elsewhere.
        assert!(!same_subnet(ip("192.168.1.20"), ip("192.168.7.9"), &ifs));
        // A listener on [::] sees IPv4 peers as mapped addresses.
        assert!(same_subnet(ip("::ffff:192.168.1.20"), ip("::ffff:192.168.1.77"), &ifs));
        // An address that is not ours.
        assert!(!same_subnet(ip("192.168.1.21"), ip("192.168.1.77"), &ifs));
    }

    #[test]
    fn a_code_works_once_and_only_from_its_subnet() {
        let dir = std::env::temp_dir().join(format!("forkdeck-pair-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wlan = pick(&lan(), None).unwrap();
        assert_eq!(wlan.name, "wlan0");
        assert_eq!(pick(&lan(), Some("eth1")).unwrap().ip, ip("10.20.0.5"));

        let t = issue(&dir, &wlan, 120).unwrap();
        assert!(redeem(&dir, "nope", ip("192.168.1.77")).is_err());
        assert_eq!(redeem(&dir, &t, ip("192.168.1.77")), Ok(()));
        assert!(redeem(&dir, &t, ip("192.168.1.77")).unwrap_err().contains("already used"));

        let t = issue(&dir, &wlan, 120).unwrap();
        assert!(redeem(&dir, &t, ip("10.20.3.3")).unwrap_err().contains("same subnet"));
        assert!(redeem(&dir, &t, ip("192.168.1.77")).is_err(), "a code tried from elsewhere is gone");

        let t = issue(&dir, &wlan, 0).unwrap();
        assert!(redeem(&dir, &t, ip("192.168.1.77")).unwrap_err().contains("expired"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_code_redeemed_at_once_by_many_works_for_one() {
        let dir = std::env::temp_dir().join(format!("forkdeck-pair-race-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = issue(&dir, &pick(&lan(), None).unwrap(), 120).unwrap();
        let start = std::sync::Arc::new(std::sync::Barrier::new(8));
        let tries: Vec<_> = (0..8).map(|_| {
            let (dir, t, start) = (dir.clone(), t.clone(), start.clone());
            std::thread::spawn(move || { start.wait(); redeem(&dir, &t, ip("192.168.1.77")).is_ok() })
        }).collect();
        let won = tries.into_iter().map(|h| h.join().unwrap()).filter(|ok| *ok).count();
        assert_eq!(won, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_code_renders_as_a_qr() {
        assert!(qr("forkdeck://pair?h=1.2.3.4").unwrap().lines().count() > 10);
    }
}
