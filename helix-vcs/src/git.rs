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

pub fn for_each_changed_file(cwd: &Path, f: impl Fn(Result<FileChange>) -> bool) -> Result<()> {
    status(&open_repo(cwd)?.to_thread_local(), f)
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
/// `rev` accepts a ref, tag, short or full hash, with an optional `^` suffix,
/// and is always resolved to a full hash so the resulting link is a permalink.
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
                    EntryStatus::Conflict(_) => FileChange::Conflict { path },
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
