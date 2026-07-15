//! iOS-only glue: extends the app's background execution window while the
//! download engine has active work, mirroring the Android foreground service.
//!
//! The heavy lifting is a Swift `@_cdecl` shim
//! (`gen/apple/Sources/anidoku/DownloadBackgroundTask.swift`) that begins/ends
//! a `UIApplication` background task. It is compiled into the app target and
//! exposed as a plain C symbol, which this Rust staticlib links against — the
//! iOS analogue of the Android JNI bridge in `android.rs`.
//!
//! Unlike the Android foreground service (effectively unbounded), a UIKit
//! background task only buys a short grace window before iOS reclaims it. See
//! BUILD-IOS.md for the honest limitations and the URLSession follow-up.

extern "C" {
    /// Provided by DownloadBackgroundTask.swift. `active` toggles the
    /// background task on/off. Safe to call redundantly; the Swift side
    /// de-dupes and marshals onto the main thread.
    fn anidoku_set_download_active(active: bool);
}

/// Start (`true`) or stop (`false`) holding a background task so the OS keeps
/// the process — and the Rust download engine inside it — alive off-screen for
/// as long as iOS allows. No-op-safe to call on every state transition.
pub fn set_download_active(active: bool) {
    // SAFETY: the symbol is a C-ABI Swift export linked into the same binary;
    // it takes a single `bool` and returns void, matching this declaration.
    unsafe { anidoku_set_download_active(active) };
}
