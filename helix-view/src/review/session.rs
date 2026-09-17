//! Identity and ownership of a review conversation.
//!
//! A conversation is identified by a UUID derived from the worktree and a name,
//! so reopening the editor on the same branch continues the same conversation
//! with no bookkeeping, while a different branch or worktree gets its own.
//!
//! The UUID is also claimed on disk, because two editors deriving the same one
//! would otherwise drive a single conversation and interleave their turns into
//! it. Claude Code does not refuse concurrent access to a session, so nothing
//! else will catch that.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

/// Namespace for derived session UUIDs. Arbitrary but fixed: changing it
/// renames every existing conversation.
const NAMESPACE: &str = "helix-review";

/// How many `#n` suffixes to try before giving up on finding a free name.
const MAX_SUFFIX: usize = 64;

/// Conversations untouched for this long are deleted. Nothing prunes them
/// otherwise, and one accumulates per branch per worktree, forever.
const KEEP_FOR: Duration = Duration::from_secs(365 * 24 * 60 * 60);

/// A claimed review conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSession {
    /// Captured when the session starts and never recomputed: deriving it from
    /// HEAD on the fly would silently swap conversations on a branch switch.
    pub name: String,
    pub uuid: String,
    pub worktree: PathBuf,
}

/// What is written beside a claimed UUID.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ownership {
    pub pid: u32,
    /// Process start time, where the platform can report it. Guards against PID
    /// reuse: without it, a crash could leave a UUID claimed forever by whatever
    /// unrelated process later inherits the number.
    #[serde(default)]
    pub proc_start: Option<String>,
    pub started_at: u64,
    pub name: String,
    pub worktree: String,
}

/// UUID version 5: SHA-1 over namespace and name, with the version and variant
/// bits forced.
///
/// Computed here rather than via `uuid`'s `v5` feature, which would pull in
/// another hash implementation for fifteen lines of well-specified bit-twiddling
/// when `sha1` is already in the tree.
pub fn derive_uuid(worktree: &Path, name: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(NAMESPACE.as_bytes());
    hasher.update(b":");
    hasher.update(worktree.to_string_lossy().as_bytes());
    hasher.update(b":");
    hasher.update(name.as_bytes());
    let digest = hasher.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50; // version 5
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // RFC 4122 variant

    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Where claims and persisted conversations live.
pub fn review_dir() -> PathBuf {
    helix_loader::state_dir().join("review")
}

fn ownership_path(dir: &Path, uuid: &str) -> PathBuf {
    dir.join(format!("{uuid}.json"))
}

/// This process's start time, so a reused PID cannot impersonate it.
///
/// Linux only; elsewhere `None`, and ownership falls back to a plain PID check.
fn own_proc_start() -> Option<String> {
    proc_start(std::process::id())
}

#[cfg(target_os = "linux")]
fn proc_start(pid: u32) -> Option<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The comm field is parenthesised and may itself contain spaces and
    // parentheses, so fields are counted from after the final ')'.
    let rest = stat.rsplit_once(')')?.1;
    rest.split_whitespace().nth(19).map(str::to_string)
}

#[cfg(not(target_os = "linux"))]
fn proc_start(_pid: u32) -> Option<String> {
    None
}

/// Whether the recorded owner is still running.
///
/// Unknown counts as alive. A wrong "dead" lets two editors drive one
/// conversation, which is the failure this whole mechanism exists to prevent; a
/// wrong "alive" only costs a suffixed name.
fn owner_is_live(own: &Ownership) -> bool {
    #[cfg(unix)]
    {
        // Signal 0 performs the permission and existence checks without sending
        // anything. EPERM means it exists but belongs to someone else.
        let alive = unsafe { libc::kill(own.pid as libc::pid_t, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
        if !alive {
            return false;
        }
        match (&own.proc_start, proc_start(own.pid)) {
            // Same PID, different start time: the original died and the number
            // was recycled.
            (Some(recorded), Some(current)) => recorded == &current,
            _ => true,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = own;
        true
    }
}

fn read_ownership(dir: &Path, uuid: &str) -> Option<Ownership> {
    let raw = fs::read_to_string(ownership_path(dir, uuid)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Write a claim unconditionally. Only tests plant claims this way; taking one
/// for real goes through [`try_claim`], which must be exclusive.
#[cfg(test)]
fn write_ownership(dir: &Path, uuid: &str, own: &Ownership) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let raw = serde_json::to_string_pretty(own).map_err(std::io::Error::other)?;
    fs::write(ownership_path(dir, uuid), raw)
}

/// What happened when a name was claimed.
enum Claimed {
    /// This process now holds it.
    Ours,
    /// Somebody live holds it; try the next name.
    Taken,
    /// The claim could not be created at all -- a read-only state directory,
    /// say. The conversation still works, it is just unprotected, which beats
    /// refusing to comment.
    Unprotected,
}

/// How many times to go round when a stale claim is being cleared underneath us.
const CLAIM_ATTEMPTS: usize = 4;

/// Take a claim, or report who has it.
///
/// Creation is exclusive (`create_new`), which is the whole point: a read
/// followed by a write lets two editors both see no live owner and both write,
/// and they would then interleave turns into one agent session silently --
/// exactly what the claim exists to prevent.
fn try_claim(dir: &Path, uuid: &str, own: &Ownership) -> Claimed {
    let path = ownership_path(dir, uuid);
    if fs::create_dir_all(dir).is_err() {
        return Claimed::Unprotected;
    }
    let Ok(raw) = serde_json::to_string_pretty(own) else {
        return Claimed::Unprotected;
    };

    for _ in 0..CLAIM_ATTEMPTS {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                return match file.write_all(raw.as_bytes()) {
                    Ok(()) => Claimed::Ours,
                    // We hold an empty file nobody can read as a claim, which
                    // would leave the name unusable. Give it back.
                    Err(_) => {
                        let _ = fs::remove_file(&path);
                        Claimed::Unprotected
                    }
                };
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                match read_ownership(dir, uuid) {
                    // Already ours: re-entering our own session, not a rival.
                    Some(existing) if existing.pid == own.pid => return Claimed::Ours,
                    Some(existing) if owner_is_live(&existing) => return Claimed::Taken,
                    // Stale, or unparseable and therefore useless. Clear it and
                    // race for it properly. Whoever loses that race sees a live
                    // owner on the next turn and moves on to the next name.
                    _ => {
                        if fs::remove_file(&path).is_err() {
                            return Claimed::Taken;
                        }
                    }
                }
            }
            Err(_) => return Claimed::Unprotected,
        }
    }

    // Something else keeps clearing it. Carry on unprotected rather than spin.
    Claimed::Unprotected
}

/// Delete conversations nothing has touched for a year.
///
/// Files are grouped by uuid and a group is only removed whole. Deleting them
/// piecemeal could drop the marker that records the agent session was already
/// created while keeping the threads, and the next spawn would then try to
/// create a session that already exists.
///
/// A group whose claim is still held by a live process is left alone regardless
/// of age, so a long-running editor cannot have its own conversation deleted.
pub fn prune_in(dir: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    let mut groups: HashMap<String, (Vec<PathBuf>, SystemTime)> = HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // `<uuid>.json`, `<uuid>.started`, `<uuid>.threads.json`, `<uuid>.threads.json.tmp`
        let Some((uuid, _)) = name.split_once('.') else {
            continue;
        };
        let touched = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(now);

        let group = groups
            .entry(uuid.to_string())
            .or_insert_with(|| (Vec::new(), SystemTime::UNIX_EPOCH));
        group.0.push(path);
        group.1 = group.1.max(touched);
    }

    for (uuid, (paths, touched)) in groups {
        let stale = now.duration_since(touched).is_ok_and(|age| age > KEEP_FOR);
        if !stale {
            continue;
        }
        if read_ownership(dir, &uuid).is_some_and(|own| owner_is_live(&own)) {
            continue;
        }
        for path in paths {
            let _ = fs::remove_file(path);
        }
    }
}

/// Claim a conversation for `base_name` under `worktree`.
///
/// Takes the name as given when nothing live holds it, including when the
/// holder is this process's own earlier crash. Otherwise walks `name#2`,
/// `name#3` … so a second editor gets its own conversation rather than silently
/// sharing one. Never blocks.
pub fn claim(worktree: &Path, base_name: &str) -> ReviewSession {
    let dir = review_dir();
    // Claiming is the once-per-session moment, so it is where the sweep belongs:
    // a directory scan is too expensive to repeat on every save.
    prune_in(&dir, SystemTime::now());
    claim_in(&dir, worktree, base_name)
}

/// `claim`, against an explicit directory, so it can be exercised without
/// touching the real state directory.
pub fn claim_in(dir: &Path, worktree: &Path, base_name: &str) -> ReviewSession {
    for suffix in 0..MAX_SUFFIX {
        let name = if suffix == 0 {
            base_name.to_string()
        } else {
            format!("{base_name}#{}", suffix + 1)
        };
        let uuid = derive_uuid(worktree, &name);

        let own = Ownership {
            pid: std::process::id(),
            proc_start: own_proc_start(),
            started_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            name: name.clone(),
            worktree: worktree.to_string_lossy().into_owned(),
        };

        match try_claim(dir, &uuid, &own) {
            Claimed::Taken => continue,
            Claimed::Ours | Claimed::Unprotected => {
                return ReviewSession {
                    name,
                    uuid,
                    worktree: worktree.to_path_buf(),
                }
            }
        }
    }

    // Absurdly many live holders. Fall back to the plain name rather than
    // leaving the user unable to comment at all.
    ReviewSession {
        name: base_name.to_string(),
        uuid: derive_uuid(worktree, base_name),
        worktree: worktree.to_path_buf(),
    }
}

/// Drop a claim, so the next editor takes the name rather than a suffix.
pub fn release(session: &ReviewSession) {
    release_in(&review_dir(), session);
}

pub fn release_in(dir: &Path, session: &ReviewSession) {
    if read_ownership(dir, &session.uuid).is_some_and(|own| own.pid == std::process::id()) {
        let _ = fs::remove_file(ownership_path(dir, &session.uuid));
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn uuid_is_deterministic_and_well_formed() {
        let a = derive_uuid(Path::new("/repo"), "main");
        let b = derive_uuid(Path::new("/repo"), "main");
        assert_eq!(a, b, "same worktree and name must derive the same uuid");
        assert_eq!(a.len(), 36);
        assert_eq!(
            a.split('-').map(str::len).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(a.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        // Version 5 and the RFC 4122 variant, which `--session-id` requires.
        assert_eq!(a.as_bytes()[14], b'5');
        assert!(matches!(a.as_bytes()[19], b'8' | b'9' | b'a' | b'b'));
    }

    #[test]
    fn uuid_separates_branches_and_worktrees() {
        let main = derive_uuid(Path::new("/repo"), "main");
        let feature = derive_uuid(Path::new("/repo"), "feature");
        let other_tree = derive_uuid(Path::new("/other"), "main");
        assert_ne!(main, feature);
        assert_ne!(main, other_tree);
    }

    #[test]
    fn a_dead_owner_does_not_hold_a_name() {
        // PID 1 is alive but will not match a bogus start time, which is the
        // PID-reuse case the start time exists to catch.
        let recycled = Ownership {
            pid: 1,
            proc_start: Some("definitely-not-the-real-start-time".into()),
            started_at: 0,
            name: "main".into(),
            worktree: "/repo".into(),
        };
        if cfg!(target_os = "linux") {
            assert!(!owner_is_live(&recycled));
        }

        // A PID that cannot exist.
        let gone = Ownership {
            pid: u32::MAX - 1,
            proc_start: None,
            started_at: 0,
            name: "main".into(),
            worktree: "/repo".into(),
        };
        if cfg!(unix) {
            assert!(!owner_is_live(&gone));
        }
    }

    #[test]
    fn a_live_foreign_owner_forces_a_suffixed_name() {
        let dir = tempfile::tempdir().unwrap();
        let worktree = Path::new("/repo");

        // PID 1 always exists and is never us, so it stands in for another
        // editor still holding the name.
        let uuid = derive_uuid(worktree, "main");
        write_ownership(
            dir.path(),
            &uuid,
            &Ownership {
                pid: 1,
                proc_start: proc_start(1),
                started_at: 0,
                name: "main".into(),
                worktree: "/repo".into(),
            },
        )
        .unwrap();

        let session = claim_in(dir.path(), worktree, "main");
        assert_eq!(
            session.name, "main#2",
            "a live holder must not be silently shared"
        );
        assert_eq!(session.uuid, derive_uuid(worktree, "main#2"));
    }

    #[test]
    fn a_dead_owners_name_is_taken_over() {
        let dir = tempfile::tempdir().unwrap();
        let worktree = Path::new("/repo");
        let uuid = derive_uuid(worktree, "main");
        write_ownership(
            dir.path(),
            &uuid,
            &Ownership {
                pid: u32::MAX - 1,
                proc_start: None,
                started_at: 0,
                name: "main".into(),
                worktree: "/repo".into(),
            },
        )
        .unwrap();

        let session = claim_in(dir.path(), worktree, "main");
        assert_eq!(
            session.name, "main",
            "a crashed editor must not hold a name forever"
        );
    }

    #[test]
    fn releasing_frees_the_name_for_the_next_editor() {
        let dir = tempfile::tempdir().unwrap();
        let worktree = Path::new("/repo");

        let first = claim_in(dir.path(), worktree, "main");
        assert_eq!(first.name, "main");
        release_in(dir.path(), &first);

        let second = claim_in(dir.path(), worktree, "main");
        assert_eq!(second.name, "main");
        assert_eq!(
            second.uuid, first.uuid,
            "same name must resume the same conversation"
        );
    }

    /// Backdate a file so it looks untouched for `days`.
    fn age(path: &Path, days: u64) {
        let when = SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60);
        let stamp = filetime::FileTime::from_system_time(when);
        filetime::set_file_mtime(path, stamp).unwrap();
    }

    #[test]
    fn an_old_conversation_is_swept_away_whole() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = derive_uuid(Path::new("/repo"), "ancient");
        for suffix in ["json", "started", "threads.json"] {
            let path = dir.path().join(format!("{uuid}.{suffix}"));
            fs::write(&path, "{}").unwrap();
            age(&path, 400);
        }

        prune_in(dir.path(), SystemTime::now());

        for suffix in ["json", "started", "threads.json"] {
            assert!(
                !dir.path().join(format!("{uuid}.{suffix}")).exists(),
                "a stale group must be removed whole, not piecemeal: {suffix} survived"
            );
        }
    }

    #[test]
    fn a_recent_conversation_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = derive_uuid(Path::new("/repo"), "recent");
        let threads = dir.path().join(format!("{uuid}.threads.json"));
        fs::write(&threads, "{}").unwrap();
        age(&threads, 30);

        prune_in(dir.path(), SystemTime::now());
        assert!(threads.exists());
    }

    #[test]
    fn a_group_is_kept_if_any_of_it_is_recent() {
        // The threads file is rewritten on every change while the marker is
        // written once, so an active conversation has an ancient marker.
        let dir = tempfile::tempdir().unwrap();
        let uuid = derive_uuid(Path::new("/repo"), "long-running");
        let marker = dir.path().join(format!("{uuid}.started"));
        let threads = dir.path().join(format!("{uuid}.threads.json"));
        fs::write(&marker, "x").unwrap();
        fs::write(&threads, "{}").unwrap();
        age(&marker, 400);
        age(&threads, 1);

        prune_in(dir.path(), SystemTime::now());
        assert!(
            marker.exists(),
            "an old marker must not orphan live threads"
        );
        assert!(threads.exists());
    }

    #[test]
    fn a_claim_is_created_exclusively() {
        let dir = tempfile::tempdir().unwrap();
        let worktree = Path::new("/repo");
        let uuid = derive_uuid(worktree, "main");
        let own = Ownership {
            pid: std::process::id(),
            proc_start: own_proc_start(),
            started_at: 0,
            name: "main".into(),
            worktree: "/repo".into(),
        };

        // Nobody holds it: taking it succeeds and leaves a readable claim.
        assert!(matches!(try_claim(dir.path(), &uuid, &own), Claimed::Ours));
        assert_eq!(read_ownership(dir.path(), &uuid).unwrap().name, "main");

        // A live stranger holds it: the name is refused rather than taken by a
        // second write. This is the race the exclusive create exists for -- a
        // read-then-write would see "not me" and overwrite.
        write_ownership(
            dir.path(),
            &uuid,
            &Ownership {
                pid: 1,
                proc_start: proc_start(1),
                started_at: 0,
                name: "main".into(),
                worktree: "/repo".into(),
            },
        )
        .unwrap();
        if cfg!(unix) {
            assert!(matches!(try_claim(dir.path(), &uuid, &own), Claimed::Taken));
            assert_eq!(read_ownership(dir.path(), &uuid).unwrap().pid, 1);
        }

        // A dead owner is cleared out of the way and the name taken.
        write_ownership(
            dir.path(),
            &uuid,
            &Ownership {
                pid: u32::MAX - 1,
                proc_start: None,
                started_at: 0,
                name: "main".into(),
                worktree: "/repo".into(),
            },
        )
        .unwrap();
        if cfg!(unix) {
            assert!(matches!(try_claim(dir.path(), &uuid, &own), Claimed::Ours));
            assert_eq!(
                read_ownership(dir.path(), &uuid).unwrap().pid,
                std::process::id()
            );
        }
    }

    #[test]
    fn a_claim_that_cannot_be_read_is_not_treated_as_live() {
        let dir = tempfile::tempdir().unwrap();
        let worktree = Path::new("/repo");
        let uuid = derive_uuid(worktree, "main");
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(ownership_path(dir.path(), &uuid), "{ not json").unwrap();

        let own = Ownership {
            pid: std::process::id(),
            proc_start: own_proc_start(),
            started_at: 0,
            name: "main".into(),
            worktree: "/repo".into(),
        };
        // Nobody can be shown to hold it, so it is ours rather than a name that
        // is permanently unusable.
        assert!(matches!(try_claim(dir.path(), &uuid, &own), Claimed::Ours));
    }

    #[test]
    fn a_live_claim_is_never_swept_however_old() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = derive_uuid(Path::new("/repo"), "held");
        let own = dir.path().join(format!("{uuid}.json"));
        write_ownership(
            dir.path(),
            &uuid,
            &Ownership {
                pid: std::process::id(),
                proc_start: own_proc_start(),
                started_at: 0,
                name: "held".into(),
                worktree: "/repo".into(),
            },
        )
        .unwrap();
        age(&own, 400);

        prune_in(dir.path(), SystemTime::now());
        assert!(
            own.exists(),
            "an editor must not delete its own conversation"
        );
    }

    #[test]
    fn this_process_is_live() {
        let mine = Ownership {
            pid: std::process::id(),
            proc_start: own_proc_start(),
            started_at: 0,
            name: "main".into(),
            worktree: "/repo".into(),
        };
        assert!(owner_is_live(&mine));
    }
}
