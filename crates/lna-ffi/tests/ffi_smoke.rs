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
