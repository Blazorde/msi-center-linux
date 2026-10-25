//! Text previews.

/// Single-line preview of `text`: whitespace runs (including newlines) and control characters become a
/// single space, the result is trimmed and cut to at most `max_chars` characters. When cut, the last
/// character is replaced by `…` so the preview never exceeds `max_chars`. Always cuts on a `char` boundary.
#[must_use]
pub fn text_preview(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut count = 0usize;
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() || c.is_control() {
            pending_space = count > 0;
            continue;
        }
        let needed = if pending_space { 2 } else { 1 };
        if count + needed > max_chars {
            // Truncate: make room for the ellipsis.
            if count >= max_chars {
                out.pop();
            }
            while out.ends_with(' ') {
                out.pop();
            }
            out.push('…');
            return out;
        }
        if pending_space {
            out.push(' ');
            count += 1;
            pending_space = false;
        }
        out.push(c);
        count += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn short_text_is_kept() {
        assert_eq!(text_preview("hello", 10), "hello");
    }

    #[test]
    fn whitespace_is_collapsed_and_trimmed() {
        assert_eq!(text_preview("  a\n\n b\t\u{0}c  ", 10), "a b c");
    }

    #[test]
    fn long_text_is_cut_with_ellipsis() {
        assert_eq!(text_preview("abcdefghij", 5), "abcd…");
        assert_eq!(text_preview("abcde", 5), "abcde");
        assert_eq!(text_preview("abcdef", 5), "abcd…");
    }

    #[test]
    fn cut_respects_char_boundaries() {
        let s = "éééééééé";
        let p = text_preview(s, 4);
        assert_eq!(p, "ééé…");
        let emoji = "👍👍👍👍👍";
        assert_eq!(text_preview(emoji, 3), "👍👍…");
    }

    #[test]
    fn trailing_space_before_cut_is_not_kept() {
        assert_eq!(text_preview("abcd efgh", 5), "abcd…");
    }

    #[test]
    fn zero_budget() {
        assert_eq!(text_preview("abc", 0), "");
    }

    proptest! {
        #[test]
        fn preview_bounded_and_single_line(text in "\\PC{0,300}|[\\s\\S]{0,300}", max in 0usize..250) {
            let p = text_preview(&text, max);
            prop_assert!(p.chars().count() <= max);
            prop_assert!(!p.contains('\n'));
            prop_assert_eq!(p.trim(), p.as_str());
            prop_assert!(!p.contains("  "));
        }
    }
}
