//! Typed loopback protocol boundaries. The fixture is an HTTP peer, not a model.
use serde_json::{Value, json};
use std::{
    io::{BufRead, Read, Write},
    time::Duration,
};

fn endpoint(status: u16, body: String) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = std::io::BufReader::new(socket.try_clone().unwrap());
        let mut line = String::new();
        let mut length = 0;
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(n) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = n.trim().parse::<usize>().unwrap();
            }
        }
        assert!(length < 4096);
        let mut bytes = vec![0; length];
        reader.read_exact(&mut bytes).unwrap();
        let request: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(request["questions"]["relevant"]["type"], "noul");
        let redirect = if status == 302 {
            "Location: http://127.0.0.1:1/\r\n"
        } else {
            ""
        };
        // Oversize response test may close the connection as soon as the bound is reached.
        let _ = write!(
            socket,
            "HTTP/1.1 {status} Test\r\n{redirect}Content-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    });
    (format!("http://{address}/v1/decide"), handle)
}
#[test]
fn refuses_redirects_and_malformed_scores_instead_of_defaulting() {
    let cases = vec![
        (302, "{}".into()),
        (500, "{}".into()),
        (200, "not JSON".into()),
        (
            200,
            json!({"answers":{"relevant":{"type":"noul","noul":1.2}}}).to_string(),
        ),
        (
            200,
            json!({"answers":{"relevant":{"type":"choice","noul":0.5}}}).to_string(),
        ),
        (
            200,
            json!({"answers":{"relevant":{"type":"noul","noul":0.5},"extra":{}}}).to_string(),
        ),
        (200, "x".repeat(65537)),
    ];
    for (status, body) in cases {
        let (url, peer) = endpoint(status, body);
        assert!(
            kev_filter::client::relevance(&url, "fixture", "goal", "text", Duration::from_secs(2))
                .is_err()
        );
        peer.join().unwrap();
    }
}
#[test]
fn url_validation_never_allows_cloud_credentials_or_ambiguous_routes() {
    for url in [
        "https://127.0.0.1:8089/v1/decide",
        "http://example.com:8089/v1/decide",
        "http://user:secret@127.0.0.1:8089/v1/decide",
        "http://127.0.0.1:8089/v1/decide?redirect=1",
        "http://127.0.0.1:8089/other",
        "http://127.0.0.1:0/v1/decide",
    ] {
        assert!(kev_filter::client::validate_url(url).is_err(), "{url}");
    }
    assert!(kev_filter::client::validate_url("http://127.0.0.1:8009/v1/systemone").is_ok());
    assert!(kev_filter::client::validate_url("http://[::1]:8009/v1/systemone").is_ok());
}
