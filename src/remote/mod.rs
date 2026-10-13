//! Remote control from a paired phone on the local network.
//!
//! One TLS connection per phone carries calls and the event stream, framed by
//! `fory_rpc_streaming`. A phone proves itself with the client certificate it was given when
//! it paired; a connection without one may only pair. See `forkdeck_remote_proto` for the
//! messages.

pub mod devices;
pub mod feed;
pub mod pair;
pub mod pki;
pub mod view;

use crate::orchestrator::Hub;
use crate::state::{clip, AgentKind, AgentRef, Source};
use forkdeck_remote_proto::{self as proto, Call, Reply, ServerCodec, PROTO_VERSION, SUBSCRIBE};
use fory_rpc_streaming::{InboundServerMessage, OutboundServerMessage, StreamClientMessage, StreamError, StreamServerMessage};
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tarpc::{ClientMessage, Response};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio_util::codec::Framed;

const HANDSHAKE: Duration = Duration::from_secs(10);
/// How often a connected phone is checked against the paired list, so a revoke cuts it off.
const RECHECK: Duration = Duration::from_secs(3);

struct Ctx {
    hub: Hub,
    feed: Arc<feed::Feed>,
    ca: pki::Ca,
    devices: devices::Registry,
    dir: PathBuf,
    tls: watch::Receiver<Arc<rustls::ServerConfig>>,
}

/// Start listening. Runs until the process ends; errors before the listener is up are returned.
pub async fn serve(hub: Hub) -> anyhow::Result<()> {
    let cfg = hub.cfg.remote.clone();
    let ca = pki::Ca::load_or_create(&cfg.dir)?;
    let mut ips: Vec<IpAddr> = pair::interfaces().into_iter().map(|i| i.ip).collect();
    ips.sort();
    let (tls_tx, tls) = watch::channel(ca.server_config(&ips)?);
    let listener = TcpListener::bind(&cfg.listen).await?;
    let feed = Arc::new(feed::Feed::new());
    tokio::spawn(feed.clone().run(hub.clone(), Duration::from_millis(200)));
    let paired = devices::Registry::new(&cfg.dir).list().len();
    hub.log("remote", format!("listening on {} ({} phone(s) paired)", listener.local_addr()?, paired));
    let ctx = Arc::new(Ctx { hub: hub.clone(), feed, ca, devices: devices::Registry::new(&cfg.dir), dir: cfg.dir.clone(), tls });

    // Addresses change (DHCP, a new network): reissue the server certificate to match.
    let watcher = ctx.clone();
    tokio::spawn(async move {
        let mut ips = ips;
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let mut now: Vec<IpAddr> = pair::interfaces().into_iter().map(|i| i.ip).collect();
            now.sort();
            if now != ips {
                match watcher.ca.server_config(&now) {
                    Ok(c) => {
                        let _ = tls_tx.send(c);
                        watcher.hub.log("remote", "network addresses changed, server certificate reissued");
                        ips = now;
                    }
                    Err(e) => watcher.hub.log("remote", format!("could not reissue the server certificate: {}", e)),
                }
            }
        }
    });

    loop {
        let (tcp, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(e) => {
                ctx.hub.log("remote", format!("accept failed: {}", e));
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            }
        };
        let ctx = ctx.clone();
        tokio::spawn(async move {
            if let Err(e) = connection(ctx.clone(), tcp, peer).await {
                ctx.hub.log("remote", format!("{}: {}", peer.ip(), e));
            }
        });
    }
}

async fn connection(ctx: Arc<Ctx>, tcp: TcpStream, peer: SocketAddr) -> anyhow::Result<()> {
    let local = tcp.local_addr()?.ip();
    let same = pair::same_subnet(local, peer.ip(), &pair::interfaces());
    if ctx.hub.cfg.remote.lan_only && !same {
        anyhow::bail!("refused: not on this machine's subnet");
    }
    let acceptor = tokio_rustls::TlsAcceptor::from(ctx.tls.borrow().clone());
    let tls = tokio::time::timeout(HANDSHAKE, acceptor.accept(tcp)).await??;
    let fp = tls.get_ref().1.peer_certificates().and_then(|c| c.first()).map(|c| pki::sha256_hex(c));
    let device = match &fp {
        Some(fp) => Some(ctx.devices.lookup(fp).ok_or_else(|| anyhow::anyhow!("refused: certificate is not paired (revoked?)"))?),
        None if same => None,
        None => anyhow::bail!("refused: pairing must come from the same subnet as forkdeck"),
    };

    let (mut sink, mut rx) = Framed::new(tls, ServerCodec::new(proto::fory())).split();
    let (out, mut out_rx) = mpsc::channel::<OutboundServerMessage<Reply>>(256);
    let writer = tokio::spawn(async move {
        while let Some(m) = out_rx.recv().await {
            if sink.send(m).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    let mut streams: HashMap<u64, tokio::task::JoinHandle<()>> = HashMap::new();
    let mut recheck = tokio::time::interval(RECHECK);
    let mut revoked = false;
    loop {
        tokio::select! {
            msg = rx.next() => {
                let Some(Ok(msg)) = msg else { break };
                match msg {
                    InboundServerMessage::Unary(ClientMessage::Request(req), _) => {
                        let (ctx, out, device) = (ctx.clone(), out.clone(), device.clone());
                        tokio::spawn(async move {
                            let reply = match &device {
                                Some(d) => call(&ctx, d, req.message).await,
                                None => pairing(&ctx, peer.ip(), req.message).await,
                            };
                            let _ = out.send(OutboundServerMessage::Unary(Response { request_id: req.id, message: Ok(reply) }, None)).await;
                        });
                    }
                    InboundServerMessage::Unary(_, _) => {}
                    InboundServerMessage::Stream(StreamClientMessage::Open { stream_id, method_id, message, .. }, _) => {
                        match (method_id, message, device.is_some()) {
                            (SUBSCRIBE, Call::Subscribe { since }, true) => {
                                streams.insert(stream_id, tokio::spawn(subscribe(ctx.clone(), out.clone(), stream_id, since)));
                            }
                            _ => {
                                let error = StreamError::new(std::io::ErrorKind::PermissionDenied, "no such stream");
                                let _ = out.send(OutboundServerMessage::Stream(StreamServerMessage::Error { stream_id, error }, None)).await;
                            }
                        }
                    }
                    InboundServerMessage::Stream(StreamClientMessage::Cancel { stream_id } | StreamClientMessage::End { stream_id }, _) => {
                        if let Some(h) = streams.remove(&stream_id) {
                            h.abort();
                        }
                    }
                    InboundServerMessage::Stream(StreamClientMessage::Data { .. }, _) => {}
                }
            }
            _ = recheck.tick(), if fp.is_some() => {
                if fp.as_deref().and_then(|fp| ctx.devices.lookup(fp)).is_none() {
                    revoked = true;
                    break;
                }
            }
        }
    }
    for (_, h) in streams {
        h.abort();
    }
    drop(out);
    let _ = writer.await;
    if revoked {
        ctx.hub.log("remote", format!("{} revoked, disconnected", device.unwrap_or_default()));
    }
    Ok(())
}

async fn subscribe(ctx: Arc<Ctx>, out: mpsc::Sender<OutboundServerMessage<Reply>>, stream_id: u64, since: u64) {
    let send = |seq: u64, event: proto::Event| {
        OutboundServerMessage::Stream(StreamServerMessage::Data { stream_id, message: Reply::Event { seq, event } }, None)
    };
    let (backlog, mut rx) = ctx.feed.subscribe(since);
    for (seq, e) in backlog {
        if out.send(send(seq, e)).await.is_err() {
            return;
        }
    }
    loop {
        let m = match rx.recv().await {
            Ok((seq, e)) => send(seq, e),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => send(0, proto::Event::Resync),
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        };
        if out.send(m).await.is_err() {
            return;
        }
    }
}

fn hello(proto_version: u32, device: &str) -> Reply {
    if proto_version != PROTO_VERSION {
        return Reply::Err {
            msg: format!("this forkdeck speaks remote protocol {}, the app {}: update the older one", PROTO_VERSION, proto_version),
        };
    }
    Reply::Hello {
        info: proto::ServerInfo {
            proto: PROTO_VERSION,
            version: env!("CARGO_PKG_VERSION").into(),
            host: crate::team::hostname(),
            device: device.into(),
        },
    }
}

fn result(r: Result<(), String>) -> Reply {
    match r {
        Ok(()) => Reply::Ok,
        Err(msg) => Reply::Err { msg },
    }
}

async fn call(ctx: &Ctx, device: &str, c: Call) -> Reply {
    let hub = &ctx.hub;
    match c {
        Call::Hello { proto, .. } => hello(proto, device),
        Call::Snapshot => Reply::Snapshot { snap: ctx.feed.snapshot(hub) },
        Call::Job { id } => match ctx.feed.detail(hub, id) {
            Some(detail) => Reply::Job { found: true, detail },
            None => Reply::Job { found: false, detail: Default::default() },
        },
        Call::Submit { prompt, agent } => {
            let prompt = prompt.trim().to_string();
            if prompt.is_empty() {
                return Reply::Err { msg: "the task is empty".into() };
            }
            if hub.state.lock().unwrap().stopping {
                return Reply::Err { msg: "shutting down".into() };
            }
            let forced = match agent {
                proto::Agent::Auto => None,
                proto::Agent::Claude => Some(AgentRef::Kind(AgentKind::Claude)),
                proto::Agent::Codex => Some(AgentRef::Kind(AgentKind::Codex)),
            };
            let workdir = match &hub.cfg.remote.workdir {
                Some(w) => w.clone(),
                None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            };
            match hub.submit(clip(&prompt, 80), prompt.clone(), Source::Manual, workdir, forced, crate::state::Planning::IfRouted).await {
                Ok(id) => {
                    hub.log("remote", format!("{} queued #{}: {}", device, id, clip(&prompt, 60)));
                    Reply::Submitted { id }
                }
                Err(msg) => Reply::Err { msg },
            }
        }
        Call::Reply { id, text } => {
            let text = text.trim().to_string();
            if text.is_empty() {
                return Reply::Err { msg: "the reply is empty".into() };
            }
            let r = hub.reply(id, text).await;
            if r.is_ok() {
                hub.log("remote", format!("{} replied to #{}", device, id));
            }
            result(r)
        }
        Call::Cancel { id } => {
            hub.log("remote", format!("{} cancels #{}", device, id));
            hub.cancel(id);
            Reply::Ok
        }
        Call::SetAutomation { on } => {
            let mut st = hub.state.lock().unwrap();
            if on && st.stopping {
                return Reply::Err { msg: "shutting down".into() };
            }
            st.automation = on;
            st.log("remote", format!("{}: polling {}", device, if on { "on" } else { "paused" }));
            Reply::Ok
        }
        Call::PollNow => {
            if hub.cfg.repos.is_empty() {
                return Reply::Err { msg: "no [[repos]] configured, nothing to poll".into() };
            }
            hub.log("remote", format!("{} asked for a poll", device));
            crate::issues::poll_once(hub).await;
            Reply::Ok
        }
        Call::Subscribe { .. } => Reply::Err { msg: "subscribe is a stream".into() },
        Call::Pair { .. } => Reply::Err { msg: "already paired".into() },
        Call::Nop => Reply::Ok,
    }
}

/// A connection without a certificate: it may only pair.
async fn pairing(ctx: &Ctx, peer: IpAddr, c: Call) -> Reply {
    match c {
        Call::Hello { proto, .. } => hello(proto, ""),
        Call::Pair { token, csr, device } => {
            let hub = &ctx.hub;
            if let Err(msg) = pair::redeem(&ctx.dir, &token, peer) {
                hub.log("remote", format!("pairing from {} refused: {}", peer, msg));
                // Make guessing slow.
                tokio::time::sleep(Duration::from_secs(1)).await;
                return Reply::Err { msg };
            }
            let name = ctx.devices.free_name(&device);
            let cert = match ctx.ca.sign_client(&csr, &name) {
                Ok(c) => c,
                Err(e) => return Reply::Err { msg: e.to_string() },
            };
            if let Err(e) = ctx.devices.add(&name, &pki::sha256_hex(&cert)) {
                return Reply::Err { msg: format!("could not record the phone: {}", e) };
            }
            hub.log("remote", format!("paired {} from {}", name, peer));
            Reply::Paired { cert: cert.to_vec(), ca: ctx.ca.der.to_vec() }
        }
        _ => Reply::Err { msg: "pair this phone first".into() },
    }
}

/// `forkdeck remote pair`: print a code for a phone to scan.
pub fn pair_cmd(cfg: &crate::config::Config, iface: Option<&str>, ttl_secs: u64) -> anyhow::Result<()> {
    let r = &cfg.remote;
    let ifaces = pair::interfaces();
    let Some(at) = pair::pick(&ifaces, iface) else {
        let names: Vec<String> = ifaces.iter().map(|i| format!("{} {}/{}", i.name, i.ip, i.prefix)).collect();
        anyhow::bail!("no {} found; interfaces: {}", iface.map(|n| format!("interface {}", n)).unwrap_or("private IPv4 address".into()), names.join(", "));
    };
    let port = r.listen.parse::<SocketAddr>()?.port();
    let ca = pki::Ca::load_or_create(&r.dir)?;
    let token = pair::issue(&r.dir, &at, ttl_secs)?;
    let info = proto::PairInfo { host: at.ip, port, ca_sha256: ca.fingerprint(), token, prefix: at.prefix };
    let uri = info.to_uri();
    println!("{}", pair::qr(&uri)?);
    println!("Scan with the forkdeck app, or paste:\n  {}\n", uri);
    println!("Works once, for {} seconds, from {}/{} ({}).", ttl_secs, at.ip, at.prefix, at.name);
    if !r.enabled {
        println!("note: [remote] enabled = false, so nothing is listening. Set it to true and restart forkdeck.");
    }
    Ok(())
}

pub fn devices_cmd(cfg: &crate::config::Config) -> anyhow::Result<()> {
    let list = devices::Registry::new(&cfg.remote.dir).list();
    if list.is_empty() {
        println!("no phones paired (forkdeck remote pair)");
    }
    for d in list {
        println!("{:<24} paired {}  {}", d.name, d.paired_at, &d.sha256[..16]);
    }
    Ok(())
}

pub fn revoke_cmd(cfg: &crate::config::Config, name: &str) -> anyhow::Result<()> {
    if devices::Registry::new(&cfg.remote.dir).remove(name)? {
        println!("revoked {}; a running forkdeck disconnects it within seconds", name);
        Ok(())
    } else {
        anyhow::bail!("no paired phone called {:?} (forkdeck remote devices)", name)
    }
}
