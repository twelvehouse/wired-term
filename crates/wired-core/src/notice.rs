//! The idea and the badge styles (black-on-white label, red ` X `, yellow ` ! `) are taken from
//! Tabby (https://github.com/Eugeny/tabby, MIT, Copyright (c) 2017 Tabby Developers), which writes
//! its session messages into the terminal the same way: `tabby-ssh/src/components/sshTab.component.ts`
//! and `tabby-ssh/src/session/ssh.ts`.

/// The badge in front of a notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoticeKind {
    /// A named source, such as `SSH` (black on white).
    Label(String),
    /// Something failed (`X`, red text).
    Error,
    /// Something needs attention (`!`, black on yellow).
    Warning,
    /// Information or a hint for the user (`i`, black on blue, as Tabby's informational labels).
    Info,
}

/// One line written into the screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// The badge.
    pub kind: NoticeKind,
    /// The text (untrusted text is fine: control characters are replaced).
    pub text: String,
}

impl Notice {
    /// A notice with a label badge.
    pub fn label(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self { kind: NoticeKind::Label(label.into()), text: text.into() }
    }

    /// An error notice.
    pub fn error(text: impl Into<String>) -> Self {
        Self { kind: NoticeKind::Error, text: text.into() }
    }

    /// A warning notice.
    pub fn warning(text: impl Into<String>) -> Self {
        Self { kind: NoticeKind::Warning, text: text.into() }
    }

    /// An information notice.
    pub fn info(text: impl Into<String>) -> Self {
        Self { kind: NoticeKind::Info, text: text.into() }
    }

    fn render(&self) -> String {
        const RESET: &str = "\x1b[0m";
        let text = sanitize(&self.text);
        match &self.kind {
            NoticeKind::Label(label) => format!("\x1b[0;30;47m {} {RESET} {text}", sanitize(label)),
            NoticeKind::Error => format!("\x1b[0;30;41m X {RESET} \x1b[31m{text}{RESET}"),
            NoticeKind::Warning => format!("\x1b[0;30;43m ! {RESET} {text}"),
            NoticeKind::Info => format!("\x1b[0;30;44m i {RESET} {text}"),
        }
    }
}

pub(crate) fn render_notices(notices: &[Notice]) -> Vec<u8> {
    let mut out = String::from("\r\n");
    for notice in notices {
        out.push_str(&notice.render());
        out.push_str("\r\n");
    }
    out.into_bytes()
}

const MAX_NOTICE_CHARS: usize = 500;

/// Control characters would act as escape sequences in the screen; show them as spaces. Bidi
/// marks, embeddings and isolates are dropped so untrusted text cannot reorder what is shown.
fn sanitize(text: &str) -> String {
    let mut chars =
        text.chars().filter(|&c| !wired_vt::is_bidi_control(c)).map(|c| if c.is_control() { ' ' } else { c });
    let mut out: String = chars.by_ref().take(MAX_NOTICE_CHARS).collect();
    if chars.next().is_some() {
        out.pop();
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_badges_and_lines() {
        let bytes = render_notices(&[Notice::error("boom"), Notice::info("Press Enter")]);
        let text = String::from_utf8(bytes).expect("utf-8");
        assert_eq!(text, "\r\n\x1b[0;30;41m X \x1b[0m \x1b[31mboom\x1b[0m\r\n\x1b[0;30;44m i \x1b[0m Press Enter\r\n");
    }

    #[test]
    fn untrusted_text_cannot_inject_sequences() {
        let bytes = render_notices(&[Notice::label("SS\x1bH", "host\x1b]0;evil\x07name")]);
        let text = String::from_utf8(bytes).expect("utf-8");
        assert!(!text.contains("\x1b]"), "{text:?}");
        assert!(!text.contains('\x07'), "{text:?}");
        assert!(text.contains("SS H"));
    }

    #[test]
    fn untrusted_text_cannot_reorder_or_flood_the_screen() {
        assert_eq!(sanitize("Not opened: evil\u{202E}txt.exe\u{2066}\u{200F}"), "Not opened: eviltxt.exe");
        let long = sanitize(&"a".repeat(MAX_NOTICE_CHARS + 1));
        assert_eq!(long.chars().count(), MAX_NOTICE_CHARS);
        assert!(long.ends_with('…'));
        assert_eq!(sanitize(&"b".repeat(MAX_NOTICE_CHARS)), "b".repeat(MAX_NOTICE_CHARS));
    }
}
