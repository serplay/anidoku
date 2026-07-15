fn main() {
    // iOS: the download background-task shim (src/ios.rs) calls a Swift
    // `@_cdecl` symbol (`anidoku_set_download_active`) that lives in the app
    // target, not in this crate. The app links our *staticlib* (libapp.a),
    // where that reference resolves against the Swift object at final link.
    // But cargo also builds a *cdylib* for the same crate, and its link step
    // has no Swift to resolve against and would fail with "undefined symbol".
    // The cdylib is collateral (unused on iOS), so let its link defer symbol
    // resolution to load time instead of erroring. Only affects the cdylib.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        println!("cargo:rustc-cdylib-link-arg=-Wl,-undefined,dynamic_lookup");
    }
    tauri_build::build()
}
