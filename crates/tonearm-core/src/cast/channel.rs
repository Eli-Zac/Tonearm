//! The Cast protocol's wire: JSON messages in a small protobuf envelope,
//! each sent with its length first, over TLS to port 8009.
//!
//! Devices use self-signed certificates, so the certificate is not checked.
//! Every sender does the same: the device is found on the local network,
//! and nothing secret goes over the connection.

use serde_json::Value;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::Duration;

pub const CONNECTION: &str = "urn:x-cast:com.google.cast.tp.connection";
pub const HEARTBEAT: &str = "urn:x-cast:com.google.cast.tp.heartbeat";
pub const RECEIVER: &str = "urn:x-cast:com.google.cast.receiver";
pub const MEDIA: &str = "urn:x-cast:com.google.cast.media";

/// Who we are on every message.
pub const SENDER: &str = "sender-0";
/// The device itself, as opposed to an app running on it.
pub const PLATFORM: &str = "receiver-0";

/// Messages bigger than this are a broken stream, not a status.
const MAX_MESSAGE: usize = 64 * 1024;

/// One message from the device.
#[derive(Debug)]
pub struct Message {
    pub source: String,
    pub namespace: String,
    pub payload: Value,
}

#[cfg(not(target_os = "android"))]
type Tls = native_tls::TlsStream<TcpStream>;
#[cfg(target_os = "android")]
type Tls = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

#[cfg(not(target_os = "android"))]
fn secure(addr: SocketAddr, tcp: TcpStream) -> Result<Tls, String> {
    let connector = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .map_err(|e| e.to_string())?;
    connector
        .connect(&addr.ip().to_string(), tcp)
        .map_err(|e| format!("the device refused a secure connection ({e})"))
}

/// On Android, rustls. The handshake happens on the first message.
#[cfg(target_os = "android")]
fn secure(addr: SocketAddr, tcp: TcpStream) -> Result<Tls, String> {
    use std::sync::Arc;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AnyCertificate(provider)))
        .with_no_client_auth();
    let name = rustls::pki_types::ServerName::IpAddress(addr.ip().into());
    let connection = rustls::ClientConnection::new(Arc::new(config), name)
        .map_err(|e| format!("the device refused a secure connection ({e})"))?;
    Ok(rustls::StreamOwned::new(connection, tcp))
}

/// Takes the device's self-signed certificate, but still checks that the
/// device holds its key.
#[cfg(target_os = "android")]
#[derive(Debug)]
struct AnyCertificate(std::sync::Arc<rustls::crypto::CryptoProvider>);

#[cfg(target_os = "android")]
impl rustls::client::danger::ServerCertVerifier for AnyCertificate {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub struct Channel {
    tls: Tls,
    /// Bytes read but not yet a whole message.
    pending: Vec<u8>,
    /// This computer's address on the device's network, for the stream link.
    pub local_ip: IpAddr,
}

impl Channel {
    pub fn connect(addr: SocketAddr) -> Result<Channel, String> {
        let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .map_err(|e| format!("cannot reach the device ({})", e.kind()))?;
        let local_ip = tcp.local_addr().map_err(|e| e.to_string())?.ip();
        tcp.set_nodelay(true).ok();
        tcp.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let tls = secure(addr, tcp)?;
        // From here reads wait only briefly, so one thread can both listen
        // and send. A read that times out keeps what it had buffered.
        tls.get_ref()
            .set_read_timeout(Some(Duration::from_millis(40)))
            .ok();
        Ok(Channel {
            tls,
            pending: Vec::new(),
            local_ip,
        })
    }

    pub fn send(&mut self, destination: &str, namespace: &str, payload: &Value) -> Result<(), String> {
        let body = encode(SENDER, destination, namespace, &payload.to_string());
        let mut frame = (body.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&body);
        self.tls
            .write_all(&frame)
            .and_then(|_| self.tls.flush())
            .map_err(|e| format!("lost the connection ({})", e.kind()))
    }

    /// The next whole message, or None when nothing came in for a moment.
    pub fn poll(&mut self) -> Result<Option<Message>, String> {
        loop {
            if let Some(message) = self.take()? {
                return Ok(Some(message));
            }
            let mut chunk = [0u8; 4096];
            match self.tls.read(&mut chunk) {
                Ok(0) => return Err("the device closed the connection".into()),
                Ok(n) => self.pending.extend_from_slice(&chunk[..n]),
                Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    return Ok(None)
                }
                Err(e) => return Err(format!("lost the connection ({})", e.kind())),
            }
        }
    }

    /// A whole message off the front of `pending`, if one is there.
    fn take(&mut self) -> Result<Option<Message>, String> {
        if self.pending.len() < 4 {
            return Ok(None);
        }
        let length = u32::from_be_bytes(self.pending[..4].try_into().unwrap()) as usize;
        if length > MAX_MESSAGE {
            return Err("the device sent a message too big to be one".into());
        }
        if self.pending.len() < 4 + length {
            return Ok(None);
        }
        let body: Vec<u8> = self.pending.drain(..4 + length).skip(4).collect();
        Ok(decode(&body))
    }
}

// The envelope, CastMessage, has six fields: the protocol version (always
// 0), source, destination, namespace, the payload's type (0 for text) and
// the text. Written and read by hand, as there is nothing else to it.

fn varint(out: &mut Vec<u8>, mut n: u64) {
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn text_field(out: &mut Vec<u8>, field: u64, text: &str) {
    varint(out, field << 3 | 2);
    varint(out, text.len() as u64);
    out.extend_from_slice(text.as_bytes());
}

fn encode(source: &str, destination: &str, namespace: &str, payload: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 128);
    varint(&mut out, 1 << 3);
    varint(&mut out, 0);
    text_field(&mut out, 2, source);
    text_field(&mut out, 3, destination);
    text_field(&mut out, 4, namespace);
    varint(&mut out, 5 << 3);
    varint(&mut out, 0);
    text_field(&mut out, 6, payload);
    out
}

fn read_varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut n = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        n |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(n);
        }
    }
    None
}

/// The envelope's fields. A binary payload, which the media and receiver
/// namespaces never use, reads as no message.
fn decode(bytes: &[u8]) -> Option<Message> {
    let (mut source, mut namespace, mut payload) = (String::new(), String::new(), None);
    let mut at = 0;
    while at < bytes.len() {
        let key = read_varint(bytes, &mut at)?;
        match key & 7 {
            0 => {
                read_varint(bytes, &mut at)?;
            }
            2 => {
                let length = read_varint(bytes, &mut at)? as usize;
                let value = bytes.get(at..at.checked_add(length)?)?;
                at += length;
                let text = || String::from_utf8_lossy(value).into_owned();
                match key >> 3 {
                    2 => source = text(),
                    4 => namespace = text(),
                    6 => payload = serde_json::from_slice(value).ok(),
                    _ => {}
                }
            }
            _ => return None,
        }
    }
    Some(Message {
        source,
        namespace,
        payload: payload?,
    })
}

#[test]
fn envelope_round_trip() {
    let body = encode("sender-0", "receiver-0", HEARTBEAT, r#"{"type":"PING"}"#);
    let message = decode(&body).unwrap();
    assert_eq!(message.source, "sender-0");
    assert_eq!(message.namespace, HEARTBEAT);
    assert_eq!(message.payload["type"], "PING");
    // Long enough that the lengths take two bytes.
    let long = format!(r#"{{"type":"X","pad":"{}"}}"#, "a".repeat(300));
    let message = decode(&encode("a", "b", MEDIA, &long)).unwrap();
    assert_eq!(message.payload["pad"].as_str().unwrap().len(), 300);
}
