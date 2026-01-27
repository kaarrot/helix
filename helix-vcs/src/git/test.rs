use std::{cell::RefCell, fs::File, io::Write, path::Path, process::Command};

use tempfile::TempDir;

use crate::{git, FileChange};

fn exec_git_cmd(args: &str, git_dir: &Path) {
    let res = Command::new("git")
        .arg("-C")
        .arg(git_dir) // execute the git command in this directory
        .args(args.split_whitespace())
        .env_remove("GIT_DIR")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env("GIT_TERMINAL_PROMPT", "false")
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
        .unwrap_or_else(|_| panic!("`git {args}` failed"));
    if !res.status.success() {
        println!("{}", String::from_utf8_lossy(&res.stdout));
        eprintln!("{}", String::from_utf8_lossy(&res.stderr));
        panic!("`git {args}` failed (see output above)")
    }
}

fn create_commit(repo: &Path, add_modified: bool) {
    if add_modified {
        exec_git_cmd("add -A", repo);
    }
    exec_git_cmd("commit -m message", repo);
}

fn empty_git_repo() -> TempDir {
    let tmp = tempfile::tempdir().expect("create temp dir for git testing");
    exec_git_cmd("init", tmp.path());
    exec_git_cmd("config user.email test@helix.org", tmp.path());
    exec_git_cmd("config user.name helix-test", tmp.path());
    tmp
}

/// Get command output (for commit hashes)
fn exec_git_cmd_output(args: &str, git_dir: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(git_dir)
        .args(args.split_whitespace())
        .env_remove("GIT_DIR")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env("GIT_TERMINAL_PROMPT", "false")
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
        .unwrap_or_else(|_| panic!("`git {args}` failed"));

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
/// This is important to correctly cover cases where a directory is removed and replaced by a file.
/// If the contents of the directory object were returned a diff between a path and the directory children would be produced.
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

/// Test that `get_diff_base` resolves symlinks so that the same diff base is
/// used as the target file.
///
/// This is important to correctly cover cases where a symlink is removed and
/// replaced by a file. If the contents of the symlink object were returned
/// a diff between a literal file path and the actual file content would be
/// produced (bad ui).
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

/// Test that `get_diff_base` returns content when the file is a symlink to
/// another file that is in a git repo, but the symlink itself is not.
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

// ============================================================================
// Tests for for_each_changed_file_between_refs()
// ============================================================================

#[test]
fn for_each_changed_file_single_commit() {
    let repo = empty_git_repo();

    // Commit 1: Add file1.txt
    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"content1")
        .unwrap();
    exec_git_cmd("add file1.txt", repo.path());
    exec_git_cmd("commit -m first", repo.path());

    // Commit 2: Modify file1.txt, add file2.txt
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
    let commit2_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());

    // Create another commit so that commit2 is NOT HEAD
    // This is important because the function has a fast path for HEAD
    File::create(repo.path().join("file3.txt"))
        .unwrap()
        .write_all(b"content3")
        .unwrap();
    exec_git_cmd("add file3.txt", repo.path());
    exec_git_cmd("commit -m third", repo.path());

    // Test: diff-commit commit2 (should show file1 modified, file2 added)
    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), &commit2_hash, None, |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().any(|c| matches!(c,
        FileChange::Modified { path } if path.ends_with("file1.txt")
    )));
    assert!(changes.iter().any(|c| matches!(c,
        FileChange::Modified { path } if path.ends_with("file2.txt")
    )));
}

#[test]
fn for_each_changed_file_commit_range() {
    let repo = empty_git_repo();

    // Commit 1: file1.txt
    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"v1")
        .unwrap();
    exec_git_cmd("add file1.txt", repo.path());
    exec_git_cmd("commit -m c1", repo.path());
    let hash1 = exec_git_cmd_output("rev-parse HEAD", repo.path());

    // Commit 2: file2.txt
    File::create(repo.path().join("file2.txt"))
        .unwrap()
        .write_all(b"v2")
        .unwrap();
    exec_git_cmd("add file2.txt", repo.path());
    exec_git_cmd("commit -m c2", repo.path());

    // Commit 3: file3.txt
    File::create(repo.path().join("file3.txt"))
        .unwrap()
        .write_all(b"v3")
        .unwrap();
    exec_git_cmd("add file3.txt", repo.path());
    exec_git_cmd("commit -m c3", repo.path());
    let hash3 = exec_git_cmd_output("rev-parse HEAD", repo.path());

    // Test: diff-commit hash1..hash3 (should show file2 + file3)
    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), &hash1, Some(&hash3), |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().any(|c| matches!(c,
        FileChange::Modified { path } if path.ends_with("file2.txt")
    )));
    assert!(changes.iter().any(|c| matches!(c,
        FileChange::Modified { path } if path.ends_with("file3.txt")
    )));
}

#[test]
fn for_each_changed_file_short_hash() {
    let repo = empty_git_repo();

    // Create a commit
    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"content")
        .unwrap();
    exec_git_cmd("add file1.txt", repo.path());
    exec_git_cmd("commit -m initial", repo.path());

    // Create another commit
    File::create(repo.path().join("file2.txt"))
        .unwrap()
        .write_all(b"content2")
        .unwrap();
    exec_git_cmd("add file2.txt", repo.path());
    exec_git_cmd("commit -m second", repo.path());

    let full_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());
    let short_hash = &full_hash[..7]; // 7-character short hash

    // Create a third commit so the second commit is not HEAD
    File::create(repo.path().join("file3.txt"))
        .unwrap()
        .write_all(b"content3")
        .unwrap();
    exec_git_cmd("add file3.txt", repo.path());
    exec_git_cmd("commit -m third", repo.path());

    // Test: short hash should work
    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), short_hash, None, |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 1);
    assert!(changes.iter().any(|c| matches!(c,
        FileChange::Modified { path } if path.ends_with("file2.txt")
    )));
}

#[test]
fn for_each_changed_file_deleted() {
    let repo = empty_git_repo();

    // Commit 1: Add files
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

    // Commit 2: Delete one file
    std::fs::remove_file(repo.path().join("delete.txt")).unwrap();
    exec_git_cmd("add .", repo.path());
    exec_git_cmd("commit -m second", repo.path());
    let commit2_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());

    // Create a third commit so commit2 is not HEAD
    File::create(repo.path().join("another.txt"))
        .unwrap()
        .write_all(b"another")
        .unwrap();
    exec_git_cmd("add another.txt", repo.path());
    exec_git_cmd("commit -m third", repo.path());

    // Test: should show delete.txt as deleted
    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), &commit2_hash, None, |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 1);
    assert!(changes.iter().any(|c| matches!(c,
        FileChange::Deleted { path } if path.ends_with("delete.txt")
    )));
}

#[test]
fn for_each_changed_file_head_vs_working_tree() {
    let repo = empty_git_repo();

    // Commit 1: Add file
    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"committed")
        .unwrap();
    exec_git_cmd("add file1.txt", repo.path());
    exec_git_cmd("commit -m initial", repo.path());

    // Modify file in working tree (not committed)
    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"modified in working tree")
        .unwrap();

    // Test: diff-commit HEAD should show working tree changes
    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), "HEAD", None, |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 1);
    assert!(changes.iter().any(|c| matches!(c,
        FileChange::Modified { path } if path.ends_with("file1.txt")
    )));
}

#[test]
fn for_each_changed_file_multiple_file_types() {
    let repo = empty_git_repo();

    // Commit 1: Add multiple files
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

    // Commit 2: Modify one, delete one, add one
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
    let commit2_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());

    // Create a third commit so commit2 is not HEAD
    File::create(repo.path().join("yet_another.txt"))
        .unwrap()
        .write_all(b"yet another")
        .unwrap();
    exec_git_cmd("add yet_another.txt", repo.path());
    exec_git_cmd("commit -m third", repo.path());

    // Test: should show all changes
    let changes = RefCell::new(Vec::new());
    git::for_each_changed_file_between_refs(repo.path(), &commit2_hash, None, |change| {
        changes.borrow_mut().push(change.unwrap());
        true
    })
    .unwrap();

    let changes = changes.into_inner();
    assert_eq!(changes.len(), 3);
    assert!(changes.iter().any(|c| matches!(c,
        FileChange::Modified { path } if path.ends_with("modify.txt")
    )));
    assert!(changes.iter().any(|c| matches!(c,
        FileChange::Deleted { path } if path.ends_with("delete.txt")
    )));
    assert!(changes.iter().any(|c| matches!(c,
        FileChange::Modified { path } if path.ends_with("new.txt")
    )));
}

#[test]
fn for_each_changed_file_root_commit_error() {
    let repo = empty_git_repo();

    // Create only one commit (root commit)
    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"initial")
        .unwrap();
    exec_git_cmd("add file1.txt", repo.path());
    exec_git_cmd("commit -m initial", repo.path());
    let commit_hash = exec_git_cmd_output("rev-parse HEAD", repo.path());

    // Create a second commit so the root commit is not HEAD
    File::create(repo.path().join("file2.txt"))
        .unwrap()
        .write_all(b"second")
        .unwrap();
    exec_git_cmd("add file2.txt", repo.path());
    exec_git_cmd("commit -m second", repo.path());

    // Test: trying to diff root commit should fail (no parent)
    let result = git::for_each_changed_file_between_refs(
        repo.path(),
        &commit_hash,
        None,
        |_change| true,
    );

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("no parent") || err_msg.contains("root commit"));
}

// ============================================================================
// Tests for get_merge_versions()
// ============================================================================

#[test]
fn get_merge_versions_basic_conflict() {
    let repo = empty_git_repo();

    // Main branch: file.txt = "main content"
    File::create(repo.path().join("file.txt"))
        .unwrap()
        .write_all(b"main content")
        .unwrap();
    exec_git_cmd("add file.txt", repo.path());
    exec_git_cmd("commit -m main", repo.path());

    // Feature branch: file.txt = "feature content"
    exec_git_cmd("checkout -b feature", repo.path());
    File::create(repo.path().join("file.txt"))
        .unwrap()
        .write_all(b"feature content")
        .unwrap();
    exec_git_cmd("add file.txt", repo.path());
    exec_git_cmd("commit -m feature", repo.path());

    // Back to main, make conflicting change
    exec_git_cmd("checkout main", repo.path());
    File::create(repo.path().join("file.txt"))
        .unwrap()
        .write_all(b"main content 2")
        .unwrap();
    exec_git_cmd("add file.txt", repo.path());
    exec_git_cmd("commit -m main2", repo.path());

    // Attempt merge - this should create a conflict
    let _ = Command::new("git")
        .args(["merge", "feature"])
        .current_dir(repo.path())
        .output();

    // Test: get_merge_versions should return OURS and THEIRS
    let file_path = repo.path().join("file.txt");
    let result = git::get_merge_versions(&file_path);

    assert!(result.is_ok());
    let (ours, theirs) = result.unwrap();
    assert_eq!(String::from_utf8(ours).unwrap(), "main content 2");
    assert_eq!(String::from_utf8(theirs).unwrap(), "feature content");
}

#[test]
fn get_merge_versions_no_conflict() {
    let repo = empty_git_repo();

    // Create a file without conflict
    File::create(repo.path().join("file.txt"))
        .unwrap()
        .write_all(b"no conflict")
        .unwrap();
    exec_git_cmd("add file.txt", repo.path());
    exec_git_cmd("commit -m initial", repo.path());

    // Test: get_merge_versions should fail (no conflict)
    let file_path = repo.path().join("file.txt");
    let result = git::get_merge_versions(&file_path);

    assert!(result.is_err());
}

#[test]
fn get_merge_versions_multiple_conflicts() {
    let repo = empty_git_repo();

    // Main branch: create two files
    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"main file1")
        .unwrap();
    File::create(repo.path().join("file2.txt"))
        .unwrap()
        .write_all(b"main file2")
        .unwrap();
    exec_git_cmd("add .", repo.path());
    exec_git_cmd("commit -m main", repo.path());

    // Feature branch: modify both files
    exec_git_cmd("checkout -b feature", repo.path());
    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"feature file1")
        .unwrap();
    File::create(repo.path().join("file2.txt"))
        .unwrap()
        .write_all(b"feature file2")
        .unwrap();
    exec_git_cmd("add .", repo.path());
    exec_git_cmd("commit -m feature", repo.path());

    // Back to main, make conflicting changes to both
    exec_git_cmd("checkout main", repo.path());
    File::create(repo.path().join("file1.txt"))
        .unwrap()
        .write_all(b"main file1 v2")
        .unwrap();
    File::create(repo.path().join("file2.txt"))
        .unwrap()
        .write_all(b"main file2 v2")
        .unwrap();
    exec_git_cmd("add .", repo.path());
    exec_git_cmd("commit -m main2", repo.path());

    // Attempt merge - creates conflicts
    let _ = Command::new("git")
        .args(["merge", "feature"])
        .current_dir(repo.path())
        .output();

    // Test file1
    let file1_path = repo.path().join("file1.txt");
    let result1 = git::get_merge_versions(&file1_path);
    assert!(result1.is_ok());
    let (ours1, theirs1) = result1.unwrap();
    assert_eq!(String::from_utf8(ours1).unwrap(), "main file1 v2");
    assert_eq!(String::from_utf8(theirs1).unwrap(), "feature file1");

    // Test file2
    let file2_path = repo.path().join("file2.txt");
    let result2 = git::get_merge_versions(&file2_path);
    assert!(result2.is_ok());
    let (ours2, theirs2) = result2.unwrap();
    assert_eq!(String::from_utf8(ours2).unwrap(), "main file2 v2");
    assert_eq!(String::from_utf8(theirs2).unwrap(), "feature file2");
}
