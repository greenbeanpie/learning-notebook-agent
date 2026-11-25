//! Shared test harness: a scriptable OpenAI-compatible mock server and a Core wrapper.
#![allow(dead_code)]

use lna_api::CoreRuntime;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the pipeline is asking the model for, derived from the system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Rewrite,
    Followups,
    HistorySummary,
    NotebookSummary,
    SourceIndex,
    Digest,
    Reduce,
    Plan,
    Studio,
    Chat,
    Other,
}

#[derive(Debug, Clone)]
pub struct Req {
    pub kind: Kind,
    pub body: Value,
    pub system: String,
    pub auth: Option<String>,
    /// How many requests of the same kind came before this one.
    pub nth: usize,
}

impl Req {
    pub fn has_tools(&self) -> bool {
        self.body
            .get("tools")
            .and_then(|t| t.as_array())
            .is_some_and(|a| !a.is_empty())
    }
    /// All message contents joined (system excluded).
    pub fn user_text(&self) -> String {
        self.body["messages"]
            .as_array()
            .map(|m| {
                m.iter()
                    .filter(|x| x["role"] != json!("system"))
                    .map(|x| x["content"].as_str().unwrap_or("").to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }
}

pub enum Reply {
    /// Normal streamed answer.
    Text(String),
    /// Streamed answer with a pause between 7-char chunks.
    SlowText(String, u64),
    /// Text, then an abrupt close with no finish frame (stream interruption).
    Truncated(String),
    /// A single tool call, finish_reason=tool_calls.
    Tool {
        id: String,
        name: String,
        args: Value,
    },
    /// HTTP error with a body.
    Status(u16, String),
    /// HTTP error with a Retry-After header (seconds) and a body.
    RetryAfter(u16, u64, String),
    /// 302 redirect to another URL.
    Redirect(String),
    /// Raw SSE frames (each a full `data: ...` payload string without the prefix).
    Frames(Vec<String>),
}

pub fn classify(system: &str, body: &Value) -> Kind {
    if system.contains("\"standalone\"") {
        Kind::Rewrite
    } else if system.contains("\"questions\"")
        && (system.contains("follow-up") || system.contains("追问"))
    {
        Kind::Followups
    } else if system.contains("under 300 characters") || system.contains("不超过 300 字") {
        Kind::HistorySummary
    } else if system.contains("guide_summary_md") {
        Kind::SourceIndex
    } else if system.contains("title_suggestion") || system.contains("summary_md") {
        Kind::NotebookSummary
    } else if system.contains("section summarization assistant") || system.contains("章节摘要助手")
    {
        Kind::Digest
    } else if system.contains("document structure synthesis assistant")
        || system.contains("资料结构整理助手")
    {
        Kind::Reduce
    } else if system.contains("content outline planner") || system.contains("内容大纲规划助手")
    {
        Kind::Plan
    } else if system.contains("Local Notebook Studio")
        || system.contains("Studio 生成器")
        || system.contains("executive presentation architect")
        || system.contains("senior presentation visual editor")
    {
        Kind::Studio
    } else if system.contains("<sources>")
        || system.contains("来源")
        || body["tools"].as_array().is_some_and(|a| !a.is_empty())
    {
        Kind::Chat
    } else {
        Kind::Other
    }
}

type Handler = dyn Fn(&Req) -> Reply + Send + Sync;

/// Embedding function used by the mock's `/embeddings` endpoint.
pub type Embedder = Arc<dyn Fn(&str) -> Vec<f32> + Send + Sync>;

pub struct MockLlm {
    pub port: u16,
    /// Number of texts embedded so far (all `/embeddings` calls).
    pub embedded_texts: Arc<std::sync::atomic::AtomicUsize>,
    /// When set, `/embeddings` answers with this HTTP status instead of vectors.
    pub embeddings_fail: Arc<AtomicBool>,
    pub requests: Arc<Mutex<Vec<Req>>>,
    tracker: Arc<Tracker>,
    stop: Arc<AtomicBool>,
    /// When set, capability-probe requests reach the handler (tests about the probe itself).
    pub raw_probe: Arc<AtomicBool>,
}

impl MockLlm {
    pub fn start(handler: impl Fn(&Req) -> Reply + Send + Sync + 'static) -> MockLlm {
        Self::start_with(handler, None)
    }

    pub fn start_with(
        handler: impl Fn(&Req) -> Reply + Send + Sync + 'static,
        embedder: Option<Embedder>,
    ) -> MockLlm {
        let embedded_texts = Arc::new(AtomicUsize::new(0));
        let embeddings_fail = Arc::new(AtomicBool::new(false));
        let emb_ctx = (embedder, embedded_texts.clone(), embeddings_fail.clone());
        let raw_probe = Arc::new(AtomicBool::new(false));
        let raw_probe_out = raw_probe.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests: Arc<Mutex<Vec<Req>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let handler: Arc<Handler> = Arc::new(handler);
        let tracker = Arc::new(Tracker::default());
        let tracker_out = tracker.clone();
        let counts: Arc<Mutex<Vec<(Kind, usize)>>> = Arc::new(Mutex::new(Vec::new()));
        {
            let requests = requests.clone();
            let stop = stop.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let h = handler.clone();
                            let requests = requests.clone();
                            let counts = counts.clone();
                            let tracker = tracker.clone();
                            let emb_ctx = emb_ctx.clone();
                            let raw = raw_probe.load(Ordering::SeqCst);
                            std::thread::spawn(move || {
                                let _ = serve(stream, h, requests, counts, tracker, emb_ctx, raw);
                            });
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            });
        }
        MockLlm {
            port,
            embedded_texts,
            embeddings_fail,
            requests,
            tracker: tracker_out,
            stop,
            raw_probe: raw_probe_out,
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    #[allow(dead_code)]
    pub fn all_requests(&self) -> Vec<Req> {
        self.requests.lock().unwrap().clone()
    }

    pub fn of_kind(&self, k: Kind) -> Vec<Req> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.kind == k)
            .cloned()
            .collect()
    }

    pub fn total(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

/// Tracks how many requests of each kind are being served at once.
#[derive(Default)]
pub struct Tracker {
    inner: Mutex<Vec<(Kind, usize, usize)>>,
}

pub struct InflightGuard<'a> {
    tracker: &'a Tracker,
    kind: Kind,
}

impl Tracker {
    fn enter(&self, kind: Kind) -> InflightGuard<'_> {
        let mut g = self.inner.lock().unwrap();
        match g.iter_mut().find(|(k, _, _)| *k == kind) {
            Some((_, cur, max)) => {
                *cur += 1;
                *max = (*max).max(*cur);
            }
            None => g.push((kind, 1, 1)),
        }
        InflightGuard {
            tracker: self,
            kind,
        }
    }
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        let mut g = self.tracker.inner.lock().unwrap();
        if let Some((_, cur, _)) = g.iter_mut().find(|(k, _, _)| *k == self.kind) {
            *cur -= 1;
        }
    }
}

impl MockLlm {
    /// Highest number of simultaneously served requests of this kind.
    pub fn max_inflight_of(&self, kind: Kind) -> usize {
        self.tracker
            .inner
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _, _)| *k == kind)
            .map(|(_, _, m)| *m)
            .unwrap_or(0)
    }
}

impl Drop for MockLlm {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn serve(
    mut stream: TcpStream,
    handler: Arc<Handler>,
    requests: Arc<Mutex<Vec<Req>>>,
    counts: Arc<Mutex<Vec<(Kind, usize)>>>,
    tracker: Arc<Tracker>,
    emb_ctx: (Option<Embedder>, Arc<AtomicUsize>, Arc<AtomicBool>),
    raw_probe: bool,
) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end;
    loop {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = p + 4;
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut content_length = 0usize;
    let mut auth = None;
    for line in head.lines() {
        let lower = line.to_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("authorization:") {
            auth = Some(line[14..].trim().to_string());
        }
    }
    let path = head
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    while buf.len() < header_end + content_length {
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body: Value = serde_json::from_slice(&buf[header_end..]).unwrap_or(json!({}));

    if path.ends_with("/embeddings") {
        let (embedder, counter, fail) = &emb_ctx;
        if fail.load(Ordering::SeqCst) || embedder.is_none() {
            let b = r#"{"error":{"message":"embedding backend down"}}"#;
            write!(
                stream,
                "HTTP/1.1 503 X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                b.len(),
                b
            )?;
            return Ok(());
        }
        let inputs: Vec<String> = match &body["input"] {
            Value::Array(a) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            Value::String(s) => vec![s.clone()],
            _ => Vec::new(),
        };
        counter.fetch_add(inputs.len(), Ordering::SeqCst);
        let f = embedder.as_ref().unwrap();
        let data: Vec<Value> = inputs
            .iter()
            .enumerate()
            .map(|(i, s)| json!({ "index": i, "embedding": f(s) }))
            .collect();
        let b = json!({ "data": data }).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            b.len(),
            b
        )?;
        return Ok(());
    }

    if path.ends_with("/models") {
        let b = json!({"data":[{"id":"mock-model"}]}).to_string();
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            b.len(),
            b
        )?;
        return Ok(());
    }

    let system: String = body["messages"]
        .as_array()
        .map(|m| {
            m.iter()
                .filter(|x| x["role"] == json!("system"))
                .map(|x| x["content"].as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if let Some(reply) = probe_reply(&system, &body).filter(|_| !raw_probe) {
        return write_reply(stream, reply);
    }
    let kind = classify(&system, &body);
    let guard = tracker.enter(kind);
    let nth = {
        let mut c = counts.lock().unwrap();
        match c.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, n)) => {
                let cur = *n;
                *n += 1;
                cur
            }
            None => {
                c.push((kind, 1));
                0
            }
        }
    };
    let req = Req {
        kind,
        body,
        system,
        auth,
        nth,
    };
    requests.lock().unwrap().push(req.clone());
    // like real OpenAI-compatible providers: every tool call must be answered by a tool message
    let reply = match unanswered_tool_call(&req.body) {
        Some(id) => Reply::Status(
            400,
            json!({ "error": { "message": format!("An assistant message with 'tool_calls' must be followed by tool messages responding to each 'tool_call_id'. Missing: {id}") } }).to_string(),
        ),
        None => handler(&req),
    };

    write_reply_inner(&mut stream, reply)?;
    drop(guard);
    // Close gracefully so buffered data is not discarded by a TCP reset on Windows.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}

/// The capability probe run by `profile.upsert` / `profile.test` is answered like a fully capable
/// model (a tool call and valid JSON) and is not recorded, so tests only see their own traffic.
fn probe_reply(system: &str, body: &Value) -> Option<Reply> {
    let tools = body["tools"].as_array();
    if tools.is_some_and(|a| {
        a.iter()
            .any(|t| t["function"]["name"] == json!("lna_capability_probe"))
    }) {
        return Some(Reply::Tool {
            id: "probe_1".into(),
            name: "lna_capability_probe".into(),
            args: json!({ "ok": true }),
        });
    }
    if system.starts_with("Return only a JSON object with ok set to true.") {
        return Some(Reply::Text(json!({ "ok": true }).to_string()));
    }
    if system == "Reply with OK." {
        return Some(Reply::Text("OK".into()));
    }
    None
}

fn write_reply(mut stream: TcpStream, reply: Reply) -> std::io::Result<()> {
    write_reply_inner(&mut stream, reply)?;
    let _ = stream.shutdown(std::net::Shutdown::Write);
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}

fn write_reply_inner(stream: &mut TcpStream, reply: Reply) -> std::io::Result<()> {
    let sse_head =
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
    let frame = |v: Value| format!("data: {v}\n\n");
    let text_chunks = |text: &str| -> Vec<String> {
        let chars: Vec<char> = text.chars().collect();
        chars
            .chunks(7)
            .map(|c| {
                frame(json!({"choices":[{"index":0,"delta":{"content":c.iter().collect::<String>()},"finish_reason":null}]}))
            })
            .collect()
    };
    let usage = frame(json!({"choices":[],"usage":{"prompt_tokens":100,"completion_tokens":20}}));
    match reply {
        Reply::Text(t) => {
            stream.write_all(sse_head.as_bytes())?;
            for c in text_chunks(&t) {
                stream.write_all(c.as_bytes())?;
            }
            stream.write_all(
                frame(json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}))
                    .as_bytes(),
            )?;
            stream.write_all(usage.as_bytes())?;
            stream.write_all(b"data: [DONE]\n\n")?;
        }
        Reply::SlowText(t, ms) => {
            stream.write_all(sse_head.as_bytes())?;
            for c in text_chunks(&t) {
                stream.write_all(c.as_bytes())?;
                stream.flush()?;
                std::thread::sleep(Duration::from_millis(ms));
            }
            stream.write_all(
                frame(json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}))
                    .as_bytes(),
            )?;
            stream.write_all(usage.as_bytes())?;
            stream.write_all(b"data: [DONE]\n\n")?;
        }
        Reply::Truncated(t) => {
            stream.write_all(sse_head.as_bytes())?;
            for c in text_chunks(&t) {
                stream.write_all(c.as_bytes())?;
            }
        }
        Reply::Tool { id, name, args } => {
            stream.write_all(sse_head.as_bytes())?;
            stream.write_all(
                frame(json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":id,"function":{"name":name,"arguments":args.to_string()}}]},"finish_reason":null}]}))
                    .as_bytes(),
            )?;
            stream.write_all(
                frame(json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}))
                    .as_bytes(),
            )?;
            stream.write_all(usage.as_bytes())?;
            stream.write_all(b"data: [DONE]\n\n")?;
        }
        Reply::Status(code, body) => {
            write!(
                stream,
                "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            )?;
        }
        Reply::RetryAfter(code, secs, body) => {
            write!(
                stream,
                "HTTP/1.1 {code} X\r\nretry-after: {secs}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            )?;
        }
        Reply::Redirect(url) => {
            write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: {url}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            )?;
        }
        Reply::Frames(frames) => {
            stream.write_all(sse_head.as_bytes())?;
            for f in frames {
                stream.write_all(format!("data: {f}\n\n").as_bytes())?;
            }
        }
    }
    stream.flush()
}

pub fn temp_dir(tag: &str) -> std::path::PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "lna_{tag}_{}_{nanos}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ))
}

pub struct Env {
    pub core: Arc<CoreRuntime>,
    pub dir: std::path::PathBuf,
}

impl Env {
    pub fn new(tag: &str) -> Env {
        let dir = temp_dir(tag);
        let core = CoreRuntime::open(dir.clone(), true).unwrap();
        Env { core, dir }
    }

    pub fn raw(&self, method: &str, params: Value) -> Value {
        let req = json!({ "id": "t", "method": method, "params": params }).to_string();
        serde_json::from_str(&self.core.handle_json(&req)).expect("core returned invalid JSON")
    }

    /// Calls a method and returns `result`, panicking with the full response on error.
    pub fn ok(&self, method: &str, params: Value) -> Value {
        let r = self.raw(method, params);
        assert!(
            r.get("error").is_none_or(|e| e.is_null()),
            "{method} failed: {r}"
        );
        r["result"].clone()
    }

    /// Calls a method expecting an error and returns its code.
    pub fn err(&self, method: &str, params: Value) -> String {
        let r = self.raw(method, params);
        r["error"]["code"]
            .as_str()
            .unwrap_or_else(|| panic!("{method} unexpectedly succeeded: {r}"))
            .to_string()
    }

    pub fn notebook(&self) -> String {
        self.ok("notebook.create", json!({ "title": "t" }))["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    pub fn add_text(&self, nid: &str, title: &str, text: &str) -> String {
        let r = self.ok(
            "source.add_text",
            json!({ "notebook_id": nid, "title": title, "text": text }),
        );
        r["source"]["id"].as_str().unwrap().to_string()
    }

    pub fn wait_sources_ready(&self, nid: &str, n: usize) {
        self.wait(30, || {
            let l = self.ok("source.list", json!({ "notebook_id": nid }));
            let s = l["sources"].as_array().unwrap();
            s.len() >= n && s.iter().all(|x| x["status"] == json!("READY"))
        });
    }

    pub fn wait(&self, secs: u64, mut f: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("condition not met within {secs}s");
    }

    /// Creates and activates a chat_completion profile that points at the mock.
    /// Saving runs the capability probe, which the mock answers as a fully capable model.
    pub fn profile(&self, mock: &MockLlm, ctx: u32) -> Value {
        self.ok(
            "profile.upsert",
            json!({
                "display_name": "mock", "protocol": "chat_completion",
                "base_url": mock.base_url(), "model_id": "mock-model", "activate": true,
                "context_window": ctx, "max_output_tokens": 1000
            }),
        )
    }

    pub fn history(&self, nid: &str) -> Vec<Value> {
        self.ok("chat.history", json!({ "notebook_id": nid }))["messages"]
            .as_array()
            .unwrap()
            .clone()
    }

    pub fn query<T>(&self, f: impl FnOnce(&rusqlite::Connection) -> T) -> T {
        let mut out = None;
        let mut f = Some(f);
        let _ = self.core.db.read(|c| {
            out = Some((f.take().unwrap())(c));
            Ok(())
        });
        out.unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub const ZH_DOC: &str = "# 第三章 向量空间\n\n## 3.5 基与维数\n\n向量空间的一组基是线性无关且张成整个空间的向量组。维数定理：同一向量空间的任意两组基包含相同数量的向量，这个数称为维数。\n\n矩阵的行秩与列秩相等，称为秩。";

/// Valid payloads for every Studio modality, chosen from the system prompt.
pub fn studio_reply(r: &Req) -> Reply {
    let s = &r.system;
    let user_text = r.body.to_string();
    let json = if s.contains("for lecture outline") {
        json!({"title":"基与维数讲解","objectives":["理解基","理解维数"],"sections":[{"title":"基与维数","focus":"3.5 节：基的定义与维数定理"}]})
    } else if s.contains("for lecture slides") {
        json!({"slides":[
            {"layout":"title","title":"基与维数","narration":[{"text":"今天我们来讲基与维数。","cues":[{"type":"laser","target":"title","shape":"ellipse","scale":1.2,"speed":0.8,"duration_ms":1500}]}]},
            {"layout":"figure","title":"一条曲线","bullets":["要点"],"citations":[1],
             "figure_request":{"kind":"plot","purpose":"示意","details":"curve id f: y=x^2"},
             "builds":[{"step":1,"targets":["figure:f"],"effect":"draw"}],
             "narration":[{"text":"先看这条曲线。","cues":[{"type":"step","n":1},{"type":"laser","target":"figure:f","shape":"circle","speed":1.0,"duration_ms":2000}]}]}
        ]})
    } else if s.contains("structured JSON for figure") {
        if user_text.contains("Render report") {
            json!({"ok": true})
        } else {
            json!({"kind":"plot","x_range":[-2,2],"elements":[{"id":"f","type":"curve","expr":"x^2","label":"y=x²"}]})
        }
    } else if s.contains("for lecture answer") {
        json!({"answer":"基是线性无关且张成空间的向量组[1]。","speak":"基是线性无关并且张成整个空间的向量组。","adapt":{"kind":"extend","reason":"需要补充基的例子"}})
    } else if s.contains("for quiz,") || s.contains("生成 quiz") {
        json!({"title":"测验","questions":[
            {"id":"q1","type":"single","difficulty":"medium","prompt":"n+1 个向量必然？","options":["线性相关","线性无关"],"answer_indices":[0],"accepted_answers":[],"rubric":"","explanation":"维数定理","citations":[1]}]})
    } else if s.contains("for flashcards,") || s.contains("生成 flashcards") {
        json!({"title":"闪卡","cards":[{"id":"c1","front":"基是什么","back":"线性无关的生成组","hint":"","citations":[1]}]})
    } else if s.contains("for mindmap") || s.contains("生成 mindmap") {
        json!({"title":"导图","nodes":[{"id":"n1","parent_id":null,"label":"根","citations":[1]},{"id":"n2","parent_id":"n1","label":"枝","citations":[1]}]})
    } else if s.contains("for report,") || s.contains("生成 report") {
        json!({"title":"报告","format":"document","sections":[{"heading":"概述","body_md":"基与维数[1]","key_takeaways":["要点"]}],"citations":[1]})
    } else if s.contains("for slides,") || s.contains("生成 slides") || s.contains("SlidesPayload")
    {
        json!({"title":"幻灯片","theme":"executive_navy","slides":[{"layout":"title","title":"封面","bullets":[],"speaker_notes":"","citations":[]},{"layout":"cards","title":"基","bullets":["线性无关"],"cards":[{"tag":"定义","title":"基","body":"线性无关且张成空间"}],"speaker_notes":"讲稿","citations":[1]}]})
    } else if s.contains("presentation architect") {
        return Reply::Text("Slide 1: title - 封面\nSlide 2: cards - 基".into());
    } else if s.contains("for infographic,") || s.contains("生成 infographic") {
        json!({"title":"信息图","subtitle":"副标题","orientation":"landscape","style":"clean","blocks":[{"heading":"维数","metric":"3","summary":"基的个数","citations":[1]}]})
    } else if s.contains("for audio,") || s.contains("生成 audio") {
        json!({"title":"音频","speakers":[{"id":"A","role":"host"},{"id":"B","role":"guest"}],"turns":[{"speaker":"A","text":"今天聊基与维数。","citations":[1]},{"speaker":"B","text":"好的。","citations":[]}]})
    } else {
        return Reply::Text("{}".into());
    };
    Reply::Text(json.to_string())
}

/// A tiny "concept" embedder: each language-independent concept is one dimension, so a Chinese
/// question and an English passage about the same idea end up close together.
pub fn concept_embedder() -> Embedder {
    const CONCEPTS: [&[&str]; 8] = [
        &["基", "维数", "basis", "dimension"],
        &["秩", "rank"],
        &["零空间", "null space", "nullspace"],
        &["特征", "eigen", "对角化", "diagonaliz"],
        &["行列式", "determinant"],
        &[
            "投影",
            "projection",
            "projected",
            "正交",
            "orthogonal",
            "orthonormal",
            "gram",
        ],
        &["奇异值", "singular", "pca", "主成分"],
        &["最小二乘", "least squares", "normal equations", "residual"],
    ];
    Arc::new(|text: &str| {
        let lower = text.to_lowercase();
        let mut v: Vec<f32> = CONCEPTS
            .iter()
            .map(|words| {
                words
                    .iter()
                    .map(|w| lower.matches(w).count() as f32)
                    .sum::<f32>()
            })
            .collect();
        v.push(0.05);
        v
    })
}

/// Serves fixed files over HTTP on 127.0.0.1 (for model download tests).
pub struct FileServer {
    pub port: u16,
    pub hits: Arc<Mutex<Vec<String>>>,
    /// Request lines that carried a Range header (resumed downloads).
    pub ranges: Arc<Mutex<Vec<String>>>,
}

impl FileServer {
    pub fn start(files: Vec<(&'static str, Vec<u8>)>) -> FileServer {
        Self::start_with(files, false)
    }

    /// `flaky`: the first full response of every file is cut off half way (connection closed).
    pub fn start_with(files: Vec<(&'static str, Vec<u8>)>, flaky: bool) -> FileServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let cut: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (h, rg) = (hits.clone(), ranges.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let files = files.clone();
                let (h, rg, cut) = (h.clone(), rg.clone(), cut.clone());
                std::thread::spawn(move || {
                    let mut buf = [0u8; 4096];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    h.lock().unwrap().push(path.clone());
                    let start: usize = req
                        .lines()
                        .find_map(|l| {
                            l.to_lowercase()
                                .strip_prefix("range: bytes=")
                                .map(str::to_string)
                        })
                        .and_then(|r| r.trim_end_matches('-').trim().parse().ok())
                        .unwrap_or(0);
                    if start > 0 {
                        rg.lock().unwrap().push(format!("{path} from {start}"));
                    }
                    match files.iter().find(|(p, _)| *p == path) {
                        Some((_, body)) => {
                            let body = &body[start.min(body.len())..];
                            let first_time = {
                                let mut c = cut.lock().unwrap();
                                if flaky && start == 0 && !c.contains(&path) {
                                    c.push(path.clone());
                                    true
                                } else {
                                    false
                                }
                            };
                            let status = if start > 0 {
                                "206 Partial Content"
                            } else {
                                "200 OK"
                            };
                            let _ = write!(
                                s,
                                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                                body.len()
                            );
                            let limit = if first_time {
                                body.len() / 2
                            } else {
                                body.len()
                            };
                            for part in body[..limit].chunks(8192) {
                                if s.write_all(part).is_err() {
                                    return;
                                }
                                std::thread::sleep(Duration::from_millis(2));
                            }
                        }
                        None => {
                            let _ = write!(
                                s,
                                "HTTP/1.1 404 X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                            );
                        }
                    }
                    let _ = s.shutdown(std::net::Shutdown::Write);
                    std::thread::sleep(Duration::from_millis(100));
                });
            }
        });
        FileServer { port, hits, ranges }
    }
}

/// The first tool_call id of an assistant message that is not answered by a following
/// `role: tool` message (chat-completions request bodies only).
pub fn unanswered_tool_call(body: &Value) -> Option<String> {
    let msgs = body["messages"].as_array()?;
    for (i, m) in msgs.iter().enumerate() {
        if m["role"] != json!("assistant") {
            continue;
        }
        for call in m["tool_calls"].as_array().into_iter().flatten() {
            let id = call["id"].as_str().unwrap_or("");
            let answered = msgs[i + 1..]
                .iter()
                .take_while(|x| x["role"] == json!("tool"))
                .any(|x| x["tool_call_id"].as_str() == Some(id));
            if !answered {
                return Some(id.to_string());
            }
        }
    }
    None
}
