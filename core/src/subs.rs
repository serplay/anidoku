//! Subtitle conversion. The webview player only reliably renders WebVTT text
//! tracks, so external SRT (and the common subset of ASS/SSA dialogue) is
//! converted to VTT here before being handed to the player.

use crate::{Error, Result};

/// Convert SubRip (SRT) text to WebVTT.
///
/// SRT and VTT differ in three ways that matter: VTT needs a `WEBVTT` header,
/// VTT uses `.` (not `,`) as the millisecond separator, and VTT does not use
/// the numeric cue index lines. This handles those, and tolerates the
/// hour-less `MM:SS,mmm` timestamps some files use.
pub fn srt_to_vtt(srt: &str) -> String {
    let mut out = String::from("WEBVTT\n\n");
    let normalized = srt.replace("\r\n", "\n").replace('\r', "\n");

    for block in normalized.split("\n\n") {
        let block = block.trim_matches('\n');
        if block.is_empty() {
            continue;
        }
        let mut lines = block.lines().peekable();

        // Drop a leading numeric index line if present.
        if let Some(first) = lines.peek() {
            if first.trim().parse::<u32>().is_ok() {
                lines.next();
            }
        }

        let Some(timing) = lines.next() else { continue };
        if !timing.contains("-->") {
            continue;
        }
        out.push_str(&convert_timing_line(timing));
        out.push('\n');
        for text in lines {
            out.push_str(text);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

fn convert_timing_line(line: &str) -> String {
    line.split("-->")
        .map(|t| normalize_timestamp(t.trim()))
        .collect::<Vec<_>>()
        .join(" --> ")
}

fn normalize_timestamp(ts: &str) -> String {
    // Keep only the timestamp token (drop any trailing cue-position settings).
    let core = ts.split_whitespace().next().unwrap_or(ts);
    let with_dot = core.replace(',', ".");
    // VTT accepts MM:SS.mmm, but normalize to HH:MM:SS.mmm for consistency.
    let colons = with_dot.matches(':').count();
    if colons == 1 {
        format!("00:{with_dot}")
    } else {
        with_dot
    }
}

/// Convert a subtitle file to VTT based on a format hint (usually the file
/// extension). ASS/SSA is not yet parsed — see the TODO — so callers get a
/// clear error rather than a silently broken track.
pub fn to_vtt(content: &str, format_hint: &str) -> Result<String> {
    match format_hint.to_ascii_lowercase().trim_start_matches('.') {
        "vtt" => Ok(content.to_string()),
        "srt" => Ok(srt_to_vtt(content)),
        // TODO(M1+): parse ASS/SSA `[Events]` Dialogue lines, strip override
        // tags ({\...}), and map Start/End to VTT cues.
        other => Err(Error::Subtitle(format!(
            "unsupported subtitle format: {other} (only srt/vtt implemented)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_basic_conversion() {
        let srt = "1\n00:00:01,000 --> 00:00:04,000\nHello world\n\n2\n00:00:05,500 --> 00:00:07,000\nSecond line\n";
        let vtt = srt_to_vtt(srt);
        assert!(vtt.starts_with("WEBVTT\n\n"));
        assert!(vtt.contains("00:00:01.000 --> 00:00:04.000"));
        assert!(vtt.contains("Hello world"));
        assert!(vtt.contains("00:00:05.500 --> 00:00:07.000"));
        // No numeric index lines survive.
        assert!(!vtt.contains("\n1\n"));
    }

    #[test]
    fn srt_handles_crlf_and_hourless_timestamps() {
        let srt = "1\r\n01:02,000 --> 01:05,000\r\nText\r\n";
        let vtt = srt_to_vtt(srt);
        assert!(vtt.contains("00:01:02.000 --> 00:01:05.000"));
    }

    #[test]
    fn srt_multiline_cue() {
        let srt = "1\n00:00:01,000 --> 00:00:02,000\nLine A\nLine B\n";
        let vtt = srt_to_vtt(srt);
        assert!(vtt.contains("Line A\nLine B\n"));
    }

    #[test]
    fn to_vtt_dispatch() {
        assert!(to_vtt("WEBVTT\n\n", "vtt").unwrap().starts_with("WEBVTT"));
        assert!(to_vtt("1\n00:00:01,000 --> 00:00:02,000\nx\n", ".srt")
            .unwrap()
            .starts_with("WEBVTT"));
        assert!(to_vtt("x", "ass").is_err());
    }
}
