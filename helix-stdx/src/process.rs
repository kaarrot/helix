//! Helpers for spawning child processes (language servers, debug adapters) so that
//! they are isolated from helix's controlling terminal and are cleaned up when helix
//! exits.
//!
//! These exist because a spawned server shares helix's session, and therefore its
//! controlling terminal: a misbehaving server (or a child it spawns) can open
//! `/dev/tty`, issue ioctls or change termios and corrupt helix's TUI. Putting the
//! server in its own session fixes that, but a bare `setsid()` would leave the server
//! running after helix is killed. The platform-specific pieces below isolate the
//! terminal *and* bind the server's lifetime to helix.

#[cfg(unix)]
mod unix {
    use std::io;

    /// To be run inside the child, between `fork` and `exec` (via
    /// [`std::os::unix::process::CommandExt::pre_exec`] /
    /// `tokio::process::Command::pre_exec`).
    ///
    /// - `setsid()` puts the child in a new session, so it has no controlling
    ///   terminal and cannot manipulate helix's tty.
    /// - On Linux/FreeBSD the kernel is asked to send `SIGTERM` to the child when
    ///   helix dies — even on an uncatchable `SIGKILL` or a crash — so no orphaned
    ///   server is left behind. A `getppid()` recheck closes the race where helix
    ///   exited before the death signal was armed.
    ///
    /// `parent_pid` must be helix's pid (`std::process::id()`), captured in the
    /// parent *before* spawning.
    ///
    /// # Safety
    ///
    /// Runs in the fragile post-`fork`/pre-`exec` context, so it only calls
    /// async-signal-safe syscalls and allocates nothing.
    pub fn detach_from_controlling_terminal(parent_pid: u32) -> io::Result<()> {
        rustix::process::setsid()?;

        // PR_SET_PDEATHSIG (Linux) / PROC_PDEATHSIG_CTL (FreeBSD). Other unices
        // (notably macOS) have no equivalent; there the child is reaped via
        // tokio's `kill_on_drop` on a clean exit only.
        #[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
        {
            use rustix::process::{getppid, set_parent_process_death_signal, Signal};

            // Note: the death signal is delivered when the *thread* that forked us
            // exits. helix's tokio worker threads live for the whole process, so in
            // practice this fires on process death.
            set_parent_process_death_signal(Some(Signal::TERM))?;

            // If helix already exited, the death signal will never fire, so abort
            // rather than leak an orphan.
            let reparented = getppid().map_or(true, |ppid| {
                ppid.as_raw_nonzero().get() as u32 != parent_pid
            });
            if reparented {
                return Err(io::Error::other("parent process already exited"));
            }
        }
        let _ = parent_pid;

        Ok(())
    }
}

#[cfg(unix)]
pub use unix::detach_from_controlling_terminal;

#[cfg(windows)]
mod windows {
    use std::io;
    use std::os::windows::io::RawHandle;
    use std::ptr;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// Creation flags to pass to `tokio::process::Command::creation_flags` so the
    /// child does not attach to (and cannot manipulate) helix's console.
    ///
    /// `DETACHED_PROCESS` (0x8) | `CREATE_NO_WINDOW` (0x0800_0000).
    pub const DETACHED_CREATION_FLAGS: u32 = 0x0000_0008 | 0x0800_0000;

    struct Job(HANDLE);
    // The handle is only ever used to assign processes to the job and is never
    // mutated after creation.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    /// A single job object shared for helix's lifetime. It is configured with
    /// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and deliberately never closed: when
    /// helix exits (cleanly, on crash, or via taskkill) the kernel closes the
    /// handle and terminates every process assigned to the job.
    static JOB: OnceLock<Option<Job>> = OnceLock::new();

    fn job() -> Option<HANDLE> {
        JOB.get_or_init(|| unsafe {
            let handle = CreateJobObjectW(ptr::null(), ptr::null());
            if handle.is_null() {
                return None;
            }

            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                ptr::addr_of!(info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return None;
            }

            Some(Job(handle))
        })
        .as_ref()
        .map(|job| job.0)
    }

    /// Assign a freshly spawned child to helix's job object so it is terminated when
    /// helix exits. `child` is the child's raw process handle
    /// (`tokio::process::Child::raw_handle`).
    pub fn assign_to_job(child: RawHandle) -> io::Result<()> {
        if let Some(job) = job() {
            let ok = unsafe { AssignProcessToJobObject(job, child as HANDLE) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use windows::{assign_to_job, DETACHED_CREATION_FLAGS};
