//! Clipboard text cleanup before it is pasted.

const WHITESPACE: [char; 6] = ['\t', '\n', '\u{b}', '\u{c}', '\r', ' '];
const LINE_BREAKS: [char; 4] = ['\n', '\u{b}', '\u{c}', '\r'];

/// Trims trailing whitespace from a single-line paste so a copied line with its newline does not
/// run immediately; multi-line text is kept as is, and whitespace-only text becomes empty.
/// Same rule as Windows Terminal's `trimPaste` (`Utils::TrimPaste`, src/types/utils.cpp, MIT),
/// applied only without bracketed paste, as there.
pub(crate) fn trim_single_line(text: &str) -> &str {
    let Some(last_non_space) = text.rfind(|c: char| !WHITESPACE.contains(&c)) else { return "" };
    let is_multiline = text.find(LINE_BREAKS).is_some_and(|first_break| first_break < last_non_space);
    if is_multiline {
        return text;
    }
    let end = last_non_space + text[last_non_space..].chars().next().map_or(0, char::len_utf8);
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::trim_single_line;

    #[test]
    fn trims_a_single_line_with_its_newline() {
        assert_eq!(trim_single_line("echo hi\r\n"), "echo hi");
        assert_eq!(trim_single_line("echo hi  \n\n"), "echo hi");
        assert_eq!(trim_single_line("  leading kept\n"), "  leading kept");
        assert_eq!(trim_single_line("日本語\r\n"), "日本語");
    }

    #[test]
    fn keeps_multiline_text_and_drops_whitespace_only() {
        assert_eq!(trim_single_line("a\nb\n"), "a\nb\n");
        assert_eq!(trim_single_line(" \r\n\t"), "");
        assert_eq!(trim_single_line("no newline"), "no newline");
    }
}
