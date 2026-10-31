use super::*;

#[cfg(feature = "git")]
use helix_vcs::FileChange;
use helix_view::editor::MergeViewState;

/// Per-item data shared with the changed-file picker's column formatters.
/// Hoisted to module scope (rather than nested in `changed_file_picker`) so the
/// background-refresh callback can name `Picker<FileChange, FileChangeData>`
/// when looking the picker back up in the compositor.
#[cfg(feature = "git")]
pub struct FileChangeData {
    cwd: PathBuf,
    style_untracked: Style,
    style_modified: Style,
    style_conflict: Style,
    style_deleted: Style,
    style_renamed: Style,
}

/// Collect the full changed-file listing for a diff range into a `Vec`.
/// This is the expensive part (a tree diff and/or a working-tree scan), so it
/// is run off the main thread by `changed_file_picker`.
///
/// Tracked changes and untracked files are gathered separately: tracked changes
/// come from gix (fast index<->worktree pass), untracked files from a parallel
/// walker. [`spawn_changed_file_refresh`] uses the same two-phase split to
/// stream tracked changes into the picker before the untracked walk finishes.
#[cfg(feature = "git")]
fn collect_changed_files(
    cwd: &std::path::Path,
    base_ref: &str,
    target_ref: Option<&str>,
) -> anyhow::Result<Vec<FileChange>> {
    scan_changed_files(cwd, base_ref, target_ref, |_| {})
}

#[cfg(feature = "git")]
fn scan_changed_files(
    cwd: &Path,
    base_ref: &str,
    target_ref: Option<&str>,
    emit: impl Fn(&FileChange),
) -> anyhow::Result<Vec<FileChange>> {
    use helix_vcs::git;
    // The callbacks take an `Fn`, so accumulate through interior mutability
    // rather than capturing the `Vec` mutably.
    let files = std::cell::RefCell::new(Vec::new());
    let error = std::cell::RefCell::new(None);
    git::for_each_tracked_change(cwd, base_ref, target_ref, |change| match change {
        Ok(change) => {
            emit(&change);
            files.borrow_mut().push(change);
            true
        }
        Err(err) => {
            *error.borrow_mut() = Some(err);
            false
        }
    })?;
    if let Some(err) = error.into_inner() {
        return Err(err);
    }
    let mut files = files.into_inner();
    // Untracked files only exist when comparing against the working tree.
    if target_ref.is_none() {
        let untracked = std::sync::Mutex::new(Vec::new());
        git::for_each_untracked_file(cwd, |change| {
            untracked.lock().unwrap().push(change);
        })?;
        let mut untracked = untracked.into_inner().unwrap();
        sort_changes(&mut untracked);
        for change in &untracked {
            emit(change);
        }
        files.extend(untracked);
    }
    // A tracked scan can discover a rename or conflict after an earlier
    // modification for the same path. Keep its more informative identity.
    let mut by_path = std::collections::HashMap::new();
    for change in files {
        let existing = by_path
            .entry(change.path().to_path_buf())
            .or_insert_with(|| change.clone());
        if matches!(change, FileChange::Conflict { .. })
            || matches!(change, FileChange::Renamed { .. })
                && !matches!(existing, FileChange::Conflict { .. })
        {
            *existing = change;
        }
    }
    let renamed_sources: Vec<_> = by_path
        .values()
        .filter_map(|change| match change {
            FileChange::Renamed { from_path, .. } => Some(from_path.clone()),
            _ => None,
        })
        .collect();
    for path in renamed_sources {
        by_path.remove(&path);
    }
    let mut files: Vec<_> = by_path.into_values().collect();
    sort_changes(&mut files);
    Ok(files)
}

/// Sort a changed-file listing so the tracked changes the user is usually after
/// (conflicts, modifications, renames, deletions) sit above the untracked
/// noise, then by path. The picker shows items in insertion order for an empty
/// query, so this ordering is what the user sees on open.
#[cfg(feature = "git")]
fn sort_changes(files: &mut [FileChange]) {
    files.sort_by(|a, b| {
        change_sort_rank(a)
            .cmp(&change_sort_rank(b))
            .then_with(|| a.path().cmp(b.path()))
    });
}

/// Ordering key for the changed-file picker: tracked changes before untracked.
#[cfg(feature = "git")]
fn change_sort_rank(change: &FileChange) -> u8 {
    match change {
        FileChange::Conflict { .. } => 0,
        FileChange::Modified { .. } => 1,
        FileChange::Renamed { .. } => 2,
        FileChange::Deleted { .. } => 3,
        FileChange::Untracked { .. } => 4,
    }
}

/// The list to seed the changed-file picker with on open. Under the
/// `integration` feature this is the freshly-computed listing (tests act on it
/// synchronously); otherwise it is the cached listing for this range (empty on
/// a cache miss), which [`spawn_changed_file_refresh`] reconciles in the
/// background.
#[cfg(feature = "git")]
pub(crate) fn changed_file_seed(
    editor: &Editor,
    cwd: &std::path::Path,
    base_ref: &str,
    target_ref: Option<&str>,
) -> anyhow::Result<std::sync::Arc<[FileChange]>> {
    #[cfg(feature = "integration")]
    {
        let _ = editor;
        Ok(collect_changed_files(cwd, base_ref, target_ref)?.into())
    }
    #[cfg(not(feature = "integration"))]
    {
        Ok(editor
            .diff
            .changed_file_cache
            .as_ref()
            .filter(|cache| {
                cache.cwd == cwd
                    && cache.base_ref == base_ref
                    && cache.target_ref.as_deref() == target_ref
            })
            .map(|cache| cache.files.clone())
            .unwrap_or_default())
    }
}

/// Build the changed-file picker, seeded with `seed` (shown instantly). Shared
/// by `space g` (`changed_file_picker`) and `:diff-files` so the two stay a
/// single implementation. Must run on the main thread: it reads the theme and
/// the current file, and the picker it returns is not `Send`.
///
/// Also returns the picker's injector so that on a cache miss (empty seed) the
/// background scan can stream files in as they are discovered, instead of the
/// picker staying empty until the whole scan finishes.
#[cfg(feature = "git")]
pub(crate) fn changed_file_picker_component(
    editor: &mut Editor,
    cwd: PathBuf,
    base_ref: String,
    target_ref: Option<String>,
    seed: std::sync::Arc<[FileChange]>,
) -> (
    Box<dyn Component>,
    crate::ui::picker::Injector<FileChange, FileChangeData>,
) {
    let current_file_path = {
        let (_, doc) = current_ref!(editor);
        doc.git_revision
            .as_ref()
            .map(|rev| rev.path.clone())
            .or_else(|| doc.path().cloned())
            .or_else(|| editor.diff.last_changed_file_selection.clone())
    };

    let added = editor.theme.get("diff.plus");
    let modified = editor.theme.get("diff.delta");
    let conflict = editor.theme.get("diff.delta.conflict");
    let deleted = editor.theme.get("diff.minus");
    let renamed = editor.theme.get("diff.delta.moved");

    let columns = [
        PickerColumn::new("change", |change: &FileChange, data: &FileChangeData| {
            match change {
                FileChange::Untracked { .. } => Span::styled("+ untracked", data.style_untracked),
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
        seed.iter().cloned(),
        FileChangeData {
            cwd,
            style_untracked: added,
            style_modified: modified,
            style_conflict: conflict,
            style_deleted: deleted,
            style_renamed: renamed,
        },
        move |cx, meta: &FileChange, _action| {
            if let Err(err) =
                open_changed_file(cx.editor, meta, &base_ref, target_ref.as_deref(), None)
            {
                cx.editor
                    .set_error(format!("Cannot open comparison: {err:#}"));
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
    (Box::new(overlaid(picker)), injector)
}

#[cfg(feature = "git")]
fn open_changed_file(
    editor: &mut Editor,
    change: &FileChange,
    base_ref: &str,
    target_ref: Option<&str>,
    split: Option<bool>,
) -> anyhow::Result<()> {
    let path = change.path();
    if matches!(change, FileChange::Conflict { .. }) {
        editor.open_merge_view(path)?;
        let result_view = editor.tree.focus;
        for state in editor
            .diff
            .merge_views
            .values_mut()
            .filter(|state| state.result_view_id == result_view)
        {
            state.review_range = Some(DiffRange {
                base_ref: base_ref.into(),
                target_ref: target_ref.map(str::to_owned),
            });
        }
    } else {
        let detected_source = if target_ref.is_none()
            && matches!(
                change,
                FileChange::Untracked { .. } | FileChange::Modified { .. }
            ) {
            helix_vcs::git::find_rename_source(path, base_ref)?
        } else {
            None
        };
        let base_path = match change {
            FileChange::Renamed { from_path, .. } => from_path.as_path(),
            _ => detected_source.as_deref().unwrap_or(path),
        };
        editor.open_diff_view_range_with_paths(base_path, path, base_ref, target_ref, split)?;
        editor.set_status(format!(
            "Diff: {base_ref} vs {} — space m v toggles split view",
            target_ref.unwrap_or("working tree")
        ));
    }
    editor.diff.last_changed_file_selection = Some(path.to_path_buf());
    Ok(())
}

pub fn diff_next_file(cx: &mut Context) {
    diff_file(cx, Direction::Forward);
}
pub fn diff_prev_file(cx: &mut Context) {
    diff_file(cx, Direction::Backward);
}

fn diff_file(cx: &mut Context, direction: Direction) {
    #[cfg(not(feature = "git"))]
    {
        let _ = direction;
        cx.editor.set_error("git support not compiled in");
    }
    #[cfg(feature = "git")]
    {
        let cwd = helix_stdx::env::current_working_dir();
        let view_id = cx.editor.tree.focus;
        let state = cx.editor.diff.views.get(&view_id);
        if state.is_some_and(|state| state.source == helix_view::diff_view::DiffViewSource::Buffers)
        {
            cx.editor
                .set_error("Buffer comparisons have no changed-file list");
            return;
        }
        let split = state.map(|state| state.is_split());
        let range = state
            .map(|state| DiffRange {
                base_ref: state.base_ref.clone(),
                target_ref: state.target_ref.clone(),
            })
            .or_else(|| {
                cx.editor
                    .diff
                    .merge_views
                    .get(&view_id)
                    .and_then(|state| state.review_range.clone())
            })
            .or_else(|| cx.editor.diff.range.clone())
            .unwrap_or_else(|| DiffRange {
                base_ref: "HEAD".into(),
                target_ref: None,
            });
        if let Err(err) = range.validate(&cwd) {
            cx.editor.set_error(format!("Cannot list changes: {err:#}"));
            return;
        }
        let doc = doc!(cx.editor);
        let current = doc
            .git_revision
            .as_ref()
            .map(|revision| revision.path.clone())
            .or_else(|| doc.path().cloned())
            .or_else(|| cx.editor.diff.last_changed_file_selection.clone());
        cx.editor.diff.changed_file_request = cx.editor.diff.changed_file_request.wrapping_add(1);
        let request = cx.editor.diff.changed_file_request;
        let base = range.base_ref.clone();
        let target = range.target_ref.clone();
        cx.jobs.callback(async move {
            let files = tokio::task::spawn_blocking(move || {
                collect_changed_files(&cwd, &base, target.as_deref())
            })
            .await?
            .map_err(|err| anyhow::anyhow!("Cannot list changes: {err:#}"))?;
            Ok(job::Callback::EditorCompositor(Box::new(
                move |editor, _| {
                    if editor.diff.changed_file_request != request {
                        return;
                    }
                    if files.is_empty() {
                        editor.set_status("No changed files in this comparison");
                        return;
                    }
                    let index = current
                        .as_ref()
                        .and_then(|path| files.iter().position(|change| change.path() == path));
                    let next = match (direction, index) {
                        (Direction::Forward, Some(index)) if index + 1 == files.len() => {
                            editor.set_status("Last changed file");
                            return;
                        }
                        (Direction::Backward, Some(0)) => {
                            editor.set_status("First changed file");
                            return;
                        }
                        (Direction::Forward, Some(index)) => index + 1,
                        (Direction::Backward, Some(index)) => index - 1,
                        (Direction::Forward, None) => 0,
                        (Direction::Backward, None) => files.len() - 1,
                    };
                    if let Err(err) = open_changed_file(
                        editor,
                        &files[next],
                        &range.base_ref,
                        range.target_ref.as_deref(),
                        split,
                    ) {
                        editor.set_error(format!("Cannot open comparison: {err:#}"));
                    } else {
                        editor.set_status(format!(
                            "File {}/{}: {}",
                            next + 1,
                            files.len(),
                            files[next].path().display()
                        ));
                    }
                },
            )))
        });
    }
}

/// Recompute the changed-file listing on a background thread, then on the main
/// thread refresh the cache and reconcile the open picker.
///
/// `injector` selects how the picker learns about the fresh listing:
/// - `Some` (cache miss — the picker opened empty): tracked changes are scanned
///   first and streamed in immediately (the fast index<->worktree pass), then
///   untracked files are enumerated with the parallel walker and streamed in
///   below them. So a modified file shows in milliseconds even when the
///   untracked walk takes seconds. Pushes are best-effort; the scan runs to
///   completion regardless so the cache is filled for the next open.
/// - `None` (cache hit — the cached listing is already on screen): the fresh
///   listing is swapped in only at the end, and only if it actually differs,
///   keeping the user's query and selection stable while they filter.
#[cfg(all(feature = "git", not(feature = "integration")))]
pub(crate) fn spawn_changed_file_refresh(
    cwd: PathBuf,
    base_ref: String,
    target_ref: Option<String>,
    seed: std::sync::Arc<[FileChange]>,
    request: u64,
    injector: Option<crate::ui::picker::Injector<FileChange, FileChangeData>>,
) {
    std::thread::spawn(move || {
        let streamed = injector.is_some();
        let fresh = scan_changed_files(&cwd, &base_ref, target_ref.as_deref(), |change| {
            if let Some(injector) = &injector {
                let _ = injector.push(change.clone());
            }
        });

        job::dispatch_blocking(move |editor, compositor| {
            // An old scan must not overwrite either a newer picker or its cache.
            if editor.diff.changed_file_request != request {
                return;
            }
            let fresh: std::sync::Arc<[FileChange]> = match fresh {
                Ok(files) => files.into(),
                Err(err) => {
                    editor.diff.changed_file_cache = None;
                    editor.set_error(format!("Cannot list changes: {err:#}"));
                    if let Some(overlay) = compositor
                        .find::<crate::ui::overlay::Overlay<Picker<FileChange, FileChangeData>>>()
                    {
                        overlay.content.replace_options(std::iter::empty());
                    }
                    return;
                }
            };
            editor.diff.changed_file_cache = Some(helix_view::diff_view::ChangedFileCache {
                cwd,
                base_ref,
                target_ref,
                files: fresh.clone(),
            });

            if fresh.is_empty() {
                editor.set_status("No changed files in this comparison");
            }
            // Nothing changed since the cached seed; leave the picker (and the
            // user's selection/query) untouched.
            if !streamed && fresh == seed {
                return;
            }
            if let Some(overlay) =
                compositor.find::<crate::ui::overlay::Overlay<Picker<FileChange, FileChangeData>>>()
            {
                overlay.content.replace_options(fresh.iter().cloned());
            }
        });
    });
}

pub fn changed_file_picker(cx: &mut Context) {
    #[cfg(not(feature = "git"))]
    {
        cx.editor.set_error("git support not compiled in");
        return;
    }

    #[cfg(feature = "git")]
    {
        let cwd = helix_stdx::env::current_working_dir();
        if !cwd.exists() {
            cx.editor
                .set_error("Current working directory does not exist");
            return;
        }

        // Resolve the diff range (defaults to HEAD vs working tree).
        let diff_range = cx.editor.diff.range.clone().unwrap_or_else(|| DiffRange {
            base_ref: "HEAD".to_string(),
            target_ref: None,
        });
        let base_ref = diff_range.base_ref.clone();
        let target_ref = diff_range.target_ref.clone();
        if let Err(err) = diff_range.validate(&cwd) {
            cx.editor.set_error(format!("Cannot list changes: {err:#}"));
            return;
        }
        cx.editor
            .set_status(format!("Loading changes for: {}", diff_range.display()));

        cx.editor.diff.changed_file_request = cx.editor.diff.changed_file_request.wrapping_add(1);
        #[cfg(not(feature = "integration"))]
        let request = cx.editor.diff.changed_file_request;

        let seed = match changed_file_seed(cx.editor, &cwd, &base_ref, target_ref.as_deref()) {
            Ok(seed) => seed,
            Err(err) => {
                cx.editor.set_error(format!("Cannot list changes: {err:#}"));
                return;
            }
        };

        let (component, injector) = changed_file_picker_component(
            cx.editor,
            cwd.clone(),
            base_ref.clone(),
            target_ref.clone(),
            seed.clone(),
        );
        cx.push_layer(component);

        #[cfg(not(feature = "integration"))]
        spawn_changed_file_refresh(
            cwd,
            base_ref,
            target_ref,
            seed.clone(),
            request,
            // Stream into the empty picker on a cache miss; reconcile the
            // already-visible cached listing silently on a hit.
            seed.is_empty().then_some(injector),
        );
        #[cfg(feature = "integration")]
        let _ = injector;
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
    Base,
}

impl AcceptSide {
    fn pick(self, ours: &str, base: Option<&str>, theirs: &str, both: &str) -> Option<String> {
        match self {
            AcceptSide::Ours => Some(ours.to_string()),
            AcceptSide::Theirs => Some(theirs.to_string()),
            AcceptSide::Both => Some(both.to_string()),
            AcceptSide::Base => base.map(str::to_owned),
        }
    }
    fn label(self) -> &'static str {
        match self {
            AcceptSide::Ours => "HEAD",
            AcceptSide::Theirs => "incoming",
            AcceptSide::Both => "both",
            AcceptSide::Base => "base",
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

pub fn merge_accept_base(cx: &mut Context) {
    merge_accept_impl(cx, AcceptSide::Base);
}
pub fn merge_accept_ours_and_next(cx: &mut Context) {
    merge_accept_and_next(cx, AcceptSide::Ours);
}
pub fn merge_accept_theirs_and_next(cx: &mut Context) {
    merge_accept_and_next(cx, AcceptSide::Theirs);
}
pub fn merge_accept_both_and_next(cx: &mut Context) {
    merge_accept_and_next(cx, AcceptSide::Both);
}

fn merge_accept_and_next(cx: &mut Context, side: AcceptSide) {
    let (_, doc_id) = merge_target(cx.editor);
    let version = cx.editor.document(doc_id).map(Document::version);
    merge_accept_impl(cx, side);
    if cx.editor.document(doc_id).map(Document::version) != version && !cx.editor.is_err() {
        merge_jump_impl(cx, Direction::Forward);
    }
}

pub fn merge_show_base(cx: &mut Context) {
    if let Err(err) = cx.editor.show_merge_base(cx.editor.tree.focus) {
        cx.editor.set_error(format!("{err:#}"));
    }
}

/// The document/view merge commands should edit. Inside a merge session this
/// is always RESULT, even when OURS/THEIRS is focused.
fn merge_target(editor: &Editor) -> (ViewId, DocumentId) {
    let view_id = view!(editor).id;
    if let Some(state) = editor.diff.merge_views.get(&view_id) {
        (state.result_view_id, state.result_doc_id)
    } else {
        (view_id, doc!(editor).id())
    }
}

fn merge_scan(editor: &Editor, doc_id: DocumentId) -> helix_view::merge_view::ConflictScan {
    let size = editor
        .diff
        .merge_views
        .values()
        .find(|state| state.result_doc_id == doc_id)
        .map_or(7, |state| state.marker_size);
    helix_view::merge_view::scan_conflicts_with_size(editor.documents[&doc_id].text(), size)
}

fn merge_accept_impl(cx: &mut Context, side: AcceptSide) {
    use helix_view::merge_view::{extract_sides, LastResolution};

    let current_view_id = view!(cx.editor).id;
    if let Err(err) = cx.editor.ensure_merge_inputs_current(current_view_id) {
        cx.editor.set_error(format!("{err:#}"));
        return;
    }
    let (target_view_id, target_doc_id) = merge_target(cx.editor);
    // Each accept is an undo step even when invoked from a read-only side.
    {
        let view = cx.editor.tree.get_mut(target_view_id);
        cx.editor
            .documents
            .get_mut(&target_doc_id)
            .unwrap()
            .append_changes_to_history(view);
    }
    let scan = merge_scan(cx.editor, target_doc_id);
    if scan.malformed {
        cx.editor
            .set_error("Malformed conflict markers; repair them before accepting a side");
        return;
    }
    if let Some(state) = cx
        .editor
        .diff
        .merge_views
        .get(&current_view_id)
        .cloned()
        .filter(|state| state.whole_file && scan.marker_lines == 0)
    {
        let ours = cx.editor.documents[&state.ours_doc_id].text().to_string();
        let theirs = cx.editor.documents[&state.theirs_doc_id].text().to_string();
        let base = state
            .base_doc_id
            .map(|id| cx.editor.documents[&id].text().to_string());
        let both = format!("{ours}{theirs}");
        let Some(replacement) = side.pick(&ours, base.as_deref(), &theirs, &both) else {
            cx.editor.set_error("This conflict has no base version");
            return;
        };
        let delete = matches!(side, AcceptSide::Ours) && state.ours_deleted
            || matches!(side, AcceptSide::Theirs) && state.theirs_deleted;
        let doc = cx.editor.documents.get_mut(&target_doc_id).unwrap();
        let tx = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(replacement.into()))].into_iter(),
        );
        if !doc.apply(&tx, target_view_id) {
            cx.editor.set_error("Cannot apply resolution");
            return;
        }
        let view = cx.editor.tree.get_mut(target_view_id);
        doc.append_changes_to_history(view);
        for entry in cx
            .editor
            .diff
            .merge_views
            .values_mut()
            .filter(|entry| entry.result_doc_id == target_doc_id)
        {
            entry.delete_result = delete;
        }
        cx.editor.set_status(if delete {
            "Accepted deletion; f removes and stages this file"
        } else {
            "Accepted whole-file version; f saves and stages this file"
        });
        return;
    }
    let prior = cx
        .editor
        .diff
        .merge_views
        .get(&current_view_id)
        .and_then(|s| s.last_resolution.clone());

    // 1. If we just resolved a conflict and the RESULT cursor is still inside
    //    the inserted replacement (and RESULT has not been edited since),
    //    replace it again with the new side instead of hunting for markers.
    if let Some(prior) = prior {
        let reuse = cx.editor.document(target_doc_id).is_some_and(|doc| {
            doc.selections().contains_key(&target_view_id)
                && prior.contains_cursor(
                    doc.selection(target_view_id)
                        .primary()
                        .cursor(doc.text().slice(..)),
                )
                && prior.doc_version == doc.version()
        });
        if reuse {
            let Some(replacement) = side.pick(
                &prior.ours,
                prior.base.as_deref(),
                &prior.theirs,
                &prior.both,
            ) else {
                cx.editor.set_error(
                    "Base is not included in these markers; space m B shows the index base",
                );
                return;
            };
            let new_end = prior.start_char + replacement.chars().count();
            let doc = doc_mut!(cx.editor, &target_doc_id);
            let tx = helix_core::Transaction::change(
                doc.text(),
                std::iter::once((prior.start_char, prior.end_char, Some(replacement.into()))),
            );
            if !doc.apply(&tx, target_view_id) {
                cx.editor.set_error("Cannot apply resolution");
                return;
            }
            doc.append_changes_to_history(cx.editor.tree.get_mut(target_view_id));
            let new_resolution = LastResolution {
                start_char: prior.start_char,
                end_char: new_end,
                ours: prior.ours,
                base: prior.base,
                theirs: prior.theirs,
                both: prior.both,
                doc_version: doc.version(),
            };
            update_last_resolutions(
                &mut cx.editor.diff.merge_views,
                target_doc_id,
                Some(new_resolution),
            );
            cx.editor
                .set_status(format!("Switched to {} version", side.label()));
            return;
        }
    }

    // 2. Otherwise find a real conflict at the RESULT cursor and resolve it.
    let (text, cursor_line) = {
        let Some(doc) = cx.editor.document(target_doc_id) else {
            return;
        };
        if !doc.selections().contains_key(&target_view_id) {
            cx.editor.set_status("No conflict at cursor");
            return;
        }
        let cursor_line = doc
            .selection(target_view_id)
            .primary()
            .cursor_line(doc.text().slice(..));
        (doc.text().clone(), cursor_line)
    };

    let Some(region) = scan
        .regions
        .into_iter()
        .find(|r| cursor_line >= r.start_line && cursor_line <= r.end_line)
    else {
        cx.editor.set_status("No conflict at cursor");
        return;
    };

    let Some((ours, base, theirs)) = extract_sides(&text, &region) else {
        cx.editor.set_error("Malformed conflict markers");
        return;
    };
    let base = base.or_else(|| {
        let state = cx.editor.diff.merge_views.get(&current_view_id)?;
        let base = cx.editor.document(state.base_doc_id?)?.text();
        // Map the complete current marker block back to the index base.
        // Computing from the current rope also handles hand edits made after
        // opening the view, without a second mutable conflict model.
        let inverse = helix_core::diff::compare_ropes(&text, base);
        let start = text.line_to_char(region.start_line);
        let end = if region.end_line + 1 < text.len_lines() {
            text.line_to_char(region.end_line + 1)
        } else {
            text.len_chars()
        };
        let start = inverse.changes().map_pos(start, helix_core::Assoc::Before);
        let end = inverse.changes().map_pos(end, helix_core::Assoc::After);
        Some(base.slice(start..end).to_string())
    });
    let both = format!("{}{}", ours, theirs);
    let Some(replacement) = side.pick(&ours, base.as_deref(), &theirs, &both) else {
        cx.editor
            .set_error("No base version is available for this conflict");
        return;
    };

    let doc = doc_mut!(cx.editor, &target_doc_id);
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
        std::iter::once((start_char, end_char, Some(replacement.into()))),
    );
    if !doc.apply(&tx, target_view_id) {
        cx.editor.set_error("Cannot apply resolution");
        return;
    }
    doc.append_changes_to_history(cx.editor.tree.get_mut(target_view_id));
    let new_resolution = LastResolution {
        start_char,
        end_char: new_end,
        ours,
        base,
        theirs,
        both,
        doc_version: doc.version(),
    };
    update_last_resolutions(
        &mut cx.editor.diff.merge_views,
        target_doc_id,
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
    let (view_id, doc_id) = merge_target(cx.editor);
    if let Err(err) = cx.editor.ensure_merge_inputs_current(view_id) {
        cx.editor.set_error(format!("{err:#}"));
        return;
    }
    let cursor_line = {
        let Some(doc) = cx.editor.document(doc_id) else {
            return;
        };
        if !doc.selections().contains_key(&view_id) {
            return;
        }
        doc.selection(view_id)
            .primary()
            .cursor_line(doc.text().slice(..))
    };
    let scan = merge_scan(cx.editor, doc_id);
    if scan.malformed {
        cx.editor.set_error(
            "Malformed conflict markers remain; repair them before navigating conflicts",
        );
        return;
    }
    let conflicts = scan.regions;

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
        let pos = {
            let Some(doc) = cx.editor.document(doc_id) else {
                return;
            };
            doc.text().line_to_char(start_line)
        };
        {
            let doc = doc_mut!(cx.editor, &doc_id);
            doc.set_selection(view_id, helix_core::Selection::point(pos));
        }
        cx.editor.focus(view_id);
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

#[cfg(not(feature = "git"))]
pub fn merge_finish(cx: &mut Context) {
    cx.editor.set_error("git support not compiled in");
}

#[cfg(feature = "git")]
pub fn merge_finish(cx: &mut Context) {
    let (view_id, doc_id) = merge_target(cx.editor);
    if let Err(err) = cx.editor.ensure_merge_inputs_current(view_id) {
        cx.editor.set_error(format!("{err:#}"));
        return;
    }
    let (scan, path) = {
        let Some(doc) = cx.editor.document(doc_id) else {
            return;
        };
        (
            merge_scan(cx.editor, doc_id),
            doc.path().map(|p| p.to_path_buf()),
        )
    };

    if scan.marker_lines > 0 {
        cx.editor.set_error(if scan.malformed {
            "Malformed conflict markers remain".to_string()
        } else {
            format!("{} conflict(s) still unresolved", scan.regions.len())
        });
        return;
    }

    let Some(path) = path else {
        cx.editor
            .set_error("Cannot finish a conflict without a file path");
        return;
    };
    #[cfg(feature = "git")]
    match helix_vcs::git::is_conflicted(&path) {
        Ok(true) => {}
        Ok(false) => {
            cx.editor.set_error("This file has no unstaged conflict resolution; finish only stages conflicted files");
            return;
        }
        Err(err) => {
            cx.editor
                .set_error(format!("Cannot check conflict state: {err:#}"));
            return;
        }
    }
    #[cfg(feature = "git")]
    {
        let size = match helix_vcs::git::conflict_marker_size(&path) {
            Ok(size) => size,
            Err(err) => {
                cx.editor
                    .set_error(format!("Cannot check conflict marker size: {err:#}"));
                return;
            }
        };
        let scan = helix_view::merge_view::scan_conflicts_with_size(
            cx.editor.documents[&doc_id].text(),
            size,
        );
        if scan.marker_lines > 0 {
            cx.editor
                .set_error("Conflict marker lines remain; file was not staged");
            return;
        }
    }
    let deletion = cx
        .editor
        .diff
        .merge_views
        .get(&view_id)
        .filter(|state| state.delete_result)
        .cloned();
    if let Some(state) = &deletion {
        if cx.editor.documents[&doc_id].text().len_chars() != 0 {
            cx.editor.set_error("The result has text; accept a file version before finishing instead of deleting it");
            return;
        }
        if let Err(err) = cx.block_try_flush_writes() {
            cx.editor.set_error(format!("Failed to save: {err}"));
            return;
        }
        let current = match std::fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => {
                cx.editor
                    .set_error(format!("Cannot check file before deletion: {err}"));
                return;
            }
        };
        if current.as_deref() != state.result_on_disk.as_deref() {
            cx.editor
                .set_error("File changed on disk; reload before accepting its deletion");
            return;
        }
        if current.is_some() {
            if let Err(err) = std::fs::remove_file(&path) {
                cx.editor
                    .set_error(format!("Cannot delete resolved file: {err}"));
                return;
            }
        }
        for entry in cx
            .editor
            .diff
            .merge_views
            .values_mut()
            .filter(|entry| entry.result_doc_id == doc_id)
        {
            entry.result_on_disk = None;
        }
    } else if let Err(e) = cx.editor.save::<PathBuf>(doc_id, None, false) {
        cx.editor.set_error(format!("Failed to save: {e}"));
        return;
    }
    // The save above is only queued; flush so git add stages the new content.
    if let Err(e) = cx.block_try_flush_writes() {
        cx.editor.set_error(format!("Failed to save: {e}"));
        return;
    }

    let mut command = std::process::Command::new("git");
    if let Some(parent) = path.parent() {
        command.arg("-C").arg(parent);
    }
    match command.args(["add", "--"]).arg(&path).status() {
        Ok(status) if status.success() => {
            for state in cx
                .editor
                .diff
                .merge_views
                .values_mut()
                .filter(|state| state.result_doc_id == doc_id)
            {
                state.finished = true;
                state.stale = false;
            }
            if deletion.is_some() {
                let doc = cx.editor.documents.get_mut(&doc_id).unwrap();
                doc.reset_modified();
                doc.set_path(None);
                doc.display_name_override = Some(format!(
                    "{} [deleted]",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
                cx.editor.set_status("File deleted and staged (git add)");
            } else {
                cx.editor.set_status("File saved and staged (git add)");
            }
        }
        Ok(status) => {
            cx.editor
                .set_error(format!("File saved but git add failed: {status}"));
        }
        Err(e) => {
            cx.editor
                .set_error(format!("File saved but git add failed to run: {e}"));
        }
    }
}

pub fn diff_toggle_sync_scroll(cx: &mut Context) {
    let view_id = view!(cx.editor).id;
    if let Some(state) = cx.editor.diff.merge_views.get(&view_id).cloned() {
        let new_value = !state.sync_scroll;
        for entry in cx
            .editor
            .diff
            .merge_views
            .values_mut()
            .filter(|entry| entry.result_doc_id == state.result_doc_id)
        {
            entry.sync_scroll = new_value;
        }
        cx.editor.set_status(if new_value {
            "Sync scroll on"
        } else {
            "Sync scroll off"
        });
        return;
    }
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

#[derive(Clone, Copy)]
enum DiffToggleSide {
    Base,
    Working,
}

struct DiffTogglePosition {
    side: DiffToggleSide,
    selection: Selection,
    view_offset: helix_view::view::ViewPosition,
}

fn capture_diff_toggle_position(
    editor: &Editor,
    view_id: ViewId,
    diff_state: &helix_view::diff_view::DiffViewState,
    new_split: bool,
) -> Option<DiffTogglePosition> {
    let view = editor.tree.get(view_id);
    let doc = editor.document(view.doc)?;
    let selection = doc.selection(view.id).clone();
    let view_offset = doc.view_offset(view.id);

    let side = if view.doc == diff_state.base_doc_id {
        DiffToggleSide::Base
    } else {
        DiffToggleSide::Working
    };

    if new_split || matches!(side, DiffToggleSide::Working) {
        return Some(DiffTogglePosition {
            side,
            selection,
            view_offset,
        });
    }

    let cursor = selection.primary().cursor(doc.text().slice(..));
    let target_cursor = editor.map_position_between_documents(
        view_id,
        diff_state.base_doc_id,
        diff_state.working_doc_id,
        cursor,
    )?;
    let target_anchor = editor.map_position_between_documents(
        view_id,
        diff_state.base_doc_id,
        diff_state.working_doc_id,
        view_offset.anchor,
    )?;

    Some(DiffTogglePosition {
        side: DiffToggleSide::Working,
        selection: Selection::point(target_cursor),
        view_offset: helix_view::view::ViewPosition {
            anchor: target_anchor,
            vertical_offset: view_offset.vertical_offset,
            horizontal_offset: view_offset.horizontal_offset,
        },
    })
}

fn restore_diff_toggle_position(editor: &mut Editor, mut position: DiffTogglePosition) {
    let focused_view_id = editor.tree.focus;
    let Some(diff_state) = editor.diff.views.get(&focused_view_id).cloned() else {
        return;
    };

    let (doc_id, view_id, paired) = match position.side {
        DiffToggleSide::Base if diff_state.base_view_id != diff_state.working_view_id => (
            diff_state.base_doc_id,
            diff_state.base_view_id,
            Some((diff_state.working_doc_id, diff_state.working_view_id)),
        ),
        _ => (
            diff_state.working_doc_id,
            diff_state.working_view_id,
            (diff_state.base_view_id != diff_state.working_view_id)
                .then_some((diff_state.base_doc_id, diff_state.base_view_id)),
        ),
    };

    if let Some(doc) = editor.document(doc_id) {
        position.selection = position.selection.ensure_invariants(doc.text().slice(..));
        position.view_offset.anchor = position.view_offset.anchor.min(doc.text().len_chars());
    }
    let paired_position = paired.and_then(|(paired_doc_id, paired_view_id)| {
        let doc = editor.document(doc_id)?;
        let cursor = position.selection.primary().cursor(doc.text().slice(..));
        let mapped_cursor =
            editor.map_position_between_documents(view_id, doc_id, paired_doc_id, cursor)?;
        let mapped_anchor = editor.map_position_between_documents(
            view_id,
            doc_id,
            paired_doc_id,
            position.view_offset.anchor,
        )?;

        Some((
            paired_doc_id,
            paired_view_id,
            Selection::point(mapped_cursor),
            helix_view::view::ViewPosition {
                anchor: mapped_anchor,
                vertical_offset: position.view_offset.vertical_offset,
                horizontal_offset: position.view_offset.horizontal_offset,
            },
        ))
    });

    if let Some(doc) = editor.document_mut(doc_id) {
        doc.set_selection(view_id, position.selection);
        doc.set_view_offset(view_id, position.view_offset);
    }
    if let Some((paired_doc_id, paired_view_id, paired_selection, paired_offset)) = paired_position
    {
        if let Some(doc) = editor.document_mut(paired_doc_id) {
            doc.set_selection(paired_view_id, paired_selection);
            doc.set_view_offset(paired_view_id, paired_offset);
        }
    }
    editor.focus(view_id);
    editor.ensure_cursor_in_view(view_id);
}

pub fn diff_toggle_split_view(cx: &mut Context) {
    let view_id = view!(cx.editor).id;
    let Some(diff_state) = cx.editor.diff.views.get(&view_id).cloned() else {
        cx.editor.set_error("Not in a diff view");
        return;
    };
    let was_split = diff_state.base_view_id != diff_state.working_view_id;
    let new_split = !was_split;
    let history = cx.editor.tree.get(view_id).docs_access_history.clone();
    let position = capture_diff_toggle_position(&cx.editor, view_id, &diff_state, new_split);

    let base_path = diff_state.base_path.clone();
    let working_path = diff_state.working_path.clone();
    let base_ref = diff_state.base_ref.clone();
    let target_ref = diff_state.target_ref.clone();

    let result = match diff_state.source {
        helix_view::diff_view::DiffViewSource::Git => {
            if base_path.as_os_str().is_empty() || working_path.as_os_str().is_empty() {
                cx.editor
                    .set_error("Can't toggle: diff view missing reopen paths");
                return;
            }
            cx.editor.open_diff_view_range_with_paths(
                &base_path,
                &working_path,
                &base_ref,
                target_ref.as_deref(),
                Some(new_split),
            )
        }
        helix_view::diff_view::DiffViewSource::Buffers => cx.editor.open_buffer_diff_view(
            diff_state.base_doc_id,
            diff_state.working_doc_id,
            Some(new_split),
        ),
    };
    if let Err(e) = result {
        cx.editor
            .set_error(format!("Failed to toggle split view: {e}"));
        return;
    }
    cx.editor.diff.split_view_override = Some(new_split);
    // A new working pane inherits the buffers the original pane could return
    // to, including the ordinary file behind a read-only revision.
    let working_view = cx.editor.diff.views[&cx.editor.tree.focus].working_view_id;
    let history: Vec<_> = history
        .into_iter()
        .filter(|id| cx.editor.documents.contains_key(id))
        .collect();
    let view = cx.editor.tree.get_mut(working_view);
    for doc in history {
        if doc != view.doc {
            view.add_to_history(doc);
        }
    }
    // Starting from the base pane leaves the old working pane in place when
    // the comparison is detached. A successful collapse no longer needs it.
    if was_split
        && !new_split
        && diff_state.working_view_id != working_view
        && cx.editor.tree.contains(diff_state.working_view_id)
    {
        cx.editor.close(diff_state.working_view_id);
    }
    if let Some(position) = position {
        restore_diff_toggle_position(&mut cx.editor, position);
    }
    cx.editor.set_status(if new_split {
        "Split view on"
    } else {
        "Split view off"
    });
}

pub fn diff_reset(cx: &mut Context) {
    reset_diff_state(cx.editor);
}

pub fn diff_refresh(cx: &mut Context) {
    if let Err(err) = refresh_diff(cx.editor) {
        cx.editor
            .set_error(format!("Cannot refresh comparison: {err:#}"));
    }
}

pub(crate) fn change_comparison_base(editor: &mut Editor, base: &str) -> anyhow::Result<()> {
    let view_id = editor.tree.focus;
    let state = editor
        .diff
        .views
        .get(&view_id)
        .cloned()
        .context("Not in a diff view")?;
    let position = capture_diff_toggle_position(editor, view_id, &state, state.is_split());
    editor.open_diff_view_range_with_paths(
        &state.base_path,
        &state.working_path,
        base,
        state.target_ref.as_deref(),
        Some(state.is_split()),
    )?;
    if let Some(position) = position {
        restore_diff_toggle_position(editor, position);
    }
    Ok(())
}

pub(crate) fn refresh_diff(editor: &mut Editor) -> anyhow::Result<()> {
    let view_id = editor.tree.focus;
    editor.refresh_comparison_state();
    if let Some(state) = editor.diff.merge_views.get(&view_id).cloned() {
        let result = &editor.documents[&state.result_doc_id];
        anyhow::ensure!(!state.stale || !result.is_modified(), "Merge inputs changed and the result has unsaved edits; preserve those edits before refreshing");
        let path = result
            .path()
            .context("Merge result has no file path")?
            .clone();
        editor.open_merge_view(&path)?;
        let result_view = editor.tree.focus;
        for entry in editor
            .diff
            .merge_views
            .values_mut()
            .filter(|entry| entry.result_view_id == result_view)
        {
            entry.review_range = state.review_range.clone();
        }
        return Ok(());
    }
    let state = editor
        .diff
        .views
        .get(&view_id)
        .cloned()
        .context("Not in a diff or merge view")?;
    anyhow::ensure!(
        state.source == helix_view::diff_view::DiffViewSource::Git,
        "Buffer comparisons already update with edits"
    );
    let position = capture_diff_toggle_position(editor, view_id, &state, state.is_split());
    editor.open_diff_view_range_with_paths(
        &state.base_path,
        &state.working_path,
        &state.base_ref,
        state.target_ref.as_deref(),
        Some(state.is_split()),
    )?;
    if let Some(position) = position {
        restore_diff_toggle_position(editor, position);
    }
    editor.set_status("Comparison refreshed");
    Ok(())
}

pub fn diff_revert_hunk(cx: &mut Context) {
    match super::typed::revert_diff_changes(cx.editor) {
        Ok(changes) => cx.editor.set_status(format!(
            "Reset {changes} change{}",
            if changes == 1 { "" } else { "s" }
        )),
        Err(err) => cx.editor.set_error(format!("{err:#}")),
    }
}

pub(crate) fn reset_diff_state(editor: &mut Editor) {
    editor.diff.suspended.clear();
    let views: Vec<_> = editor.diff.views.keys().copied().collect();
    for id in views {
        editor.close_diff_view(id);
    }
    let merges: Vec<_> = editor.diff.merge_views.keys().copied().collect();
    for id in merges {
        editor.close_merge_view(id);
    }
    editor.diff.range = None;
    editor.diff.split_view_override = None;
    editor.diff.last_changed_file_selection = None;
    editor.diff.changed_file_cache = None;
    editor.diff.changed_file_request = editor.diff.changed_file_request.wrapping_add(1);
    let ids: Vec<_> = editor.documents.keys().copied().collect();
    for id in ids {
        let doc = editor.documents.get_mut(&id).unwrap();
        doc.char_diff_enabled = false;
        doc.char_diff_minus_side = false;
        doc.linked_diff_doc = None;
        doc.reset_diff_comparison();
        if !doc.is_virtual_base {
            if let Some(path) = doc.path().cloned() {
                if let Some(bytes) = editor.diff_providers.get_diff_base(&path) {
                    doc.set_diff_base(bytes);
                }
            }
        }
    }
    editor.set_status("Diff reset to HEAD");
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
        // The selected commit is the base, so its own changes are not shown.
        let range = DiffRange {
            base_ref: hash.clone(),
            target_ref: None,
        };
        if !set_checked_range(cx.editor, range) {
            return;
        }
        // With HEAD as the base only uncommitted changes remain, which may be
        // none at all. Say so, so an empty picker does not look like a bug.
        let cwd = helix_stdx::env::current_working_dir();
        let providers = &cx.editor.diff_providers;
        let is_head = providers
            .resolve_commit_id(&cwd, &hash)
            .is_some_and(|id| providers.resolve_commit_id(&cwd, "HEAD") == Some(id));
        if is_head {
            cx.editor.set_warning(format!(
                "Diff set: {} (HEAD) vs working tree: uncommitted changes only, space m C shows the commit",
                hash
            ));
        } else {
            cx.editor
                .set_status(format!("Diff set: {} vs working tree", hash));
        }
    } else {
        // git log lists newest first. The bottom line is the base and is not
        // included: OLDER..NEWER, the same as typed `:diff-commit A..B`.
        //
        // The two ends are compared as snapshots. On linear history the
        // line below a commit is its parent, but with merges it may be a
        // commit from another branch, and the diff then includes that
        // branch's changes too. Revisit if that turns out to matter.
        let (Some(newer), Some(older)) = (
            extract_commit_hash(lines[0]),
            extract_commit_hash(lines[lines.len() - 1]),
        ) else {
            cx.editor.set_error("Invalid commit hash in selection");
            return;
        };
        let (older, newer) = (older.to_string(), newer.to_string());
        let range = DiffRange {
            base_ref: older.clone(),
            target_ref: Some(newer.clone()),
        };
        if !set_checked_range(cx.editor, range) {
            return;
        }
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
    let range = DiffRange {
        base_ref: format!("{}^", hash),
        target_ref: Some(hash.clone()),
    };
    if !set_checked_range(cx.editor, range) {
        return;
    }
    cx.editor
        .set_status(format!("Diff set: showing changes in {}", hash));
}

fn set_checked_range(editor: &mut Editor, range: DiffRange) -> bool {
    if let Err(err) = range.validate(&helix_stdx::env::current_working_dir()) {
        editor.set_error(format!("Cannot set diff range: {err:#}"));
        false
    } else {
        editor.diff.range = Some(range);
        true
    }
}

#[cfg(all(test, feature = "git"))]
mod listing_tests {
    use super::*;

    #[test]
    fn failed_tracked_scan_never_falls_back_to_untracked_files() {
        let dir = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .arg("init")
            .arg(dir.path())
            .output()
            .unwrap();
        assert!(status.status.success());
        std::fs::write(dir.path().join("untracked.txt"), "untracked\n").unwrap();
        let emitted = std::cell::Cell::new(0);
        let result = scan_changed_files(dir.path(), "no-such-reference", None, |_| {
            emitted.set(emitted.get() + 1)
        });
        assert!(result.is_err());
        assert_eq!(emitted.get(), 0);
        let files = collect_changed_files(dir.path(), "HEAD", None).unwrap();
        assert_eq!(files.len(), 1);
        assert!(matches!(files[0], FileChange::Untracked { .. }));
    }
}
