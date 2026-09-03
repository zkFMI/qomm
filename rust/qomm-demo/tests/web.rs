use qomm_demo::web::{
    http_reply, parse_query, proxy_websocket_connection, read_client_message, server_frame,
    static_response, unquote, TEXT,
};
use std::io::{Cursor, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread;

fn masked(payload: &[u8]) -> Vec<u8> {
    let key = [1_u8, 2, 3, 4];
    let server = server_frame(TEXT, payload);
    let mut out = vec![server[0], server[1] | 0x80];
    let prefix = if payload.len() < 126 {
        2
    } else if payload.len() < (1 << 16) {
        out.extend(&server[2..4]);
        4
    } else {
        out.extend(&server[2..10]);
        10
    };
    out.extend(key);
    out.extend(
        server[prefix..]
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ key[index & 3]),
    );
    out
}

#[test]
fn browser_frames_round_trip_in_all_three_length_forms() {
    for payload in [b"{}".to_vec(), vec![b'x'; 200], vec![b'y'; 70_000]] {
        let (opcode, decoded) = read_client_message(&mut Cursor::new(masked(&payload))).unwrap();
        assert_eq!(opcode, TEXT);
        assert_eq!(decoded, payload);
    }
}

#[test]
fn traversal_is_refused_and_static_assets_are_embedded() {
    for path in ["/../../../etc/passwd", "/..%2f..%2fsecret", "/nope.js"] {
        assert!(static_response(path).starts_with(b"HTTP/1.1 404"));
    }
    assert!(static_response("/demo.js").starts_with(b"HTTP/1.1 200 OK"));
    assert!(static_response("/react-flow.js").starts_with(b"HTTP/1.1 200 OK"));
    assert!(static_response("/react-flow.css").starts_with(b"HTTP/1.1 200 OK"));
    assert!(static_response("/")
        .windows(14)
        .any(|part| part == b"Cache-Control:"));
    assert!(static_response("/")
        .windows(42)
        .any(|part| part == b"<meta name=\"qomm-gateway-port\" content=\"\">"));
}

#[test]
fn query_and_cache_headers_match_browser_contract() {
    assert_eq!(
        parse_query("seat=node%3A3&label=Ann%C3%A9"),
        std::collections::BTreeMap::from([
            ("label".into(), "Anné".into()),
            ("seat".into(), "node:3".into()),
        ])
    );
    assert!(parse_query("").is_empty());
    assert_eq!(unquote("a+b"), "a b");
    assert!(http_reply("200 OK", b"x", "text/html")
        .windows(23)
        .any(|part| part == b"Cache-Control: no-store"));
}

#[test]
fn static_frontend_relays_websocket_bytes_to_its_private_gateway() {
    let gateway_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let gateway_port = gateway_listener.local_addr().unwrap().port();
    let browser_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let browser_port = browser_listener.local_addr().unwrap().port();
    let request = b"GET /ws?session=abc HTTP/1.1\r\nUpgrade: websocket\r\n\r\n".to_vec();
    let expected_request = request.clone();

    let gateway = thread::spawn(move || {
        let (mut stream, _) = gateway_listener.accept().unwrap();
        let mut received = vec![0_u8; expected_request.len()];
        stream.read_exact(&mut received).unwrap();
        assert_eq!(received, expected_request);
        let mut browser_payload = [0_u8; 4];
        stream.read_exact(&mut browser_payload).unwrap();
        assert_eq!(&browser_payload, b"ping");
        stream.write_all(b"pong").unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
    });

    let mut browser = TcpStream::connect(("127.0.0.1", browser_port)).unwrap();
    let (frontend_stream, _) = browser_listener.accept().unwrap();
    let proxy = thread::spawn(move || {
        proxy_websocket_connection(frontend_stream, &request, "127.0.0.1", gateway_port).unwrap();
    });
    browser.write_all(b"ping").unwrap();
    let mut response = [0_u8; 4];
    browser.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"pong");
    browser.shutdown(Shutdown::Both).unwrap();
    proxy.join().unwrap();
    gateway.join().unwrap();
}
