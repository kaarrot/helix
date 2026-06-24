use crate::{
    editor::{DiffRange, MergeViewState},
    DocumentId, ViewId,
};
use helix_vcs::FileChange;
use std::{collections::HashMap, path::PathBuf};

/// All diff/merge runtime state aggregated into one Editor field.
#[derive(Debug, Default)]
pub struct DiffSession {
    pub views: HashMap<ViewId, DiffViewState>,
    pub merge_views: HashMap<ViewId, MergeViewState>,
    pub range: Option<DiffRange>,
    pub split_view_override: Option<bool>,
    pub last_changed_file_selection: Option<PathBuf>,
    /// Last computed changed-file listing, kept so the changed-file picker
    /// (`space g`) can display instantly on reopen instead of re-scanning the
    /// repo every time. This is only a display seed: the picker always
    /// recomputes in the background and reconciles, so a stale cache can never
    /// produce a wrong result — only a brief out-of-date flash on a huge repo.
    pub changed_file_cache: Option<ChangedFileCache>,
    /// Incremented every time the changed-file picker opens. A background
    /// refresh captures the value at launch and only touches the picker if it
    /// still matches, so a slow refresh from an earlier invocation can't
    /// clobber a picker that was reopened in the meantime.
    pub changed_file_request: u64,
}

/// A changed-file listing cached together with the diff range it was computed
/// for, so the cache can be reused only when the range still matches.
#[derive(Debug)]
pub struct ChangedFileCache {
    pub base_ref: String,
    pub target_ref: Option<String>,
    pub files: Vec<FileChange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffViewSource {
    Git,
    Buffers,
}

#[derive(Debug, Clone)]
pub struct DiffViewState {
    pub base_doc_id: DocumentId,
    pub working_doc_id: DocumentId,
    pub base_view_id: ViewId,
    pub working_view_id: ViewId,
    pub sync_scroll: bool,
    pub git_ref: String,
    pub source: DiffViewSource,
    /// Reopen recipe: paths and refs so :diff-toggle-split can rebuild the view.
    pub base_path: PathBuf,
    pub working_path: PathBuf,
    pub base_ref: String,
    pub target_ref: Option<String>,
    pub close_base_doc_on_close: bool,
    pub close_working_doc_on_close: bool,
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
            source: DiffViewSource::Git,
            base_path: PathBuf::new(),
            working_path: PathBuf::new(),
            base_ref: git_ref,
            target_ref: None,
            close_base_doc_on_close: false,
            close_working_doc_on_close: false,
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

    pub fn with_buffer_reopen(mut self) -> Self {
        self.source = DiffViewSource::Buffers;
        self
    }

    pub fn close_base_doc_on_close(mut self) -> Self {
        self.close_base_doc_on_close = true;
        self
    }

    pub fn close_working_doc_on_close(mut self) -> Self {
        self.close_working_doc_on_close = true;
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
