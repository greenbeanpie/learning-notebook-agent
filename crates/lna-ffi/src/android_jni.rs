#![cfg(target_os = "android")]

use jni::objects::{GlobalRef, JClass, JObject, JString, JValue};
use jni::sys::jlong;
use jni::{JNIEnv, JavaVM};
use lna_api::CoreRuntime;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

struct AndroidCore {
    runtime: Arc<CoreRuntime>,
    event_thread: Option<std::thread::JoinHandle<()>>,
}

#[no_mangle]
pub extern "system" fn Java_com_localnotebook_agent_core_NativeBridge_nativeOpen(
    mut env: JNIEnv,
    _class: JClass,
    data_dir: JString,
    listener: JObject,
) -> jlong {
    let dir_str: String = match env.get_string(&data_dir) {
        Ok(s) => s.into(),
        Err(_) => return 0,
    };
    // On Android, OS desktop keyrings are unavailable; Kotlin persists secrets in EncryptedSharedPreferences
    // and hydrates them into the core volatile secret store on startup.
    let runtime = match CoreRuntime::open(PathBuf::from(dir_str), true) {
        Ok(rt) => rt,
        Err(_) => return 0,
    };

    let event_thread = if !listener.is_null() {
        match (env.get_java_vm(), env.new_global_ref(listener)) {
            (Ok(vm), Ok(global_listener)) => Some(spawn_event_forwarder(vm, global_listener, runtime.clone())),
            _ => None,
        }
    } else {
        None
    };

    let boxed = Box::new(AndroidCore {
        runtime,
        event_thread,
    });
    Box::into_raw(boxed) as jlong
}

fn spawn_event_forwarder(
    vm: JavaVM,
    listener: GlobalRef,
    runtime: Arc<CoreRuntime>,
) -> std::thread::JoinHandle<()> {
    let mut rx = runtime.events.subscribe();
    std::thread::spawn(move || loop {
        match rx.blocking_recv() {
            Ok(ev) => {
                if let Ok(json_s) = serde_json::to_string(&ev) {
                    if let Ok(mut env) = vm.attach_current_thread() {
                        if let Ok(jstr) = env.new_string(&json_s) {
                            let _ = env.call_method(
                                listener.as_obj(),
                                "onCoreEvent",
                                "(Ljava/lang/String;)V",
                                &[JValue::Object(&jstr)],
                            );
                        }
                    }
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    })
}

fn empty_jstring<'local>(env: &JNIEnv<'local>) -> JString<'local> {
    env.new_string("{}").unwrap_or_else(|_| JString::default())
}

#[no_mangle]
pub extern "system" fn Java_com_localnotebook_agent_core_NativeBridge_nativeCall<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    request_json: JString<'local>,
) -> JString<'local> {
    if handle == 0 {
        return env
            .new_string(r#"{"error":{"code":"INTERNAL","message":"Core not initialized"}}"#)
            .unwrap_or_else(|_| empty_jstring(&env));
    }
    let req_str: String = match env.get_string(&request_json) {
        Ok(s) => s.into(),
        Err(_) => return empty_jstring(&env),
    };
    let core_ref = unsafe { &*(handle as *const AndroidCore) };
    let reply = core_ref.runtime.handle_json(&req_str);
    env.new_string(reply).unwrap_or_else(|_| empty_jstring(&env))
}

#[no_mangle]
pub extern "system" fn Java_com_localnotebook_agent_core_NativeBridge_nativeClose(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle == 0 {
        return;
    }
    let mut boxed = unsafe { Box::from_raw(handle as *mut AndroidCore) };
    let rt = boxed.runtime.clone();
    let ev_thread = boxed.event_thread.take();
    drop(boxed);

    let join = std::thread::spawn(move || {
        rt.shutdown();
        drop(rt);
        if let Some(h) = ev_thread {
            let _ = h.join();
        }
    });
    let deadline = std::time::Instant::now() + Duration::from_millis(1800);
    while !join.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}
