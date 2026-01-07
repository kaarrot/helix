//! Side-by-side diff view state management.
//!
//! This module provides structures for managing linked diff views
//! where a read-only git revision is shown alongside the editable working copy.

use crate::{DocumentId, ViewId};

/// Represents a linked side-by-side diff view session.
///
/// When active, two views are linked together:
/// - The base view shows a read-only git revision
/// - The working view shows the editable working copy
///
/// Synchronized scrolling keeps both views aligned.
#[derive(Debug, Clone)]
pub struct DiffViewState {
    /// The base document ID (read-only git revision)
    pub base_doc_id: DocumentId,
    /// The working document ID (editable)
    pub working_doc_id: DocumentId,
    /// The view showing the base revision
    pub base_view_id: ViewId,
    /// The view showing the working copy
    pub working_view_id: ViewId,
    /// Whether synchronized scrolling is enabled
    pub sync_scroll: bool,
    /// Git reference being compared against (e.g., "HEAD", "main", commit hash)
    pub git_ref: String,
}

impl DiffViewState {
    /// Create a new diff view state.
    pub fn new(
        base_doc_id: DocumentId,
        working_doc_id: DocumentId,
        base_view_id: ViewId,
        working_view_id: ViewId,
        git_ref: String,
    ) -> Self {
        Self {
            base_doc_id,
            working_doc_id,
            base_view_id,
            working_view_id,
            sync_scroll: true,
            git_ref,
        }
    }

    /// Check if a given view is part of this diff session.
    pub fn contains_view(&self, view_id: ViewId) -> bool {
        self.base_view_id == view_id || self.working_view_id == view_id
    }

    /// Get the paired view ID for a given view in this diff session.
    pub fn paired_view(&self, view_id: ViewId) -> Option<ViewId> {
        if view_id == self.base_view_id {
            Some(self.working_view_id)
        } else if view_id == self.working_view_id {
            Some(self.base_view_id)
        } else {
            None
        }
    }

    /// Check if the given view is the base (read-only) view.
    pub fn is_base_view(&self, view_id: ViewId) -> bool {
        view_id == self.base_view_id
    }

    /// Check if the given view is the working (editable) view.
    pub fn is_working_view(&self, view_id: ViewId) -> bool {
        view_id == self.working_view_id
    }
}
