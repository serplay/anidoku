//! Android-only glue: toggles the download foreground service
//! (`DownloadForegroundService.kt`) so the OS keeps the process — and the
//! Rust download engine inside it — alive while episodes download.

use jni::objects::{JObject, JValue};

/// Start (`true`) or stop (`false`) the foreground service. Safe to call
/// redundantly; failures are logged and swallowed (a dead notification is
/// not worth crashing playback over).
pub fn set_download_service_active(active: bool) {
    let ctx = ndk_context::android_context();
    let vm = match unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) } {
        Ok(vm) => vm,
        Err(e) => {
            eprintln!("android: no JavaVM: {e}");
            return;
        }
    };
    let mut env = match vm.attach_current_thread() {
        Ok(env) => env,
        Err(e) => {
            eprintln!("android: attach_current_thread failed: {e}");
            return;
        }
    };
    let context = unsafe { JObject::from_raw(ctx.context().cast()) };

    // FindClass on a native thread only sees system classes; resolve the app
    // class through the application context's class loader instead.
    let result = (|| -> jni::errors::Result<()> {
        let loader = env
            .call_method(&context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
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
            &[JValue::Object(&context), JValue::Bool(active as u8)],
        )?;
        Ok(())
    })();

    if let Err(e) = result {
        // Clear any pending Java exception so later JNI calls aren't poisoned.
        let _ = env.exception_clear();
        eprintln!("android: DownloadForegroundService.setActive failed: {e}");
    }
}
