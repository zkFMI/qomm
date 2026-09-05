//! Real TCP/TLS regression tests of the production context constructors.
use super::*;
use crate::key_management::{create_ca, issue_mutual_tls_certificate, write_tls_bundle};
use openssl::ssl::SslVersion;

struct Pki {
    _directory: tempfile::TempDir,
    server: (PathBuf, PathBuf, PathBuf),
    client: (PathBuf, PathBuf, PathBuf),
}

impl Pki {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let (ca_key, ca) = create_ca("pqc-test-ca", 1).unwrap();
        let bundle = |name| {
            let (key, certificate) =
                issue_mutual_tls_certificate(&ca_key, &ca, name, &[name], &["127.0.0.1"], 1)
                    .unwrap();
            write_tls_bundle(directory.path().join(name), name, &key, &certificate, &ca).unwrap()
        };
        let server = bundle("node-0");
        let client = bundle("client-0");
        Self {
            _directory: directory,
            server,
            client,
        }
    }

    fn server(&self, hybrid: bool) -> Arc<SslAcceptor> {
        let (key, cert, ca) = &self.server;
        if hybrid {
            return server_ssl_context(cert, key, ca).unwrap().acceptor;
        }
        let mut builder = SslAcceptor::mozilla_modern_v5(SslMethod::tls_server()).unwrap();
        builder
            .set_min_proto_version(Some(SslVersion::TLS1_3))
            .unwrap();
        builder.set_groups_list("X25519").unwrap();
        builder.set_certificate_chain_file(cert).unwrap();
        builder
            .set_private_key(&load_owner_private_key(key).unwrap())
            .unwrap();
        builder.set_ca_file(ca).unwrap();
        builder.set_verify(SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT);
        Arc::new(builder.build())
    }

    fn client(&self, hybrid: bool) -> Arc<SslConnector> {
        let (key, cert, ca) = &self.client;
        if hybrid {
            return client_ssl_context(cert, key, ca).unwrap().connector;
        }
        let mut builder = SslConnector::builder(SslMethod::tls_client()).unwrap();
        builder
            .set_min_proto_version(Some(SslVersion::TLS1_3))
            .unwrap();
        builder.set_groups_list("X25519").unwrap();
        builder.set_certificate_chain_file(cert).unwrap();
        builder
            .set_private_key(&load_owner_private_key(key).unwrap())
            .unwrap();
        builder.set_ca_file(ca).unwrap();
        builder.set_verify(SslVerifyMode::PEER);
        Arc::new(builder.build())
    }
}

fn timeout(stream: &TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
}

#[test]
fn hybrid_tls_real_tcp_reuses_connection_and_reconnects() {
    let pki = Pki::new();
    let acceptor = pki.server(true);
    let connector = pki.client(true);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (tcp, _) = listener.accept().unwrap();
            timeout(&tcp);
            let mut stream = acceptor.accept(tcp).unwrap();
            assert_eq!(stream.ssl().version_str(), "TLSv1.3");
            assert!(!stream.ssl().session_reused());
            for _ in 0..3 {
                let mut request = [0; 16];
                stream.read_exact(&mut request).unwrap();
                stream.write_all(&request).unwrap();
            }
        }
    });
    for connection in 0..2u8 {
        let tcp = TcpStream::connect(address).unwrap();
        timeout(&tcp);
        let mut stream = connector.connect("node-0", tcp).unwrap();
        assert_eq!(stream.ssl().version_str(), "TLSv1.3");
        assert!(!stream.ssl().session_reused());
        for sequence in 0..3u8 {
            let request = [connection + sequence; 16];
            stream.write_all(&request).unwrap();
            let mut response = [0; 16];
            stream.read_exact(&mut response).unwrap();
            assert_eq!(response, request);
        }
    }
    server.join().unwrap();
}

fn rejects_mixed_groups(server_hybrid: bool) {
    let pki = Pki::new();
    let acceptor = pki.server(server_hybrid);
    let connector = pki.client(!server_hybrid);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        timeout(&tcp);
        assert!(
            acceptor.accept(tcp).is_err(),
            "classical-only handshake accepted"
        );
    });
    let tcp = TcpStream::connect(address).unwrap();
    timeout(&tcp);
    assert!(
        connector.connect("node-0", tcp).is_err(),
        "classical-only handshake accepted"
    );
    server.join().unwrap();
}

#[test]
fn hybrid_server_rejects_classical_only_client() {
    rejects_mixed_groups(true);
}

#[test]
fn hybrid_client_rejects_classical_only_server() {
    rejects_mixed_groups(false);
}
