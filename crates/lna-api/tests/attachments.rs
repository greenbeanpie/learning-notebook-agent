//! Pictures attached to chat messages and Studio requests.
mod common;

use base64::Engine as _;
use common::*;
use serde_json::{json, Value};

fn png(w: u32, h: u32) -> String {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb([30, 90, 200])));
    let mut buf = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
    base64::engine::general_purpose::STANDARD.encode(buf.into_inner())
}

fn notebook(env: &Env) -> String {
    let nid = env.notebook();
    env.add_text(&nid, "讲义.md", ZH_DOC);
    env.wait_sources_ready(&nid, 1);
    nid
}

fn profile(env: &Env, mock: &MockLlm, vision: bool) {
    env.ok(
        "profile.upsert",
        json!({
            "display_name": "mock", "protocol": "chat_completion",
            "base_url": mock.base_url(), "model_id": "mock-model", "activate": true,
            "context_window": 32000, "max_output_tokens": 1000,
            "capabilities": { "streaming": true, "tools": false, "stream_usage": true, "supports_vision": vision }
        }),
    );
}

fn chat_body_text(mock: &MockLlm) -> String {
    mock.of_kind(Kind::Chat)
        .iter()
        .map(|r| r.body.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn vision_model_receives_the_picture_and_history_returns_it() {
    let mock = MockLlm::start(|_| Reply::Text("这是回答。".into()));
    let env = Env::new("att_vision");
    let nid = notebook(&env);
    profile(&env, &mock, true);

    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": "这道题怎么做", "images": [{ "name": "题.png", "data_base64": png(64, 48) }] }),
    );
    assert_eq!(msg["role"], json!("assistant"));

    let body = chat_body_text(&mock);
    assert!(body.contains("data:image/"), "image part must reach a vision model: {body}");

    let hist = env.history(&nid);
    let user = hist.iter().find(|m| m["role"] == json!("user")).unwrap();
    let atts = user["attachments"].as_array().expect("attachments on the user message");
    assert_eq!(atts.len(), 1);
    assert_eq!(atts[0]["name"], json!("题.png"));
    let path = atts[0]["path"].as_str().unwrap();
    assert!(std::path::Path::new(path).exists(), "stored file {path}");
    let asst = hist.iter().find(|m| m["role"] == json!("assistant")).unwrap();
    assert!(asst["attachments"].as_array().map_or(true, |a| a.is_empty()));
}

#[test]
fn text_only_model_gets_a_description_instead_of_pixels() {
    let mock = MockLlm::start(|_| Reply::Text("好的。".into()));
    let env = Env::new("att_text_model");
    let nid = notebook(&env);
    profile(&env, &mock, false);

    env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": "看图", "images": [{ "name": "a.png", "data_base64": png(32, 32) }] }),
    );
    let body = chat_body_text(&mock);
    assert!(!body.contains("data:image/"), "a model without vision must not get pixels");
    assert!(body.contains("attached_images"), "the recognised picture content is sent as text: {body}");
}

#[test]
fn picture_alone_is_a_valid_message_and_bad_pictures_are_rejected() {
    let mock = MockLlm::start(|_| Reply::Text("看到了。".into()));
    let env = Env::new("att_only");
    let nid = notebook(&env);
    profile(&env, &mock, true);

    env.ok("chat.send", json!({ "notebook_id": nid, "images": [{ "name": "a.png", "data_base64": png(16, 16) }] }));
    let n_before = env.history(&nid).len();

    let r = env.raw("chat.send", json!({ "notebook_id": nid, "text": "x", "images": [{ "name": "bad.png", "data_base64": "bm90IGFuIGltYWdl" }] }));
    assert_eq!(r["error"]["code"], json!("INVALID_ARGUMENT"), "{r}");
    assert!(r["error"]["message"].as_str().unwrap_or("").contains("bad.png"), "{r}");
    assert_eq!(env.err("chat.send", json!({ "notebook_id": nid })), "INVALID_ARGUMENT");
    assert_eq!(env.history(&nid).len(), n_before, "a rejected send stores nothing");
}

#[test]
fn regenerate_keeps_the_pictures_and_clear_removes_the_files() {
    let mock = MockLlm::start(|_| Reply::Text("答。".into()));
    let env = Env::new("att_regen");
    let nid = notebook(&env);
    profile(&env, &mock, true);

    env.ok("chat.send", json!({ "notebook_id": nid, "text": "问", "images": [{ "name": "a.png", "data_base64": png(20, 20) }] }));
    env.ok("chat.regenerate", json!({ "notebook_id": nid }));
    let hist = env.history(&nid);
    let user: Vec<&Value> = hist.iter().filter(|m| m["role"] == json!("user")).collect();
    assert_eq!(user.len(), 1);
    let path = user[0]["attachments"][0]["path"].as_str().unwrap().to_string();
    assert!(std::path::Path::new(&path).exists());
    let sent = mock.of_kind(Kind::Chat).iter().filter(|r| r.body.to_string().contains("data:image/")).count();
    assert!(sent >= 2, "the regenerated answer sees the picture again");

    env.ok("chat.clear", json!({ "notebook_id": nid }));
    assert!(!std::path::Path::new(&path).exists(), "clearing the chat deletes its pictures");
}

#[test]
fn studio_artifact_keeps_picture_ids_not_paths_and_delete_removes_files() {
    let mock = MockLlm::start(|r| match r.kind {
        Kind::Digest => Reply::Text("摘要".into()),
        Kind::Reduce => Reply::Text("大纲".into()),
        Kind::Plan => Reply::Text(r#"{"outline":["封面"]}"#.into()),
        Kind::Studio => studio_reply(r),
        _ => Reply::Text("图中是一道物理题。".into()),
    });
    let env = Env::new("att_studio");
    let nid = notebook(&env);
    profile(&env, &mock, true);

    let art = env.ok(
        "studio.create",
        json!({
            "notebook_id": nid, "artifact_type": "quiz", "config": { "topic": "图中的题" },
            "images": [{ "name": "q.png", "data_base64": png(40, 40) }]
        }),
    );
    let id = art["id"].as_str().unwrap().to_string();
    let ids = art["config"]["images"].as_array().expect("image ids in config");
    assert_eq!(ids.len(), 1);
    let cfg = art["config"].to_string();
    assert!(!cfg.contains("attachments"), "no file path may be echoed into prompts: {cfg}");

    env.wait(60, || {
        let a = env.ok("studio.get", json!({ "artifact_id": id }));
        a["status"] == json!("READY") || a["status"] == json!("FAILED")
    });
    // the picture was read once and its content reached the generation prompts
    let sent_pixels = mock.requests.lock().unwrap().iter().any(|r| r.body.to_string().contains("data:image/"));
    assert!(sent_pixels, "a vision model reads the attached picture");
    let any_brief = mock.requests.lock().unwrap().iter().any(|r| r.body.to_string().contains("user_images"));
    assert!(any_brief, "the picture content is part of the generation material");

    let dir = env.dir.join("attachments");
    // stored files, not their `.meta.json` sidecars
    let files = |d: &std::path::Path| {
        std::fs::read_dir(d)
            .map(|r| r.flatten().filter(|e| !e.file_name().to_string_lossy().ends_with(".json")).count())
            .unwrap_or(0)
    };
    assert_eq!(files(&dir), 1);
    env.ok("studio.delete", json!({ "artifact_id": id }));
    assert_eq!(files(&dir), 0, "deleting the artifact deletes its pictures");
}

#[test]
fn deleting_a_notebook_deletes_its_pictures() {
    let mock = MockLlm::start(|_| Reply::Text("答。".into()));
    let env = Env::new("att_nb_delete");
    let nid = notebook(&env);
    profile(&env, &mock, true);

    env.ok("chat.send", json!({ "notebook_id": nid, "text": "问", "images": [{ "name": "a.png", "data_base64": png(20, 20) }] }));
    let dir = env.dir.join("attachments");
    // stored files, not their `.meta.json` sidecars
    let files = |d: &std::path::Path| {
        std::fs::read_dir(d)
            .map(|r| r.flatten().filter(|e| !e.file_name().to_string_lossy().ends_with(".json")).count())
            .unwrap_or(0)
    };
    assert_eq!(files(&dir), 1);
    env.ok("notebook.delete", json!({ "notebook_id": nid }));
    assert_eq!(files(&dir), 0, "pictures must not outlive their notebook");
}

#[test]
fn picture_only_question_without_sources_reaches_the_model() {
    let mock = MockLlm::start(|_| Reply::Text("图中是一道两体问题。".into()));
    let env = Env::new("att_no_source");
    let nid = env.notebook(); // no sources at all
    profile(&env, &mock, true);

    env.ok("chat.send", json!({ "notebook_id": nid, "images": [{ "name": "题.png", "data_base64": png(40, 40) }] }));
    let body = chat_body_text(&mock);
    assert!(body.contains("data:image/"), "{body}");
    let hist = env.history(&nid);
    let user = hist.iter().find(|m| m["role"] == json!("user")).unwrap();
    assert_eq!(user["content"], json!(""), "no filler text is shown for a picture-only message");
    let asst = hist.iter().find(|m| m["role"] == json!("assistant")).unwrap();
    assert_eq!(asst["content"], json!("图中是一道两体问题。"));
}

#[test]
fn pictures_of_a_stored_message_survive_a_failed_send() {
    let env = Env::new("att_no_model");
    let nid = env.notebook();
    // no model profile: the send fails after the user message is stored
    let r = env.raw("chat.send", json!({ "notebook_id": nid, "text": "看图", "images": [{ "name": "a.png", "data_base64": png(20, 20) }] }));
    assert!(!r["error"].is_null(), "{r}");
    let hist = env.history(&nid);
    let user = hist.iter().find(|m| m["role"] == json!("user")).unwrap();
    let path = user["attachments"][0]["path"].as_str().unwrap();
    assert!(std::path::Path::new(path).exists(), "picture of the stored message was deleted");
}

// ---------- one-off reference files and links ----------

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The id of the attached file called `name`, from the manifest in the request.
fn attached_id(r: &Req, name: &str) -> Option<String> {
    let text = r.body.to_string().replace("\\\"", "\"");
    let key = format!("name=\"{name}\"");
    let at = text.find(&key)?;
    let head = &text[..at];
    let start = head.rfind("id=\"")? + 4;
    let end = start + head[start..].find('"')?;
    Some(head[start..end].to_string())
}

fn tool_results(r: &Req) -> Vec<String> {
    r.body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == json!("tool"))
        .map(|m| m["content"].as_str().unwrap_or("").to_string())
        .collect()
}

fn files_named(env: &Env, suffix: &str) -> Vec<String> {
    std::fs::read_dir(env.dir.join("attachments"))
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.ends_with(suffix))
                .collect()
        })
        .unwrap_or_default()
}

fn attachment_count(env: &Env) -> usize {
    std::fs::read_dir(env.dir.join("attachments")).map(|d| d.count()).unwrap_or(0)
}

#[test]
fn text_file_is_read_raw_without_parsing() {
    let mock = MockLlm::start(|r| {
        if r.kind != Kind::Chat || !r.has_tools() {
            return Reply::Text("{}".into());
        }
        let results = tool_results(r);
        if results.is_empty() {
            let id = attached_id(r, "notes.txt").expect("manifest lists the file");
            return Reply::Tool { id: "c1".into(), name: "read_attachment".into(), args: json!({ "id": id }) };
        }
        assert!(results[0].contains("拉格朗日中值定理"), "{results:?}");
        Reply::Text("附件里讲的是拉格朗日中值定理。".into())
    });
    let env = Env::new("ctx_text");
    let nid = env.notebook(); // no sources: the attachment is enough
    env.profile(&mock, 32000);
    let msg = env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": "附件讲了什么？",
                "files": [{ "name": "notes.txt", "data_base64": b64("第三节：拉格朗日中值定理。".as_bytes()) }] }),
    );
    assert_eq!(msg["content"], json!("附件里讲的是拉格朗日中值定理。"), "{msg}");
    assert!(files_named(&env, ".parsed.json").is_empty(), "a raw read must not parse the file");
    let hist = env.history(&nid);
    let user = hist.iter().find(|m| m["role"] == json!("user")).unwrap();
    assert_eq!(user["attachments"][0]["kind"], json!("file"));
    assert_eq!(user["attachments"][0]["name"], json!("notes.txt"));
}

#[test]
fn document_is_parsed_only_when_asked_and_once_and_follow_ups_can_read_it() {
    // PDFium is not available to the tests, so a Markdown document stands in for PDF/Word here
    let doc = "# 第一章 极限

数列极限的定义。

# 第二章 导数

导数的定义与几何意义：切线斜率。
";
    let mock = MockLlm::start(|r| {
        if r.kind != Kind::Chat || !r.has_tools() {
            return Reply::Text("{}".into());
        }
        let id = attached_id(r, "book.md").expect("manifest lists the file (also in follow-ups)");
        let results = tool_results(r);
        match results.len() {
            0 => Reply::Tool { id: "c1".into(), name: "parse_attachment".into(), args: json!({ "id": id }) },
            1 => {
                // pick the outline node of chapter 2 from the parse result
                let r0 = &results[0];
                let pos = r0.find("] 第二章").expect("outline lists chapter 2");
                let node = r0[..pos].rsplit('[').next().unwrap().to_string();
                Reply::Tool { id: "c2".into(), name: "read_parsed".into(), args: json!({ "id": id, "node_id": node }) }
            }
            _ => Reply::Text("读完了。".into()),
        }
    });
    let env = Env::new("ctx_doc");
    let nid = env.notebook();
    env.profile(&mock, 32000);
    env.ok(
        "chat.send",
        json!({ "notebook_id": nid, "text": "讲讲第二章", "files": [{ "name": "book.md", "data_base64": b64(doc.as_bytes()) }] }),
    );
    let reqs = mock.of_kind(Kind::Chat);
    let last = reqs.iter().rev().find(|r| tool_results(r).len() >= 2).expect("two tool rounds");
    let results = tool_results(last);
    assert!(results[0].contains("已解析"), "{results:?}");
    assert!(results[1].contains("切线斜率") && !results[1].contains("数列极限"), "only chapter 2: {results:?}");
    assert_eq!(files_named(&env, ".parsed.json").len(), 1);

    // a follow-up without the file still sees it and reuses the cache
    env.ok("chat.send", json!({ "notebook_id": nid, "text": "再看一下第二章" }));
    assert_eq!(files_named(&env, ".parsed.json").len(), 1);

    // clearing the conversation removes the file and its sidecars
    env.ok("chat.clear", json!({ "notebook_id": nid }));
    assert_eq!(attachment_count(&env), 0);
}

#[test]
fn a_numbered_problem_is_extracted_by_instruction_and_a_pdf_is_never_sent_whole() {
    let doc = "# 习题一

1. 求极限 $\\lim_{x\\to0} \\frac{\\sin x}{x}$。

2. 证明数列 $1/n$ 收敛。

# 习题二

3. 设 $f(x)=x^2$，求 $f'(3)$。
（1）写出导数的定义式；
（2）计算结果。

13. 求 $\\int_3^5 x\\,dx$。
";
    let mock = MockLlm::start(|r| {
        let body = r.body.to_string();
        if body.contains("You copy requested items") {
            // the extraction call: answer with what it was given for problem 3
            return Reply::Text("3. 设 $f(x)=x^2$，求 $f'(3)$。（1）写出导数的定义式；（2）计算结果。".into());
        }
        if r.kind != Kind::Chat || !r.has_tools() {
            return Reply::Text("{}".into());
        }
        let md = attached_id(r, "book.md").expect("book.md listed");
        let pdf = attached_id(r, "scan.pdf").expect("scan.pdf listed");
        match tool_results(r).len() {
            0 => Reply::Tool { id: "c1".into(), name: "read_attachment".into(), args: json!({ "id": pdf }) },
            1 => Reply::Tool {
                id: "c2".into(),
                name: "extract_from_attachment".into(),
                args: json!({ "id": md, "instruction": "第3题（含所有小问）" }),
            },
            _ => Reply::Text("好的。".into()),
        }
    });
    let env = Env::new("ctx_extract");
    let nid = env.notebook();
    env.profile(&mock, 32000);
    env.ok(
        "chat.send",
        json!({
            "notebook_id": nid,
            "text": "把第3题原样抄给我",
            "files": [
                { "name": "book.md", "data_base64": b64(doc.as_bytes()) },
                { "name": "scan.pdf", "data_base64": b64(b"%PDF-1.4 not really") }
            ]
        }),
    );
    let reqs = mock.of_kind(Kind::Chat);
    // no request ever carries the PDF itself
    assert!(
        mock.all_requests().iter().all(|r| !r.body.to_string().contains("application/pdf")),
        "the PDF is never handed over whole"
    );
    let last = reqs.iter().rev().find(|r| tool_results(r).len() >= 2).expect("two tool rounds");
    let results = tool_results(last);
    assert!(results[0].contains("parse_attachment") && results[0].contains("extract_from_attachment"), "{results:?}");
    assert!(results[1].contains("f'(3)") && results[1].contains("按文字提取"), "{results:?}");
    // the extraction call saw the located passage
    let ex = mock
        .all_requests()
        .into_iter()
        .find(|r| r.body.to_string().contains("You copy requested items"))
        .expect("extraction call");
    let ex_body = ex.body.to_string();
    assert!(ex_body.contains("f(x)=x^2") && ex_body.contains("第3题"), "{ex_body}");
}

#[test]
fn unsupported_formats_and_media_without_a_media_model_are_refused() {
    let mock = MockLlm::start(|_| Reply::Text("好".into()));
    let env = Env::new("ctx_refuse");
    let nid = env.notebook();
    env.profile(&mock, 32000);
    let r = env.raw(
        "chat.send",
        json!({ "notebook_id": nid, "text": "看", "files": [{ "name": "a.xyz", "data_base64": b64(b"x") }] }),
    );
    assert_eq!(r["error"]["code"], json!("SOURCE_UNSUPPORTED"), "{r}");
    assert!(r["error"]["message"].as_str().unwrap().contains("a.xyz"));
    // audio: the same rule as importing it as a source (no audio model configured here)
    let r = env.raw(
        "chat.send",
        json!({ "notebook_id": nid, "text": "听", "files": [{ "name": "talk.mp3", "data_base64": b64(b"ID3") }] }),
    );
    assert_eq!(r["error"]["code"], json!("MODEL_NOT_CONFIGURED"), "{r}");
    assert!(r["error"]["message"].as_str().unwrap().contains("音频"), "{r}");
    env.ok("settings.set", json!({ "key": "media_processing", "value": { "audio_video_enabled": false } }));
    let r = env.raw(
        "chat.send",
        json!({ "notebook_id": nid, "text": "听", "files": [{ "name": "talk.mp3", "data_base64": b64(b"ID3") }] }),
    );
    assert!(r["error"]["message"].as_str().unwrap().contains("已在设置中关闭音视频"), "{r}");
    assert_eq!(attachment_count(&env), 0, "nothing is stored for a refused request");
}

#[test]
fn studio_reads_attached_files_on_demand_and_keeps_only_ids() {
    let mock = MockLlm::start(|r| {
        if r.has_tools() && r.system.contains("attached reference files") {
            if tool_results(r).is_empty() {
                let id = attached_id(r, "lesson.md").unwrap();
                return Reply::Tool { id: "f1".into(), name: "read_attachment".into(), args: json!({ "id": id }) };
            }
            return Reply::Text("- 牛顿第二定律 F=ma（lesson.md）".into());
        }
        match r.kind {
            Kind::Studio => Reply::Text(
                json!({
                    "title": "测验",
                    "questions": [{ "id": "q1", "type": "single_choice", "question": "F=?", "options": ["ma", "mv"],
                                    "answer": 0, "explanation": "牛顿第二定律", "citations": [] }]
                })
                .to_string(),
            ),
            _ => Reply::Text("{}".into()),
        }
    });
    let env = Env::new("ctx_studio");
    let nid = env.notebook(); // no sources
    env.profile(&mock, 32000);
    let art = env.ok(
        "studio.create",
        json!({ "notebook_id": nid, "artifact_type": "quiz", "config": { "topic": "牛顿定律" },
                "files": [{ "name": "lesson.md", "data_base64": b64("# 牛顿第二定律\n\nF = ma".as_bytes()) }] }),
    );
    let ids = art["config"]["files"].as_array().expect("file ids in config");
    assert_eq!(ids.len(), 1);
    assert!(!art["config"].to_string().contains("attachments"), "no paths in config: {}", art["config"]);
    let id = art["id"].as_str().unwrap().to_string();
    env.wait(30, || {
        let a = env.ok("studio.get", json!({ "artifact_id": id }));
        a["status"] == json!("READY") || a["status"] == json!("FAILED")
    });
    let a = env.ok("studio.get", json!({ "artifact_id": id }));
    assert!(
        mock.of_kind(Kind::Studio).iter().any(|r| r.body.to_string().contains("牛顿第二定律 F=ma")),
        "the excerpt read from the file reaches the generation request: {a}
{:?}",
        mock.requests.lock().unwrap().iter().map(|r| (r.kind, r.system.chars().take(60).collect::<String>())).collect::<Vec<_>>()
    );
    env.ok("studio.delete", json!({ "artifact_id": id }));
    assert_eq!(attachment_count(&env), 0);
}
