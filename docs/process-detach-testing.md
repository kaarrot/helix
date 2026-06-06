# Testing language-server / debug-adapter terminal detachment

Helix spawns language servers (and stdio debug adapters) **detached** from its
controlling terminal so a misbehaving server cannot corrupt the TUI, while still
guaranteeing the server is cleaned up when helix exits. This is implemented in
`helix-stdx/src/process.rs` and wired into `helix-lsp/src/client.rs` and
`helix-dap/src/client.rs`.

The behavior can be turned off per server / adapter:

```toml
# languages.toml — opt a misbehaving server out of detachment
[language-server.some-server]
command = "some-server"
detached = false   # default: true

[[language.debugger]]              # debug adapters use the same flag
name = "some-adapter"
detached = false   # default: true (stdio transport only; tcp is always detached)
```

What each platform guarantees:

| Platform            | Terminal isolation | Killed when helix exits (incl. SIGKILL/crash) |
|---------------------|:------------------:|:---------------------------------------------:|
| Linux / Android     | ✅ `setsid`        | ✅ `PR_SET_PDEATHSIG`                          |
| FreeBSD             | ✅ `setsid`        | ✅ `PROC_PDEATHSIG_CTL`                        |
| Windows             | ✅ detached console| ✅ Job Object `KILL_ON_JOB_CLOSE`             |
| macOS / other BSDs  | ✅ `setsid`        | ⚠️ best-effort (`kill_on_drop`, clean exit only) |

The Linux/Android path is verified. The sections below describe how to verify the
**Windows** and **macOS** paths on real hardware/VMs.

---

## Common setup

1. Build helix: `cargo build` (or `cargo install --path helix-term --locked`).
2. Configure a language server that is easy to spawn, e.g. `rust-analyzer` for Rust
   or `pylsp`/`pyright` for Python, in your `languages.toml`.
3. Open a matching file in helix so the server starts: `hx src/main.rs`.
4. In another terminal, find the helix PID and the server PID (the server is a child
   of helix, or — once detached — reparented to init).

---

## Windows

The guarantee on Windows comes from a **Job Object** created with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: helix never closes the job handle, so when the
helix process dies (cleanly, on crash, or via `taskkill /F`) the kernel terminates
every server assigned to the job. Detachment also uses
`DETACHED_PROCESS | CREATE_NO_WINDOW` so the server never attaches to helix's console.

Use **PowerShell** (and optionally Sysinternals **Process Explorer**).

### 1. Console isolation

- Start helix from a console and open a file to spawn the server.
- Confirm **no extra console window** flashes/opens for the server.
- In Process Explorer, the server process should show **no console** and (lower pane /
  Properties → Job tab) membership in a job object owned by helix.

### 2. Cleanup on forced kill (the key test)

```powershell
# find the PIDs
Get-Process hx, rust-analyzer | Select-Object Name, Id

# force-kill helix (simulates a crash; cleanup must not depend on graceful exit)
taskkill /F /PID <helix_pid>

# the server must be gone almost immediately
Get-Process rust-analyzer -ErrorAction SilentlyContinue   # expect: nothing
```

Also test by simply **closing the terminal window** while helix runs — the server must
still terminate.

### 3. Opt-out behaves like the old code

Set `detached = false` for the server, restart helix, spawn the server, then
`taskkill /F` helix. With detachment off the server is **not** in the kill-on-close job,
so it may linger (orphan) — this confirms the flag is actually toggling the job
assignment.

> Note: this code path is not compiled/tested on the Linux CI dev box; a real Windows
> build is required to validate it.

---

## macOS (and other non-FreeBSD BSDs)

macOS has no `PR_SET_PDEATHSIG` equivalent, so only **terminal isolation** is guaranteed
(`setsid`). Cleanup relies on tokio's `kill_on_drop`, which only runs on a *graceful*
helix exit; a `SIGKILL`/crash of helix can leave an orphan. Both points below should be
verified.

### 1. Terminal isolation (the guarantee)

Start helix, open a file to spawn the server, then inspect sessions/ttys:

```sh
# helix's own session id and controlling tty
ps -o pid,sid,tty,comm -p $(pgrep -x hx)

# the language server: SID should EQUAL its own PID (it is a session leader)
# and TTY should be "??" (no controlling terminal)
ps -o pid,ppid,sid,tty,comm -p $(pgrep -x rust-analyzer)
```

Expected: the server's `SID == its PID` and `TTY == ??`, and its `SID` differs from
helix's `SID`. That proves it cannot touch helix's terminal.

### 2. Cleanup behavior

- **Graceful quit** (`:q` / `:quit` in helix): the server should disappear
  (`kill_on_drop`).
  ```sh
  pgrep -x rust-analyzer   # expect: nothing after quitting helix
  ```
- **Forced kill** (documented limitation):
  ```sh
  kill -9 $(pgrep -x hx)
  pgrep -x rust-analyzer   # may still be ALIVE — known macOS limitation
  ```
  If you see an orphan here, that is expected on macOS today. A future fix would add a
  small `kqueue`/`NOTE_EXIT` supervisor to close this gap.

### 3. Opt-out

Set `detached = false`, restart, spawn the server, and re-run the `ps` check from step 1:
the server should now share **helix's** SID and controlling tty (`TTY` no longer `??`),
confirming the flag disables `setsid`.

---

## Debug adapters

The same flag and mechanism apply to **stdio** debug adapters (e.g. `debugpy`,
`lldb-dap`). Start a debug session, find the adapter PID, and repeat the platform steps
above. `tcp` transport adapters are intentionally always spawned detached and are not
affected by the flag.
