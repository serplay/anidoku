//! Pure HLS playlist logic for the download engine: master-variant selection,
//! segment/key extraction, and rewriting a playlist to point at local files.
//! No I/O here, so all of it is unit-testable against playlist fixtures.

use crate::proxy::{resolve_url, rewrite_playlist};
use std::collections::HashMap;

/// One variant stream from a master playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub url: String,
    pub bandwidth: u64,
    /// Vertical resolution from RESOLUTION=WxH, 0 when absent.
    pub height: u32,
    /// `AUDIO="…"` group, when the variant's audio lives in a separate
    /// rendition (demuxed HLS) rather than inside its own segments.
    pub audio_group: Option<String>,
}

/// One `#EXT-X-MEDIA:TYPE=AUDIO` rendition that has its own playlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioRendition {
    pub group: String,
    pub name: String,
    pub language: String,
    pub default: bool,
    pub url: String,
}

/// Parse `#EXT-X-STREAM-INF` variants out of a master playlist. Returns an
/// empty vec for media playlists (no variants → the playlist itself is the
/// media playlist).
pub fn parse_variants(playlist: &str, base_url: &str) -> Vec<Variant> {
    let mut out = Vec::new();
    let mut pending: Option<(u64, u32, Option<String>)> = None;
    for line in playlist.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let attrs = parse_attrs(rest);
            let bandwidth = attrs
                .get("BANDWIDTH")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let height = attrs
                .get("RESOLUTION")
                .and_then(|v| v.split(['x', 'X']).nth(1))
                .and_then(|h| h.trim().parse::<u32>().ok())
                .unwrap_or(0);
            pending = Some((bandwidth, height, attrs.get("AUDIO").cloned()));
        } else if !line.starts_with('#') {
            if let Some((bandwidth, height, audio_group)) = pending.take() {
                out.push(Variant {
                    url: resolve_url(base_url, line),
                    bandwidth,
                    height,
                    audio_group,
                });
            }
        }
    }
    out
}

/// Audio renditions of a master playlist that carry their own playlist. A
/// rendition without a `URI` is muxed into the variant and needs nothing extra.
pub fn parse_audio_renditions(playlist: &str, base_url: &str) -> Vec<AudioRendition> {
    playlist
        .lines()
        .filter_map(|line| line.trim().strip_prefix("#EXT-X-MEDIA:"))
        .map(parse_attrs)
        .filter(|a| a.get("TYPE").map(String::as_str) == Some("AUDIO"))
        .filter_map(|a| {
            Some(AudioRendition {
                url: resolve_url(base_url, a.get("URI")?),
                group: a.get("GROUP-ID").cloned().unwrap_or_default(),
                name: a.get("NAME").cloned().unwrap_or_default(),
                language: a.get("LANGUAGE").cloned().unwrap_or_default(),
                default: a.get("DEFAULT").map(String::as_str) == Some("YES"),
            })
        })
        .collect()
}

/// The audio rendition to keep for an offline copy: within the variant's
/// group, a `preferred_language` match if asked for and present, else the one
/// the stream marks as default, else the first.
pub fn pick_audio<'a>(
    renditions: &'a [AudioRendition],
    group: &str,
    preferred_language: Option<&str>,
) -> Option<&'a AudioRendition> {
    let in_group: Vec<&AudioRendition> = renditions.iter().filter(|r| r.group == group).collect();
    preferred_language
        .and_then(|lang| {
            in_group
                .iter()
                .find(|r| r.language.to_ascii_lowercase().starts_with(lang))
        })
        .or_else(|| in_group.iter().find(|r| r.default))
        .or_else(|| in_group.first())
        .copied()
}

/// A local master playlist tying a downloaded video playlist to its separately
/// downloaded audio playlist (both localized, sitting next to it).
pub fn local_master(
    variant: &Variant,
    audio: &AudioRendition,
    video: &str,
    audio_file: &str,
) -> String {
    let mut out = String::from("#EXTM3U\n#EXT-X-VERSION:3\n");
    out.push_str(&format!(
        "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",NAME=\"{}\",LANGUAGE=\"{}\",DEFAULT=YES,AUTOSELECT=YES,URI=\"{audio_file}\"\n",
        audio.name.replace('"', ""),
        audio.language.replace('"', ""),
    ));
    out.push_str(&format!(
        "#EXT-X-STREAM-INF:BANDWIDTH={},AUDIO=\"audio\"\n{video}\n",
        variant.bandwidth.max(1)
    ));
    out
}

/// Bare URI lines of a media playlist (the segments), resolved to absolute
/// URLs, in playlist order.
pub fn parse_segments(playlist: &str, base_url: &str) -> Vec<String> {
    playlist
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| resolve_url(base_url, l))
        .collect()
}

/// `URI="..."` attribute values in tag lines (EXT-X-KEY, EXT-X-MAP, ...),
/// resolved to absolute URLs, deduplicated in first-seen order.
pub fn parse_uri_attrs(playlist: &str, base_url: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in playlist.lines() {
        let line = line.trim();
        if !line.starts_with('#') {
            continue;
        }
        if let Some(start) = line.find("URI=\"") {
            let rest = &line[start + 5..];
            if let Some(end) = rest.find('"') {
                let abs = resolve_url(base_url, &rest[..end]);
                if !out.contains(&abs) {
                    out.push(abs);
                }
            }
        }
    }
    out
}

/// Rewrite a media playlist so every URI (segments and URI="..." attributes)
/// is replaced with its local filename from `map` (absolute upstream URL →
/// local relative name). URLs not in the map are left untouched.
pub fn localize_playlist(playlist: &str, base_url: &str, map: &HashMap<String, String>) -> String {
    rewrite_playlist(playlist, base_url, |abs| {
        map.get(abs).cloned().unwrap_or_else(|| abs.to_string())
    })
}

/// Choose a variant for a desired quality: "best" (or anything non-numeric)
/// picks the highest height/bandwidth; a numeric preference ("720") picks the
/// closest height, breaking ties towards the higher bandwidth.
pub fn pick_variant<'a>(variants: &'a [Variant], desired: &str) -> Option<&'a Variant> {
    if variants.is_empty() {
        return None;
    }
    match desired.trim().parse::<i64>() {
        Ok(want) => variants.iter().min_by_key(|v| {
            let h = v.height as i64;
            // Unknown heights sort last; ties prefer higher bandwidth.
            let dist = if h == 0 {
                i64::MAX / 2
            } else {
                (h - want).abs()
            };
            (dist, std::cmp::Reverse(v.bandwidth))
        }),
        Err(_) => variants.iter().max_by_key(|v| (v.height, v.bandwidth)),
    }
}

/// Parse an m3u8 attribute list (`A=1,B="x,y",C=2`) respecting quoted values.
fn parse_attrs(s: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = s;
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim().to_string();
        rest = &rest[eq + 1..];
        let value;
        if let Some(stripped) = rest.strip_prefix('"') {
            let Some(endq) = stripped.find('"') else {
                break;
            };
            value = stripped[..endq].to_string();
            rest = &stripped[endq + 1..];
            rest = rest.strip_prefix(',').unwrap_or(rest);
        } else {
            match rest.find(',') {
                Some(c) => {
                    value = rest[..c].trim().to_string();
                    rest = &rest[c + 1..];
                }
                None => {
                    value = rest.trim().to_string();
                    rest = "";
                }
            }
        }
        out.insert(key, value);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "#EXTM3U\n\
#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1920x1080,CODECS=\"avc1.4d401f,mp4a.40.2\"\n\
1080/index.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=1280x720\n\
720/index.m3u8\n\
#EXT-X-STREAM-INF:BANDWIDTH=400000,RESOLUTION=854x480\n\
https://other.cdn/480/index.m3u8\n";

    const MEDIA: &str = "#EXTM3U\n\
#EXT-X-VERSION:3\n\
#EXT-X-KEY:METHOD=AES-128,URI=\"key.bin\",IV=0x1\n\
#EXTINF:6.0,\n\
seg0.ts\n\
#EXTINF:6.0,\n\
seg1.ts\n\
#EXTINF:4.2,\n\
sub/seg2.ts\n\
#EXT-X-ENDLIST\n";

    #[test]
    fn variants_parse_with_quoted_codecs() {
        let v = parse_variants(MASTER, "https://cdn/hls/master.m3u8");
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].height, 1080);
        assert_eq!(v[0].bandwidth, 2_000_000);
        assert_eq!(v[0].url, "https://cdn/hls/1080/index.m3u8");
        assert_eq!(v[2].url, "https://other.cdn/480/index.m3u8");
        // A media playlist has no variants.
        assert!(parse_variants(MEDIA, "https://cdn/hls/720/index.m3u8").is_empty());
    }

    #[test]
    fn segments_resolve_in_order() {
        let segs = parse_segments(MEDIA, "https://cdn/hls/720/index.m3u8");
        assert_eq!(
            segs,
            vec![
                "https://cdn/hls/720/seg0.ts",
                "https://cdn/hls/720/seg1.ts",
                "https://cdn/hls/720/sub/seg2.ts",
            ]
        );
    }

    #[test]
    fn uri_attrs_extract_keys() {
        let keys = parse_uri_attrs(MEDIA, "https://cdn/hls/720/index.m3u8");
        assert_eq!(keys, vec!["https://cdn/hls/720/key.bin"]);
    }

    #[test]
    fn localize_rewrites_to_local_names() {
        let base = "https://cdn/hls/720/index.m3u8";
        let mut map = HashMap::new();
        map.insert(
            "https://cdn/hls/720/seg0.ts".to_string(),
            "seg_00000.ts".to_string(),
        );
        map.insert(
            "https://cdn/hls/720/seg1.ts".to_string(),
            "seg_00001.ts".to_string(),
        );
        map.insert(
            "https://cdn/hls/720/sub/seg2.ts".to_string(),
            "seg_00002.ts".to_string(),
        );
        map.insert(
            "https://cdn/hls/720/key.bin".to_string(),
            "key_00.bin".to_string(),
        );
        let local = localize_playlist(MEDIA, base, &map);
        assert!(local.contains("\nseg_00000.ts\n"));
        assert!(local.contains("\nseg_00001.ts\n"));
        assert!(local.contains("\nseg_00002.ts\n"));
        assert!(local.contains("URI=\"key_00.bin\""));
        // Tags and durations survive untouched.
        assert!(local.contains("#EXTINF:6.0,"));
        assert!(local.contains("#EXT-X-ENDLIST"));
        assert!(!local.contains("https://cdn"));
    }

    #[test]
    fn pick_variant_best_and_closest() {
        let v = parse_variants(MASTER, "https://cdn/hls/master.m3u8");
        assert_eq!(pick_variant(&v, "best").unwrap().height, 1080);
        assert_eq!(pick_variant(&v, "720").unwrap().height, 720);
        assert_eq!(pick_variant(&v, "600").unwrap().height, 720); // closest
        assert_eq!(pick_variant(&v, "480").unwrap().height, 480);
        assert_eq!(pick_variant(&v, "144").unwrap().height, 480);
        assert!(pick_variant(&[], "best").is_none());
    }

    #[test]
    fn pick_variant_without_resolution_uses_bandwidth() {
        let v = vec![
            Variant {
                url: "a".into(),
                bandwidth: 100,
                height: 0,
                audio_group: None,
            },
            Variant {
                url: "b".into(),
                bandwidth: 900,
                height: 0,
                audio_group: None,
            },
        ];
        assert_eq!(pick_variant(&v, "best").unwrap().url, "b");
        assert_eq!(pick_variant(&v, "720").unwrap().url, "b");
    }

    /// Shape of a real demuxed master (AniZone): audio in its own renditions.
    const DEMUXED: &str = "#EXTM3U\n\
#EXT-X-VERSION:3\n\
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"group_audio\",NAME=\"English (US)\",DEFAULT=NO,LANGUAGE=\"en\",CHANNELS=\"2\",URI=\"audio/2_en/playlist.m3u8\"\n\
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"group_audio\",NAME=\"Japanese\",DEFAULT=YES,LANGUAGE=\"ja\",CHANNELS=\"2\",URI=\"audio/3_ja/playlist.m3u8\"\n\
#EXT-X-MEDIA:TYPE=SUBTITLES,GROUP-ID=\"subs\",NAME=\"English\",URI=\"subs/en.m3u8\"\n\
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"muxed\",NAME=\"In-band\",DEFAULT=YES\n\
#EXT-X-STREAM-INF:BANDWIDTH=3476000,RESOLUTION=1920x1080,CODECS=\"avc1.640032,mp4a.40.2\",AUDIO=\"group_audio\"\n\
video/1080/playlist.m3u8\n";

    #[test]
    fn variants_carry_their_audio_group() {
        let v = parse_variants(DEMUXED, "https://cdn.x/a/master.m3u8");
        assert_eq!(v[0].audio_group.as_deref(), Some("group_audio"));
        // A muxed master has none.
        let muxed = parse_variants(MASTER, "https://cdn.x/a/master.m3u8");
        assert!(muxed.iter().all(|v| v.audio_group.is_none()));
    }

    #[test]
    fn audio_renditions_are_the_ones_with_their_own_playlist() {
        let a = parse_audio_renditions(DEMUXED, "https://cdn.x/a/master.m3u8");
        // Subtitles and the URI-less in-band rendition are not downloads.
        assert_eq!(a.len(), 2);
        assert_eq!(a[1].name, "Japanese");
        assert_eq!(a[1].url, "https://cdn.x/a/audio/3_ja/playlist.m3u8");
        assert!(a[1].default && !a[0].default);
        assert!(parse_audio_renditions(MASTER, "https://cdn.x/m.m3u8").is_empty());
    }

    #[test]
    fn pick_audio_prefers_language_then_default_then_first() {
        let a = parse_audio_renditions(DEMUXED, "https://cdn.x/a/master.m3u8");
        assert_eq!(pick_audio(&a, "group_audio", None).unwrap().language, "ja");
        assert_eq!(
            pick_audio(&a, "group_audio", Some("en")).unwrap().language,
            "en"
        );
        // Asked-for language missing: fall back to the default, not nothing.
        assert_eq!(
            pick_audio(&a, "group_audio", Some("fr")).unwrap().language,
            "ja"
        );
        assert!(pick_audio(&a, "other_group", None).is_none());
        let mut no_default = a.clone();
        no_default[1].default = false;
        assert_eq!(
            pick_audio(&no_default, "group_audio", None)
                .unwrap()
                .language,
            "en"
        );
    }

    #[test]
    fn local_master_links_video_to_its_audio() {
        let v = &parse_variants(DEMUXED, "https://cdn.x/a/master.m3u8")[0];
        let a = parse_audio_renditions(DEMUXED, "https://cdn.x/a/master.m3u8");
        let m = local_master(v, &a[1], "video.m3u8", "audio.m3u8");
        // Round-trips through our own parsers as a demuxed master.
        let back = parse_variants(&m, "http://127.0.0.1:1/dl/show/1/index.m3u8");
        assert_eq!(back[0].url, "http://127.0.0.1:1/dl/show/1/video.m3u8");
        let audio = parse_audio_renditions(&m, "http://127.0.0.1:1/dl/show/1/index.m3u8");
        assert_eq!(audio[0].url, "http://127.0.0.1:1/dl/show/1/audio.m3u8");
        assert_eq!(
            pick_audio(&audio, back[0].audio_group.as_deref().unwrap(), None),
            Some(&audio[0])
        );
    }
}
