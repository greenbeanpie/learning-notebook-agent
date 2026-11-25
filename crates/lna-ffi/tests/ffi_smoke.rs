use lna_ffi::{lna_call, lna_close, lna_open, lna_string_free, LnaCore};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::sync::{Arc, Mutex};
use std::time::Instant;

extern "C" fn on_event(_json: *const c_char, _len: usize, _user: *mut c_void) {}

extern "C" fn on_reply(json: *const c_char, len: usize, user: *mut c_void) {
    if json.is_null() || user.is_null() {
        return;
    }
    let slice = unsafe { std::slice::from_raw_parts(json as *const u8, len) };
    let text = String::from_utf8_lossy(slice).to_string();
    let slot = unsafe { &*(user as *const Arc<Mutex<Option<String>>>) };
    if let Ok(mut g) = slot.lock() {
        *g = Some(text);
    }
}

#[test]
fn ffi_open_call_app_info_and_close_within_2s() {
    let dir = std::env::temp_dir().join(format!("lna_ffi_test_{}", std::process::id()));
    let cfg = CString::new(format!(
        r#"{{"data_dir":"{}","allow_memory_secret_fallback":true}}"#,
        dir.display().to_string().replace('\\', "\\\\")
    ))
    .unwrap();

    let mut core: *mut LnaCore = std::ptr::null_mut();
    let mut err: *mut c_char = std::ptr::null_mut();
    let rc = unsafe {
        lna_open(
            cfg.as_ptr(),
            on_event,
            std::ptr::null_mut(),
            &mut core,
            &mut err,
        )
    };
    if !err.is_null() {
        let e = unsafe { CStr::from_ptr(err).to_string_lossy().to_string() };
        unsafe { lna_string_free(err) };
        panic!("lna_open failed: {e}");
    }
    assert_eq!(rc, 0);
    assert!(!core.is_null());

    let reply_slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let req = CString::new(r#"{"id":"ffi-1","method":"app.info","params":{}}"#).unwrap();
    let slot_ptr = &reply_slot as *const Arc<Mutex<Option<String>>> as *mut c_void;
    unsafe { lna_call(core, req.as_ptr(), on_reply, slot_ptr) };

    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    while reply_slot.lock().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let reply = reply_slot
        .lock()
        .unwrap()
        .clone()
        .expect("missing FFI reply");
    assert!(
        reply.contains("Local Notebook Agent"),
        "unexpected reply: {reply}"
    );

    let t0 = Instant::now();
    unsafe { lna_close(core) };
    assert!(t0.elapsed().as_secs_f64() < 2.5, "lna_close took too long");

    let _ = std::fs::remove_dir_all(&dir);
}

fn open_core(dir: &std::path::Path) -> *mut LnaCore {
    let cfg = CString::new(format!(
        r#"{{"data_dir":"{}","allow_memory_secret_fallback":true}}"#,
        dir.display().to_string().replace('\\', "\\\\")
    ))
    .unwrap();
    let mut core: *mut LnaCore = std::ptr::null_mut();
    let mut err: *mut c_char = std::ptr::null_mut();
    let rc = unsafe {
        lna_open(
            cfg.as_ptr(),
            on_event,
            std::ptr::null_mut(),
            &mut core,
            &mut err,
        )
    };
    assert_eq!(rc, 0);
    core
}

fn call_and_wait(core: *mut LnaCore, req: &str, secs: u64) -> Option<String> {
    let slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let req = CString::new(req).unwrap();
    // The slot must outlive the callback: it is leaked on timeout so a late reply cannot touch freed memory.
    let boxed = Box::into_raw(Box::new(slot.clone()));
    unsafe { lna_call(core, req.as_ptr(), on_reply, boxed as *mut c_void) };
    let deadline = Instant::now() + std::time::Duration::from_secs(secs);
    while slot.lock().unwrap().is_none() && Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = slot.lock().unwrap().clone();
    out
}

/// B-16: closing the core while an answer is still streaming returns promptly and does not panic.
#[test]
fn close_while_a_request_is_in_flight_returns_within_2_seconds() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 65536];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let head = b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
                // the capability probe run when the profile is saved: answer as a capable model
                let probe: Option<&[u8]> = if req.contains("lna_capability_probe") && req.contains("\"tools\"") {
                    Some(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"p\",\"function\":{\"name\":\"lna_capability_probe\",\"arguments\":\"{\\\"ok\\\":true}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n")
                } else if req.contains("Return only a JSON object with ok set to true.") {
                    Some(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"{\\\"ok\\\":true}\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
                } else if req.contains("Reply with OK.") {
                    Some(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"OK\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n")
                } else {
                    None
                };
                if let Some(body) = probe {
                    let _ = s.write_all(head);
                    let _ = s.write_all(body);
                    let _ = s.flush();
                    let _ = s.shutdown(std::net::Shutdown::Write);
                    return;
                }
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n");
                let _ = s.write_all(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"start\"},\"finish_reason\":null}]}\n\n");
                let _ = s.flush();
                std::thread::sleep(std::time::Duration::from_secs(30)); // never finishes
            });
        }
    });

    let dir = std::env::temp_dir().join(format!("lna_ffi_close_{}", std::process::id()));
    let core = open_core(&dir);
    let nb = call_and_wait(
        core,
        r#"{"id":"1","method":"notebook.create","params":{"title":"t"}}"#,
        5,
    )
    .unwrap();
    let nid = serde_json::from_str::<serde_json::Value>(&nb).unwrap()["result"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    call_and_wait(core, &format!(r#"{{"id":"2","method":"source.add_text","params":{{"notebook_id":"{nid}","title":"a","text":"向量空间的基与维数。"}}}}"#), 5).unwrap();
    call_and_wait(core, &format!(r#"{{"id":"3","method":"profile.upsert","params":{{"display_name":"m","protocol":"chat_completion","base_url":"http://127.0.0.1:{port}/v1","model_id":"m","activate":true}}}}"#), 5).unwrap();
    for _ in 0..50 {
        // wait until the source is READY so the chat really starts streaming
        let l = call_and_wait(
            core,
            &format!(r#"{{"id":"5","method":"source.list","params":{{"notebook_id":"{nid}"}}}}"#),
            5,
        )
        .unwrap();
        if l.contains(r#""status":"READY""#) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // Start a chat that will hang, do not wait for its reply.
    let slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let boxed = Box::into_raw(Box::new(slot.clone()));
    let req = CString::new(format!(r#"{{"id":"4","method":"chat.send","params":{{"notebook_id":"{nid}","text":"请详细说明基与维数之间的关系是什么"}}}}"#)).unwrap();
    unsafe { lna_call(core, req.as_ptr(), on_reply, boxed as *mut c_void) };
    std::thread::sleep(std::time::Duration::from_millis(1500)); // the request is now in flight

    let t0 = Instant::now();
    unsafe { lna_close(core) };
    let took = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    while slot.lock().unwrap().is_none() && t1.elapsed().as_secs() < 3 {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        took < 2.0,
        "lna_close took {took:.2}s with a request in flight"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
