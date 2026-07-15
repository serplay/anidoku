// iOS background-execution shim for the download engine.
//
// The Rust download engine (reqwest/tokio, shared with desktop + Android) runs
// in-process. When AniDoku is backgrounded iOS suspends the process within
// seconds, freezing any in-flight download. This mirrors the Android
// foreground service (DownloadForegroundService.kt): while downloads are active
// we hold a `UIApplication` background task so the OS grants the process extra
// wall-clock time to keep the transfer running.
//
// HONEST LIMITATION: unlike an Android foreground service (effectively
// unbounded), a UIKit background task only buys a short grace window (iOS
// currently grants ~30s, historically up to a few minutes) before the
// expiration handler must release it or the app is killed. Truly unbounded
// background downloading on iOS requires migrating the engine to a native
// background `URLSession` (out-of-process, resumed by the system) — a large
// rewrite tracked in BUILD-IOS.md. This shim keeps foregrounded downloads and
// the brief background tail working, which covers the common case.
//
// Rust calls `anidoku_set_download_active(_:)` from src-tauri/src/ios.rs
// whenever the number of queued/downloading rows crosses zero. The `@_cdecl`
// export makes it a plain C symbol the Rust staticlib links against, the iOS
// analogue of the Android JNI bridge.

import UIKit

private var downloadBackgroundTask: UIBackgroundTaskIdentifier = .invalid

@_cdecl("anidoku_set_download_active")
func anidoku_set_download_active(_ active: Bool) {
    // UIApplication must be touched on the main thread; Rust calls this from
    // the download event loop's thread.
    DispatchQueue.main.async {
        if active {
            guard downloadBackgroundTask == .invalid else { return }
            downloadBackgroundTask = UIApplication.shared.beginBackgroundTask(
                withName: "com.anidoku.app.downloads"
            ) {
                // Expiration handler: the OS reclaimed our time. Release the
                // task so we are not force-killed. The engine will resume when
                // the app is next foregrounded (segment index is the resume
                // checkpoint), or continues if still foregrounded.
                if downloadBackgroundTask != .invalid {
                    UIApplication.shared.endBackgroundTask(downloadBackgroundTask)
                    downloadBackgroundTask = .invalid
                }
            }
        } else {
            guard downloadBackgroundTask != .invalid else { return }
            UIApplication.shared.endBackgroundTask(downloadBackgroundTask)
            downloadBackgroundTask = .invalid
        }
    }
}
