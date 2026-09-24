# Review comments

Leave a comment on any line of code and an AI agent answers it right there, in a
box under that line. You keep editing as normal while it works. The
conversation stays attached to the line as the code moves, and it is still there
the next time you open Helix on the same branch.

```
   41 │ fn resolve_commit(repo, rev) {
      ▏ agent 3/5 ──────────────────────────── 1-4 of 12
      ▏ It falls back to HEAD when the rev is empty,
      ▏ which is why the picker shows nothing on a
      ▏ clean tree.
   42 │     let commit = ...
```

Comments work in any buffer. They are most useful in a [diff view](./diff-and-merge.md),
where you are already reading changes line by line, but you do not need one.

> **The agent can change your files.** It runs with full access to the
> worktree: it can edit files and run commands without asking, because a
> comment on a line usually asks for a change to it. A process started by Helix
> has no terminal to ask you in, so there is no approval step. Its edits show
> up in the diff you are reviewing. Treat it like any agent with write access to
> your checkout.

<!-- toc -->

## How it works

### Three things to keep apart

Everything else follows from three ideas:

| Idea | What it is | How many |
| --- | --- | --- |
| **Thread** | One comment and the replies to it, anchored to one line of one file. Shown as one box. | One per commented line |
| **Review conversation** | Every thread for one branch in one worktree, saved together in one file. | One per branch per worktree |
| **Agent conversation** | The agent's own memory of one thread. Each thread gets its own, so the agent answering one comment never sees another. | One per thread |

So a branch with five comments has one review conversation that holds five
threads, and each of those threads has its own agent conversation.

### Where things are stored

All of it lives in one directory, `~/.local/state/helix/review/` on Linux (Helix's
state directory, plus `review`):

| File | Holds | Written when |
| --- | --- | --- |
| `<uuid>.threads.json` | Every thread of one review conversation: file, line, side of the diff, messages, any unsent draft | Shortly after anything changes, at most every 0.5 seconds |
| `<uuid>.json` | A lock: which running Helix owns this review conversation | On your first comment, removed when Helix exits |
| `<uuid>.started` | A marker that one thread's agent conversation exists, so the next turn continues it rather than starting afresh | After that thread's first reply |

The agent's full transcript is not kept by Helix. It lives wherever the agent
keeps its own sessions. Files nothing has touched for a year are deleted
automatically.

### How Helix finds the right conversation

The file name of a review conversation is not looked up anywhere. It is
**computed** from where you are:

```
uuid = UUIDv5( "helix-review" : <worktree path> : <branch name> )
```

The same branch in the same worktree always gives the same UUID, so reopening
Helix finds the same file with nothing to configure. Another branch or another
worktree gives another UUID, and therefore a separate conversation.

When a file opens, Helix works out the UUID for its worktree and branch, loads
that file and shows the threads. This is only looking: nothing is saved and
nothing is locked until you leave a comment.

### From comment to reply

```
 you press c            Helix                           agent process
 ───────────            ─────                           ─────────────
 type the comment ──▶   save it as a draft
 send it          ──▶   add file, line, 6 lines of
                        context either side, diff range
                        start one process for this turn ──▶  reads the prompt
                                                             edits, runs commands
 watch it stream  ◀──   draw each piece in the box  ◀────── streams the reply
                        save the thread to disk      ◀────── exits
```

- **One turn is one process.** Every send starts the agent, gives it the
  prompt, and lets it exit when the reply is done. Nothing stays running
  between turns.
- **The first turn creates the thread's agent conversation** (`--session-id
  <uuid>`), and every later turn continues it (`--resume <uuid>`). A follow-up
  only carries the new text, because the agent already holds the context.
- **Several threads can be waiting at once.** Each has its own process, and a
  reply can only land on the thread that asked for it.
- **One thread waits for one reply at a time.** A follow-up sent while its reply
  is still arriving is held back and goes out when that reply lands.
- **Helix is never blocked.** You can edit, switch buffers or leave more
  comments while replies arrive.

### When the code moves

While a file is open, each thread is pinned to its line and follows it through
every edit, yours or the agent's. When the file is closed, the last known line
is written back, so the thread reopens in the right place. If the line itself is
deleted, the thread is kept and drawn dimmed where the line used to be, rather
than thrown away.

### When you switch branch

Each branch has its own review conversation, so after a checkout Helix moves to
the new branch's one. It saves the old one first. Helix does not watch the
repository, so it checks for a new branch at three moments:

- the terminal window gets focus again, which is usually right after you ran
  `git checkout` in another window;
- a file is opened;
- a buffer is reloaded with `:reload` or `:reload-all`.

A reply still arriving at that moment is marked as stopped, with a note saying
which branch was checked out.

It stays put in three cases:

- **HEAD is detached**, for example during a rebase, so you keep your threads
  throughout.
- **You are typing a comment.** It moves once you have finished.
- **You named the conversation yourself** with `:review-session <name>`. A
  conversation named after anything but a branch belongs to no branch, so it
  never moves.

### Two Helix windows on one branch

The first one to comment takes the lock on that branch's review conversation.
The second sees the lock is held by a running Helix and uses `<branch>#2`
instead, so the two never write into each other's file. A lock left behind by a
Helix that crashed is ignored.

## Features

- **Comments on any line of any buffer**, on either side of a split diff. A
  comment on the old side is about the old text.
- **Conversations, not one-off questions.** Reply to a thread as often as you
  like; the agent remembers the whole thread.
- **Replies stream in** as they are written, with a spinner in the box header
  while the agent works.
- **Replies are rendered as markdown**: headings, lists, tables and code blocks
  are laid out rather than shown as source.
- **One entry on screen at a time.** The header shows which (`3/5`), and you step
  through the history. A box never takes more than half the window, so the code
  stays visible; a long reply shows which rows are on screen (`12-40 of 201`)
  and scrolls inside the box.
- **Drafts.** Write several comments, then send them all at once. Unsent drafts
  are saved and survive a restart. The status line can show how many are waiting.
- **Rewind.** Step back to an older entry and reply from there to take the
  conversation another way. The entries after it are dropped, and the agent is
  told they no longer stand.
- **Copy from a reply.** Drag across it with the mouse, or copy the whole entry.
- **Open a path from a reply.** `gf` or Ctrl+click on a `path:line` or a link in
  a reply opens that file at that line.
- **Continue in a terminal.** The thread's conversation id is at the right of its
  header; click it to copy it, then run `claude --resume <uuid>` in the worktree.
  Do not reply from Helix while that session is open elsewhere.
- **Choice of agent.** Claude Code (`claude`) by default, or Grok with
  `:review-session grok`.
- **Hide everything** with one key when you want to read the code alone.

## Shortcuts

### Leaving and sending comments

Most keys only mean something on a line that carries a thread. Anywhere else
they do what they always do.

| Key | What it does |
| --- | --- |
| `Space m R c` | Comment on this line, reply to its thread, or reopen its unsent draft |
| `c` | Same as above, but only on a line that has a thread; elsewhere it is the normal change |
| `Space m R S` | Send every unsent draft |
| `Ctrl-s` / `Ctrl-Shift-s` | Send the draft on this line; with no draft there, save the selection to the jumplist as usual |
| `Space m R t` | Collapse or expand the thread on this line |
| `Space m R h` | Hide or show every box |

### In the comment box

| Key | What it does |
| --- | --- |
| `Enter` | New line |
| `Ctrl-s` | Save as a draft and close |
| `Ctrl-Shift-s` | Save and send now |
| `Esc` | Close, leaving any saved draft as it was |

### On a box

Moving down onto a commented line stops on its box first: the cursor stays put
and the box is drawn as focused. The next press moves on. A count such as `10j`
goes straight through. These keys work once a box is focused:

| Key | What it does |
| --- | --- |
| `Ctrl-Left` / `Ctrl-Right` | Previous / next entry in the thread |
| `Ctrl-Up` / `Ctrl-Down` | Move up / down inside a long reply |
| `c` | Reply from the entry on screen |
| `d` | Delete the entry on screen; deleting the last one removes the thread |
| `y` | Copy the selected rows, or the whole entry, to the system clipboard |
| `gf` | Open the path under the box's cursor |
| Drag with the mouse | Select rows in a reply |
| Ctrl+click | Open the path or link clicked on |
| Click the id in the header | Copy the thread's conversation id |

### Moving between comments

| Key | What it does |
| --- | --- |
| `]c` / `[c` | Next / previous comment in this diff; outside a diff, the usual code-comment motion |
| `]C` / `[C` | Next / previous comment anywhere in the conversation, opening its file if needed |

### Commands

| Command | What it does |
| --- | --- |
| `:review-session` | Show the current conversation and agent |
| `:review-session <name>` | Name, rename or switch the conversation. A name other than the branch stays put across checkouts |
| `:review-session grok` / `claude` | Choose the agent for this Helix, without renaming the conversation |
| `:review-session <name> grok` | Both at once, in either order |

`:q` refuses while a reply is still arriving, as it does with unsaved buffers.
`:q!` stops the agent, along with any command it is running, and keeps what had
arrived, marked as stopped.

## A typical review

1. **Open the changes.** Press `Space g` and pick a file to open its diff.
2. **Walk through it.** Use `]g` / `[g` to go from hunk to hunk.
3. **Comment where something needs attention.** Press `Space m R c` on the line
   (or just `c` on a line that already has a thread)
   and write what you want, short and to the point: "why does this fall back to
   HEAD?" or "rename this to match the caller". Helix adds the file, the line and
   the code around it, so you do not need to.
4. **Save it as a draft or send it now.** `Ctrl-s` saves it; `Ctrl-Shift-s`
   sends it straight away. Saving several drafts and sending them together with
   `Space m R S` suits a first pass through a large change.
5. **Keep reviewing while the agent works.** A spinner in the box header shows
   the reply is on its way.
6. **Read the reply.** Move down onto the box, and use `Ctrl-Up` / `Ctrl-Down` if
   it is long.
7. **Answer it.** `c` replies in the same thread. If a reply went the wrong way,
   step back with `Ctrl-Left` and reply from the earlier entry.
8. **Check what it changed.** If the agent edited files, the edits are in the
   diff in front of you. Press `Space g` again to see every file it touched.
9. **Tidy up.** Delete entries with `d` once they are dealt with, or hide all
   boxes with `Space m R h` to read the result without them.
10. **Come back later.** Close Helix whenever you like. Opening any file on the
    same branch brings the threads back where you left them.

## Status line and theme

Add `review-session` to a status line section in [the editor
config](./editor.md) to show the conversation's name once you have commented,
with `+n` when drafts are waiting to be sent. A `▐` in the gutter marks every
commented line.

All theme keys are optional. Without them the boxes use shades derived from the
editor background.

| Key | Used for |
| --- | --- |
| `ui.review.comment.user` | Your messages |
| `ui.review.comment.agent` | The agent's replies |
| `ui.review.comment.pending` | Unsent drafts |
| `ui.review.comment.collapsed` | A collapsed thread |
| `ui.review.comment.orphaned` | A thread whose line was deleted |
| `ui.review.comment.cursor` | The box on the cursor's line |
| `ui.review.comment.focused` | The box that has the keys |
| `ui.review.comment.line` | The cursor row inside a box |
| `ui.review.comment.selection` | Selected rows inside a box |
| `ui.review.input` | The comment box being typed into |
| `ui.review.gutter` | The `▐` gutter marker |
