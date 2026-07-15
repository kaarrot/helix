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
use gix::{Commit, ObjectId, Repository, ThreadSafeRepository};

use crate::FileChange;

#[cfg(test)]
mod test;

#[inline]
fn get_repo_dir(file: &Path) -> Result<&Path> {
    file.parent().context("file has no parent directory")
}

/// Resolve a file path, handling the case where the file doesn't exist yet
/// (e.g. a deleted file being diffed). Falls back to resolving the parent directory.
fn resolve_file_path(file: &Path) -> Result<PathBuf> {
    if file.exists() {
        gix::path::realpath(file).context("resolve symlinks")
    } else {
        let parent = get_repo_dir(file)?;
        let parent = gix::path::realpath(parent).context("resolve symlinks for parent")?;
        let file_name = file.file_name().context("file has no file name")?;
        Ok(parent.join(file_name))
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
    let file_oid = find_file_in_commit(&repo, &head, &file)?;

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

    let commit = resolve_commit(&repo, ref_name)?;
    let file_oid = find_file_in_commit(&repo, &commit, &file)?;

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

/// Fetch OURS (stage 2) and THEIRS (stage 3) versions of a conflicted file
/// from the git index. Returns `(ours_bytes, theirs_bytes)`.
pub fn get_merge_versions(file: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    debug_assert!(file.is_absolute());
    let file = resolve_file_path(file)?;

    let repo_dir = get_repo_dir(&file)?;
    let repo = open_repo(repo_dir)
        .context("failed to open git repo")?
        .to_thread_local();

    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow::anyhow!("not a working tree"))?;
    let rela_path = file.strip_prefix(work_dir)?;
    let rela_path = gix::path::try_into_bstr(rela_path)?;

    let index = repo.index_or_load_from_head()?;
    let mut ours_oid = None;
    let mut theirs_oid = None;

    for entry in index.entries() {
        if entry.path(&index) == rela_path.as_ref() {
            match entry.stage() {
                gix::index::entry::Stage::Ours => ours_oid = Some(entry.id),
                gix::index::entry::Stage::Theirs => theirs_oid = Some(entry.id),
                _ => {}
            }
        }
    }

    let ours_oid = ours_oid.ok_or_else(|| {
        anyhow::anyhow!("OURS (stage 2) not in index — is there an active merge?")
    })?;
    let theirs_oid = theirs_oid.ok_or_else(|| anyhow::anyhow!("THEIRS (stage 3) not in index"))?;

    let (mut pipeline, _) = repo.filter_pipeline(None)?;

    let ours_data = repo.find_object(ours_oid)?.detach().data;
    let mut ours_out =
        pipeline.convert_to_worktree(&ours_data, rela_path.as_ref(), Delay::Forbid)?;
    let mut ours_buf = Vec::with_capacity(ours_data.len());
    ours_out.read_to_end(&mut ours_buf)?;

    let (mut pipeline2, _) = repo.filter_pipeline(None)?;
    let theirs_data = repo.find_object(theirs_oid)?.detach().data;
    let mut theirs_out =
        pipeline2.convert_to_worktree(&theirs_data, rela_path.as_ref(), Delay::Forbid)?;
    let mut theirs_buf = Vec::with_capacity(theirs_data.len());
    theirs_out.read_to_end(&mut theirs_buf)?;

    Ok((ours_buf, theirs_buf))
}

pub fn for_each_changed_file(cwd: &Path, f: impl Fn(Result<FileChange>) -> bool) -> Result<()> {
    status(&open_repo(cwd)?.to_thread_local(), UntrackedFiles::Files, f)
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
            let base_commit = resolve_commit(&repo, base_ref)?;
            let head_commit = repo.head_commit()?;

            if base_commit.id == head_commit.id {
                return status(&repo, untracked, f);
            }

            let work_dir = repo
                .workdir()
                .ok_or_else(|| anyhow::anyhow!("working tree not found"))?
                .to_path_buf();

            let mut seen = std::collections::HashSet::new();
            let base_tree = base_commit.tree()?;
            let head_tree = head_commit.tree()?;
            let mut cancelled = false;

            base_tree.changes()?.for_each_to_obtain_tree(
                &head_tree,
                |change| -> Result<_, std::convert::Infallible> {
                    use gix::object::tree::diff::Change;
                    let file_change = match change {
                        Change::Addition { entry_mode, .. }
                        | Change::Modification { entry_mode, .. }
                        | Change::Rewrite { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                Some(FileChange::Modified {
                                    path: work_dir.join(gix::path::from_bstr(change.location())),
                                })
                            } else {
                                None
                            }
                        }
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
                    Ok(fc) if seen.contains(fc.path()) => true,
                    _ => f(change),
                })?;
            }
        }
        Some(target) => {
            let base_commit = resolve_commit(&repo, base_ref)?;
            let target_commit = resolve_commit(&repo, target)?;
            let work_dir = repo
                .workdir()
                .ok_or_else(|| anyhow::anyhow!("working tree not found"))?
                .to_path_buf();

            let base_tree = base_commit.tree()?;
            let target_tree = target_commit.tree()?;

            base_tree.changes()?.for_each_to_obtain_tree(
                &target_tree,
                |change| -> Result<_, std::convert::Infallible> {
                    use gix::object::tree::diff::Change;
                    let file_change = match change {
                        Change::Addition { entry_mode, .. }
                        | Change::Modification { entry_mode, .. }
                        | Change::Rewrite { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                Some(FileChange::Modified {
                                    path: work_dir.join(gix::path::from_bstr(change.location())),
                                })
                            } else {
                                None
                            }
                        }
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
/// untracked symlink is reported as an entry, like git).
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

    let walker = ignore::WalkBuilder::new(&work_dir)
        .hidden(false) // git scans dotfiles; `.git` is pruned below
        .parents(true)
        .ignore(false) // honor only git ignore sources, not `.ignore` files
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|entry| entry.file_name() != ".git")
        .threads(0) // 0 = pick a sensible default based on CPUs
        .build_parallel();

    walker.run(|| {
        Box::new(|result| {
            if let Ok(entry) = result {
                // Files and symlinks (anything that isn't a directory); git
                // lists an untracked symlink as its own entry.
                if entry.file_type().map_or(false, |ft| !ft.is_dir()) {
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
                Change::Addition { entry_mode, .. }
                | Change::Modification { entry_mode, .. }
                | Change::Rewrite { entry_mode, .. } => {
                    if entry_mode.is_blob() {
                        Some(FileChange::Modified {
                            path: work_dir.join(gix::path::from_bstr(change.location())),
                        })
                    } else {
                        None
                    }
                }
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

/// Resolve a git reference or commit hash (full or short, with optional `^` parent suffix).
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

    if let Ok(reference) = repo.find_reference(ref_name) {
        let object_id = reference.into_fully_peeled_id()?.detach();
        return repo
            .find_object(object_id)?
            .try_into_commit()
            .context(format!("'{}' is not a commit", ref_name));
    }

    let prefix = gix::hash::Prefix::from_hex(ref_name).context(format!(
        "'{}' is not a valid reference or commit hash",
        ref_name
    ))?;

    let maybe_oid = repo
        .objects
        .lookup_prefix(prefix, None)
        .context("failed to lookup object by hash")?;

    let object_id = match maybe_oid {
        Some(oid) => oid.map_err(|_| {
            anyhow::anyhow!(
                "ambiguous hash prefix '{}'; try using more characters",
                ref_name
            )
        })?,
        None => bail!("no commit found with hash '{}'", ref_name),
    };

    repo.find_object(object_id)?
        .try_into_commit()
        .context(format!("'{}' is not a commit", ref_name))
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

/// Emulates the result of running `git status` from the command line.
///
/// `untracked` selects how new files are reported. Callers that enumerate
/// untracked files separately (via [`for_each_untracked_file`], which uses a
/// parallel walker) pass [`UntrackedFiles::None`] so gix does no directory
/// walk at all — leaving only the fast index<->worktree modification pass.
fn status(
    repo: &Repository,
    untracked: UntrackedFiles,
    f: impl Fn(Result<FileChange>) -> bool,
) -> Result<()> {
    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow::anyhow!("working tree not found"))?
        .to_path_buf();

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
        .index_worktree_rewrites(None::<Rewrites>);

    // No filtering based on path
    let empty_patterns = vec![];

    let status_iter = status_platform.into_index_worktree_iter(empty_patterns)?;

    for item in status_iter {
        let Ok(item) = item.map_err(|err| f(Err(err.into()))) else {
            continue;
        };
        let change = match item {
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
        };
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
fn find_file_in_commit(repo: &Repository, commit: &Commit, file: &Path) -> Result<ObjectId> {
    let repo_dir = repo.workdir().context("repo has no worktree")?;
    let rel_path = file.strip_prefix(repo_dir)?;
    let tree = commit.tree()?;
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
