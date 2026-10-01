//! Offline download engine.
//!
//! A `DownloadManager` runs a scheduler over the persistent `downloads` table
//! (SQLite): rows move queued → downloading → done, with paused/failed
//! transitions and resume checkpoints (`bytes_done` for MP4 byte-offset
//! resume, `segments_done` for HLS segment-index resume). Cancel deletes the
//! row and its files. In-flight rows revert to `queued` at startup so a killed
//! app resumes where it stopped.
//!
//! Storage layout, under the app-data downloads root:
//!   <anime_id>/<episode>/video.mp4              (MP4)
//!   <anime_id>/<episode>/index.m3u8 + seg_*.ts  (HLS, playlist localized)
//!   <anime_id>/<episode>/sub_XX_<lang>.vtt      (subtitles, converted to VTT)
//!   <anime_id>/<episode>/manifest.json          (written on completion)

pub mod hls;

use crate::db::Database;
use crate::models::{DownloadRow, DownloadState, StreamKind, SubtitleTrack, VideoSource};
use crate::provider::{aggregate, playability_rank, Registry};
use crate::proxy::ProxyClient;
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Notify};

/// Global concurrency limit: how many episodes download at once.
pub const MAX_ACTIVE_DOWNLOADS: usize = 2;
/// How many HLS segments are fetched concurrently within one job.
pub const SEGMENT_CONCURRENCY: usize = 4;
/// Minimum interval between progress events/persists (~2 per second).
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(500);
/// Per-request timeout for segment fetches / MP4 chunk reads.
const FETCH_TIMEOUT: Duration = Duration::from_secs(45);

// Control-flag codes (checked by running jobs between chunks/segments).
const CTL_RUN: u8 = 0;
const CTL_PAUSE: u8 = 1;
const CTL_CANCEL: u8 = 2;

/// How many times a transient network failure is retried before the job fails.
const MAX_RETRIES: u32 = 4;
/// First backoff delay; doubles each attempt up to `BACKOFF_CAP`.
const BACKOFF_BASE: Duration = Duration::from_millis(500);
/// Upper bound on any single backoff sleep.
const BACKOFF_CAP: Duration = Duration::from_secs(8);

/// Is `err` a transient network fault a retry might recover from? A flaky CDN
/// resets mid-body, stalls, or returns a 5xx/429; those are worth retrying. A
/// 4xx (other than 429), an IO/DB/decrypt error, or a missing source is not.
fn is_transient(err: &Error) -> bool {
    match err {
        Error::Network(e) => {
            // A reset/incomplete body arrives as is_body()/is_decode(); a dropped
            // or refused connection as is_connect()/is_request().
            if e.is_timeout() || e.is_connect() || e.is_request() || e.is_body() || e.is_decode() {
                return true;
            }
            matches!(e.status().map(|s| s.as_u16()), Some(s) if s >= 500 || s == 429)
        }
        // The stall / early-end / timeout family we raise ourselves (see
        // `run_mp4` / `fetch_with_timeout`), plus the retryable HTTP statuses
        // `run_mp4` folds into its "upstream returned HTTP {status}" message.
        Error::Download(msg) => {
            msg.contains("stalled")
                || msg.contains("connection ended early")
                || msg.contains("timed out")
                || msg.contains("upstream returned HTTP 5")
                || msg.contains("upstream returned HTTP 429")
        }
        _ => false,
    }
}

/// Un-jittered exponential backoff: `BACKOFF_BASE * 2^attempt`, capped at
/// `BACKOFF_CAP`. Pure and monotonic non-decreasing (the sequence the tests
/// assert against).
fn backoff_delay(attempt: u32) -> Duration {
    let mult = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
    BACKOFF_BASE
        .checked_mul(mult)
        .unwrap_or(BACKOFF_CAP)
        .min(BACKOFF_CAP)
}

/// Sleep up to `dur`, but return the instant the control flag leaves `CTL_RUN`
/// (a pause/cancel request) so backoff never delays a cancel — the caller then
/// re-runs its operation, which observes the flag and returns promptly.
async fn interruptible_sleep(dur: Duration, flag: &AtomicU8) {
    let deadline = Instant::now() + dur;
    loop {
        let now = Instant::now();
        if now >= deadline || flag.load(Ordering::SeqCst) != CTL_RUN {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50).min(deadline - now)).await;
    }
}

/// Run `op`, retrying transient network failures with bounded exponential
/// backoff. Successes (including the pause/cancel outcomes an op returns as
/// `Ok`), non-transient errors, and exhausted attempts return immediately. The
/// backoff is cancel/pause-aware via `interruptible_sleep`.
async fn retry_transient<T, F, Fut>(flag: &AtomicU8, mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut attempt = 0u32;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                if attempt >= MAX_RETRIES || !is_transient(&e) {
                    return Err(e);
                }
                interruptible_sleep(backoff_delay(attempt), flag).await;
                attempt += 1;
            }
        }
    }
}

/// Is `from` → `to` a legal state transition?
///
///   queued      → downloading (claim), paused (user pause before start)
///   downloading → done, failed, paused, queued (startup recovery)
///   paused      → queued (resume)
///   failed      → queued (retry)
///   done        → terminal (delete removes the row)
pub fn can_transition(from: DownloadState, to: DownloadState) -> bool {
    use DownloadState::*;
    matches!(
        (from, to),
        (Queued, Downloading)
            | (Queued, Paused)
            | (Downloading, Done)
            | (Downloading, Failed)
            | (Downloading, Paused)
            | (Downloading, Queued)
            | (Paused, Queued)
            | (Failed, Queued)
    )
}

/// Parse the leading number of a quality label ("1080", "1080p" → 1080).
pub fn quality_num(q: &str) -> Option<i64> {
    let digits: String = q
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Choose the source to download for a desired quality preference ("best" or
/// a number like "720"). Embed pages (playability rank 2) are not downloadable
/// and are excluded; direct media (rank 0) beats unknown (rank 1). An HLS
/// master with a non-numeric quality can serve any preference via its
/// variants, so it scores as a near-exact match.
pub fn pick_source<'a>(sources: &'a [VideoSource], desired: &str) -> Option<&'a VideoSource> {
    let want = quality_num(desired);
    sources
        .iter()
        .filter(|s| playability_rank(s) < 2)
        .min_by_key(|s| {
            let rank = playability_rank(s);
            let num = quality_num(&s.quality);
            let dist: i64 = match (want, num) {
                // Numeric preference with a numeric label: distance.
                (Some(w), Some(n)) => (n - w).abs(),
                // Numeric preference, unlabeled: HLS can adapt (variants),
                // an unlabeled MP4 is a gamble.
                (Some(_), None) if s.kind == StreamKind::Hls => 1,
                (Some(_), None) => 400,
                // "best": higher resolution is better; unlabeled HLS sits
                // between 720 and 1080.
                (None, Some(n)) => 10_000 - n,
                (None, None) if s.kind == StreamKind::Hls => 10_000 - 900,
                (None, None) => 10_000 - 400,
            };
            (rank, dist)
        })
}

/// Sanitize an id/episode string into a filesystem-safe path component.
pub fn sanitize_component(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    // Avoid empty names and dot-only names like "..".
    if out.is_empty() || out.chars().all(|c| c == '.') {
        "_".to_string()
    } else {
        out
    }
}

/// Episode directory relative to the downloads root.
pub fn episode_dir_rel(anime_id: &str, episode: &str) -> String {
    format!(
        "{}/{}",
        sanitize_component(anime_id),
        sanitize_component(episode)
    )
}

/// Written to `<episode dir>/manifest.json` on completion; the offline
/// playback path reads it to build the player URL + subtitle tracks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub kind: StreamKind,
    pub quality: String,
    /// Video entry point, relative to the episode dir ("video.mp4" or
    /// "index.m3u8").
    pub video: String,
    pub subtitles: Vec<ManifestSub>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestSub {
    pub label: String,
    pub lang: String,
    pub file: String,
    /// Mirrors [`SubtitleTrack::default`]; absent in manifests written before
    /// it existed.
    #[serde(default)]
    pub default: bool,
}

/// Read the completion manifest for an episode dir (relative to `root`).
pub fn read_manifest(root: &Path, dir_rel: &str) -> Option<Manifest> {
    let text = std::fs::read_to_string(root.join(dir_rel).join("manifest.json")).ok()?;
    serde_json::from_str(&text).ok()
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct ProgressPayload {
    pub id: i64,
    pub anime_id: String,
    pub episode_number: String,
    pub bytes_done: i64,
    pub bytes_total: Option<i64>,
    pub segments_done: i64,
    pub segments_total: Option<i64>,
    /// Bytes per second over the last progress interval.
    pub speed_bps: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct StatePayload {
    pub id: i64,
    pub anime_id: String,
    pub episode_number: String,
    pub state: DownloadState,
    pub error: Option<String>,
    /// True when the row was removed entirely (cancel / delete).
    pub removed: bool,
}

#[derive(Debug, Clone)]
pub enum DownloadEvent {
    Progress(ProgressPayload),
    State(StatePayload),
}

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

pub struct DownloadManager {
    db: Arc<Database>,
    proxy: Arc<ProxyClient>,
    sources: Arc<Registry>,
    root: PathBuf,
    events: mpsc::UnboundedSender<DownloadEvent>,
    /// Control flags for active jobs (pause/cancel requests).
    controls: Mutex<HashMap<i64, Arc<AtomicU8>>>,
    wake: Notify,
}

enum JobOutcome {
    Done,
    Paused,
    Canceled,
}

impl DownloadManager {
    pub fn new(
        db: Arc<Database>,
        proxy: Arc<ProxyClient>,
        sources: Arc<Registry>,
        root: PathBuf,
        events: mpsc::UnboundedSender<DownloadEvent>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            proxy,
            sources,
            root,
            events,
            controls: Mutex::new(HashMap::new()),
            wake: Notify::new(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Recover in-flight rows and start the scheduler loop. Must be called
    /// from within a Tokio runtime.
    pub fn start(self: &Arc<Self>) {
        let _ = self.db.recover_in_flight_downloads();
        let mgr = self.clone();
        tokio::spawn(async move { mgr.scheduler().await });
    }

    /// Enqueue one episode. Returns the row id, or None when the episode is
    /// already queued/downloading/paused/done.
    pub fn enqueue(
        &self,
        anime_id: &str,
        episode: &str,
        quality: Option<&str>,
        dub: bool,
    ) -> Result<Option<i64>> {
        let id = self.db.enqueue_download(anime_id, episode, quality, dub)?;
        if let Some(id) = id {
            if let Ok(Some(row)) = self.db.get_download(id) {
                self.emit_state(&row, None, false);
            }
            self.wake.notify_one();
        }
        Ok(id)
    }

    /// Pause a job: flags an active job (it checkpoints and stops), or moves a
    /// queued row directly to paused.
    pub fn pause(&self, id: i64) -> Result<()> {
        if let Some(flag) = self.controls.lock().unwrap().get(&id) {
            flag.store(CTL_PAUSE, Ordering::SeqCst);
            return Ok(());
        }
        let row = self.row(id)?;
        if !can_transition(row.state, DownloadState::Paused) {
            return Err(Error::Download(format!(
                "cannot pause a {} download",
                row.state.as_str()
            )));
        }
        self.db
            .set_download_state(id, DownloadState::Paused, None)?;
        self.emit_row_state(id, false);
        Ok(())
    }

    /// Resume a paused (or retry a failed) job by re-queueing it.
    pub fn resume(&self, id: i64) -> Result<()> {
        let row = self.row(id)?;
        if !can_transition(row.state, DownloadState::Queued) {
            return Err(Error::Download(format!(
                "cannot resume a {} download",
                row.state.as_str()
            )));
        }
        self.db
            .set_download_state(id, DownloadState::Queued, None)?;
        self.emit_row_state(id, false);
        self.wake.notify_one();
        Ok(())
    }

    /// Cancel/delete a download: an active job is flagged (its files + row are
    /// removed once it stops); any other row is removed immediately.
    pub fn remove(&self, id: i64) -> Result<()> {
        if let Some(flag) = self.controls.lock().unwrap().get(&id) {
            flag.store(CTL_CANCEL, Ordering::SeqCst);
            return Ok(());
        }
        let row = self.row(id)?;
        self.delete_row_and_files(&row);
        Ok(())
    }

    /// Delete every completed download of one show.
    pub fn remove_anime_completed(&self, anime_id: &str) -> Result<usize> {
        let rows = self.db.downloads_for_anime(anime_id)?;
        let mut n = 0;
        for row in rows.iter().filter(|r| r.state == DownloadState::Done) {
            self.delete_row_and_files(row);
            n += 1;
        }
        Ok(n)
    }

    /// Delete all completed downloads (bulk cleanup).
    pub fn remove_all_completed(&self) -> Result<usize> {
        let rows = self.db.list_downloads()?;
        let mut n = 0;
        for row in rows.iter().filter(|r| r.state == DownloadState::Done) {
            self.delete_row_and_files(row);
            n += 1;
        }
        Ok(n)
    }

    // -- internals ----------------------------------------------------------

    fn row(&self, id: i64) -> Result<DownloadRow> {
        self.db
            .get_download(id)?
            .ok_or_else(|| Error::Download(format!("download {id} not found")))
    }

    fn delete_row_and_files(&self, row: &DownloadRow) {
        let dir_rel = row
            .dir_path
            .clone()
            .unwrap_or_else(|| episode_dir_rel(&row.anime_id, &row.episode_number));
        let dir = self.root.join(&dir_rel);
        let _ = std::fs::remove_dir_all(&dir);
        // Prune the parent (anime) dir when empty.
        if let Some(parent) = dir.parent() {
            if parent != self.root {
                let _ = std::fs::remove_dir(parent);
            }
        }
        let _ = self.db.delete_download_row(row.id);
        self.emit(DownloadEvent::State(StatePayload {
            id: row.id,
            anime_id: row.anime_id.clone(),
            episode_number: row.episode_number.clone(),
            state: row.state,
            error: None,
            removed: true,
        }));
    }

    fn emit(&self, ev: DownloadEvent) {
        let _ = self.events.send(ev);
    }

    fn emit_state(&self, row: &DownloadRow, error: Option<String>, removed: bool) {
        self.emit(DownloadEvent::State(StatePayload {
            id: row.id,
            anime_id: row.anime_id.clone(),
            episode_number: row.episode_number.clone(),
            state: row.state,
            error,
            removed,
        }));
    }

    fn emit_row_state(&self, id: i64, removed: bool) {
        if let Ok(Some(row)) = self.db.get_download(id) {
            let err = row.error.clone();
            self.emit_state(&row, err, removed);
        }
    }

    async fn scheduler(self: Arc<Self>) {
        loop {
            loop {
                let active = self.db.count_downloading().unwrap_or(0) as usize;
                if active >= MAX_ACTIVE_DOWNLOADS {
                    break;
                }
                let Ok(Some(row)) = self.db.claim_next_queued() else {
                    break;
                };
                let flag = Arc::new(AtomicU8::new(CTL_RUN));
                self.controls.lock().unwrap().insert(row.id, flag.clone());
                self.emit_state(&row, None, false);
                let mgr = self.clone();
                tokio::spawn(async move {
                    mgr.run_job(row, flag).await;
                    mgr.wake.notify_one();
                });
            }
            // Wake on enqueue/resume/job-finish; the periodic tick is a safety
            // net against a lost notify.
            tokio::select! {
                _ = self.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(15)) => {}
            }
        }
    }

    async fn run_job(&self, row: DownloadRow, flag: Arc<AtomicU8>) {
        let outcome = self.execute(&row, &flag).await;
        self.controls.lock().unwrap().remove(&row.id);
        match outcome {
            Ok(JobOutcome::Done) => {
                let _ = self
                    .db
                    .set_download_state(row.id, DownloadState::Done, None);
                self.emit_row_state(row.id, false);
            }
            Ok(JobOutcome::Paused) => {
                let _ = self
                    .db
                    .set_download_state(row.id, DownloadState::Paused, None);
                self.emit_row_state(row.id, false);
            }
            Ok(JobOutcome::Canceled) => {
                if let Ok(Some(row)) = self.db.get_download(row.id) {
                    self.delete_row_and_files(&row);
                }
            }
            Err(e) => {
                let _ =
                    self.db
                        .set_download_state(row.id, DownloadState::Failed, Some(&e.to_string()));
                self.emit_row_state(row.id, false);
            }
        }
    }

    async fn execute(&self, row: &DownloadRow, flag: &AtomicU8) -> Result<JobOutcome> {
        use crate::models::TranslationType;
        let mode = if row.dub {
            TranslationType::Dub
        } else {
            TranslationType::Sub
        };
        let desired = row.quality.as_deref().unwrap_or("best");

        // Source URLs expire, so every (re)start re-resolves. Going through
        // the dispatch layer means a job whose source rotated mid-queue can
        // still finish from another source mapped to the same show.
        let sources = aggregate::sources_for(
            &self.sources,
            &self.db,
            &row.anime_id,
            &row.episode_number,
            mode,
        )
        .await?;
        let source = pick_source(&sources, desired)
            .ok_or_else(|| Error::Download("no downloadable source found".into()))?
            .clone();

        let dir_rel = episode_dir_rel(&row.anime_id, &row.episode_number);
        let dir = self.root.join(&dir_rel);
        tokio::fs::create_dir_all(&dir).await?;
        self.db
            .set_download_meta(row.id, &dir_rel, source.kind, &source.quality)?;

        // Subtitles first: cheap, idempotent, and present even if the video
        // pauses midway.
        let subs = self
            .download_subtitles(&dir, &source.subtitles, source.referer.as_deref())
            .await;

        let mut tracker = Tracker::new(self, row);
        let (outcome, video_file, quality_label) = match source.kind {
            StreamKind::Mp4 => {
                let out = self.run_mp4(&dir, &source, row, flag, &mut tracker).await?;
                (out, "video.mp4".to_string(), source.quality.clone())
            }
            StreamKind::Hls => {
                let (out, label) = self
                    .run_hls(&dir, &dir_rel, &source, row, desired, flag, &mut tracker)
                    .await?;
                (out, "index.m3u8".to_string(), label)
            }
        };

        if matches!(outcome, JobOutcome::Done) {
            let manifest = Manifest {
                kind: source.kind,
                quality: quality_label,
                video: video_file,
                subtitles: subs,
            };
            tokio::fs::write(
                dir.join("manifest.json"),
                serde_json::to_vec_pretty(&manifest).unwrap_or_default(),
            )
            .await?;
            tracker.finish();
        }
        Ok(outcome)
    }

    /// Ranged MP4 download with byte-offset resume. The file length on disk is
    /// the source of truth for the checkpoint, so each retry is naturally a
    /// resume: a transient failure re-reads the offset and re-issues the ranged
    /// GET, continuing the same file.
    async fn run_mp4(
        &self,
        dir: &Path,
        source: &VideoSource,
        _row: &DownloadRow,
        flag: &AtomicU8,
        tracker: &mut Tracker<'_>,
    ) -> Result<JobOutcome> {
        let path = dir.join("video.mp4");
        // Inline (rather than via `retry_transient`) because the attempt borrows
        // `tracker` mutably, which a reusable `FnMut`-based wrapper can't express.
        let mut attempt = 0u32;
        loop {
            match self.mp4_attempt(&path, source, flag, tracker).await {
                Ok(out) => return Ok(out),
                Err(e) => {
                    if attempt >= MAX_RETRIES || !is_transient(&e) {
                        return Err(e);
                    }
                    interruptible_sleep(backoff_delay(attempt), flag).await;
                    attempt += 1;
                }
            }
        }
    }

    /// One MP4 (re)attempt: read the on-disk offset, issue the ranged GET, and
    /// stream to the file. Returns `Err` (possibly transient) so `run_mp4` can
    /// retry from the new offset.
    async fn mp4_attempt(
        &self,
        path: &Path,
        source: &VideoSource,
        flag: &AtomicU8,
        tracker: &mut Tracker<'_>,
    ) -> Result<JobOutcome> {
        // A cancel/pause requested during backoff short-circuits before we spend
        // a request on the retry.
        match flag.load(Ordering::SeqCst) {
            CTL_PAUSE => return Ok(JobOutcome::Paused),
            CTL_CANCEL => return Ok(JobOutcome::Canceled),
            _ => {}
        }
        let mut offset: i64 = match tokio::fs::metadata(&path).await {
            Ok(m) => m.len() as i64,
            Err(_) => 0,
        };

        let range = if offset > 0 {
            Some(format!("bytes={offset}-"))
        } else {
            None
        };
        let resp = self
            .proxy
            .get_ranged(&source.url, source.referer.as_deref(), range.as_deref())
            .await?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(Error::Download(format!("upstream returned HTTP {status}")));
        }

        let mut total: Option<i64> = None;
        if status == 206 {
            // Content-Range: bytes a-b/total
            if let Some(cr) = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
            {
                total = cr.rsplit('/').next().and_then(|t| t.parse().ok());
            }
        } else {
            // Server ignored the Range (or fresh start): restart from zero.
            if offset > 0 {
                offset = 0;
            }
            total = resp.content_length().map(|l| l as i64);
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            // Never truncate: a resumed download reuses the bytes already on
            // disk (we set_len + seek to `offset` below).
            .truncate(false)
            .write(true)
            .open(&path)
            .await?;
        file.set_len(offset as u64).await?;
        use tokio::io::AsyncSeekExt;
        file.seek(std::io::SeekFrom::Start(offset as u64)).await?;

        tracker.set(offset, total, 0, None);
        let mut resp = resp;
        loop {
            match flag.load(Ordering::SeqCst) {
                CTL_PAUSE => {
                    file.flush().await?;
                    tracker.persist_now();
                    return Ok(JobOutcome::Paused);
                }
                CTL_CANCEL => return Ok(JobOutcome::Canceled),
                _ => {}
            }
            let chunk = tokio::time::timeout(FETCH_TIMEOUT, resp.chunk())
                .await
                .map_err(|_| Error::Download("download stalled (timeout)".into()))??;
            let Some(bytes) = chunk else { break };
            file.write_all(&bytes).await?;
            offset += bytes.len() as i64;
            tracker.set(offset, total, 0, None);
        }
        file.flush().await?;
        tracker.persist_now();

        if let Some(t) = total {
            if offset < t {
                return Err(Error::Download(format!(
                    "connection ended early ({offset}/{t} bytes) — resume to continue"
                )));
            }
        }
        Ok(JobOutcome::Done)
    }

    /// HLS download: resolve the variant, fetch segments concurrently in
    /// batches, and write a localized playlist on completion. The number of
    /// contiguous completed segments is the resume checkpoint.
    #[allow(clippy::too_many_arguments)]
    async fn run_hls(
        &self,
        dir: &Path,
        dir_rel: &str,
        source: &VideoSource,
        row: &DownloadRow,
        desired: &str,
        flag: &AtomicU8,
        tracker: &mut Tracker<'_>,
    ) -> Result<(JobOutcome, String)> {
        let referer = source.referer.as_deref();
        let fetched = self.proxy.fetch(&source.url, referer).await?;
        let mut playlist = String::from_utf8_lossy(&fetched.bytes).into_owned();
        let mut base_url = source.url.clone();
        let mut quality_label = source.quality.clone();

        // A demuxed master keeps audio in a separate rendition; downloading
        // only the variant would produce a silent file, so the matching audio
        // playlist is fetched alongside it.
        let mut demuxed: Option<(hls::Variant, hls::AudioRendition, String)> = None;
        let variants = hls::parse_variants(&playlist, &base_url);
        if !variants.is_empty() {
            let v = hls::pick_variant(&variants, desired)
                .ok_or_else(|| Error::Download("empty master playlist".into()))?
                .clone();
            if v.height > 0 {
                quality_label = v.height.to_string();
            }
            if let Some(group) = &v.audio_group {
                let renditions = hls::parse_audio_renditions(&playlist, &base_url);
                let language = row.dub.then_some("en");
                if let Some(audio) = hls::pick_audio(&renditions, group, language) {
                    let af = self.proxy.fetch(&audio.url, referer).await?;
                    let text = String::from_utf8_lossy(&af.bytes).into_owned();
                    demuxed = Some((v.clone(), audio.clone(), text));
                }
            }
            base_url = v.url.clone();
            let vf = self.proxy.fetch(&base_url, referer).await?;
            playlist = String::from_utf8_lossy(&vf.bytes).into_owned();
        }
        self.db
            .set_download_meta(row.id, dir_rel, StreamKind::Hls, &quality_label)?;

        // One track for a muxed stream, two (video + audio) for a demuxed
        // one. Their segments are downloaded as a single ordered list so the
        // existing contiguous-count checkpoint covers both.
        let mut tracks = vec![HlsTrack::new(playlist, base_url, "seg", "key")];
        if let Some((_, audio, text)) = &demuxed {
            tracks.push(HlsTrack::new(
                text.clone(),
                audio.url.clone(),
                "aud",
                "akey",
            ));
        }
        if tracks.iter().any(|t| t.segments.is_empty()) {
            return Err(Error::Download("HLS playlist has no segments".into()));
        }
        // (track index, segment index) in download order.
        let order: Vec<(usize, usize)> = tracks
            .iter()
            .enumerate()
            .flat_map(|(t, track)| (0..track.segments.len()).map(move |i| (t, i)))
            .collect();
        let total = order.len() as i64;

        // Resume checkpoint: valid only if the segment count still matches
        // (a re-resolve can land on a different variant/host).
        let mut done = row.segments_done;
        if row.segments_total != Some(total) || done > total {
            done = 0;
        }
        let mut bytes_done = if done > 0 { row.bytes_done } else { 0 };

        // Keys/init sections are small: always (re)download them.
        for track in &tracks {
            for url in &track.keys {
                let data = self.fetch_with_retry(url, referer, flag).await?;
                tokio::fs::write(dir.join(&track.map[url]), &data).await?;
            }
        }

        tracker.set(bytes_done, None, done, Some(total));
        while done < total {
            match flag.load(Ordering::SeqCst) {
                CTL_PAUSE => {
                    tracker.persist_now();
                    return Ok((JobOutcome::Paused, quality_label));
                }
                CTL_CANCEL => return Ok((JobOutcome::Canceled, quality_label)),
                _ => {}
            }
            let end = (done as usize + SEGMENT_CONCURRENCY).min(total as usize);
            let fetches = order[done as usize..end].iter().map(|&(t, i)| {
                let url = tracks[t].segments[i].clone();
                async move {
                    let data = self.fetch_with_retry(&url, referer, flag).await?;
                    Ok::<(usize, usize, Vec<u8>), Error>((t, i, data))
                }
            });
            let results = futures_util::future::join_all(fetches).await;
            for res in results {
                let (t, i, data) = res?;
                bytes_done += data.len() as i64;
                let track = &tracks[t];
                tokio::fs::write(dir.join(&track.map[&track.segments[i]]), &data).await?;
            }
            done = end as i64;
            tracker.set(bytes_done, None, done, Some(total));
        }

        // All segments on disk: write the localized playlist(s). The entry
        // point is always index.m3u8 — the media playlist itself when muxed,
        // a small master pointing at video.m3u8 + audio.m3u8 when demuxed.
        match &demuxed {
            None => {
                tokio::fs::write(dir.join("index.m3u8"), tracks[0].localized()).await?;
            }
            Some((variant, audio, _)) => {
                tokio::fs::write(dir.join("video.m3u8"), tracks[0].localized()).await?;
                tokio::fs::write(dir.join("audio.m3u8"), tracks[1].localized()).await?;
                let master = hls::local_master(variant, audio, "video.m3u8", "audio.m3u8");
                tokio::fs::write(dir.join("index.m3u8"), master).await?;
            }
        }
        tracker.set(bytes_done, Some(bytes_done), total, Some(total));
        tracker.persist_now();
        Ok((JobOutcome::Done, quality_label))
    }

    /// `fetch_with_timeout` with bounded backoff retry on transient failures, so
    /// one flaky segment/key/init fetch self-heals instead of failing the job.
    async fn fetch_with_retry(
        &self,
        url: &str,
        referer: Option<&str>,
        flag: &AtomicU8,
    ) -> Result<Vec<u8>> {
        retry_transient(flag, || self.fetch_with_timeout(url, referer)).await
    }

    async fn fetch_with_timeout(&self, url: &str, referer: Option<&str>) -> Result<Vec<u8>> {
        let fetched = tokio::time::timeout(FETCH_TIMEOUT, self.proxy.fetch(url, referer))
            .await
            .map_err(|_| Error::Download(format!("fetch timed out: {url}")))??;
        Ok(fetched.bytes)
    }

    /// Download the source's subtitle tracks into the episode dir as VTT.
    /// Best-effort: unparseable/unfetchable tracks are skipped.
    async fn download_subtitles(
        &self,
        dir: &Path,
        subs: &[SubtitleTrack],
        referer: Option<&str>,
    ) -> Vec<ManifestSub> {
        let mut out = Vec::new();
        for (i, sub) in subs.iter().enumerate() {
            let Ok(bytes) = self.fetch_with_timeout(&sub.url, referer).await else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            let vtt = if text.trim_start().starts_with("WEBVTT") {
                text.into_owned()
            } else {
                let ext = crate::subs::url_extension(&sub.url).unwrap_or_else(|| "srt".into());
                match crate::subs::to_vtt(&text, &ext) {
                    Ok(v) => v,
                    Err(_) => continue,
                }
            };
            let file = format!("sub_{i:02}_{}.vtt", sanitize_component(&sub.lang));
            if tokio::fs::write(dir.join(&file), vtt).await.is_ok() {
                out.push(ManifestSub {
                    label: sub.label.clone(),
                    lang: sub.lang.clone(),
                    file,
                    default: sub.default,
                });
            }
        }
        out
    }
}

/// One media playlist of an HLS download (the video, or its separate audio)
/// with the local filenames its segments and keys are saved under.
struct HlsTrack {
    playlist: String,
    base_url: String,
    segments: Vec<String>,
    keys: Vec<String>,
    /// Absolute upstream URL -> local filename.
    map: HashMap<String, String>,
}

impl HlsTrack {
    /// `seg_prefix` / `key_prefix` keep two tracks' files apart in one dir.
    fn new(playlist: String, base_url: String, seg_prefix: &str, key_prefix: &str) -> Self {
        let segments = hls::parse_segments(&playlist, &base_url);
        let keys = hls::parse_uri_attrs(&playlist, &base_url);
        let mut map: HashMap<String, String> = HashMap::new();
        for (i, url) in segments.iter().enumerate() {
            map.entry(url.clone())
                .or_insert_with(|| format!("{seg_prefix}_{i:05}.{}", seg_ext(url)));
        }
        for (i, url) in keys.iter().enumerate() {
            map.entry(url.clone())
                .or_insert_with(|| format!("{key_prefix}_{i:02}.bin"));
        }
        Self {
            playlist,
            base_url,
            segments,
            keys,
            map,
        }
    }

    /// The playlist rewritten to reference the local filenames.
    fn localized(&self) -> String {
        hls::localize_playlist(&self.playlist, &self.base_url, &self.map)
    }
}

/// File extension for a local segment file (playback doesn't strictly need it,
/// but keeping fMP4 segments as .m4s is tidier).
fn seg_ext(url: &str) -> &'static str {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    if path.ends_with(".m4s") {
        "m4s"
    } else if path.ends_with(".mp4") {
        "mp4"
    } else if path.ends_with(".aac") {
        "aac"
    } else {
        "ts"
    }
}

/// Throttled progress persistence + event emission (~2/s) with speed math.
struct Tracker<'a> {
    mgr: &'a DownloadManager,
    id: i64,
    anime_id: String,
    episode_number: String,
    bytes_done: i64,
    bytes_total: Option<i64>,
    segments_done: i64,
    segments_total: Option<i64>,
    last_tick: Instant,
    last_bytes: i64,
}

impl<'a> Tracker<'a> {
    fn new(mgr: &'a DownloadManager, row: &DownloadRow) -> Self {
        Self {
            mgr,
            id: row.id,
            anime_id: row.anime_id.clone(),
            episode_number: row.episode_number.clone(),
            bytes_done: row.bytes_done,
            bytes_total: row.bytes_total,
            segments_done: row.segments_done,
            segments_total: row.segments_total,
            last_tick: Instant::now(),
            last_bytes: row.bytes_done,
        }
    }

    fn set(
        &mut self,
        bytes_done: i64,
        bytes_total: Option<i64>,
        segments_done: i64,
        segments_total: Option<i64>,
    ) {
        self.bytes_done = bytes_done;
        if bytes_total.is_some() {
            self.bytes_total = bytes_total;
        }
        self.segments_done = segments_done;
        if segments_total.is_some() {
            self.segments_total = segments_total;
        }
        if self.last_tick.elapsed() >= PROGRESS_INTERVAL {
            self.persist_now();
        }
    }

    /// Persist the checkpoint and emit a progress event immediately.
    fn persist_now(&mut self) {
        let dt = self.last_tick.elapsed().as_secs_f64().max(0.001);
        let speed = (self.bytes_done - self.last_bytes).max(0) as f64 / dt;
        self.last_tick = Instant::now();
        self.last_bytes = self.bytes_done;
        let _ = self.mgr.db.update_download_progress(
            self.id,
            self.bytes_done,
            self.bytes_total,
            self.segments_done,
            self.segments_total,
        );
        self.mgr.emit(DownloadEvent::Progress(ProgressPayload {
            id: self.id,
            anime_id: self.anime_id.clone(),
            episode_number: self.episode_number.clone(),
            bytes_done: self.bytes_done,
            bytes_total: self.bytes_total,
            segments_done: self.segments_done,
            segments_total: self.segments_total,
            speed_bps: speed,
        }));
    }

    fn finish(&mut self) {
        if self.bytes_total.is_none() {
            self.bytes_total = Some(self.bytes_done);
        }
        self.persist_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::StreamKind::*;

    fn src(name: &str, quality: &str, url: &str, kind: crate::models::StreamKind) -> VideoSource {
        VideoSource {
            source: String::new(),
            provider_name: name.into(),
            quality: quality.into(),
            url: url.into(),
            kind,
            referer: None,
            subtitles: Vec::new(),
        }
    }

    #[test]
    fn transitions_follow_the_state_machine() {
        use DownloadState::*;
        // Legal.
        assert!(can_transition(Queued, Downloading));
        assert!(can_transition(Queued, Paused));
        assert!(can_transition(Downloading, Done));
        assert!(can_transition(Downloading, Failed));
        assert!(can_transition(Downloading, Paused));
        assert!(can_transition(Downloading, Queued)); // startup recovery
        assert!(can_transition(Paused, Queued));
        assert!(can_transition(Failed, Queued));
        // Illegal.
        assert!(!can_transition(Done, Queued));
        assert!(!can_transition(Done, Downloading));
        assert!(!can_transition(Queued, Done));
        assert!(!can_transition(Paused, Done));
        assert!(!can_transition(Paused, Downloading)); // must re-queue first
        assert!(!can_transition(Failed, Downloading));
    }

    #[test]
    fn pick_source_prefers_exact_quality_then_direct() {
        let sources = vec![
            src("a", "1080", "https://cdn/a.mp4", Mp4),
            src("b", "720", "https://cdn/b.mp4", Mp4),
            src("c", "auto", "https://cdn/c.m3u8", Hls),
            src("embed", "1080", "https://ok.ru/videoembed/1", Mp4),
        ];
        assert_eq!(pick_source(&sources, "720").unwrap().provider_name, "b");
        assert_eq!(pick_source(&sources, "1080").unwrap().provider_name, "a");
        // "best" picks the highest numeric quality.
        assert_eq!(pick_source(&sources, "best").unwrap().provider_name, "a");
        // Embed pages are never downloadable.
        let only_embed = vec![src("embed", "1080", "https://ok.ru/videoembed/1", Mp4)];
        assert!(pick_source(&only_embed, "best").is_none());
    }

    #[test]
    fn pick_source_hls_master_adapts_to_any_quality() {
        let sources = vec![
            src("mp4", "480", "https://cdn/a.mp4", Mp4),
            src("hls", "hls-multi", "https://cdn/master.m3u8", Hls),
        ];
        // The master can serve 1080 via its variants; the 480 mp4 cannot.
        assert_eq!(pick_source(&sources, "1080").unwrap().provider_name, "hls");
        // But an exact numeric match still wins.
        assert_eq!(pick_source(&sources, "480").unwrap().provider_name, "mp4");
    }

    #[test]
    fn sanitize_and_episode_dir() {
        assert_eq!(sanitize_component("abc123"), "abc123");
        assert_eq!(sanitize_component("5.5"), "5.5");
        assert_eq!(sanitize_component("a/b\\c:d"), "a_b_c_d");
        assert_eq!(sanitize_component(".."), "_");
        assert_eq!(sanitize_component(""), "_");
        assert_eq!(episode_dir_rel("show1", "5.5"), "show1/5.5");
        assert_eq!(episode_dir_rel("../evil", "1"), ".._evil/1");
    }

    #[test]
    fn quality_num_parses_labels() {
        assert_eq!(quality_num("1080"), Some(1080));
        assert_eq!(quality_num("720p"), Some(720));
        assert_eq!(quality_num("best"), None);
        assert_eq!(quality_num("hls-multi"), None);
    }

    #[test]
    fn manifest_roundtrip() {
        let dir = std::env::temp_dir().join(format!("anidoku-manifest-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("show/1")).unwrap();
        let m = Manifest {
            kind: Hls,
            quality: "1080".into(),
            video: "index.m3u8".into(),
            subtitles: vec![ManifestSub {
                label: "English".into(),
                lang: "en".into(),
                file: "sub_00_en.vtt".into(),
                default: false,
            }],
        };
        std::fs::write(
            dir.join("show/1/manifest.json"),
            serde_json::to_vec(&m).unwrap(),
        )
        .unwrap();
        let read = read_manifest(&dir, "show/1").unwrap();
        assert_eq!(read.video, "index.m3u8");
        assert_eq!(read.subtitles.len(), 1);
        assert!(read_manifest(&dir, "show/2").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn is_transient_classifies_download_errors() {
        // The stall / early-end / timeout family + retryable HTTP statuses.
        assert!(is_transient(&Error::Download(
            "download stalled (timeout)".into()
        )));
        assert!(is_transient(&Error::Download(
            "connection ended early (10/20 bytes) — resume to continue".into()
        )));
        assert!(is_transient(&Error::Download(
            "fetch timed out: https://cdn/seg".into()
        )));
        assert!(is_transient(&Error::Download(
            "upstream returned HTTP 503".into()
        )));
        assert!(is_transient(&Error::Download(
            "upstream returned HTTP 429".into()
        )));
        // A 4xx (other than 429) and non-network errors are terminal.
        assert!(!is_transient(&Error::Download(
            "upstream returned HTTP 404".into()
        )));
        assert!(!is_transient(&Error::Download(
            "no downloadable source found".into()
        )));
        assert!(!is_transient(&Error::Io(std::io::Error::other(
            "disk full"
        ))));
    }

    #[test]
    fn backoff_delay_is_monotonic_and_capped() {
        let seq: Vec<Duration> = (0..8).map(backoff_delay).collect();
        assert_eq!(seq[0], BACKOFF_BASE);
        // Doubles until the cap, then holds.
        for w in seq.windows(2) {
            assert!(w[1] >= w[0], "backoff must be non-decreasing");
            assert!(w[1] <= BACKOFF_CAP, "backoff must never exceed the cap");
        }
        assert_eq!(*seq.last().unwrap(), BACKOFF_CAP);
    }

    #[test]
    fn hls_tracks_keep_video_and_audio_files_apart() {
        let media = "#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:6,\ns0.ts\n#EXTINF:6,\ns1.ts\n#EXT-X-ENDLIST\n";
        let video = HlsTrack::new(
            media.to_string(),
            "https://cdn.x/video/1080/playlist.m3u8".into(),
            "seg",
            "key",
        );
        let audio = HlsTrack::new(
            media.to_string(),
            "https://cdn.x/audio/ja/playlist.m3u8".into(),
            "aud",
            "akey",
        );
        assert_eq!(video.segments.len(), 2);
        // Same relative names upstream, distinct files on disk.
        let v = video.localized();
        let a = audio.localized();
        assert!(
            v.contains("seg_00000.ts") && v.contains("URI=\"key_00.bin\""),
            "{v}"
        );
        assert!(
            a.contains("aud_00001.ts") && a.contains("URI=\"akey_00.bin\""),
            "{a}"
        );
        assert!(!a.contains("seg_") && !v.contains("https://"));
    }

    #[tokio::test]
    async fn retry_transient_drives_attempts_then_succeeds() {
        let flag = AtomicU8::new(CTL_RUN);
        let calls = std::cell::Cell::new(0u32);
        let out: Result<u32> = retry_transient(&flag, || {
            let n = calls.get() + 1;
            calls.set(n);
            async move {
                if n < 3 {
                    Err(Error::Download("download stalled (timeout)".into()))
                } else {
                    Ok(n)
                }
            }
        })
        .await;
        assert_eq!(out.unwrap(), 3);
        assert_eq!(calls.get(), 3);
    }

    #[tokio::test]
    async fn retry_transient_gives_up_on_non_transient() {
        let flag = AtomicU8::new(CTL_RUN);
        let calls = std::cell::Cell::new(0u32);
        let out: Result<u32> = retry_transient(&flag, || {
            calls.set(calls.get() + 1);
            async { Err(Error::Download("upstream returned HTTP 404".into())) }
        })
        .await;
        assert!(out.is_err());
        assert_eq!(calls.get(), 1, "a non-transient error must not retry");
    }

    /// End-to-end-ish: a localhost server that promises the full length then
    /// drops mid-body on the first attempt (reqwest sees an incomplete body →
    /// transient), and honors the `Range` on the retry. Byte-accurate resume
    /// must reassemble the exact bytes.
    #[tokio::test]
    async fn ranged_retry_resumes_after_midbody_reset() {
        use std::io::{Read, Write};

        let full: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let first_chunk = 400usize;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let body = full.clone();
        std::thread::spawn(move || {
            // Attempt 1: promise the full length, then close mid-body.
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf);
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(hdr.as_bytes());
                let _ = sock.write_all(&body[..first_chunk]);
                // Drop `sock` here, leaving the promised body incomplete.
            }
            // Attempt 2: honor the Range and serve the remainder in full.
            if let Ok((mut sock, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let n = sock.read(&mut buf).unwrap_or(0);
                // reqwest/hyper may emit the header name lower-cased, so match on
                // the `bytes=` marker rather than a case-sensitive prefix.
                let req = String::from_utf8_lossy(&buf[..n]);
                let start = req
                    .split("bytes=")
                    .nth(1)
                    .and_then(|r| r.split(['-', '\r', '\n', ' ']).next())
                    .and_then(|n| n.parse::<usize>().ok())
                    .unwrap_or(0);
                let rest = &body[start..];
                let resp = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {}-{}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    start,
                    body.len() - 1,
                    body.len(),
                    rest.len()
                );
                let _ = sock.write_all(resp.as_bytes());
                let _ = sock.write_all(rest);
            }
        });

        let proxy = ProxyClient::new();
        let url = format!("http://{addr}/video.mp4");
        let flag = AtomicU8::new(CTL_RUN);
        let buf = Arc::new(Mutex::new(Vec::<u8>::new()));

        let out: Result<()> = retry_transient(&flag, || {
            let proxy = &proxy;
            let url = &url;
            let buf = buf.clone();
            async move {
                // Re-read the on-disk (here in-memory) offset: each retry resumes.
                let offset = buf.lock().unwrap().len() as i64;
                let range = (offset > 0).then(|| format!("bytes={offset}-"));
                let mut resp = proxy.get_ranged(url, None, range.as_deref()).await?;
                let status = resp.status().as_u16();
                if !(200..300).contains(&status) {
                    return Err(Error::Download(format!("upstream returned HTTP {status}")));
                }
                while let Some(chunk) = resp.chunk().await? {
                    buf.lock().unwrap().extend_from_slice(&chunk);
                }
                Ok(())
            }
        })
        .await;

        assert!(out.is_ok(), "retry should recover the download: {out:?}");
        assert_eq!(*buf.lock().unwrap(), full, "resumed bytes must be exact");
    }
}
