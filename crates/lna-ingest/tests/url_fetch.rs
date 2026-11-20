//! URL import safety and behaviour (B3): SSRF blocks, redirect handling, size limit, page parsing.
use lna_ingest::ssrf::{safe_fetch_url, safe_fetch_url_allowing_local_start};
use lna_types::ErrorCode;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// Tiny HTTP server: path -> (status, headers, body). Records the paths it was asked for.
struct Site {
    port: u16,
    hits: Arc<Mutex<Vec<String>>>,
}

fn site(routes: Vec<(&'static str, u16, Vec<String>, Vec<u8>)>) -> Site {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(Mutex::new(Vec::new()));
    let h = hits.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let routes = routes.clone();
            let h = h.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                h.lock().unwrap().push(path.clone());
                let (status, headers, body) = routes
                    .iter()
                    .find(|(p, ..)| *p == path)
                    .map(|(_, s, h, b)| (*s, h.clone(), b.clone()))
                    .unwrap_or((404, vec![], b"nope".to_vec()));
                let mut head = format!(
                    "HTTP/1.1 {status} X\r\ncontent-length: {}\r\nconnection: close\r\n",
                    body.len()
                );
                for line in headers {
                    head.push_str(&line);
                    head.push_str("\r\n");
                }
                head.push_str("\r\n");
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
                let _ = s.flush();
                let _ = s.shutdown(std::net::Shutdown::Write);
                std::thread::sleep(std::time::Duration::from_millis(100));
            });
        }
    });
    Site { port, hits }
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn production_fetch_refuses_local_private_metadata_and_non_http_targets() {
    let s = site(vec![("/", 200, vec![], b"secret".to_vec())]);
    let local = format!("http://127.0.0.1:{}/", s.port);
    for url in [
        local.as_str(),
        "http://localhost:8080/",
        "http://10.1.2.3/",
        "http://192.168.0.10/",
        "http://172.16.5.5/",
        "http://169.254.169.254/latest/meta-data/",
        "http://[::1]/",
        "file:///etc/passwd",
        "ftp://example.com/",
        "gopher://example.com/",
    ] {
        let err = rt().block_on(safe_fetch_url(url)).unwrap_err();
        assert_eq!(err.code, ErrorCode::UrlBlocked, "{url}: {err:?}");
    }
    assert!(
        s.hits.lock().unwrap().is_empty(),
        "blocked URL must not be contacted"
    );
}

#[test]
fn redirects_are_revalidated_on_every_hop() {
    for target in [
        "http://169.254.169.254/latest/meta-data/",
        "http://127.0.0.1:9/admin",
        "http://10.0.0.1/",
        "file:///etc/passwd",
        "http://localhost/",
    ] {
        let s = site(vec![(
            "/start",
            302,
            vec![format!("Location: {target}")],
            vec![],
        )]);
        let url = format!("http://127.0.0.1:{}/start", s.port);
        let err = rt()
            .block_on(safe_fetch_url_allowing_local_start(&url))
            .unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::UrlBlocked,
            "redirect to {target}: {err:?}"
        );
    }
}

#[test]
fn at_most_five_redirects_are_followed() {
    // /r0 -> /r1 -> ... -> /r6: six hops redirect, so the limit is hit.
    let mut routes = Vec::new();
    let names: Vec<&'static str> = vec!["/r0", "/r1", "/r2", "/r3", "/r4", "/r5", "/r6", "/r7"];
    for (i, n) in names.iter().enumerate().take(7) {
        routes.push((*n, 302, vec![format!("Location: {}", names[i + 1])], vec![]));
    }
    routes.push(("/r7", 200, vec![], b"end".to_vec()));
    let s = site(routes);
    let err = rt()
        .block_on(safe_fetch_url_allowing_local_start(&format!(
            "http://127.0.0.1:{}/r0",
            s.port
        )))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::FetchFailed);
    assert!(
        err.message.contains("Too many redirects"),
        "{}",
        err.message
    );
    assert_eq!(
        s.hits.lock().unwrap().len(),
        6,
        "6 requests = initial + 5 redirects"
    );

    // exactly five redirects still succeeds
    let mut routes = Vec::new();
    let names: Vec<&'static str> = vec!["/a0", "/a1", "/a2", "/a3", "/a4", "/a5"];
    for (i, n) in names.iter().enumerate().take(5) {
        routes.push((*n, 302, vec![format!("Location: {}", names[i + 1])], vec![]));
    }
    routes.push((
        "/a5",
        200,
        vec![],
        "<html><title>终点</title></html>".as_bytes().to_vec(),
    ));
    let s = site(routes);
    let (final_url, body) = rt()
        .block_on(safe_fetch_url_allowing_local_start(&format!(
            "http://127.0.0.1:{}/a0",
            s.port
        )))
        .unwrap();
    assert!(final_url.ends_with("/a5"));
    assert!(String::from_utf8_lossy(&body).contains("终点"));
}

#[test]
fn responses_over_ten_megabytes_are_rejected() {
    let big = vec![b'a'; 10 * 1024 * 1024 + 1];
    let s = site(vec![("/big", 200, vec![], big)]);
    let err = rt()
        .block_on(safe_fetch_url_allowing_local_start(&format!(
            "http://127.0.0.1:{}/big",
            s.port
        )))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::SourceTooLarge);
}

#[test]
fn http_errors_are_reported_as_fetch_failures() {
    let s = site(vec![]);
    let err = rt()
        .block_on(safe_fetch_url_allowing_local_start(&format!(
            "http://127.0.0.1:{}/missing",
            s.port
        )))
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::FetchFailed);
    assert!(err.message.contains("404"));
}

#[test]
fn a_normal_page_is_fetched_and_its_title_and_text_are_extracted() {
    let html = "<html><head><title>向量空间入门</title></head><body><nav>菜单</nav><article><h1>基</h1><p>基是线性无关的生成组。</p></article><script>alert(1)</script></body></html>";
    let s = site(vec![("/page", 200, vec![], html.as_bytes().to_vec())]);
    let (_, bytes) = rt()
        .block_on(safe_fetch_url_allowing_local_start(&format!(
            "http://127.0.0.1:{}/page",
            s.port
        )))
        .unwrap();
    let parsed = lna_ingest::parser::parse_html(&lna_ingest::parser::decode_text_bytes(&bytes));
    assert_eq!(parsed.title.as_deref(), Some("向量空间入门"));
    let text: String = parsed
        .blocks
        .iter()
        .map(|b| b.text.clone())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("基是线性无关的生成组"));
    assert!(!text.contains("alert(1)"), "scripts are dropped");
}
