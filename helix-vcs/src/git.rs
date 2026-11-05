use anyhow::{bail, Context, Result};
use arc_swap::ArcSwap;
use gix::filter::plumbing::driver::apply::Delay;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gix::bstr::ByteSlice;
use gix::diff::Rewrites;
use gix::dir::entry::Status;
use gix::objs::tree::EntryKind;
use gix::sec::trust::DefaultForLevel;
use gix::status::{
    index_worktree::Item,
    plumbing::index_as_worktree::{Change, EntryStatus},
    UntrackedFiles,
};
use gix::{Commit, ObjectId, Repository, ThreadSafeRepository, Tree};

use crate::FileChange;

#[cfg(test)]
mod test;

#[inline]
fn get_repo_dir(file: &Path) -> Result<&Path> {
    file.parent()
        .context("file has no parent directory")?
        .ancestors()
        .find(|path| path.is_dir())
        .context("file has no existing ancestor directory")
}

/// Resolve a file path, handling the case where the file doesn't exist yet
/// (e.g. a deleted file being diffed). Resolve the nearest existing directory,
/// preserving the missing suffix even when several directories are gone.
fn resolve_file_path(file: &Path) -> Result<PathBuf> {
    if file.exists() {
        gix::path::realpath(file).context("resolve symlinks")
    } else {
        let ancestor = get_repo_dir(file)?;
        let resolved = gix::path::realpath(ancestor).context("resolve symlinks for ancestor")?;
        Ok(resolved.join(file.strip_prefix(ancestor)?))
    }
}

pub fn get_diff_base(file: &Path) -> Result<Vec<u8>> {
    debug_assert!(!file.exists() || file.is_file());
    debug_assert!(file.is_absolute());
    let file = resolve_file_path(file)?;

    // TODO cache repository lookup

    let repo_dir = get_repo_dir(&file)?;
    let repo = open_repo(repo_dir)
        .context("failed to open git repo")?
        .to_thread_local();
    let head = repo.head_commit()?;
    let file_oid = find_file_in_tree(&repo, &head.tree()?, &file)?;

    let file_object = repo.find_object(file_oid)?;
    let data = file_object.detach().data;
    // Get the actual data that git would make out of the git object.
    // This will apply the user's git config or attributes like crlf conversions.
    if let Some(work_dir) = repo.workdir() {
        let rela_path = file.strip_prefix(work_dir)?;
        let rela_path = gix::path::try_into_bstr(rela_path)?;
        let (mut pipeline, _) = repo.filter_pipeline(None)?;
        let mut worktree_outcome =
            pipeline.convert_to_worktree(&data, rela_path.as_ref(), Delay::Forbid)?;
        let mut buf = Vec::with_capacity(data.len());
        worktree_outcome.read_to_end(&mut buf)?;
        Ok(buf)
    } else {
        Ok(data)
    }
}

pub fn get_diff_base_from_ref(file: &Path, ref_name: &str) -> Result<Vec<u8>> {
    debug_assert!(!file.exists() || file.is_file());
    debug_assert!(file.is_absolute());
    let file = resolve_file_path(file)?;

    let repo_dir = get_repo_dir(&file)?;
    let repo = open_repo(repo_dir)
        .context("failed to open git repo")?
        .to_thread_local();

    let tree = resolve_tree(&repo, ref_name)?;
    let file_oid = find_file_in_tree(&repo, &tree, &file)?;

    let file_object = repo.find_object(file_oid)?;
    let data = file_object.detach().data;

    if let Some(work_dir) = repo.workdir() {
        let rela_path = file.strip_prefix(work_dir)?;
        let rela_path = gix::path::try_into_bstr(rela_path)?;
        let (mut pipeline, _) = repo.filter_pipeline(None)?;
        let mut worktree_outcome =
            pipeline.convert_to_worktree(&data, rela_path.as_ref(), Delay::Forbid)?;
        let mut buf = Vec::with_capacity(data.len());
        worktree_outcome.read_to_end(&mut buf)?;
        Ok(buf)
    } else {
        Ok(data)
    }
}

/// The pieces needed to build a web link to `file` as it exists in `rev`.
#[derive(Debug, Clone)]
pub struct FileWebLink {
    /// The remote URL as git resolved it, with any `insteadOf` rewrite applied.
    pub remote_url: String,
    /// Full commit hash, so the link stays valid when the branch moves on.
    pub commit: String,
    /// Repository-root relative path, forward slashes.
    pub path: String,
}

/// Resolve everything needed to link to `file` at `rev` on the repository's
/// web host: which remote this repository tracks, what commit `rev` names, and
/// where the file sits relative to the repository root.
///
/// `rev` accepts the same spellings as the diff commands (a ref, tag, short or
/// full hash, with an optional `^` suffix) and is always resolved to a full
/// hash so the resulting link is a permalink.
pub fn file_web_link(file: &Path, rev: &str) -> Result<FileWebLink> {
    debug_assert!(file.is_absolute());
    let file = resolve_file_path(file)?;

    let repo_dir = get_repo_dir(&file)?;
    let repo = open_repo(repo_dir)
        .context("failed to open git repo")?
        .to_thread_local();

    let remote_url = remote_url(&repo)?;
    let commit = resolve_commit(&repo, rev)?.id().to_hex().to_string();

    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow::anyhow!("not a working tree"))?;
    let rela_path = file
        .strip_prefix(work_dir)
        .context("file is outside the repository")?;
    let path = rela_path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    Ok(FileWebLink {
        remote_url,
        commit,
        path,
    })
}

/// The URL of the remote this repository tracks. Prefers the remote configured
/// for the checked-out branch (what `git fetch` with no arguments would use),
/// then falls back to `origin`, so it works both on a tracking branch and on a
/// detached HEAD.
fn remote_url(repo: &Repository) -> Result<String> {
    // `find_fetch_remote(None)` is what `git fetch` with no arguments resolves:
    // the checked-out branch's remote, then the only/`origin` remote. On a
    // detached HEAD or an odd config it can fail outright, so `origin` is tried
    // once more before giving up.
    let remote = match repo.find_fetch_remote(None) {
        Ok(remote) => remote,
        Err(_) => repo
            .find_remote("origin")
            .map_err(|_| anyhow::anyhow!("no git remote configured for this repository"))?,
    };

    let url = remote
        .url(gix::remote::Direction::Fetch)
        .ok_or_else(|| anyhow::anyhow!("git remote has no fetch URL"))?;

    Ok(url.to_bstring().to_string())
}

pub fn get_current_head_name(file: &Path) -> Result<Arc<ArcSwap<Box<str>>>> {
    debug_assert!(!file.exists() || file.is_file());
    debug_assert!(file.is_absolute());
    let file = resolve_file_path(file)?;

    let repo_dir = get_repo_dir(&file)?;
    let repo = open_repo(repo_dir)
        .context("failed to open git repo")?
        .to_thread_local();
    let head_ref = repo.head_ref()?;
    let head_commit = repo.head_commit()?;

    let name = match head_ref {
        Some(reference) => reference.name().shorten().to_string(),
        None => head_commit.id.to_hex_with_len(8).to_string(),
    };

    Ok(Arc::new(ArcSwap::from_pointee(name.into_boxed_str())))
}

/// The full hash of the commit `rev` names, in the repository holding `path`.
///
/// `rev` takes the same spellings as the diff commands: a ref, a tag, a short
/// or full hash, with an optional `^`. `path` is a file in the repository or
/// a directory of it, such as the worktree root.
pub fn resolve_commit_id(path: &Path, rev: &str) -> Result<String> {
    let repo_dir = if path.is_dir() {
        path
    } else {
        get_repo_dir(path)?
    };
    let repo = open_repo(repo_dir)
        .context("failed to open git repo")?
        .to_thread_local();
    let commit = resolve_commit(&repo, rev)?.id().to_hex().to_string();
    Ok(commit)
}

/// Whether a file still has unresolved index stages, including a file whose
/// markers have already been removed by hand but which is not staged yet.
pub fn is_conflicted(file: &Path) -> Result<bool> {
    let file = resolve_file_path(file)?;
    let repo = open_repo(get_repo_dir(&file)?)?.to_thread_local();
    let work_dir = repo.workdir().context("not a working tree")?;
    let path = gix::path::try_into_bstr(file.strip_prefix(work_dir)?)?;
    let index = repo.index_or_load_from_head()?;
    Ok(index.entries().iter().any(|entry| {
        entry.path(&index) == path.as_ref()
            && entry.stage() != gix::index::entry::Stage::Unconflicted
    }))
}

#[derive(Debug)]
pub struct MergeInputs {
    pub marker_size: usize,
    pub base: Option<Vec<u8>>,
    pub ours: Option<Vec<u8>>,
    pub theirs: Option<Vec<u8>>,
    pub fingerprint: String,
    pub ours_label: String,
    pub theirs_label: String,
}

struct MergeIndex {
    marker_size: usize,
    repo: Repository,
    path: PathBuf,
    stages: [Option<ObjectId>; 3],
    fingerprint: String,
    ours_label: String,
    theirs_label: String,
}

fn merge_index(file: &Path) -> Result<MergeIndex> {
    let file = resolve_file_path(file)?;
    let repo = open_repo(get_repo_dir(&file)?)?.to_thread_local();
    let path = file
        .strip_prefix(repo.workdir().context("not a working tree")?)?
        .to_path_buf();
    let rela_path = gix::path::try_into_bstr(path.as_path())?;
    let index = repo.index_or_load_from_head()?;
    let mut stages = [None; 3];
    for entry in index.entries() {
        if entry.path(&index) == rela_path.as_ref() {
            match entry.stage() {
                gix::index::entry::Stage::Base => stages[0] = Some(entry.id),
                gix::index::entry::Stage::Ours => stages[1] = Some(entry.id),
                gix::index::entry::Stage::Theirs => stages[2] = Some(entry.id),
                _ => {}
            }
        }
    }
    let marker_size = marker_size_for_path(&repo, &index, &path)?;
    let state = repo.state();
    let rebase = matches!(
        state,
        Some(
            gix::state::InProgress::Rebase
                | gix::state::InProgress::RebaseInteractive
                | gix::state::InProgress::ApplyMailboxRebase
        )
    );
    let head = repo
        .head_id()
        .ok()
        .map(|id| id.to_hex().to_string())
        .unwrap_or_default();
    let incoming = ["REBASE_HEAD", "CHERRY_PICK_HEAD", "MERGE_HEAD"]
        .into_iter()
        .find_map(|name| {
            resolve_commit(&repo, name)
                .ok()
                .map(|commit| commit.id().to_hex().to_string())
        })
        .unwrap_or_default();
    let short = |id: &str| id.chars().take(8).collect::<String>();
    let ours_label = format!(
        "{} {}",
        if rebase { "upstream HEAD" } else { "HEAD" },
        short(&head)
    );
    let theirs_label = format!(
        "{} {}",
        if rebase {
            "replayed commit"
        } else {
            "incoming"
        },
        short(&incoming)
    );
    let ours_label = if stages[1].is_none() {
        format!("{ours_label} (deleted)")
    } else {
        ours_label
    };
    let theirs_label = if stages[2].is_none() {
        format!("{theirs_label} (deleted)")
    } else {
        theirs_label
    };
    let fingerprint = format!("{state:?}:{head}:{incoming}:{stages:?}:{marker_size}");
    drop(index);
    Ok(MergeIndex {
        marker_size,
        repo,
        path,
        stages,
        fingerprint,
        ours_label,
        theirs_label,
    })
}

pub fn merge_fingerprint(file: &Path) -> Result<String> {
    Ok(merge_index(file)?.fingerprint)
}

pub fn conflict_marker_size(file: &Path) -> Result<usize> {
    let file = resolve_file_path(file)?;
    let repo = open_repo(get_repo_dir(&file)?)?.to_thread_local();
    let path = file.strip_prefix(repo.workdir().context("not a working tree")?)?;
    let index = repo.index_or_load_from_head()?;
    marker_size_for_path(&repo, &index, path)
}

fn marker_size_for_path(
    repo: &Repository,
    index: &gix::index::State,
    path: &Path,
) -> Result<usize> {
    let mut attrs = repo.attributes_only(
        index,
        gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
    )?;
    let mut matches = attrs.selected_attribute_matches(["conflict-marker-size"]);
    attrs.at_path(path, None)?.matching_attributes(&mut matches);
    let size = matches.iter_selected().find_map(|attr| {
        attr.assignment
            .state
            .as_bstr()
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|&size| size > 0)
    });
    Ok(size.unwrap_or(7))
}

/// Read all available conflict stages. Missing sides describe deletions;
/// add/add conflicts have no base.
pub fn get_merge_inputs(file: &Path) -> Result<MergeInputs> {
    let state = merge_index(file)?;
    anyhow::ensure!(
        state.stages.iter().any(Option::is_some),
        "File has no unresolved index stages"
    );
    let read = |id: Option<ObjectId>| -> Result<Option<Vec<u8>>> {
        let Some(id) = id else {
            return Ok(None);
        };
        let data = state.repo.find_object(id)?.detach().data;
        let (mut pipeline, _) = state.repo.filter_pipeline(None)?;
        let path = gix::path::try_into_bstr(state.path.as_path())?;
        let mut outcome = pipeline.convert_to_worktree(&data, path.as_ref(), Delay::Forbid)?;
        let mut bytes = Vec::with_capacity(data.len());
        outcome.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    };
    Ok(MergeInputs {
        marker_size: state.marker_size,
        base: read(state.stages[0])?,
        ours: read(state.stages[1])?,
        theirs: read(state.stages[2])?,
        fingerprint: state.fingerprint,
        ours_label: state.ours_label,
        theirs_label: state.theirs_label,
    })
}

/// Fetch the two sides for callers that require a text/text conflict.
pub fn get_merge_versions(file: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    anyhow::ensure!(
        is_conflicted(file)?,
        "OURS (stage 2) not in index — is there an active merge?"
    );
    let inputs = get_merge_inputs(file)?;
    Ok((
        inputs.ours.context("OURS (stage 2) not in index")?,
        inputs.theirs.context("THEIRS (stage 3) not in index")?,
    ))
}

pub fn for_each_changed_file(cwd: &Path, f: impl Fn(Result<FileChange>) -> bool) -> Result<()> {
    status(&open_repo(cwd)?.to_thread_local(), UntrackedFiles::Files, f)
}

/// Resolve the old name only for a selected new file. This avoids hashing
/// every untracked file during the changed-file picker's initial scan.
pub fn find_rename_source(file: &Path, base_ref: &str) -> Result<Option<PathBuf>> {
    let file = resolve_file_path(file)?;
    let repo = open_repo(get_repo_dir(&file)?)?.to_thread_local();
    let tree = resolve_tree(&repo, base_ref)?;
    match find_file_in_tree(&repo, &tree, &file) {
        Ok(_) => return Ok(None),
        Err(err) if err.is::<FileNotFoundInRevision>() => {}
        Err(err) => return Err(err),
    }
    let workdir = repo.workdir().context("not a working tree")?;
    let candidates = std::cell::RefCell::new(Vec::new());
    for_each_tracked_change(workdir, base_ref, None, |change| {
        if let Ok(FileChange::Deleted { path }) = change {
            candidates.borrow_mut().push(path);
        }
        true
    })?;
    let selected = std::fs::read(&file)?;
    if selected.is_empty() {
        return Ok(None);
    }
    let mut selected_lines = std::collections::HashMap::new();
    for line in selected.lines() {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        *selected_lines.entry(line).or_insert(0usize) += 1;
    }
    let mut best = None;
    let mut best_score = 0.0;
    let mut tied = false;
    for path in candidates.into_inner() {
        let oid = find_file_in_tree(&repo, &tree, &path)?;
        let data = repo.find_object(oid)?.detach().data;
        let mut remaining = selected_lines.clone();
        let mut common = 0;
        for line in data.lines() {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if let Some(count) = remaining.get_mut(line).filter(|count| **count > 0) {
                *count -= 1;
                common += line.len() + 1;
            }
        }
        let score = common as f64 / data.len().max(selected.len()) as f64;
        if score < 0.5 {
            continue;
        }
        if score > best_score {
            best_score = score;
            best = Some(path);
            tied = false;
        } else if score == best_score && best.is_some() {
            tied = true;
        }
    }
    Ok(best.filter(|_| !tied))
}

/// Iterate over changed files between a git ref and the working tree, or between two refs.
///
/// - `target_ref = None`: compare `base_ref` vs working tree (all changes since that ref)
/// - `target_ref = Some(t)`: compare `base_ref` vs `t` (two-commit tree diff)
///
/// Untracked files are included (via gix's dirwalk). For the changed-file
/// picker, prefer [`for_each_tracked_change`] + [`for_each_untracked_file`],
/// which split the work so tracked changes appear instantly and untracked files
/// are enumerated with a faster parallel walker.
pub fn for_each_changed_file_between_refs(
    cwd: &Path,
    base_ref: &str,
    target_ref: Option<&str>,
    f: impl Fn(Result<FileChange>) -> bool,
) -> Result<()> {
    for_each_change_impl(cwd, base_ref, target_ref, UntrackedFiles::Files, f)
}

/// Like [`for_each_changed_file_between_refs`] but reports only *tracked*
/// changes (modified / deleted / conflicted), never untracked files. gix does
/// no directory walk, so against the working tree this is just the fast
/// index<->worktree modification pass.
pub fn for_each_tracked_change(
    cwd: &Path,
    base_ref: &str,
    target_ref: Option<&str>,
    f: impl Fn(Result<FileChange>) -> bool,
) -> Result<()> {
    for_each_change_impl(cwd, base_ref, target_ref, UntrackedFiles::None, f)
}

fn for_each_change_impl(
    cwd: &Path,
    base_ref: &str,
    target_ref: Option<&str>,
    untracked: UntrackedFiles,
    f: impl Fn(Result<FileChange>) -> bool,
) -> Result<()> {
    let repo = open_repo(cwd)?.to_thread_local();

    match target_ref {
        None => {
            let base_tree = resolve_tree(&repo, base_ref)?;
            if repo.head()?.is_unborn() {
                return status(&repo, untracked, f);
            }
            let head_commit = repo.head_commit()?;
            let head_tree = head_commit.tree()?;

            if base_tree.id == head_tree.id {
                return status(&repo, untracked, f);
            }

            let work_dir = repo
                .workdir()
                .ok_or_else(|| anyhow::anyhow!("working tree not found"))?
                .to_path_buf();

            let mut seen = std::collections::HashSet::new();
            let mut cancelled = false;

            base_tree.changes()?.for_each_to_obtain_tree(
                &head_tree,
                |change| -> Result<_, std::convert::Infallible> {
                    use gix::object::tree::diff::Change;
                    let file_change = match change {
                        Change::Addition { entry_mode, .. }
                        | Change::Modification { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                Some(FileChange::Modified {
                                    path: work_dir.join(gix::path::from_bstr(change.location())),
                                })
                            } else {
                                None
                            }
                        }
                        Change::Rewrite {
                            source_location,
                            location,
                            entry_mode,
                            ..
                        } => entry_mode.is_blob().then(|| FileChange::Renamed {
                            from_path: work_dir.join(gix::path::from_bstr(source_location)),
                            to_path: work_dir.join(gix::path::from_bstr(location)),
                        }),
                        Change::Deletion { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                Some(FileChange::Deleted {
                                    path: work_dir.join(gix::path::from_bstr(change.location())),
                                })
                            } else {
                                None
                            }
                        }
                    };
                    if let Some(change_item) = file_change {
                        seen.insert(change_item.path().to_path_buf());
                        if !f(Ok(change_item)) {
                            cancelled = true;
                            return Ok(gix::object::tree::diff::Action::Cancel);
                        }
                    }
                    Ok(gix::object::tree::diff::Action::Continue)
                },
            )?;

            if !cancelled {
                status(&repo, untracked, |change| match &change {
                    Ok(fc)
                        if seen.contains(fc.path())
                            && !matches!(fc, FileChange::Conflict { .. }) =>
                    {
                        true
                    }
                    _ => f(change),
                })?;
            }
        }
        Some(target) => {
            let base_tree = resolve_tree(&repo, base_ref)?;
            let target_tree = resolve_commit(&repo, target)?.tree()?;
            let work_dir = repo
                .workdir()
                .ok_or_else(|| anyhow::anyhow!("working tree not found"))?
                .to_path_buf();

            base_tree.changes()?.for_each_to_obtain_tree(
                &target_tree,
                |change| -> Result<_, std::convert::Infallible> {
                    use gix::object::tree::diff::Change;
                    let file_change = match change {
                        Change::Addition { entry_mode, .. }
                        | Change::Modification { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                Some(FileChange::Modified {
                                    path: work_dir.join(gix::path::from_bstr(change.location())),
                                })
                            } else {
                                None
                            }
                        }
                        Change::Rewrite {
                            source_location,
                            location,
                            entry_mode,
                            ..
                        } => entry_mode.is_blob().then(|| FileChange::Renamed {
                            from_path: work_dir.join(gix::path::from_bstr(source_location)),
                            to_path: work_dir.join(gix::path::from_bstr(location)),
                        }),
                        Change::Deletion { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                Some(FileChange::Deleted {
                                    path: work_dir.join(gix::path::from_bstr(change.location())),
                                })
                            } else {
                                None
                            }
                        }
                    };
                    if let Some(change_item) = file_change {
                        if !f(Ok(change_item)) {
                            return Ok(gix::object::tree::diff::Action::Cancel);
                        }
                    }
                    Ok(gix::object::tree::diff::Action::Continue)
                },
            )?;
        }
    }

    Ok(())
}

/// Enumerate untracked files in the working tree, invoking `f` for each.
///
/// This replaces gix's single-threaded status dirwalk with the parallel
/// `ignore` walker: `f` is called concurrently from multiple threads (hence the
/// `Sync` bound), which hides per-entry I/O latency on network filesystems. A
/// file is untracked when it exists on disk, is not excluded by any git ignore
/// source, and is not present in the index. Matches `git status -uall` (each
/// untracked symlink is reported as an entry, like git), except that a nested
/// repository is skipped rather than listed as one `nested/` entry, since the
/// picker opens files. Submodules are skipped too, as git does.
pub fn for_each_untracked_file(cwd: &Path, f: impl Fn(FileChange) + Sync) -> Result<()> {
    let repo = open_repo(cwd)?.to_thread_local();
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow::anyhow!("working tree not found"))?
        .to_path_buf();

    // Absolute paths of every entry in the index — everything git already
    // tracks (including staged-but-uncommitted files, which are not untracked).
    let index = repo.index_or_empty()?;
    let tracked: std::collections::HashSet<PathBuf> = index
        .entries()
        .iter()
        .map(|entry| work_dir.join(gix::path::from_bstr(entry.path(&index))))
        .collect();

    // Submodule paths. Their files belong to another repository and are not in
    // this index, so without pruning every one of them would read as untracked.
    let gitlinks: std::collections::HashSet<PathBuf> = index
        .entries()
        .iter()
        .filter(|entry| entry.mode == gix::index::entry::Mode::COMMIT)
        .map(|entry| work_dir.join(gix::path::from_bstr(entry.path(&index))))
        .collect();

    let walker = ignore::WalkBuilder::new(&work_dir)
        .hidden(false) // git scans dotfiles; `.git` is pruned below
        .parents(true)
        .ignore(false) // honor only git ignore sources, not `.ignore` files
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(move |entry| {
            if entry.file_name() == ".git" {
                return false;
            }
            // Stop at repository boundaries: a submodule, even an
            // uninitialized one, or any nested clone (its own `.git`).
            let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
            !(is_dir
                && entry.depth() > 0
                && (gitlinks.contains(entry.path()) || entry.path().join(".git").exists()))
        })
        .threads(0) // 0 = pick a sensible default based on CPUs
        .build_parallel();

    walker.run(|| {
        Box::new(|result| {
            if let Ok(entry) = result {
                // Regular files and symlinks; git lists an untracked symlink as
                // its own entry. Sockets and FIFOs are skipped, as git does.
                if entry
                    .file_type()
                    .is_some_and(|ft| ft.is_file() || ft.is_symlink())
                {
                    let path = entry.into_path();
                    if !tracked.contains(&path) {
                        f(FileChange::Untracked { path });
                    }
                }
            }
            ignore::WalkState::Continue
        })
    });

    Ok(())
}

/// Iterate over files changed between a specific commit and HEAD.
pub fn for_each_changed_file_between_commits(
    cwd: &Path,
    ref_name: &str,
    f: impl Fn(Result<FileChange>) -> bool,
) -> Result<()> {
    let repo = open_repo(cwd)?.to_thread_local();
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow::anyhow!("working tree not found"))?
        .to_path_buf();

    let head_commit = repo.head_commit()?;
    let target_commit = resolve_commit(&repo, ref_name)?;
    let head_tree = head_commit.tree()?;
    let target_tree = target_commit.tree()?;

    target_tree.changes()?.for_each_to_obtain_tree(
        &head_tree,
        |change| -> Result<_, std::convert::Infallible> {
            use gix::object::tree::diff::Change;
            let file_change = match change {
                Change::Addition { entry_mode, .. } | Change::Modification { entry_mode, .. } => {
                    if entry_mode.is_blob() {
                        Some(FileChange::Modified {
                            path: work_dir.join(gix::path::from_bstr(change.location())),
                        })
                    } else {
                        None
                    }
                }
                Change::Rewrite {
                    source_location,
                    location,
                    entry_mode,
                    ..
                } => entry_mode.is_blob().then(|| FileChange::Renamed {
                    from_path: work_dir.join(gix::path::from_bstr(source_location)),
                    to_path: work_dir.join(gix::path::from_bstr(location)),
                }),
                Change::Deletion { entry_mode, .. } => {
                    if entry_mode.is_blob() {
                        Some(FileChange::Deleted {
                            path: work_dir.join(gix::path::from_bstr(change.location())),
                        })
                    } else {
                        None
                    }
                }
            };
            if let Some(change_item) = file_change {
                if !f(Ok(change_item)) {
                    return Ok(gix::object::tree::diff::Action::Cancel);
                }
            }
            Ok(gix::object::tree::diff::Action::Continue)
        },
    )?;

    Ok(())
}

/// Resolve a diff base to its tree. Like [`resolve_commit`], except that the
/// parent of a root commit (`ROOT^`, which `Space-m c` builds from a log line)
/// is the empty tree, so the root commit's own files read as added.
fn resolve_tree<'a>(repo: &'a Repository, ref_name: &str) -> Result<Tree<'a>> {
    if ref_name == "HEAD" && repo.head()?.is_unborn() {
        return Ok(repo.empty_tree());
    }
    if let Some(base) = ref_name.strip_suffix('^') {
        let commit = resolve_commit(repo, base)?;
        if commit.parent_ids().next().is_none() {
            return Ok(repo.empty_tree());
        }
    }
    Ok(resolve_commit(repo, ref_name)?.tree()?)
}

/// Validate both ends before a command publishes a new comparison range.
pub fn validate_diff_range(cwd: &Path, base: &str, target: Option<&str>) -> Result<()> {
    let repo = open_repo(cwd)?.to_thread_local();
    resolve_tree(&repo, base).with_context(|| format!("invalid diff base '{base}'"))?;
    if let Some(target) = target {
        resolve_commit(&repo, target).with_context(|| format!("invalid diff target '{target}'"))?;
    }
    Ok(())
}

/// The base used by `git diff A...B`: the common ancestor of A and B.
pub fn merge_base(cwd: &Path, base: &str, target: &str) -> Result<String> {
    let repo = open_repo(cwd)?.to_thread_local();
    let base = resolve_commit(&repo, base)?;
    let target = resolve_commit(&repo, target)?;
    Ok(repo
        .merge_base(base.id().detach(), target.id().detach())?
        .to_hex()
        .to_string())
}

/// Resolve a Git revision and peel annotated tags to commits.
fn resolve_commit<'a>(repo: &'a Repository, ref_name: &str) -> Result<Commit<'a>> {
    if let Some(base) = ref_name.strip_suffix('^') {
        let commit = resolve_commit(repo, base)?;
        let parent_id = commit
            .parent_ids()
            .next()
            .ok_or_else(|| anyhow::anyhow!("commit {} has no parent (root commit)", commit.id))?;
        return repo
            .find_object(parent_id)?
            .try_into_commit()
            .context(format!("parent of '{}' is not a commit", base));
    }

    // This gix version traverses the starting object without first peeling
    // annotated tags. Normalize just that starting object, leaving all Git
    // revision operators to its revision parser.
    let normalized;
    let spec = if let Some(index) = ref_name.find(['^', '~']).filter(|&index| index > 0) {
        let start = repo
            .rev_parse_single(ref_name[..index].as_bytes().as_bstr())?
            .object()?
            .peel_to_commit()?;
        normalized = format!("{}{}", start.id, &ref_name[index..]);
        normalized.as_str()
    } else {
        ref_name
    };
    repo.rev_parse_single(spec.as_bytes().as_bstr())
        .with_context(|| format!("cannot resolve revision '{ref_name}'"))?
        .object()?
        .peel_to_commit()
        .with_context(|| format!("'{ref_name}' is not a commit"))
}

fn open_repo(path: &Path) -> Result<ThreadSafeRepository> {
    // custom open options
    let mut git_open_opts_map = gix::sec::trust::Mapping::<gix::open::Options>::default();

    // On windows various configuration options are bundled as part of the installations
    // This path depends on the install location of git and therefore requires some overhead to lookup
    // This is basically only used on windows and has some overhead hence it's disabled on other platforms.
    // `gitoxide` doesn't use this as default
    let config = gix::open::permissions::Config {
        system: true,
        git: true,
        user: true,
        env: true,
        includes: true,
        git_binary: cfg!(windows),
    };
    // change options for config permissions without touching anything else
    git_open_opts_map.reduced = git_open_opts_map
        .reduced
        .permissions(gix::open::Permissions {
            config,
            ..gix::open::Permissions::default_for_level(gix::sec::Trust::Reduced)
        });
    git_open_opts_map.full = git_open_opts_map.full.permissions(gix::open::Permissions {
        config,
        ..gix::open::Permissions::default_for_level(gix::sec::Trust::Full)
    });

    let open_options = gix::discover::upwards::Options {
        dot_git_only: true,
        ..Default::default()
    };

    let res = ThreadSafeRepository::discover_with_environment_overrides_opts(
        path,
        open_options,
        git_open_opts_map,
    )?;

    Ok(res)
}

/// Whether an index entry is a regular file or symlink, as opposed to a
/// submodule or sparse directory. Mirrors the `is_blob` filtering the tree-diff
/// paths above apply, for which index entry modes have no direct equivalent.
fn is_file_mode(mode: gix::index::entry::Mode) -> bool {
    mode.to_tree_entry_mode()
        .is_some_and(|mode| mode.is_blob_or_symlink())
}

/// Emulates the result of running `git status` from the command line.
///
/// `untracked` selects how new files are reported. Callers that enumerate
/// untracked files separately (via [`for_each_untracked_file`], which uses a
/// parallel walker) pass [`UntrackedFiles::None`] so gix does no directory
/// walk at all — leaving only the fast head-tree<->index and index<->worktree
/// modification passes.
fn status(
    repo: &Repository,
    untracked: UntrackedFiles,
    f: impl Fn(Result<FileChange>) -> bool,
) -> Result<()> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow::anyhow!("working tree not found"))?
        .to_path_buf();

    // HEAD^{tree} is compared against the index as well as the index against the
    // worktree. Without the head tree, staged work is invisible here: `git add`
    // makes the index match the worktree, so an index<->worktree scan alone
    // reports nothing while the change is plainly still there against HEAD.
    // `or_empty` covers an unborn HEAD, where every indexed file reads as added.
    let head_tree_id = repo.head_tree_id_or_empty()?;

    let index = repo.index_or_empty()?;
    let conflicted: std::collections::HashSet<_> = index
        .entries()
        .iter()
        .filter(|entry| entry.stage() != gix::index::entry::Stage::Unconflicted)
        .map(|entry| work_dir.join(gix::path::from_bstr(entry.path(&index))))
        .collect();

    let status_platform = repo
        .status(gix::progress::Discard)?
        // Here we discard the `status.showUntrackedFiles` config, as it makes little sense in
        // our case to not list new (untracked) files. We could have respected this config
        // if the default value weren't `Collapsed` though, as this default value would render
        // the feature unusable to many.
        .untracked_files(untracked)
        // Rename detection between index and worktree is deliberately OFF: to
        // find content matches gix reads and hashes untracked files, which
        // measured as roughly half the total scan time on a large repo with a
        // few thousand untracked files, worse still on network filesystems. A
        // renamed file surfaces as an untracked + deleted pair instead.
        .index_worktree_rewrites(None::<Rewrites>)
        .head_tree(head_tree_id)
        // Staged renames use already-hashed objects; this does not walk or
        // hash the untracked worktree files.
        .tree_index_track_renames(gix::status::tree_index::TrackRenames::AsConfigured);

    // No filtering based on path
    let empty_patterns = vec![];

    // Yields head-tree<->index and index<->worktree changes. Under the `parallel`
    // feature the two run concurrently with the dirwalk, so the added comparison
    // costs no extra filesystem I/O: it reads objects, never the worktree.
    let status_iter = status_platform.into_iter(empty_patterns)?;

    // A file can be reported by both halves (staged, then modified again). Emit
    // it once. Ordering between the halves is explicitly undefined under
    // `parallel`, so which half labels it is not guaranteed; every label here
    // means "changed", and the picker keys on the path.
    let mut seen = std::collections::HashSet::new();

    for item in status_iter {
        let Ok(item) = item.map_err(|err| f(Err(err.into()))) else {
            continue;
        };
        let change = match item {
            gix::status::Item::IndexWorktree(item) => match item {
                Item::Modification {
                    rela_path, status, ..
                } => {
                    let path = work_dir.join(rela_path.to_path()?);
                    match status {
                        EntryStatus::Conflict { .. } => FileChange::Conflict { path },
                        EntryStatus::Change(Change::Removed) => FileChange::Deleted { path },
                        EntryStatus::Change(Change::Modification { .. }) => {
                            FileChange::Modified { path }
                        }
                        _ => continue,
                    }
                }
                Item::DirectoryContents { entry, .. } if entry.status == Status::Untracked => {
                    FileChange::Untracked {
                        path: work_dir.join(entry.rela_path.to_path()?),
                    }
                }
                Item::Rewrite {
                    source,
                    dirwalk_entry,
                    ..
                } => FileChange::Renamed {
                    from_path: work_dir.join(source.rela_path().to_path()?),
                    to_path: work_dir.join(dirwalk_entry.rela_path.to_path()?),
                },
                _ => continue,
            },
            gix::status::Item::TreeIndex(change) => {
                use gix::diff::index::ChangeRef;
                match change {
                    // Staged additions are tracked, so they are a modification
                    // against HEAD rather than an untracked file.
                    ChangeRef::Addition {
                        location,
                        entry_mode,
                        ..
                    }
                    | ChangeRef::Modification {
                        location,
                        entry_mode,
                        ..
                    } => {
                        if !is_file_mode(entry_mode) {
                            continue;
                        }
                        FileChange::Modified {
                            path: work_dir.join(gix::path::from_bstr(location.as_ref())),
                        }
                    }
                    ChangeRef::Deletion {
                        location,
                        entry_mode,
                        ..
                    } => {
                        if !is_file_mode(entry_mode) {
                            continue;
                        }
                        FileChange::Deleted {
                            path: work_dir.join(gix::path::from_bstr(location.as_ref())),
                        }
                    }
                    ChangeRef::Rewrite {
                        source_location,
                        location,
                        ..
                    } => FileChange::Renamed {
                        from_path: work_dir.join(gix::path::from_bstr(source_location.as_ref())),
                        to_path: work_dir.join(gix::path::from_bstr(location.as_ref())),
                    },
                }
            }
        };

        let change = if conflicted.contains(change.path()) {
            FileChange::Conflict {
                path: change.path().to_path_buf(),
            }
        } else {
            change
        };
        if !seen.insert(change.path().to_path_buf())
            && !matches!(change, FileChange::Renamed { .. })
        {
            continue;
        }
        if !f(Ok(change)) {
            break;
        }
    }

    Ok(())
}

/// The file does not exist in the requested revision (e.g. untracked or newly added).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileNotFoundInRevision;

impl std::fmt::Display for FileNotFoundInRevision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("file is untracked")
    }
}

impl std::error::Error for FileNotFoundInRevision {}

/// Finds the object that contains the contents of a file at a specific commit.
fn find_file_in_tree(repo: &Repository, tree: &Tree, file: &Path) -> Result<ObjectId> {
    let repo_dir = repo.workdir().context("repo has no worktree")?;
    let rel_path = file.strip_prefix(repo_dir)?;
    let tree_entry = tree
        .lookup_entry_by_path(rel_path)?
        .ok_or(FileNotFoundInRevision)?;
    match tree_entry.mode().kind() {
        // not a file, everything is new, do not show diff
        mode @ (EntryKind::Tree | EntryKind::Commit | EntryKind::Link) => {
            bail!("entry at {} is not a file but a {mode:?}", file.display())
        }
        // found a file
        EntryKind::Blob | EntryKind::BlobExecutable => Ok(tree_entry.object_id()),
    }
}
