use lna_llm::{
    adapters::adapter_for, LlmClient, LlmMessage, LlmRequest, OutputMode, Part, ReasoningOptions,
    ResolvedProfile, Role, ToolMode,
};
use lna_types::{ErrorCode, ModelCapabilities, ProtocolKind};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn profile(protocol: ProtocolKind, base_url: &str) -> ResolvedProfile {
    ResolvedProfile {
        id: "p_mm".into(),
        display_name: "mm".into(),
        protocol,
        base_url: base_url.into(),
        model_id: "test-model".into(),
        api_key: None,
        show_thinking: false,
        effort: None,
        context_window: 32000,
        max_output_tokens: 1024,
        capabilities: ModelCapabilities::default(),
        extra_headers: BTreeMap::new(),
        anthropic_version: "2023-06-01".into(),
    }
}

fn req_with_parts(parts: Vec<Part>) -> LlmRequest {
    LlmRequest {
        system: "sys".into(),
        messages: vec![LlmMessage {
            role: Role::User,
            parts,
        }],
        tools: vec![],
        tool_mode: ToolMode::None,
        output: OutputMode::Text,
        max_output_tokens: 512,
        reasoning: ReasoningOptions::default(),
    }
}

#[test]
fn gemini_serializes_inline_data_and_file_uri() {
    let client = reqwest::Client::new();
    let req = req_with_parts(vec![
        Part::Media {
            mime_type: "image/png".into(),
            data_base64: Some("QUJD".into()),
            file_uri: None,
            audio_format: None,
        },
        Part::Media {
            mime_type: "video/mp4".into(),
            data_base64: None,
            file_uri: Some("https://generativelanguage.googleapis.com/v1beta/files/v1".into()),
            audio_format: None,
        },
        Part::Text("describe".into()),
    ]);
    let built = adapter_for(ProtocolKind::Gemini)
        .build(&client, &profile(ProtocolKind::Gemini, "https://example.com/v1beta"), &req)
        .unwrap();
    let body: Value = serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(
        body["contents"][0]["parts"][0]["inlineData"],
        json!({ "mimeType": "image/png", "data": "QUJD" })
    );
    assert_eq!(
        body["contents"][0]["parts"][1]["fileData"],
        json!({
            "mimeType": "video/mp4",
            "fileUri": "https://generativelanguage.googleapis.com/v1beta/files/v1"
        })
    );
    assert_eq!(body["contents"][0]["parts"][2]["text"], json!("describe"));
}

#[test]
fn chat_completion_serializes_image_and_input_audio_and_rejects_video() {
    let client = reqwest::Client::new();
    let p = profile(ProtocolKind::ChatCompletion, "https://example.com/v1");
    let req = req_with_parts(vec![
        Part::Text("check".into()),
        Part::Media {
            mime_type: "image/jpeg".into(),
            data_base64: Some("SU1H".into()),
            file_uri: None,
            audio_format: None,
        },
        Part::Media {
            mime_type: "audio/mpeg".into(),
            data_base64: Some("QVVE".into()),
            file_uri: None,
            audio_format: Some("mp3".into()),
        },
    ]);
    let built = adapter_for(ProtocolKind::ChatCompletion)
        .build(&client, &p, &req)
        .unwrap();
    let body: Value = serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(body["messages"][1]["content"][0], json!({ "type": "text", "text": "check" }));
    assert_eq!(
        body["messages"][1]["content"][1],
        json!({ "type": "image_url", "image_url": { "url": "data:image/jpeg;base64,SU1H" } })
    );
    assert_eq!(
        body["messages"][1]["content"][2],
        json!({ "type": "input_audio", "input_audio": { "data": "QVVE", "format": "mp3" } })
    );

    let vid_req = req_with_parts(vec![Part::Media {
        mime_type: "video/mp4".into(),
        data_base64: Some("VklE".into()),
        file_uri: None,
        audio_format: None,
    }]);
    let err = adapter_for(ProtocolKind::ChatCompletion)
        .build(&client, &p, &vid_req)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::CapabilityMissing);
}

#[test]
fn responses_and_anthropic_serialize_images_and_reject_audio() {
    let client = reqwest::Client::new();
    let img_req = req_with_parts(vec![
        Part::Text("ocr".into()),
        Part::Media {
            mime_type: "image/png".into(),
            data_base64: Some("UE5H".into()),
            file_uri: None,
            audio_format: None,
        },
    ]);

    let resp_built = adapter_for(ProtocolKind::Responses)
        .build(&client, &profile(ProtocolKind::Responses, "https://example.com/v1"), &img_req)
        .unwrap();
    let r_body: Value =
        serde_json::from_slice(resp_built.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(
        r_body["input"][0]["content"][1],
        json!({ "type": "input_image", "image_url": "data:image/png;base64,UE5H" })
    );

    let ant_built = adapter_for(ProtocolKind::Anthropic)
        .build(&client, &profile(ProtocolKind::Anthropic, "https://example.com/v1"), &img_req)
        .unwrap();
    let a_body: Value =
        serde_json::from_slice(ant_built.body().unwrap().as_bytes().unwrap()).unwrap();
    assert_eq!(
        a_body["messages"][0]["content"][1],
        json!({
            "type": "image",
            "source": { "type": "base64", "media_type": "image/png", "data": "UE5H" }
        })
    );

    let aud_req = req_with_parts(vec![Part::Media {
        mime_type: "audio/mpeg".into(),
        data_base64: Some("TVAz".into()),
        file_uri: None,
        audio_format: Some("mp3".into()),
    }]);
    assert_eq!(
        adapter_for(ProtocolKind::Anthropic)
            .build(&client, &profile(ProtocolKind::Anthropic, "https://example.com/v1"), &aud_req)
            .unwrap_err()
            .code,
        ErrorCode::CapabilityMissing
    );
}

#[tokio::test]
async fn gemini_files_api_resumable_upload_and_delete_roundtrip() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}/v1beta");

    let srv = tokio::spawn(async move {
        for step in 0..3 {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            let req_text = String::from_utf8_lossy(&buf[..n]).to_string();
            let resp = match step {
                0 => {
                    assert!(req_text.contains("POST /upload/v1beta/files"));
                    format!(
                        "HTTP/1.1 200 OK\r\nX-Goog-Upload-URL: http://{addr}/upload-session/1\r\nContent-Length: 0\r\n\r\n"
                    )
                }
                1 => {
                    assert!(req_text.contains("POST /upload-session/1"));
                    let body = json!({
                        "file": {
                            "name": "files/test123",
                            "uri": format!("http://{addr}/v1beta/files/test123"),
                            "state": "ACTIVE"
                        }
                    })
                    .to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    )
                }
                _ => {
                    assert!(req_text.starts_with("DELETE /v1beta/files/test123"));
                    "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".to_string()
                }
            };
            sock.write_all(resp.as_bytes()).await.unwrap();
        }
    });

    let tmp = std::env::temp_dir().join(format!("lna_gemini_up_{}.mp4", std::process::id()));
    std::fs::write(&tmp, b"fake mp4 payload").unwrap();

    let llm = LlmClient::new();
    let prof = profile(ProtocolKind::Gemini, &base);
    let (uri, name) = llm
        .upload_gemini_file(&prof, &tmp, "video/mp4", "demo.mp4")
        .await
        .unwrap();
    assert_eq!(name, "files/test123");
    assert!(uri.ends_with("/files/test123"));
    llm.delete_gemini_file(&prof, &name).await;

    let _ = std::fs::remove_file(&tmp);
    srv.await.unwrap();
}
