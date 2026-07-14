//! HTTP `Range` header parsing for the media server.
//!
//! macOS AVFoundation (native `<video>` for MP4 and native HLS in WKWebView)
//! opens every media resource with a byte-range probe (`Range: bytes=0-1`) and
//! stalls or errors if it receives a `200` full body instead of a `206
//! Partial Content` with `Content-Range`. For content we generate in-memory
//! (rewritten HLS playlists) we must satisfy those ranges ourselves; for
//! upstream media we forward the header and relay the upstream `206`. This
//! module handles the former.

/// A resolved byte range over content of a known total length. `end` is
/// inclusive, matching HTTP semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
    pub total: u64,
}

impl ByteRange {
    /// Length of the range (inclusive), in bytes.
    pub fn len(&self) -> u64 {
        self.end - self.start + 1
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `Content-Range` header value for this range.
    pub fn content_range(&self) -> String {
        format!("bytes {}-{}/{}", self.start, self.end, self.total)
    }
}

/// Parse a single-range `Range` header value against a known `total` length.
///
/// Supports the three forms HTML media elements emit:
/// - `bytes=START-END`   (closed range; END clamped to total-1)
/// - `bytes=START-`      (open-ended; to end of content)
/// - `bytes=-SUFFIX`     (last SUFFIX bytes)
///
/// Returns `Ok(Some(range))` for a satisfiable range, `Ok(None)` when there is
/// no (or a malformed / multi-range) header — caller should serve the full
/// body — and `Err(())` when the range is syntactically valid but not
/// satisfiable (caller should return `416`).
pub fn parse_range(header: &str, total: u64) -> std::result::Result<Option<ByteRange>, ()> {
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return Ok(None);
    };
    // Multi-range (comma-separated) is not supported by media elements; bail
    // to a full-body response rather than mishandle it.
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((start_s, end_s)) = spec.split_once('-') else {
        return Ok(None);
    };
    let start_s = start_s.trim();
    let end_s = end_s.trim();

    if total == 0 {
        return Err(());
    }
    let last = total - 1;

    let range = match (start_s.is_empty(), end_s.is_empty()) {
        // "bytes=-SUFFIX": last SUFFIX bytes.
        (true, false) => {
            let suffix: u64 = end_s.parse().map_err(|_| ())?;
            if suffix == 0 {
                return Err(());
            }
            let start = total.saturating_sub(suffix);
            ByteRange { start, end: last, total }
        }
        // "bytes=START-": from START to the end.
        (false, true) => {
            let start: u64 = start_s.parse().map_err(|_| ())?;
            if start > last {
                return Err(());
            }
            ByteRange { start, end: last, total }
        }
        // "bytes=START-END": closed range, END clamped.
        (false, false) => {
            let start: u64 = start_s.parse().map_err(|_| ())?;
            let mut end: u64 = end_s.parse().map_err(|_| ())?;
            if start > end || start > last {
                return Err(());
            }
            if end > last {
                end = last;
            }
            ByteRange { start, end, total }
        }
        // "bytes=-" is malformed.
        (true, true) => return Ok(None),
    };
    Ok(Some(range))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_header_prefix_serves_full() {
        assert_eq!(parse_range("items=0-1", 100).unwrap(), None);
        assert_eq!(parse_range("", 100).unwrap(), None);
    }

    #[test]
    fn closed_range() {
        let r = parse_range("bytes=0-99", 1000).unwrap().unwrap();
        assert_eq!((r.start, r.end, r.total), (0, 99, 1000));
        assert_eq!(r.len(), 100);
        assert_eq!(r.content_range(), "bytes 0-99/1000");
    }

    #[test]
    fn avfoundation_initial_probe() {
        // The exact probe macOS sends first.
        let r = parse_range("bytes=0-1", 5_000_000).unwrap().unwrap();
        assert_eq!((r.start, r.end), (0, 1));
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn open_ended_range() {
        let r = parse_range("bytes=500-", 1000).unwrap().unwrap();
        assert_eq!((r.start, r.end), (500, 999));
        assert_eq!(r.len(), 500);
    }

    #[test]
    fn suffix_range() {
        let r = parse_range("bytes=-200", 1000).unwrap().unwrap();
        assert_eq!((r.start, r.end), (800, 999));
        assert_eq!(r.len(), 200);
    }

    #[test]
    fn suffix_larger_than_content_clamps_to_start() {
        let r = parse_range("bytes=-5000", 1000).unwrap().unwrap();
        assert_eq!((r.start, r.end), (0, 999));
    }

    #[test]
    fn end_beyond_content_is_clamped() {
        let r = parse_range("bytes=900-100000", 1000).unwrap().unwrap();
        assert_eq!((r.start, r.end), (900, 999));
    }

    #[test]
    fn unsatisfiable_ranges_error() {
        assert!(parse_range("bytes=2000-3000", 1000).is_err()); // start past end
        assert!(parse_range("bytes=1000-", 1000).is_err()); // start == total
        assert!(parse_range("bytes=-0", 1000).is_err()); // zero-length suffix
        assert!(parse_range("bytes=0-0", 0).is_err()); // empty content
    }

    #[test]
    fn malformed_falls_back_to_full() {
        assert_eq!(parse_range("bytes=-", 1000).unwrap(), None);
        assert_eq!(parse_range("bytes=abc-def", 1000).unwrap_err(), ());
        assert_eq!(parse_range("bytes=0-99,200-299", 1000).unwrap(), None);
    }

    #[test]
    fn whitespace_tolerated() {
        let r = parse_range("bytes= 10 - 20 ", 1000).unwrap().unwrap();
        assert_eq!((r.start, r.end), (10, 20));
    }
}
