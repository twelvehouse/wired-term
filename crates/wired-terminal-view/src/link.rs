//! Links in the output: hyperlinks the program attached (OSC 8) and URLs found in the text.
//! Ctrl+hover underlines the link under the pointer; Ctrl+click opens it.

use std::ops::Range;

use gpui::{Context, PromptLevel, Window};
use wired_vt::{CellWidth, GridSnapshot};

use crate::{Notice, TerminalView};

/// A link and the cells it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Link {
    pub(crate) uri: String,
    /// Viewport row, first column, end column (exclusive), per row the link spans.
    pub(crate) segments: Vec<(u16, u16, u16)>,
}

/// What a Ctrl+click does with a link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinkAction {
    Open,
    /// Another scheme (`file:`, custom ones): only after the user confirmed the full link.
    Confirm,
    /// Not something to hand to the system.
    Refuse,
}

/// Longest link shown for confirmation; a longer one could not be read in the dialog.
const MAX_CONFIRMED_LEN: usize = 1024;

/// Characters that change how the rest of a text is shown without being seen themselves
/// (bidirectional controls, zero-width and other format characters), and spaces, which no link
/// contains. With them a link could look different from what would open.
fn is_deceptive(c: char) -> bool {
    c.is_control()
        || c.is_whitespace()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{061C}'
                | '\u{115F}'
                | '\u{1160}'
                | '\u{17B4}'
                | '\u{17B5}'
                | '\u{180B}'..='\u{180F}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{3164}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF0}'..='\u{FFFB}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E0FFF}'
        )
}

/// The untrusted program chooses the link: only web and mail open directly, other schemes need
/// confirmation, and a link that would not display as what opens is refused.
pub(crate) fn action_for(uri: &str) -> LinkAction {
    if uri.is_empty() || uri.chars().any(is_deceptive) {
        return LinkAction::Refuse;
    }
    let Some((scheme, _)) = uri.split_once(':') else { return LinkAction::Refuse };
    let is_scheme = scheme.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" | "mailto" => LinkAction::Open,
        _ if is_scheme && uri.chars().count() <= MAX_CONFIRMED_LEN => LinkAction::Confirm,
        _ => LinkAction::Refuse,
    }
}

/// How far around the pointer a line is searched, each way, so a very long line costs the same
/// as a short one. A URL reaching past it is not a link (it would open cut short).
const SEARCH_RADIUS: usize = 4 * 1024;

/// Ghostty's URL pattern (`src/config/url.zig`, scheme branch) for `http`/`https`: an IPv6 host
/// or URL characters, each run optionally ending in a bracketed word (`Rust_(video_game)`).
/// Its look-behind parts are checked on each match instead (see [`find_url`]), since this
/// engine matches in linear time and has no look-around.
static URL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)https?://(?:\[[:0-9a-fA-F]+(?:[:0-9a-fA-F]*)+\](?::[0-9]+)?|[\w\-.~:/?#@!$&*+,;=%]+(?:[(\[]\w*[)\]])?)+",
    )
    .expect("the URL pattern is valid")
});

/// Length of `url` without trailing punctuation (`.,:;!?`) and without closing brackets that
/// have no opening one inside it; one pass from the end.
fn trimmed_len(url: &str) -> usize {
    let open = url.matches(['(', '[']).count();
    let mut close = url.matches([')', ']']).count();
    let mut end = url.len();
    while let Some(c) = url[..end].chars().next_back() {
        match c {
            '.' | ',' | ':' | ';' | '!' | '?' => {}
            ')' | ']' if open < close => close -= 1,
            _ => break,
        }
        end -= c.len_utf8();
    }
    end
}

fn floor_boundary(text: &str, byte: usize) -> usize {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}

/// The `http`/`https` URL in `text` that covers byte `at`. A match running into the end of the
/// searched span may continue past it, so it is not a link.
pub(crate) fn find_url(text: &str, at: usize) -> Option<Range<usize>> {
    let from = floor_boundary(text, at.saturating_sub(SEARCH_RADIUS));
    let to = floor_boundary(text, at.saturating_add(SEARCH_RADIUS));
    let scope = &text[from..to];
    for found in URL.find_iter(scope) {
        let (start, end) = (from + found.start(), from + found.end());
        if start > at {
            break;
        }
        if end == to && to < text.len() {
            return None;
        }
        if text[..start].chars().next_back().is_some_and(char::is_alphanumeric) {
            continue;
        }
        let scheme = if found.as_str().get(..5).is_some_and(|s| s.eq_ignore_ascii_case("https")) { 8 } else { 7 };
        let end = start + trimmed_len(&text[start..end]);
        if end > start + scheme && (start..end).contains(&at) {
            return Some(start..end);
        }
    }
    None
}

/// Where a cell's text sits in a logical line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Place {
    byte: usize,
    row: u16,
    col: u16,
    cols: u16,
}

/// A logical line as far as the viewport shows it.
#[derive(Debug)]
struct Line {
    text: String,
    places: Vec<Place>,
    /// The line goes on above the viewport.
    is_cut_above: bool,
    /// The line goes on below the viewport.
    is_cut_below: bool,
}

/// The logical line through viewport row `row` (soft-wrapped rows joined) and where each cell's
/// text starts in it. Blank cells count as a space, so links end at gaps.
fn logical_line(snapshot: &GridSnapshot, row: usize) -> Line {
    let rows = &snapshot.rows;
    let mut first = row;
    while first > 0 && rows[first - 1].is_wrapped {
        first -= 1;
    }
    let mut last = row;
    while last + 1 < rows.len() && rows[last].is_wrapped {
        last += 1;
    }
    let mut text = String::new();
    let mut places = Vec::new();
    for (index, r) in rows.iter().enumerate().take(last + 1).skip(first) {
        for (col, cell) in r.cells.iter().enumerate() {
            if cell.width.is_spacer() {
                continue;
            }
            places.push(Place {
                byte: text.len(),
                row: u16::try_from(index).unwrap_or(u16::MAX),
                col: u16::try_from(col).unwrap_or(u16::MAX),
                cols: if cell.width == CellWidth::Wide { 2 } else { 1 },
            });
            text.push_str(if cell.text_len == 0 { " " } else { r.cell_text(cell) });
        }
    }
    Line { text, places, is_cut_above: rows[first].is_wrap_continuation, is_cut_below: rows[last].is_wrapped }
}

/// One segment per viewport row of consecutive `places`.
fn segments(places: &[Place]) -> Vec<(u16, u16, u16)> {
    let mut out: Vec<(u16, u16, u16)> = Vec::new();
    for p in places {
        match out.last_mut() {
            Some(last) if last.0 == p.row => last.2 = p.col + p.cols,
            _ => out.push((p.row, p.col, p.col + p.cols)),
        }
    }
    out
}

impl TerminalView {
    /// The program's hyperlink or a URL at viewport cell (`col`, `row`) as last drawn; none if the
    /// terminal changed since the draw or the URL is cut off by the viewport.
    pub(crate) fn link_at(&self, col: u16, row: u16) -> Option<Link> {
        let snapshot = self.snapshot.borrow();
        if usize::from(row) >= snapshot.rows.len() {
            return None;
        }
        let Line { text, places, is_cut_above, is_cut_below } = logical_line(&snapshot, usize::from(row));
        let at = places.iter().position(|p| p.row == row && (p.col..p.col + p.cols).contains(&col))?;
        let has_hyperlink = |p: &Place| snapshot.rows[usize::from(p.row)].cells[usize::from(p.col)].has_hyperlink;
        if has_hyperlink(&places[at]) {
            let vt = self.terminal.lock();
            if vt.generation() != snapshot.generation {
                return None;
            }
            let uri = vt.hyperlink_uri(places[at].col, places[at].row)?;
            let same = |p: &Place| has_hyperlink(p) && vt.hyperlink_uri(p.col, p.row).is_some_and(|other| other == uri);
            let mut first = at;
            while first > 0 && same(&places[first - 1]) {
                first -= 1;
            }
            let mut last = at;
            while last + 1 < places.len() && same(&places[last + 1]) {
                last += 1;
            }
            return Some(Link { segments: segments(&places[first..=last]), uri });
        }
        let range = find_url(&text, places[at].byte)?;
        if (is_cut_above && range.start == 0) || (is_cut_below && range.end == text.len()) {
            return None;
        }
        let first = places.iter().position(|p| p.byte >= range.start)?;
        let last = places.iter().rposition(|p| p.byte < range.end)?;
        Some(Link { uri: text[range].to_owned(), segments: segments(&places[first..=last]) })
    }

    /// Opens `uri` (a Ctrl+click): web and mail links at once, other schemes after the user
    /// confirmed the full link, nothing else.
    pub(crate) fn open_link(&mut self, uri: String, window: &mut Window, cx: &mut Context<Self>) {
        match action_for(&uri) {
            LinkAction::Open => self.open_now(&uri, cx),
            LinkAction::Confirm => {
                let answer = window.prompt(
                    PromptLevel::Warning,
                    "Open this link? It is not a web link and could open a file or start a program.",
                    Some(&uri),
                    &["Open", "Cancel"],
                    cx,
                );
                cx.spawn(async move |this, cx| {
                    if answer.await == Ok(0) {
                        let _ = this.update(cx, |view, cx| view.open_now(&uri, cx));
                    }
                })
                .detach();
            }
            LinkAction::Refuse => self.show_notices(&[Notice::error(format!("Not opened: {uri}"))], cx),
        }
    }

    fn open_now(&mut self, uri: &str, cx: &mut Context<Self>) {
        let uri = uri.to_owned();
        cx.spawn(async move |this, cx| {
            let opened = cx.background_executor().spawn({
                let uri = uri.clone();
                async move { wired_platform::open_url(&uri) }
            });
            if !opened.await {
                let _ =
                    this.update(cx, |view, cx| view.show_notices(&[Notice::error(format!("Cannot open {uri}"))], cx));
            }
        })
        .detach();
    }

    /// Ctrl over the grid at viewport cell `cell` underlines the link there; anything else removes
    /// it. Remembers which drawn screen it looked at, so a later frame can look again.
    pub(crate) fn hover_link(&mut self, cell: Option<(u16, u16)>, cx: &mut Context<Self>) {
        let link = cell.and_then(|(col, row)| self.link_at(col, row));
        let origin = self.frame.layout.get().map(|l| l.origin);
        self.frame.hover_checked.set(Some((self.snapshot.borrow().generation, origin)));
        if link != self.hovered_link {
            self.hovered_link = link;
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use wired_vt::{Vt, VtOptions};

    use super::*;

    fn url(text: &str, at: usize) -> Option<&str> {
        find_url(text, at).map(|r| &text[r])
    }

    #[test]
    fn finds_urls_without_trailing_punctuation() {
        let text = "(see https://example.com/a?b=c). Next";
        assert_eq!(url(text, 10), Some("https://example.com/a?b=c"));
        assert_eq!(url(text, 2), None);
        let wiki = "https://en.wikipedia.org/wiki/Rust_(video_game), ok";
        assert_eq!(url(wiki, 0), Some("https://en.wikipedia.org/wiki/Rust_(video_game)"));
        assert_eq!(url("HTTP://A.B/c!", 3), Some("HTTP://A.B/c"));
        assert_eq!(url("xhttps://a.b", 3), None, "not inside a word");
        assert_eq!(url("https://", 0), None);
        assert_eq!(url("go: http://日本.jp/パス next", 8), Some("http://日本.jp/パス"));
        assert_eq!(url("[http://[::1]:8080/x]", 3), Some("http://[::1]:8080/x"));
        assert_eq!(url("a) b https://a.b/c d", 7), Some("https://a.b/c"));
    }

    #[test]
    fn a_url_running_past_the_searched_span_is_not_a_link() {
        let long = format!("https://a.b/{}", "x".repeat(2 * SEARCH_RADIUS));
        assert_eq!(url(&long, 3), None, "the start would open cut short");
        assert_eq!(url(&format!("{long} end"), 3), None);
        let short = format!("{} https://a.b/c", "x ".repeat(SEARCH_RADIUS));
        assert_eq!(url(&short, short.len() - 3), Some("https://a.b/c"));
    }

    #[test]
    fn a_line_going_past_the_viewport_edge_is_marked() {
        // 12 columns, 2 rows: the URL line wraps over 3 rows, then one more line.
        let mut vt = Vt::new(12, 2, &VtOptions::default()).expect("vt");
        vt.feed(b"> https://example.com/abcdefghijklmn\r\nend");
        let mut snap = GridSnapshot::default();
        vt.snapshot(&mut snap).expect("snapshot");
        // Its last row is on top; the rest is above the viewport.
        let top = logical_line(&snap, 0);
        assert!(top.is_cut_above && !top.is_cut_below, "{top:?}");
        // Scrolled back to its first two rows: the rest is below the viewport, and the URL
        // found there reaches the cut.
        vt.scroll_viewport(wired_vt::Scroll::Delta(-2));
        vt.snapshot(&mut snap).expect("snapshot");
        let line = logical_line(&snap, 0);
        assert!(!line.is_cut_above && line.is_cut_below, "{line:?}");
        let range = find_url(&line.text, 3).expect("url");
        assert_eq!(range.end, line.text.len());
    }

    #[test]
    fn long_lines_and_many_brackets_stay_cheap() {
        // A line far longer than the search radius, and a URL ending in many closing brackets.
        let mut text = "x ".repeat(100_000);
        text.push_str("https://a.b/");
        text.push_str(&")".repeat(50_000));
        let at = text.len() - 50_000 - 3;
        let start = std::time::Instant::now();
        assert_eq!(url(&text, at), Some("https://a.b/"));
        assert!(start.elapsed() < std::time::Duration::from_millis(200), "{:?}", start.elapsed());
    }

    #[test]
    fn only_web_and_mail_links_open_directly() {
        assert_eq!(action_for("https://example.com"), LinkAction::Open);
        assert_eq!(action_for("MAILTO:a@b.c"), LinkAction::Open);
        assert_eq!(action_for("file:///C:/Windows/System32/calc.exe"), LinkAction::Confirm);
        assert_eq!(action_for("ms-settings:display"), LinkAction::Confirm);
        assert_eq!(action_for("C:\\Windows\\notepad.exe"), LinkAction::Confirm, "a drive letter reads as a scheme");
        assert_eq!(action_for("calc.exe"), LinkAction::Refuse);
        assert_eq!(action_for("https://a.b/\u{7}"), LinkAction::Refuse);
        assert_eq!(action_for("1x:y"), LinkAction::Refuse);
        // Shown reversed or with hidden parts, it would not read as what opens.
        assert_eq!(action_for("file:///C:/evil\u{202E}txt.exe"), LinkAction::Refuse);
        assert_eq!(action_for("https://exa\u{200B}mple.com"), LinkAction::Refuse);
        assert_eq!(action_for("file:///C:/a b"), LinkAction::Refuse);
        assert_eq!(action_for(&format!("file:///{}", "a".repeat(2000))), LinkAction::Refuse);
        assert_eq!(action_for("https://例え.jp/パス"), LinkAction::Open, "ordinary non-ASCII stays");
    }

    fn snapshot_of(cols: u16, bytes: &[u8]) -> (Vt, GridSnapshot) {
        let mut vt = Vt::new(cols, 4, &VtOptions::default()).expect("vt");
        vt.feed(bytes);
        let mut snap = GridSnapshot::default();
        vt.snapshot(&mut snap).expect("snapshot");
        (vt, snap)
    }

    #[test]
    fn a_url_wrapped_across_rows_is_one_link() {
        let (_, snap) = snapshot_of(12, b"> https://example.com/abc x");
        let Line { text, places, .. } = logical_line(&snap, 1);
        let at = places.iter().position(|p| p.row == 1 && p.col == 2).expect("cell");
        let range = find_url(&text, places[at].byte).expect("url");
        assert_eq!(&text[range.clone()], "https://example.com/abc");
        let first = places.iter().position(|p| p.byte >= range.start).expect("first");
        let last = places.iter().rposition(|p| p.byte < range.end).expect("last");
        // "> https://ex" / "ample.com/ab" / "c x"
        assert_eq!(segments(&places[first..=last]), [(0, 2, 12), (1, 0, 12), (2, 0, 1)]);
    }
}
