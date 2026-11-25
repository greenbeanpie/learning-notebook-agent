use lna_api::CoreRuntime;
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn read_http_request(sock: &mut TcpStream) -> String {
    let mut buf = Vec::with_capacity(65536);
    let mut tmp = [0u8; 8192];
    let mut header_end = None;
    let mut content_len = 0usize;

    loop {
        match sock.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if header_end.is_none() {
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let hend = pos + 4;
                        header_end = Some(hend);
                        let hdr_s = String::from_utf8_lossy(&buf[..hend]);
                        for line in hdr_s.lines() {
                            if let Some((k, v)) = line.split_once(':') {
                                if k.trim().eq_ignore_ascii_case("content-length") {
                                    content_len = v.trim().parse().unwrap_or(0);
                                }
                            }
                        }
                    }
                }
                if let Some(hend) = header_end {
                    if buf.len() >= hend + content_len {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// `profile.upsert` runs the capability probe, which must be answered like a fully capable
/// model: a plain ping, one tool call and a valid JSON object.
fn probe_sse(req: &str, gemini: bool) -> Option<String> {
    let frames = |vs: Vec<Value>| {
        vs.iter()
            .map(|v| format!("data: {v}\n\n"))
            .collect::<String>()
    };
    let text = |t: &str| {
        if gemini {
            frames(vec![
                json!({ "candidates": [{ "content": { "parts": [{ "text": t }] } }] }),
                json!({ "candidates": [{ "finishReason": "STOP" }] }),
            ])
        } else {
            frames(vec![
                json!({ "choices": [{ "index": 0, "delta": { "content": t } }] }),
                json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }] }),
            ])
        }
    };
    if req.contains("Call the capability probe now.") {
        return Some(if gemini {
            frames(vec![
                json!({ "candidates": [{ "content": { "parts": [{ "functionCall": {
                    "name": "lna_capability_probe", "args": { "ok": true } } }] } }] }),
                json!({ "candidates": [{ "finishReason": "STOP" }] }),
            ])
        } else {
            frames(vec![
                json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{
                    "index": 0, "id": "probe_1",
                    "function": { "name": "lna_capability_probe", "arguments": "{\"ok\":true}" } }] } }] }),
                json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }] }),
            ])
        });
    }
    if req.contains("Return the probe object.") {
        return Some(text("{\"ok\":true}"));
    }
    if req.contains("Reply with OK.") {
        return Some(text("OK"));
    }
    None
}

#[test]
fn agent_autonomously_inspects_hybrid_pdf_pages_and_images_across_models() {
    let pdfium_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug");
    if !pdfium_dir.join("pdfium.dll").exists() && !pdfium_dir.join("libpdfium.so").exists() {
        eprintln!("skipped: no PDFium in target/debug");
        return;
    }
    std::env::set_var("LNA_PDFIUM_DIR", &pdfium_dir);

    let dir = std::env::temp_dir().join(format!(
        "lna_mm_api_{}_{}",
        std::process::id(),
        lna_types::now_ms()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let expected_sid = Arc::new(std::sync::Mutex::new(String::new()));
    let expected_sid_srv = expected_sid.clone();
    let vis_called = Arc::new(AtomicBool::new(false));
    let vis_called_srv = vis_called.clone();

    let (tx_addrs, rx_addrs) = std::sync::mpsc::channel();
    let srv_thread = std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let chat_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let chat_addr = chat_listener.local_addr().unwrap();
            let vis_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let vis_addr = vis_listener.local_addr().unwrap();
            tx_addrs.send((chat_addr, vis_addr)).unwrap();

            let chat_srv = tokio::spawn(async move {
                let mut round = 0usize;
                loop {
                    let (mut sock, _) = chat_listener.accept().await.unwrap();
                    let req_str = read_http_request(&mut sock).await;
                    if let Some(sse) = probe_sse(&req_str, false) {
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                            sse.len(),
                            sse
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                        continue;
                    }
                    let sid = expected_sid_srv.lock().unwrap().clone();
                    let is_chat_turn = req_str.contains("inspect_source_structure")
                        && req_str.contains("第2页的图表内容");
                    let sse = if !is_chat_turn {
                        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"[\\\"概览问题1\\\",\\\"概览问题2\\\"]\"}}]}\n\n\
                         data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n"
                            .to_string()
                    } else {
                        let s = match round {
                            0 => {
                                assert!(req_str.contains("inspect_source_pages"));
                                assert!(req_str.contains("inspect_source_images"));
                                let args_json = json!({ "source_id": sid }).to_string();
                                format!(
                                    "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"tool_calls\":[{{\"index\":0,\"id\":\"call_1\",\"function\":{{\"name\":\"inspect_source_structure\",\"arguments\":{}}}}}]}}}}]}}\n\n\
                                     data: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\n",
                                    serde_json::to_string(&args_json).unwrap()
                                )
                            }
                            1 => {
                                let args_json = json!({
                                    "source_id": sid,
                                    "pages": [2],
                                    "focus_question": "第2页的图表内容是什么？"
                                })
                                .to_string();
                                format!(
                                    "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"tool_calls\":[{{\"index\":0,\"id\":\"call_2\",\"function\":{{\"name\":\"inspect_source_pages\",\"arguments\":{}}}}}]}}}}]}}\n\n\
                                     data: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"tool_calls\"}}]}}\n\n",
                                    serde_json::to_string(&args_json).unwrap()
                                )
                            }
                            _ => {
                                assert!(
                                    req_str.contains("图表显示准确率达到 99%"),
                                    "missing vision output in req: {req_str}"
                                );
                                // Find the citation number assigned right before `(multi_page.pdf)`
                                let cite_tag = req_str
                                    .split("(multi_page.pdf)")
                                    .next()
                                    .and_then(|before| before.rsplit('[').next())
                                    .and_then(|num| num.split(']').next())
                                    .unwrap_or("1");
                                format!(
                                    "data: {{\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"根据视觉阅读第2页结果，图表准确率达到 99% [{cite_tag}]。\"}}}}]}}\n\n\
                                     data: {{\"choices\":[{{\"index\":0,\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\n"
                                )
                            }
                        };
                        round += 1;
                        s
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        sse.len(),
                        sse
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    if round >= 3 {
                        break;
                    }
                }
            });

            let vis_srv = tokio::spawn(async move {
                loop {
                    let (mut sock, _) = vis_listener.accept().await.unwrap();
                    let req_str = read_http_request(&mut sock).await;
                    if let Some(sse) = probe_sse(&req_str, true) {
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                            sse.len(),
                            sse
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                        continue;
                    }
                    assert!(
                        req_str.contains("inlineData") && req_str.contains("image/jpeg"),
                        "vision req missing inlineData/jpeg: {}",
                        &req_str[..req_str.len().min(600)]
                    );
                    vis_called_srv.store(true, Ordering::SeqCst);
                    let sse = "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"图表显示准确率达到 99%\"}]}}]}\n\n\
                               data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
                        sse.len(),
                        sse
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    break;
                }
            });

            let _ = tokio::join!(chat_srv, vis_srv);
        });
    });

    let (chat_addr, vis_addr) = rx_addrs.recv().unwrap();

    let core = CoreRuntime::open(dir.clone(), true).unwrap();
    let call = |method: &str, params: Value| {
        let req = json!({ "id": "1", "method": method, "params": params }).to_string();
        let raw = core.handle_json(&req);
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert!(v.get("error").is_none(), "{method} failed: {raw}");
        v["result"].clone()
    };

    let nb = call("notebook.create", json!({ "title": "HybridPdf" }));
    let nid = nb["id"].as_str().unwrap().to_string();

    call(
        "profile.upsert",
        json!({
            "id": "p_chat",
            "display_name": "Chat",
            "protocol": "chat_completion",
            "base_url": format!("http://{chat_addr}/v1"),
            "model_id": "deepseek-chat",
            "allow_insecure_http": true,
            "capabilities": { "tools": true, "streaming": true },
            "supports_vision": false,
            "activate": true
        }),
    );
    call(
        "profile.upsert",
        json!({
            "id": "p_vis",
            "display_name": "Gemini Vision",
            "protocol": "gemini",
            "base_url": format!("http://{vis_addr}/v1beta"),
            "model_id": "gemini-2.5-flash",
            "allow_insecure_http": true,
            "supports_vision": true,
            "activate": false
        }),
    );

    let pdf_src =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/pdf/multi_page.pdf");
    let copied = dir.join("multi_page.pdf");
    let mut f = std::fs::File::create(&copied).unwrap();
    f.write_all(&std::fs::read(&pdf_src).unwrap()).unwrap();

    let added = call(
        "source.add_files",
        json!({ "notebook_id": nid, "paths": [copied.to_string_lossy()] }),
    );
    *expected_sid.lock().unwrap() = added["items"][0]["source"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    for _ in 0..80 {
        let list = call("source.list", json!({ "notebook_id": nid }));
        if list["sources"][0]["status"] == json!("READY") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let list = call("source.list", json!({ "notebook_id": nid }));
    assert_eq!(list["sources"][0]["status"], json!("READY"));

    let reply = call(
        "chat.send",
        json!({ "notebook_id": nid, "text": "第2页的图表内容是什么？" }),
    );
    let content = reply["content"].as_str().unwrap();
    assert!(content.contains("99%"), "reply: {content}");
    assert!(vis_called.load(Ordering::SeqCst));
    assert!(reply["citations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["page_start"] == json!(2) && c["chunk_id"].is_null()));

    let cites = reply["citations"].as_array().unwrap();
    let cite_no = cites[0]["no"].as_i64().unwrap();
    let detail = call(
        "chat.citation_detail",
        json!({ "message_id": reply["id"], "citation_no": cite_no }),
    );
    assert_eq!(detail["source_removed"], json!(false));
    assert_eq!(detail["page_start"], json!(2));

    let _ = srv_thread.join();
    core.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
