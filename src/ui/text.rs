//! Text measuring, wrapping and formatting.

use std::borrow::Cow;

use mongodb::bson::{Bson, Document};
use ratatui::text::{Line, Span};

/// Display width of `s`, in terminal cells.
pub fn width(s: &str) -> usize {
    Span::raw(s).width()
}

/// Display width of `c`, in terminal cells.
pub fn char_width(c: char) -> usize {
    width(c.encode_utf8(&mut [0; 4]))
}

/// `s` with control characters (tabs, line breaks, escapes, ...) replaced by
/// spaces, so it renders on a single line with predictable widths.
pub fn sanitize(s: &str) -> Cow<'_, str> {
    if s.chars().any(char::is_control) {
        Cow::Owned(
            s.chars()
                .map(|c| if c.is_control() { ' ' } else { c })
                .collect(),
        )
    } else {
        Cow::Borrowed(s)
    }
}

/// Owned version of [`sanitize`], reusing `s` when it is clean.
pub fn sanitize_owned(s: String) -> String {
    match sanitize(&s) {
        Cow::Borrowed(_) => s,
        Cow::Owned(clean) => clean,
    }
}

/// `s` cut to at most `max` cells, ending with `…` when shortened.
pub fn truncate(s: &str, max: usize) -> Cow<'_, str> {
    if width(s) <= max {
        return Cow::Borrowed(s);
    }
    if max == 0 {
        return Cow::Borrowed("");
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = char_width(c);
        if used + w > max - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    Cow::Owned(out)
}

/// Splits styled text into lines at most `max` cells wide, breaking between
/// any two characters and keeping the styles. Empty text gives one empty
/// line.
pub fn wrap_chars(spans: &[Span<'_>], max: usize) -> Vec<Line<'static>> {
    let max = max.max(1);
    let mut lines = Vec::new();
    let mut line: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for span in spans {
        let mut piece = String::new();
        for c in span.content.chars() {
            let w = char_width(c);
            if used + w > max && used > 0 {
                if !piece.is_empty() {
                    line.push(Span::styled(std::mem::take(&mut piece), span.style));
                }
                lines.push(Line::from(std::mem::take(&mut line)));
                used = 0;
            }
            piece.push(c);
            used += w;
        }
        if !piece.is_empty() {
            line.push(Span::styled(piece, span.style));
        }
    }
    lines.push(Line::from(line));
    lines
}

/// Number of lines [`wrap_chars`] splits `text` into once [`sanitize`]d,
/// without building them.
pub fn wrapped_rows(text: &str, max: usize) -> usize {
    let max = max.max(1);
    if text.is_ascii() {
        return text.len().div_ceil(max).max(1);
    }
    let mut rows = 1;
    let mut used = 0;
    for c in text.chars() {
        let w = if c.is_control() { 1 } else { char_width(c) };
        if used + w > max && used > 0 {
            rows += 1;
            used = 0;
        }
        used += w;
    }
    rows
}

/// Splits `text` into lines at most `max` cells wide, breaking at spaces when
/// possible and inside words longer than a line. Whitespace runs collapse to
/// one space. Empty text gives one empty line.
pub fn wrap_words(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut used = 0;
    for word in text.split_whitespace() {
        let w = width(word);
        if used > 0 && used + 1 + w <= max {
            line.push(' ');
            line.push_str(word);
            used += 1 + w;
            continue;
        }
        if used > 0 {
            lines.push(std::mem::take(&mut line));
            used = 0;
        }
        for c in word.chars() {
            let cw = char_width(c);
            if used + cw > max && used > 0 {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push(c);
            used += cw;
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

/// `doc` as pretty-printed relaxed extended JSON.
pub fn pretty_json(doc: &Document) -> String {
    let value = Bson::Document(doc.clone()).into_relaxed_extjson();
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
}

/// `n` as a terminal coordinate, saturating.
pub fn to_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use mongodb::bson::{DateTime, doc};
    use ratatui::style::{Color, Style};

    use super::*;

    #[test]
    fn widths() {
        assert_eq!(width("abc"), 3);
        assert_eq!(width("✓▲▼≥…"), 5);
        assert_eq!(width("日本"), 4);
        assert_eq!(char_width('日'), 2);
        assert_eq!(char_width('a'), 1);
    }

    #[test]
    fn sanitize_replaces_control_characters() {
        assert!(matches!(sanitize("plain"), Cow::Borrowed("plain")));
        assert_eq!(sanitize("a\tb\nc\u{1b}"), "a b c ");
        assert_eq!(sanitize_owned("x\ry".to_owned()), "x y");
    }

    #[test]
    fn truncate_adds_ellipsis() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 4), "hel…");
        assert_eq!(truncate("hello", 1), "…");
        assert_eq!(truncate("hello", 0), "");
        assert_eq!(truncate("日本語", 4), "日…");
    }

    #[test]
    fn wrapped_rows_matches_wrap_chars() {
        let samples = [
            "",
            "a",
            "abcdef",
            "abcdefg",
            "tab\tand\u{1b}escape",
            "日本語のテキスト",
            "e\u{301}e\u{301}e\u{301}e\u{301}",
            "mixed 日本 text with ✓ and ▲",
            "\u{85}next line",
        ];
        for text in samples {
            for max in 0..12 {
                let wrapped = wrap_chars(&[Span::raw(sanitize(text))], max).len();
                assert_eq!(wrapped_rows(text, max), wrapped, "{text:?} at {max}");
            }
        }
    }

    #[test]
    fn wrap_chars_breaks_anywhere_and_keeps_styles() {
        let red = Style::new().fg(Color::Red);
        let lines = wrap_chars(&[Span::raw("abc"), Span::styled("defg", red)], 3);
        let text: Vec<String> = lines.iter().map(ToString::to_string).collect();
        assert_eq!(text, ["abc", "def", "g"]);
        assert_eq!(lines[1].spans[0].style, red);
        assert_eq!(lines[2].spans[0].style, red);
        assert_eq!(wrap_chars(&[Span::raw("")], 5).len(), 1);
        assert_eq!(wrap_chars(&[Span::raw("abc")], 0).len(), 3);
    }

    #[test]
    fn wrap_words_prefers_spaces() {
        assert_eq!(
            wrap_words("the quick brown fox", 10),
            ["the quick", "brown fox"]
        );
        assert_eq!(wrap_words("abcdefghij kl", 4), ["abcd", "efgh", "ij", "kl"]);
        assert_eq!(wrap_words("  spaced   out  ", 20), ["spaced out"]);
        assert_eq!(wrap_words("", 5), [""]);
    }

    #[test]
    fn pretty_json_uses_relaxed_extended_json() {
        let doc = doc! {
            "find": "users",
            "limit": 5_i64,
            "at": DateTime::from_millis(0),
        };
        let json = pretty_json(&doc);
        assert!(json.contains("\"find\": \"users\""), "{json}");
        assert!(json.contains("\"limit\": 5"), "{json}");
        assert!(
            json.contains("\"$date\": \"1970-01-01T00:00:00Z\""),
            "{json}"
        );
        assert!(json.lines().count() > 3);
    }

    #[test]
    fn to_u16_saturates() {
        assert_eq!(to_u16(7), 7);
        assert_eq!(to_u16(usize::MAX), u16::MAX);
    }
}
