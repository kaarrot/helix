use std::path::Path;

use helix_view::review::{DiffSide, ReviewAnchor, ThreadId};
use helix_view::Document;
use helix_view::Editor;

use crate::commands::Context;

/// The file and side the focused view comments on, plus its document id.
fn identity_in(editor: &Editor) -> Option<(std::path::PathBuf, DiffSide)> {
    let (view, doc) = current_ref!(editor);
    view.review_identity(doc, &editor.diff.views)
}

fn identity(cx: &mut Context) -> Option<(std::path::PathBuf, DiffSide)> {
    identity_in(cx.editor)
}

/// Threads anchored in the focused document, as `(line, id)`, sorted by line.
fn threads_in_view(cx: &mut Context) -> Vec<(usize, ThreadId)> {
    // Hidden means hidden for the keys too: `c`, `d` and the motion stops all
    // fall back to what they normally do, so a box you cannot see cannot be
    // replied to or deleted by mistake.
    if cx.editor.diff.reviews.hidden {
        return Vec::new();
    }
    let Some((file, side)) = identity(cx) else {
        return Vec::new();
    };
    let (view, doc) = current_ref!(cx.editor);
    let _ = view;
    let text = doc.text();
    let mut threads: Vec<_> = cx
        .editor
        .diff
        .reviews
        .for_file(&file)
        .filter(|thread| thread.side == side)
        .map(|thread| (thread.line_in(&doc.review_anchors, text), thread.id))
        .collect();
    threads.sort_unstable();
    threads
}

fn cursor_line(cx: &mut Context) -> usize {
    let (view, doc) = current_ref!(cx.editor);
    let text = doc.text().slice(..);
    doc.selection(view.id).primary().cursor_line(text)
}

/// The thread anchored on the cursor's line, if any.
fn thread_at_cursor(cx: &mut Context) -> Option<ThreadId> {
    let line = cursor_line(cx);
    threads_in_view(cx)
        .into_iter()
        .find_map(|(thread_line, id)| (thread_line == line).then_some(id))
}

/// Comment on the current line, or reply to the thread already there.
///
/// Replying is the same key as commenting because it is the same intent from
/// the reader's side: say something about this line. A thread already on the
/// line makes it a reply, which is what turns a question and an answer into a
/// conversation.
pub fn review_add(cx: &mut Context) {
    // Claim the conversation first. Claiming is what loads the saved threads,
    // and until it happens the store is empty -- so on a freshly opened editor
    // an existing draft is invisible here and a second thread gets opened on
    // top of it. It also captures the name from the branch the review began on.
    let session = cx
        .editor
        .review_session()
        .map(|session| session.name.clone());

    // Asking to comment is asking to see them.
    cx.editor.diff.reviews.hidden = false;
    let existing = thread_at_cursor(cx);

    if let Some(id) = existing {
        // Replying while looking at an older entry continues from there: the
        // replies after it are dropped, because the point of going back is to
        // take the conversation a different way.
        let discarded = if is_focused(cx) {
            let index = cx
                .editor
                .diff
                .reviews
                .get(id)
                .map_or(0, |thread| thread.view_index());
            cx.editor.diff.reviews.rewind_to(id, index)
        } else {
            0
        };

        let had_draft = cx
            .editor
            .diff
            .reviews
            .get(id)
            .is_some_and(|thread| thread.is_pending());

        let Some(anchor) = ({
            let (view, doc) = current_ref!(cx.editor);
            let _ = view;
            cx.editor.diff.reviews.get(id).map(|thread| {
                let line = thread.line_in(&doc.review_anchors, doc.text()) as u32;
                (thread.file.clone(), thread.side, line)
            })
        }) else {
            return;
        };

        // So Ctrl-left/right still walk this thread while the box is open,
        // without first having to stop on it with j/k.
        cx.editor.diff.reviews.focused = Some(id);

        prompt_at_cursor(cx, "reply: ", anchor, move |cx, input, send_now| {
            if input.trim().is_empty() {
                return;
            }
            cx.editor
                .diff
                .reviews
                .set_draft(id, input.trim().to_string());
            let pending = cx.editor.diff.reviews.pending_count();
            crate::review_agent::schedule_save();
            let drafted = if discarded > 0 {
                // Say what was thrown away. This is the only place the
                // store discards text, so it should never be silent.
                format!("Reply drafted, {discarded} later entries discarded ({pending} pending)")
            } else if had_draft {
                // Say so: the previous unsent text is gone, and silently
                // dropping something the user typed would be worse.
                format!("Reply draft replaced ({pending} pending)")
            } else {
                format!("Reply drafted ({pending} pending)")
            };
            cx.editor.set_status(drafted);
            if send_now {
                send_now_from_box(cx, id);
            }
        });
        return;
    }

    let Some((file, side)) = identity(cx) else {
        cx.editor
            .set_error("Cannot comment here: this buffer has no file path");
        return;
    };
    let line = cursor_line(cx);

    prompt_at_cursor(
        cx,
        "comment: ",
        (file.clone(), side, line as u32),
        move |cx, input, send_now| {
            if input.trim().is_empty() {
                return;
            }
            let id = cx.editor.diff.reviews.draft(
                file.clone(),
                side,
                line as u32,
                input.trim().to_string(),
            );
            // Anchor it in the open document so it tracks edits from here on.
            let doc = doc_mut!(cx.editor);
            let anchor = ReviewAnchor::for_line(id, doc.text(), line);
            doc.review_anchors.push(anchor);

            let pending = cx.editor.diff.reviews.pending_count();
            crate::review_agent::schedule_save();
            cx.editor.set_status(match session.clone() {
                Some(name) => format!("Comment drafted ({pending} pending) · {name}"),
                None => format!("Comment drafted ({pending} pending)"),
            });
            if send_now {
                send_now_from_box(cx, id);
            }
        },
    );
}

/// How `Ctrl-S` / `Ctrl-Shift-S` should finish the comment box.
///
/// `Some(false)` saves a draft. `Some(true)` saves and sends. Both must be
/// recognised here: if the box ignores the key it falls through to normal mode,
/// which tries to send a draft that does not exist yet.
///
/// Helix stores `C-S-s` as Control+'S' (shift stripped, letter uppercased).
/// Live terminals may still report Control+Shift+'s' or Control+Shift+'S'.
/// Control+'s' with no shift, and the ASCII DC3 that some Windows consoles
/// send for Ctrl-S, mean save only.
fn comment_box_ctrl_s(key: helix_view::input::KeyEvent) -> Option<bool> {
    use helix_view::input::{KeyCode, KeyModifiers};
    if !key.modifiers.contains(KeyModifiers::CONTROL) {
        return None;
    }
    match key.code {
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::SHIFT) => Some(true),
        KeyCode::Char('S') => Some(true),
        KeyCode::Char('s') | KeyCode::Char('\u{13}') => Some(false),
        _ => None,
    }
}

/// A small multi-line editor for composing a comment.
///
/// `Prompt` cannot be reused here: it is a single-line control built for the
/// status bar, and a review comment is prose that wants to break lines. Enter
/// therefore inserts a newline. `Ctrl-S` saves a draft; `Ctrl-Shift-S` saves
/// and sends. Alt-Enter is not used: on Windows it toggles fullscreen.
struct CommentInput {
    label: String,
    lines: Vec<String>,
    row: usize,
    /// Char index within the current line.
    col: usize,
    /// Called with the text and whether to send it straight away.
    on_submit: Box<dyn FnMut(&mut crate::compositor::Context, &str, bool)>,
    /// Where it was last drawn, so the cursor can be placed in it.
    area: helix_view::graphics::Rect,
}

impl CommentInput {
    fn new(
        label: String,
        on_submit: impl FnMut(&mut crate::compositor::Context, &str, bool) + 'static,
    ) -> Self {
        Self {
            label,
            lines: vec![String::new()],
            row: 0,
            col: 0,
            on_submit: Box::new(on_submit),
            area: helix_view::graphics::Rect::default(),
        }
    }

    fn text(&self) -> String {
        self.lines.join("\n")
    }

    fn insert(&mut self, c: char) {
        let line = &mut self.lines[self.row];
        let at = line
            .char_indices()
            .nth(self.col)
            .map_or(line.len(), |(index, _)| index);
        line.insert(at, c);
        self.col += 1;
    }

    fn newline(&mut self) {
        let line = &mut self.lines[self.row];
        let at = line
            .char_indices()
            .nth(self.col)
            .map_or(line.len(), |(index, _)| index);
        let rest = line.split_off(at);
        self.lines.insert(self.row + 1, rest);
        self.row += 1;
        self.col = 0;
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            let line = &mut self.lines[self.row];
            let at = line
                .char_indices()
                .nth(self.col - 1)
                .map(|(index, _)| index);
            if let Some(at) = at {
                line.remove(at);
                self.col -= 1;
            }
        } else if self.row > 0 {
            // Joining onto the previous line keeps the cursor where the join
            // happened, which is where the eye already is.
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].chars().count();
            self.lines[self.row].push_str(&line);
        }
    }

    fn clamp_col(&mut self) {
        self.col = self.col.min(self.lines[self.row].chars().count());
    }
}

impl crate::compositor::Component for CommentInput {
    fn render(
        &mut self,
        viewport: helix_view::graphics::Rect,
        surface: &mut tui::buffer::Buffer,
        cx: &mut crate::compositor::Context,
    ) {
        let area = self.area(viewport, cx.editor);
        self.area = area;

        let theme = &cx.editor.theme;
        // Deliberately not `ui.popup`: themes commonly set it to something a
        // shade from the editor background, which leaves the box invisible --
        // you cannot tell you are typing into anything. A derived surface steps
        // far enough from the background to read as a box on any theme, and
        // `ui.review.input` overrides it for themes that would rather choose.
        let surface_style = theme
            .try_get("ui.review.input")
            .filter(|style| style.bg.is_some())
            .unwrap_or_else(|| crate::ui::diff::blended_surface(theme, 34, false));
        let hint = surface_style.patch(theme.get("ui.text.inactive"));
        let text = surface_style;

        // Paint every row explicitly rather than relying on a clear: the box has
        // to be opaque over the code it covers.
        let blank = " ".repeat(area.width as usize);
        for row in 0..area.height {
            surface.set_stringn(
                area.x,
                area.y + row,
                &blank,
                area.width as usize,
                surface_style,
            );
        }

        // The same left rail the comment rows use, so the box reads as the
        // thing it is about to become rather than as a different kind of object.
        let body_width = area.width.saturating_sub(1) as usize;
        for row in 0..area.height {
            surface.set_stringn(
                area.x,
                area.y + row,
                helix_view::review::MARKER,
                1,
                surface_style,
            );
        }

        // Rule out to the edge, the same way a thread's header does, so the
        // box has a visible top rather than trailing off into the buffer.
        let mut header = self.label.clone();
        let fill = body_width.saturating_sub(header.chars().count());
        header.extend(std::iter::repeat('─').take(fill));
        surface.set_stringn(area.x + 1, area.y, &header, body_width, hint);
        for (row, line) in self.lines.iter().enumerate() {
            let y = area.y + 1 + row as u16;
            if y >= area.y + area.height {
                break;
            }
            surface.set_stringn(area.x + 1, y, line, body_width, text);
        }
    }

    fn handle_event(
        &mut self,
        event: &crate::compositor::Event,
        cx: &mut crate::compositor::Context,
    ) -> crate::compositor::EventResult {
        use crate::compositor::EventResult;
        use helix_view::input::{KeyCode, KeyModifiers};

        // Paste has to be handled here. Ignoring it does not mean "nothing
        // happens": the event falls through to the editor and the text lands in
        // the document being reviewed instead of in the box.
        if let crate::compositor::Event::Paste(data) = event {
            for (i, line) in data.split('\n').enumerate() {
                if i > 0 {
                    self.newline();
                }
                for c in line.chars().filter(|c| *c != '\r') {
                    self.insert(c);
                }
            }
            return EventResult::Consumed(None);
        }

        let crate::compositor::Event::Key(key) = event else {
            // Anything else is swallowed rather than passed on, for the same
            // reason: while the box is open, keystrokes belong to it.
            return EventResult::Consumed(None);
        };

        let close = || {
            EventResult::Consumed(Some(Box::new(
                |compositor: &mut crate::compositor::Compositor,
                 cx: &mut crate::compositor::Context| {
                    compositor.pop();
                    // Give the space back, or the thread would stay hidden
                    // behind a box that is no longer there.
                    cx.editor.diff.reviews.composing = None;
                    // Replying sets focused so Ctrl-left/right walk while the
                    // box is open. Once it is gone the cursor is only on the
                    // line again: `j` must be able to stop on the box, and
                    // `d` must not delete an entry as if it already had.
                    cx.editor.diff.reviews.focused = None;
                },
            ) as crate::compositor::Callback))
        };

        match (key.code, key.modifiers) {
            (KeyCode::Esc, _) => return close(),
            _ => {
                if let Some(send_now) = comment_box_ctrl_s(*key) {
                    let text = self.text();
                    let mut on_submit =
                        std::mem::replace(&mut self.on_submit, Box::new(|_, _, _| {}));
                    on_submit(cx, &text, send_now);
                    return close();
                }
            }
        }

        match (key.code, key.modifiers) {
            (KeyCode::Enter, _) => self.newline(),
            (KeyCode::Backspace, _) => self.backspace(),
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::CONTROL) => self.insert(c),
            // Bare arrows move the caret in the box. Ctrl-arrows belong to the
            // thread (previous/next entry, scroll) and must not be swallowed
            // here, or they cannot reach the keymap.
            (KeyCode::Left, m) if !m.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.col = self.col.saturating_sub(1)
            }
            (KeyCode::Right, m) if !m.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.col = (self.col + 1).min(self.lines[self.row].chars().count())
            }
            (KeyCode::Up, m) if !m.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.row = self.row.saturating_sub(1);
                self.clamp_col();
            }
            (KeyCode::Down, m) if !m.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.row = (self.row + 1).min(self.lines.len() - 1);
                self.clamp_col();
            }
            (KeyCode::Home, _) => self.col = 0,
            (KeyCode::End, _) => self.col = self.lines[self.row].chars().count(),
            _ => return EventResult::Ignored(None),
        }

        // The box grows as it is written, so the space held for it has to grow
        // too, or the code below would sit under the last line typed.
        if let Some(composing) = cx.editor.diff.reviews.composing.as_mut() {
            composing.rows = self.lines.len() + 1;
        }

        EventResult::Consumed(None)
    }

    fn cursor(
        &self,
        _area: helix_view::graphics::Rect,
        _editor: &helix_view::Editor,
    ) -> (
        Option<helix_core::Position>,
        helix_view::graphics::CursorKind,
    ) {
        // Against where it was actually drawn, not the viewport it was offered.
        (
            Some(helix_core::Position::new(
                self.area.y as usize + 1 + self.row,
                self.area.x as usize + 1 + self.col,
            )),
            helix_view::graphics::CursorKind::Block,
        )
    }
}

impl CommentInput {
    /// Sit directly under the line being commented on, spanning the same width
    /// as the comment box it will become, so writing and reading a comment
    /// happen in the same place and the same shape.
    fn area(
        &self,
        viewport: helix_view::graphics::Rect,
        editor: &helix_view::Editor,
    ) -> helix_view::graphics::Rect {
        let height = (self.lines.len() as u16 + 1).min(viewport.height.max(2));
        let cursor_row = editor
            .cursor()
            .0
            .map_or(viewport.y, |position| position.row as u16);

        // Below the line when there is room, above it when there is not, so the
        // box never falls off the bottom of the screen.
        let below = cursor_row.saturating_add(1);
        let y = if below + height <= viewport.bottom() {
            below
        } else {
            cursor_row.saturating_sub(height)
        };

        helix_view::graphics::Rect::new(viewport.x, y, viewport.width, height)
    }
}

/// Send the comment just drafted in the box. Other unsent drafts stay pending:
/// `Space-m-R S` is the send-all path, same as after `Ctrl-S` in normal mode.
fn send_now_from_box(cx: &mut crate::compositor::Context, id: ThreadId) {
    match send_pending_ids(cx.editor, Some(id)) {
        Ok(sent) => cx.editor.set_status(match sent {
            1 => "Sent 1 comment".to_string(),
            n => format!("Sent {n} comments"),
        }),
        Err(message) => cx.editor.set_error(message),
    }
}

/// Put the input where the comment will appear, rather than on the status line
/// Put the input where the comment will appear, rather than on the status line
/// at the bottom of the screen.
///
/// The comment row itself cannot be typed into: virtual rows are painted by a
/// decoration and are not part of the document, so no cursor can go there. A
/// popup anchored under the line is the closest thing that is actually
/// editable, and it lands in the same place the comment will.
fn prompt_at_cursor(
    cx: &mut Context,
    label: &'static str,
    anchor: (std::path::PathBuf, DiffSide, u32),
    callback: impl FnMut(&mut crate::compositor::Context, &str, bool) + 'static,
) {
    // Hold room for the box being typed into, in place of the thread that lives
    // there. A one-line reply should not leave a tall answer underneath it.
    let (file, side, line) = anchor;
    cx.editor.diff.reviews.composing = Some(helix_view::review::Composing {
        file,
        side,
        line,
        rows: 2,
    });

    let input = CommentInput::new(
        format!("{label}  (ret: newline · ctrl-s: save · ctrl-shift-s: send · esc: cancel)"),
        callback,
    );

    cx.push_layer(Box::new(input));
}

/// How many lines either side of the anchor to quote.
const CONTEXT_LINES: usize = 6;

/// Compose what actually goes to the agent.
///
/// The user's comment stays short and pointed because Helix supplies the
/// context: with the file, the line and its surroundings attached, the agent
/// has no need to go looking, and no API for doing so is required.
fn compose_prompt(editor: &Editor, thread_id: ThreadId, comment: &str) -> String {
    // Take the file from the *thread*, not from whatever happens to be focused.
    // A batch send flushes drafts across several files, and quoting the focused
    // buffer for all of them would attach every comment to the wrong place.
    let Some(thread) = editor.diff.reviews.get(thread_id) else {
        return comment.to_string();
    };
    let file = thread.file.clone();
    let side = thread.side;
    let stored_line = thread.line as usize;
    // The draft has already become a message by this point, so anything beyond
    // the first means this is a follow-up in a conversation the agent is
    // already holding.
    let is_followup = thread.messages.len() > 1;
    let side_label = match side {
        DiffSide::Base => "base",
        DiffSide::Working => "working",
    };

    let (line, quoted) = quote_thread(editor, thread_id, &file, side, stored_line);

    let rewound = editor
        .diff
        .reviews
        .get(thread_id)
        .is_some_and(|thread| thread.rewound);

    if is_followup {
        // The agent is in the same session and still holds the replies that
        // were dropped, so it has to be told or it will answer as though they
        // still stood.
        let rewind_note = if rewound {
            "The reviewer has removed part of this thread since your last reply; \
             some of what was said no longer stands, so do not rely on it.\n\n"
        } else {
            ""
        };
        // No need to resend the quoted context: it is the same thread in the
        // same session, so the agent already has it. Only the line is worth
        // repeating, since edits may have moved it since the last turn.
        return format!(
            "A follow-up on the review comment at {}:{} (side: {side_label}).\n\n\
             {rewind_note}\
             follow-up: {comment}\n\n\
             Answer it.",
            file.display(),
            line + 1
        );
    }

    let range = match &editor.diff.range {
        Some(range) => format!("diff range: {range:?}\n"),
        // Said explicitly rather than omitted, so the agent does not assume a
        // diff it cannot see.
        None => "no diff range is set; this is the working tree\n".to_string(),
    };

    format!(
        "A review comment was left in the editor.\n\n\
         file: {}\n\
         side: {side_label}\n\
         line: {}\n\
         {range}\n\
         ```\n{quoted}```\n\n\
         comment: {comment}\n\n\
         Answer the comment.",
        file.display(),
        line + 1
    )
}

/// Quote the document that matches `side`, not whichever buffer happens to be
/// the working tree. A base-side comment is about the old text.
fn quote_thread(
    editor: &Editor,
    thread_id: ThreadId,
    file: &Path,
    side: DiffSide,
    stored_line: usize,
) -> (usize, String) {
    if let Some(doc) = document_for_side(editor, file, side) {
        let text = doc.text();
        let line = doc
            .review_anchors
            .iter()
            .find(|anchor| anchor.thread == thread_id)
            .map_or(stored_line, |anchor| anchor.line(text));
        return (line, quote_rope(text, line));
    }
    let contents = match side {
        DiffSide::Base => git_revision_text(editor, file),
        DiffSide::Working => std::fs::read_to_string(file).ok(),
    };
    (
        stored_line,
        contents.map_or_else(String::new, |contents| quote_str(&contents, stored_line)),
    )
}

fn document_for_side<'a>(editor: &'a Editor, file: &Path, side: DiffSide) -> Option<&'a Document> {
    if let Some(state) = diff_state_for_file(editor, file) {
        let id = match side {
            DiffSide::Base => state.base_doc_id,
            DiffSide::Working => state.working_doc_id,
        };
        return editor.document(id);
    }
    (side == DiffSide::Working)
        .then(|| {
            editor
                .documents()
                .find(|doc| !doc.is_virtual_base && doc.path().map(|p| p.as_path()) == Some(file))
        })
        .flatten()
}

fn diff_state_for_file<'a>(
    editor: &'a Editor,
    file: &Path,
) -> Option<&'a helix_view::diff_view::DiffViewState> {
    editor.diff.views.values().find(|state| {
        (!state.working_path.as_os_str().is_empty() && state.working_path == file)
            || (!state.base_path.as_os_str().is_empty() && state.base_path == file)
    })
}

fn git_revision_text(editor: &Editor, file: &Path) -> Option<String> {
    let git_ref = diff_state_for_file(editor, file)
        .map(|state| state.base_ref.as_str())
        .or_else(|| {
            editor
                .diff
                .range
                .as_ref()
                .map(|range| range.base_ref.as_str())
        })
        .unwrap_or("HEAD");
    #[cfg(feature = "git")]
    {
        helix_vcs::git::get_diff_base_from_ref(file, git_ref)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
    #[cfg(not(feature = "git"))]
    {
        let _ = git_ref;
        None
    }
}

fn quote_rope(text: &helix_core::Rope, line: usize) -> String {
    let last_line = text.len_lines().saturating_sub(1);
    let first = line.saturating_sub(CONTEXT_LINES);
    let last = (line + CONTEXT_LINES).min(last_line);
    let mut quoted = String::new();
    for n in first..=last {
        quoted.push_str(&quote_line(n, n == line, &text.line(n).to_string()));
    }
    quoted
}

fn quote_str(contents: &str, line: usize) -> String {
    let lines: Vec<&str> = contents.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let last = lines.len().saturating_sub(1);
    let first = line.saturating_sub(CONTEXT_LINES);
    let last = (line + CONTEXT_LINES).min(last);
    (first..=last)
        .filter_map(|n| {
            lines
                .get(n)
                .map(|content| quote_line(n, n == line, content))
        })
        .collect()
}

fn quote_line(n: usize, anchored: bool, content: &str) -> String {
    let marker = if anchored { ">" } else { " " };
    let content = content.trim_end_matches('\n');
    format!("{marker} {:>5} | {content}\n", n + 1)
}

/// Spawn the agent if this is the first send of the session.
fn ensure_agent(editor: &mut Editor) -> Result<(), String> {
    if editor.diff.agent.is_some() {
        return Ok(());
    }
    let Some(session) = editor.review_session() else {
        return Err("Cannot start a review session: this buffer is not in a repository".into());
    };
    let (uuid, worktree) = (session.uuid.clone(), session.worktree.clone());
    let kind = editor.diff.agent_kind;

    let agent: Box<dyn helix_view::review::agent::ReviewAgent> = match kind {
        helix_view::review::agent::ReviewAgentKind::Claude => {
            match crate::review_agent::ClaudeChildAgent::spawn(&uuid, &worktree) {
                Ok(agent) => Box::new(agent),
                Err(err) => return Err(format!("Could not start the agent: {err}")),
            }
        }
        helix_view::review::agent::ReviewAgentKind::Grok => {
            Box::new(crate::review_agent::GrokChildAgent::new(uuid, worktree))
        }
    };
    editor.diff.agent = Some(agent);
    Ok(())
}

/// Send every unsent draft, oldest first. Returns how many went out.
///
/// Each draft is its own turn so that each reply lands on the thread that asked
/// for it; they still share one conversation, so the agent sees them together.
pub fn send_pending(editor: &mut Editor) -> Result<usize, String> {
    send_pending_ids(editor, None)
}

fn send_pending_ids(editor: &mut Editor, only: Option<ThreadId>) -> Result<usize, String> {
    let pending: Vec<ThreadId> = editor
        .diff
        .reviews
        .pending()
        .map(|thread| thread.id)
        .filter(|id| only.is_none_or(|want| *id == want))
        .collect();

    if pending.is_empty() {
        return Err("No drafts to send".into());
    }
    ensure_agent(editor)?;

    let mut sent = 0;
    for id in pending {
        let Some(text) = editor.diff.reviews.take_draft(id) else {
            continue;
        };
        let prompt = compose_prompt(editor, id, &text);
        if let Some(thread) = editor.diff.reviews.get_mut(id) {
            thread.rewound = false;
        }
        let result = match editor.diff.agent.as_mut() {
            Some(agent) => agent.send(id, prompt),
            None => Err(anyhow::anyhow!("no agent")),
        };
        match result {
            Ok(()) => sent += 1,
            Err(err) => {
                editor.diff.reviews.apply_agent_event(
                    helix_view::review::agent::AgentEvent::Failed(id, err.to_string()),
                );
            }
        }
    }
    crate::review_agent::schedule_save();
    Ok(sent)
}

/// Each comment is its own turn in one shared conversation, so replies land on
/// the thread that asked while the agent still sees the others.
pub fn review_send_all(cx: &mut Context) {
    match send_pending(cx.editor) {
        Ok(sent) => cx.editor.set_status(match sent {
            1 => "Sent 1 comment".to_string(),
            n => format!("Sent {n} comments"),
        }),
        Err(message) => cx.editor.set_error(message),
    }
}

/// Send the draft on this line if there is one, otherwise save the selection
/// to the jumplist (`C-s` elsewhere).
///
/// After saving a comment the box is closed, so `Ctrl-S` / `Ctrl-Shift-S` land
/// in normal mode. Windows in particular often delivers both as `C-s`, which
/// would otherwise only write the jumplist.
pub fn review_send_or_save_selection(cx: &mut Context) {
    if !cx.editor.diff.reviews.hidden {
        if let Some(id) = thread_at_cursor(cx) {
            if cx
                .editor
                .diff
                .reviews
                .get(id)
                .is_some_and(|thread| thread.is_pending())
            {
                match send_pending_ids(cx.editor, Some(id)) {
                    Ok(sent) => cx.editor.set_status(match sent {
                        1 => "Sent 1 comment".to_string(),
                        n => format!("Sent {n} comments"),
                    }),
                    Err(message) => cx.editor.set_error(message),
                }
                return;
            }
        }
    }
    super::save_selection(cx);
}

pub fn review_toggle_collapse(cx: &mut Context) {
    let Some(id) = thread_at_cursor(cx) else {
        cx.editor.set_error("No review comment on this line");
        return;
    };
    if let Some(thread) = cx.editor.diff.reviews.get_mut(id) {
        thread.collapsed = !thread.collapsed;
    }
    crate::review_agent::schedule_save();
}

/// Hide or show every review box. The conversations are untouched: this only
/// stops them being drawn, and stops the keys that act on them from doing so.
pub fn review_toggle_visible(cx: &mut Context) {
    let hidden = !cx.editor.diff.reviews.hidden;
    cx.editor.diff.reviews.hidden = hidden;
    if hidden {
        // Nothing should stay focused that is no longer drawn.
        cx.editor.diff.reviews.focused = None;
    }

    let count = cx.editor.diff.reviews.len();
    cx.editor.set_status(if hidden {
        format!("Review boxes hidden ({count} threads)")
    } else {
        format!("Review boxes shown ({count} threads)")
    });
}

fn review_step_message(cx: &mut Context, forward: bool) {
    // Only when the box has been stopped on. Walking a thread's history is an
    // action on the box, so it should not fire from merely being near one.
    if !is_focused(cx) {
        cx.editor
            .set_error("No review comment focused; move onto its box first");
        return;
    }
    let Some(id) = thread_at_cursor(cx) else {
        return;
    };
    let Some(thread) = cx.editor.diff.reviews.get_mut(id) else {
        return;
    };
    let moved = thread.step_view(forward);
    if moved {
        // A different entry starts at its beginning, not wherever the last one
        // had been scrolled to.
        thread.reset_reading();
    }
    let (index, total) = (thread.view_index() + 1, thread.entry_count());
    if moved {
        cx.editor
            .set_status(format!("Comment entry {index}/{total}"));
    } else if forward {
        cx.editor
            .set_status(format!("Newest entry ({index}/{total})"));
    } else {
        cx.editor
            .set_status(format!("Oldest entry ({index}/{total})"));
    }
}

/// Width the focused view draws a comment body at.
///
/// The in-box cursor counts in wrapped rows, so moving it and copying from it
/// both have to measure at the width the box was actually drawn at.
fn box_width(cx: &mut Context) -> usize {
    let (view, doc) = current_ref!(cx.editor);
    view.inner_width(doc) as usize
}

/// Move the cursor inside the focused box, for an entry too tall to show at
/// once. The box scrolls when the cursor would otherwise leave it.
fn review_scroll(cx: &mut Context, down: bool) {
    if !is_focused(cx) {
        cx.editor
            .set_error("No review comment focused; move onto its box first");
        return;
    }
    let Some(id) = thread_at_cursor(cx) else {
        return;
    };
    let width = box_width(cx);
    if let Some(thread) = cx.editor.diff.reviews.get_mut(id) {
        let last = thread.body_rows(width).len().saturating_sub(1);
        thread.cursor = if down {
            thread.cursor.saturating_add(1).min(last)
        } else {
            thread.cursor.saturating_sub(1)
        };
    }
}

/// Copy from the focused box to the system clipboard: the rows selected with
/// the mouse, or the whole entry when nothing is selected.
///
/// `false` when there is no box to copy from, so the caller can fall back to
/// whatever the key means the rest of the time.
///
/// The clipboard rather than a register on purpose -- the point of copying a
/// reply is to paste it somewhere Helix does not reach, including back into a
/// comment box, which takes a terminal paste.
fn review_copy(cx: &mut Context) -> bool {
    if !is_focused(cx) {
        return false;
    }
    let Some(id) = thread_at_cursor(cx) else {
        return false;
    };
    let width = box_width(cx);
    let Some(thread) = cx.editor.diff.reviews.get(id) else {
        return false;
    };
    let Some(text) = thread.copy_text(width) else {
        cx.editor.set_error("Nothing to copy");
        return true;
    };
    let lines = text.lines().count().max(1);
    let partial = thread.selected_rows(width).is_some();

    match cx.editor.registers.write('+', vec![text]) {
        Ok(()) => {
            // The selection has served its purpose; leaving it standing would
            // make the next copy take something the reader has stopped
            // pointing at.
            if let Some(thread) = cx.editor.diff.reviews.get_mut(id) {
                thread.select = None;
            }
            cx.editor.set_status(if partial {
                format!("Copied {lines} lines to the clipboard")
            } else {
                format!("Copied the whole entry ({lines} lines) to the clipboard")
            });
        }
        Err(err) => cx.editor.set_error(err.to_string()),
    }
    true
}

pub fn review_copy_or_yank(cx: &mut Context) {
    if !review_copy(cx) {
        super::yank(cx);
    }
}

pub fn review_copy_or_yank_to_clipboard(cx: &mut Context) {
    if !review_copy(cx) {
        super::yank_to_clipboard(cx);
    }
}

/// Put the text cursor on a thread's anchored line and make its box the focused
/// one, which is what the keys that act on a box look for.
fn focus_thread(editor: &mut Editor, view_id: helix_view::ViewId, thread: ThreadId) {
    editor.focus(view_id);

    let doc_id = editor.tree.get(view_id).doc;
    let Some(doc) = editor.documents.get(&doc_id) else {
        return;
    };
    let text = doc.text();
    let line = doc
        .review_anchors
        .iter()
        .find(|anchor| anchor.thread == thread)
        .map(|anchor| anchor.line(text))
        .or_else(|| editor.diff.reviews.get(thread).map(|t| t.line as usize));
    let Some(line) = line else {
        return;
    };
    let pos = text.line_to_char(line.min(text.len_lines().saturating_sub(1)));

    let doc = doc_mut!(editor, &doc_id);
    doc.set_selection(view_id, helix_core::Selection::point(pos));
    editor.diff.reviews.focused = Some(thread);
}

/// A left press. In a box it points at the row pressed on and waits to see
/// whether the pointer moves; anywhere else it gives up a selection the reader
/// has stopped looking at. `true` when the press belonged to a box.
pub fn review_mouse_down(editor: &mut Editor, row: u16, column: u16) -> bool {
    let hit = editor.diff.reviews.hit_at(row, column);

    // Whatever was selected in a box, a press elsewhere ends it.
    if let Some(id) = editor.diff.reviews.focused {
        if hit.is_none_or(|hit| hit.thread != id) {
            if let Some(thread) = editor.diff.reviews.get_mut(id) {
                thread.select = None;
            }
        }
    }
    editor.diff.reviews.press = None;

    let Some(hit) = hit else {
        return false;
    };
    focus_thread(editor, hit.view, hit.thread);

    // A press selects nothing on its own: clicking into a box is asking to
    // point at it, and only dragging is asking for a range.
    if let Some(body) = hit.body {
        if let Some(thread) = editor.diff.reviews.get_mut(hit.thread) {
            thread.cursor = body;
            thread.select = None;
        }
        editor.diff.reviews.press = Some(helix_view::review::BoxPress {
            thread: hit.thread,
            body,
            dragged: false,
        });
    }
    true
}

/// The pointer moving with the button down. The first move is what turns the
/// press into a selection.
pub fn review_mouse_drag(editor: &mut Editor, row: u16) -> bool {
    let Some(press) = editor.diff.reviews.press else {
        return false;
    };
    // Rows outside the box clamp to its nearest one, so dragging past the edge
    // keeps extending rather than stopping dead.
    let Some(body) = editor.diff.reviews.drag_row(press.thread, row) else {
        return false;
    };
    if let Some(thread) = editor.diff.reviews.get_mut(press.thread) {
        thread.select = Some(press.body);
        thread.cursor = body;
    }
    editor.diff.reviews.press = Some(helix_view::review::BoxPress {
        dragged: true,
        ..press
    });
    true
}

/// The button coming back up. `true` when it ends a press that was in a box,
/// so the release is not also read as a selection made in the document.
pub fn review_mouse_up(editor: &mut Editor) -> bool {
    editor.diff.reviews.press.take().is_some()
}

pub fn review_scroll_down(cx: &mut Context) {
    review_scroll(cx, true);
}

pub fn review_scroll_up(cx: &mut Context) {
    review_scroll(cx, false);
}

pub fn review_prev_message(cx: &mut Context) {
    review_step_message(cx, false);
}

pub fn review_next_message(cx: &mut Context) {
    review_step_message(cx, true);
}

pub fn review_delete(cx: &mut Context) {
    let Some(id) = thread_at_cursor(cx) else {
        cx.editor.set_error("No review comment on this line");
        return;
    };
    cx.editor.diff.reviews.remove(id);
    let doc = doc_mut!(cx.editor);
    doc.review_anchors.retain(|anchor| anchor.thread != id);
    crate::review_agent::schedule_save();
    cx.editor.set_status("Comment deleted");
}

fn goto_review_comment_impl(cx: &mut Context, forward: bool) {
    let threads = threads_in_view(cx);
    if threads.is_empty() {
        cx.editor.set_error("No review comments in this file");
        return;
    }

    let line = cursor_line(cx);
    let target = if forward {
        threads
            .iter()
            .find(|(thread_line, _)| *thread_line > line)
            .or_else(|| threads.first())
    } else {
        threads
            .iter()
            .rev()
            .find(|(thread_line, _)| *thread_line < line)
            .or_else(|| threads.last())
    };
    let Some(&(target_line, _)) = target else {
        return;
    };

    let (view, doc) = current!(cx.editor);
    super::push_jump(view, doc);
    let text = doc.text().slice(..);
    let pos = text.line_to_char(target_line.min(text.len_lines().saturating_sub(1)));
    doc.set_selection(view.id, helix_core::Selection::point(pos));
    let scrolloff = cx.editor.config().scrolloff;
    let (view, doc) = current!(cx.editor);
    view.ensure_cursor_in_view_center(doc, scrolloff);

    let index = threads
        .iter()
        .position(|(thread_line, _)| *thread_line == target_line)
        .map_or(1, |index| index + 1);
    cx.editor
        .set_status(format!("Comment {index}/{}", threads.len()));
}

pub fn goto_next_review_comment(cx: &mut Context) {
    goto_review_comment_impl(cx, true);
}

pub fn goto_prev_review_comment(cx: &mut Context) {
    goto_review_comment_impl(cx, false);
}

/// Whether the cursor has stopped on the box below it.
fn is_focused(cx: &mut Context) -> bool {
    let Some(focused) = cx.editor.diff.reviews.focused else {
        return false;
    };
    thread_at_cursor(cx) == Some(focused)
}

/// Vertical motion that treats a comment box as a place the cursor can be.
///
/// A box is painted into virtual rows, which are not document text, so the
/// cursor cannot physically land in one -- it steps straight over. Instead the
/// box becomes an extra stop in the motion: going down, one press focuses it
/// and the next leaves the line; going up, one press lands on the line with it
/// focused and the next carries on. Either way passing a thread costs one more
/// press than passing a plain line, which is what makes it possible to stop
/// there at all.
fn review_line_move(cx: &mut Context, down: bool) {
    let plain = |cx: &mut Context| {
        if down {
            super::move_visual_line_down(cx)
        } else {
            super::move_visual_line_up(cx)
        }
    };

    // A count means the user is travelling, not reading. Stopping at every box
    // on the way would make `10j` mean something unpredictable.
    if cx.count.is_some() || cx.editor.mode() != helix_view::document::Mode::Normal {
        cx.editor.diff.reviews.focused = None;
        plain(cx);
        return;
    }

    if is_focused(cx) {
        cx.editor.diff.reviews.focused = None;
        plain(cx);
        return;
    }

    if down {
        match thread_at_cursor(cx) {
            // Stop on the box without moving: the cursor stays on the line the
            // thread belongs to, which is what the box is about.
            Some(id) => cx.editor.diff.reviews.focused = Some(id),
            None => plain(cx),
        }
    } else {
        plain(cx);
        // Landing on a line that carries a thread stops on its box, so going up
        // visits boxes as reliably as going down.
        cx.editor.diff.reviews.focused = thread_at_cursor(cx);
    }
}

pub fn review_line_down(cx: &mut Context) {
    review_line_move(cx, true);
}

pub fn review_line_up(cx: &mut Context) {
    review_line_move(cx, false);
}

/// `d` removes the entry being looked at when a box is focused, and otherwise
/// deletes the selection as it always does.
///
/// Unlike `c`, this requires the box to have been stopped on rather than merely
/// having the cursor on its line: it throws something away, so it should take a
/// deliberate act to reach.
pub fn review_delete_or_change(cx: &mut Context) {
    if !is_focused(cx) {
        return super::delete_selection(cx);
    }
    let Some(id) = thread_at_cursor(cx) else {
        return super::delete_selection(cx);
    };
    let index = cx
        .editor
        .diff
        .reviews
        .get(id)
        .map_or(0, |thread| thread.view_index());

    let remaining = cx.editor.diff.reviews.remove_entry(id, index);
    crate::review_agent::schedule_save();

    match remaining {
        Some(left) => cx
            .editor
            .set_status(format!("Entry deleted, {left} left in the thread")),
        None => {
            // The thread went with its last entry, so its anchor goes too.
            let doc = doc_mut!(cx.editor);
            doc.review_anchors.retain(|anchor| anchor.thread != id);
            cx.editor.diff.reviews.focused = None;
            cx.editor.set_status("Comment deleted");
        }
    }
}

/// `c` opens a reply when the cursor is on a thread, and otherwise changes the
/// selection as it always does.
///
/// The cost is real: on a line carrying a thread you cannot `c` to change the
/// text, which during a review is something you might well want. `s`, `d` then
/// `i`, or moving off the line, remain.
pub fn review_comment_or_change(cx: &mut Context) {
    if thread_at_cursor(cx).is_some() {
        review_add(cx);
    } else {
        super::change_selection(cx);
    }
}

/// `]c`/`[c` inside a diff view mean review comments; elsewhere they keep their
/// tree-sitter code-comment meaning. Reviewing is the only context where the
/// review sense is the more useful of the two.
pub fn goto_next_comment_or_review(cx: &mut Context) {
    if cx.editor.diff.views.contains_key(&cx.editor.tree.focus) {
        goto_next_review_comment(cx);
    } else {
        super::goto_next_comment(cx);
    }
}

pub fn goto_prev_comment_or_review(cx: &mut Context) {
    if cx.editor.diff.views.contains_key(&cx.editor.tree.focus) {
        goto_prev_review_comment(cx);
    } else {
        super::goto_prev_comment(cx);
    }
}

#[cfg(test)]
mod test {
    use super::comment_box_ctrl_s;
    use helix_view::input::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers }
    }

    #[test]
    fn ctrl_s_saves_and_ctrl_shift_s_sends() {
        use KeyCode::Char;
        use KeyModifiers as M;

        assert_eq!(
            comment_box_ctrl_s(key(Char('s'), M::CONTROL)),
            Some(false),
            "Ctrl-S saves a draft"
        );
        assert_eq!(
            comment_box_ctrl_s(key(Char('\u{13}'), M::CONTROL)),
            Some(false),
            "ASCII DC3 (legacy Ctrl-S) saves a draft"
        );

        // How the keymap stores C-S-s after FromStr normalisation.
        assert_eq!(
            comment_box_ctrl_s(key(Char('S'), M::CONTROL)),
            Some(true),
            "Control+'S' is Ctrl-Shift-S"
        );
        // How a terminal often reports the same chord, shift bit still set.
        assert_eq!(
            comment_box_ctrl_s(key(Char('s'), M::CONTROL | M::SHIFT)),
            Some(true),
            "Control+Shift+'s' sends"
        );
        assert_eq!(
            comment_box_ctrl_s(key(Char('S'), M::CONTROL | M::SHIFT)),
            Some(true),
            "Control+Shift+'S' sends"
        );

        assert_eq!(comment_box_ctrl_s(key(Char('s'), M::NONE)), None);
        assert_eq!(comment_box_ctrl_s(key(Char('x'), M::CONTROL)), None);
    }
}
