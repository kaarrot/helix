use super::*;

#[cfg(feature = "git")]
use helix_vcs::FileChange;
use helix_view::editor::MergeViewState;

pub fn changed_file_picker(cx: &mut Context) {
    #[cfg(not(feature = "git"))]
    {
        cx.editor.set_error("git support not compiled in");
        return;
    }

    #[cfg(feature = "git")]
    {
        pub struct FileChangeData {
            cwd: PathBuf,
            style_untracked: Style,
            style_modified: Style,
            style_conflict: Style,
            style_deleted: Style,
            style_renamed: Style,
        }

        let cwd = helix_stdx::env::current_working_dir();
        if !cwd.exists() {
            cx.editor
                .set_error("Current working directory does not exist");
            return;
        }

        // Get the current file path for pre-selection in the picker
        let current_file_path = {
            let (_, doc) = current!(cx.editor);
            doc.path().map(|p| p.to_path_buf())
        };

        // Get diff range from editor state or default to HEAD vs working tree
        let diff_range = cx.editor.diff.range.clone().unwrap_or_else(|| DiffRange {
            base_ref: "HEAD".to_string(),
            target_ref: None,
        });

        let base_ref = diff_range.base_ref.clone();
        let target_ref = diff_range.target_ref.clone();

        cx.editor
            .set_status(format!("Loading changes for: {}", diff_range.display()));

        let added = cx.editor.theme.get("diff.plus");
        let modified = cx.editor.theme.get("diff.delta");
        let conflict = cx.editor.theme.get("diff.delta.conflict");
        let deleted = cx.editor.theme.get("diff.minus");
        let renamed = cx.editor.theme.get("diff.delta.moved");

        let columns = [
            PickerColumn::new("change", |change: &FileChange, data: &FileChangeData| {
                match change {
                    FileChange::Untracked { .. } => {
                        Span::styled("+ untracked", data.style_untracked)
                    }
                    FileChange::Modified { .. } => Span::styled("~ modified", data.style_modified),
                    FileChange::Conflict { .. } => Span::styled("x conflict", data.style_conflict),
                    FileChange::Deleted { .. } => Span::styled("- deleted", data.style_deleted),
                    FileChange::Renamed { .. } => Span::styled("> renamed", data.style_renamed),
                }
                .into()
            }),
            PickerColumn::new("path", |change: &FileChange, data: &FileChangeData| {
                let display_path = |path: &PathBuf| {
                    path.strip_prefix(&data.cwd)
                        .unwrap_or(path)
                        .display()
                        .to_string()
                };
                match change {
                    FileChange::Untracked { path } => display_path(path),
                    FileChange::Modified { path } => display_path(path),
                    FileChange::Conflict { path } => display_path(path),
                    FileChange::Deleted { path } => display_path(path),
                    FileChange::Renamed { from_path, to_path } => {
                        format!("{} -> {}", display_path(from_path), display_path(to_path))
                    }
                }
                .into()
            }),
        ];

        let picker = Picker::new(
            columns,
            1, // path
            [],
            FileChangeData {
                cwd: cwd.clone(),
                style_untracked: added,
                style_modified: modified,
                style_conflict: conflict,
                style_deleted: deleted,
                style_renamed: renamed,
            },
            {
                let base_ref_cb = base_ref.clone();
                let target_ref_cb = target_ref.clone();
                move |cx, meta: &FileChange, _action| {
                    cx.editor.diff.last_changed_file_selection = Some(meta.path().to_path_buf());
                    let path = meta.path();
                    // Conflict files open the 3-way merge view directly
                    if matches!(meta, FileChange::Conflict { .. }) {
                        if let Err(e) = cx.editor.open_merge_view(path) {
                            cx.editor
                                .set_error(format!("Failed to open merge view: {e}"));
                        }
                        return;
                    }
                    if let Err(e) =
                        cx.editor
                            .open_diff_view_range(path, &base_ref_cb, target_ref_cb.as_deref(), None)
                    {
                        cx.editor
                            .set_error(format!("Failed to open diff view: {e}"));
                    }
                }
            },
        )
        .with_preview(|_editor, meta| Some((meta.path().into(), None)));

        let picker = picker.with_pre_select(move |file_change: &FileChange| {
            current_file_path
                .as_ref()
                .map_or(false, |p| file_change.path() == p)
        });

        let injector = picker.injector();

        #[cfg(feature = "integration")]
        {
            use helix_vcs::git;
            let _ = git::for_each_changed_file_between_refs(
                &cwd,
                &base_ref,
                target_ref.as_deref(),
                move |change| match change {
                    Ok(change) => injector.push(change).is_ok(),
                    Err(_) => true,
                },
            );
        }

        #[cfg(not(feature = "integration"))]
        std::thread::spawn(move || {
            use helix_vcs::git;
            let _ = git::for_each_changed_file_between_refs(
                &cwd,
                &base_ref,
                target_ref.as_deref(),
                move |change| match change {
                    Ok(change) => injector.push(change).is_ok(),
                    Err(_) => true,
                },
            );
        });

        cx.push_layer(Box::new(overlaid(picker)));
    }
}

pub fn close_diff_or_merge_view(cx: &mut Context) {
    let view_id = view!(cx.editor).id;
    let closed = cx.editor.close_diff_view(view_id) || cx.editor.close_merge_view(view_id);
    if !closed {
        cx.editor.set_error("Not in a diff or merge view");
    }
}

#[derive(Copy, Clone)]
enum AcceptSide {
    Ours,
    Theirs,
    Both,
}

impl AcceptSide {
    fn pick(self, ours: &str, theirs: &str, both: &str) -> String {
        match self {
            AcceptSide::Ours => ours.to_string(),
            AcceptSide::Theirs => theirs.to_string(),
            AcceptSide::Both => both.to_string(),
        }
    }
    fn label(self) -> &'static str {
        match self {
            AcceptSide::Ours => "HEAD",
            AcceptSide::Theirs => "incoming",
            AcceptSide::Both => "both",
        }
    }
}

pub fn merge_accept_ours(cx: &mut Context) {
    merge_accept_impl(cx, AcceptSide::Ours);
}

pub fn merge_accept_theirs(cx: &mut Context) {
    merge_accept_impl(cx, AcceptSide::Theirs);
}

pub fn merge_accept_both(cx: &mut Context) {
    merge_accept_impl(cx, AcceptSide::Both);
}

fn merge_accept_impl(cx: &mut Context, side: AcceptSide) {
    use helix_view::merge_view::{extract_sides, find_conflicts, LastResolution};

    let view_id = view!(cx.editor).id;
    let doc_id = doc!(cx.editor).id();

    // 1. If we just resolved a conflict and the cursor is still inside the
    //    inserted replacement, replace it again with the new side instead of
    //    hunting for markers that no longer exist.
    let prior = cx
        .editor
        .diff
        .merge_views
        .get(&view_id)
        .and_then(|s| s.last_resolution.clone());
    if let Some(prior) = prior {
        let (view, doc) = current!(cx.editor);
        let cursor = doc
            .selection(view.id)
            .primary()
            .cursor(doc.text().slice(..));
        if cursor >= prior.start_char && cursor <= prior.end_char {
            let replacement = side.pick(&prior.ours, &prior.theirs, &prior.both);
            let new_end = prior.start_char + replacement.chars().count();
            let text = doc.text();
            let tx = helix_core::Transaction::change(
                text,
                std::iter::once((
                    prior.start_char,
                    prior.end_char,
                    Some(replacement.clone().into()),
                )),
            );
            doc.apply(&tx, view_id);
            let new_resolution = LastResolution {
                start_char: prior.start_char,
                end_char: new_end,
                ours: prior.ours,
                theirs: prior.theirs,
                both: prior.both,
            };
            update_last_resolutions(
                &mut cx.editor.diff.merge_views,
                doc_id,
                Some(new_resolution),
            );
            cx.editor
                .set_status(format!("Switched to {} version", side.label()));
            return;
        }
    }

    // 2. Otherwise find a real conflict at cursor and resolve it.
    let text = doc!(cx.editor).text().clone();
    let cursor_line = {
        let (view, doc) = current_ref!(cx.editor);
        doc.selection(view.id)
            .primary()
            .cursor_line(doc.text().slice(..))
    };

    let Some(region) = find_conflicts(&text)
        .into_iter()
        .find(|r| cursor_line >= r.start_line && cursor_line <= r.end_line)
    else {
        cx.editor.set_status("No conflict at cursor");
        return;
    };

    let Some((ours, _, theirs)) = extract_sides(&text, &region) else {
        cx.editor.set_error("Malformed conflict markers");
        return;
    };
    let both = format!("{}{}", ours, theirs);
    let replacement = side.pick(&ours, &theirs, &both);

    let (view, doc) = current!(cx.editor);
    let text = doc.text();
    let start_char = text.line_to_char(region.start_line);
    let end_char = if region.end_line + 1 < text.len_lines() {
        text.line_to_char(region.end_line + 1)
    } else {
        text.len_chars()
    };
    let new_end = start_char + replacement.chars().count();
    let tx = helix_core::Transaction::change(
        text,
        std::iter::once((start_char, end_char, Some(replacement.clone().into()))),
    );
    doc.apply(&tx, view.id);
    let new_resolution = LastResolution {
        start_char,
        end_char: new_end,
        ours,
        theirs,
        both,
    };
    update_last_resolutions(
        &mut cx.editor.diff.merge_views,
        doc_id,
        Some(new_resolution),
    );
    cx.editor
        .set_status(format!("Accepted {} version", side.label()));
}

/// Update `last_resolution` on every MergeViewState entry tied to the given
/// result doc. Passing `None` clears the resolution (e.g. on conflict navigation).
fn update_last_resolutions(
    merge_views: &mut std::collections::HashMap<ViewId, MergeViewState>,
    result_doc_id: DocumentId,
    value: Option<helix_view::merge_view::LastResolution>,
) {
    for state in merge_views.values_mut() {
        if state.result_doc_id == result_doc_id {
            state.last_resolution = value.clone();
        }
    }
}

pub fn merge_next_conflict(cx: &mut Context) {
    merge_jump_impl(cx, Direction::Forward);
}

pub fn merge_prev_conflict(cx: &mut Context) {
    merge_jump_impl(cx, Direction::Backward);
}

fn merge_jump_impl(cx: &mut Context, direction: Direction) {
    use helix_view::merge_view::find_conflicts;
    let (view, doc) = current!(cx.editor);
    let view_id = view.id;
    let doc_id = doc.id();
    let cursor_line = doc
        .selection(view_id)
        .primary()
        .cursor_line(doc.text().slice(..));
    let text = doc.text().clone();
    let conflicts = find_conflicts(&text);

    // Moving to a new conflict invalidates the switch-without-undo shortcut.
    update_last_resolutions(&mut cx.editor.diff.merge_views, doc_id, None);

    if conflicts.is_empty() {
        cx.editor.set_status("No conflicts remaining");
        return;
    }

    let total = conflicts.len();
    let target_start = match direction {
        Direction::Forward => conflicts
            .iter()
            .find(|r| r.start_line > cursor_line)
            .or_else(|| conflicts.first())
            .map(|r| r.start_line),
        Direction::Backward => conflicts
            .iter()
            .rev()
            .find(|r| r.start_line < cursor_line)
            .or_else(|| conflicts.last())
            .map(|r| r.start_line),
    };

    if let Some(start_line) = target_start {
        let pos = doc.text().line_to_char(start_line);
        let (view, doc) = current!(cx.editor);
        let view_id = view.id;
        doc.set_selection(view_id, helix_core::Selection::point(pos));
        cx.editor.ensure_cursor_in_view(view_id);

        let current_idx = conflicts
            .iter()
            .position(|r| r.start_line == start_line)
            .map(|i| i + 1)
            .unwrap_or(1);
        cx.editor
            .set_status(format!("Conflict {}/{}", current_idx, total));
    }
}

pub fn merge_finish(cx: &mut Context) {
    use helix_view::merge_view::conflict_count;
    let (_, doc) = current!(cx.editor);
    let remaining = conflict_count(doc.text());

    if remaining > 0 {
        cx.editor
            .set_error(format!("{} conflict(s) still unresolved", remaining));
        return;
    }

    let doc_id = doc.id();
    let path = doc.path().map(|p| p.to_path_buf());

    if let Err(e) = cx.editor.save::<PathBuf>(doc_id, None, false) {
        cx.editor.set_error(format!("Failed to save: {e}"));
        return;
    }

    // Stage with git add
    if let Some(p) = path {
        let _ = std::process::Command::new("git")
            .args(["add", "--"])
            .arg(&p)
            .status();
    }

    cx.editor.set_status("File saved and staged (git add)");
}

pub fn diff_toggle_sync_scroll(cx: &mut Context) {
    let view_id = view!(cx.editor).id;
    let Some(diff_state) = cx.editor.diff.views.get(&view_id).cloned() else {
        cx.editor.set_error("Not in a diff view");
        return;
    };
    let new_val = !diff_state.sync_scroll;
    for id in [diff_state.base_view_id, diff_state.working_view_id] {
        if let Some(state) = cx.editor.diff.views.get_mut(&id) {
            state.sync_scroll = new_val;
        }
    }
    cx.editor.set_status(if new_val {
        "Sync scroll enabled"
    } else {
        "Sync scroll disabled"
    });
}

pub fn diff_toggle_split_view(cx: &mut Context) {
    let view_id = view!(cx.editor).id;
    let Some(diff_state) = cx.editor.diff.views.get(&view_id).cloned() else {
        cx.editor.set_error("Not in a diff view");
        return;
    };
    let was_split = diff_state.base_view_id != diff_state.working_view_id;
    let new_split = !was_split;

    let base_path = diff_state.base_path.clone();
    let working_path = diff_state.working_path.clone();
    let base_ref = diff_state.base_ref.clone();
    let target_ref = diff_state.target_ref.clone();

    if base_path.as_os_str().is_empty() || working_path.as_os_str().is_empty() {
        cx.editor
            .set_error("Can't toggle: diff view missing reopen paths");
        return;
    }

    cx.editor.diff.split_view_override = Some(new_split);
    cx.editor.close_diff_view(view_id);

    let result = cx.editor.open_diff_view_range_with_paths(
        &base_path,
        &working_path,
        &base_ref,
        target_ref.as_deref(),
        Some(new_split),
    );
    if let Err(e) = result {
        cx.editor
            .set_error(format!("Failed to toggle split view: {e}"));
        return;
    }
    cx.editor.set_status(if new_split {
        "Split view on"
    } else {
        "Split view off"
    });
}

pub fn diff_reset(cx: &mut Context) {
    let view_id = view!(cx.editor).id;
    if cx.editor.diff.views.contains_key(&view_id) {
        cx.editor.close_diff_view(view_id);
    }
    cx.editor.diff.range = None;
    let diff_docs: Vec<_> = cx
        .editor
        .documents
        .iter()
        .filter_map(|(id, doc)| doc.path().map(|_| *id))
        .collect();
    for (_, doc) in cx.editor.documents.iter_mut() {
        doc.char_diff_enabled = false;
    }
    for id in diff_docs {
        if let Some(doc) = cx.editor.documents.get_mut(&id) {
            let _ = doc.reset_diff_base();
        }
    }
    cx.editor.set_status("Diff reset to HEAD");
}

/// Extract a leading git-hash-like token from a line of text.
fn extract_commit_hash(line: &str) -> Option<&str> {
    let token = line.split_whitespace().next()?;
    let is_hash = (7..=40).contains(&token.len()) && token.chars().all(|c| c.is_ascii_hexdigit());
    is_hash.then_some(token)
}

pub fn diff_commit_from_selection(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let text = doc.text().slice(..);
    let selected = doc.selection(view.id).primary().fragment(text).to_string();

    let lines: Vec<&str> = selected.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        cx.editor.set_error("No selection");
        return;
    }

    if lines.len() == 1 {
        let Some(hash) = extract_commit_hash(lines[0]) else {
            cx.editor.set_error("Invalid commit hash in selection");
            return;
        };
        let hash = hash.to_string();
        cx.editor.diff.range = Some(DiffRange {
            base_ref: hash.clone(),
            target_ref: None,
        });
        cx.editor
            .set_status(format!("Diff set: {} vs working tree", hash));
    } else {
        // In git log output, the first line is the newer commit; diff goes OLD..NEW.
        let (Some(newer), Some(older)) = (
            extract_commit_hash(lines[0]),
            extract_commit_hash(lines[lines.len() - 1]),
        ) else {
            cx.editor.set_error("Invalid commit hash in selection");
            return;
        };
        let (older, newer) = (older.to_string(), newer.to_string());
        cx.editor.diff.range = Some(DiffRange {
            base_ref: older.clone(),
            target_ref: Some(newer.clone()),
        });
        cx.editor
            .set_status(format!("Diff set: {}..{}", older, newer));
    }
}

pub fn diff_show_commit_from_selection(cx: &mut Context) {
    let (view, doc) = current_ref!(cx.editor);
    let text = doc.text().slice(..);
    let selected = doc.selection(view.id).primary().fragment(text).to_string();

    let lines: Vec<&str> = selected.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.len() != 1 {
        cx.editor.set_error("Select exactly one commit line");
        return;
    }
    let Some(hash) = extract_commit_hash(lines[0]) else {
        cx.editor.set_error("Invalid commit hash in selection");
        return;
    };
    let hash = hash.to_string();
    cx.editor.diff.range = Some(DiffRange {
        base_ref: format!("{}^", hash),
        target_ref: Some(hash.clone()),
    });
    cx.editor
        .set_status(format!("Diff set: showing changes in {}", hash));
}
