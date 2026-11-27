//! MiMo (Xiaomi) chat API: inline audio/video wire format, and unchanged generic Chat Completion
//! behaviour for every other profile.

use lna_llm::{
    adapters::adapter_for, LlmMessage, LlmRequest, OutputMode, Part, ReasoningOptions,
    ResolvedProfile, Role, ToolMode,
};
use lna_types::{ErrorCode, ModelCapabilities, ProtocolKind};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn profile(preset: Option<&str>) -> ResolvedProfile {
    ResolvedProfile {
        id: "p_mimo".into(),
        display_name: "mimo".into(),
        protocol: ProtocolKind::ChatCompletion,
        base_url: "https://example.com/v1".into(),
        model_id: "mimo-test".into(),
        api_key: None,
        show_thinking: false,
        effort: None,
        context_window: 32000,
        max_output_tokens: 1024,
        capabilities: ModelCapabilities::default(),
        extra_headers: BTreeMap::new(),
        anthropic_version: "2023-06-01".into(),
        preset: preset.map(str::to_string),
        shrink_media: false,
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

fn media(mime: &str, b64: &str, audio_format: Option<&str>) -> Part {
    Part::Media {
        mime_type: mime.into(),
        data_base64: Some(b64.into()),
        file_uri: None,
        audio_format: audio_format.map(str::to_string),
    }
}

fn build(p: &ResolvedProfile, req: &LlmRequest) -> Result<Value, lna_types::LnaError> {
    let client = reqwest::Client::new();
    let built = adapter_for(ProtocolKind::ChatCompletion).build(&client, p, req)?;
    Ok(serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap())
}

#[test]
fn mimo_audio_and_video_use_data_url_wire_format() {
    let p = profile(Some("mimo"));
    let req = req_with_parts(vec![
        Part::Text("transcribe".into()),
        media("audio/mpeg", "QVVE", Some("mp3")),
        media("video/mp4", "VklE", None),
    ]);
    let body = build(&p, &req).unwrap();
    let content = &body["messages"][1]["content"];
    assert_eq!(content[0], json!({ "type": "text", "text": "transcribe" }));
    // Audio: data URL inside `data`, no `format` field.
    assert_eq!(
        content[1],
        json!({
            "type": "input_audio",
            "input_audio": { "data": "data:audio/mpeg;base64,QVVE" }
        })
    );
    // Video: fps and media_resolution are siblings of video_url.
    assert_eq!(
        content[2],
        json!({
            "type": "video_url",
            "video_url": { "url": "data:video/mp4;base64,VklE" },
            "fps": 1,
            "media_resolution": "default"
        })
    );
}

#[test]
fn mimo_images_stay_image_url() {
    let p = profile(Some("mimo"));
    let req = req_with_parts(vec![media("image/png", "UE5H", None)]);
    let body = build(&p, &req).unwrap();
    assert_eq!(
        body["messages"][1]["content"][0],
        json!({ "type": "image_url", "image_url": { "url": "data:image/png;base64,UE5H" } })
    );
}

#[test]
fn mimo_accepts_every_listed_format() {
    let p = profile(Some("mimo"));
    for mime in [
        "audio/mpeg",
        "audio/wav",
        "audio/x-wav",
        "audio/flac",
        "audio/mp4",
        "audio/x-m4a",
        "audio/ogg",
    ] {
        let req = req_with_parts(vec![media(mime, "QQ==", None)]);
        let body = build(&p, &req).unwrap();
        assert_eq!(
            body["messages"][1]["content"][0]["type"],
            json!("input_audio"),
            "{mime}"
        );
    }
    for mime in [
        "video/mp4",
        "video/quicktime",
        "video/x-msvideo",
        "video/x-ms-wmv",
    ] {
        let req = req_with_parts(vec![media(mime, "QQ==", None)]);
        let body = build(&p, &req).unwrap();
        assert_eq!(
            body["messages"][1]["content"][0]["type"],
            json!("video_url"),
            "{mime}"
        );
    }
}

#[test]
fn mimo_rejects_unlisted_formats_with_allowed_list() {
    let p = profile(Some("mimo"));

    let webm = req_with_parts(vec![media("video/webm", "QQ==", None)]);
    let err = build(&p, &webm).unwrap_err();
    assert_eq!(err.code, ErrorCode::CapabilityMissing);
    assert!(err.message.contains("MP4"), "{}", err.message);
    assert!(err.message.contains("video/webm"), "{}", err.message);

    let aac = req_with_parts(vec![media("audio/aac", "QQ==", None)]);
    let err = build(&p, &aac).unwrap_err();
    assert_eq!(err.code, ErrorCode::CapabilityMissing);
    assert!(err.message.contains("FLAC"), "{}", err.message);
}

#[test]
fn mimo_rejects_base64_over_50_mib() {
    let p = profile(Some("mimo"));
    let limit = 50 * 1024 * 1024;

    let at_limit = "A".repeat(limit);
    let ok = req_with_parts(vec![media("video/mp4", &at_limit, None)]);
    assert!(build(&p, &ok).is_ok());

    let over = "A".repeat(limit + 1);
    let req = req_with_parts(vec![media("audio/mpeg", &over, Some("mp3"))]);
    let err = build(&p, &req).unwrap_err();
    assert_eq!(err.code, ErrorCode::CapabilityMissing);
    assert!(err.message.contains("50 MB"), "{}", err.message);
}

#[test]
fn non_mimo_chat_completion_audio_is_unchanged() {
    // Generic profiles keep the `format` field and never get the video_url shape.
    for preset in [None, Some("openai"), Some("cloudflare")] {
        let p = profile(preset);
        let req = req_with_parts(vec![media("audio/mpeg", "QVVE", Some("mp3"))]);
        let body = build(&p, &req).unwrap();
        assert_eq!(
            body["messages"][1]["content"][0],
            json!({
                "type": "input_audio",
                "input_audio": { "data": "QVVE", "format": "mp3" }
            }),
            "preset {preset:?}"
        );
    }
}

#[test]
fn non_mimo_chat_completion_still_rejects_video() {
    for preset in [None, Some("openai")] {
        let p = profile(preset);
        let req = req_with_parts(vec![media("video/mp4", "VklE", None)]);
        let err = build(&p, &req).unwrap_err();
        assert_eq!(err.code, ErrorCode::CapabilityMissing, "preset {preset:?}");
    }
}

#[test]
fn mimo_detection_follows_preset_id() {
    assert!(profile(Some("mimo")).is_mimo());
    assert!(!profile(None).is_mimo());
    assert!(!profile(Some("mimo-other")).is_mimo());
}
