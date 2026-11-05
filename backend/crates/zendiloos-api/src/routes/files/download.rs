//! What a download answers: the part of the file asked for (`Range`), and
//! whether the browser may show it (a preview) or must save it.

/// The part of a file of `size` bytes to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Part {
    Whole,
    /// From `start` to `end`, both included.
    Bytes {
        start: u64,
        end: u64,
    },
    /// Nothing of the file is in the range asked for.
    Unsatisfiable,
}

/// Reads a `Range` header of one byte range (RFC 9110, 14.1.2). Several
/// ranges, other units or a malformed header get the whole file, as the
/// standard allows; a range that starts beyond the end gets nothing.
pub(super) fn part(range: Option<&str>, size: u64) -> Part {
    let Some(spec) = range.and_then(|range| range.trim().strip_prefix("bytes=")) else { return Part::Whole };
    if spec.contains(',') {
        return Part::Whole;
    }
    let Some((first, last)) = spec.split_once('-') else { return Part::Whole };
    let number = |text: &str| -> Option<u64> {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    };
    let (first, last) = (first.trim(), last.trim());
    if first.is_empty() {
        // The last `n` bytes.
        return match number(last) {
            None => Part::Whole,
            Some(0) => Part::Unsatisfiable,
            Some(_) if size == 0 => Part::Unsatisfiable,
            Some(suffix) => Part::Bytes { start: size.saturating_sub(suffix), end: size - 1 },
        };
    }
    let Some(start) = number(first) else { return Part::Whole };
    let end = match last {
        "" => None,
        last => match number(last) {
            Some(end) => Some(end),
            None => return Part::Whole,
        },
    };
    if start >= size {
        return Part::Unsatisfiable;
    }
    match end {
        Some(end) if end < start => Part::Whole,
        Some(end) => Part::Bytes { start, end: end.min(size - 1) },
        None => Part::Bytes { start, end: size - 1 },
    }
}

/// The media type a browser may show inline, for the few kinds that
/// cannot run code: pictures, sound, video and plain text. Everything
/// else (HTML, SVG, PDF, …) is only ever saved.
pub(super) fn preview_type(name: &str) -> Option<&'static str> {
    let (stem, extension) = name.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    Some(match extension.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "txt" | "log" | "md" | "csv" => "text/plain; charset=utf-8",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_range_of_bytes_is_sent_as_asked() {
        assert_eq!(part(None, 100), Part::Whole);
        assert_eq!(part(Some("bytes=0-9"), 100), Part::Bytes { start: 0, end: 9 });
        assert_eq!(part(Some("bytes=90-"), 100), Part::Bytes { start: 90, end: 99 });
        assert_eq!(part(Some("bytes=90-500"), 100), Part::Bytes { start: 90, end: 99 }, "cut at the end");
        assert_eq!(part(Some("bytes=-10"), 100), Part::Bytes { start: 90, end: 99 });
        assert_eq!(part(Some("bytes=-500"), 100), Part::Bytes { start: 0, end: 99 });
    }

    #[test]
    fn a_range_beyond_the_end_gets_nothing() {
        assert_eq!(part(Some("bytes=100-"), 100), Part::Unsatisfiable);
        assert_eq!(part(Some("bytes=-0"), 100), Part::Unsatisfiable);
        assert_eq!(part(Some("bytes=0-"), 0), Part::Unsatisfiable);
    }

    #[test]
    fn anything_else_gets_the_whole_file() {
        for odd in
            ["items=0-9", "bytes=0-9,20-29", "bytes=9-0", "bytes=a-b", "bytes=", "bytes=-", "bytes=+1-2"]
        {
            assert_eq!(part(Some(odd), 100), Part::Whole, "{odd}");
        }
    }

    #[test]
    fn only_harmless_kinds_are_shown_in_the_browser() {
        assert_eq!(preview_type("Beach.JPG"), Some("image/jpeg"));
        assert_eq!(preview_type("film.mp4"), Some("video/mp4"));
        assert_eq!(preview_type("notes.txt"), Some("text/plain; charset=utf-8"));
        for saved in ["page.html", "logo.svg", "paper.pdf", "script.js", "README", ".png"] {
            assert_eq!(preview_type(saved), None, "{saved}");
        }
    }
}
