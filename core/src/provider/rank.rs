//! Source-neutral playability ranking.
//!
//! Lives outside any one source module because every scraper produces the same
//! mix of direct media links and embed *pages*, and the download engine ranks
//! them the same way regardless of where they came from.

use crate::models::{StreamKind, VideoSource};

/// Best-effort playability ranking for ordering sources (0 = best).
///
/// A webview `<video>` can only play a direct media file (MP4/HLS), not an
/// embed *page*. allanime mixes both kinds into one list; ani-cli sidesteps
/// this by only handling a known subset. We can't extract embed pages here, so
/// we at least float the directly-playable sources to the top:
///   0 — looks like a direct media file / known direct CDN
///   1 — unknown (could be either)
///   2 — looks like an HTML embed page (ok.ru, mp4upload, /e/…, …)
pub fn playability_rank(s: &VideoSource) -> u8 {
    let url = s.url.to_ascii_lowercase();
    let path = url.split(['?', '#']).next().unwrap_or(&url);

    let is_media_ext = path.ends_with(".m3u8")
        || path.ends_with(".mp4")
        || path.ends_with(".m4v")
        || path.ends_with(".mkv")
        || path.ends_with(".webm");
    let is_direct_cdn = url.contains("fast4speed")
        || url.contains("wixmp")
        || (url.contains("sharepoint.com") && url.contains("download.aspx"));
    if matches!(s.kind, StreamKind::Hls) || is_media_ext || is_direct_cdn {
        return 0;
    }

    let is_embed_page = path.ends_with(".html")
        || url.contains("/e/")
        || url.contains("/embed")
        || url.contains("videoembed")
        || url.contains("ok.ru")
        || url.contains("mp4upload")
        || url.contains("vidnest")
        // Fragment-routed single-page embeds (e.g. allanime.uns.bio/#abc123).
        || url.contains("uns.bio");
    if is_embed_page {
        return 2;
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(url: &str, kind: StreamKind) -> VideoSource {
        VideoSource {
            source: String::new(),
            provider_name: "t".into(),
            quality: "auto".into(),
            url: url.into(),
            kind,
            referer: None,
            subtitles: Vec::new(),
        }
    }

    #[test]
    fn playability_rank_orders_direct_media_before_embeds() {
        use crate::models::StreamKind::*;
        // Direct media / known CDNs -> 0
        assert_eq!(playability_rank(&src("https://cdn/x.mp4", Mp4)), 0);
        assert_eq!(playability_rank(&src("https://cdn/x.m3u8", Hls)), 0);
        assert_eq!(
            playability_rank(&src(
                "https://tools.fast4speed.rsvp/media/1?Authorization=z",
                Mp4
            )),
            0
        );
        assert_eq!(
            playability_rank(&src(
                "https://x.sharepoint.com/_layouts/15/download.aspx?id=1",
                Mp4
            )),
            0
        );
        // HTML embed pages -> 2
        assert_eq!(
            playability_rank(&src("https://ok.ru/videoembed/123", Mp4)),
            2
        );
        assert_eq!(
            playability_rank(&src("https://mp4upload.com/embed-a.html", Mp4)),
            2
        );
        assert_eq!(playability_rank(&src("https://vidnest.io/e/abc", Mp4)), 2);
        assert_eq!(
            playability_rank(&src("https://allanime.uns.bio/#abc", Mp4)),
            2
        );
        // Unknown -> 1
        assert_eq!(playability_rank(&src("https://weird.host/thing", Mp4)), 1);
    }
}
