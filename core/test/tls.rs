use super::*;
use axum::{Router, routing::get};
use openssl::ssl::{SslConnector, SslSession, SslSessionCacheMode, SslStream as BlockingSslStream};
use std::{
    io::{Read, Write},
    sync::Mutex,
};

/// A self-signed certificate for localhost and its key, as PEM files in a fresh directory.
fn localhost_certificate() -> (std::path::PathBuf, std::path::PathBuf) {
    let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
    let key = PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap();
    let mut name = X509NameBuilder::new().unwrap();
    name.append_entry_by_nid(Nid::COMMONNAME, "localhost")
        .unwrap();
    let name = name.build();
    let mut cert = X509::builder().unwrap();
    cert.set_version(2).unwrap();
    cert.set_subject_name(&name).unwrap();
    cert.set_issuer_name(&name).unwrap();
    cert.set_pubkey(&key).unwrap();
    cert.set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    cert.set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    cert.sign(&key, MessageDigest::sha256()).unwrap();
    let dir = std::env::temp_dir().join(format!("bettertest-tls-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (cert_path, key_path) = (dir.join("boss.crt"), dir.join("boss.key"));
    std::fs::write(&cert_path, cert.build().to_pem().unwrap()).unwrap();
    std::fs::write(&key_path, key.private_key_to_pem_pkcs8().unwrap()).unwrap();
    (cert_path, key_path)
}

struct Visit {
    alpn: Option<Vec<u8>>,
    resumed: bool,
    reply: String,
}

/// One connection, as a browser makes it: offering HTTP/2 first, and the session of an earlier
/// visit if there was one.
fn visit(connector: &SslConnector, addr: SocketAddr, session: Option<SslSession>) -> Visit {
    let mut ssl = connector
        .configure()
        .unwrap()
        .verify_hostname(false)
        .into_ssl("localhost")
        .unwrap();
    if let Some(session) = session {
        // SAFETY: the session came from a connection of this same connector.
        unsafe { ssl.set_session(&session).unwrap() };
    }
    let tcp = std::net::TcpStream::connect(addr).unwrap();
    let mut tls = BlockingSslStream::new(ssl, tcp).unwrap();
    tls.connect().unwrap();
    tls.write_all(b"GET / HTTP/1.1\r\nhost: localhost\r\n\r\n")
        .unwrap();
    let mut reply = Vec::new();
    while !reply.ends_with(b"hello") {
        let mut chunk = [0; 1024];
        let n = tls.read(&mut chunk).unwrap();
        assert!(
            n > 0,
            "connection closed after {:?}",
            String::from_utf8_lossy(&reply)
        );
        reply.extend_from_slice(&chunk[..n]);
    }
    // Closed cleanly: OpenSSL resumes no session whose connection ended without a close_notify.
    let _ = tls.shutdown();
    Visit {
        alpn: tls.ssl().selected_alpn_protocol().map(<[u8]>::to_vec),
        resumed: tls.ssl().session_reused(),
        reply: String::from_utf8_lossy(&reply).into_owned(),
    }
}

/// iPhone Safari loaded the page over HTTP/2 from the boss but never started the app (#15), and
/// does over HTTP/1.1 with the same optional client certificate request. So the boss offers no
/// HTTP/2, and a second visit still resumes its TLS session.
#[tokio::test]
async fn boss_speaks_http1_over_tls_and_resumes_sessions() {
    let (cert, key) = localhost_certificate();
    let tls = load(&cert, &key).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = Router::new().route("/", get(|| async { "hello" }));
    spawn(async move { axum::serve(listen(listener, &tls), app).await });

    let visits = tokio::task::spawn_blocking(move || {
        // The sessions the boss hands out (TLS 1.3 tickets), as a browser keeps them.
        let sessions = Arc::new(Mutex::new(Vec::new()));
        let mut connector = SslConnector::builder(SslMethod::tls_client()).unwrap();
        connector.set_verify(SslVerifyMode::NONE);
        connector.set_alpn_protos(b"\x02h2\x08http/1.1").unwrap();
        connector.set_session_cache_mode(SslSessionCacheMode::CLIENT);
        let kept = sessions.clone();
        connector.set_new_session_callback(move |_, session| crate::lock(&kept).push(session));
        let connector = connector.build();
        let first = visit(&connector, addr, None);
        let session = crate::lock(&sessions).pop();
        assert!(
            session.is_some(),
            "the boss handed out no session to resume"
        );
        let second = visit(&connector, addr, session);
        [first, second]
    })
    .await
    .unwrap();

    for (n, visit) in visits.iter().enumerate() {
        assert_eq!(visit.alpn, None, "visit {n} negotiated a protocol");
        assert!(
            visit.reply.starts_with("HTTP/1.1 200") && visit.reply.ends_with("hello"),
            "visit {n} got {:?}",
            visit.reply
        );
    }
    assert!(!visits[0].resumed);
    assert!(visits[1].resumed, "the second visit did a full handshake");
}
