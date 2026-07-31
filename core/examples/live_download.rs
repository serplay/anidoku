//! Live end-to-end check of the download engine against a real allanime
//! source. Network-dependent; not part of `cargo test`.
//!
//!   cargo run -p anidoku-core --example live_download -- "frieren" [episode]
//!
//! What it does:
//!   1. Resolves the show/episode via the provider and enqueues a download
//!      into a temp dir + temp SQLite db, then runs the real manager.
//!   2. Waits for a chunk of progress, pauses (interrupt), and asserts the
//!      on-disk checkpoint matches the DB checkpoint.
//!   3. Resumes, waits for more progress, and (MP4) verifies the bytes
//!      spanning the pause point against an upstream ranged fetch —
//!      resume-corruption check.
//!   4. Spawns the media server over the temp downloads root and prints the
//!      `/dl/` URL so the serving path can be probed with curl, then blocks.

use anidoku_core::db::Database;
use anidoku_core::downloads::{episode_dir_rel, DownloadEvent, DownloadManager};
use anidoku_core::media_server;
use anidoku_core::models::{DownloadState, StreamKind, TranslationType};
use anidoku_core::provider::{allanime::AllAnime, Provider};
use anidoku_core::proxy::ProxyClient;
use std::sync::Arc;
use std::time::{Duration, Instant};

const PAUSE_AT_BYTES: i64 = 1_500_000;
const RESUME_EXTRA_BYTES: i64 = 1_000_000;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let query = args.next().unwrap_or_else(|| "frieren".into());
    let want_ep = args.next();

    // Resolve show + episode.
    let provider = Arc::new(AllAnime::new());
    let results = provider
        .search(&query, TranslationType::Sub)
        .await
        .expect("search");
    // Prefer the result with the most episodes: full-length series make a
    // better interrupt/resume test than 10 MB specials.
    let show = results
        .iter()
        .max_by_key(|r| r.available_episodes)
        .expect("no search results");
    println!("show: {} ({})", show.title, show.provider_id);
    let eps = provider
        .episodes(&show.provider_id, TranslationType::Sub)
        .await
        .expect("episodes");
    let ep = want_ep.unwrap_or_else(|| eps.first().expect("no episodes").clone());
    println!("episode: {ep}");

    // Temp root + db.
    let root = std::env::temp_dir().join(format!("anidoku-live-dl-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let db = Arc::new(Database::open(&root.join("test.db")).expect("db"));
    let proxy = Arc::new(ProxyClient::new());

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                DownloadEvent::Progress(p) => println!(
                    "  [progress] bytes={}/{:?} segs={}/{:?} speed={:.0} KB/s",
                    p.bytes_done,
                    p.bytes_total,
                    p.segments_done,
                    p.segments_total,
                    p.speed_bps / 1024.0
                ),
                DownloadEvent::State(s) => println!(
                    "  [state] id={} -> {:?} err={:?} removed={}",
                    s.id, s.state, s.error, s.removed
                ),
            }
        }
    });

    let mgr = DownloadManager::new(
        db.clone(),
        proxy.clone(),
        provider.clone(),
        root.clone(),
        tx,
    );
    mgr.start();

    let id = mgr
        .enqueue(&show.provider_id, &ep, Some("best"), false)
        .expect("enqueue")
        .expect("row id");
    println!("enqueued download id={id} root={}", root.display());

    // Phase 1: wait for progress, then interrupt.
    wait_for(&db, id, Duration::from_secs(180), |r| {
        r.bytes_done >= PAUSE_AT_BYTES
            || r.state == DownloadState::Done
            || r.state == DownloadState::Failed
    })
    .await;
    let row = db.get_download(id).unwrap().unwrap();
    if row.state == DownloadState::Failed {
        panic!("download failed: {:?}", row.error);
    }
    let kind = row.kind.expect("kind resolved");
    if row.state == DownloadState::Done {
        println!("\n== episode finished before the interrupt point (small file) — skipping pause/resume phases");
    } else {
        println!(
            "\n== interrupting at bytes={} segs={} (kind={:?}, quality={:?})",
            row.bytes_done, row.segments_done, kind, row.quality
        );
        mgr.pause(id).expect("pause");
        wait_for(&db, id, Duration::from_secs(60), |r| {
            r.state == DownloadState::Paused || r.state == DownloadState::Done
        })
        .await;
    }

    let paused = db.get_download(id).unwrap().unwrap();
    let dir_rel = paused
        .dir_path
        .clone()
        .unwrap_or_else(|| episode_dir_rel(&show.provider_id, &ep));
    let dir = root.join(&dir_rel);
    println!(
        "== paused: state={:?} bytes_done={} segments_done={}",
        paused.state, paused.bytes_done, paused.segments_done
    );

    // Checkpoint integrity: disk must match DB.
    let checkpoint_bytes = paused.bytes_done;
    match kind {
        StreamKind::Mp4 => {
            let len = std::fs::metadata(dir.join("video.mp4"))
                .expect("video.mp4 exists")
                .len() as i64;
            assert_eq!(
                len, paused.bytes_done,
                "file length must equal bytes_done checkpoint"
            );
            println!("== checkpoint OK: video.mp4 length {len} == bytes_done");
        }
        StreamKind::Hls => {
            let segs = std::fs::read_dir(&dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().starts_with("seg_"))
                .count() as i64;
            assert!(
                segs >= paused.segments_done,
                "at least segments_done segment files on disk (found {segs})"
            );
            println!(
                "== checkpoint OK: {segs} segment files >= segments_done {}",
                paused.segments_done
            );
        }
    }

    // Phase 2: resume and confirm progress continues past the checkpoint.
    if paused.state == DownloadState::Paused {
        println!("\n== resuming");
        mgr.resume(id).expect("resume");
        wait_for(&db, id, Duration::from_secs(180), |r| {
            r.bytes_done >= checkpoint_bytes + RESUME_EXTRA_BYTES
                || r.state == DownloadState::Done
                || r.state == DownloadState::Failed
        })
        .await;
        let resumed = db.get_download(id).unwrap().unwrap();
        if resumed.state == DownloadState::Failed {
            panic!("resume failed: {:?}", resumed.error);
        }
        assert!(
            resumed.bytes_done > checkpoint_bytes,
            "resume advanced past the checkpoint"
        );
        println!(
            "== resume OK: bytes {} -> {}",
            checkpoint_bytes, resumed.bytes_done
        );

        // Stop again (unless already done) so the partial file is stable for
        // the integrity check + serving probe.
        if resumed.state != DownloadState::Done {
            mgr.pause(id).expect("pause2");
            wait_for(&db, id, Duration::from_secs(60), |r| {
                r.state != DownloadState::Downloading
            })
            .await;
        }
    }

    // Phase 3 (MP4): bytes spanning the pause point must match upstream.
    let final_row = db.get_download(id).unwrap().unwrap();
    if kind == StreamKind::Mp4 {
        let sources = provider
            .sources(&show.provider_id, &ep, TranslationType::Sub)
            .await
            .expect("re-resolve sources");
        let src = anidoku_core::downloads::pick_source(&sources, "best").expect("source");
        let span_start = (checkpoint_bytes - 32_768).max(0) as u64;
        let span_len: usize = 65_536;
        let resp = proxy
            .get_ranged(
                &src.url,
                src.referer.as_deref(),
                Some(&format!(
                    "bytes={}-{}",
                    span_start,
                    span_start + span_len as u64 - 1
                )),
            )
            .await
            .expect("upstream ranged fetch");
        assert_eq!(resp.status().as_u16(), 206, "upstream must honor Range");
        let upstream = resp.bytes().await.expect("upstream bytes");
        let local = std::fs::read(dir.join("video.mp4")).unwrap();
        let local_span =
            &local[span_start as usize..(span_start as usize + span_len).min(local.len())];
        assert_eq!(
            &upstream[..local_span.len()],
            local_span,
            "bytes spanning the pause point must match upstream (no resume corruption)"
        );
        println!(
            "== integrity OK: {} bytes spanning the pause point match upstream",
            local_span.len()
        );
    }

    // Phase 4: serve the downloaded tree through the media server.
    let handle = media_server::spawn(proxy.clone(), Some(root.clone()))
        .await
        .expect("media server");
    let video_file = match kind {
        StreamKind::Mp4 => "video.mp4",
        StreamKind::Hls => "index.m3u8",
    };
    println!("\nSERVE_BASE={}", handle.base);
    println!("SERVE_URL={}/dl/{}/{}", handle.base, dir_rel, video_file);
    println!(
        "STATE={:?} BYTES={} DIR={}",
        final_row.state,
        final_row.bytes_done,
        dir.display()
    );
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
            let row = db.get_download(id).ok().flatten();
            panic!("timeout waiting for condition; row={row:?}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}
