use std::path::{Path, PathBuf};

use helix_view::editor::Action;
use helix_view::review::{DiffSide, ReviewAnchor, Role, ThreadId};
use helix_view::{Document, DocumentId, Editor, ViewId};

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

/// Threads anchored on the cursor's line, in the order their boxes are drawn.
///
/// Usually one, but an edit can collapse two threads onto the same line: an
/// orphaned thread sits at the point its text was deleted from.
fn threads_at_cursor(cx: &mut Context) -> Vec<ThreadId> {
    let line = cursor_line(cx);
    threads_in_view(cx)
        .into_iter()
        .filter_map(|(thread_line, id)| (thread_line == line).then_some(id))
        .collect()
}

/// The thread the keys on the cursor's line act on, if any.
///
/// The focused box wins when it is on this line. Taking the first thread
/// instead would send a reply typed under one box to the conversation of the
/// box above it.
fn thread_at_cursor(cx: &mut Context) -> Option<ThreadId> {
    let on_line = threads_at_cursor(cx);
    cx.editor
        .diff
        .reviews
        .focused
        .filter(|id| on_line.contains(id))
        .or_else(|| on_line.first().copied())
}

/// Comment on the current line, or reply to the thread already there.
///
/// Replying is the same key as commenting because it is the same intent from
/// the reader's side: say something about this line. A thread already on the
/// line makes it a reply, which is what turns a question and an answer into a
/// conversation.
///
/// An unsent draft is the comment still being written. `Ctrl-S` only closes
/// the box, so the same key opens that draft again with the text in place.
/// A reply looked at from an older entry still starts blank: continuing from
/// there discards the draft along with everything after that entry. Nothing is
/// discarded until the reply is saved, so `Esc` leaves the thread as it was.
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
        // take the conversation a different way. Only once the reply is saved,
        // though. Dropping them as the box opens would lose them to an `Esc`.
        let rewind = if is_focused(cx) {
            cx.editor.diff.reviews.get(id).and_then(|thread| {
                let index = thread.view_index();
                (index + 1 < thread.entry_count()).then_some(index)
            })
        } else {
            None
        };

        // A rewind throws the draft away with the rest, so it is not one this
        // reply continues.
        let had_draft = rewind.is_none()
            && cx
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

        // The draft is what the box should show, caret at the end, ready to
        // continue. Not when rewinding: saving would discard it.
        let (initial, label, agent_session) = match cx.editor.diff.reviews.get(id) {
            Some(thread) => (
                if rewind.is_some() {
                    String::new()
                } else {
                    thread.draft.clone().unwrap_or_default()
                },
                if thread.messages.is_empty() {
                    "comment: "
                } else {
                    "reply: "
                },
                thread.agent_session.clone(),
            ),
            None => return,
        };

        prompt_at_cursor(
            cx,
            label,
            agent_session,
            anchor,
            &initial,
            move |cx, input, send_now| {
                if input.trim().is_empty() {
                    return;
                }
                let discarded =
                    rewind.map_or(0, |index| cx.editor.diff.reviews.rewind_to(id, index));
                cx.editor
                    .diff
                    .reviews
                    .set_draft(id, input.trim().to_string());
                let pending = cx.editor.diff.reviews.pending_count();
                crate::review_agent::schedule_save();
                let drafted = if discarded > 0 {
                    // Say what was thrown away. This is the only place the
                    // store discards text, so it should never be silent.
                    format!(
                        "Reply drafted, {discarded} later entries discarded ({pending} pending)"
                    )
                } else if had_draft {
                    format!("Draft updated ({pending} pending)")
                } else {
                    format!("Reply drafted ({pending} pending)")
                };
                cx.editor.set_status(drafted);
                if send_now {
                    send_now_from_box(cx, id);
                }
            },
        );
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
        None,
        (file.clone(), side, line as u32),
        "",
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
    /// The thread's agent conversation, shown at the right of the title row so
    /// it can be resumed in the agent itself. `None` for a new comment.
    session: Option<String>,
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
        session: Option<String>,
        initial: &str,
        on_submit: impl FnMut(&mut crate::compositor::Context, &str, bool) + 'static,
    ) -> Self {
        // `split` keeps a trailing empty line, so a draft that ended on Enter
        // reopens with the caret on that new line rather than glued to the
        // previous one. `lines()` would drop it.
        let mut lines: Vec<String> = initial.split('\n').map(str::to_string).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let row = lines.len() - 1;
        let col = lines[row].chars().count();
        Self {
            label,
            session,
            lines,
            row,
            col,
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

    /// Deletes back over any whitespace, then over one run of either word
    /// characters or punctuation, the way readline's Alt-Backspace does. At the
    /// start of a line it joins onto the previous one, like `backspace`.
    fn delete_word_backward(&mut self) {
        if self.col == 0 {
            self.backspace();
            return;
        }
        let line = &mut self.lines[self.row];
        let chars: Vec<char> = line.chars().collect();
        let mut start = self.col;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        if start > 0 {
            let is_word = |c: char| helix_core::chars::char_is_word(c);
            let word = is_word(chars[start - 1]);
            while start > 0
                && !chars[start - 1].is_whitespace()
                && is_word(chars[start - 1]) == word
            {
                start -= 1;
            }
        }
        let byte = |col: usize| line.char_indices().nth(col).map_or(line.len(), |(i, _)| i);
        let range = byte(start)..byte(self.col);
        line.replace_range(range, "");
        self.col = start;
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
        let header = helix_view::review::rule_with_tail(
            self.label.clone(),
            &[self.session.as_deref()],
            body_width,
        );
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
            // The same bindings `Prompt` uses. Terminals disagree on what
            // Ctrl-Backspace sends, so all three are taken.
            (KeyCode::Backspace, m) if m.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                self.delete_word_backward()
            }
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => self.delete_word_backward(),
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
    let outcome = send_pending_ids(cx.editor, Some(id));
    report_send(cx.editor, outcome);
}

/// Put the input where the comment will appear, rather than on the status line
/// at the bottom of the screen.
///
/// The comment row itself cannot be typed into: virtual rows are painted by a
/// decoration and are not part of the document, so no cursor can go there. A
/// popup anchored under the line is the closest thing that is actually
/// editable, and it lands in the same place the comment will.
///
/// `initial` is the unsent draft being continued, or empty for a new comment.
fn prompt_at_cursor(
    cx: &mut Context,
    label: &'static str,
    agent_session: Option<String>,
    anchor: (std::path::PathBuf, DiffSide, u32),
    initial: &str,
    callback: impl FnMut(&mut crate::compositor::Context, &str, bool) + 'static,
) {
    // Hold room for the box being typed into, in place of the thread that lives
    // there. A one-line reply should not leave a tall answer underneath it.
    // A resumed draft may already be several lines, so reserve that up front
    // rather than after the first keystroke.
    let (file, side, line) = anchor;
    let body_lines = if initial.is_empty() {
        1
    } else {
        initial.split('\n').count()
    };
    cx.editor.diff.reviews.composing = Some(helix_view::review::Composing {
        file,
        side,
        line,
        rows: body_lines + 1,
    });

    let input = CommentInput::new(
        format!("{label}  (ret: newline · ctrl-s: save · ctrl-shift-s: send · esc: cancel)"),
        agent_session,
        initial,
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
fn compose_prompt(
    editor: &Editor,
    thread_id: ThreadId,
    comment: &str,
    had_session: bool,
) -> String {
    // Take the file from the *thread*, not from whatever happens to be focused.
    // A batch send flushes drafts across several files, and quoting the focused
    // buffer for all of them would attach every comment to the wrong place.
    let Some(thread) = editor.diff.reviews.get(thread_id) else {
        return comment.to_string();
    };
    let file = thread.file.clone();
    let side = thread.side;
    let stored_line = thread.line as usize;
    // The draft has already become a message. A follow-up is a later turn in
    // the conversation this thread already started. Messages left over from
    // before that conversation existed are not one: the agent has not seen them.
    let is_followup = had_session && thread.messages.len() > 1;
    let earlier = earlier_transcript(thread, had_session);
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
        // This thread's conversation still holds the replies that were dropped,
        // so it has to be told or it will answer as though they still stood.
        let rewind_note = if rewound {
            "The reviewer has removed part of this thread since your last reply; \
             some of what was said no longer stands, so do not rely on it.\n\n"
        } else {
            ""
        };
        // No need to resend the quoted context: this thread's conversation
        // already has it. Only the line is worth repeating, since edits may
        // have moved it since the last turn.
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
         {earlier}\
         comment: {comment}\n\n\
         Answer the comment.",
        file.display(),
        line + 1
    )
}

/// Messages already on a thread whose agent conversation is only just starting.
///
/// A comment saved before each thread had its own conversation still has that
/// history in the editor. The new conversation has not seen it, and a follow-up
/// prompt would assume that it had.
fn earlier_transcript(thread: &helix_view::review::Thread, had_session: bool) -> String {
    if had_session || thread.messages.len() <= 1 {
        return String::new();
    }
    let mut out = String::from(
        "Earlier messages from this thread, which this conversation has not seen yet:\n\n",
    );
    for message in &thread.messages[..thread.messages.len() - 1] {
        let who = match message.role {
            Role::User => "reviewer",
            Role::Agent => "agent",
        };
        out.push_str(who);
        out.push_str(": ");
        out.push_str(&message.text);
        out.push_str("\n\n");
    }
    out
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
    let worktree = session.worktree.clone();
    let kind = editor.diff.agent_kind;

    let agent: Box<dyn helix_view::review::agent::ReviewAgent> = match kind {
        helix_view::review::agent::ReviewAgentKind::Claude => {
            Box::new(crate::review_agent::ClaudeChildAgent::new(worktree))
        }
        helix_view::review::agent::ReviewAgentKind::Grok => {
            Box::new(crate::review_agent::GrokChildAgent::new(worktree))
        }
    };
    editor.diff.agent = Some(agent);
    Ok(())
}

/// What a send did with the drafts it was asked to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendOutcome {
    /// Went out to the agent.
    pub sent: usize,
    /// Held until the reply already arriving on their thread lands.
    pub queued: usize,
}

/// Say what a send did, in the status line.
fn report_send(editor: &mut Editor, outcome: Result<SendOutcome, String>) {
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(message) => {
            editor.set_error(message);
            return;
        }
    };
    let comments = |n: usize| match n {
        1 => "1 comment".to_string(),
        n => format!("{n} comments"),
    };
    let status = match (outcome.sent, outcome.queued) {
        (sent, 0) => format!("Sent {}", comments(sent)),
        (0, 1) => "A reply is still arriving; this one goes out when it lands".to_string(),
        (0, queued) => format!(
            "Replies are still arriving; {} go out when they land",
            comments(queued)
        ),
        (sent, queued) => format!(
            "Sent {}; {queued} more wait for replies still arriving",
            comments(sent)
        ),
    };
    editor.set_status(status);
}

/// Send every unsent draft, oldest first.
///
/// Each draft is its own conversation, so two comments cannot see each other or
/// take each other's reply. A follow-up resumes the thread it belongs to.
pub fn send_pending(editor: &mut Editor) -> Result<SendOutcome, String> {
    send_pending_ids(editor, None)
}

/// Send a draft that was held back while its thread's previous reply was
/// arriving, now that the reply has landed.
///
/// Called for every finished turn. Does nothing unless a send was queued on
/// that thread and its draft is still there.
pub fn send_queued(editor: &mut Editor, id: ThreadId) {
    if !editor.diff.reviews.take_queued_send(id) {
        return;
    }
    let outcome = send_pending_ids(editor, Some(id));
    report_send(editor, outcome);
}

fn send_pending_ids(editor: &mut Editor, only: Option<ThreadId>) -> Result<SendOutcome, String> {
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

    let mut outcome = SendOutcome { sent: 0, queued: 0 };
    for id in pending {
        // One conversation runs one turn at a time. A second prompt sent while
        // the first reply streams would be answered into the same entry, so
        // the draft stays a draft until that reply lands.
        if let Some(thread) = editor
            .diff
            .reviews
            .get_mut(id)
            .filter(|thread| thread.awaiting)
        {
            thread.send_queued = true;
            outcome.queued += 1;
            continue;
        }
        // Captured before the session is created: that is what distinguishes a
        // follow-up from the first turn, including a thread saved before each
        // comment had its own conversation.
        let had_session = editor
            .diff
            .reviews
            .get(id)
            .is_some_and(|thread| thread.agent_session.is_some());
        let Some(session) = editor.diff.reviews.ensure_agent_session(id) else {
            continue;
        };
        let Some(text) = editor.diff.reviews.take_draft(id) else {
            continue;
        };
        let prompt = compose_prompt(editor, id, &text, had_session);
        if let Some(thread) = editor.diff.reviews.get_mut(id) {
            thread.rewound = false;
        }
        let result = match editor.diff.agent.as_mut() {
            Some(agent) => agent.send(id, session, prompt),
            None => Err(anyhow::anyhow!("no agent")),
        };
        match result {
            Ok(()) => {
                // Marked here rather than left to the agent's `Started`, which
                // arrives through the job queue: a second send before that job
                // runs would otherwise see nothing in flight.
                if let Some(thread) = editor.diff.reviews.get_mut(id) {
                    thread.awaiting = true;
                }
                outcome.sent += 1;
            }
            Err(err) => {
                editor.diff.reviews.apply_agent_event(
                    helix_view::review::agent::AgentEvent::Failed(id, err.to_string()),
                );
            }
        }
    }
    crate::review_agent::schedule_save();
    Ok(outcome)
}

/// Each comment is its own conversation. Replies stay on the thread that asked,
/// and one comment does not see the others.
pub fn review_send_all(cx: &mut Context) {
    let outcome = send_pending(cx.editor);
    report_send(cx.editor, outcome);
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
                let outcome = send_pending_ids(cx.editor, Some(id));
                report_send(cx.editor, outcome);
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

/// `gf` on a focused box opens the path the in-box cursor is on, as `gf` does
/// on text. Anywhere else it is `gf`.
pub fn review_goto_file_or_goto_file(cx: &mut Context) {
    if !is_focused(cx) {
        super::goto_file(cx);
        return;
    }
    let Some(id) = thread_at_cursor(cx) else {
        super::goto_file(cx);
        return;
    };
    goto_box_target(cx, id);
}

/// Ctrl+click on a box row: point the in-box cursor there and open what it is
/// on, in one go. `true` when the click belonged to a box body.
pub fn review_mouse_goto(cx: &mut Context, row: u16, column: u16) -> bool {
    let Some(hit) = cx.editor.diff.reviews.hit_at(row, column) else {
        return false;
    };
    let Some(body) = hit.body else {
        return false;
    };
    focus_thread(cx.editor, hit.view, hit.thread);
    if let Some(thread) = cx.editor.diff.reviews.get_mut(hit.thread) {
        thread.cursor = body;
        thread.cursor_col = box_column(&hit, column);
        thread.select = None;
    }
    cx.editor.diff.reviews.press = None;
    goto_box_target(cx, hit.thread);
    true
}

/// Where something in a box points.
#[derive(Debug, PartialEq)]
enum BoxTarget {
    File(PathBuf, helix_core::Position),
    Url(url::Url),
}

/// Open what the in-box cursor of `id` is on: a link covering the cursor
/// column, else a `path[:line[:col]]` in the row's text that names a file.
///
/// No fallback to the document's `gf`: the text cursor sits on the anchored
/// line, not on anything the reader pointed at, so falling through would open
/// whatever that line happens to mention.
fn goto_box_target(cx: &mut Context, id: ThreadId) {
    let width = box_width(cx);
    let Some(thread) = cx.editor.diff.reviews.get(id) else {
        return;
    };
    let rows = thread.body_rows(width);
    let row = thread.cursor.min(rows.len().saturating_sub(1));
    let links = thread.row_links(width, row);
    let base = thread.file.parent().map(Path::to_path_buf);
    let full = helix_view::review::body_width(width);
    let resolve = |text: &str| resolve_target(text, base.as_deref());
    let Some(target) = box_target(&rows, &links, row, thread.cursor_col, full, resolve) else {
        cx.editor
            .set_error("No file path on this row of the review comment");
        return;
    };
    match target {
        BoxTarget::Url(url) => super::open_url(cx, url, Action::Replace),
        BoxTarget::File(path, pos) => open_at(cx.editor, &path, pos),
    }
}

/// Open `path` with the cursor at `pos`.
///
/// A pane already showing the file is reused, so a path naming the file under
/// review moves within its diff rather than replacing it. Otherwise a diff pane
/// is split rather than replaced: the reader is following a reference out of
/// the review and will want to come back to it.
fn open_at(editor: &mut Editor, path: &Path, pos: helix_core::Position) {
    let showing = editor
        .tree
        .views()
        .filter(|(view, _)| {
            editor
                .document(view.doc)
                .and_then(|doc| doc.path())
                .is_some_and(|shown| shown.as_path() == path)
        })
        .map(|(view, focused)| (view.id, focused))
        .max_by_key(|(_, focused)| *focused)
        .map(|(view, _)| view);
    if let Some(view_id) = showing {
        editor.focus(view_id);
        let (view, doc) = current!(editor);
        super::push_jump(view, doc);
    } else {
        let focus = editor.tree.focus;
        let in_diff =
            editor.diff.views.contains_key(&focus) || editor.diff.merge_views.contains_key(&focus);
        let action = if in_diff {
            Action::HorizontalSplit
        } else {
            Action::Replace
        };
        if let Err(err) = editor.open(path, action) {
            editor.set_error(format!("Cannot open {}: {err}", path.display()));
            return;
        }
    }
    let (view, doc) = current!(editor);
    let at = helix_core::pos_at_coords(doc.text().slice(..), pos, true);
    doc.set_selection(view.id, helix_core::Selection::point(at));
    helix_view::align_view(doc, view, helix_view::Align::Center);
}

/// What a piece of text in a box names, if it names a file that exists (or a
/// URL). `base` is the reviewed file's directory: a relative path is tried
/// against the working directory, then the workspace the reviewed file is in,
/// then that directory itself.
fn resolve_target(text: &str, base: Option<&Path>) -> Option<BoxTarget> {
    if let Ok(url) = url::Url::parse(text) {
        // `C:` and the like parse as a scheme; only take what looks like a URL.
        if text.contains("://") {
            return match url.scheme() {
                "file" => url
                    .to_file_path()
                    .ok()
                    .filter(|path| path.is_file())
                    .map(|path| BoxTarget::File(path, helix_core::Position::default())),
                _ => Some(BoxTarget::Url(url)),
            };
        }
    }
    let (path, pos) = split_target(text);
    let path = helix_stdx::path::expand(&path).into_owned();
    let found = if path.is_absolute() {
        Some(path).filter(|path| path.is_file())
    } else {
        let mut bases = vec![helix_stdx::env::current_working_dir()];
        if let Some(base) = base {
            bases.push(helix_loader::find_workspace_in(base).0);
            bases.push(base.to_path_buf());
        }
        bases
            .into_iter()
            .map(|dir| dir.join(&path))
            .find(|path| path.is_file())
    }?;
    Some(BoxTarget::File(helix_stdx::path::canonicalize(found), pos))
}

/// `path:line[:col]`, or a link's `path#L12`, into a path and a position.
fn split_target(text: &str) -> (PathBuf, helix_core::Position) {
    if let Some((path, fragment)) = text.split_once('#') {
        let line = fragment
            .strip_prefix('L')
            .map(|rest| {
                rest.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
            })
            .and_then(|digits| digits.parse::<usize>().ok());
        let (path, pos) = crate::args::parse_file(path);
        return match line {
            Some(line) => (path, helix_core::Position::new(line.saturating_sub(1), 0)),
            None => (path, pos),
        };
    }
    crate::args::parse_file(text)
}

/// The target at display column `col` of body row `row`.
///
/// A link covering the column wins, since a link's text need not look like a
/// path at all. Otherwise the row's text is searched: the path covering the
/// column, else the first one on the row. Only candidates `resolve` accepts
/// count, so an ordinary word is never taken for a file.
///
/// A row filled to exactly `full` columns may be a long path broken by the
/// wrapper, so the rows after it are tried joined on as well.
fn box_target(
    rows: &[String],
    links: &[helix_view::annotations::rows::CommentLink],
    row: usize,
    col: usize,
    full: usize,
    resolve: impl Fn(&str) -> Option<BoxTarget>,
) -> Option<BoxTarget> {
    use helix_core::unicode::width::UnicodeWidthStr;

    if let Some(target) = links
        .iter()
        .filter(|link| (link.start..link.end).contains(&col))
        .find_map(|link| resolve(&link.dest))
    {
        return Some(target);
    }

    let line = rows.get(row)?;
    let at = byte_at_col(line, col);
    let mut texts = vec![line.clone()];
    let mut joined = line.clone();
    let mut last = line;
    for next in rows.iter().skip(row + 1).take(2) {
        if last.width() != full || next.starts_with(char::is_whitespace) {
            break;
        }
        joined.push_str(next);
        texts.push(joined.clone());
        last = next;
    }

    let mut candidates = Vec::new();
    // Longest text first, so a path that runs on past the row is preferred
    // to the piece of it the row shows.
    for text in texts.iter().rev() {
        for range in paths_in(text) {
            if range.start >= line.len() {
                continue;
            }
            let found = &text[range.clone()];
            if candidates.iter().any(|(start, _, _)| *start == range.start) {
                continue;
            }
            if let Some(target) = resolve(found) {
                candidates.push((range.start, range.end, target));
            }
        }
    }
    candidates.sort_by_key(|(start, _, _)| *start);
    let covering = candidates
        .iter()
        .position(|(start, end, _)| *start <= at && at <= *end);
    let pick = covering.unwrap_or(0);
    (pick < candidates.len()).then(|| candidates.swap_remove(pick).2)
}

/// Byte offset of display column `col` in `line`, clamped to its end.
fn byte_at_col(line: &str, col: usize) -> usize {
    use helix_core::unicode::width::UnicodeWidthChar;

    let mut width = 0;
    for (byte, ch) in line.char_indices() {
        width += ch.width().unwrap_or(0);
        if width > col {
            return byte;
        }
    }
    line.len()
}

/// Byte ranges of the paths in `text`, each with its `:line[:col]` if it has
/// one. The path pattern stops at `:` so that this suffix can be read off.
fn paths_in(text: &str) -> Vec<std::ops::Range<usize>> {
    let rope = helix_core::Rope::from(text);
    helix_stdx::path::find_paths(rope.slice(..), true)
        .map(|range| {
            let mut end = range.end;
            let mut numbers = 0;
            while numbers < 2 {
                let rest = &text[end..];
                let Some(digits) = rest.strip_prefix(':') else {
                    break;
                };
                let len = digits.bytes().take_while(u8::is_ascii_digit).count();
                if len == 0 {
                    break;
                }
                end += 1 + len;
                numbers += 1;
            }
            // Prose punctuation is allowed inside a path, but not at the end of
            // one: `see foo.rs.` means `foo.rs`.
            if numbers == 0 {
                end = range.start
                    + text[range.start..end]
                        .trim_end_matches(['.', ',', ';', '!', '?'])
                        .len();
            }
            range.start..end
        })
        .filter(|range| !range.is_empty())
        .collect()
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

/// Display column of a screen column inside a box row, after the marker.
fn box_column(hit: &helix_view::review::BoxHit, column: u16) -> usize {
    column.saturating_sub(hit.x.saturating_add(1)) as usize
}

/// A left press. In a box it points at the row and column pressed on and
/// waits to see whether the pointer moves; anywhere else it gives up a
/// selection the reader has stopped looking at. `true` when the press belonged
/// to a box.
pub fn review_mouse_down(editor: &mut Editor, row: u16, column: u16) -> bool {
    let hit = editor.diff.reviews.hit_at(row, column);

    // Whatever was selected in a box, a press elsewhere ends it. A copied id
    // stops being picked out on any press at all: it has been seen.
    if let Some(id) = editor.diff.reviews.focused {
        let elsewhere = hit.is_none_or(|hit| hit.thread != id);
        if let Some(thread) = editor.diff.reviews.get_mut(id) {
            thread.id_marked = false;
            if elsewhere {
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
            thread.cursor_col = box_column(&hit, column);
            thread.select = None;
        }
    } else {
        copy_header_id(editor, &hit, column);
    }
    // A press on the header owns the drag too. Left alone, the drag would reach
    // the document and select the code under the box.
    editor.diff.reviews.press = Some(helix_view::review::BoxPress {
        thread: hit.thread,
        body: hit.body,
        dragged: false,
    });
    true
}

/// A press on a header that lands on the conversation id copies it, since the
/// id is only there to be taken to `--resume`. It stays picked out until the
/// next press, so it is plain what was copied.
fn copy_header_id(editor: &mut Editor, hit: &helix_view::review::BoxHit, column: u16) {
    let col = box_column(hit, column);
    let Some(thread) = editor.diff.reviews.get(hit.thread) else {
        return;
    };
    let Some((start, end)) = thread.header_id_cols() else {
        return;
    };
    if !(start..end).contains(&col) {
        return;
    }
    let Some(id) = thread.agent_session.clone() else {
        return;
    };
    match editor.registers.write('+', vec![id]) {
        Ok(()) => {
            if let Some(thread) = editor.diff.reviews.get_mut(hit.thread) {
                thread.id_marked = true;
            }
            editor.set_status("Copied conversation id to the clipboard");
        }
        Err(err) => editor.set_error(err.to_string()),
    }
}

/// The pointer moving with the button down. The first move is what turns the
/// press into a selection.
pub fn review_mouse_drag(editor: &mut Editor, row: u16) -> bool {
    let Some(press) = editor.diff.reviews.press else {
        return false;
    };
    // Pressed on the header: nothing to select, but the drag is still ours.
    let Some(anchor) = press.body else {
        return true;
    };
    // Rows outside the box clamp to its nearest one, so dragging past the edge
    // keeps extending rather than stopping dead.
    let Some(body) = editor.diff.reviews.drag_row(press.thread, row) else {
        return false;
    };
    if let Some(thread) = editor.diff.reviews.get_mut(press.thread) {
        thread.select = Some(anchor);
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

/// One review comment `]C` / `[C` can land on.
#[derive(Clone)]
struct ReviewStop {
    file: PathBuf,
    side: DiffSide,
    /// Line in the open document when it is loaded, otherwise the stored line.
    line: usize,
    id: ThreadId,
}

/// Where the cursor is, so the next stop can be chosen relative to it.
struct Here {
    file: PathBuf,
    side: DiffSide,
    line: usize,
}

fn side_rank(side: DiffSide) -> u8 {
    match side {
        DiffSide::Base => 0,
        DiffSide::Working => 1,
    }
}

fn scratch_key(id: DocumentId) -> PathBuf {
    PathBuf::from(format!("[scratch {id}]"))
}

fn is_scratch_key(file: &Path) -> bool {
    file.to_str()
        .is_some_and(|name| name.starts_with("[scratch "))
}

/// The open document whose review threads are keyed as `(file, side)`.
///
/// A diff state's working path is the store key for both panes. Matching
/// `base_path` as well would hand a buffer-diff's working document back when
/// the caller asked about the other file.
fn document_for_stop<'a>(editor: &'a Editor, file: &Path, side: DiffSide) -> Option<&'a Document> {
    if let Some(state) = editor
        .diff
        .views
        .values()
        .find(|state| state.working_path == file)
    {
        let id = match side {
            DiffSide::Base => state.base_doc_id,
            DiffSide::Working => state.working_doc_id,
        };
        if let Some(doc) = editor.document(id) {
            return Some(doc);
        }
    }
    if side != DiffSide::Working {
        return None;
    }
    if let Some(doc) = editor.document_by_path(file) {
        if !doc.is_virtual_base {
            return Some(doc);
        }
    }
    editor
        .documents()
        .find(|doc| doc.path().is_none() && scratch_key(doc.id()) == file)
}

/// A view that already shows this comment. The focused one wins, so a jump
/// inside the current pane does not leap to another split of the same file.
fn view_showing(editor: &Editor, file: &Path, side: DiffSide) -> Option<ViewId> {
    let mut fallback = None;
    for (view, focused) in editor.tree.views() {
        let Some(doc) = editor.document(view.doc) else {
            continue;
        };
        let Some((path, view_side)) = view.review_identity(doc, &editor.diff.views) else {
            continue;
        };
        if path == file && view_side == side {
            if focused {
                return Some(view.id);
            }
            fallback.get_or_insert(view.id);
        }
    }
    fallback
}

fn resolved_line(editor: &Editor, id: ThreadId, stored: u32, file: &Path, side: DiffSide) -> usize {
    let Some(doc) = document_for_stop(editor, file, side) else {
        return stored as usize;
    };
    doc.review_anchors
        .iter()
        .find(|anchor| anchor.thread == id)
        .map(|anchor| anchor.line(doc.text()))
        .unwrap_or(stored as usize)
}

/// Working-tree comments can be opened. Base-side comments only exist on a
/// split pane; a single-pane diff reuses the working view, so there is nowhere
/// to show the base text without tearing that diff down.
fn stop_is_reachable(editor: &Editor, file: &Path, side: DiffSide) -> bool {
    if view_showing(editor, file, side).is_some() {
        return true;
    }
    if side != DiffSide::Working {
        return false;
    }
    if document_for_stop(editor, file, side).is_some() {
        return true;
    }
    !is_scratch_key(file) && file.is_file()
}

fn review_stops(editor: &Editor) -> Vec<ReviewStop> {
    let pending: Vec<_> = editor
        .diff
        .reviews
        .iter()
        .map(|thread| (thread.file.clone(), thread.side, thread.line, thread.id))
        .collect();
    let mut stops = Vec::new();
    for (file, side, stored, id) in pending {
        if !stop_is_reachable(editor, &file, side) {
            continue;
        }
        stops.push(ReviewStop {
            line: resolved_line(editor, id, stored, &file, side),
            file,
            side,
            id,
        });
    }
    stops.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(side_rank(a.side).cmp(&side_rank(b.side)))
            .then(a.line.cmp(&b.line))
            .then(a.id.cmp(&b.id))
    });
    stops
}

fn stop_key(stop: &ReviewStop) -> (&Path, u8, usize) {
    (stop.file.as_path(), side_rank(stop.side), stop.line)
}

fn here_key(here: &Here) -> (&Path, u8, usize) {
    (here.file.as_path(), side_rank(here.side), here.line)
}

fn pick_stop_index(
    stops: &[ReviewStop],
    here: Option<&Here>,
    forward: bool,
    count: usize,
) -> usize {
    let len = stops.len();
    let step = count.max(1).saturating_sub(1) % len;
    let start = if forward {
        stops
            .iter()
            .position(|stop| match here {
                Some(here) => stop_key(stop) > here_key(here),
                None => true,
            })
            .unwrap_or(0)
    } else {
        stops
            .iter()
            .rposition(|stop| match here {
                Some(here) => stop_key(stop) < here_key(here),
                None => false,
            })
            .unwrap_or(len - 1)
    };
    if forward {
        (start + step) % len
    } else {
        (start + len - step) % len
    }
}

fn comment_status(stops: &[ReviewStop], index: usize) -> String {
    let stop = &stops[index];
    let total = stops.len();
    let n = index + 1;
    let several_files = stops
        .iter()
        .map(|stop| stop.file.as_path())
        .collect::<std::collections::HashSet<_>>()
        .len()
        > 1;
    let name = stop
        .file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| stop.file.display().to_string());
    match (several_files, stop.side) {
        (true, DiffSide::Base) => format!("Comment {n}/{total} · {name} (base)"),
        (true, DiffSide::Working) => format!("Comment {n}/{total} · {name}"),
        (false, DiffSide::Base) => format!("Comment {n}/{total} (base)"),
        (false, DiffSide::Working) => format!("Comment {n}/{total}"),
    }
}

fn move_to_review_line(editor: &mut Editor, line: usize, record_jump: bool) {
    if editor.tree.try_get(editor.tree.focus).is_none() {
        return;
    }
    if record_jump {
        let (view, doc) = current!(editor);
        super::push_jump(view, doc);
    }
    let (view, doc) = current!(editor);
    let text = doc.text().slice(..);
    let line = line.min(text.len_lines().saturating_sub(1));
    let pos = text.line_to_char(line);
    doc.set_selection(view.id, helix_core::Selection::point(pos));
    let scrolloff = editor.config().scrolloff;
    let (view, doc) = current!(editor);
    view.ensure_cursor_in_view_center(doc, scrolloff);
}

/// Focus a pane that already shows `stop`. `switch` would replace the document
/// under a diff and leave the diff state describing a file it no longer shows.
fn focus_stop(editor: &mut Editor, stop: &ReviewStop) -> bool {
    let Some(view_id) = view_showing(editor, &stop.file, stop.side) else {
        return false;
    };
    if editor.tree.focus != view_id {
        editor.focus(view_id);
    }
    move_to_review_line(editor, stop.line, true);
    true
}

fn leave_focused_diff(editor: &mut Editor) {
    let focus = editor.tree.focus;
    if editor.diff.views.contains_key(&focus) {
        editor.close_diff_view(focus);
    }
    let focus = editor.tree.focus;
    if editor.tree.try_get(focus).is_some() && editor.diff.merge_views.contains_key(&focus) {
        editor.close_merge_view(focus);
    }
}

fn show_stop(editor: &mut Editor, stop: &ReviewStop) -> bool {
    if focus_stop(editor, stop) {
        return true;
    }

    let focus = editor.tree.focus;
    if editor.diff.views.contains_key(&focus) || editor.diff.merge_views.contains_key(&focus) {
        // The next comment is another file. Leave the diff first so the buffer
        // switch does not retarget a pane the diff still owns.
        leave_focused_diff(editor);
        if focus_stop(editor, stop) {
            return true;
        }
    }

    if editor.tree.try_get(editor.tree.focus).is_none() {
        editor.set_error("No view to show the review comment");
        return false;
    }
    if stop.side != DiffSide::Working {
        editor.set_error("That review comment is on a diff base that is not open");
        return false;
    }

    // Copy the id out before switching: the document borrow cannot live across
    // `switch`, which needs the editor mutably.
    let doc_id = document_for_stop(editor, &stop.file, stop.side).map(|doc| doc.id());
    if let Some(doc_id) = doc_id {
        let current = editor.tree.get(editor.tree.focus).doc;
        let switched = current != doc_id;
        if switched {
            // `switch` already records the buffer we left.
            editor.switch(doc_id, Action::Replace);
        }
        move_to_review_line(editor, stop.line, !switched);
        return true;
    }

    match editor.open(&stop.file, Action::Replace) {
        Ok(_) => {
            move_to_review_line(editor, stop.line, false);
            true
        }
        Err(err) => {
            editor.set_error(format!(
                "Cannot open {} for its review comment: {err}",
                stop.file.display()
            ));
            false
        }
    }
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

/// `]C` / `[C`: every review comment in the session, switching buffers.
///
/// Order is the file path, then the base side before the working side, then
/// the line. The list wraps. A pane that already shows the comment is focused.
/// A working-tree file that is not open is opened. A diff or merge pane is
/// left only when the next comment lives in some other file.
fn goto_review_comment_across_buffers(cx: &mut Context, forward: bool) {
    if cx.editor.diff.reviews.hidden {
        cx.editor.set_error("Review comments are hidden");
        return;
    }
    let stops = review_stops(cx.editor);
    if stops.is_empty() {
        cx.editor.set_error("No review comments");
        return;
    }
    let here = identity(cx).map(|(file, side)| Here {
        file,
        side,
        line: cursor_line(cx),
    });
    let index = pick_stop_index(&stops, here.as_ref(), forward, cx.count());
    let stop = stops[index].clone();
    if !show_stop(cx.editor, &stop) {
        return;
    }
    // Landing is not the same as stopping on the box with `j`. A stale focus
    // would make `d` act on whatever thread still matched.
    cx.editor.diff.reviews.focused = None;
    let status = comment_status(&stops, index);
    cx.editor.set_status(status);
}

pub fn goto_next_review_comment(cx: &mut Context) {
    goto_review_comment_across_buffers(cx, true);
}

pub fn goto_prev_review_comment(cx: &mut Context) {
    goto_review_comment_across_buffers(cx, false);
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

    let on_line = threads_at_cursor(cx);
    let focused = cx
        .editor
        .diff
        .reviews
        .focused
        .and_then(|id| on_line.iter().position(|candidate| *candidate == id));

    if let Some(at) = focused {
        // Boxes stacked on one line are each a stop, in the order drawn.
        let next = if down {
            on_line.get(at + 1)
        } else {
            at.checked_sub(1).and_then(|at| on_line.get(at))
        };
        if let Some(&id) = next {
            cx.editor.diff.reviews.focused = Some(id);
            return;
        }
        cx.editor.diff.reviews.focused = None;
        plain(cx);
        return;
    }

    if down {
        match on_line.first() {
            // Stop on the box without moving: the cursor stays on the line the
            // thread belongs to, which is what the box is about.
            Some(&id) => cx.editor.diff.reviews.focused = Some(id),
            None => plain(cx),
        }
    } else {
        plain(cx);
        // Landing on a line that carries a thread stops on its box, so going up
        // visits boxes as reliably as going down. The lowest box is the one
        // reached first from below.
        cx.editor.diff.reviews.focused = threads_at_cursor(cx).last().copied();
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
/// selection as it always does. An unsent draft on that line is opened again
/// so it can be edited, rather than replaced by a blank box.
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

/// `]c`/`[c` inside a diff view move between that view's review comments, and
/// stay in the file. Elsewhere they keep their tree-sitter code-comment
/// meaning. `]C`/`[C` are the session-wide motion.
pub fn goto_next_comment_or_review(cx: &mut Context) {
    if cx.editor.diff.views.contains_key(&cx.editor.tree.focus) {
        goto_review_comment_impl(cx, true);
    } else {
        super::goto_next_comment(cx);
    }
}

pub fn goto_prev_comment_or_review(cx: &mut Context) {
    if cx.editor.diff.views.contains_key(&cx.editor.tree.focus) {
        goto_review_comment_impl(cx, false);
    } else {
        super::goto_prev_comment(cx);
    }
}

#[cfg(test)]
mod test {
    use std::path::PathBuf;

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

    #[test]
    fn delete_word_backward_takes_one_word_or_punctuation_run() {
        let mut input =
            super::CommentInput::new(String::new(), None, "fix  foo.bar  ", |_, _, _| {});
        input.delete_word_backward();
        assert_eq!(input.text(), "fix  foo.");
        input.delete_word_backward();
        assert_eq!(input.text(), "fix  foo");
        input.delete_word_backward();
        assert_eq!(input.text(), "fix  ");
        input.delete_word_backward();
        assert_eq!(input.text(), "");
        assert_eq!(input.col, 0);

        // At the start of a line it joins onto the previous one.
        let mut input = super::CommentInput::new(String::new(), None, "one\n", |_, _, _| {});
        input.delete_word_backward();
        assert_eq!((input.text().as_str(), input.row, input.col), ("one", 0, 3));

        // Mid-line, only what is left of the caret goes.
        let mut input = super::CommentInput::new(String::new(), None, "ünï cödé", |_, _, _| {});
        input.col = 3;
        input.delete_word_backward();
        assert_eq!((input.text().as_str(), input.col), (" cödé", 0));
    }

    fn stop(file: &str, side: super::DiffSide, line: usize) -> super::ReviewStop {
        super::ReviewStop {
            file: PathBuf::from(file),
            side,
            line,
            id: super::ThreadId(line as u32 + super::side_rank(side) as u32),
        }
    }

    #[test]
    fn review_stops_walk_files_in_path_order_and_wrap() {
        use super::DiffSide::{Base, Working};
        use super::{pick_stop_index, Here};

        let stops = vec![
            stop("a.rs", Working, 1),
            stop("a.rs", Working, 8),
            stop("b.rs", Working, 0),
        ];
        let on_second = Here {
            file: PathBuf::from("a.rs"),
            side: Working,
            line: 8,
        };
        assert_eq!(pick_stop_index(&stops, Some(&on_second), true, 1), 2);
        assert_eq!(
            pick_stop_index(&stops, Some(&on_second), true, 2),
            0,
            "a count wraps to the first comment"
        );
        let on_first = Here {
            file: PathBuf::from("a.rs"),
            side: Working,
            line: 1,
        };
        assert_eq!(pick_stop_index(&stops, Some(&on_first), false, 1), 2);
        let on_beta = Here {
            file: PathBuf::from("b.rs"),
            side: Working,
            line: 0,
        };
        assert_eq!(pick_stop_index(&stops, Some(&on_beta), false, 1), 1);

        // Base comments of a file come before its working comments, so [C
        // from the first working comment lands on the base side. ]C wraps
        // back to it once the working side runs out.
        let with_base = vec![stop("a.rs", Base, 3), stop("a.rs", Working, 1)];
        let on_working = Here {
            file: PathBuf::from("a.rs"),
            side: Working,
            line: 1,
        };
        assert_eq!(pick_stop_index(&with_base, Some(&on_working), false, 1), 0);
        assert_eq!(pick_stop_index(&with_base, Some(&on_working), true, 1), 0);
        let on_base = Here {
            file: PathBuf::from("a.rs"),
            side: Base,
            line: 3,
        };
        assert_eq!(pick_stop_index(&with_base, Some(&on_base), true, 1), 1);

        assert_eq!(pick_stop_index(&stops, None, true, 1), 0);
        assert_eq!(pick_stop_index(&stops, None, false, 1), 2);
    }

    mod box_targets {
        use std::path::{Path, PathBuf};

        use helix_core::Position;
        use helix_view::annotations::rows::CommentLink;

        use super::super::{box_target, resolve_target, split_target, BoxTarget};

        /// Anything with an extension counts as a file, so the tests do not
        /// need one on disk.
        fn fake(text: &str) -> Option<BoxTarget> {
            let (path, pos) = split_target(text);
            let is_file = path.extension().is_some();
            is_file.then_some(BoxTarget::File(path, pos))
        }

        fn file(path: &str, line: usize, col: usize) -> Option<BoxTarget> {
            Some(BoxTarget::File(
                PathBuf::from(path),
                Position::new(line, col),
            ))
        }

        fn rows(rows: &[&str]) -> Vec<String> {
            rows.iter().map(|row| row.to_string()).collect()
        }

        #[test]
        fn a_path_with_its_line_is_found_inside_brackets() {
            let rows = rows(&["is a helper (/data/x/test_hdas.py:162) that saves"]);
            let col = rows[0].find("test").unwrap();
            assert_eq!(
                box_target(&rows, &[], 0, col, 80, fake),
                file("/data/x/test_hdas.py", 161, 0)
            );
        }

        #[test]
        fn the_column_picks_between_paths_and_a_word_is_not_a_path() {
            let rows = rows(&["a.rs:1:4 and b.rs:2"]);
            let on_b = rows[0].find("b.rs").unwrap() + 1;
            assert_eq!(
                box_target(&rows, &[], 0, on_b, 80, fake),
                file("b.rs", 1, 0)
            );
            // Not on either: the first path on the row, never the word `and`.
            let on_and = rows[0].find("and").unwrap();
            assert_eq!(
                box_target(&rows, &[], 0, on_and, 80, fake),
                file("a.rs", 0, 3)
            );
        }

        #[test]
        fn a_link_under_the_cursor_wins_over_the_text() {
            let rows = rows(&["see the helper here.rs:3"]);
            let links = [CommentLink {
                start: 4,
                end: 14,
                dest: "/abs/foo.py#L12".into(),
            }];
            assert_eq!(
                box_target(&rows, &links, 0, 6, 80, fake),
                file("/abs/foo.py", 11, 0)
            );
            // Off the link, the text is searched as usual.
            assert_eq!(
                box_target(&rows, &links, 0, 20, 80, fake),
                file("here.rs", 2, 0)
            );
        }

        #[test]
        fn a_path_broken_across_full_rows_is_joined() {
            let rows = rows(&["see /very/long/pa", "th/file.rs:3 ok"]);
            assert_eq!(
                box_target(&rows, &[], 0, 8, 17, fake),
                file("/very/long/path/file.rs", 2, 0)
            );
            // A row that stops short was wrapped at a space, so it is not joined.
            assert_eq!(box_target(&rows, &[], 0, 8, 40, fake), None);
        }

        #[test]
        fn prose_punctuation_is_not_part_of_the_path() {
            let rows = rows(&["open foo.rs, then bar.rs."]);
            assert_eq!(box_target(&rows, &[], 0, 6, 80, fake), file("foo.rs", 0, 0));
            let on_bar = rows[0].find("bar").unwrap();
            assert_eq!(
                box_target(&rows, &[], 0, on_bar, 80, fake),
                file("bar.rs", 0, 0)
            );
        }

        #[test]
        fn a_relative_path_resolves_against_the_reviewed_files_directory() {
            let dir = tempfile::tempdir().unwrap();
            let dir = helix_stdx::path::canonicalize(dir.path());
            std::fs::write(dir.join("near.py"), "x\ny\n").unwrap();

            assert_eq!(
                resolve_target("near.py:2", Some(&dir)),
                Some(BoxTarget::File(dir.join("near.py"), Position::new(1, 0)))
            );
            assert_eq!(resolve_target("missing.py:2", Some(&dir)), None);
            assert_eq!(
                resolve_target("near.py", Some(Path::new("/nonexistent"))),
                None
            );
        }
    }
}
