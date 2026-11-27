//! The app-managed ffmpeg copy: status and removal. Nothing here touches the network; the download
//! and archive checks are unit tests in `lna-ingest`.
mod common;

use common::*;
use serde_json::json;

fn exe() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

#[test]
fn status_and_remove_follow_the_app_copy() {
    let env = Env::new("ffmpeg_status");
    // the core keeps ffmpeg in `<data dir>/tools`; this is the only test in this binary, so
    // its core is the one that fixes the folder

    // Empty data dir: only the shape is fixed (a system ffmpeg on PATH may still be found).
    let st = env.ok("ffmpeg.status", json!({}));
    for key in ["installed", "path", "version", "source", "supported"] {
        assert!(st.get(key).is_some(), "`{key}` missing from {st}");
    }
    let installed = st["installed"].as_bool().expect("installed is a bool");
    assert!(st["supported"].is_boolean());
    assert_eq!(st["path"].is_string(), installed, "{st}");
    match st["source"].as_str() {
        None => assert!(!installed, "{st}"),
        Some(src) => {
            assert!(installed, "{st}");
            assert_eq!(src, "system", "nothing is installed in the temp dir: {st}");
        }
    }

    // A copy in tools/ is the app's: status reports it, remove deletes exactly that file.
    let tools = env.dir.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    let copy = tools.join(exe());
    std::fs::write(&copy, b"not really ffmpeg").unwrap();

    let st = env.ok("ffmpeg.status", json!({}));
    assert_eq!(st["installed"], json!(true), "{st}");
    assert_eq!(st["source"], json!("app"), "{st}");
    assert_eq!(st["path"], json!(copy.to_string_lossy()), "{st}");
    assert!(
        st["version"].is_null(),
        "a non-runnable file has no version: {st}"
    );

    let after = env.ok("ffmpeg.remove", json!({}));
    assert!(!copy.exists(), "remove must delete the app copy");
    assert_ne!(after["source"], json!("app"), "{after}");
    assert!(after.get("supported").is_some(), "{after}");

    // Removing again is not an error.
    let _ = env.ok("ffmpeg.remove", json!({}));
}
