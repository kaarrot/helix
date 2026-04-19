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
    pub theirs: String,
    pub both: String,
}

/// Parse `<<<<<<<` / `=======` / `>>>>>>>` conflict markers from a rope
/// and return the line ranges of each conflict block.
pub struct ConflictRegion {
    /// Line of `<<<<<<<` marker (inclusive)
    pub start_line: usize,
    /// Line of `>>>>>>>` marker (inclusive)
    pub end_line: usize,
}

pub fn find_conflicts(text: &Rope) -> Vec<ConflictRegion> {
    let mut regions = Vec::new();
    let mut start: Option<usize> = None;

    for (i, line) in text.lines().enumerate() {
        let s: String = line.chars().collect();
        let s = s.trim_end_matches(['\n', '\r']);
        if s.starts_with("<<<<<<<") {
            start = Some(i);
        } else if s.starts_with(">>>>>>>") {
            if let Some(s_line) = start.take() {
                regions.push(ConflictRegion { start_line: s_line, end_line: i });
            }
        }
    }

    regions
}

/// Count remaining (unresolved) conflict blocks.
pub fn conflict_count(text: &Rope) -> usize {
    find_conflicts(text).len()
}

/// Extract OURS and THEIRS content from a single conflict region in `text`.
/// Returns `None` if the markers are not well-formed.
pub fn extract_sides(text: &Rope, region: &ConflictRegion) -> Option<(String, Option<String>, String)> {
    let mut ours = Vec::new();
    let mut base: Option<Vec<String>> = None;
    let mut theirs = Vec::new();
    let mut state = 0u8; // 0=before, 1=ours, 2=base, 3=theirs

    for (i, line) in text.lines().enumerate() {
        if i < region.start_line || i > region.end_line {
            continue;
        }
        let s: String = line.chars().collect();
        let s_trim = s.trim_end_matches(['\n', '\r']);
        if i == region.start_line && s_trim.starts_with("<<<<<<<") {
            state = 1;
            continue;
        }
        if s_trim.starts_with("|||||||") {
            base = Some(Vec::new());
            state = 2;
            continue;
        }
        if s_trim.starts_with("=======") {
            state = 3;
            continue;
        }
        if s_trim.starts_with(">>>>>>>") {
            break;
        }
        match state {
            1 => ours.push(s),
            2 => { if let Some(ref mut b) = base { b.push(s); } }
            3 => theirs.push(s),
            _ => {}
        }
    }

    if state == 0 { return None; }

    let join = |lines: Vec<String>| -> String {
        lines.into_iter().collect()
    };

    Some((join(ours), base.map(join), join(theirs)))
}
