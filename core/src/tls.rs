// HTTPS for the boss, when it is given a certificate and its key (`--cert`, `--key`). bettertest is not
// a certificate authority and makes no certificates: whoever turns HTTPS on brings one their browsers
// and workers already trust, usually from the site's own CA. Without them the boss speaks plain HTTP.
//
// The listener is axum's low-level OpenSSL one (its `low-level-openssl` example): accept, hand the
// connection to OpenSSL for the handshake, serve. TLS itself is all OpenSSL's.

use axum::{
    extract::connect_info::Connected,
    serve::{IncomingStream, Listener},
};
use openssl::{
    asn1::Asn1Time,
    bn::{BigNum, MsbOption},
    ec::{EcGroup, EcKey},
    error::ErrorStack,
    hash::MessageDigest,
    nid::Nid,
    pkcs12::Pkcs12,
    pkey::PKey,
    ssl::{Ssl, SslAcceptor, SslFiletype, SslMethod, SslVerifyMode},
    x509::{
        X509, X509NameBuilder,
        extension::{BasicConstraints, ExtendedKeyUsage, KeyUsage},
    },
};
use std::{io, net::SocketAddr, path::Path, pin::Pin, sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    spawn,
    sync::mpsc,
    time::timeout,
};
use tokio_openssl::SslStream;

/// A client that has not finished its handshake by then is dropped,
/// so a stalled connection cannot hold anything up.
const PATIENCE: Duration = Duration::from_secs(10);

/// The certificate the boss presents, and what people need to know about it.
pub struct Tls {
    acceptor: Arc<SslAcceptor>,
    // The names the certificate covers.
    pub names: Vec<String>,
    // The one workers check it against: its first name, or one its first wildcard covers.
    pub name: String,
    pub expires: String,
    pub days_left: i32,
}

/// Loads the certificate (with any intermediates after it) and its key, both PEM. Err says what is
/// wrong with them in words for whoever started the boss: a key that is not the certificate's, a
/// certificate that has expired.
pub fn load(cert: &Path, key: &Path) -> Result<Tls, String> {
    let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls_server())
        .map_err(|e| format!("could not set up TLS: {e}"))?;
    acceptor
        .set_certificate_chain_file(cert)
        .map_err(|e| format!("could not use {} as a PEM certificate: {e}", cert.display()))?;
    // OpenSSL checks the key against the certificate here too.
    acceptor
        .set_private_key_file(key, SslFiletype::PEM)
        .map_err(|e| {
            format!(
                "could not use {} as the PEM key of {}: {e}",
                key.display(),
                cert.display()
            )
        })?;
    // Ask every browser for a client certificate, without insisting, and take any it offers: the
    // boss decides whose it is by its fingerprint (certificate login), so it needs no CA.
    acceptor.set_verify_callback(SslVerifyMode::PEER, |_, _| true);
    // Once it asks for client certificates, OpenSSL resumes no session without this, and every
    // connection pays for a full handshake.
    acceptor
        .set_session_id_context(b"bettertest")
        .map_err(|e| format!("could not set up TLS: {e}"))?;
    // No ALPN, so no HTTP/2: iPhone Safari got the page over it but never started the app, while
    // over HTTP/1.1, with the same certificate request, it does.
    let acceptor = acceptor.build();
    let Some(leaf) = acceptor.context().certificate() else {
        return Err(format!("{} is not a PEM certificate", cert.display()));
    };
    let now = Asn1Time::days_from_now(0).map_err(|e| e.to_string())?;
    let days_left = now.diff(leaf.not_after()).map_err(|e| e.to_string())?.days;
    let expires = leaf.not_after().to_string();
    if days_left < 0 {
        return Err(format!("{} expired on {expires}", cert.display()));
    }
    let mut names: Vec<String> = leaf
        .subject_alt_names()
        .map(|sans| {
            sans.iter()
                .filter_map(|n| n.dnsname().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if names.is_empty() {
        let common = leaf.subject_name().entries_by_nid(Nid::COMMONNAME).next();
        if let Some(name) = common.and_then(|e| e.data().to_string().ok()) {
            names.push(name);
        }
    }
    let Some(first) = names.first() else {
        return Err(format!("{} names no host", cert.display()));
    };
    // Workers reach the boss by its address, never by DNS, so any name the certificate covers will
    // do: for *.site.example, boss.site.example.
    let name = first.replace("*.", "boss.");
    Ok(Tls {
        acceptor: Arc::new(acceptor),
        name,
        names,
        expires,
        days_left,
    })
}

/// Accepts connections on `listener` and shakes hands with each, on its own, so a slow one blocks
/// nobody.
pub fn listen(listener: TcpListener, tls: &Tls) -> HttpsListener {
    let local = listener.local_addr();
    let acceptor = tls.acceptor.clone();
    let (tx, rx) = mpsc::channel(64);
    spawn(async move {
        loop {
            let (mut stream, addr) = match listener.accept().await {
                Ok(accepted) => accepted,
                // Out of file descriptors and the like: give it a moment rather than spin.
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };
            crate::no_delay(&mut stream);
            let (acceptor, tx) = (acceptor.clone(), tx.clone());
            spawn(async move {
                if let Ok(Some(stream)) = timeout(PATIENCE, handshake(stream, &acceptor)).await {
                    let _ = tx.send((stream, addr)).await;
                }
            });
        }
    });
    HttpsListener { rx, local }
}

/// The TLS stream once a client has shaken hands, or None for one that could not (plain HTTP, say).
async fn handshake(stream: TcpStream, acceptor: &SslAcceptor) -> Option<SslStream<TcpStream>> {
    let ssl = Ssl::new(acceptor.context()).ok()?;
    let mut tls = SslStream::new(ssl, stream).ok()?;
    Pin::new(&mut tls).accept().await.ok()?;
    Some(tls)
}

/// The client certificate a connection presented, by its SHA-256 fingerprint (hex), if any. Every
/// request on the connection carries it, for certificate login.
#[derive(Clone)]
pub struct Peer {
    pub certificate: Option<String>,
}

impl Connected<IncomingStream<'_, HttpsListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, HttpsListener>) -> Self {
        let certificate = stream.io().ssl().peer_certificate();
        Peer {
            certificate: certificate.and_then(|c| fingerprint(&c).ok()),
        }
    }
}

/// Plain HTTP: TCP, each socket set to no delay. (axum's `tap_io` would do, but then no
/// `Connected` for `Peer` can be written for it.)
pub struct PlainListener(pub TcpListener);

impl Listener for PlainListener {
    type Io = TcpStream;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (mut stream, addr) = Listener::accept(&mut self.0).await;
        crate::no_delay(&mut stream);
        (stream, addr)
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Listener::local_addr(&self.0)
    }
}

impl Connected<IncomingStream<'_, PlainListener>> for Peer {
    fn connect_info(_: IncomingStream<'_, PlainListener>) -> Self {
        Peer { certificate: None }
    }
}

fn fingerprint(certificate: &X509) -> Result<String, ErrorStack> {
    let digest = certificate.digest(MessageDigest::sha256())?;
    Ok(crate::auth::hex(&digest))
}

/// A client certificate for `user`, made fresh: a P-256 key and a self-signed certificate good for
/// two years, packed with the key as PKCS#12 under a random password, which is how browsers import
/// one. The boss keeps only the fingerprint; the key leaves with the download.
pub struct ClientCertificate {
    pub p12: Vec<u8>,
    pub password: String,
    pub fingerprint: String,
    pub expires: String,
}

pub fn client_certificate(user: &str, label: &str) -> Result<ClientCertificate, String> {
    make_client_certificate(user, label).map_err(|e| e.to_string())
}

fn make_client_certificate(user: &str, label: &str) -> Result<ClientCertificate, ErrorStack> {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1)?;
    let key = PKey::from_ec_key(EcKey::generate(&group)?)?;
    let mut name = X509NameBuilder::new()?;
    name.append_entry_by_nid(Nid::COMMONNAME, user)?;
    name.append_entry_by_nid(Nid::ORGANIZATIONNAME, "bettertest")?;
    let name = name.build();
    let mut serial = BigNum::new()?;
    serial.rand(128, MsbOption::MAYBE_ZERO, false)?;
    let mut cert = X509::builder()?;
    cert.set_version(2)?;
    cert.set_serial_number(&*serial.to_asn1_integer()?)?;
    cert.set_subject_name(&name)?;
    cert.set_issuer_name(&name)?;
    cert.set_pubkey(&key)?;
    cert.set_not_before(&*Asn1Time::days_from_now(0)?)?;
    cert.set_not_after(&*Asn1Time::days_from_now(730)?)?;
    cert.append_extension(BasicConstraints::new().critical().build()?)?;
    cert.append_extension(KeyUsage::new().critical().digital_signature().build()?)?;
    cert.append_extension(ExtendedKeyUsage::new().client_auth().build()?)?;
    cert.sign(&key, MessageDigest::sha256())?;
    let cert = cert.build();
    let mut random = [0u8; 9];
    openssl::rand::rand_bytes(&mut random)?;
    let password = openssl::base64::encode_block(&random).replace(['+', '/'], "x");
    let p12 = Pkcs12::builder()
        .name(label)
        .pkey(&key)
        .cert(&cert)
        .build2(&password)?
        .to_der()?;
    Ok(ClientCertificate {
        p12,
        password,
        fingerprint: fingerprint(&cert)?,
        expires: cert.not_after().to_string(),
    })
}

pub struct HttpsListener {
    rx: mpsc::Receiver<(SslStream<TcpStream>, SocketAddr)>,
    local: io::Result<SocketAddr>,
}

impl Listener for HttpsListener {
    type Io = SslStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(accepted) => accepted,
            // The accepting task never ends, so neither does this.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        match &self.local {
            Ok(addr) => Ok(*addr),
            Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
        }
    }
}

#[cfg(test)]
#[path = "../test/tls.rs"]
mod tests;
