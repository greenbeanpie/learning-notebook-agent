//! Throttled requests (HTTP 429, 503 with Retry-After) are waited out and retried, bounded and
//! cancellable; a profile without a limit (`max_parallel` 0) is not capped by the client gate.
//! The mock endpoint is a raw TCP server that answers each request from a script.
use futures_util::future::BoxFuture;
use lna_llm::{
    LlmClient, LlmEvent, LlmMessage, LlmRequest, OutputMode, Part, ReasoningOptions,
    ResolvedProfile, Role, StopReason, ToolMode,
};
use lna_types::{ErrorCode, ModelCapabilities, ProtocolKind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

/// One canned answer of the mock endpoint.
struct Reply {
    status: u16,
    /// Extra header lines, each ending in "\r\n".
    extra: String,
    content_type: &'static str,
    body: String,
}

impl Reply {
    fn throttled(retry_after: Option<&str>) -> Reply {
        let extra = match retry_after {
            Some(v) => format!("Retry-After: {v}\r\n"),
            None => String::new(),
        };
        Reply {
            status: 429,
            extra,
            content_type: "application/json",
            body: r#"{"error":{"message":"rate limited"}}"#.into(),
        }
    }

    fn status(status: u16, body: &str) -> Reply {
        Reply {
            status,
            extra: String::new(),
            content_type: "text/plain",
            body: body.into(),
        }
    }

    /// A complete chat-completions stream answering "ok".
    fn ok() -> Reply {
        Reply {
            status: 200,
            extra: String::new(),
            content_type: "text/event-stream",
            body: concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n"
            )
            .into(),
        }
    }

    fn to_http(&self) -> String {
        format!(
            "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{}",
            self.status,
            self.content_type,
            self.body.len(),
            self.extra,
            self.body
        )
    }
}

type Handler = Arc<dyn Fn(usize) -> BoxFuture<'static, Reply> + Send + Sync>;

/// Reads one HTTP request (headers and a Content-Length body) from the socket.
async fn read_request(sock: &mut TcpStream) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        let n = sock.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            while buf.len() < pos + 4 + len {
                let n = sock.read(&mut tmp).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            return;
        }
    }
}

/// Starts the mock endpoint. `handler(n)` answers the n-th request (0-based); the returned counter
/// is the number of requests received so far.
async fn serve(handler: Handler) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let seen_c = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let handler = handler.clone();
            let seen = seen_c.clone();
            tokio::spawn(async move {
                read_request(&mut sock).await;
                let n = seen.fetch_add(1, Ordering::SeqCst);
                let reply = handler(n).await;
                let _ = sock.write_all(reply.to_http().as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://{addr}/v1"), seen)
}

fn profile(base_url: String) -> ResolvedProfile {
    ResolvedProfile {
        id: "p".into(),
        display_name: "p".into(),
        protocol: ProtocolKind::ChatCompletion,
        base_url,
        model_id: "m".into(),
        api_key: Some(secrecy::SecretString::from("sk-test-key".to_string())),
        show_thinking: false,
        effort: None,
        context_window: 32000,
        max_output_tokens: 1000,
        capabilities: ModelCapabilities::default(),
        extra_headers: Default::default(),
        anthropic_version: "2023-06-01".into(),
        preset: None,
        shrink_media: false,
    }
}

fn request() -> LlmRequest {
    LlmRequest {
        system: "sys".into(),
        messages: vec![LlmMessage {
            role: Role::User,
            parts: vec![Part::Text("hi".into())],
        }],
        tools: vec![],
        tool_mode: ToolMode::None,
        output: OutputMode::Text,
        max_output_tokens: 100,
        reasoning: ReasoningOptions::default(),
    }
}

/// Runs one streamed request and returns its result and the text it produced.
async fn run(
    client: &LlmClient,
    profile: &ResolvedProfile,
    cancel: CancellationToken,
) -> (lna_types::LnaResult<StopReason>, String) {
    let mut text = String::new();
    let res = client
        .stream(profile, &request(), cancel, |ev| {
            if let LlmEvent::TextDelta(t) = ev {
                text.push_str(&t);
            }
        })
        .await;
    (res, text)
}

#[tokio::test(flavor = "multi_thread")]
async fn throttled_requests_are_waited_out_and_then_succeed() {
    let (base, seen) = serve(Arc::new(|n| {
        Box::pin(async move {
            match n {
                // Retry-After: 0 and a plain 429 (backoff) before the endpoint takes the request
                0 => Reply::throttled(Some("0")),
                1 => Reply::throttled(None),
                _ => Reply::ok(),
            }
        })
    }))
    .await;
    let client = LlmClient::new().with_retry_base(Duration::from_millis(1));
    let (res, text) = run(&client, &profile(base), CancellationToken::new()).await;
    assert!(res.is_ok(), "{res:?}");
    assert_eq!(text, "ok");
    assert_eq!(seen.load(Ordering::SeqCst), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_endpoint_that_always_throttles_fails_after_eight_tries() {
    let (base, seen) = serve(Arc::new(|_| Box::pin(async { Reply::throttled(None) }))).await;
    let client = LlmClient::new().with_retry_base(Duration::from_millis(1));
    let (res, _) = run(&client, &profile(base), CancellationToken::new()).await;
    let err = res.expect_err("the request must fail once the tries are used up");
    assert_eq!(err.code, ErrorCode::ModelRateLimited, "{err:?}");
    assert_eq!(seen.load(Ordering::SeqCst), 8);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_retry_after_wait_is_cancelled_at_once() {
    let (base, seen) = serve(Arc::new(|_| {
        Box::pin(async { Reply::throttled(Some("30")) })
    }))
    .await;
    let client = LlmClient::new();
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        stopper.cancel();
    });
    let started = Instant::now();
    let (res, _) = run(&client, &profile(base), cancel).await;
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the wait ignored the cancel"
    );
    let err = res.expect_err("a cancelled request must fail");
    assert_eq!(err.code, ErrorCode::Cancelled, "{err:?}");
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_without_a_limit_runs_ten_requests_at_once() {
    // Each request is held until all ten have arrived; a gate that capped the profile would leave
    // the endpoint waiting and the requests would fail with 400 after the timeout.
    let (arrived_tx, arrived_rx) = tokio::sync::watch::channel(0usize);
    let arrived_tx = Arc::new(arrived_tx);
    let (base, seen) = serve(Arc::new(move |_| {
        let tx = arrived_tx.clone();
        let mut rx = arrived_rx.clone();
        Box::pin(async move {
            tx.send_modify(|n| *n += 1);
            let all_in =
                tokio::time::timeout(Duration::from_secs(10), rx.wait_for(|n| *n >= 10)).await;
            if matches!(all_in, Ok(Ok(_))) {
                Reply::ok()
            } else {
                Reply::status(400, "not all requests arrived at once")
            }
        })
    }))
    .await;

    let client = Arc::new(LlmClient::new());
    client.set_parallel_limit("p", 0);
    let profile = Arc::new(profile(base));
    let mut tasks = Vec::new();
    for _ in 0..10 {
        let (client, profile) = (client.clone(), profile.clone());
        tasks.push(tokio::spawn(async move {
            run(&client, &profile, CancellationToken::new()).await
        }));
    }
    for t in tasks {
        let (res, text) = t.await.unwrap();
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(text, "ok");
    }
    assert_eq!(seen.load(Ordering::SeqCst), 10);
}
