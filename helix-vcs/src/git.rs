use anyhow::{bail, Context, Result};
use arc_swap::ArcSwap;
use gix::filter::plumbing::driver::apply::Delay;
use std::io::Read;
use std::path::Path;
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

pub fn get_diff_base(file: &Path) -> Result<Vec<u8>> {
    debug_assert!(!file.exists() || file.is_file());
    debug_assert!(file.is_absolute());
    let file = gix::path::realpath(file).context("resolve symlinks")?;

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
    let file = gix::path::realpath(file).context("resolve symlinks")?;

    let repo_dir = get_repo_dir(&file)?;
    let repo = open_repo(repo_dir)
        .context("failed to open git repo")?
        .to_thread_local();

    // Resolve the reference or commit hash
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
    let file = gix::path::realpath(file).context("resolve symlinks")?;

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

/// Get the OURS and THEIRS versions of a conflicted file during a merge.
///
/// Returns (ours_content, theirs_content) where:
/// - ours_content: Stage 2 in git index (current branch/HEAD)
/// - theirs_content: Stage 3 in git index (incoming branch being merged)
pub fn get_merge_versions(file: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    debug_assert!(!file.exists() || file.is_file());
    debug_assert!(file.is_absolute());
    let file = gix::path::realpath(file).context("resolve symlinks")?;

    let repo_dir = get_repo_dir(&file)?;
    let repo = open_repo(repo_dir)
        .context("failed to open git repo")?
        .to_thread_local();

    let work_dir = repo
        .workdir()
        .ok_or_else(|| anyhow::anyhow!("working tree not found"))?;
    let rela_path = file.strip_prefix(work_dir)?;
    let rela_path = gix::path::try_into_bstr(rela_path)?;

    // Access git index to get staged versions
    let index = repo.index_or_load_from_head()?;

    // Find stage 2 (OURS) and stage 3 (THEIRS) entries
    let mut ours_oid = None;
    let mut theirs_oid = None;

    for entry in index.entries() {
        if entry.path(&index) == rela_path.as_ref() {
            let stage = entry.stage();
            match stage {
                gix::index::entry::Stage::Ours => ours_oid = Some(entry.id),
                gix::index::entry::Stage::Theirs => theirs_oid = Some(entry.id),
                _ => {}
            }
        }
    }

    let ours_oid = ours_oid.ok_or_else(|| {
        anyhow::anyhow!("OURS version (stage 2) not found in index for conflicted file")
    })?;
    let theirs_oid = theirs_oid.ok_or_else(|| {
        anyhow::anyhow!("THEIRS version (stage 3) not found in index for conflicted file")
    })?;

    // Read the blob contents
    let ours_object = repo.find_object(ours_oid)?;
    let theirs_object = repo.find_object(theirs_oid)?;

    let ours_data = ours_object.detach().data;
    let theirs_data = theirs_object.detach().data;

    // Apply git filters (like crlf conversion) to both versions
    let (mut pipeline, _) = repo.filter_pipeline(None)?;

    let mut ours_worktree = pipeline.convert_to_worktree(&ours_data, rela_path.as_ref(), Delay::Forbid)?;
    let mut ours_buf = Vec::with_capacity(ours_data.len());
    ours_worktree.read_to_end(&mut ours_buf)?;

    // Re-create pipeline for theirs to avoid borrow issues
    let (mut pipeline2, _) = repo.filter_pipeline(None)?;
    let mut theirs_worktree = pipeline2.convert_to_worktree(&theirs_data, rela_path.as_ref(), Delay::Forbid)?;
    let mut theirs_buf = Vec::with_capacity(theirs_data.len());
    theirs_worktree.read_to_end(&mut theirs_buf)?;

    Ok((ours_buf, theirs_buf))
}

/// Resolve a git reference or commit hash to a Commit object
/// Supports: branch names, tags, full commit hashes, short hashes, and ^ (parent) suffix
fn resolve_commit<'a>(repo: &'a Repository, ref_name: &str) -> Result<Commit<'a>> {
    // Handle ^ (parent) suffix: e.g. "abc123^" means parent of abc123
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

    // First try as a reference (branch, tag, etc.)
    if let Ok(reference) = repo.find_reference(ref_name) {
        let object_id = reference.into_fully_peeled_id()?.detach();
        return repo.find_object(object_id)?.try_into_commit()
            .context(format!("'{}' is not a commit", ref_name));
    }

    // Not a reference, try as a commit hash (full or partial)
    // Use gix's prefix resolution which handles both short and full hashes
    let prefix = gix::hash::Prefix::from_hex(ref_name)
        .context(format!("'{}' is not a valid reference or commit hash", ref_name))?;

    let maybe_oid = repo.objects
        .lookup_prefix(prefix, None)
        .context("failed to lookup object by hash")?;

    let object_id = match maybe_oid {
        Some(oid) => oid.map_err(|_| {
            anyhow::anyhow!(
                "ambiguous hash prefix '{}' matches multiple objects. \
                Try using at least 12 characters of the commit hash. \
                Get the full hash with: git log --oneline --all | grep {}",
                ref_name, ref_name
            )
        })?,
        None => bail!("no commit found with hash '{}'", ref_name),
    };

    // Verify it's actually a commit object
    let object = repo.find_object(object_id)?;
    if !matches!(object.kind, gix::object::Kind::Commit) {
        bail!(
            "'{}' resolves to a {} object, not a commit. Use a commit hash instead.",
            ref_name,
            object.kind
        );
    }

    object.try_into_commit()
        .context(format!("'{}' is not a commit", ref_name))
}

pub fn for_each_changed_file(cwd: &Path, f: impl Fn(Result<FileChange>) -> bool) -> Result<()> {
    status(&open_repo(cwd)?.to_thread_local(), f)
}

/// Iterate over changed files between two references
/// - If target_ref is None: compare base_ref vs working tree
/// - If target_ref is Some: compare base_ref vs target_ref (two commits)
pub fn for_each_changed_file_between_refs(
    cwd: &Path,
    base_ref: &str,
    target_ref: Option<&str>,
    f: impl Fn(Result<FileChange>) -> bool,
) -> Result<()> {
    let repo = open_repo(cwd)?.to_thread_local();

    match target_ref {
        // Compare base_ref vs working tree (all changes since base_ref)
        None => {
            let base_commit = resolve_commit(&repo, base_ref)?;
            let head_commit = repo.head_commit()?;

            // Fast path: if base_ref is HEAD, just use status for efficiency
            if base_commit.id == head_commit.id {
                return status(&repo, f);
            }

            let work_dir = repo.workdir()
                .ok_or_else(|| anyhow::anyhow!("working tree not found"))?
                .to_path_buf();

            // Collect paths already reported from the tree diff so we don't
            // duplicate them when we also run status() below.
            let mut seen = std::collections::HashSet::new();

            // 1. Diff base_ref tree → HEAD tree (committed changes since base_ref)
            let base_tree = base_commit.tree()?;
            let head_tree = head_commit.tree()?;
            let mut cancelled = false;

            base_tree.changes()?.for_each_to_obtain_tree(
                &head_tree,
                |change| -> Result<_, std::convert::Infallible> {
                    use gix::object::tree::diff::Change;

                    let file_change = match change {
                        Change::Addition { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                let path = work_dir.join(gix::path::from_bstr(change.location()));
                                Some(FileChange::Modified { path })
                            } else {
                                None
                            }
                        }
                        Change::Deletion { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                let path = work_dir.join(gix::path::from_bstr(change.location()));
                                Some(FileChange::Deleted { path })
                            } else {
                                None
                            }
                        }
                        Change::Modification { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                let path = work_dir.join(gix::path::from_bstr(change.location()));
                                Some(FileChange::Modified { path })
                            } else {
                                None
                            }
                        }
                        Change::Rewrite { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                let path = work_dir.join(gix::path::from_bstr(change.location()));
                                Some(FileChange::Modified { path })
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

            if cancelled {
                return Ok(());
            }

            // 2. Also include working tree changes (unstaged/untracked)
            //    that weren't already covered by the tree diff
            status(&repo, |change| {
                match &change {
                    Ok(fc) if seen.contains(fc.path()) => true, // skip, already reported
                    _ => f(change),
                }
            })?;

            return Ok(())
        }
        // Compare two commits
        Some(target) => {
            let base_commit = resolve_commit(&repo, base_ref)
                .context(format!("Failed to resolve base ref '{}'", base_ref))?;
            let target_commit = resolve_commit(&repo, target)
                .context(format!("Failed to resolve target ref '{}'", target))?;
            let work_dir = repo
                .workdir()
                .ok_or_else(|| anyhow::anyhow!("working tree not found"))?
                .to_path_buf();

            // Get trees from both commits
            let base_tree = base_commit.tree()?;
            let target_tree = target_commit.tree()?;

            // Diff the trees (from base to target)
            base_tree.changes()?.for_each_to_obtain_tree(
                &target_tree,
                |change| -> Result<_, std::convert::Infallible> {
                    use gix::object::tree::diff::Change;

                    let file_change = match change {
                        Change::Addition { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                let path = work_dir.join(gix::path::from_bstr(change.location()));
                                Some(FileChange::Modified { path })
                            } else {
                                None
                            }
                        }
                        Change::Deletion { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                let path = work_dir.join(gix::path::from_bstr(change.location()));
                                Some(FileChange::Deleted { path })
                            } else {
                                None
                            }
                        }
                        Change::Modification { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                let path = work_dir.join(gix::path::from_bstr(change.location()));
                                Some(FileChange::Modified { path })
                            } else {
                                None
                            }
                        }
                        Change::Rewrite { entry_mode, .. } => {
                            if entry_mode.is_blob() {
                                let path = work_dir.join(gix::path::from_bstr(change.location()));
                                Some(FileChange::Modified { path })
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
    }
}

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

    // Get HEAD commit
    let head_commit = repo.head_commit()?;

    // Resolve the target commit (supporting short hashes)
    let target_commit = resolve_commit(&repo, ref_name)?;

    // Get trees from both commits
    let head_tree = head_commit.tree()?;
    let target_tree = target_commit.tree()?;

    // Diff the trees
    target_tree.changes()?.for_each_to_obtain_tree(
        &head_tree,
        |change| -> Result<_, std::convert::Infallible> {
            use gix::object::tree::diff::Change;

            let file_change = match change {
                Change::Addition { entry_mode, .. } => {
                    if entry_mode.is_blob() {
                        let path = work_dir.join(gix::path::from_bstr(change.location()));
                        Some(FileChange::Modified { path })
                    } else {
                        None
                    }
                }
                Change::Deletion { entry_mode, .. } => {
                    if entry_mode.is_blob() {
                        let path = work_dir.join(gix::path::from_bstr(change.location()));
                        Some(FileChange::Deleted { path })
                    } else {
                        None
                    }
                }
                Change::Modification { entry_mode, .. } => {
                    if entry_mode.is_blob() {
                        let path = work_dir.join(gix::path::from_bstr(change.location()));
                        Some(FileChange::Modified { path })
                    } else {
                        None
                    }
                }
                Change::Rewrite { entry_mode, .. } => {
                    // Handle rewrites (renames) if needed
                    if entry_mode.is_blob() {
                        let path = work_dir.join(gix::path::from_bstr(change.location()));
                        Some(FileChange::Modified { path })
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
fn status(repo: &Repository, f: impl Fn(Result<FileChange>) -> bool) -> Result<()> {
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
        .untracked_files(UntrackedFiles::Files)
        // Turn on file rename detection, which is off by default.
        .index_worktree_rewrites(Some(Rewrites {
            copies: None,
            percentage: Some(0.5),
            limit: 1000,
            ..Default::default()
        }));

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

/// Finds the object that contains the contents of a file at a specific commit.
fn find_file_in_commit(repo: &Repository, commit: &Commit, file: &Path) -> Result<ObjectId> {
    let repo_dir = repo.workdir().context("repo has no worktree")?;
    let rel_path = file.strip_prefix(repo_dir)?;
    let tree = commit.tree()?;
    let tree_entry = tree
        .lookup_entry_by_path(rel_path)?
        .context("file is untracked")?;
    match tree_entry.mode().kind() {
        // not a file, everything is new, do not show diff
        mode @ (EntryKind::Tree | EntryKind::Commit | EntryKind::Link) => {
            bail!("entry at {} is not a file but a {mode:?}", file.display())
        }
        // found a file
        EntryKind::Blob | EntryKind::BlobExecutable => Ok(tree_entry.object_id()),
    }
}
