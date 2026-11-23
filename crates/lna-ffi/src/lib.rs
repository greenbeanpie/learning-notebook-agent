#[cfg(target_os = "android")]
mod android_jni;

use lna_api::CoreRuntime;
use serde::Deserialize;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_void};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub type LnaCallback = extern "C" fn(json: *const c_char, len: usize, user: *mut c_void);

#[repr(C)]
pub struct LnaCore {
    runtime: Arc<CoreRuntime>,
    event_task: Option<std::thread::JoinHandle<()>>,
}

#[derive(Deserialize)]
struct OpenConfig {
    data_dir: String,
    #[serde(default)]
    allow_memory_secret_fallback: bool,
}

/// Opens a core instance and starts forwarding events to `on_event`.
///
/// # Safety
/// `config_json` must be a valid NUL-terminated UTF-8 string. `out` must be a
/// valid writable pointer; `out_error` may be null or must be writable, and any
/// string written to it must be released with [`lna_string_free`]. `on_event`
/// is invoked from a background thread and `user` must stay valid until
/// [`lna_close`] returns.
#[no_mangle]
pub unsafe extern "C" fn lna_open(
    config_json: *const c_char,
    on_event: LnaCallback,
    user: *mut c_void,
    out: *mut *mut LnaCore,
    out_error: *mut *mut c_char,
) -> i32 {
    let res = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if config_json.is_null() || out.is_null() {
            return Err("Invalid null pointer passed to lna_open".to_string());
        }
        let cfg_str = CStr::from_ptr(config_json)
            .to_str()
            .map_err(|e| format!("Invalid UTF-8 config: {e}"))?;
        let cfg: OpenConfig =
            serde_json::from_str(cfg_str).map_err(|e| format!("Invalid config JSON: {e}"))?;

        let runtime = CoreRuntime::open(
            PathBuf::from(cfg.data_dir),
            cfg.allow_memory_secret_fallback,
        )
        .map_err(|e| e.message)?;

        let mut rx = runtime.events.subscribe();
        let user_addr = user as usize;
        let handle = std::thread::spawn(move || loop {
            match rx.blocking_recv() {
                Ok(ev) => {
                    if let Ok(json_s) = serde_json::to_string(&ev) {
                        if let Ok(cstr) = CString::new(json_s) {
                            let bytes = cstr.as_bytes();
                            on_event(cstr.as_ptr(), bytes.len(), user_addr as *mut c_void);
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        });

        let boxed = Box::new(LnaCore {
            runtime,
            event_task: Some(handle),
        });
        *out = Box::into_raw(boxed);
        Ok(())
    }));

    match res {
        Ok(Ok(())) => 0,
        Ok(Err(msg)) => {
            if !out_error.is_null() {
                if let Ok(cstr) = CString::new(msg) {
                    *out_error = cstr.into_raw();
                }
            }
            -1
        }
        Err(_) => {
            if !out_error.is_null() {
                if let Ok(cstr) = CString::new("Panic in lna_open") {
                    *out_error = cstr.into_raw();
                }
            }
            -2
        }
    }
}

/// Dispatches one JSON-RPC request; the reply is delivered once to `on_reply`.
///
/// # Safety
/// `core` must be null or a live pointer returned by [`lna_open`] that has not
/// been passed to [`lna_close`]. `request_json` must be a valid NUL-terminated
/// string. `on_reply` runs on a background thread and `user` must stay valid
/// until it has been called.
#[no_mangle]
pub unsafe extern "C" fn lna_call(
    core: *mut LnaCore,
    request_json: *const c_char,
    on_reply: LnaCallback,
    user: *mut c_void,
) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if core.is_null() || request_json.is_null() {
            return;
        }
        let core_ref = &*core;
        let req_str = match CStr::from_ptr(request_json).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return,
        };
        let rt = core_ref.runtime.clone();
        let user_addr = user as usize;
        std::thread::spawn(move || {
            let reply = rt.handle_json(&req_str);
            if let Ok(cstr) = CString::new(reply) {
                let bytes = cstr.as_bytes();
                on_reply(cstr.as_ptr(), bytes.len(), user_addr as *mut c_void);
            }
        });
    }));
}

/// Shuts the core down, waiting at most about two seconds.
///
/// # Safety
/// `core` must be null or a pointer returned by [`lna_open`]; it must not be
/// used again after this call.
#[no_mangle]
pub unsafe extern "C" fn lna_close(core: *mut LnaCore) {
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        if core.is_null() {
            return;
        }
        let mut boxed = Box::from_raw(core);
        let rt = boxed.runtime.clone();
        let ev_handle = boxed.event_task.take();
        drop(boxed);

        // Shut down runtime on a dedicated OS thread with bounded wait (B-16)
        let join_close = std::thread::spawn(move || {
            rt.shutdown();
            drop(rt);
            if let Some(h) = ev_handle {
                let _ = h.join();
            }
        });
        let deadline = std::time::Instant::now() + Duration::from_millis(1800);
        while !join_close.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
    }));
}

/// Releases a string allocated by this library.
///
/// # Safety
/// `s` must be null or a pointer this library handed out (such as the
/// `out_error` of [`lna_open`]), and must be freed only once.
#[no_mangle]
pub unsafe extern "C" fn lna_string_free(s: *mut c_char) {
    if !s.is_null() {
        let _ = CString::from_raw(s);
    }
}
