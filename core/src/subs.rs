//! Subtitle conversion. The webview player only reliably renders WebVTT text
//! tracks, so SRT and the dialogue subset of ASS/SSA are converted to VTT here
//! before being handed to the player.

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

/// Convert Advanced SubStation Alpha (ASS/SSA) to WebVTT.
///
/// ASS is the dominant fansub/simulcast format and far richer than VTT, so
/// this is deliberately a *dialogue* conversion, not a typesetting one: every
/// `Dialogue:` event becomes a cue, override blocks (`{\pos(..)\c&H..}`) are
/// dropped, `\N` becomes a line break, and vector drawings (`\p1`) are skipped
/// because their "text" is path data. The only layout that survives is top
/// alignment (`\an7`-`\an9`, or a style whose `Alignment` is 7-9), so signs
/// don't sit on top of the dialogue line.
///
/// Column positions come from each section's `Format:` line rather than being
/// assumed, since SSA v4 and ASS v4+ order them differently.
pub fn ass_to_vtt(ass: &str) -> String {
    #[derive(PartialEq)]
    enum Section {
        Styles,
        Events,
        Other,
    }
    let mut section = Section::Other;
    let mut style_format: Vec<String> = Vec::new();
    let mut event_format: Vec<String> = Vec::new();
    let mut top_styles: Vec<String> = Vec::new();
    // (start_ms, end_ms, top, text)
    let mut cues: Vec<(u64, u64, bool, String)> = Vec::new();

    for raw in ass.trim_start_matches('\u{feff}').lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            let name = line.to_ascii_lowercase();
            section = if name.contains("styles") {
                Section::Styles
            } else if name == "[events]" {
                Section::Events
            } else {
                Section::Other
            };
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim_start();
        match (&section, key.trim().to_ascii_lowercase().as_str()) {
            (Section::Styles, "format") => style_format = format_columns(value),
            (Section::Styles, "style") => {
                let cols: Vec<&str> = value.split(',').map(str::trim).collect();
                let name = column(&style_format, &cols, "name");
                let align =
                    column(&style_format, &cols, "alignment").and_then(|a| a.parse::<u32>().ok());
                // v4+ numpad alignment: 7/8/9 are the top row.
                if let (Some(name), Some(7..=9)) = (name, align) {
                    top_styles.push(name.trim_start_matches('*').to_string());
                }
            }
            (Section::Events, "format") => event_format = format_columns(value),
            (Section::Events, "dialogue") => {
                // Text is the last column and may itself contain commas.
                let n = event_format.len().max(1);
                let cols: Vec<&str> = value.splitn(n, ',').collect();
                let (Some(start), Some(end), Some(text)) = (
                    column(&event_format, &cols, "start").and_then(ass_time_ms),
                    column(&event_format, &cols, "end").and_then(ass_time_ms),
                    column(&event_format, &cols, "text"),
                ) else {
                    continue;
                };
                if end <= start {
                    continue;
                }
                let Some((text, an_top)) = ass_text(text) else {
                    continue;
                };
                let style = column(&event_format, &cols, "style")
                    .unwrap_or("")
                    .trim()
                    .trim_start_matches('*');
                let top = an_top.unwrap_or_else(|| top_styles.iter().any(|s| s == style));
                cues.push((start, end, top, text));
            }
            _ => {}
        }
    }

    // ASS events are not required to be in time order; VTT cues are.
    cues.sort_by_key(|c| (c.0, c.1));
    let mut out = String::from("WEBVTT\n\n");
    for (start, end, top, text) in cues {
        out.push_str(&vtt_time(start));
        out.push_str(" --> ");
        out.push_str(&vtt_time(end));
        if top {
            out.push_str(" line:5%");
        }
        out.push('\n');
        out.push_str(&text);
        out.push_str("\n\n");
    }
    out
}

fn format_columns(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|c| c.trim().to_ascii_lowercase())
        .collect()
}

fn column<'a>(format: &[String], cols: &[&'a str], name: &str) -> Option<&'a str> {
    let i = format.iter().position(|c| c == name)?;
    cols.get(i).copied()
}

/// `H:MM:SS.cc` (centiseconds) to milliseconds.
fn ass_time_ms(ts: &str) -> Option<u64> {
    let mut parts = ts.trim().split(':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let (s, frac) = parts.next()?.split_once('.').unwrap_or(("0", "0"));
    let s: u64 = s.parse().ok()?;
    // Pad/truncate the fraction to milliseconds ("52" -> 520, "5" -> 500).
    let frac: String = frac.chars().chain("000".chars()).take(3).collect();
    let ms: u64 = frac.parse().ok()?;
    Some(((h * 60 + m) * 60 + s) * 1000 + ms)
}

fn vtt_time(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3_600_000,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}

/// Plain cue text for one ASS event, plus whether an inline `\an` tag pinned it
/// to the top (`None` = no inline tag, defer to the style). Returns `None` for
/// events with nothing to show: drawings and tag-only lines.
fn ass_text(text: &str) -> Option<(String, Option<bool>)> {
    let mut out = String::new();
    let mut an_top = None;
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            // Unterminated block: treat the remainder as a tag and drop it.
            rest = "";
            break;
        };
        let block = &rest[open + 1..open + close];
        for tag in block.split('\\') {
            if let Some(n) = tag.strip_prefix("an") {
                if let Ok(n) = n.trim().parse::<u32>() {
                    an_top = Some((7..=9).contains(&n));
                }
            }
            // \p1.. switches to vector drawing; the "text" is path commands.
            if let Some(n) = tag.strip_prefix('p') {
                if n.trim().parse::<u32>().is_ok_and(|n| n > 0) {
                    return None;
                }
            }
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);

    let out = out
        .replace("\\N", "\n")
        .replace("\\n", "\n")
        .replace("\\h", " ")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    // A blank line would terminate the cue early, so drop empty lines.
    let lines: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some((lines.join("\n"), an_top))
}

/// Whether [`to_vtt`] has real work to do for this format (by extension).
pub fn is_convertible(format_hint: &str) -> bool {
    matches!(
        format_hint.to_ascii_lowercase().trim_start_matches('.'),
        "srt" | "ass" | "ssa"
    )
}

/// File extension of a URL's path (query/fragment ignored), lowercased.
pub fn url_extension(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let file = path.rsplit('/').next()?;
    let (_, ext) = file.rsplit_once('.')?;
    Some(ext.to_ascii_lowercase())
}

/// Convert a subtitle file to VTT based on a format hint (usually the file
/// extension). Unknown formats get a clear error rather than a silently
/// broken track.
pub fn to_vtt(content: &str, format_hint: &str) -> Result<String> {
    match format_hint.to_ascii_lowercase().trim_start_matches('.') {
        "vtt" => Ok(content.to_string()),
        "srt" => Ok(srt_to_vtt(content)),
        "ass" | "ssa" => Ok(ass_to_vtt(content)),
        other => Err(Error::Subtitle(format!(
            "unsupported subtitle format: {other} (srt, ass, ssa and vtt are supported)"
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
        assert!(to_vtt("x", "ass").unwrap().starts_with("WEBVTT"));
        assert!(to_vtt("x", "sub").is_err());
    }

    const ASS: &str = r"[Script Info]
Title: English (US)

[V4+ Styles]
Format: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,OutlineColour,BackColour,Bold,Italic,Underline,Strikeout,ScaleX,ScaleY,Spacing,Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding
Style: Default,Trebuchet MS,24,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,2,1,2,0010,0010,0018,0
Style: Signs,Arial,24,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,-1,0,0,0,100,100,0,0,1,0,0,8,0030,0030,0010,0

[Events]
Format: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text
Dialogue: 0,0:01:05.00,0:01:07.50,Default,FRIEREN,0000,0000,0000,,Wait, what? {\i1}Really{\i0}, now?
Dialogue: 0,0:00:00.52,0:00:03.02,Default,OP,0000,0000,0000,,Like a fairy tale\NIt's a sign
Dialogue: 0,0:00:20.24,0:00:29.21,Signs,SIGN,0000,0000,0000,,{\fad(1,473)\pos(322,230)\c&HFFFCFE&}Beyond Journey's End
Comment: 0,0:00:30.00,0:00:31.00,Default,,0,0,0,,translator note
Dialogue: 0,0:00:40.00,0:00:41.00,Default,,0,0,0,,{\p1}m 0 0 l 100 0 100 100{\p0}
Dialogue: 0,0:00:42.00,0:00:43.00,Default,,0,0,0,,{\an8}Up top <3 & more
Dialogue: 0,0:00:44.00,0:00:45.00,Signs,,0,0,0,,{\an2}Forced down
Dialogue: 0,0:00:46.00,0:00:46.00,Default,,0,0,0,,zero length
";

    #[test]
    fn ass_dialogue_becomes_time_ordered_cues() {
        let vtt = ass_to_vtt(ASS);
        assert!(vtt.starts_with("WEBVTT\n\n"));
        // Centiseconds widen to milliseconds, and the 0:00:00 event that came
        // second in the file is emitted first.
        let first = vtt.find("00:00:00.520 --> 00:00:03.020").unwrap();
        let later = vtt.find("00:01:05.000 --> 00:01:07.500").unwrap();
        assert!(first < later);
        // \N is a line break; a comma inside the text column survives.
        assert!(vtt.contains("Like a fairy tale\nIt's a sign\n"));
        assert!(vtt.contains("Wait, what? Really, now?"));
    }

    #[test]
    fn ass_handles_crlf_and_a_bom() {
        let crlf = format!("\u{feff}{}", ASS.replace('\n', "\r\n"));
        assert_eq!(ass_to_vtt(&crlf), ass_to_vtt(ASS));
    }

    #[test]
    fn ass_strips_overrides_and_skips_non_dialogue() {
        let vtt = ass_to_vtt(ASS);
        assert!(vtt.contains("Beyond Journey's End"));
        assert!(!vtt.contains('{') && !vtt.contains("pos("));
        // Comment events, vector drawings and zero-length events are dropped.
        assert!(!vtt.contains("translator note"));
        assert!(!vtt.contains("m 0 0 l"));
        assert!(!vtt.contains("zero length"));
        // Characters that would open a VTT tag are escaped.
        assert!(vtt.contains("Up top &lt;3 &amp; more"));
    }

    #[test]
    fn ass_top_alignment_survives_from_style_or_inline_tag() {
        let vtt = ass_to_vtt(ASS);
        // Style "Signs" has Alignment 8.
        assert!(vtt.contains("00:00:20.240 --> 00:00:29.210 line:5%\nBeyond"));
        // Inline \an8 on a bottom-aligned style.
        assert!(vtt.contains("00:00:42.000 --> 00:00:43.000 line:5%\nUp top"));
        // Inline \an2 overrides a top-aligned style.
        assert!(vtt.contains("00:00:44.000 --> 00:00:45.000\nForced down"));
        assert!(vtt.contains("00:01:05.000 --> 00:01:07.500\nWait"));
    }

    #[test]
    fn ass_reads_columns_from_the_format_line() {
        // SSA v4 orders the event columns differently (Marked, not Layer) and
        // a file may reorder them outright.
        let ssa = "[Events]\nFormat: Marked, End, Start, Style, Text\nDialogue: Marked=0,0:00:02.00,0:00:01.00,Default,Hi, there\n";
        let vtt = ass_to_vtt(ssa);
        assert!(
            vtt.contains("00:00:01.000 --> 00:00:02.000\nHi, there"),
            "{vtt}"
        );
    }

    #[test]
    fn ass_garbage_yields_an_empty_track_not_a_panic() {
        assert_eq!(ass_to_vtt("not a subtitle file"), "WEBVTT\n\n");
        assert_eq!(ass_to_vtt("[Events]\nDialogue: 0,bad,worse"), "WEBVTT\n\n");
        let unterminated =
            "[Events]\nFormat: Start,End,Text\nDialogue: 0:00:01.00,0:00:02.00,{\\pos(1";
        assert_eq!(ass_to_vtt(unterminated), "WEBVTT\n\n");
    }

    #[test]
    fn url_extension_ignores_query_and_dotted_hosts() {
        assert_eq!(
            url_extension("https://a.b/x/3_en.ASS?t=1").as_deref(),
            Some("ass")
        );
        assert_eq!(url_extension("https://cdn.example.com/video"), None);
        assert!(is_convertible("ass") && is_convertible(".srt") && !is_convertible("vtt"));
    }
}
