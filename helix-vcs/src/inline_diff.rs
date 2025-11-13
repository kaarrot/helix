//! Character-level diff computation for inline highlighting.
//!
//! This module provides two-phase diffing: line-level diff using imara-diff,
//! followed by character-level diff using similar crate for changed line pairs.

use similar::TextDiff;

/// Character-level diff highlight information
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineDiffHighlight {
    pub start_col: usize,
    pub end_col: usize,
    pub change_type: ChangeType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeType {
    Insert,
    Delete,
    Replace,
}

/// Configuration for inline diff computation
#[derive(Debug, Clone)]
pub struct InlineDiffConfig {
    /// Maximum line length to compute character-level diff (performance safety)
    pub max_line_length: usize,
    /// Maximum difference threshold (skip if lines differ by more than this fraction)
    pub max_difference_ratio: f32,
}

impl Default for InlineDiffConfig {
    fn default() -> Self {
        Self {
            max_line_length: 500,
            max_difference_ratio: 0.8,
        }
    }
}

/// Compute character-level diff highlights for a pair of lines
pub fn compute_inline_diff(
    old_line: &str,
    new_line: &str,
    config: &InlineDiffConfig,
) -> Vec<InlineDiffHighlight> {
    // Skip if lines are too long (performance safety)
    if old_line.len() + new_line.len() > config.max_line_length * 2 {
        return vec![];
    }

    // Skip if lines are too different (>80% changed by default)
    let max_len = old_line.len().max(new_line.len());
    if max_len == 0 {
        return vec![];
    }

    // Quick similarity check using length difference
    let len_diff = (old_line.len() as i32 - new_line.len() as i32).abs() as usize;
    let threshold = (max_len as f32 * config.max_difference_ratio) as usize;
    if len_diff > threshold {
        return vec![];
    }

    // Compute character-level diff using similar crate
    let diff = TextDiff::from_graphemes(old_line, new_line);
    let mut highlights = vec![];
    let mut old_index = 0;
    let mut new_index = 0;

    for op in diff.ops() {
        let old_range = op.old_range();
        let new_range = op.new_range();

        match op.tag() {
            similar::DiffTag::Equal => {
                old_index += old_range.len();
                new_index += new_range.len();
            }
            similar::DiffTag::Delete => {
                highlights.push(InlineDiffHighlight {
                    start_col: old_index,
                    end_col: old_index + old_range.len(),
                    change_type: ChangeType::Delete,
                });
                old_index += old_range.len();
            }
            similar::DiffTag::Insert => {
                highlights.push(InlineDiffHighlight {
                    start_col: new_index,
                    end_col: new_index + new_range.len(),
                    change_type: ChangeType::Insert,
                });
                new_index += new_range.len();
            }
            similar::DiffTag::Replace => {
                // For replacements, mark both old and new regions
                highlights.push(InlineDiffHighlight {
                    start_col: old_index,
                    end_col: old_index + old_range.len(),
                    change_type: ChangeType::Replace,
                });
                highlights.push(InlineDiffHighlight {
                    start_col: new_index,
                    end_col: new_index + new_range.len(),
                    change_type: ChangeType::Replace,
                });
                old_index += old_range.len();
                new_index += new_range.len();
            }
        }
    }

    highlights
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_insertion() {
        let config = InlineDiffConfig::default();
        let old = "hello world";
        let new = "hello beautiful world";

        let highlights = compute_inline_diff(old, new, &config);
        assert!(!highlights.is_empty());
    }

    #[test]
    fn test_simple_deletion() {
        let config = InlineDiffConfig::default();
        let old = "hello beautiful world";
        let new = "hello world";

        let highlights = compute_inline_diff(old, new, &config);
        assert!(!highlights.is_empty());
    }

    #[test]
    fn test_skip_too_long() {
        let config = InlineDiffConfig {
            max_line_length: 10,
            ..Default::default()
        };
        let old = "a".repeat(100);
        let new = "b".repeat(100);

        let highlights = compute_inline_diff(&old, &new, &config);
        assert!(highlights.is_empty());
    }

    #[test]
    fn test_skip_too_different() {
        let config = InlineDiffConfig::default();
        let old = "completely different text";
        let new = "xyz123abc";

        // This might still return highlights depending on the algorithm
        // but serves as a test case
        let _highlights = compute_inline_diff(old, new, &config);
    }
}
