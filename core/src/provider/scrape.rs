//! Small text-scraping helpers shared by the HTML-backed sources.
//!
//! The sources here scrape server-rendered pages rather than a JSON API, and
//! the handful of things they need (cut a marked region out of a page, read an
//! attribute, undo HTML/JS escaping) are the same everywhere. They are plain
//! string scans on purpose: the pages are small, the markers are stable
//! anchors, and a full HTML parser would be a heavy dependency to carry on
//! every platform for what is a dozen `find`s.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The text between the first `start` marker and the next `end` after it.
pub fn between<'a>(haystack: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = haystack.find(start)? + start.len();
    let len = haystack[from..].find(end)?;
    Some(&haystack[from..from + len])
}

/// Value of the first `name="…"` / `name='…'` attribute in `tag`.
pub fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut search = tag;
    loop {
        let at = search.find(name)?;
        let after = &search[at + name.len()..];
        // Must be a whole attribute name: preceded by whitespace (or the start)
        // and followed by `=`, so `id` doesn't match inside `data-id`.
        let boundary = at == 0
            || search[..at]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if boundary {
            if let Some(rest) = after.strip_prefix('=') {
                let quote = rest.chars().next()?;
                if quote == '"' || quote == '\'' {
                    let value = &rest[1..];
                    return value.find(quote).map(|end| &value[..end]);
                }
            }
        }
        search = after;
    }
}

/// Undo the HTML entities these pages actually use. Unknown entities are left
/// as written rather than guessed at.
pub fn unescape_html(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let decoded = tail
            .find(';')
            .filter(|&semi| semi <= 10)
            .and_then(|semi| decode_entity(&tail[1..semi]).map(|c| (c, semi + 1)));
        match decoded {
            Some((c, consumed)) => {
                out.push(c);
                rest = &tail[consumed..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "nbsp" => Some(' '),
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Read a JavaScript string literal body starting just *after* its opening
/// quote, decoding escapes, up to the matching unescaped `quote`.
///
/// Pages that inline data as `JSON.parse('…')` escape the JSON a second time
/// for the JS string (`\u0022` for `"`, `\\\/` for `/`), so this has to be
/// undone before the JSON parser sees it. Returns `None` when the literal is
/// unterminated.
pub fn js_string_literal(s: &str, quote: char) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == quote {
            return Some(out);
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'x' => out.push(char::from_u32(hex(&mut chars, 2)?)?),
            'u' => {
                let hi = hex(&mut chars, 4)?;
                if (0xD800..0xDC00).contains(&hi) {
                    // Surrogate pair: must be followed by `\uDC00..DFFF`.
                    let mut look = chars.clone();
                    let lo = (look.next() == Some('\\') && look.next() == Some('u'))
                        .then(|| hex(&mut look, 4))
                        .flatten()
                        .filter(|lo| (0xDC00..0xE000).contains(lo));
                    match lo {
                        Some(lo) => {
                            chars = look;
                            let code = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                            out.push(char::from_u32(code)?);
                        }
                        None => out.push(char::REPLACEMENT_CHARACTER),
                    }
                } else {
                    out.push(char::from_u32(hi).unwrap_or(char::REPLACEMENT_CHARACTER));
                }
            }
            // `\\`, `\'`, `\"`, `\/` and any other escaped character stand for
            // themselves.
            other => out.push(other),
        }
    }
    None
}

fn hex(chars: &mut std::str::Chars<'_>, digits: usize) -> Option<u32> {
    let mut value = 0;
    for _ in 0..digits {
        value = value * 16 + chars.next()?.to_digit(16)?;
    }
    Some(value)
}

/// Ordering for episode labels: numeric ("2" before "10", "5.5" between 5 and
/// 6), with anything non-numeric after the numbers. Use with a stable sort so
/// non-numeric labels keep their original order.
pub fn episode_order(a: &str, b: &str) -> Ordering {
    match (a.parse::<f64>(), b.parse::<f64>()) {
        (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
        (Ok(_), Err(_)) => Ordering::Less,
        (Err(_), Ok(_)) => Ordering::Greater,
        (Err(_), Err(_)) => Ordering::Equal,
    }
}

/// A tiny time-bounded cache for per-show page results (episode lists), so
/// opening a show and then pressing play doesn't scrape the same page twice.
pub struct TtlCache<V> {
    ttl: Duration,
    entries: Mutex<HashMap<String, (Instant, V)>>,
}

impl<V: Clone> TtlCache<V> {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, key: &str) -> Option<V> {
        let entries = self.entries.lock().ok()?;
        entries
            .get(key)
            .filter(|(at, _)| at.elapsed() < self.ttl)
            .map(|(_, v)| v.clone())
    }

    pub fn put(&self, key: &str, value: V) {
        if let Ok(mut entries) = self.entries.lock() {
            // Bounded by what a session browses; drop expired rows as we go so
            // it can't grow without limit.
            let ttl = self.ttl;
            entries.retain(|_, (at, _)| at.elapsed() < ttl);
            entries.insert(key.to_string(), (Instant::now(), value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn between_cuts_the_first_marked_region() {
        assert_eq!(between("a [x] b [y]", "[", "]"), Some("x"));
        assert_eq!(between("no markers", "[", "]"), None);
        assert_eq!(between("open [ only", "[", "]"), None);
    }

    #[test]
    fn attr_reads_either_quote_style_and_whole_names_only() {
        let tag = r##"<a href="#x" data-id='131519' data-version="subbed" id="real">"##;
        assert_eq!(attr(tag, "data-id"), Some("131519"));
        assert_eq!(attr(tag, "data-version"), Some("subbed"));
        // `id` must not match the tail of `data-id`.
        assert_eq!(attr(tag, "id"), Some("real"));
        assert_eq!(attr(tag, "missing"), None);
    }

    #[test]
    fn unescape_html_handles_named_numeric_and_unknown_entities() {
        assert_eq!(
            unescape_html("Journey&#039;s &amp; &quot;End&quot; &#x26; &lt;3"),
            "Journey's & \"End\" & <3"
        );
        // Unknown or unterminated entities are left alone, not mangled.
        assert_eq!(unescape_html("R&D &bogus; a & b"), "R&D &bogus; a & b");
    }

    #[test]
    fn js_string_literal_undoes_double_escaped_json() {
        let src = r#"[{\u0022url\u0022:\u0022http:\\\/\\\/x.to\\\/a\u0022,\u0022t\u0022:\u0022it\u0027s \\u846c\u0022}]') trailing"#;
        let json = js_string_literal(src, '\'').unwrap();
        assert_eq!(json, r#"[{"url":"http:\/\/x.to\/a","t":"it's \u846c"}]"#);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v[0]["url"], "http://x.to/a");
        assert_eq!(v[0]["t"], "it's 葬");
    }

    #[test]
    fn js_string_literal_stops_at_the_unescaped_quote_only() {
        assert_eq!(js_string_literal(r"a\'b'c", '\'').as_deref(), Some("a'b"));
        assert_eq!(js_string_literal("never closed", '\''), None);
        // Surrogate pair, and a lone surrogate that must not panic.
        assert_eq!(
            js_string_literal(r"\ud83d\ude00'", '\'').as_deref(),
            Some("😀")
        );
        assert_eq!(
            js_string_literal(r"\ud83dx'", '\'').as_deref(),
            Some("\u{fffd}x")
        );
    }

    #[test]
    fn episode_order_is_numeric_with_specials_last() {
        let mut eps = ["10", "2", "SP1", "5.5", "OVA", "1"];
        eps.sort_by(|a, b| episode_order(a, b));
        assert_eq!(eps, ["1", "2", "5.5", "10", "SP1", "OVA"]);
    }

    #[test]
    fn ttl_cache_expires() {
        let cache = TtlCache::new(Duration::from_millis(30));
        cache.put("a", 1);
        assert_eq!(cache.get("a"), Some(1));
        assert_eq!(cache.get("b"), None);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(cache.get("a"), None);
    }
}
