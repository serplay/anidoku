//! End-to-end check of downloading a *demuxed* HLS stream (audio in its own
//! rendition, as AniZone serves it) through the real download engine, against
//! a local server — no network.
//!
//! The regression this guards: taking only the picked video variant yields a
//! complete-looking download with no sound.

use anidoku_core::db::Database;
use anidoku_core::downloads::{read_manifest, DownloadManager};
use anidoku_core::models::{
    AnimeSummary, DownloadState, StreamKind, SubtitleTrack, TranslationType, VideoSource,
};
use anidoku_core::provider::{Capabilities, Provider, Registry};
use anidoku_core::proxy::ProxyClient;
use anidoku_core::Result;
use async_trait::async_trait;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::Router;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MASTER: &str = "#EXTM3U\n\
#EXT-X-VERSION:3\n\
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"group_audio\",NAME=\"English (US)\",DEFAULT=NO,LANGUAGE=\"en\",URI=\"audio/en/playlist.m3u8\"\n\
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"group_audio\",NAME=\"Japanese\",DEFAULT=YES,LANGUAGE=\"ja\",URI=\"audio/ja/playlist.m3u8\"\n\
#EXT-X-STREAM-INF:BANDWIDTH=946000,RESOLUTION=640x360,AUDIO=\"group_audio\"\n\
video/360/playlist.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=3476000,RESOLUTION=1920x1080,AUDIO=\"group_audio\"\n\
video/1080/playlist.m3u8\n";

fn media(segments: usize) -> String {
    let mut out = String::from("#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:6\n");
    for i in 0..segments {
        out.push_str(&format!("#EXTINF:6.0,\n{i}.ts\n"));
    }
    out.push_str("#EXT-X-ENDLIST\n");
    out
}

const ASS: &str = "[Events]\nFormat: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text\nDialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,Hello\n";

/// A fake CDN: master, per-variant and per-language playlists, and segments
/// whose bytes name the track they belong to.
async fn fake_cdn() -> String {
    let app = Router::new().fallback(|req: Request| async move {
        let path = req.uri().path().to_string();
        let body: Option<String> = match path.as_str() {
            "/ep/master.m3u8" => Some(MASTER.into()),
            "/ep/video/360/playlist.m3u8" => Some(media(5)),
            "/ep/video/1080/playlist.m3u8" => Some(media(5)),
            "/ep/audio/ja/playlist.m3u8" => Some(media(3)),
            "/ep/audio/en/playlist.m3u8" => Some(media(3)),
            "/ep/subs/en.ass" => Some(ASS.into()),
            p if p.ends_with(".ts") => Some(format!("bytes-of:{p}")),
            _ => None,
        };
        match body {
            Some(b) => (StatusCode::OK, b),
            None => (StatusCode::NOT_FOUND, String::new()),
        }
    });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    base
}

struct Demuxed {
    base: String,
}

#[async_trait]
impl Provider for Demuxed {
    fn id(&self) -> &'static str {
        "demuxed"
    }
    fn display_name(&self) -> &'static str {
        "Demuxed"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            dub: true,
            carries_anilist_id: false,
            subtitles: true,
        }
    }
    async fn search(&self, _q: &str, _m: TranslationType) -> Result<Vec<AnimeSummary>> {
        Ok(vec![])
    }
    async fn episodes(&self, _s: &str, _m: TranslationType) -> Result<Vec<String>> {
        Ok(vec!["1".into()])
    }
    async fn sources(&self, _s: &str, _e: &str, _m: TranslationType) -> Result<Vec<VideoSource>> {
        Ok(vec![VideoSource {
            source: String::new(),
            provider_name: "fake".into(),
            quality: "auto".into(),
            url: format!("{}/ep/master.m3u8", self.base),
            kind: StreamKind::Hls,
            referer: None,
            subtitles: vec![SubtitleTrack {
                label: "English".into(),
                lang: "en".into(),
                url: format!("{}/ep/subs/en.ass", self.base),
                default: true,
            }],
        }])
    }
}

/// Download episode 1 and return (downloads root, episode dir relative path).
async fn download(dub: bool) -> (std::path::PathBuf, String) {
    let base = fake_cdn().await;
    let root = std::env::temp_dir().join(format!(
        "anidoku-demuxed-{}-{}",
        std::process::id(),
        if dub { "dub" } else { "sub" }
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db = Arc::new(Database::open(&root.join("test.db")).unwrap());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let mgr = DownloadManager::new(
        db.clone(),
        Arc::new(ProxyClient::new()),
        Arc::new(Registry::new(vec![Arc::new(Demuxed { base })])),
        root.clone(),
        tx,
    );
    mgr.start();
    let id = mgr
        .enqueue("demuxed:show", "1", Some("1080"), dub)
        .unwrap()
        .unwrap();

    let start = Instant::now();
    let row = loop {
        let row = db.get_download(id).unwrap().unwrap();
        if matches!(row.state, DownloadState::Done | DownloadState::Failed) {
            break row;
        }
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "timeout: {row:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(row.state, DownloadState::Done, "error: {:?}", row.error);
    // Video and audio segments share one checkpoint: 5 + 3.
    assert_eq!(row.segments_total, Some(8));
    assert_eq!(row.segments_done, 8);
    (root, row.dir_path.unwrap())
}

#[tokio::test]
async fn a_demuxed_stream_downloads_with_its_audio() {
    let (root, dir_rel) = download(false).await;
    let dir = root.join(&dir_rel);
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).unwrap();

    // The entry point is a local master tying the two playlists together.
    let index = read("index.m3u8");
    assert!(index.contains("TYPE=AUDIO") && index.contains("URI=\"audio.m3u8\""));
    assert!(index.contains("NAME=\"Japanese\""), "{index}");
    assert!(index.contains("\nvideo.m3u8\n"), "{index}");

    // Both media playlists are fully localized and complete.
    let video = read("video.m3u8");
    let audio = read("audio.m3u8");
    for playlist in [&index, &video, &audio] {
        assert!(
            !playlist.contains("http"),
            "upstream URL left in: {playlist}"
        );
    }
    assert!(video.contains("seg_00004.ts") && video.contains("#EXT-X-ENDLIST"));
    assert!(audio.contains("aud_00002.ts") && audio.contains("#EXT-X-ENDLIST"));

    // The right bytes landed in the right files: the picked 1080 variant, and
    // the stream's default (Japanese) audio for a sub download.
    assert_eq!(read("seg_00000.ts"), "bytes-of:/ep/video/1080/0.ts");
    assert_eq!(read("aud_00000.ts"), "bytes-of:/ep/audio/ja/0.ts");

    // The ASS subtitle was converted and kept its default flag.
    let manifest = read_manifest(&root, &dir_rel).unwrap();
    assert_eq!(manifest.video, "index.m3u8");
    assert_eq!(manifest.quality, "1080");
    assert_eq!(manifest.subtitles.len(), 1);
    assert!(manifest.subtitles[0].default);
    let vtt = read(&manifest.subtitles[0].file);
    assert!(vtt.starts_with("WEBVTT") && vtt.contains("Hello"), "{vtt}");

    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn a_dub_download_takes_the_english_audio() {
    let (root, dir_rel) = download(true).await;
    let dir = root.join(&dir_rel);
    assert_eq!(
        std::fs::read_to_string(dir.join("aud_00000.ts")).unwrap(),
        "bytes-of:/ep/audio/en/0.ts"
    );
    let index = std::fs::read_to_string(dir.join("index.m3u8")).unwrap();
    assert!(index.contains("LANGUAGE=\"en\""), "{index}");
    let _ = std::fs::remove_dir_all(&root);
}
