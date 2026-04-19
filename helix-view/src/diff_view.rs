use crate::{editor::{DiffRange, MergeViewState}, DocumentId, ViewId};
use std::{collections::HashMap, path::PathBuf};

/// All diff/merge runtime state aggregated into one Editor field.
#[derive(Debug, Default)]
pub struct DiffSession {
    pub views: HashMap<ViewId, DiffViewState>,
    pub merge_views: HashMap<ViewId, MergeViewState>,
    pub range: Option<DiffRange>,
    pub split_view_override: Option<bool>,
    pub last_changed_file_selection: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct DiffViewState {
    pub base_doc_id: DocumentId,
    pub working_doc_id: DocumentId,
    pub base_view_id: ViewId,
    pub working_view_id: ViewId,
    pub sync_scroll: bool,
    pub git_ref: String,
    /// Reopen recipe: paths and refs so :diff-toggle-split can rebuild the view.
    pub base_path: PathBuf,
    pub working_path: PathBuf,
    pub base_ref: String,
    pub target_ref: Option<String>,
}

impl DiffViewState {
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
            git_ref: git_ref.clone(),
            base_path: PathBuf::new(),
            working_path: PathBuf::new(),
            base_ref: git_ref,
            target_ref: None,
        }
    }

    pub fn with_reopen(
        mut self,
        base_path: PathBuf,
        working_path: PathBuf,
        base_ref: String,
        target_ref: Option<String>,
    ) -> Self {
        self.base_path = base_path;
        self.working_path = working_path;
        self.base_ref = base_ref;
        self.target_ref = target_ref;
        self
    }

    pub fn contains_view(&self, view_id: ViewId) -> bool {
        self.base_view_id == view_id || self.working_view_id == view_id
    }

    pub fn paired_view(&self, view_id: ViewId) -> Option<ViewId> {
        if view_id == self.base_view_id {
            Some(self.working_view_id)
        } else if view_id == self.working_view_id {
            Some(self.base_view_id)
        } else {
            None
        }
    }

    pub fn is_base_view(&self, view_id: ViewId) -> bool {
        view_id == self.base_view_id
    }

    pub fn is_working_view(&self, view_id: ViewId) -> bool {
        view_id == self.working_view_id
    }
}
