//! A client for forkdeck's remote control: pairing, then calls and the event stream over
//! one mutually authenticated TLS connection.

use crate::{fory, Call, ClientCodec, Event, PairInfo, Reply, ServerInfo, PROTO_VERSION, SUBSCRIBE};
use fory_rpc_streaming::{InboundClientMessage, OutboundClientMessage, StreamClientMessage, StreamServerMessage};
use futures::{SinkExt, StreamExt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio_util::codec::Framed;

const CONNECT: Duration = Duration::from_secs(10);
const CALL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err(e: impl std::fmt::Display) -> Error {
    Error(e.to_string())
}

/// What a phone keeps after pairing.
#[derive(Debug, Clone, PartialEq)]
pub struct Credentials {
    pub host: IpAddr,
    pub port: u16,
    /// The phone's private key, PKCS#8 DER.
    pub key: Vec<u8>,
    /// The phone's certificate, signed by forkdeck's CA.
    pub cert: Vec<u8>,
    /// forkdeck's CA certificate: the only thing trusted when connecting.
    pub ca: Vec<u8>,
}

fn sha256_hex(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|b| format!("{:02x}", b)).collect()
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Before pairing, the phone knows the CA only by the fingerprint in the QR code. The server
/// sends the CA along with its certificate; trust it if it matches, then check as usual.
#[derive(Debug)]
struct PinnedCa {
    sha256: String,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for PinnedCa {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let ca = intermediates
            .iter()
            .find(|c| sha256_hex(c) == self.sha256)
            .ok_or_else(|| rustls::Error::General("server's CA does not match the pairing code".into()))?;
        let mut roots = rustls::RootCertStore::empty();
        roots.add(ca.clone().into_owned())?;
        let inner = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), self.provider.clone())
            .build()
            .map_err(|e| rustls::Error::General(e.to_string()))?;
        inner.verify_server_cert(end_entity, &[], server_name, ocsp, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;
type Streams = Arc<Mutex<HashMap<u64, mpsc::Sender<(u64, Event)>>>>;

/// One connection to forkdeck. Cheap to share; calls may run concurrently.
pub struct Client {
    out: mpsc::Sender<OutboundClientMessage<Call>>,
    pending: Pending,
    streams: Streams,
    next: AtomicU64,
    closed: Arc<AtomicBool>,
    pub server: ServerInfo,
}

async fn tls(
    addr: SocketAddr,
    cfg: rustls::ClientConfig,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, Error> {
    let tcp = tokio::time::timeout(CONNECT, TcpStream::connect(addr))
        .await
        .map_err(|_| err("forkdeck did not answer (same network? running with [remote] enabled?)"))?
        .map_err(err)?;
    let _ = tcp.set_nodelay(true);
    let name = ServerName::IpAddress(addr.ip().into());
    tokio::time::timeout(CONNECT, tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(name, tcp))
        .await
        .map_err(|_| err("TLS handshake timed out"))?
        .map_err(|e| err(format!("TLS: {}", e)))
}

impl Client {
    fn start(stream: tokio_rustls::client::TlsStream<TcpStream>) -> Client {
        let (mut sink, mut rx) = Framed::new(stream, ClientCodec::new(fory())).split();
        let (out, mut out_rx) = mpsc::channel::<OutboundClientMessage<Call>>(64);
        let pending: Pending = Default::default();
        let streams: Streams = Default::default();
        let closed = Arc::new(AtomicBool::new(false));
        tokio::spawn(async move {
            while let Some(m) = out_rx.recv().await {
                if sink.send(m).await.is_err() {
                    break;
                }
            }
            let _ = sink.close().await;
        });
        // Weak: the reader must not keep the writer, and with it the connection, alive once the
        // client is dropped.
        let (p, s, c, cancel) = (pending.clone(), streams.clone(), closed.clone(), out.downgrade());
        tokio::spawn(async move {
            while let Some(Ok(msg)) = rx.next().await {
                match msg {
                    InboundClientMessage::Unary(resp, _) => {
                        if let Some(tx) = p.lock().unwrap().remove(&resp.request_id) {
                            let _ = tx.send(resp.message.unwrap_or_else(|e| Reply::Err { msg: e.detail }));
                        }
                    }
                    InboundClientMessage::Stream(StreamServerMessage::Data { stream_id, message: Reply::Event { seq, event } }, _) => {
                        // Never wait on a subscriber here: this task reads every reply. One that has
                        // fallen a channel's worth behind (or gone) loses its stream, which ends; it
                        // subscribes again from the last sequence number it saw to catch up.
                        let mut streams = s.lock().unwrap();
                        if let Some(tx) = streams.get(&stream_id) {
                            if tx.try_send((seq, event)).is_err() {
                                streams.remove(&stream_id);
                                // And the server stops sending it, or each catch-up would add a stream.
                                if let Some(out) = cancel.upgrade() {
                                    let _ = out.try_send(OutboundClientMessage::Stream(StreamClientMessage::Cancel { stream_id }, None));
                                }
                            }
                        }
                    }
                    InboundClientMessage::Stream(StreamServerMessage::End { stream_id } | StreamServerMessage::Error { stream_id, .. }, _) => {
                        s.lock().unwrap().remove(&stream_id);
                    }
                    InboundClientMessage::Stream(_, _) => {}
                }
            }
            // Gone: waiting callers get an error, streams end.
            c.store(true, Ordering::SeqCst);
            p.lock().unwrap().clear();
            s.lock().unwrap().clear();
        });
        Client { out, pending, streams, next: AtomicU64::new(1), closed, server: ServerInfo::default() }
    }

    /// Connect with the credentials from pairing.
    pub async fn connect(creds: &Credentials, app: &str) -> Result<Client, Error> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(creds.ca.clone())).map_err(err)?;
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(creds.key.clone()));
        let cfg = rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .map_err(err)?
            .with_root_certificates(roots)
            .with_client_auth_cert(vec![CertificateDer::from(creds.cert.clone())], key)
            .map_err(err)?;
        let mut c = Client::start(tls(SocketAddr::new(creds.host, creds.port), cfg).await?);
        c.server = c.hello(app).await?;
        Ok(c)
    }

    async fn hello(&self, app: &str) -> Result<ServerInfo, Error> {
        match self.call(Call::Hello { proto: PROTO_VERSION, app: app.into() }).await? {
            Reply::Hello { info } => Ok(info),
            Reply::Err { msg } => Err(Error(msg)),
            other => Err(err(format!("unexpected answer to hello: {:?}", other))),
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Send a call and wait for its answer. `Reply::Err` comes back as `Ok`; only a broken
    /// connection or a timeout is an `Err`.
    pub async fn call(&self, message: Call) -> Result<Reply, Error> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        // Checked after registering: the reader clears what is pending when it stops.
        if self.is_closed() {
            self.pending.lock().unwrap().remove(&id);
            return Err(err("disconnected"));
        }
        let req = tarpc::ClientMessage::Request(tarpc::Request { context: tarpc::context::current(), id, message });
        if self.out.send(OutboundClientMessage::Unary(req, None)).await.is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(err("disconnected"));
        }
        match tokio::time::timeout(CALL, rx).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(_)) => Err(err("disconnected")),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(err("forkdeck did not answer in time"))
            }
        }
    }

    /// Events after `since` (0: only new ones). The receiver ends when the connection does, or
    /// when it falls a channel's worth of events behind: subscribe again from the last one seen.
    pub async fn subscribe(&self, since: u64) -> Result<mpsc::Receiver<(u64, Event)>, Error> {
        // Stream ids live apart from call ids.
        let stream_id = (1u64 << 62) | self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel(1024);
        self.streams.lock().unwrap().insert(stream_id, tx);
        let open = StreamClientMessage::Open {
            stream_id,
            method_id: SUBSCRIBE,
            deadline_ns: 0,
            trace_id: 0,
            span_id: 0,
            sampling: 0,
            message: Call::Subscribe { since },
        };
        self.out.send(OutboundClientMessage::Stream(open, None)).await.map_err(|_| err("disconnected"))?;
        Ok(rx)
    }
}

/// A connection without a client certificate, trusting the CA named in the code. forkdeck
/// answers only `Hello` and `Pair` on it.
pub async fn unpaired(info: &PairInfo) -> Result<Client, Error> {
    let cfg = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(err)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedCa { sha256: info.ca_sha256.clone(), provider: provider() }))
        .with_no_client_auth();
    let mut c = Client::start(tls(SocketAddr::new(info.host, info.port), cfg).await?);
    c.server = c.hello("pairing").await?;
    Ok(c)
}

/// Pair with the forkdeck in a scanned code. The phone must be on the same subnet.
pub async fn pair(info: &PairInfo, device: &str) -> Result<Credentials, Error> {
    let c = unpaired(info).await?;
    let key = rcgen::KeyPair::generate().map_err(err)?;
    let mut params = rcgen::CertificateParams::default();
    params.distinguished_name.push(rcgen::DnType::CommonName, device);
    let csr = params.serialize_request(&key).map_err(err)?;
    let reply = c.call(Call::Pair { token: info.token.clone(), csr: csr.der().to_vec(), device: device.into() }).await?;
    match reply {
        Reply::Paired { cert, ca } => {
            if sha256_hex(&ca) != info.ca_sha256 {
                return Err(err("forkdeck answered with a different CA than the pairing code names"));
            }
            Ok(Credentials { host: info.host, port: info.port, key: key.serialize_der(), cert, ca })
        }
        Reply::Err { msg } => Err(Error(msg)),
        other => Err(err(format!("unexpected answer to pairing: {:?}", other))),
    }
}
