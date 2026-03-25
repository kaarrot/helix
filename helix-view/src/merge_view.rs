//! 3-way merge conflict view state management.
//!
//! This module provides structures for managing 3-way merge conflict resolution
//! where OURS, THEIRS, and the merge result are shown in a three-pane layout.

use std::path::PathBuf;

use helix_core::Rope;

use crate::{DocumentId, ViewId};

/// Represents a 3-way merge view session for resolving git conflicts.
///
/// Layout:
/// ```text
/// +-------+-------+
/// | OURS  | THEIRS|
/// +-------+-------+
/// |    RESULT     |
/// +---------------+
/// ```
#[derive(Debug, Clone)]
pub struct MergeViewState {
    /// Document containing OURS version (read-only)
    pub ours_doc_id: DocumentId,
    /// Document containing THEIRS version (read-only)
    pub theirs_doc_id: DocumentId,
    /// Document containing merge result (editable)
    pub result_doc_id: DocumentId,
    /// View ID for OURS pane
    pub ours_view_id: ViewId,
    /// View ID for THEIRS pane
    pub theirs_view_id: ViewId,
    /// View ID for RESULT pane
    pub result_view_id: ViewId,
    /// Parsed conflict hunks
    pub conflicts: Vec<ConflictHunk>,
    /// Index of currently focused conflict (0-indexed)
    pub current_conflict: usize,
    /// Path to the original conflicted file
    pub original_path: PathBuf,
    /// Whether to synchronize scroll between OURS and THEIRS panes
    pub sync_scroll: bool,
}

impl MergeViewState {
    /// Create a new merge view state.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ours_doc_id: DocumentId,
        theirs_doc_id: DocumentId,
        result_doc_id: DocumentId,
        ours_view_id: ViewId,
        theirs_view_id: ViewId,
        result_view_id: ViewId,
        conflicts: Vec<ConflictHunk>,
        original_path: PathBuf,
    ) -> Self {
        Self {
            ours_doc_id,
            theirs_doc_id,
            result_doc_id,
            ours_view_id,
            theirs_view_id,
            result_view_id,
            conflicts,
            current_conflict: 0,
            original_path,
            sync_scroll: true,
        }
    }

    /// Check if a given view is part of this merge session.
    pub fn contains_view(&self, view_id: ViewId) -> bool {
        self.ours_view_id == view_id
            || self.theirs_view_id == view_id
            || self.result_view_id == view_id
    }

    /// Get the current conflict hunk, if any.
    pub fn current_conflict(&self) -> Option<&ConflictHunk> {
        self.conflicts.get(self.current_conflict)
    }

    /// Move to the next conflict.
    pub fn next_conflict(&mut self) {
        if !self.conflicts.is_empty() {
            self.current_conflict = (self.current_conflict + 1) % self.conflicts.len();
        }
    }

    /// Move to the previous conflict.
    pub fn prev_conflict(&mut self) {
        if !self.conflicts.is_empty() {
            self.current_conflict = if self.current_conflict == 0 {
                self.conflicts.len() - 1
            } else {
                self.current_conflict - 1
            };
        }
    }

    /// Count of resolved conflicts.
    pub fn resolved_count(&self) -> usize {
        self.conflicts
            .iter()
            .filter(|c| !matches!(c.resolution, ConflictResolution::Unresolved))
            .count()
    }

    /// Check if all conflicts are resolved.
    pub fn all_resolved(&self) -> bool {
        self.resolved_count() == self.conflicts.len()
    }
}

/// Represents a single conflict hunk parsed from conflict markers.
#[derive(Debug, Clone)]
pub struct ConflictHunk {
    /// Start line in original file (the <<<<<<< marker line)
    pub start_line: usize,
    /// End line in original file (the >>>>>>> marker line)
    pub end_line: usize,
    /// OURS content (between <<<<<<< and =======)
    pub ours_content: String,
    /// THEIRS content (between ======= and >>>>>>>)
    pub theirs_content: String,
    /// BASE content if present (between ||||||| and =======, for diff3 style)
    pub base_content: Option<String>,
    /// Current resolution state
    pub resolution: ConflictResolution,
    /// Line ranges in the OURS document
    pub ours_lines: (usize, usize),
    /// Line ranges in the THEIRS document
    pub theirs_lines: (usize, usize),
}

impl ConflictHunk {
    /// Create a new conflict hunk.
    pub fn new(
        start_line: usize,
        end_line: usize,
        ours_content: String,
        theirs_content: String,
        base_content: Option<String>,
    ) -> Self {
        Self {
            start_line,
            end_line,
            ours_content,
            theirs_content,
            base_content,
            resolution: ConflictResolution::Unresolved,
            ours_lines: (0, 0),
            theirs_lines: (0, 0),
        }
    }

    /// Get the resolved content based on current resolution.
    pub fn resolved_content(&self) -> Option<String> {
        match &self.resolution {
            ConflictResolution::Unresolved => None,
            ConflictResolution::AcceptOurs => Some(self.ours_content.clone()),
            ConflictResolution::AcceptTheirs => Some(self.theirs_content.clone()),
            ConflictResolution::AcceptBoth => {
                let mut result = self.ours_content.clone();
                if !result.ends_with('\n') && !self.theirs_content.is_empty() {
                    result.push('\n');
                }
                result.push_str(&self.theirs_content);
                Some(result)
            }
            ConflictResolution::Custom(content) => Some(content.clone()),
        }
    }

    /// Reconstruct the original conflict markers for this hunk.
    /// Used when undoing a resolution to try a different option.
    pub fn original_conflict_markers(&self) -> String {
        let mut result = String::from("<<<<<<< HEAD\n");
        result.push_str(&self.ours_content);
        if !self.ours_content.is_empty() && !self.ours_content.ends_with('\n') {
            result.push('\n');
        }

        if let Some(ref base) = self.base_content {
            result.push_str("||||||| base\n");
            result.push_str(base);
            if !base.is_empty() && !base.ends_with('\n') {
                result.push('\n');
            }
        }

        result.push_str("=======\n");
        result.push_str(&self.theirs_content);
        if !self.theirs_content.is_empty() && !self.theirs_content.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(">>>>>>> branch\n");

        result
    }
}

/// Resolution state for a conflict hunk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ConflictResolution {
    /// Conflict is not yet resolved
    #[default]
    Unresolved,
    /// Accept OURS (current branch) version
    AcceptOurs,
    /// Accept THEIRS (incoming branch) version
    AcceptTheirs,
    /// Accept both versions (OURS followed by THEIRS)
    AcceptBoth,
    /// Custom manually edited resolution
    Custom(String),
}

/// Builder for parsing conflict hunks from text.
#[derive(Debug, Default)]
struct ConflictBuilder {
    start_line: usize,
    base_start: Option<usize>,
    separator_line: Option<usize>,
    ours_lines: Vec<String>,
    base_lines: Vec<String>,
    theirs_lines: Vec<String>,
}

impl ConflictBuilder {
    fn new(start_line: usize) -> Self {
        Self {
            start_line,
            ..Default::default()
        }
    }

    fn set_base_start(&mut self, line: usize) {
        self.base_start = Some(line);
    }

    fn set_separator(&mut self, line: usize) {
        self.separator_line = Some(line);
    }

    fn add_line(&mut self, line: &str) {
        if self.separator_line.is_some() {
            // After separator, collecting THEIRS
            self.theirs_lines.push(line.to_string());
        } else if self.base_start.is_some() {
            // After base marker, collecting BASE
            self.base_lines.push(line.to_string());
        } else {
            // Before separator, collecting OURS
            self.ours_lines.push(line.to_string());
        }
    }

    fn build(self, end_line: usize) -> Option<ConflictHunk> {
        // Separator must be present for a valid conflict
        self.separator_line?;

        let ours_content = self.ours_lines.join("\n");
        let theirs_content = self.theirs_lines.join("\n");
        let base_content = if self.base_start.is_some() {
            Some(self.base_lines.join("\n"))
        } else {
            None
        };

        Some(ConflictHunk::new(
            self.start_line,
            end_line,
            ours_content,
            theirs_content,
            base_content,
        ))
    }
}

/// Compute the line ranges that each conflict occupies in the OURS and THEIRS
/// documents.
///
/// The OURS/THEIRS docs replace each conflict marker block with only that
/// side's content.  Non-conflict regions have identical line counts across all
/// versions, so we can walk the RESULT structure and accumulate offsets.
pub fn compute_conflict_line_ranges(conflicts: &mut [ConflictHunk]) {
    let mut ours_line: usize = 0;
    let mut theirs_line: usize = 0;
    let mut last_result_end: usize = 0;

    for conflict in conflicts.iter_mut() {
        // Non-conflict lines between previous conflict end and this one
        let prefix_lines = conflict.start_line.saturating_sub(last_result_end);
        ours_line += prefix_lines;
        theirs_line += prefix_lines;

        let ours_count = if conflict.ours_content.is_empty() {
            0
        } else {
            conflict.ours_content.lines().count()
        };
        let theirs_count = if conflict.theirs_content.is_empty() {
            0
        } else {
            conflict.theirs_content.lines().count()
        };

        conflict.ours_lines = (ours_line, ours_line + ours_count);
        conflict.theirs_lines = (theirs_line, theirs_line + theirs_count);

        ours_line += ours_count;
        theirs_line += theirs_count;
        last_result_end = conflict.end_line + 1;
    }
}

/// Parse conflict markers from file content.
///
/// Supports both standard and diff3 conflict marker formats:
///
/// Standard format:
/// ```text
/// <<<<<<< HEAD
/// our changes
/// =======
/// their changes
/// >>>>>>> branch
/// ```
///
/// Diff3 format:
/// ```text
/// <<<<<<< HEAD
/// our changes
/// ||||||| base
/// original content
/// =======
/// their changes
/// >>>>>>> branch
/// ```
pub fn parse_conflicts(text: &Rope) -> Vec<ConflictHunk> {
    let mut conflicts = Vec::new();
    let mut current_builder: Option<ConflictBuilder> = None;

    for (line_idx, line) in text.lines().enumerate() {
        let line_str: String = line.chars().collect();
        // Strip trailing newline/carriage return - Rope.lines() includes them
        let line_str = line_str
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();

        if line_str.starts_with("<<<<<<<") {
            // Start of a new conflict
            current_builder = Some(ConflictBuilder::new(line_idx));
        } else if let Some(ref mut builder) = current_builder {
            if line_str.starts_with("|||||||") {
                // Base marker (diff3 style)
                builder.set_base_start(line_idx);
            } else if line_str.starts_with("=======") {
                // Separator between OURS/BASE and THEIRS
                builder.set_separator(line_idx);
            } else if line_str.starts_with(">>>>>>>") {
                // End of conflict
                if let Some(hunk) = current_builder.take().and_then(|b| b.build(line_idx)) {
                    conflicts.push(hunk);
                }
            } else {
                // Regular content line
                builder.add_line(&line_str);
            }
        }
    }

    conflicts
}

/// Extract separate OURS and THEIRS document content from a conflicted file.
///
/// Returns (ours_content, theirs_content) where each string contains the
/// file content with that side's version of each conflict.
pub fn extract_conflict_versions(text: &Rope, conflicts: &[ConflictHunk]) -> (String, String) {
    if conflicts.is_empty() {
        let content: String = text.chars().collect();
        return (content.clone(), content);
    }

    let mut ours_parts = Vec::new();
    let mut theirs_parts = Vec::new();
    let mut last_end = 0;

    let lines: Vec<String> = text.lines().map(|l| l.chars().collect()).collect();

    for conflict in conflicts {
        // Add content before this conflict (same in both)
        if conflict.start_line > last_end {
            let before: String = lines[last_end..conflict.start_line].join("\n");
            if !before.is_empty() {
                ours_parts.push(before.clone());
                theirs_parts.push(before);
            }
        }

        // Add the respective conflict content
        ours_parts.push(conflict.ours_content.clone());
        theirs_parts.push(conflict.theirs_content.clone());

        last_end = conflict.end_line + 1;
    }

    // Add any remaining content after the last conflict
    if last_end < lines.len() {
        let after: String = lines[last_end..].join("\n");
        if !after.is_empty() {
            ours_parts.push(after.clone());
            theirs_parts.push(after);
        }
    }

    (ours_parts.join("\n"), theirs_parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_conflict() {
        let text = Rope::from(
            "line before\n\
             <<<<<<< HEAD\n\
             our change\n\
             =======\n\
             their change\n\
             >>>>>>> branch\n\
             line after\n",
        );

        let conflicts = parse_conflicts(&text);
        assert_eq!(conflicts.len(), 1);

        let conflict = &conflicts[0];
        assert_eq!(conflict.start_line, 1);
        assert_eq!(conflict.end_line, 5);
        assert_eq!(conflict.ours_content, "our change");
        assert_eq!(conflict.theirs_content, "their change");
        assert!(conflict.base_content.is_none());
    }

    #[test]
    fn test_parse_diff3_conflict() {
        let text = Rope::from(
            "<<<<<<< HEAD\n\
             our change\n\
             ||||||| base\n\
             original\n\
             =======\n\
             their change\n\
             >>>>>>> branch\n",
        );

        let conflicts = parse_conflicts(&text);
        assert_eq!(conflicts.len(), 1);

        let conflict = &conflicts[0];
        assert_eq!(conflict.ours_content, "our change");
        assert_eq!(conflict.theirs_content, "their change");
        assert_eq!(conflict.base_content, Some("original".to_string()));
    }

    #[test]
    fn test_extract_versions() {
        let text = Rope::from(
            "header\n\
             <<<<<<< HEAD\n\
             ours\n\
             =======\n\
             theirs\n\
             >>>>>>> branch\n\
             footer\n",
        );

        let conflicts = parse_conflicts(&text);
        let (ours, theirs) = extract_conflict_versions(&text, &conflicts);

        assert!(ours.contains("header"));
        assert!(ours.contains("ours"));
        assert!(ours.contains("footer"));
        assert!(!ours.contains("<<<<<<<"));

        assert!(theirs.contains("header"));
        assert!(theirs.contains("theirs"));
        assert!(theirs.contains("footer"));
    }

    #[test]
    fn test_compute_conflict_line_ranges() {
        // File:
        //   0: header
        //   1: <<<<<<< HEAD
        //   2: our line 1
        //   3: our line 2
        //   4: =======
        //   5: their line
        //   6: >>>>>>> branch
        //   7: middle
        //   8: <<<<<<< HEAD
        //   9: our2
        //  10: =======
        //  11: their2 a
        //  12: their2 b
        //  13: their2 c
        //  14: >>>>>>> branch
        //  15: footer
        let text = Rope::from(
            "header\n\
             <<<<<<< HEAD\n\
             our line 1\n\
             our line 2\n\
             =======\n\
             their line\n\
             >>>>>>> branch\n\
             middle\n\
             <<<<<<< HEAD\n\
             our2\n\
             =======\n\
             their2 a\n\
             their2 b\n\
             their2 c\n\
             >>>>>>> branch\n\
             footer\n",
        );

        let mut conflicts = parse_conflicts(&text);
        assert_eq!(conflicts.len(), 2);
        compute_conflict_line_ranges(&mut conflicts);

        // OURS doc:  header | our line 1 | our line 2 | middle | our2 | footer
        //            line 0   line 1       line 2       line 3   line 4  line 5
        assert_eq!(conflicts[0].ours_lines, (1, 3)); // lines 1..3
        assert_eq!(conflicts[1].ours_lines, (4, 5)); // line 4

        // THEIRS doc: header | their line | middle | their2 a | their2 b | their2 c | footer
        //             line 0   line 1       line 2   line 3     line 4     line 5      line 6
        assert_eq!(conflicts[0].theirs_lines, (1, 2)); // line 1
        assert_eq!(conflicts[1].theirs_lines, (3, 6)); // lines 3..6
    }

    #[test]
    fn test_conflict_resolution() {
        let mut hunk = ConflictHunk::new(0, 5, "ours".to_string(), "theirs".to_string(), None);

        assert!(hunk.resolved_content().is_none());

        hunk.resolution = ConflictResolution::AcceptOurs;
        assert_eq!(hunk.resolved_content(), Some("ours".to_string()));

        hunk.resolution = ConflictResolution::AcceptTheirs;
        assert_eq!(hunk.resolved_content(), Some("theirs".to_string()));

        hunk.resolution = ConflictResolution::AcceptBoth;
        assert_eq!(hunk.resolved_content(), Some("ours\ntheirs".to_string()));
    }
}
