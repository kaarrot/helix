use helix_core::Rope;

/// Remembers the last conflict resolution so a follow-up accept replaces it
/// instead of failing to find markers that no longer exist.
#[derive(Debug, Clone)]
pub struct LastResolution {
    /// Absolute char offset where the replacement begins in the RESULT doc.
    pub start_char: usize,
    /// Absolute char offset where the replacement ends (exclusive).
    pub end_char: usize,
    pub ours: String,
    pub base: Option<String>,
    pub theirs: String,
    pub both: String,
    /// `Document::version()` after the accept that produced this range.
    /// Any later RESULT edit invalidates the switch-side shortcut.
    pub doc_version: i32,
}

impl LastResolution {
    /// Whether `cursor` is still inside the last inserted replacement.
    ///
    /// `end_char` is exclusive, so a cursor sitting on the first character
    /// after the replacement (including an immediately following conflict)
    /// is *not* treated as still inside it. An empty replacement is a point.
    pub fn contains_cursor(&self, cursor: usize) -> bool {
        if self.start_char == self.end_char {
            cursor == self.start_char
        } else {
            cursor >= self.start_char && cursor < self.end_char
        }
    }
}

/// Parse `<<<<<<<` / `=======` / `>>>>>>>` conflict markers from a rope
/// and return the line ranges of each conflict block.
#[derive(Debug, Clone)]
pub struct ConflictRegion {
    pub marker_size: usize,
    /// Line of `<<<<<<<` marker (inclusive)
    pub start_line: usize,
    /// Line of `>>>>>>>` marker (inclusive)
    pub end_line: usize,
}

#[derive(Debug, Default)]
pub struct ConflictScan {
    pub regions: Vec<ConflictRegion>,
    pub marker_lines: usize,
    pub malformed: bool,
}

/// Scan all marker-shaped lines, including orphaned markers. A malformed
/// block must never become an apparently resolved file, or a smaller block
/// whose acceptance silently discards content.
pub fn scan_conflicts(text: &Rope) -> ConflictScan {
    scan_conflicts_impl(text, None)
}

pub fn scan_conflicts_with_size(text: &Rope, marker_size: usize) -> ConflictScan {
    scan_conflicts_impl(text, Some(marker_size))
}

fn scan_conflicts_impl(text: &Rope, marker_size: Option<usize>) -> ConflictScan {
    let mut scan = ConflictScan::default();
    let mut start: Option<(usize, usize)> = None;
    let mut state = 0;

    for (i, line) in text.lines().enumerate() {
        let s = line.to_string();
        let s = s.trim_end_matches(['\n', '\r']);
        let Some((kind, width)) = marker(s, marker_size) else {
            continue;
        };
        // A divider without an open conflict is ordinary text, for example a
        // Markdown heading underline. Labelled orphan markers still signal
        // damaged conflict syntax and must block destructive merge commands.
        if kind == '=' && start.is_none() {
            continue;
        }
        scan.marker_lines += 1;
        if start.is_some_and(|(_, expected)| width != expected) {
            scan.malformed = true;
        }
        match kind {
            '<' if start.is_none() => {
                start = Some((i, width));
                state = 1;
            }
            '|' if state == 1 => state = 2,
            '=' if state == 1 || state == 2 => state = 3,
            '>' => {
                if let Some((start_line, width)) = start.take() {
                    scan.regions.push(ConflictRegion {
                        marker_size: width,
                        start_line,
                        end_line: i,
                    });
                } else {
                    scan.malformed = true;
                }
                scan.malformed |= state != 3;
                state = 0;
            }
            _ => scan.malformed = true,
        }
    }
    scan.malformed |= start.is_some();
    scan
}

fn marker(line: &str, marker_size: Option<usize>) -> Option<(char, usize)> {
    let kind = line.chars().next()?;
    if !matches!(kind, '<' | '|' | '=' | '>') {
        return None;
    }
    let width = line.bytes().take_while(|&b| b == kind as u8).count();
    let valid_width = marker_size.map_or(width >= 7, |size| width == size);
    let suffix = &line[width..];
    let valid_suffix = if kind == '=' {
        suffix.is_empty()
    } else {
        suffix.is_empty() || suffix.starts_with([' ', '\t'])
    };
    (valid_width && valid_suffix).then_some((kind, width))
}

pub fn find_conflicts(text: &Rope) -> Vec<ConflictRegion> {
    scan_conflicts(text).regions
}

/// Count remaining (unresolved) conflict blocks.
pub fn conflict_count(text: &Rope) -> usize {
    find_conflicts(text).len()
}

/// Extract OURS and THEIRS content from a single conflict region in `text`.
/// Returns `None` if the markers are not well-formed.
pub fn extract_sides(
    text: &Rope,
    region: &ConflictRegion,
) -> Option<(String, Option<String>, String)> {
    if scan_conflicts_with_size(text, region.marker_size).malformed {
        return None;
    }
    let mut ours = Vec::new();
    let mut base: Option<Vec<String>> = None;
    let mut theirs = Vec::new();
    let mut state = 0u8; // 0=before, 1=ours, 2=base, 3=theirs
    let mut seen_separator = false;

    for (i, line) in text.lines().enumerate() {
        if i < region.start_line || i > region.end_line {
            continue;
        }
        let s: String = line.chars().collect();
        let s_trim = s.trim_end_matches(['\n', '\r']);
        let kind = marker(s_trim, Some(region.marker_size)).map(|(kind, _)| kind);
        if i == region.start_line && kind == Some('<') {
            state = 1;
            continue;
        }
        if kind == Some('|') {
            if state != 1 || base.is_some() || seen_separator {
                return None;
            }
            base = Some(Vec::new());
            state = 2;
            continue;
        }
        if kind == Some('=') {
            if !(state == 1 || state == 2) || seen_separator {
                return None;
            }
            seen_separator = true;
            state = 3;
            continue;
        }
        if kind == Some('>') {
            break;
        }
        match state {
            1 => ours.push(s),
            2 => {
                if let Some(ref mut b) = base {
                    b.push(s);
                }
            }
            3 => theirs.push(s),
            _ => {}
        }
    }

    if state != 3 || !seen_separator {
        return None;
    }

    let join = |lines: Vec<String>| -> String { lines.into_iter().collect() };

    Some((join(ours), base.map(join), join(theirs)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regression_ordinary_banners_are_not_conflicts() {
        for input in [
            "Heading\n=======\n",
            "================================\n",
            "<<<<<<<operator\n",
            ">>>>>>>shift\n",
            "=======suffix\n",
        ] {
            let scan = scan_conflicts(&Rope::from(input));
            assert!(!scan.malformed, "{input:?}");
            assert_eq!(scan.marker_lines, 0, "{input:?}");
        }
        let text =
            Rope::from("Heading\n=======\n<<<<<<< ours\nours\n=======\ntheirs\n>>>>>>> theirs\n");
        let scan = scan_conflicts(&text);
        assert!(!scan.malformed);
        assert_eq!(scan.regions.len(), 1);
        assert_eq!(extract_sides(&text, &scan.regions[0]).unwrap().0, "ours\n");
    }

    #[test]
    fn regression_known_marker_width_is_exact() {
        let text = Rope::from("<<<<<<<<< ours\nours\n=========\ntheirs\n>>>>>>>>> theirs\n");
        assert!(scan_conflicts_with_size(&text, 7).regions.is_empty());
        assert_eq!(scan_conflicts_with_size(&text, 9).regions.len(), 1);
    }

    const START_MARKER: &str = "<<<<<<<";
    const BASE_MARKER: &str = "|||||||";
    const SPLIT_MARKER: &str = "=======";
    const END_MARKER: &str = ">>>>>>>";

    fn rope(text: &str) -> Rope {
        Rope::from(text)
    }

    #[test]
    fn parses_two_way_conflict() {
        let text = rope(&format!(
            "{START_MARKER} ours\nours line\n{SPLIT_MARKER}\ntheirs line\n{END_MARKER} theirs\n"
        ));

        let conflicts = find_conflicts(&text);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflict_count(&text), 1);

        let (ours, base, theirs) = extract_sides(&text, &conflicts[0]).unwrap();
        assert_eq!(ours, "ours line\n");
        assert_eq!(base, None);
        assert_eq!(theirs, "theirs line\n");
    }

    #[test]
    fn parses_diff3_conflict() {
        let text = rope(&format!(
            "{START_MARKER} ours\nours line\n{BASE_MARKER} base\nbase line\n{SPLIT_MARKER}\ntheirs line\n{END_MARKER} theirs\n"
        ));

        let conflicts = find_conflicts(&text);
        assert_eq!(conflicts.len(), 1);

        let (ours, base, theirs) = extract_sides(&text, &conflicts[0]).unwrap();
        assert_eq!(ours, "ours line\n");
        assert_eq!(base.as_deref(), Some("base line\n"));
        assert_eq!(theirs, "theirs line\n");
    }

    #[test]
    fn counts_multiple_conflicts() {
        let text = rope(&format!(
            "{START_MARKER} ours\nfirst ours\n{SPLIT_MARKER}\nfirst theirs\n{END_MARKER} theirs\n\
\n\
{START_MARKER} ours\nsecond ours\n{SPLIT_MARKER}\nsecond theirs\n{END_MARKER} theirs\n"
        ));

        let conflicts = find_conflicts(&text);
        assert_eq!(conflicts.len(), 2);
        assert_eq!(conflict_count(&text), 2);
        assert_eq!(conflicts[0].start_line, 0);
        assert_eq!(conflicts[1].start_line, 6);
    }

    #[test]
    fn rejects_malformed_conflict_markers() {
        let text = rope(&format!(
            "{START_MARKER} ours\nours line\n{END_MARKER} theirs\n"
        ));
        let conflicts = find_conflicts(&text);
        assert_eq!(conflicts.len(), 1);
        assert!(extract_sides(&text, &conflicts[0]).is_none());
    }

    #[test]
    fn orphaned_and_nested_markers_are_not_resolved() {
        for input in [
            "=======\ntheirs\n>>>>>>> theirs\n",
            "<<<<<<< ours\nours\n=======\ntheirs\n",
            "||||||| base\nbase\n",
            "<<<<<<< ours\n<<<<<<< quoted\nours\n=======\ntheirs\n>>>>>>> theirs\n",
            "<<<<<<< ours\nours\n=======\ntheirs\n>>>>>>> quoted\nmore\n>>>>>>> theirs\n",
        ] {
            let scan = scan_conflicts(&rope(input));
            assert!(scan.malformed, "{input}");
            assert!(scan.marker_lines > 0);
        }
    }

    #[test]
    fn supports_long_markers_and_crlf_without_losing_content() {
        let text = rope("<<<<<<<<< ours\r\nours\r\n||||||||| base\r\nbase\r\n=========\r\ntheirs\r\n>>>>>>>>> theirs\r\n");
        let scan = scan_conflicts(&text);
        assert!(!scan.malformed);
        let (ours, base, theirs) = extract_sides(&text, &scan.regions[0]).unwrap();
        assert_eq!(ours, "ours\r\n");
        assert_eq!(base.as_deref(), Some("base\r\n"));
        assert_eq!(theirs, "theirs\r\n");
    }

    #[test]
    fn last_resolution_contains_cursor_is_exclusive() {
        let prior = LastResolution {
            start_char: 10,
            end_char: 20,
            ours: String::new(),
            base: None,
            theirs: String::new(),
            both: String::new(),
            doc_version: 1,
        };
        assert!(prior.contains_cursor(10));
        assert!(prior.contains_cursor(19));
        assert!(!prior.contains_cursor(20));
        assert!(!prior.contains_cursor(9));
    }

    #[test]
    fn last_resolution_empty_replacement_is_a_point() {
        let prior = LastResolution {
            start_char: 5,
            end_char: 5,
            ours: String::new(),
            base: None,
            theirs: String::new(),
            both: String::new(),
            doc_version: 1,
        };
        assert!(prior.contains_cursor(5));
        assert!(!prior.contains_cursor(4));
        assert!(!prior.contains_cursor(6));
    }
}
