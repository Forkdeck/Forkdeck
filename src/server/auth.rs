use sha2::{Digest, Sha256};
use std::{collections::HashMap, time::{Duration, Instant}};

pub struct Auth {
    code: Option<([u8; 32], Instant)>,
    sessions: HashMap<[u8; 32], Instant>,
}

fn hash(token: &str) -> [u8; 32] { Sha256::digest(token.as_bytes()).into() }

fn token() -> anyhow::Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| anyhow::anyhow!("randomness unavailable: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

impl Auth {
    pub fn new() -> anyhow::Result<(Self, String)> {
        let code = token()?;
        Ok((Self { code: Some((hash(&code), Instant::now() + Duration::from_secs(120))), sessions: HashMap::new() }, code))
    }

    pub fn exchange(&mut self, code: &str) -> Option<String> {
        let (expected, expires) = self.code.as_ref()?;
        if *expires <= Instant::now() || hash(code) != *expected { return None; }
        let session = token().ok()?;
        self.code = None;
        self.sessions.retain(|_, until| *until > Instant::now());
        self.sessions.insert(hash(&session), Instant::now() + Duration::from_secs(12 * 60 * 60));
        Some(session)
    }

    pub fn valid(&self, token: &str) -> bool {
        self.sessions.get(&hash(token)).is_some_and(|until| *until > Instant::now())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn login_code_is_one_use_and_expires() {
        let (mut auth, code) = Auth::new().unwrap();
        assert!(auth.exchange("wrong").is_none());
        let session = auth.exchange(&code).unwrap();
        assert!(auth.valid(&session));
        assert!(auth.exchange(&code).is_none());
        let (mut auth, code) = Auth::new().unwrap();
        auth.code.as_mut().unwrap().1 = Instant::now();
        assert!(auth.exchange(&code).is_none());
    }
}
