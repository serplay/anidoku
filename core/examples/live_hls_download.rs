//! Live end-to-end check of the HLS download path. allanime's reliably
//! playable sources are usually direct MP4, so this drives the real engine
//! against a public HLS test stream via a stub provider instead:
//! master-variant pick, concurrent segment batches, interrupt + segment-index
//! resume, localized playlist + manifest on completion, then /dl serving.
//!
//!   cargo run -p anidoku-core --example live_hls_download
//!
//! Network-dependent; not part of `cargo test`.

use anidoku_core::db::Database;
use anidoku_core::downloads::{DownloadEvent, DownloadManager};
use anidoku_core::media_server;
use anidoku_core::models::{AnimeSummary, DownloadState, StreamKind, TranslationType, VideoSource};
use anidoku_core::provider::Provider;
use anidoku_core::proxy::ProxyClient;
use anidoku_core::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// hls.js's canonical test stream (10-min Big Buck Bunny, multi-variant).
const HLS_URL: &str = "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8";

struct StubProvider;

#[async_trait]
impl Provider for StubProvider {
    fn name(&self) -> &'static str {
        "stub-hls"
    }
    async fn search(&self, _q: &str, _m: TranslationType) -> Result<Vec<AnimeSummary>> {
        Ok(vec![])
    }
    async fn episodes(&self, _s: &str, _m: TranslationType) -> Result<Vec<String>> {
        Ok(vec!["1".into()])
    }
    async fn sources(&self, _s: &str, _e: &str, _m: TranslationType) -> Result<Vec<VideoSource>> {
        Ok(vec![VideoSource {
            provider_name: "mux-test".into(),
            quality: "hls-multi".into(),
            url: HLS_URL.into(),
            kind: StreamKind::Hls,
            referer: None,
            subtitles: vec![],
        }])
    }
}

#[tokio::main]
async fn main() {
    let root = std::env::temp_dir().join(format!("anidoku-live-hls-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let db = Arc::new(Database::open(&root.join("test.db")).expect("db"));
    let proxy = Arc::new(ProxyClient::new());

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                DownloadEvent::Progress(p) => println!(
                    "  [progress] segs={}/{:?} bytes={} speed={:.0} KB/s",
                    p.segments_done,
                    p.segments_total,
                    p.bytes_done,
                    p.speed_bps / 1024.0
                ),
                DownloadEvent::State(s) => {
                    println!("  [state] id={} -> {:?} err={:?}", s.id, s.state, s.error)
                }
            }
        }
    });

    let mgr = DownloadManager::new(
        db.clone(),
        proxy.clone(),
        Arc::new(StubProvider),
        root.clone(),
        tx,
    );
    mgr.start();

    // Lowest variant (240-ish) keeps the full download small.
    let id = mgr
        .enqueue("hlsshow", "1", Some("240"), false)
        .expect("enqueue")
        .expect("id");
    println!("enqueued HLS download id={id} root={}", root.display());

    // Interrupt after a few segments.
    wait_for(&db, id, Duration::from_secs(120), |r| {
        r.segments_done >= 4 || r.state == DownloadState::Done || r.state == DownloadState::Failed
    })
    .await;
    let row = db.get_download(id).unwrap().unwrap();
    if row.state == DownloadState::Failed {
        panic!("download failed: {:?}", row.error);
    }
    if row.state != DownloadState::Done {
        println!(
            "\n== interrupting at segments={}/{:?}",
            row.segments_done, row.segments_total
        );
        mgr.pause(id).expect("pause");
        wait_for(&db, id, Duration::from_secs(60), |r| {
            r.state == DownloadState::Paused || r.state == DownloadState::Done
        })
        .await;
    }
    let paused = db.get_download(id).unwrap().unwrap();
    let dir_rel = paused.dir_path.clone().expect("dir recorded");
    let dir = root.join(&dir_rel);
    let seg_files = || {
        std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("seg_"))
            .count() as i64
    };
    let checkpoint = paused.segments_done;
    println!(
        "== paused: state={:?} segments_done={} seg files on disk={} quality={:?}",
        paused.state,
        checkpoint,
        seg_files(),
        paused.quality
    );
    assert!(
        seg_files() >= checkpoint,
        "checkpointed segments exist on disk"
    );

    // Resume to completion.
    if paused.state == DownloadState::Paused {
        println!("\n== resuming");
        mgr.resume(id).expect("resume");
    }
    wait_for(&db, id, Duration::from_secs(600), |r| {
        r.state == DownloadState::Done || r.state == DownloadState::Failed
    })
    .await;
    let done = db.get_download(id).unwrap().unwrap();
    if done.state == DownloadState::Failed {
        panic!("resume/complete failed: {:?}", done.error);
    }
    assert_eq!(done.segments_done, done.segments_total.unwrap());
    assert!(
        checkpoint > 0 && done.segments_done > checkpoint,
        "resume continued past the checkpoint"
    );
    println!(
        "== done: segments {}/{} bytes={} (resumed from checkpoint {})",
        done.segments_done,
        done.segments_total.unwrap(),
        done.bytes_done,
        checkpoint
    );

    // Localized playlist + manifest.
    let playlist = std::fs::read_to_string(dir.join("index.m3u8")).expect("index.m3u8 written");
    assert!(
        !playlist.contains("http"),
        "playlist fully localized (no upstream URLs)"
    );
    assert!(
        playlist.contains("seg_00000."),
        "playlist references local segments"
    );
    assert!(playlist.contains("#EXT-X-ENDLIST"), "playlist complete");
    let manifest = anidoku_core::downloads::read_manifest(&root, &dir_rel).expect("manifest.json");
    assert_eq!(manifest.video, "index.m3u8");
    println!(
        "== playlist localized OK; manifest kind={:?} quality={}",
        manifest.kind, manifest.quality
    );

    // Serve and print for curl probes.
    let handle = media_server::spawn(proxy.clone(), Some(root.clone()))
        .await
        .expect("media server");
    println!("\nSERVE_BASE={}", handle.base);
    println!("SERVE_URL={}/dl/{}/index.m3u8", handle.base, dir_rel);
    println!("DIR={}", dir.display());
    println!("(serving for curl probes; Ctrl-C to stop)");
    std::future::pending::<()>().await;
}

async fn wait_for<F: Fn(&anidoku_core::models::DownloadRow) -> bool>(
    db: &Database,
    id: i64,
    timeout: Duration,
    pred: F,
) {
    let start = Instant::now();
    loop {
        if let Ok(Some(row)) = db.get_download(id) {
            if pred(&row) {
                return;
            }
        }
        if start.elapsed() > timeout {
            panic!("timeout; row={:?}", db.get_download(id).ok().flatten());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
