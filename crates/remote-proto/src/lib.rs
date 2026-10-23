//! Wire types for forkdeck's remote control.
//!
//! One TLS connection carries both unary calls and the event stream, framed by
//! `fory_rpc_streaming`'s codec. Calls and their answers are the [`Call`] and
//! [`Reply`] enums; the event stream is opened with [`Call::Subscribe`] and
//! carries [`Reply::Event`] items. Both ends must build their `Fory` with
//! [`fory()`] so the registration ids agree.

#[cfg(feature = "client")]
pub mod client;

use std::net::IpAddr;
use std::sync::Arc;

use fory::{Fory, ForyObject};
use fory_rpc_streaming::{register_stream_types, StreamingClientCodec, StreamingServerCodec};
use tarpc_fory::envelope::{
    ForyClientMessage, ForyRequest, ForyResponse, ForyServerError, ForyTraceContext,
};

/// Bumped on any change to the types below. `Hello` refuses a mismatch.
pub const PROTO_VERSION: u32 = 1;

/// `method_id` of the only stream, opened with [`Call::Subscribe`].
pub const SUBSCRIBE: u32 = 1;

pub type ServerCodec = StreamingServerCodec<Call, Reply>;
pub type ClientCodec = StreamingClientCodec<Call, Reply>;

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    #[default]
    Auto,
    Claude,
    Codex,
}

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    #[default]
    Queued,
    Routing,
    Working,
    Reviewing,
    Fixing,
    Publishing,
    Ci,
    Deploying,
    RollingBack,
    Done,
    Failed,
    Cancelled,
}

impl Status {
    pub fn finished(self) -> bool {
        matches!(self, Status::Done | Status::Failed | Status::Cancelled)
    }
}

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum StageState {
    #[default]
    Idle,
    Running,
    Ok,
    Failed,
    Skipped,
}

/// Which box of a job's tree a stage event is about.
#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    #[default]
    Worker,
    Reviewer,
}

/// Board fields projected from a job, shared by every browser viewport.
#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, Debug, Default, Clone, PartialEq)]
pub struct BoardProjection {
    pub lane: String,
    pub state: String,
    pub meta: String,
    pub elapsed: String,
    pub route: String,
    pub stages: Vec<u8>,
    pub log: Vec<Vec<String>>,
    pub warn: String,
}

/// One row of the job list.
#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
pub struct JobSummary {
    pub id: u32,
    pub title: String,
    /// "manual", "issue", "pr" or "deploy".
    pub kind: String,
    /// What the job came from, for display: "owner/repo#12", a short sha, or empty.
    pub origin: String,
    pub url: String,
    pub status: Status,
    /// Chosen agent once routed, else empty.
    pub agent: String,
    pub tier: String,
    /// When the job (or its latest reply) started, Unix seconds.
    pub created_unix: u64,
    /// 0 = no verdict yet, 1 = pass, 2 = fail.
    pub verdict: u8,
    pub pr_url: String,
    pub rounds: u32,
    /// A finished typed task: [`Call::Reply`] continues it.
    pub can_reply: bool,
    pub outcome: String,
}

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
pub struct StageView {
    pub agent: String,
    pub label: String,
    pub model: String,
    pub effort: String,
    pub state: StageState,
    pub last: String,
    pub tools: u32,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost: f64,
    pub secs: u64,
    /// Lines ever pushed to this stage's tail; the tail holds the newest ones.
    pub pushed: u64,
}

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
pub struct GateView {
    pub name: String,
    pub answer: String,
    pub value: f64,
    pub sharp: bool,
}

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
pub struct DecisionView {
    pub agent: String,
    pub tier: String,
    pub review: bool,
    pub gates: Vec<GateView>,
    pub source: String,
    pub ms: u64,
    pub note: String,
}

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
pub struct TurnView {
    pub ask: String,
    pub answer: String,
}

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
pub struct JobDetail {
    pub summary: JobSummary,
    pub prompt: String,
    pub has_decision: bool,
    pub decision: DecisionView,
    pub worker: StageView,
    pub reviewer: StageView,
    pub worker_tail: Vec<String>,
    pub reviewer_tail: Vec<String>,
    pub turns: Vec<TurnView>,
    /// 0 = not run, 1 = passed, 2 = failed.
    pub checks: u8,
    pub findings: Vec<String>,
}

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
pub struct LogLine {
    /// Position in the session log, counting every line ever logged.
    pub seq: u64,
    pub time: String,
    pub who: String,
    pub msg: String,
}

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
pub struct RepoView {
    pub name: String,
    pub label: String,
    pub last_error: String,
}

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
pub struct Snapshot {
    /// Seq of the last event folded into this snapshot; subscribe from here.
    pub seq: u64,
    pub automation: bool,
    pub stopping: bool,
    pub have_claude: bool,
    pub have_codex: bool,
    pub jobs: Vec<JobSummary>,
    pub log: Vec<LogLine>,
    pub repos: Vec<RepoView>,
}

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
pub struct ServerInfo {
    pub proto: u32,
    pub version: String,
    pub host: String,
    pub device: String,
}

#[derive(ts_rs::TS, serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
pub enum Event {
    /// The client missed events: fetch a [`Call::Snapshot`] and start over.
    #[default]
    Resync,
    Job { job: JobSummary },
    Stage { id: u32, which: Which, stage: StageView },
    /// New tail lines; `from` is the `pushed` count before the first of them.
    Tail { id: u32, which: Which, from: u64, lines: Vec<String> },
    Log { lines: Vec<LogLine> },
    Automation { on: bool },
}

#[derive(serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
pub enum Call {
    #[default]
    Nop,
    Hello { proto: u32, app: String },
    Snapshot,
    Job { id: u32 },
    Submit { prompt: String, agent: Agent },
    Reply { id: u32, text: String },
    Cancel { id: u32 },
    SetAutomation { on: bool },
    PollNow,
    /// Stream: events after `since` (0 = only new ones).
    Subscribe { since: u64 },
    /// The only call allowed without a client certificate.
    Pair { token: String, csr: Vec<u8>, device: String },
}

#[derive(serde::Serialize, serde::Deserialize, ForyObject, Debug, Default, Clone, PartialEq)]
// A wire type: one large variant (a snapshot) is fine, and boxing it would change its API.
#[allow(clippy::large_enum_variant)]
pub enum Reply {
    #[default]
    Ok,
    Err { msg: String },
    Hello { info: ServerInfo },
    Snapshot { snap: Snapshot },
    Job { found: bool, detail: JobDetail },
    Submitted { id: u32 },
    Paired { cert: Vec<u8>, ca: Vec<u8> },
    Event { seq: u64, event: Event },
}

/// A `Fory` with every wire type registered under fixed ids.
pub fn fory() -> Arc<Fory> {
    fn build() -> Result<Fory, fory::Error> {
        let mut f = Fory::default();
        // tarpc-fory's envelope ids, registered by hand because
        // `register_envelope_types` assumes one type for requests and responses.
        f.register_serializer::<ForyTraceContext>(2)?;
        f.register_serializer::<ForyServerError>(3)?;
        f.register_serializer::<ForyRequest<Call>>(5)?;
        f.register_serializer::<ForyResponse<Reply>>(6)?;
        f.register_serializer::<ForyClientMessage<Call>>(7)?;
        f.register::<Agent>(100)?;
        f.register::<Status>(101)?;
        f.register::<StageState>(102)?;
        f.register::<Which>(103)?;
        f.register::<JobSummary>(104)?;
        f.register::<StageView>(105)?;
        f.register::<GateView>(106)?;
        f.register::<DecisionView>(107)?;
        f.register::<TurnView>(108)?;
        f.register::<JobDetail>(109)?;
        f.register::<LogLine>(110)?;
        f.register::<RepoView>(111)?;
        f.register::<Snapshot>(112)?;
        f.register::<ServerInfo>(113)?;
        f.register::<Event>(114)?;
        f.register::<Call>(115)?;
        f.register::<Reply>(116)?;
        register_stream_types::<Call>(&mut f)?;
        register_stream_types::<Reply>(&mut f)?;
        Ok(f)
    }
    Arc::new(build().expect("wire types register"))
}

/// What the pairing QR code carries.
#[derive(Debug, Clone, PartialEq)]
pub struct PairInfo {
    pub host: IpAddr,
    pub port: u16,
    /// SHA-256 of the CA certificate (DER), lowercase hex.
    pub ca_sha256: String,
    pub token: String,
    /// Prefix length of the server's subnet on `host`.
    pub prefix: u8,
}

impl PairInfo {
    pub fn to_uri(&self) -> String {
        format!(
            "forkdeck://pair?h={}&p={}&fp={}&t={}&n={}",
            self.host, self.port, self.ca_sha256, self.token, self.prefix
        )
    }

    pub fn parse(uri: &str) -> Result<PairInfo, String> {
        let query = uri
            .trim()
            .strip_prefix("forkdeck://pair?")
            .ok_or("not a forkdeck pairing code")?;
        let (mut host, mut port, mut fp, mut token, mut prefix) = (None, None, None, None, None);
        for kv in query.split('&') {
            let (k, v) = kv.split_once('=').ok_or("malformed pairing code")?;
            match k {
                "h" => host = Some(v.parse::<IpAddr>().map_err(|_| "bad host in pairing code")?),
                "p" => port = Some(v.parse::<u16>().map_err(|_| "bad port in pairing code")?),
                "fp" => fp = Some(v.to_ascii_lowercase()),
                "t" => token = Some(v.to_string()),
                "n" => prefix = Some(v.parse::<u8>().map_err(|_| "bad subnet in pairing code")?),
                _ => {}
            }
        }
        let info = PairInfo {
            host: host.ok_or("pairing code has no host")?,
            port: port.ok_or("pairing code has no port")?,
            ca_sha256: fp.ok_or("pairing code has no fingerprint")?,
            token: token.ok_or("pairing code has no token")?,
            prefix: prefix.ok_or("pairing code has no subnet")?,
        };
        let max = if info.host.is_ipv4() { 32 } else { 128 };
        if info.prefix > max {
            return Err("bad subnet in pairing code".into());
        }
        if info.ca_sha256.len() != 64 || !info.ca_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("bad fingerprint in pairing code".into());
        }
        Ok(info)
    }
}

/// Whether `ip` is in the subnet `net/prefix`. Addresses of different
/// families never match; IPv4-mapped IPv6 addresses count as IPv4.
pub fn in_subnet(ip: IpAddr, net: IpAddr, prefix: u8) -> bool {
    fn canon(ip: IpAddr) -> IpAddr {
        match ip {
            IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
            v4 => v4,
        }
    }
    match (canon(ip), canon(net)) {
        (IpAddr::V4(a), IpAddr::V4(b)) if prefix <= 32 => {
            let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
            u32::from(a) & mask == u32::from(b) & mask
        }
        (IpAddr::V6(a), IpAddr::V6(b)) if prefix <= 128 => {
            let mask = if prefix == 0 { 0 } else { u128::MAX << (128 - prefix) };
            u128::from(a) & mask == u128::from(b) & mask
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round<T>(v: T) -> T
    where
        T: fory_core::Serializer + fory_core::ForyDefault + std::fmt::Debug + PartialEq + 'static,
    {
        let f = fory();
        let bytes = f.serialize(&v).unwrap();
        let back: T = f.deserialize(&bytes).unwrap();
        assert_eq!(back, v);
        back
    }

    fn summary() -> JobSummary {
        JobSummary {
            id: 7,
            title: "fix the flaky test".into(),
            kind: "issue".into(),
            origin: "forkdeck/forkdeck#12".into(),
            url: "https://example.invalid/12".into(),
            status: Status::Reviewing,
            agent: "codex".into(),
            tier: "standard".into(),
            created_unix: 1_760_000_000,
            verdict: 2,
            pr_url: String::new(),
            rounds: 1,
            can_reply: false,
            outcome: String::new(),
        }
    }

    #[test]
    fn every_call_survives_the_wire() {
        for c in [
            Call::Nop,
            Call::Hello { proto: PROTO_VERSION, app: "android 0.1".into() },
            Call::Snapshot,
            Call::Job { id: 3 },
            Call::Submit { prompt: "add a flag".into(), agent: Agent::Codex },
            Call::Reply { id: 3, text: "also the docs".into() },
            Call::Cancel { id: 3 },
            Call::SetAutomation { on: true },
            Call::PollNow,
            Call::Subscribe { since: u64::MAX - 1 },
            Call::Pair { token: "t".into(), csr: vec![0, 255, 7], device: "pixel".into() },
        ] {
            round(c);
        }
    }

    #[test]
    fn every_reply_survives_the_wire() {
        let stage = StageView {
            agent: "claude".into(),
            state: StageState::Running,
            cost: 0.125,
            pushed: 400,
            ..Default::default()
        };
        let detail = JobDetail {
            summary: summary(),
            prompt: "p".into(),
            has_decision: true,
            decision: DecisionView {
                agent: "codex".into(),
                gates: vec![GateView { name: "agent".into(), answer: "codex".into(), value: 0.8, sharp: true }],
                ms: 12,
                ..Default::default()
            },
            worker: stage.clone(),
            reviewer: StageView::default(),
            worker_tail: vec!["a".into(), "b".into()],
            reviewer_tail: vec![],
            turns: vec![TurnView { ask: "q".into(), answer: String::new() }],
            checks: 1,
            findings: vec!["missing test".into()],
        };
        for r in [
            Reply::Ok,
            Reply::Err { msg: "no such job".into() },
            Reply::Hello { info: ServerInfo { proto: 1, version: "0.1.0".into(), host: "box".into(), device: "pixel".into() } },
            Reply::Snapshot {
                snap: Snapshot {
                    seq: 9,
                    automation: true,
                    jobs: vec![summary()],
                    log: vec![LogLine { seq: 4, time: "12:00:01".into(), who: "issues".into(), msg: "polled".into() }],
                    repos: vec![RepoView { name: "a/b".into(), ..Default::default() }],
                    ..Default::default()
                },
            },
            Reply::Job { found: true, detail },
            Reply::Job { found: false, detail: JobDetail::default() },
            Reply::Submitted { id: 8 },
            Reply::Paired { cert: vec![1; 300], ca: vec![2; 300] },
            Reply::Event { seq: 1, event: Event::Resync },
            Reply::Event { seq: 2, event: Event::Job { job: summary() } },
            Reply::Event { seq: 3, event: Event::Stage { id: 7, which: Which::Reviewer, stage } },
            Reply::Event { seq: 4, event: Event::Tail { id: 7, which: Which::Worker, from: 198, lines: vec!["x".into()] } },
            Reply::Event { seq: 5, event: Event::Log { lines: vec![LogLine::default()] } },
            Reply::Event { seq: 6, event: Event::Automation { on: false } },
        ] {
            round(r);
        }
    }

    #[test]
    fn a_pairing_code_round_trips_and_rejects_junk() {
        let info = PairInfo {
            host: "192.168.1.20".parse().unwrap(),
            port: 7443,
            ca_sha256: "ab".repeat(32),
            token: "Zx81".into(),
            prefix: 24,
        };
        assert_eq!(PairInfo::parse(&info.to_uri()).unwrap(), info);
        assert!(PairInfo::parse("https://example.com").is_err());
        assert!(PairInfo::parse(&info.to_uri().replace("n=24", "n=33")).is_err());
        assert!(PairInfo::parse(&info.to_uri().replace(&"ab".repeat(32), "abc")).is_err());
        assert!(PairInfo::parse("forkdeck://pair?h=10.0.0.1&p=1&t=x&n=8").is_err());
    }

    #[test]
    fn subnet_membership_follows_the_prefix() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(in_subnet(ip("192.168.1.77"), ip("192.168.1.20"), 24));
        assert!(!in_subnet(ip("192.168.2.77"), ip("192.168.1.20"), 24));
        assert!(in_subnet(ip("10.9.8.7"), ip("10.0.0.1"), 8));
        assert!(!in_subnet(ip("192.168.1.21"), ip("192.168.1.20"), 32));
        assert!(in_subnet(ip("::ffff:192.168.1.77"), ip("192.168.1.20"), 24));
        assert!(in_subnet(ip("fe80::1"), ip("fe80::abcd"), 64));
        assert!(!in_subnet(ip("fe80::1"), ip("192.168.1.20"), 0));
        assert!(!in_subnet(ip("192.168.1.77"), ip("192.168.1.20"), 40));
    }
}

/// JSON contracts use numbers for integers, matching serde_json's wire representation.
pub fn web_contracts() -> String {
    use ts_rs::TS;
    let cfg = ts_rs::Config::default().with_large_int("number");
    let mut out = String::from("// Generated from crates/remote-proto. Run npm run contracts to refresh.\n");
    for declaration in [BoardProjection::decl(&cfg), Agent::decl(&cfg), Status::decl(&cfg), StageState::decl(&cfg), Which::decl(&cfg), JobSummary::decl(&cfg), StageView::decl(&cfg), GateView::decl(&cfg), DecisionView::decl(&cfg), TurnView::decl(&cfg), JobDetail::decl(&cfg), LogLine::decl(&cfg), RepoView::decl(&cfg), Snapshot::decl(&cfg), ServerInfo::decl(&cfg), Event::decl(&cfg)] {
        out.push_str(&format!("export {declaration}\n"));
    }
    out
}

#[cfg(test)]
mod web_contract_tests {
    #[test]
    fn typescript_contracts_match_the_shared_dtos() {
        assert_eq!(super::web_contracts(), include_str!("../../../web/src/contracts.ts"), "run npm run contracts in web/");
    }
}
