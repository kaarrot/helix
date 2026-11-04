# External benchmark: diff review and 3-way merge UX for a modal terminal editor

Checked 2026-10-30. Evidence tags: **[Doc]** official docs, release notes, or source code. **[Maint]** statement by a project maintainer. **[Research]** peer-reviewed study. **[Anecdotal]** an individual user's opinion (issue comment, HN, blog). Issue and PR states are as read on the check date. The Helix branch was not read for these notes.

## How do leading tools handle diff review?

### Takeaway
The tools have converged on one review loop: a persistent file list, side-by-side with an inline fallback, a single next/previous-hunk motion, one-key stage or revert that keeps your place, aligned scrolling with visible fillers, collapsed unchanged text, and one obvious exit. They split on one question: is a diff a set of synchronised windows (Vim family, JetBrains, Ediff) or a single editable text surface (Magit, Zed, fugitive inline, VS Code multi-diff)? Zed's experience shows the second does not replace the first, because users kept asking for whole-file views.

### Cited Findings

#### File list and moving between files
- **diffview.nvim:** a single tabpage holds a file panel with a list or tree (`i`), `j`/`k`, `<cr>`/`o`/`l` to open, and `<tab>`/`<s-tab>` to cycle files from either the panel or the diff itself. `[F`/`]F` go to first and last file. — [README](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md) [Doc]
- **fugitive:** no panel. The summary buffer is the list, and `:Git difftool` fills the quickfix list with one entry per hunk, or per file with `--name-only`. `:Git difftool -y` opens one tab per file. — [fugitive.txt](https://raw.githubusercontent.com/tpope/vim-fugitive/master/doc/fugitive.txt) [Doc]
- **Neovim core:** 0.12 added `:DiffTool`, which compares directories as well as files. — [news-0.12](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/news-0.12.txt) [Doc]
- **Magit:** the status buffer is a section tree. `n`/`p` move between visible sections, `M-n`/`M-p` between siblings, `^` to the parent. — [magit.org](https://raw.githubusercontent.com/magit/magit/main/docs/magit.org) [Doc]
- **VS Code:** the multi-file diff editor gives all changes in one scrollable view. It previewed in [1.85](https://code.visualstudio.com/updates/v1_85), reached everyone in [1.86](https://code.visualstudio.com/updates/v1_86) and updates live as files change or get staged. Next/previous change across files arrived only in [1.106](https://code.visualstudio.com/updates/v1_106) (Oct 2025). An experimental file list with A/M/D/R status letters appears in [1.143](https://code.visualstudio.com/updates/v1_143). [Doc; read via search snippets]
- **Zed:** the Git Panel offers flat or tree view and watches the repository live. Project Diff shows changes as editable excerpts in a multibuffer. — [Zed git docs](https://zed.dev/docs/git) [Doc]
- **Zed single-file diff:** a "Solo Diff View" shipped later (cmd/ctrl-click, with a configurable primary click action). — [Zed discussion #33773](https://github.com/zed-industries/zed/discussions/33773)
- **JetBrains:** `Alt+Left`/`Alt+Right` move to the previous or next changed file inside the viewer. — [Diff Viewer docs](https://www.jetbrains.com/help/idea/differences-viewer.html) [Doc]
- **lazygit:** a Files panel plus a main panel; `<enter>` focuses the file's diff. — [keybindings](https://raw.githubusercontent.com/jesseduffield/lazygit/master/docs/keybindings/Keybindings_en.md) [Doc]

#### Inline versus side-by-side
- **VS Code:** Side by Side, Inline, and Automatic (default), which switches on editor width with a 900 px default breakpoint (`diffEditor.renderSideBySideInlineBreakpoint`). — [VS Code staging docs](https://code.visualstudio.com/docs/sourcecontrol/staging-commits) [Doc]
- **Zed:** Split is the default, Unified is one setting (`diff_view_style`), and it applies to Project Diff, File History and Stash Diff View. — [Zed git docs](https://zed.dev/docs/git) [Doc]
- **JetBrains:** side-by-side and unified viewers. Text editing is limited to the right pane or lower line. — [Diff Viewer docs](https://www.jetbrains.com/help/idea/differences-viewer.html) [Doc]
- **fugitive:** both forms from one list: `=`/`>`/`<` for an inline diff in the summary, `dd`/`dv`/`ds` for splits. — [fugitive.txt](https://raw.githubusercontent.com/tpope/vim-fugitive/master/doc/fugitive.txt) [Doc]
- **diffview:** `g<C-x>` cycles layouts live, including stacked `diff2_vertical`. — [README](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md) [Doc]
- **lazygit:** `|` cycles configured diff renderers. — [keybindings](https://raw.githubusercontent.com/jesseduffield/lazygit/master/docs/keybindings/Keybindings_en.md) [Doc]
- **delta:** a pager option (`side-by-side = true`). — [delta docs](https://dandavison.github.io/delta/side-by-side-view.html) [Doc]
- **difftastic:** defaults to side-by-side but uses a single column when a change is only additions or only removals. `side-by-side-show-both` forces two columns, and `inline` gives one. — [options.rs](https://raw.githubusercontent.com/Wilfred/difftastic/master/src/options.rs) [Doc]

#### Hunk navigation
- **Vim family:** `]c`/`[c` take a count, and it is an error if no change exists to move to. — [Neovim diff.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/diff.txt) [Doc]
- diffview relies on `]c`/`[c`. Zed's vim mode reuses `] c`/`[ c`. — [README](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md); [Zed vim docs](https://zed.dev/docs/vim) [Doc]
- fugitive's `]c`/`[c` auto-expand inline diffs in the summary buffer. — [fugitive.txt](https://raw.githubusercontent.com/tpope/vim-fugitive/master/doc/fugitive.txt) [Doc]
- **JetBrains:** `F7`/`Shift+F7`. — [Diff Viewer docs](https://www.jetbrains.com/help/idea/differences-viewer.html) [Doc]
- **lazygit:** `h`/`l` between hunks, and `a` toggles line-by-line versus hunk selection. — [keybindings](https://raw.githubusercontent.com/jesseduffield/lazygit/master/docs/keybindings/Keybindings_en.md) [Doc]
- **Ediff:** `n`/`p`, `j` to jump by number, `#h`/`#f` to skip "boring" regions by regexp. — [ediff.texi](https://raw.githubusercontent.com/emacs-mirror/emacs/master/doc/misc/ediff.texi) [Doc]

#### Hunk staging and reverting
- **Vim core:** `do`/`dp` and `:diffget`/`:diffput`. A bare `:diffget` works only when exactly one other buffer is in diff mode, and `do`/`dp` do not work in Visual mode. — [Neovim diff.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/diff.txt) [Doc]
- **diffview:** staging a hunk means writing to an index buffer, which updates the index. The panel adds `s`/`-`, `S`, `U`. `X` restores a file, saves the prior state in the object database and echoes an undo command. — [README](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md) [Doc]
- **fugitive:** in a `:Gdiffsplit`, `do`/`dp` stage and unstage. `s`/`u`/`-`/`U` work on file or hunk from the summary. `X` discards and echoes an undo command. — [fugitive.txt](https://raw.githubusercontent.com/tpope/vim-fugitive/master/doc/fugitive.txt) [Doc]
- **Magit:** `s`/`u` act on the file or hunk at point. With an active region they act on only the marked lines. — [Magit staging](https://docs.magit.vc/magit/Staging-and-Unstaging.html) [Doc]
- **VS Code:** gutter Stage and Revert per change; with a selection they act on that selection. — [VS Code staging docs](https://code.visualstudio.com/docs/sourcecontrol/staging-commits) [Doc]
- **Zed:** `git: stage and next` as the core review verb, with `git: restore` for reverting. — [Zed git docs](https://zed.dev/docs/git) [Doc]
- **Zed's design reasoning:** with separate staged and unstaged sections, staging moves the hunk and focus is lost, so Zed shows both interleaved and the cursor stays put. Stated priority: "Keyboard first". — [Zed blog](https://zed.dev/blog/git) [Doc]
- **JetBrains:** chevrons accept a change into the other pane, Ctrl-click appends instead, and undo/redo work for these operations. — [Diff Viewer docs](https://www.jetbrains.com/help/idea/differences-viewer.html) [Doc]

#### Scroll binding and change alignment
- **Vim family:** `scrollbind` plus filler lines. — [Neovim diff.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/diff.txt) [Doc]
- **Neovim `linematch:{n}`:** aligns the most similar lines inside a hunk, but above `{n}` total lines it does not, "because for very large diff hunks there will be a noticeable lag". Its help text calls `linematch:60` reasonable (a 30-line two-buffer hunk or a 20-line three-buffer hunk), yet the default is 40. — [options.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/options.txt) [Doc]
- **`inline:{none|simple|char|word}`:** `char` and `word` run a real internal diff inside the change. `word` merges gaps of 5 bytes or less into one highlight. — [options.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/options.txt) [Doc]
- **Defaults moved over two releases.** `linematch:40` became default in [0.11](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/news-0.11.txt), and `indent-heuristic` plus `inline:char` in [0.12](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/news-0.12.txt). The current default is `internal,filler,closeoff,indent-heuristic,inline:char,linematch:40`. — [options.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/options.txt) [Doc]
- **Anchors and caveats:** 0.12 added `'diffanchors'` for manually anchoring alignment. Ignoring blank lines (`iblank`) carries a warning that "the diff windows will get out of sync". — [news-0.12](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/news-0.12.txt); [options.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/options.txt) [Doc]
- **JetBrains:** synchronized scrolling is a setting. Highlighting offers Words, Lines, Split changes, Characters, or None. Ignore policies are None, Trim whitespaces, Ignore whitespaces, Ignore whitespaces and empty lines, and Ignore imports and formatting. — [Diff Viewer docs](https://www.jetbrains.com/help/idea/differences-viewer.html) [Doc]
- **VS Code:** `diffEditor.experimental.showMoves` is an experimental moved-code option. — [settings dump, unofficial gist](https://gist.github.com/jonathasborges1/98c25c59b92c1f906c23cf525ccb28ae) [Doc via third party]

#### Collapsing unchanged regions
- **Vim family:** folds, with 6 lines of context by default (`context:{n}`). A huge value disables folding. — [options.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/options.txt) [Doc]
- **VS Code:** `hideUnchangedRegions` has `enabled`, `contextLineCount`, `minimumLineCount` and `revealLineCount`, is off by default, and has a toolbar toggle. — [Monaco options](https://microsoft.github.io/monaco-editor/typedoc/interfaces/editor.IDiffEditorBaseOptions.html); [VS Code staging docs](https://code.visualstudio.com/docs/sourcecontrol/staging-commits) [Doc]
- **JetBrains:** "Collapse unchanged fragments", with the visible line count configurable. — [Diff Viewer docs](https://www.jetbrains.com/help/idea/differences-viewer.html) [Doc]
- **Zed:** multibuffer excerpts are collapsed by construction. `editor: expand all diff hunks` is the escape hatch. — [Zed git docs](https://zed.dev/docs/git) [Doc]

#### Ending a review
- **Neovim:** `:diffoff` for the window, `:diffoff!` for all diff windows in the tabpage. `closeoff` (default) ends diff mode when the second-to-last diff window closes, undoing a `:diffsplit`. — [diff.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/diff.txt); [options.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/options.txt) [Doc]
- **fugitive:** `dq` closes all diff buffers except the focused one and runs `:diffoff!`. `gq` closes the status buffer. — [fugitive.txt](https://raw.githubusercontent.com/tpope/vim-fugitive/master/doc/fugitive.txt) [Doc]
- **diffview:** `:DiffviewClose` or `:tabclose`, because the review owns a tabpage. No default close key is listed for the main view. — [README](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md) [Doc]
- **Ediff:** `q` ends the session, and `1q` also offers to delete variant buffers, though "Modified files and the results of merges are never deleted". `C-l` restores the window setup after you have wandered off. — [ediff.texi](https://raw.githubusercontent.com/emacs-mirror/emacs/master/doc/misc/ediff.texi) [Doc]
- **lazygit:** `<esc>` returns from the merge view to the files panel. — [keybindings](https://raw.githubusercontent.com/jesseduffield/lazygit/master/docs/keybindings/Keybindings_en.md) [Doc]

#### Staleness flags
- **diffview upstream** (`sindrets/diffview.nvim`) was last pushed 2024-08-02, per the [GitHub API](https://api.github.com/repos/sindrets/diffview.nvim). Its design is still the reference, but active work has moved. `dlyongemallo/diffview-plus.nvim` was pushed 2026-10-28 and describes itself as actively maintained ([API](https://api.github.com/repos/dlyongemallo/diffview.nvim)). `esmuellert/codediff.nvim` (VS Code-style diffs, 1,611 stars, pushed 2026-09-15) is a newer contender ([API](https://api.github.com/repos/esmuellert/codediff.nvim)).
- **JetBrains docs** are unversioned "current" pages. Meld's help text dates from 2012 ([example](https://meld.app/help/file-changes.html)) and is thin.

#### Sentiment [Anecdotal]
- Zed users pushed back on multibuffer-only review: "almost never what I want as an end user", and "handy" but sometimes "distracting", with no overview of where changes fall in a file. When asked to pick a default, commenters overwhelmingly chose full file. — [Zed discussion #33773](https://github.com/zed-industries/zed/discussions/33773)
- One Zed user asked for keyboard shortcuts for next/previous file and for staging. Others wanted single-file diffs from branch diffs and commit history, not just the working tree. — [same discussion](https://github.com/zed-industries/zed/discussions/33773)
- A Helix PR tester said "ideally I would be able to explore the code-base while reviewing a diff", and noted that one session per file pair forces a new session and LSP reload for each file. — [Helix PR #15631](https://github.com/helix-editor/helix/pull/15631)
- A Helix user switching from Neovim called a diff preview in the changed-file picker "one of my most missed features". — [Helix #12955](https://github.com/helix-editor/helix/issues/12955)

### Inferences
- **Baseline for a credible review mode:** file list reachable from inside the diff, hunk motion, single-key stage or revert, filler alignment, inline or stacked fallback, one-command exit. Missing any one is a noticeable gap against all the tools above.
- **Staging keeps your place.** Magit, Zed and fugitive differ in almost everything else, but in all three staging happens where you are reading. Forcing a trip to another view is the anti-pattern.
- **Review needs real code navigation.** Editable right sides (Zed, JetBrains, diffview's local files) let go-to-definition and last-minute edits work, and Helix testers asked for exactly that.
- **Exit has to clean up without closing the user's windows.** diffview's tabpage model gives this cheaply. Helix has no tabpages, so its diff session has to restore the prior layout itself.
- **Neovim's defaults are the fairest terminal benchmark:** similar-line alignment and char-level inline highlights on by default, with a size cap on the expensive step.

### Gaps
- I found no first-party description of how VS Code or Zed end a review beyond closing a tab.
- Ediff's session-group and registry UI for multi-file review was not extracted, so I make no claim about it.
- The VS Code multi-diff details rest on release-note search snippets rather than full page reads.
- Whether 1.143's file list has left experimental status is unconfirmed.

## How do 3-way merge tools design layout, labels, actions, progress and auto-merge?

### Takeaway
Nobody has "won" on layout. Tools offer 2-way+result (VS Code, diffview default, JetBrains), 3-way+result (git vimdiff, diffview `diff4_mixed`, kdiff3) and in-place single-pane (smerge, Zed, lazygit), and the mature ones let you switch. The strongest documented lessons are from VS Code: base must be reachable, the fast inline path must stay, and labels and result state must be honest about what the VCS actually did.

### Cited Findings

#### Comparison

| Tool | Default layout | Base | Side labels | Accept commands | Progress / resolved | Auto-merge | Source |
|---|---|---|---|---|---|---|---|
| VS Code merge editor | Incoming (left), Current (right), Result (bottom) | Hidden; on via More Actions | Current/Incoming; docs note that in a rebase "current" is the new base | Per-conflict accept either or both, Accept Combination; edit Result directly | Unresolved counter (click jumps to next); Complete Merge stages and closes | One-sided and identical changes auto-resolved | [docs](https://code.visualstudio.com/docs/sourcecontrol/merge-conflicts), [1.73](https://code.visualstudio.com/updates/v1_73), [1.74](https://code.visualstudio.com/updates/v1_74) |
| VS Code inline | Conflict markers with CodeLens | Only with diff3 markers | Current/Incoming | Accept Current, Incoming, Both; Compare Changes | Merge Changes list in SCM view | Git's | [docs](https://code.visualstudio.com/docs/sourcecontrol/merge-conflicts) |
| JetBrains | Left local, right repo (both read-only), centre editable result | Centre starts as base; Compare contents menu | Local/repository, branch names in brackets | Accept arrow, Ignore x, Resolve using Left/Right | Apply opens next file; Conflicts list; Revert conflict resolution; Accept and Finish | Toolbar Apply All Non-Conflicting (optional automatic); magic wand for same-line edits | [docs](https://www.jetbrains.com/help/idea/resolve-conflicts.html), [tutorial](https://jetbrains.com.cn/guide/java/tutorials/resolving-git-merge-conflicts/git-merge-dialog) |
| diffview merge tool | OURS, LOCAL, THEIRS (`diff3_horizontal`) | `diff4_mixed` adds BASE pane; `1do`, `<leader>cb` | OURS/THEIRS in winbar (on by default) | `<leader>co/ct/cb/ca`, `dx`; capitals for whole file; `2do/3do` | File panel shows remaining marker count, `!` unopened, check mark done | Git's result is the LOCAL file | [README](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md), [doc](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/doc/diffview.txt) |
| fugitive `:Gdiffsplit!` | Three-way against ours and theirs | Not in the layout; object syntax addresses index stages | Stage numbers (2 target, 3 merged) | `d2o`, `d3o` | `:Gwrite` stages; `dq` tidies up; `:Git mergetool` quickfix | Git's | [fugitive.txt](https://raw.githubusercontent.com/tpope/vim-fugitive/master/doc/fugitive.txt) |
| git mergetool vimdiff | LOCAL, BASE, REMOTE over MERGED | Shown by default | LOCAL/REMOTE/BASE | Vim `:diffget` | Save and quit; `:cq` aborts | `mergetool.hideResolved` off by default | [vimdiff docs](https://raw.githubusercontent.com/git/git/master/Documentation/mergetools/vimdiff.adoc), [config](https://raw.githubusercontent.com/git/git/master/Documentation/config/mergetool.adoc) |
| Ediff | Buffers A, B plus merge C | `/` toggles ancestor | A/B/C | `a`, `b`, `+` combine, `r` restore | `$$` and `$*` skip handled regions | One-sided regions copied automatically; `&` re-merges with a new default | [ediff.texi](https://raw.githubusercontent.com/emacs-mirror/emacs/master/doc/misc/ediff.texi) |
| smerge-mode (Emacs) | In place, on markers | `C-c ^ b`, plus `C-c ^ = <`, `= >`, `= =` diffs | Upper/Lower (renamed from mine/other) | `C-c ^ u/l/b/a`, `RET` for section at point | `n`/`p`; `smerge-auto-leave` exits when the last conflict is resolved | `smerge-resolve` tries auto-resolving | [smerge-mode.el](https://raw.githubusercontent.com/emacs-mirror/emacs/master/lisp/vc/smerge-mode.el) |
| Magit | Wraps smerge and Ediff | Via those | Not specified | `k` asks which side to keep; on a side, keeps it | smerge auto-stages on saving the last resolution | Git's | [Magit](https://docs.magit.vc/magit/Resolving-Conflicts.html) |
| KDiff3 | Inputs A (base), B, C above output | Always shown | Letters | Buttons A/B/C, click order sets line order; bulk for all, unsolved, or whitespace only | Go to Next Unsolved; saving disabled until all solved; auto-advance | Automatic; `Automatically Solve Simple Conflicts` | [handbook](https://invent.kde.org/sdk/kdiff3/-/raw/master/doc/en/index.docbook) |
| Meld | Not stated in its help | Not stated | Not stated | Arrows; Shift delete, Ctrl insert | Not stated | `--auto-merge` highlights only conflicts (Git default off) | [help](https://meld.app/help/file-changes.html), [Git config](https://raw.githubusercontent.com/git/git/master/Documentation/config/mergetool.adoc) |
| Zed | In place, on markers | Not described | Actual branch names | Use [branch], Use [other], Use Both | Warning icon in panel; stage and commit | Git's | [docs](https://zed.dev/docs/git) |
| lazygit | Single pane, conflict hunks | Not described | Hunk at cursor | `<space>` pick, `b` both, `z` undo, `M` menu | `h`/`l` next conflict | Git's | [keybindings](https://raw.githubusercontent.com/jesseduffield/lazygit/master/docs/keybindings/Keybindings_en.md) |

#### Layout flexibility
- diffview ships seven layouts, including `diff1_plain`, offered "in case you prefer to edit conflict markers manually without the visual noise from diffs", plus `diff3_mixed` and `diff4_mixed`. — [diffview.txt](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/doc/diffview.txt) [Doc]
- Git's vimdiff layouts are a small grammar, for example `LOCAL,MERGED,REMOTE` to drop base, or `MERGED` alone, with `+` for extra tabs. — [vimdiff docs](https://raw.githubusercontent.com/git/git/master/Documentation/mergetools/vimdiff.adoc) [Doc]

#### VS Code redesign history
- **1.69 (Jun 2022):** preview behind `git.mergeEditor`. Checkboxes accepted "Theirs" and "Yours", and word-level merging applied both sides when they did not intersect. — [1.69](https://code.visualstudio.com/updates/v1_69) [Doc]
- **1.70 (Jul 2022):** enabled by default. — [1.70](https://code.visualstudio.com/updates/v1_70) [Doc]
- **1.71 (Aug 2022):** stopped modifying the result file on open, restored the inline decorators, and added a one-click switch. — [1.71](https://code.visualstudio.com/updates/v1_71) [Doc]
- **Sep 2022, reversal:** the team set `git.mergeEditor` back to `false`. Stated reasons: "We have underestimated how good the previous inline conflict experience is", and "The large majority of the GitHub, Twitter feedback we got strongly prefers the previous inline conflict experience". The editor stayed as opt-in with a button. — [#160806](https://github.com/microsoft/vscode/issues/160806) [Maint]
- **1.72 (Sep 2022):** the result is recomputed from scratch. One-sided conflicts resolve automatically, proper conflicts reset to base and are marked "unhandled", a base view arrives, and CodeLens labels replace checkboxes because checkboxes were hard to discover and could not represent all states. The earlier design was called confusing because VS Code's merge algorithm and Git's did not align perfectly. — [1.72](https://code.visualstudio.com/updates/v1_72) [Doc]
- **Base pane:** a maintainer said an earlier release that replaced conflicts with base content was dropped because it "breaks other git commands/tools when the merge editor automatically removes all conflict markers". — [#155277](https://github.com/microsoft/vscode/issues/155277) [Maint]
- **1.73 (Oct 2022):** Accept Combination for conflicts with no character-level overlap, a clickable counter, and a focused-side diff against base. — [1.73](https://code.visualstudio.com/updates/v1_73) [Doc]
- **1.74 (Nov 2022):** undo/redo tracks handled state, equal changes auto-resolve, and "Mark as handled" arrives. — [1.74](https://code.visualstudio.com/updates/v1_74) [Doc]
- **Today:** Complete Merge stages only that file and closes the editor, and the docs caution that a zero conflict count does not confirm the code works. — [docs](https://code.visualstudio.com/docs/sourcecontrol/merge-conflicts) [Doc]

#### Rebase labelling
- Git itself swaps the sides: "the side reported as _ours_ is the so-far rebased series, starting with _<upstream>_, and _theirs_ is the working branch. In other words, the sides are swapped." — [git-rebase](https://git-scm.com/docs/git-rebase) [Doc]
- VS Code documents that labels change with the operation. It still has an open accessibility bug about left/right and Current/Incoming being reversed during rebase, and a maintainer called a fix "complicated". — [docs](https://code.visualstudio.com/docs/sourcecontrol/merge-conflicts); [#247687](https://github.com/microsoft/vscode/issues/247687) [Maint]
- A separate open issue covers "deleted by Us" during rebase, where a maintainer said "I agree feels backward with rebase". — [#113593](https://github.com/microsoft/vscode/issues/113593) [Maint]
- Zed sidesteps the words with real branch names (Use [branch-name]). — [Zed docs](https://zed.dev/docs/git) [Doc]
- Diffview documents OURS as "current branch" and THEIRS as "incoming branch" with no rebase note. — [diffview.txt](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/doc/diffview.txt) [Doc]

#### Published reasoning on panes and base
- **Against 3-way panes:** the diffconflicts README argues syntax highlighting alone cannot showcase differences among that many versions, and that default keys are "not well suited to moving individual changes between that many windows". Its fix splits the marker sides into a 2-way diff, keeping Git's auto-merged work, with base and history a separate view. — [diffconflicts](https://github.com/whiteinge/diffconflicts) [Doc, author's opinion]
- **For base always:** one issue argued the editor "perpetuates the dangerous myth that merging without seeing the base is safe". — [#159920](https://github.com/microsoft/vscode/issues/159920) [Anecdotal, closed as not planned]
- **Middle ground:** delta renders a zdiff3 conflict as two diffs, "from the ancestor to each side of the conflict". — [delta](https://dandavison.github.io/delta/merge-conflicts.html) [Doc]
- git-mediate has the user apply the simpler change to the other sides so the conflict becomes trivial, then detects that and picks a side. `-d` shows the diffs from base, and it `git add`s once all conflicts are gone. — [git-mediate](https://raw.githubusercontent.com/Peaker/git-mediate/master/README.md) [Doc]
- Git says the `merge` conflict style tends to produce smaller regions than `diff3`. — [merge config](https://raw.githubusercontent.com/git/git/master/Documentation/config/merge.adoc) [Doc]
- Tools disagree on zdiff3. git-mediate works with zdiff3, while mergiraf recommends diff3 because zdiff3 can strip braces and break parsing. — [git-mediate](https://raw.githubusercontent.com/Peaker/git-mediate/master/README.md); [mergiraf](https://mergiraf.org/usage.html) [Doc]

#### Automation and trust
- mergiraf returns line-based merges when they are clean, keeps markers when a merge looks suspicious, and when it does solve conflicts it prints an ID for `mergiraf review`. The docs do not treat auto-resolved output as final. — [mergiraf](https://mergiraf.org/); [usage](https://mergiraf.org/usage.html) [Doc]
- JetBrains' Apply All Non-Conflicting skips magic-wand simple conflicts, leaving them to the user. — [docs](https://www.jetbrains.com/help/idea/resolve-conflicts.html) [Doc]
- Git's `mergetool.hideResolved` rewrites LOCAL and REMOTE so only unresolved conflicts are presented. — [config](https://raw.githubusercontent.com/git/git/master/Documentation/config/mergetool.adoc) [Doc]
- jj records conflicts in commits and by default uses markers with a snapshot plus a diff to apply (`+++++++`, `%%%%%%%`), with an alternative `snapshot` style. Tools that parse only Git's markers will miss these. — [jj conflicts](https://raw.githubusercontent.com/jj-vcs/jj/main/docs/conflicts.md) [Doc]

#### Sentiment [Anecdotal]
- On #160806, one user called JetBrains' three-column editor "far superior". Another wrote "I love the new merge editor!" and questioned treating vocal opinions as consensus. A third said the "on-boarding to this concept was too steep". — [#160806](https://github.com/microsoft/vscode/issues/160806)
- One user wrote "the key is being able to see the base that was each person's starting point" and valued base "horizontally aligned and in scroll sync". Another said they would leave Beyond Compare if VS Code offered a 4-window view. — [#155277](https://github.com/microsoft/vscode/issues/155277)
- A DEV post rates IntelliJ's tool above others for Java. A JetBrains forum user preferred KDiff3 at auto-resolving. — [DEV](https://dev.to/korthout/git-mergetool-for-java-files-26jd); [JetBrains forum](https://intellij-support.jetbrains.com/hc/en-us/community/posts/206156239/comments/206258945)
- HN commenters recommend 3- or 4-pane tools and call base "key". Many say ours/theirs stays confusing after years, and suggest "current" and "other" or branch names. — [HN](https://news.ycombinator.com/item?id=47479687)

### Inferences
- **Default to a moderate layout and let users change it.** Two sides plus result, with base one key away, matches where VS Code and diffview ended up, while serving the users who insist on base.
- **Keep the inline path.** VS Code's reversal is the clearest natural experiment here: replacing plain marker resolution by default lost, and the rich view survived as opt-in. A Helix merge view should work from a file that still has markers and leave them valid.
- **Do not reset what the user already did.** VS Code's recompute-from-scratch design explains [#197391](https://github.com/microsoft/vscode/issues/197391), where manual fixes made before opening the editor are not recognised. This is my causal reading, not something the VS Code team states.
- **Name sides by what they are, not where they sit.** Branch or commit labels (Zed) avoid most of the rebase problem, and position-derived labels are exactly what broke VS Code's screen reader output.
- **Treat Zero Conflicts as a checkpoint, not as proof.** Both VS Code's docs and mergiraf's review step show mature tools saying so explicitly.

### Gaps
- JetBrains rebase labels could not be verified. Its docs do not cover them and my attempts to read the source bundle returned 404. A [2012 post](https://jessitron.com/2012/10/13/git-rebase-inside-intellij-idea/) says column headings ran opposite to expectations, but this is old and anecdotal.
- Meld's pane order and conflict navigation are not in its help pages.
- The Sep 2022 default change is described as merged "into 1.71" in #160806 and announced in the 1.72 notes. I infer a 1.71 recovery release, but did not confirm the exact build.
- I did not find whether fugitive or diffview swap anything during rebase. Their docs are silent.
- Sublime Merge, p4merge and Beyond Compare were not reviewed.

## What are the most common, well-documented UX complaints about merge and diff tools?

### Takeaway
The peer-reviewed evidence says the top barrier is understanding the conflicting code rather than operating the tool, and that tools degrade with complexity more than with size. Issue trackers add recurring specifics: hidden base, flipped labels during rebase, forced new defaults, undiscoverable controls, lost manual edits, and keyboard paths that are second-class.

### Cited Findings

#### Research [Research]
No study I found evaluates merge tool UIs head to head. The closest evidence is two linked studies of practitioner perception, from 2017 and 2019, so tool lists are dated.
- **Barriers:** the top-rated needs are ease of understanding conflicting code (mean 3.89 of 5), expertise in that area (3.72), and amount of information about it (3.62). Tools presenting understandable information rates 3.48, and trustworthiness of tools 3.12. — [Nelson et al. 2019](https://web.engr.oregonstate.edu/~sarmaa/wp-content/uploads/2020/08/Nelson2019_Article_TheLife-cycleOfMergeConflictsP.pdf)
- **Wanted improvements:** better usability (mean 3.43), filtering less relevant information (3.41), exploring history (3.30), graphical presentation (3.14), transparency (2.82), consistent terminology (2.53). — [McKee et al. 2017](https://epiclab.github.io/publications/icsme17-mckee.pdf)
- **Tool switching:** participants use an average of 2.5 tools and up to 7 per conflict. One interviewee said "I have to jump around between tools and copy and paste version numbers...this is why integration matters." — [McKee et al. 2017](https://epiclab.github.io/publications/icsme17-mckee.pdf)
- **Basic tools dominate:** Git (37 mentions), Vim/vi (17), unspecified text editor (14), Git Diff (11), GitHub (11), Eclipse (10), KDiff3 (9), Meld (8). — [McKee et al. 2017](https://epiclab.github.io/publications/icsme17-mckee.pdf)
- **Complexity hurts more than size:** mean toolset effectiveness falls from 4.28 (simple, small) to 3.78 (simple, large), 3.35 (complex, small) and 2.78 (complex, large). — [McKee et al. 2017](https://epiclab.github.io/publications/icsme17-mckee.pdf)
- **Trust:** 8.26% trust their toolset a little or not at all, and 42.15% report some gap. The authors note people discontinue tools after errors, so these are survivors' ratings. Interviewees mistrust tools that "obscure the steps and rationale". — [McKee et al. 2017](https://epiclab.github.io/publications/icsme17-mckee.pdf)

#### Issue-tracker patterns
| Complaint | Evidence | Type |
|---|---|---|
| Base hidden or awkward | [#155277](https://github.com/microsoft/vscode/issues/155277), [#159920](https://github.com/microsoft/vscode/issues/159920) | Maint and Anecdotal |
| ours/theirs unclear, flipped in rebase | [git-rebase](https://git-scm.com/docs/git-rebase), [#247687](https://github.com/microsoft/vscode/issues/247687), [#113593](https://github.com/microsoft/vscode/issues/113593), [#238260](https://github.com/microsoft/vscode/issues/238260), [GitLab #329842](https://gitlab.com/gitlab-org/gitlab/-/issues/329842) | Doc and Anecdotal |
| Wide-reaching default changed without opt-in | [#160806](https://github.com/microsoft/vscode/issues/160806) | Maint |
| Controls hard to discover | [1.72 on checkboxes](https://code.visualstudio.com/updates/v1_72) | Doc |
| Manual resolutions not recognised | [#197391](https://github.com/microsoft/vscode/issues/197391), closed out of scope | Maint |
| Keyboard paths broken | [#225319](https://github.com/microsoft/vscode/issues/225319): after a keyboard accept, the first range stayed "locked" until a mouse click (fixed 2024) | Issue |
| Slow open | [#192580](https://github.com/microsoft/vscode/issues/192580): minutes for a simple conflict | Issue |
| Accept Both yields invalid code | [VS Code docs](https://code.visualstudio.com/docs/sourcecontrol/merge-conflicts) warn about duplicates | Doc |
| Hunk-by-hunk picking is tedious in big files; users want a bulk action | [lazygit #1608](https://github.com/jesseduffield/lazygit/discussions/1608) (lazygit now has an `M` options menu) | Anecdotal |
| Markers hard to read and edit | [diffconflicts](https://github.com/whiteinge/diffconflicts) | Author opinion |
| LSP noise from markers | [Helix #5132](https://github.com/helix-editor/helix/issues/5132) ("the LSP freaks out about this"); diffview disables diagnostics in merge tool by default ([README](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md)) | Anecdotal and Doc |
| Misalignment and drift | Helix PR testers reported alignment drift and `]g` stuck on a change ([#15631](https://github.com/helix-editor/helix/pull/15631)) | Maint |
| Whitespace noise | Helix PR request to ignore head/tail whitespace ([#15631](https://github.com/helix-editor/helix/pull/15631)); JetBrains and kdiff3 handle this explicitly ([Diff Viewer](https://www.jetbrains.com/help/idea/differences-viewer.html), [KDiff3](https://invent.kde.org/sdk/kdiff3/-/raw/master/doc/en/index.docbook)) | Anecdotal and Doc |

GitLab's usability session recorded a new developer who first said they understood "use theirs" versus "use ours", then changed their answer, saying "I assume that GitLab is theirs ...". A team member noted that "ours and theirs can flip". — [GitLab #329842](https://gitlab.com/gitlab-org/gitlab/-/issues/329842)

### Inferences
- Filtering is the highest-leverage research-backed request, and unchanged-region collapse, `hideResolved` behaviour and conflict-only navigation all serve it.
- Help understanding the code (context, history, blame, LSP in the result) may matter more than a richer accept command set.
- Terminology ranks lowest in the survey (2.53), yet ours/theirs appears constantly in issues. Both can be true: people rate it low in general but get burned during rebase specifically. The survey did not isolate rebase.
- A terminal editor sits well here, since a merge view inside the editor removes tool-switching.

### Gaps
- No controlled usability comparison of 2-way versus 3-way layouts was found. The evidence is design reasoning and opinion.
- I could not quantify complaint frequency across tools. The table shows recurrence, not prevalence, and leans on VS Code because its tracker is the largest and most searchable.
- Reddit was not read directly. Community sentiment here comes from GitHub and HN.

## What works under terminal constraints: narrow width, no mouse, limited colour?

### Takeaway
Good tools treat side-by-side as width-dependent and offer an automatic or one-key fallback to stacked or single-column views. Three-pane layouts need that fallback even more, since each extra pane takes a share of the same columns. Non-colour signalling is well established (letters, gutter symbols, filler glyphs, semantic labels), and VS Code's screen-reader bug shows why these need to come from meaning, not from position.

### Cited Findings

#### Narrow terminals
- VS Code falls back to inline when narrow (900 px default, configurable). — [VS Code staging docs](https://code.visualstudio.com/docs/sourcecontrol/staging-commits); [Monaco options](https://microsoft.github.io/monaco-editor/typedoc/interfaces/editor.IDiffEditorBaseOptions.html) [Doc]
- difftastic detects terminal width (override with `--width`), and collapses to a single column for pure additions or removals. — [options.rs](https://raw.githubusercontent.com/Wilfred/difftastic/master/src/options.rs) [Doc]
- delta wraps rather than truncates side-by-side lines, with arrow markers showing where text wrapped and `--wrap-max-lines` to limit it. — [delta](https://dandavison.github.io/delta/side-by-side-view.html) [Doc]
- diffview offers `diff2_vertical`, `diff3_vertical`, `diff3_mixed` and `diff1_plain`, with `g<C-x>` to cycle. — [diffview.txt](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/doc/diffview.txt) [Doc]
- Ediff's `|` toggles split direction, and its manual suggests `m` when the frame "is too narrow and lines are cut off". — [ediff.texi](https://raw.githubusercontent.com/emacs-mirror/emacs/master/doc/misc/ediff.texi) [Doc]
- Git's vimdiff layouts can put BASE, LOCAL and REMOTE in extra tabs (`+`) rather than extra columns. — [vimdiff docs](https://raw.githubusercontent.com/git/git/master/Documentation/mergetools/vimdiff.adoc) [Doc]
- Single-pane resolution (smerge, lazygit, Zed, Helix PR #15791) needs no extra width at all. — [smerge-mode.el](https://raw.githubusercontent.com/emacs-mirror/emacs/master/lisp/vc/smerge-mode.el); [Helix PR #15791](https://github.com/helix-editor/helix/pull/15791)
- mergiraf's `--compact` shows only the mismatching parts, which can be narrower, though the output often needs reformatting afterwards. — [usage](https://mergiraf.org/usage.html) [Doc]

#### No mouse
- Terminal tools have keyboard paths throughout, as the sections above show. Mouse-first affordances such as Meld's arrows and JetBrains' chevrons need a keymap: JetBrains exposes them as assignable shortcuts under Version Control Systems | Diff & Merge. — [JetBrains](https://www.jetbrains.com/help/idea/comparing-files-and-folders.html); [Meld](https://meld.app/help/file-changes.html) [Doc]
- VS Code's #225319 bug shows keyboard flows can silently depend on mouse-set state: the active conflict did not follow the cursor. — [#225319](https://github.com/microsoft/vscode/issues/225319)
- kdiff3's "Automatically Go to Next Unsolved Conflict After Source Selection" is an accept-and-advance flow, but its manual warns to disable it before choosing lines from several inputs. — [handbook](https://invent.kde.org/sdk/kdiff3/-/raw/master/doc/en/index.docbook) [Doc]
- Zed's `stage and next` shows the same pattern for review. — [Zed docs](https://zed.dev/docs/git) [Doc]
- The Helix PR known-issues list includes "Clicking on a filler line places the cursor above the intended row", a reminder that filler lines need a defined cursor behaviour. — [PR #15631](https://github.com/helix-editor/helix/pull/15631) [Doc]

#### Limited colour and accessibility
- WCAG 2.2 SC 1.4.1: "Color is not used as the only visual means of conveying information, indicating an action, prompting a response, or distinguishing a visual element." — [W3C](https://www.w3.org/WAI/WCAG22/Understanding/use-of-color.html) [Doc]
- kdiff3's summary column prints the source letter per line, `?` for conflicts and `m` for manual edits. — [handbook](https://invent.kde.org/sdk/kdiff3/-/raw/master/doc/en/index.docbook) [Doc]
- diffview's FAQ suggests a `╱` filler via `fillchars`, so filler text is distinguishable by pattern as well as background. — [README](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md) [Doc]
- diffview's file panel uses `!`, a count, and a check mark for per-file conflict state. — [diffview.txt](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/doc/diffview.txt) [Doc]
- VS Code's Accessible Diff Viewer presents changes as a unified patch (F7), and audio cues signal inserted or deleted lines on navigation. — [VS Code docs](https://code.visualstudio.com/docs/sourcecontrol/staging-commits); [1.74](https://code.visualstudio.com/updates/v1_74) [Doc]
- VS Code's #247687 shows the failure case: a screen reader announces a side incorrectly during rebase, and generic "input1/input2" labels were called "not self-explanatory and very confusing". — [#247687](https://github.com/microsoft/vscode/issues/247687)
- difftastic signals changes with colour plus bold, and underlines novel words in comments. With colour off, its line numbers print unstyled. I did not confirm how changed text is marked in that mode. — [style.rs](https://raw.githubusercontent.com/Wilfred/difftastic/master/src/display/style.rs) [Doc]
- Helix's intra-line highlight needs theme contrast, per the PR's known issues: "won't show up well if the active theme has low contrast between `diff.delta`'s fg and bg". Theme reuse was also debated, which led to a `.view` scope namespace. — [PR #15631](https://github.com/helix-editor/helix/pull/15631)
- Helix's gutter symbols have had open and closed customisation requests. — [#6206](https://github.com/helix-editor/helix/issues/6206), [#13625](https://github.com/helix-editor/helix/pull/13625)

### Inferences
- Two 80-column panes need about 160 columns, and three need about 240. That is my arithmetic, not a sourced threshold. VS Code's 900 px is very roughly 100 to 120 columns depending on font, which suggests a tool can reasonably keep split view below 160 if long lines are handled.
- **Filler lines need a glyph** or diff rows become indistinguishable from blank code in low-colour themes.
- **Every pane needs a text label naming its role.** A statusline or header line (OURS main, THEIRS feature, RESULT) costs one row per pane and solves both colour-independence and rebase confusion.
- Theme fallback matters more in Helix than elsewhere, since it ships many community themes, so a `.view` background scope cannot be assumed to exist in each. Verify the branch's behaviour across several themes, including a 16-colour terminal.

### Gaps
- I found no published measurement of usable pane widths for side-by-side code in terminals.
- Colour-blind palette guidance specific to diff tools was not found in primary sources beyond the general WCAG rule.
- I did not confirm how Helix behaves with a merge view under 16-colour terminals, as that needs the branch itself.

## What does the Helix community expect, and how do selection-first editors adapt diffs?

### Takeaway
Demand is high and long-standing, with 256 thumbs-up on #405 and 81 on #5132. Maintainers want the feature but have flagged rendering cost and warned against proliferating modes. Scope lines exist: read-only VCS display is welcome in core, while write operations on the git index have been called plugin material, though at least one maintainer sketched staging via a generic `diffput`. Two upstream PRs overlap this branch's scope, and Kakoune supplies the strongest selection-first precedent.

### Cited Findings

#### Demand
- [#405 Side by side file diffing](https://github.com/helix-editor/helix/issues/405): open since Jul 2021, 256 thumbs-up, 29 comments, active Oct 2026. [#5132 In-editor resolution of merge conflicts](https://github.com/helix-editor/helix/issues/5132): open since Dec 2022, 81 thumbs-up. [#227 VCS/git support](https://github.com/helix-editor/helix/issues/227): open since 2021, 122 thumbs-up. Counts are from the GitHub API.
- Users on #405 share workarounds: keybindings that run `git diff` on the hunk into an overlay or split, and one built on `:diffg` and `:sh echo`. — [#405](https://github.com/helix-editor/helix/issues/405) [Anecdotal]
- Related open requests: [#13383](https://github.com/helix-editor/helix/issues/13383) diff hunks picker, [#12955](https://github.com/helix-editor/helix/issues/12955) diff as changed-file preview, [#14538](https://github.com/helix-editor/helix/pull/14538) diff gutter in picker preview, [#16252](https://github.com/helix-editor/helix/pull/16252) `editor.diff-base`, [#15938](https://github.com/helix-editor/helix/pull/15938) custom diff base with a picker, [#5825](https://github.com/helix-editor/helix/issues/5825) ignore staged changes.
- A vimdiff-style MVP was requested: two files, commands to copy chunks between files, next/previous difference. — [#405](https://github.com/helix-editor/helix/issues/405) [Anecdotal]
- Some prefer inline: "It is more convenient to use, e.g. go to definition feature". — [#9127](https://github.com/helix-editor/helix/issues/9127) [Anecdotal]
- Some users lean on lazygit via a keymap, with redraw and key-handling problems reported. — [#12045](https://github.com/helix-editor/helix/discussions/12045); [#13818](https://github.com/helix-editor/helix/issues/13818); [Helix wiki Recipes](https://github.com/helix-editor/helix/wiki/Recipes) [Anecdotal]

#### Maintainer positions
- **Scope** (the-mikedavis, Apr 2022): "much of the fugitive/magit/lazygit/gitui functionality belongs outside the core as plugin(s) but I think some read-only VCS information could belong in the core". — [#227](https://github.com/helix-editor/helix/issues/227) [Maint]
- **Write operations** (the-mikedavis, Apr 2022): "read-only operations on the index like diff gutters or a blame gutter are fine for core but anything that takes on write features like committing, stashing, pushing, etc. should live as a plugin", while adding that core versus plugin "tends to become a per-feature conversation" with no hard rule. — [#2261](https://github.com/helix-editor/helix/issues/2261) [Maint]
- **Staging** (pascalkuthe, Dec 2022): "Staging would probably work by writing to the git index (with a more generic `diffput` command) altough there are a ton of unanswered questions". A hunk preview "could be covered by a quick way to toggle side by side diffs". — [#5166](https://github.com/helix-editor/helix/issues/5166) [Maint]
- **One design** (pascalkuthe, Jan 2024): "We are also unlikely to implement 5 different diff modes so the decision which design to use will be made during implementation (and informed by testing)". — [#9127](https://github.com/helix-editor/helix/issues/9127) [Maint]
- **Conflicts fold into diff** (pascalkuthe, Dec 2022): suggested folding merge conflicts into side-by-side work to avoid special-cased behaviour. matklad preferred in-editor resolution, with accept exposed as a code action. — [#5132](https://github.com/helix-editor/helix/issues/5132) [Maint and Anecdotal]
- **Rendering** (archseer, Aug 2025): "a highly desired feature for me too", but it "requires pretty significant changes to the rendering". — [#405](https://github.com/helix-editor/helix/issues/405) [Maint]
- **Minimalism** (the-mikedavis, Apr 2024): rejected extra diff popup commands as unnecessary given `:reset-diff-change` followed by undo. — [#10587](https://github.com/helix-editor/helix/pull/10587) [Maint]
- **Vision:** "Selection -> Action", "We aren't playing code golf" (the keymap should be "consistent and easy to memorize"), "Batteries included", "Don't try to be everything for everyone", and "The basics should be built-in". — [vision.md](https://raw.githubusercontent.com/helix-editor/helix/master/docs/vision.md) [Doc]

#### Overlapping upstream work (all open)
- **[PR #15631](https://github.com/helix-editor/helix/pull/15631) side-by-side viewer:** non-draft, with 7 thumbs-up, and `pascalkuthe` requested as reviewer with no review visible. It provides `:diff-open`/`:diffs`, `:diff-this`, `:diff-off`, `:diffget`, `:diffput`, `hx --diff`, session-aware `]g`/`[g`, filler via `LineAnnotation`, and word-level intra-line highlights.
- **archseer's review of that PR** said "Great work, this is awesome to see!", proposed a palette, and expected the cursor to sync on window switch: "I'd expect the cursor to stay on the same relative line position", noting that today's jump is "disorientating". — [PR #15631 comment](https://github.com/helix-editor/helix/pull/15631) [Maint]
- **[PR #15791](https://github.com/helix-editor/helix/pull/15791) smerge-inspired conflicts (draft):** background colours for each section, word-level differences, diagnostics suppressed inside conflicts, a parser for git 2-way, diff3 and jj N-way markers, `]=`/`[=` motions, and `Space x` then `c`/`i`/`b`/`a`, with `xx` for the section under the cursor. No maintainer comment visible.
- **[Discussion #13218](https://github.com/helix-editor/helix/discussions/13218):** argues Helix does not need Magit but that conflict resolution is the real gap, because external TUIs only pick sides and resolving in the editor gets LSP validation.
- **[#15611 multi-buffer type](https://github.com/helix-editor/helix/issues/15611):** RoloEdits floated a vertical multi-buffer replacing side-by-side splits and a "Files changed"-style view. A prototype exists on a fork.

#### Keymap constraints
- `]g`/`[g`/`]G`/`[G` are change motions, while `]c` is comment and `]x` is XML element, so Vim and diffview bindings do not port directly. `]=`/`[=` are not listed. — [keymap.md](https://raw.githubusercontent.com/helix-editor/helix/master/book/src/keymap.md) [Doc]
- `g` is already a text object for "Change". — [textobjects.md](https://raw.githubusercontent.com/helix-editor/helix/master/book/src/textobjects.md) [Doc]
- `:diffget` and `:diffg` are already aliases of `:reset-diff-change`. — [typable-cmd.md](https://raw.githubusercontent.com/helix-editor/helix/master/book/src/generated/typable-cmd.md) [Doc]
- `Space g` opens the changed-file picker. Space mode is described as "a kludge of mappings, mostly pickers", and `Space x` is not listed. — [keymap.md](https://raw.githubusercontent.com/helix-editor/helix/master/book/src/keymap.md) [Doc]
- Minor modes (`g`, `m`, `z`, `Z`, `Ctrl-w`, `Space`) "typically switch back to normal mode after a command". `Z` is a sticky variant, and users can define custom modes. — [keymap.md](https://raw.githubusercontent.com/helix-editor/helix/master/book/src/keymap.md); [remapping.md](https://raw.githubusercontent.com/helix-editor/helix/master/book/src/remapping.md) [Doc]

#### How other modal and selection-first editors adapt diffs
- **Kakoune `patch`:** "apply selections in diff to a file". The rule is elegant: a selection with a newline applies the selected lines, while "For selections that contain no newline, the entire enclosing diff hunk is applied", or the entire diff if the cursor is in a diff header. — [patch.kak](https://raw.githubusercontent.com/mawww/kakoune/master/rc/tools/patch.kak) [Doc]
- **Kakoune `git`:** `git apply` wraps this, with `--reverse`, `--cached`, `--index`, `--3way`, and "if buffile is tracked, use the changes to selected lines instead". It also has `show-diff`, `next-hunk`, `prev-hunk`. — [git.kak](https://raw.githubusercontent.com/mawww/kakoune/master/rc/tools/git.kak) [Doc]
- **kak-diffviz** offers `:diff-git`, `:diff-buffers`, `:diff-end`, but is experimental and last pushed 2020. — [kak-diffviz](https://www.github.com/winged/kak-diffviz)
- **Evil (evil-collection):** remaps Ediff's control panel to modal idioms: `n`/`N`, `gg`/`G`, `j`/`k` to scroll, `a`/`l` copy A to B, `b`/`h` copy B to A. — [evil-collection-ediff.el](https://raw.githubusercontent.com/emacs-evil/evil-collection/master/modes/ediff/evil-collection-ediff.el) [Doc]
- **Zed vim:** `] c`/`[ c`, `d o` expand, `d p` revert, `d u` stage and next, `d U` unstage and next. Note `do` no longer means "obtain". — [Zed vim docs](https://zed.dev/docs/vim) [Doc]
- **Zed Helix mode** exists but is a work in progress, and its docs describe no diff-specific Helix adaptation. — [Zed Helix docs](https://zed.dev/docs/helix) [Doc]

### Inferences
- **Fit patterns:** hunk motions on `]g`/`[g`, conflict motions on a new pair, `Ctrl-w`-consistent pane movement, picker-based file lists, Kakoune's selection rule, and accept verbs that work on every selection.
- **Poor-fit patterns:** Vim's count-prefixed `2do`/`3do` (numeric buffer IDs are hard to memorise, and verb-first keys run against Selection -> Action), mouse arrows, tabpage-owned sessions, and "control panels" that replace normal motions.
- **Selection-first gives a natural bulk gesture:** multi-select conflicts or hunks, then one accept. This could beat diffview's separate capital-letter whole-file commands. Worth checking whether the branch applies accept across all selections.
- **Expect scrutiny on breadth:** many diff entry points are fine if they share one session abstraction. The "5 different diff modes" remark suggests maintainers would resist separate mechanisms.
- **Staging is the most contested scope line.** Read-only diff is safest for upstream. Staging fits pascalkuthe's `diffput` sketch more than the-mikedavis's 2022 plugin stance, but both are years old and no newer statement was found.
- **Alignment with #15631 names** (`:diff-open`, `:diffget`, `:diffput`, `:diff-off`, `hx --diff`) reduces friction, whichever lands first.
- **Accept commands are in a conflict namespace too:** #15791's `Space x` scheme is the nearest precedent, and two incompatible schemes would hurt users.

### Gaps
- I found no maintainer statement from 2025 or 2026 on staging or on merge views. PR #15791 has no maintainer comments, and PR #15631 has no review from `pascalkuthe`.
- The current state of Helix's plugin system was not researched. A contributor was still asking in Oct 2025 whether to wait for it ([#5132](https://github.com/helix-editor/helix/issues/5132)).
- Maintainer comments on PR-review threads (as opposed to issue comments) were read only through a page summary, so inline review remarks may be missing.
- Evil's and Kakoune's merge-conflict workflows beyond those above were not found.

## Derived checklist: UX heuristics for diff and merge in a terminal modal editor

### Takeaway
Twenty-six heuristics follow, ranked M (must), S (should), C (could). Each is written as a testable check against the branch. The justification is the source evidence; the ranking is my judgement.

### Cited Findings

#### Review
| # | Pri | Heuristic and test | Justification | Source |
|---|---|---|---|---|
| 1 | M | One hunk motion everywhere: `]g`/`[g` work in all diff views, in both panes, and never get stuck on a change | Universal pattern; archseer hit the stuck case | [Neovim](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/diff.txt), [PR #15631](https://github.com/helix-editor/helix/pull/15631) |
| 2 | M | Move across files without leaving the diff, including from the last hunk or by a next-file key | diffview `<tab>`, JetBrains `Alt+Right`; VS Code added this only in 2025; Zed users asked for it | [diffview](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md), [JetBrains](https://www.jetbrains.com/help/idea/differences-viewer.html), [1.106](https://code.visualstudio.com/updates/v1_106), [Zed](https://github.com/zed-industries/zed/discussions/33773) |
| 3 | M | Panes stay aligned through scrolling, edits and soft wrap; switching panes lands on the matching line | Filler and scrollbind are core; archseer explicitly expected sync | [Neovim](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/diff.txt), [PR #15631](https://github.com/helix-editor/helix/pull/15631) |
| 4 | M | Intra-line highlights use a real inner diff and stay cheap on large hunks | Neovim default `inline:char`; `linematch` caps cost to avoid lag | [options.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/options.txt), [news-0.12](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/news-0.12.txt) |
| 5 | S | Unchanged regions can collapse, with context lines and quick reveal | Every major tool; filtering is the second-ranked request | [JetBrains](https://www.jetbrains.com/help/idea/differences-viewer.html), [McKee](https://epiclab.github.io/publications/icsme17-mckee.pdf) |
| 6 | M | Right side stays a normal editable buffer with LSP, goto and undo | Zed's and JetBrains' editable views; Helix users want to explore during review | [Zed blog](https://zed.dev/blog/git), [PR #15631](https://github.com/helix-editor/helix/pull/15631) |
| 7 | S | Act in place and keep focus: revert or apply does not jump, and a next action is one key away | Zed's focus reasoning; `stage and next` | [Zed blog](https://zed.dev/blog/git), [Zed docs](https://zed.dev/docs/git) |
| 8 | S | Actions respect selections: no-newline selection means whole hunk, a line selection means those lines, and multi-selection applies to each | Kakoune rule; Magit region; VS Code selection; Helix's Selection -> Action goal | [patch.kak](https://raw.githubusercontent.com/mawww/kakoune/master/rc/tools/patch.kak), [Magit](https://docs.magit.vc/magit/Staging-and-Unstaging.html), [vision.md](https://raw.githubusercontent.com/helix-editor/helix/master/docs/vision.md) |
| 9 | S | Destructive actions are undoable and say so | diffview and fugitive echo undo commands; JetBrains supports undo | [diffview](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md), [fugitive](https://raw.githubusercontent.com/tpope/vim-fugitive/master/doc/fugitive.txt) |
| 10 | M | One clean exit restores the prior layout, leaves files intact and works from any pane | `:diffoff!`, `dq`, `closeoff`, Ediff `q` and `C-l` | [Neovim](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/diff.txt), [fugitive](https://raw.githubusercontent.com/tpope/vim-fugitive/master/doc/fugitive.txt), [Ediff](https://raw.githubusercontent.com/emacs-mirror/emacs/master/doc/misc/ediff.texi) |
| 11 | C | Whitespace-ignore toggle that does not break alignment | Helix request; JetBrains and kdiff3; Neovim warns about `iblank` | [PR #15631](https://github.com/helix-editor/helix/pull/15631), [JetBrains](https://www.jetbrains.com/help/idea/differences-viewer.html), [options.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/options.txt) |

#### Merge
| # | Pri | Heuristic and test | Justification | Source |
|---|---|---|---|---|
| 12 | M | Resolution works from a marker file and never loses earlier manual edits or Git's auto-merged work | VS Code's recompute lost manual fixes; diffconflicts preserves Git's work; markers-removal broke other tools | [#197391](https://github.com/microsoft/vscode/issues/197391), [diffconflicts](https://github.com/whiteinge/diffconflicts), [#155277](https://github.com/microsoft/vscode/issues/155277) |
| 13 | M | Base is one key away, without a pane change or lost place | VS Code's top feedback; diff from base per side is the most useful form | [#155277](https://github.com/microsoft/vscode/issues/155277), [1.73](https://code.visualstudio.com/updates/v1_73), [delta](https://dandavison.github.io/delta/merge-conflicts.html) |
| 14 | M | Sides are labelled by real ref or commit, correct in merge, rebase and cherry-pick, and accept keys follow that meaning rather than screen position | Git swaps sides; VS Code's open bugs; Zed's branch-name labels | [git-rebase](https://git-scm.com/docs/git-rebase), [#247687](https://github.com/microsoft/vscode/issues/247687), [Zed](https://zed.dev/docs/git) |
| 15 | M | Full accept set: ours, theirs, both, base, none, plus a whole-file variant; after "both", the result is easy to review for duplicates | diffview's set is the fullest; lazygit users asked for bulk; docs warn Both may duplicate | [diffview](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md), [lazygit #1608](https://github.com/jesseduffield/lazygit/discussions/1608), [VS Code](https://code.visualstudio.com/docs/sourcecontrol/merge-conflicts) |
| 16 | M | Visible progress: conflicts left in file and in repo, next-unresolved motion, and per-file state in the list | VS Code counter; diffview markers; kdiff3 unsolved navigation | [VS Code](https://code.visualstudio.com/docs/sourcecontrol/merge-conflicts), [diffview.txt](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/doc/diffview.txt), [KDiff3](https://invent.kde.org/sdk/kdiff3/-/raw/master/doc/en/index.docbook) |
| 17 | S | Accepts are undoable, with handled state tracking undo | VS Code 1.74; Ediff `r`; lazygit `z` | [1.74](https://code.visualstudio.com/updates/v1_74), [Ediff](https://raw.githubusercontent.com/emacs-mirror/emacs/master/doc/misc/ediff.texi), [lazygit](https://raw.githubusercontent.com/jesseduffield/lazygit/master/docs/keybindings/Keybindings_en.md) |
| 18 | S | Finishing is explicit and honest: warn on unresolved conflicts, state what is staged, and do not imply the code works | VS Code warns and caveats; kdiff3 blocks save; mergiraf review | [1.69](https://code.visualstudio.com/updates/v1_69), [VS Code](https://code.visualstudio.com/docs/sourcecontrol/merge-conflicts), [KDiff3](https://invent.kde.org/sdk/kdiff3/-/raw/master/doc/en/index.docbook), [mergiraf](https://mergiraf.org/usage.html) |
| 19 | S | Result stays editable with LSP, and diagnostics behave sensibly in conflict regions | Editing Result is always the escape hatch; LSP noise is a Helix complaint; diffview disables diagnostics | [VS Code](https://code.visualstudio.com/docs/sourcecontrol/merge-conflicts), [#5132](https://github.com/helix-editor/helix/issues/5132), [#13218](https://github.com/helix-editor/helix/discussions/13218), [diffview](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md) |

#### Terminal and modal fit
| # | Pri | Heuristic and test | Justification | Source |
|---|---|---|---|---|
| 20 | M | Usable at 80 columns: automatic or one-key fallback to stacked or single-pane; long lines are handled deliberately | VS Code auto-inline; diffview layouts; delta wrapping | [VS Code](https://code.visualstudio.com/docs/sourcecontrol/staging-commits), [diffview.txt](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/doc/diffview.txt), [delta](https://dandavison.github.io/delta/side-by-side-view.html) |
| 21 | M | Nothing depends on colour alone: gutter symbols, filler glyph, pane title, usable on a monochrome or 16-colour theme | WCAG 1.4.1; kdiff3 letters; `╱` filler; PR contrast caveat | [WCAG](https://www.w3.org/WAI/WCAG22/Understanding/use-of-color.html), [KDiff3](https://invent.kde.org/sdk/kdiff3/-/raw/master/doc/en/index.docbook), [diffview](https://raw.githubusercontent.com/sindrets/diffview.nvim/main/README.md), [PR #15631](https://github.com/helix-editor/helix/pull/15631) |
| 22 | M | No mouse required, and keyboard actions target the cursor's conflict, never stale state | VS Code #225319; JetBrains exposes actions as shortcuts | [#225319](https://github.com/microsoft/vscode/issues/225319), [JetBrains](https://www.jetbrains.com/help/idea/comparing-files-and-folders.html) |
| 23 | M | Keys fit Helix: motions in `[`/`]`, panes through `Ctrl-w`, lists as pickers, new verbs discoverable in minor-mode help menus, no clash with `]c` and `]x` | Helix conventions; "consistent and easy to memorize" | [keymap.md](https://raw.githubusercontent.com/helix-editor/helix/master/book/src/keymap.md), [vision.md](https://raw.githubusercontent.com/helix-editor/helix/master/docs/vision.md), [#13165](https://github.com/helix-editor/helix/issues/13165) |
| 24 | S | Theme scopes are separate from gutter and diff-language scopes, with sane fallbacks in themes that define none | `.view` namespace outcome; archseer palette | [PR #15631](https://github.com/helix-editor/helix/pull/15631) |
| 25 | S | One session model for all entry points (working tree, commit, range, buffer pair, merge), with consistent commands, and opt-in, never replacing default file opening | "5 different diff modes"; VS Code default reversal | [#9127](https://github.com/helix-editor/helix/issues/9127), [#160806](https://github.com/microsoft/vscode/issues/160806) |
| 26 | C | Handles Git's three styles and jj markers, or says clearly what it does not support | PR #15791 parser; jj docs; zdiff3 disagreement; `merge.conflictStyle` | [PR #15791](https://github.com/helix-editor/helix/pull/15791), [jj](https://raw.githubusercontent.com/jj-vcs/jj/main/docs/conflicts.md), [mergiraf](https://mergiraf.org/usage.html) |

### Inferences
- **If time is limited, test 12, 13, 14, 16 and 20 first.** These cover the failures most often documented elsewhere and are the hardest to retrofit.
- **Evaluate first-run behaviour.** VS Code's history suggests first impressions drive reversal, so a user opening a conflicted file should see normal text with an optional merge view.

### Gaps
- Priorities are judgement calls. No study ranks these heuristics.
- Heuristics 8 and 23 reflect Helix's values rather than user testing, as I found no usability data on selection-first diff workflows.
