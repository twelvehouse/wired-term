//! Bundled fonts and the choice of the terminal font family.

use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

const MAX_FONT_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_FONT_FILES: usize = 32;

fn fonts_dir() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("fonts"))
}

/// Registers the fonts shipped in `fonts\` and its direct subfolders with gpui.
pub fn load_bundled(cx: &gpui::App) {
    let Some(dir) = fonts_dir() else { return };
    let mut files = Vec::new();
    collect_font_files(&dir, 1, &mut files);
    files.truncate(MAX_FONT_FILES);
    let fonts: Vec<Cow<'static, [u8]>> =
        files.iter().filter_map(|path| std::fs::read(path).ok()).map(Cow::Owned).collect();
    if !fonts.is_empty()
        && let Err(error) = cx.text_system().add_fonts(fonts)
    {
        eprintln!("wired: bundled fonts not loaded: {error:#}");
    }
}

fn collect_font_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let Ok(meta) = entry.metadata() else { continue };
        let path = entry.path();
        if meta.is_dir() {
            if depth > 0 {
                collect_font_files(&path, depth - 1, out);
            }
        } else if meta.len() <= MAX_FONT_FILE_BYTES && is_font_file(&path) {
            out.push(path);
        }
    }
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["ttf", "otf", "ttc"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// The first of `family` and `fallbacks` that is available, and the fallbacks after it. When none
/// is available, the configured family is kept and the platform picks a font.
pub fn choose_family(family: &str, fallbacks: &[String], available: &[String]) -> (String, Vec<String>) {
    let is_available = |name: &str| available.iter().any(|a| a.eq_ignore_ascii_case(name));
    if is_available(family) {
        return (family.to_owned(), fallbacks.to_vec());
    }
    match fallbacks.iter().position(|f| is_available(f)) {
        Some(i) => (fallbacks[i].clone(), fallbacks[i + 1..].to_vec()),
        None => (family.to_owned(), fallbacks.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn keeps_an_available_family() {
        let chosen =
            choose_family("UDEV Gothic NFLG", &names(&["Consolas"]), &names(&["udev gothic nflg", "Consolas"]));
        assert_eq!(chosen, ("UDEV Gothic NFLG".to_owned(), names(&["Consolas"])));
    }

    #[test]
    fn falls_back_in_order() {
        let chosen = choose_family(
            "Missing",
            &names(&["Also Missing", "Cascadia Mono", "Consolas"]),
            &names(&["Consolas", "Cascadia Mono"]),
        );
        assert_eq!(chosen, ("Cascadia Mono".to_owned(), names(&["Consolas"])));
    }

    #[test]
    fn keeps_the_family_when_nothing_is_available() {
        let chosen = choose_family("Missing", &names(&["Gone"]), &names(&["Arial"]));
        assert_eq!(chosen, ("Missing".to_owned(), names(&["Gone"])));
    }

    #[test]
    fn recognizes_font_files() {
        assert!(is_font_file(Path::new("a/UDEVGothicNFLG-Regular.TTF")));
        assert!(!is_font_file(Path::new("a/LICENSE")));
    }
}
