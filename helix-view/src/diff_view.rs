use crate::{DocumentId, ViewId};

#[derive(Debug, Clone)]
pub struct DiffViewState {
    pub base_doc_id: DocumentId,
    pub working_doc_id: DocumentId,
    pub base_view_id: ViewId,
    pub working_view_id: ViewId,
    pub sync_scroll: bool,
    pub git_ref: String,
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
            git_ref,
        }
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
