//! Android-only glue: toggles the download foreground service
//! (`DownloadForegroundService.kt`) so the OS keeps the process — and the
//! Rust download engine inside it — alive while episodes download.
//!
//! Goes through Tauri's `with_webview` → `jni_handle().exec(..)` escape
//! hatch, which runs the closure on the JVM thread with a live `JNIEnv` and
//! the Tauri activity (ndk-context is NOT initialized under Tauri v2 mobile,
//! so `jni::JavaVM::from_raw`-style glue panics).

use jni::objects::{JObject, JValue};
use tauri::Manager;

/// Start (`true`) or stop (`false`) the foreground service. Safe to call
/// redundantly; failures are logged and swallowed (a missing notification is
/// not worth crashing the event loop over). No-ops if the main window is not
/// up yet — the next state transition retries.
pub fn set_download_service_active(app: &tauri::AppHandle, active: bool) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let result = window.with_webview(move |webview| {
        webview.jni_handle().exec(move |env, activity, _webview| {
            if let Err(e) = call_set_active(env, activity, active) {
                let _ = env.exception_clear();
                eprintln!("android: DownloadForegroundService.setActive failed: {e}");
            }
        });
    });
    if let Err(e) = result {
        eprintln!("android: with_webview failed: {e}");
    }
}

fn call_set_active(
    env: &mut jni::JNIEnv,
    activity: &JObject,
    active: bool,
) -> jni::errors::Result<()> {
    // FindClass on a JNI-attached thread only sees system classes; resolve the
    // app class through the activity's class loader instead.
    let loader = env
        .call_method(activity, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
        .l()?;
    let name = env.new_string("com.anidoku.app.DownloadForegroundService")?;
    let class = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&JObject::from(name))],
        )?
        .l()?;
    env.call_static_method(
        <&jni::objects::JClass>::from(&class),
        "setActive",
        "(Landroid/content/Context;Z)V",
        &[JValue::Object(activity), JValue::Bool(active as u8)],
    )?;
    Ok(())
}
