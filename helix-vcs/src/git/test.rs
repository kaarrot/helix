use std::{
    cell::RefCell,
    fs::{self, File},
    io::Write,
    path::Path,
    process::{Command, Output},
};

use tempfile::TempDir;

use crate::{git, FileChange};

fn exec_git(args: &[&str], git_dir: &Path) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(git_dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env("GIT_TERMINAL_PROMPT", "false")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .env("GIT_AUTHOR_DATE", "2000-01-01 00:00:00 +0000")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_AUTHOR_NAME", "author")
        .env("GIT_COMMITTER_DATE", "2000-01-02 00:00:00 +0000")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_COMMITTER_NAME", "committer")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_CONFIG_KEY_1", "init.defaultBranch")
        .env("GIT_CONFIG_VALUE_1", "main")
        .output()
        .unwrap_or_else(|_| panic!("`git {}` failed", args.join(" ")))
}

fn exec_git_cmd(args: &str, git_dir: &Path) {
    let res = exec_git(&args.split_whitespace().collect::<Vec<_>>(), git_dir);
    if !res.status.success() {
        println!("{}", String::from_utf8_lossy(&res.stdout));
        eprintln!("{}", String::from_utf8_lossy(&res.stderr));
        panic!("`git {args}` failed (see output above)")
    }
}

fn exec_git_cmd_args(args: &[&str], git_dir: &Path) {
    let res = exec_git(args, git_dir);
    if !res.status.success() {
        println!("{}", String::from_utf8_lossy(&res.stdout));
        eprintln!("{}", String::from_utf8_lossy(&res.stderr));
        panic!("`git {}` failed (see output above)", args.join(" "))
    }
}

fn create_commit(repo: &Path, add_modified: bool) {
    if add_modified {
        exec_git_cmd("add -A", repo);
    }
    exec_git_cmd("commit -m message", repo);
}

fn create_commit_with_message(repo: &Path, message: &str) {
    exec_git_cmd_args(&["add", "-A"], repo);
    exec_git_cmd_args(&["commit", "-m", message], repo);
}

fn empty_git_repo() -> TempDir {
    let tmp = tempfile::tempdir().expect("create temp dir for git testing");
    exec_git_cmd("init", tmp.path());
    exec_git_cmd("config user.email test@helix.org", tmp.path());
    exec_git_cmd("config user.name helix-test", tmp.path());
    tmp
}

fn exec_git_cmd_output(args: &str, git_dir: &Path) -> String {
    let output = exec_git(&args.split_whitespace().collect::<Vec<_>>(), git_dir);
    if !output.status.success() {
        println!("{}", String::from_utf8_lossy(&output.stdout));
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        panic!("`git {args}` failed (see output above)")
    }
    String::from_utf8(output.stdout)
        .expect("git output is not valid UTF-8")
        .trim()
        .to_string()
}

fn write_repo_file(repo: &Path, relative_path: &str, contents: &str) {
    let path = repo.join(relative_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn checkout_new_branch(repo: &Path, branch: &str) {
    exec_git_cmd_args(&["checkout", "-b", branch], repo);
}

fn checkout_branch(repo: &Path, branch: &str) {
    exec_git_cmd_args(&["checkout", branch], repo);
}

fn merge_expect_conflict(repo: &Path, branch: &str) {
    let output = exec_git(&["merge", branch], repo);
    assert!(
        !output.status.success(),
        "expected `git merge {branch}` to conflict"
    );
}

#[test]
fn missing_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    File::create(&file).unwrap().write_all(b"foo").unwrap();
    assert!(git::get_diff_base(&file).is_err());
}

#[test]
fn unmodified_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = b"foo".as_slice();
    File::create(&file).unwrap().write_all(contents).unwrap();
    create_commit(temp_git.path(), true);
    assert_eq!(git::get_diff_base(&file).unwrap(), Vec::from(contents));
}

#[test]
fn modified_file() {
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = b"foo".as_slice();
    File::create(&file).unwrap().write_all(contents).unwrap();
    create_commit(temp_git.path(), true);
    File::create(&file).unwrap().write_all(b"bar").unwrap();
    assert_eq!(git::get_diff_base(&file).unwrap(), Vec::from(contents));
}

/// Test that `get_file_head` does not return content for a directory.
#[test]
fn directory() {
    let temp_git = empty_git_repo();
    let dir = temp_git.path().join("file.txt");
    std::fs::create_dir(&dir).expect("");
    let file = dir.join("file.txt");
    let contents = b"foo".as_slice();
    File::create(file).unwrap().write_all(contents).unwrap();
    create_commit(temp_git.path(), true);
    std::fs::remove_dir_all(&dir).unwrap();
    File::create(&dir).unwrap().write_all(b"bar").unwrap();
    assert!(git::get_diff_base(&dir).is_err());
}

#[cfg(any(unix, windows))]
#[test]
fn symlink() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    use std::os::windows::fs::symlink_file as symlink;

    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = Vec::from(b"foo");
    File::create(&file).unwrap().write_all(&contents).unwrap();
    let file_link = temp_git.path().join("file_link.txt");
    symlink("file.txt", &file_link).unwrap();
    create_commit(temp_git.path(), true);
    assert_eq!(git::get_diff_base(&file_link).unwrap(), contents);
    assert_eq!(git::get_diff_base(&file).unwrap(), contents);
}

#[cfg(any(unix, windows))]
#[test]
fn symlink_to_git_repo() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    use std::os::windows::fs::symlink_file as symlink;

    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let temp_git = empty_git_repo();
    let file = temp_git.path().join("file.txt");
    let contents = Vec::from(b"foo");
    File::create(&file).unwrap().write_all(&contents).unwrap();
    create_commit(temp_git.path(), true);
    let file_link = temp_dir.path().join("file_link.txt");
    symlink(&file, &file_link).unwrap();
    assert_eq!(git::get_diff_base(&file_link).unwrap(), contents);
    assert_eq!(git::get_diff_base(&file).unwrap(), contents);
}

#[test]
fn for_each_changed_file_reports_untracked_files() {
    let repo = empty_git_repo();
    write_repo_file(repo.path(), "tracked.txt", "tracked\n");
    create_commit(repo.path(), true);

    write_repo_file(repo.path(), "untracked.txt", "new\n");

    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file(repo.path(), |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert!(changes.iter().any(|change| {
        matches!(change, FileChange::Untracked { path } if path.ends_with("untracked.txt"))
    }));
}

#[test]
fn for_each_changed_file_reports_renames() {
    let repo = empty_git_repo();
    write_repo_file(repo.path(), "old-name.txt", "same content\n");
    create_commit(repo.path(), true);

    fs::rename(
        repo.path().join("old-name.txt"),
        repo.path().join("new-name.txt"),
    )
    .unwrap();

    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file(repo.path(), |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert!(changes.iter().any(|change| {
        matches!(
            change,
            FileChange::Renamed { from_path, to_path }
                if from_path.ends_with("old-name.txt") && to_path.ends_with("new-name.txt")
        )
    }));
}

#[test]
fn for_each_changed_file_reports_conflicts_and_gets_merge_versions() {
    let repo = empty_git_repo();
    write_repo_file(repo.path(), "conflict.txt", "base\n");
    create_commit_with_message(repo.path(), "base");

    checkout_new_branch(repo.path(), "feature");
    write_repo_file(repo.path(), "conflict.txt", "theirs\n");
    create_commit_with_message(repo.path(), "feature change");

    checkout_branch(repo.path(), "main");
    write_repo_file(repo.path(), "conflict.txt", "ours\n");
    create_commit_with_message(repo.path(), "main change");

    merge_expect_conflict(repo.path(), "feature");

    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file(repo.path(), |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert!(changes.iter().any(|change| {
        matches!(change, FileChange::Conflict { path } if path.ends_with("conflict.txt"))
    }));

    let (ours, theirs) = git::get_merge_versions(&repo.path().join("conflict.txt")).unwrap();
    assert_eq!(String::from_utf8(ours).unwrap(), "ours\n");
    assert_eq!(String::from_utf8(theirs).unwrap(), "theirs\n");
}

#[test]
fn for_each_changed_file_between_refs_respects_callback_cancellation() {
    let repo = empty_git_repo();
    write_repo_file(repo.path(), "file1.txt", "v1\n");
    create_commit_with_message(repo.path(), "first");
    let base = exec_git_cmd_output("rev-parse HEAD", repo.path());

    write_repo_file(repo.path(), "file1.txt", "v2\n");
    write_repo_file(repo.path(), "file2.txt", "new\n");
    create_commit_with_message(repo.path(), "second");

    let changes = RefCell::new(Vec::new());
    let err = git::for_each_changed_file_between_refs(repo.path(), &base, Some("HEAD"), |change| {
        changes.borrow_mut().push(change.unwrap());
        false
    })
    .unwrap_err();

    assert!(err.to_string().contains("delegate cancelled"));
    assert_eq!(changes.into_inner().len(), 1);
}

#[test]
fn get_merge_versions_fails_without_merge_stages() {
    let repo = empty_git_repo();
    write_repo_file(repo.path(), "clean.txt", "clean\n");
    create_commit_with_message(repo.path(), "clean");

    let err = git::get_merge_versions(&repo.path().join("clean.txt")).unwrap_err();
    assert!(err.to_string().contains("OURS (stage 2) not in index"));
}

#[test]
fn parent_ref_suffix_fails_for_root_commit() {
    let repo = empty_git_repo();
    write_repo_file(repo.path(), "root.txt", "root\n");
    create_commit_with_message(repo.path(), "root");

    let err = git::for_each_changed_file_between_refs(repo.path(), "HEAD^", Some("HEAD"), |_| true)
        .unwrap_err();
    assert!(err.to_string().contains("has no parent"));
}

// ============================================================================
// Tests for for_each_changed_file_between_refs()
// ============================================================================

#[test]
fn for_each_changed_file_base_to_working_tree() {
    let repo = empty_git_repo();

    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"content1")
        .unwrap();
    exec_git_cmd("add file1.txt", repo.path());
    exec_git_cmd("commit -m first", repo.path());
    let commit1_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());

    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"content1 modified")
        .unwrap();
    File::create(repo.path().join("file2.txt"))
        .unwrap()
        .write_all(b"content2")
        .unwrap();
    exec_git_cmd("add .", repo.path());
    exec_git_cmd("commit -m second", repo.path());

    File::create(repo.path().join("file3.txt"))
        .unwrap()
        .write_all(b"content3")
        .unwrap();
    exec_git_cmd("add file3.txt", repo.path());
    exec_git_cmd("commit -m third", repo.path());

    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), &commit1_hash, None, |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 3);
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Modified { path } if path.ends_with("file1.txt"))));
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Modified { path } if path.ends_with("file2.txt"))));
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Modified { path } if path.ends_with("file3.txt"))));
}

#[test]
fn for_each_changed_file_includes_working_tree() {
    let repo = empty_git_repo();

    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"content1")
        .unwrap();
    exec_git_cmd("add file1.txt", repo.path());
    exec_git_cmd("commit -m first", repo.path());
    let commit1_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());

    File::create(repo.path().join("file2.txt"))
        .unwrap()
        .write_all(b"content2")
        .unwrap();
    exec_git_cmd("add file2.txt", repo.path());
    exec_git_cmd("commit -m second", repo.path());

    // Modify file1 in working tree only (not committed)
    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"working tree change")
        .unwrap();

    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), &commit1_hash, None, |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Modified { path } if path.ends_with("file1.txt"))));
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Modified { path } if path.ends_with("file2.txt"))));
}

#[test]
fn for_each_changed_file_commit_range() {
    let repo = empty_git_repo();

    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"v1")
        .unwrap();
    exec_git_cmd("add file1.txt", repo.path());
    exec_git_cmd("commit -m c1", repo.path());
    let hash1 = exec_git_cmd_output("rev-parse HEAD", repo.path());

    File::create(repo.path().join("file2.txt"))
        .unwrap()
        .write_all(b"v2")
        .unwrap();
    exec_git_cmd("add file2.txt", repo.path());
    exec_git_cmd("commit -m c2", repo.path());

    File::create(repo.path().join("file3.txt"))
        .unwrap()
        .write_all(b"v3")
        .unwrap();
    exec_git_cmd("add file3.txt", repo.path());
    exec_git_cmd("commit -m c3", repo.path());
    let hash3 = exec_git_cmd_output("rev-parse HEAD", repo.path());

    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), &hash1, Some(&hash3), |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 2);
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Modified { path } if path.ends_with("file2.txt"))));
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Modified { path } if path.ends_with("file3.txt"))));
}

#[test]
fn for_each_changed_file_short_hash() {
    let repo = empty_git_repo();

    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"content")
        .unwrap();
    exec_git_cmd("add file1.txt", repo.path());
    exec_git_cmd("commit -m initial", repo.path());

    File::create(repo.path().join("file2.txt"))
        .unwrap()
        .write_all(b"content2")
        .unwrap();
    exec_git_cmd("add file2.txt", repo.path());
    exec_git_cmd("commit -m second", repo.path());
    let full_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());
    let short_hash = &full_hash[..7];

    File::create(repo.path().join("file3.txt"))
        .unwrap()
        .write_all(b"content3")
        .unwrap();
    exec_git_cmd("add file3.txt", repo.path());
    exec_git_cmd("commit -m third", repo.path());

    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), short_hash, None, |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 1);
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Modified { path } if path.ends_with("file3.txt"))));
}

#[test]
fn for_each_changed_file_parent_ref_suffix() {
    let repo = empty_git_repo();

    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"v1")
        .unwrap();
    exec_git_cmd("add file1.txt", repo.path());
    exec_git_cmd("commit -m c1", repo.path());
    let hash1 = exec_git_cmd_output("rev-parse HEAD", repo.path());

    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"v2")
        .unwrap();
    File::create(repo.path().join("file2.txt"))
        .unwrap()
        .write_all(b"new")
        .unwrap();
    exec_git_cmd("add .", repo.path());
    exec_git_cmd("commit -m c2", repo.path());
    let hash2 = exec_git_cmd_output("rev-parse HEAD", repo.path());

    File::create(repo.path().join("file3.txt"))
        .unwrap()
        .write_all(b"filler")
        .unwrap();
    exec_git_cmd("add file3.txt", repo.path());
    exec_git_cmd("commit -m c3", repo.path());

    let changes_explicit = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), &hash1, Some(&hash2), |change| {
        changes_explicit.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let hash2_parent = format!("{}^", hash2);
    let changes_caret = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), &hash2_parent, Some(&hash2), |change| {
        changes_caret.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    assert_eq!(changes_explicit.into_inner().len(), 2);
    assert_eq!(changes_caret.into_inner().len(), 2);

    let base_from_hash1 =
        git::get_diff_base_from_ref(&repo.path().join("file1.txt"), &hash1).unwrap();
    let base_from_parent =
        git::get_diff_base_from_ref(&repo.path().join("file1.txt"), &hash2_parent).unwrap();
    assert_eq!(base_from_hash1, base_from_parent);
}

#[test]
fn for_each_changed_file_deleted() {
    let repo = empty_git_repo();

    File::create(repo.path().join("keep.txt"))
        .unwrap()
        .write_all(b"keep")
        .unwrap();
    File::create(repo.path().join("delete.txt"))
        .unwrap()
        .write_all(b"delete")
        .unwrap();
    exec_git_cmd("add .", repo.path());
    exec_git_cmd("commit -m first", repo.path());
    let commit1_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());

    std::fs::remove_file(repo.path().join("delete.txt")).unwrap();
    exec_git_cmd("add .", repo.path());
    exec_git_cmd("commit -m second", repo.path());

    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), &commit1_hash, None, |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 1);
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Deleted { path } if path.ends_with("delete.txt"))));
}

#[test]
fn for_each_changed_file_deleted_in_range() {
    let repo = empty_git_repo();

    File::create(repo.path().join("keep.txt"))
        .unwrap()
        .write_all(b"keep")
        .unwrap();
    File::create(repo.path().join("delete.txt"))
        .unwrap()
        .write_all(b"delete")
        .unwrap();
    exec_git_cmd("add .", repo.path());
    exec_git_cmd("commit -m first", repo.path());
    let commit1_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());

    std::fs::remove_file(repo.path().join("delete.txt")).unwrap();
    exec_git_cmd("add .", repo.path());
    exec_git_cmd("commit -m second", repo.path());
    let commit2_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());

    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(
        repo.path(),
        &commit1_hash,
        Some(&commit2_hash),
        |change| {
            changes.borrow_mut().push(change.unwrap());
            true
        },
    )
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 1);
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Deleted { path } if path.ends_with("delete.txt"))));
}

#[test]
fn for_each_changed_file_head_vs_working_tree() {
    let repo = empty_git_repo();

    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"committed")
        .unwrap();
    exec_git_cmd("add file1.txt", repo.path());
    exec_git_cmd("commit -m initial", repo.path());

    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"modified in working tree")
        .unwrap();

    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), "HEAD", None, |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 1);
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Modified { path } if path.ends_with("file1.txt"))));
}

#[test]
fn for_each_changed_file_multiple_file_types() {
    let repo = empty_git_repo();

    File::create(repo.path().join("existing.txt"))
        .unwrap()
        .write_all(b"existing")
        .unwrap();
    File::create(repo.path().join("modify.txt"))
        .unwrap()
        .write_all(b"will modify")
        .unwrap();
    File::create(repo.path().join("delete.txt"))
        .unwrap()
        .write_all(b"will delete")
        .unwrap();
    exec_git_cmd("add .", repo.path());
    exec_git_cmd("commit -m first", repo.path());
    let commit1_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());

    File::create(repo.path().join("modify.txt"))
        .unwrap()
        .write_all(b"modified")
        .unwrap();
    std::fs::remove_file(repo.path().join("delete.txt")).unwrap();
    File::create(repo.path().join("new.txt"))
        .unwrap()
        .write_all(b"new file")
        .unwrap();
    exec_git_cmd("add .", repo.path());
    exec_git_cmd("commit -m second", repo.path());

    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), &commit1_hash, None, |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 3);
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Modified { path } if path.ends_with("modify.txt"))));
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Deleted { path } if path.ends_with("delete.txt"))));
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Modified { path } if path.ends_with("new.txt"))));
}

#[test]
fn for_each_changed_file_from_root_commit() {
    let repo = empty_git_repo();

    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"initial")
        .unwrap();
    exec_git_cmd("add file1.txt", repo.path());
    exec_git_cmd("commit -m initial", repo.path());
    let commit_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());

    File::create(repo.path().join("file2.txt"))
        .unwrap()
        .write_all(b"second")
        .unwrap();
    exec_git_cmd("add file2.txt", repo.path());
    exec_git_cmd("commit -m second", repo.path());

    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), &commit_hash, None, |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 1);
    assert!(changes
        .iter()
        .any(|c| matches!(c, FileChange::Modified { path } if path.ends_with("file2.txt"))));
}
