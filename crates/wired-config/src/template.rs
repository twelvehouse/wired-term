//! Settings files written from their declarations: the file a first start writes, the lines a
//! newer version adds to an existing file, and the check for keys nobody declared.
//!
//! The file is parsed as TOML to find its sections and keys (any valid header spelling, strings
//! spanning lines); new lines are inserted into the original text, so everything the user wrote
//! (values, comments, order, spacing) stays as it is.

/// One declared line of a settings file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Line {
    /// `[section]` it belongs to.
    pub section: &'static str,
    /// The key it sets (or shows, when commented out).
    pub key: &'static str,
    /// The whole line as written, comments included.
    pub text: String,
}

/// `key = value` padded so that the comments of a block line up, then `# comment`. With
/// `commented_out` the line is a comment itself: the key is documented but not set.
pub(crate) fn line(section: &'static str, key: &'static str, value: &str, comment: &str, commented_out: bool) -> Line {
    let assignment = format!("{}{key} = {value}", if commented_out { "# " } else { "" });
    let text = if comment.is_empty() { assignment } else { format!("{assignment:<31} # {comment}") };
    Line { section, key, text }
}

/// The file: `header`, then every section in the order its first line appears.
pub(crate) fn render(header: &str, lines: &[Line]) -> String {
    let mut text = String::from(header);
    for section in sections(lines) {
        text.push_str(&format!("\n[{section}]\n"));
        for line in lines.iter().filter(|l| l.section == section) {
            text.push_str(&line.text);
            text.push('\n');
        }
    }
    text
}

fn sections(lines: &[Line]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for line in lines {
        if !out.contains(&line.section) {
            out.push(line.section);
        }
    }
    out
}

/// The key a comment line shows (`# key = ...`), if any.
fn commented_key(line: &str) -> Option<&str> {
    let body = line.trim_start().strip_prefix('#')?.trim_start();
    let (key, _) = body.split_once('=')?;
    let key = key.trim().trim_matches('"');
    let is_bare = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    is_bare.then_some(key)
}

/// Byte offsets where every table header of `doc` starts (`[a]`, `[a.b]`, `[[a]]`), sorted.
fn header_starts(doc: &toml_edit::Document<&str>) -> Vec<usize> {
    fn visit(table: &toml_edit::Table, out: &mut Vec<usize>) {
        if let Some(span) = table.span().filter(|s| !s.is_empty()) {
            out.push(span.start);
        }
        for (_, item) in table.iter() {
            match item {
                toml_edit::Item::Table(t) => visit(t, out),
                toml_edit::Item::ArrayOfTables(array) => array.iter().for_each(|t| visit(t, out)),
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    visit(doc.as_table(), &mut out);
    out.sort_unstable();
    out
}

/// Adds every declared line whose key the file neither sets nor shows commented out: at the
/// end of its section, or in a new section at the end. The file is parsed as TOML first, so
/// sections are found however their headers are written; a file that does not parse, or a
/// result that would not, is an error and nothing changes. Returns the new text and the keys
/// added (`section.key`); the text is unchanged when nothing is missing.
pub(crate) fn add_missing(text: &str, lines: &[Line]) -> Result<(String, Vec<String>), String> {
    let doc = toml_edit::Document::parse(text).map_err(|e| e.to_string())?;
    let starts = header_starts(&doc);
    // (byte offset to insert at, text) per section, applied from the end.
    let mut inserts: Vec<(usize, String)> = Vec::new();
    let mut appended = String::new();
    let mut added = Vec::new();
    for section in sections(lines) {
        let declared = lines.iter().filter(|l| l.section == section);
        let existing = doc.as_table().get(section);
        let missing: Vec<&Line> = match existing {
            None => declared.collect(),
            Some(toml_edit::Item::Table(table)) => {
                let header = table.span().filter(|s| !s.is_empty() && !table.is_dotted() && !table.is_implicit());
                let Some(header) = header else {
                    // Set through dotted keys (`font.size = 1`): no section to add lines to.
                    continue;
                };
                let end = starts.iter().copied().find(|&s| s > header.start).unwrap_or(text.len());
                let body = &text[header.end..end];
                let shown: Vec<&str> = body.lines().filter_map(commented_key).collect();
                let missing: Vec<&Line> =
                    declared.filter(|l| !table.contains_key(l.key) && !shown.contains(&l.key)).collect();
                if !missing.is_empty() {
                    // After the section's last non-blank line, so a blank line before the next
                    // section stays where it is.
                    let at = header.end + body.trim_end().len();
                    let block: String = missing.iter().map(|l| format!("\n{}", l.text)).collect();
                    inserts.push((at, block));
                }
                missing
            }
            Some(_) => continue,
        };
        if missing.is_empty() {
            continue;
        }
        added.extend(missing.iter().map(|l| format!("{section}.{}", l.key)));
        if existing.is_none() {
            appended.push_str(&format!("\n[{section}]\n"));
            for line in missing {
                appended.push_str(&line.text);
                appended.push('\n');
            }
        }
    }
    if added.is_empty() {
        return Ok((text.to_owned(), added));
    }
    let mut out = text.to_owned();
    inserts.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    for (at, block) in inserts {
        out.insert_str(at, &block);
    }
    if !appended.is_empty() {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&appended);
    }
    toml_edit::Document::parse(out.as_str()).map_err(|e| format!("adding new entries would break the file: {e}"))?;
    Ok((out, added))
}

/// Keys the file sets in declared sections that no line declares, as `section.key`. Keys of
/// undeclared sections are left to the parser. A file that does not parse has none (the parse
/// error is reported instead).
pub(crate) fn unknown_keys(text: &str, lines: &[Line]) -> Vec<String> {
    let Ok(doc) = toml_edit::Document::parse(text) else { return Vec::new() };
    let mut out = Vec::new();
    for section in sections(lines) {
        let Some(table) = doc.as_table().get(section).and_then(toml_edit::Item::as_table_like) else { continue };
        for (key, _) in table.iter() {
            if !lines.iter().any(|l| l.section == section && l.key == key) {
                out.push(format!("{section}.{key}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines() -> Vec<Line> {
        vec![
            line("font", "size", "12.0", "", false),
            line("font", "gamma", "1.8", "1 .. 3", true),
            line("pane", "divider_width", "4.0", "px", false),
        ]
    }

    #[test]
    fn renders_sections_in_order() {
        let text = render("# head\n", &lines());
        assert_eq!(
            text,
            "# head\n\n[font]\nsize = 12.0\n# gamma = 1.8                   # 1 .. 3\n\n[pane]\ndivider_width = 4.0             # px\n"
        );
    }

    #[test]
    fn adds_only_what_is_missing_and_keeps_the_rest() {
        let user = "# mine\n[font]\nsize = 14 # big\n\n[colors]\nforeground = \"#fff\"\n";
        let (text, added) = add_missing(user, &lines()).expect("parses");
        assert_eq!(added, ["font.gamma", "pane.divider_width"]);
        assert_eq!(
            text,
            "# mine\n[font]\nsize = 14 # big\n# gamma = 1.8                   # 1 .. 3\n\n[colors]\nforeground = \"#fff\"\n\n[pane]\ndivider_width = 4.0             # px\n"
        );
        // Run again: nothing to add, text unchanged.
        let (again, added) = add_missing(&text, &lines()).expect("parses");
        assert!(added.is_empty());
        assert_eq!(again, text);
    }

    #[test]
    fn a_commented_out_key_counts_as_present() {
        let user = "[font]\n# size = 10\n#gamma=2\n[pane]\ndivider_width = 2\n";
        let (_, added) = add_missing(user, &lines()).expect("parses");
        assert!(added.is_empty(), "{added:?}");
    }

    #[test]
    fn any_valid_header_spelling_is_the_same_section() {
        // A quoted table name and a multi-line string that looks like a header.
        let user = "['font']\nsize = 14\nnote = '''\n[pane]\n'''\n";
        let (text, added) = add_missing(user, &lines()).expect("parses");
        assert_eq!(added, ["font.gamma", "pane.divider_width"]);
        assert_eq!(text.matches("['font']").count(), 1);
        assert!(text.contains("note = '''\n[pane]\n'''\n# gamma = 1.8"), "{text}");
        assert!(text.ends_with("\n[pane]\ndivider_width = 4.0             # px\n"), "{text}");
    }

    #[test]
    fn a_file_that_does_not_parse_is_left_alone() {
        assert!(add_missing("[font\nsize = 1\n", &lines()).is_err());
    }

    #[test]
    fn dotted_keys_get_no_new_section() {
        // `font` exists only through a dotted key: adding `[font]` would redefine it.
        let user = "font.size = 14\n[pane]\ndivider_width = 2\n";
        let (text, added) = add_missing(user, &lines()).expect("parses");
        assert!(added.is_empty(), "{added:?}");
        assert_eq!(text, user);
    }

    #[test]
    fn unknown_keys_in_declared_sections_are_found() {
        let user = "[font]\nsize = 1\nsizee = 2\n# old = 3\n[other]\nx = 1\n";
        assert_eq!(unknown_keys(user, &lines()), ["font.sizee"]);
    }
}
