use super::*;

use helix_core::Selection;
use helix_term::application::Application;
use helix_view::current_ref;

/// `:debug-console` opens a scratch buffer and remembers it, so asking again
/// returns to the same transcript rather than piling up splits.
#[tokio::test(flavor = "multi_thread")]
async fn opens_once_and_is_reused() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;

    send_keys(&mut app, ":debug-console<ret>").await?;

    let console = app.editor.dap_console.expect("console was not registered");
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.id(), console, "the console should be focused");
    // The input point sits directly after the prompt, not on the line below:
    // a new file already holds a line ending, and seeding around that is easy
    // to get wrong.
    assert_eq!(doc.text().to_string(), ">>> ");
    assert_eq!(doc.selection(view.id).primary().head, 4);
    // Never dirty -- the transcript must not stand between the user and `:q`.
    assert!(!doc.is_modified());
    assert_eq!(app.editor.tree.views().count(), 2);

    // Move away, then ask again: same document, same split.
    send_keys(&mut app, "<C-w>w:debug-console<ret>").await?;

    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.id(), console);
    assert_eq!(
        app.editor.tree.views().count(),
        2,
        "reopening must not add another split"
    );

    Ok(())
}

/// Adapter output appends to the transcript instead of overwriting the status
/// line, and only follows the end while the cursor is already there.
#[tokio::test(flavor = "multi_thread")]
async fn output_appends_and_follows_only_at_the_end() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;

    send_keys(&mut app, ":debug-console<ret>").await?;

    let console = app.editor.dap_console.unwrap();
    let view_id = app.editor.tree.focus;

    // With the cursor at the end, the transcript scrolls along.
    assert!(app.editor.dap_console_append("first\n"));
    assert!(app.editor.dap_console_append("second\n"));

    let doc = app.editor.document(console).unwrap();
    assert_eq!(doc.text().to_string(), ">>> first\nsecond\n");
    assert_eq!(
        doc.selection(view_id).primary().head,
        doc.text().len_chars(),
        "the cursor should follow output it was already tracking"
    );
    assert!(!doc.is_modified());

    // Reading back through the scrollback pins the cursor: output keeps
    // arriving underneath without dragging the view along. Compare anchors --
    // `set_selection` widens a bare point into the 1-wide range a normal-mode
    // cursor is, so the head sits one past where the cursor was put.
    let doc = app.editor.document_mut(console).unwrap();
    doc.set_selection(view_id, Selection::point(2));
    assert!(app.editor.dap_console_append("third\n"));

    let doc = app.editor.document(console).unwrap();
    assert_eq!(doc.text().to_string(), ">>> first\nsecond\nthird\n");
    assert_eq!(
        doc.selection(view_id).primary().anchor,
        2,
        "output must not drag a cursor that was reading scrollback"
    );
    assert!(!doc.is_modified());

    Ok(())
}

/// The console survives its split being closed: output still collects, so
/// reopening shows what was missed rather than starting blank.
#[tokio::test(flavor = "multi_thread")]
async fn survives_its_split_being_closed() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;

    send_keys(&mut app, ":debug-console<ret>").await?;
    let console = app.editor.dap_console.unwrap();

    send_keys(&mut app, ":q<ret>").await?;
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_eq!(
        app.editor.dap_console,
        Some(console),
        "closing the split should not forget the transcript"
    );

    assert!(app.editor.dap_console_append("while hidden\n"));

    send_keys(&mut app, ":debug-console<ret>").await?;
    let (_, doc) = current_ref!(app.editor);
    assert_eq!(doc.id(), console);
    assert_eq!(doc.text().to_string(), ">>> while hidden\n");

    Ok(())
}

/// With no console open there is nothing to append to, and the caller is told
/// so it can fall back to the status line.
#[tokio::test(flavor = "multi_thread")]
async fn append_reports_when_there_is_no_console() -> anyhow::Result<()> {
    let app = AppBuilder::new().build()?;
    let mut app: Application = app;
    assert!(!app.editor.dap_console_append("ignored\n"));
    assert!(app.editor.dap_console.is_none());
    Ok(())
}

/// Output belongs to the evaluation still running, so it is inserted above the
/// marker standing in for that evaluation's result -- which is what keeps a
/// loop's prints in front of the value the loop returned.
#[tokio::test(flavor = "multi_thread")]
async fn output_lands_above_a_pending_marker() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    send_keys(&mut app, ":debug-console<ret>").await?;
    let console = app.editor.dap_console.unwrap();

    app.editor.dap_console_push("for i in items:\n");
    let marker = format!("\u{22ef} running #{}", app.editor.dap_console_next_id());
    app.editor.dap_console_push(&format!("{}\n", marker));

    // Two prints arrive while the evaluation is outstanding.
    assert!(app.editor.dap_console_append("2\n"));
    assert!(app.editor.dap_console_append("4\n"));

    let doc = app.editor.document(console).unwrap();
    assert_eq!(
        doc.text().to_string(),
        ">>> for i in items:\n2\n4\n\u{22ef} running #1\n",
        "output should collect above the marker, not below it"
    );

    // The result replaces the marker, and a fresh prompt follows.
    assert!(app.editor.dap_console_resolve(&marker, ""));
    assert!(app.editor.dap_console_push(">>> "));

    let doc = app.editor.document(console).unwrap();
    assert_eq!(doc.text().to_string(), ">>> for i in items:\n2\n4\n>>> ");
    assert!(!doc.is_modified());

    Ok(())
}

/// Two evaluations in flight each get their own marker, so results land where
/// they belong even when they come back out of order.
#[tokio::test(flavor = "multi_thread")]
async fn results_land_at_their_own_marker() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    send_keys(&mut app, ":debug-console<ret>").await?;
    let console = app.editor.dap_console.unwrap();

    let first = format!("\u{22ef} running #{}", app.editor.dap_console_next_id());
    app.editor.dap_console_push(&format!("slow\n{}\n", first));
    let second = format!("\u{22ef} running #{}", app.editor.dap_console_next_id());
    app.editor.dap_console_push(&format!(">>> quick\n{}\n", second));

    // The second finishes first.
    assert!(app.editor.dap_console_resolve(&second, "2\n"));
    assert!(app.editor.dap_console_resolve(&first, "1\n"));

    let doc = app.editor.document(console).unwrap();
    assert_eq!(doc.text().to_string(), ">>> slow\n1\n>>> quick\n2\n");

    Ok(())
}

/// A result whose marker the user deleted is appended rather than dropped.
#[tokio::test(flavor = "multi_thread")]
async fn a_lost_marker_does_not_lose_the_result() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    send_keys(&mut app, ":debug-console<ret>").await?;
    let console = app.editor.dap_console.unwrap();

    assert!(app
        .editor
        .dap_console_resolve("\u{22ef} running #7", "42\n"));

    let doc = app.editor.document(console).unwrap();
    assert_eq!(doc.text().to_string(), ">>> 42\n");

    Ok(())
}

/// `ret` is bound globally in normal mode, so it must do nothing at all unless
/// the console is the focused buffer.
#[tokio::test(flavor = "multi_thread")]
async fn submitting_outside_the_console_does_nothing() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    send_keys(&mut app, ":debug-console<ret>").await?;
    let console = app.editor.dap_console.unwrap();

    // Move to the other split and type into it, then press Enter.
    send_keys(&mut app, "<C-w>wihello<esc><ret>").await?;

    let (_, doc) = current_ref!(app.editor);
    assert_ne!(doc.id(), console);
    assert_eq!(doc.text().to_string(), "hello\n");

    let console_doc = app.editor.document(console).unwrap();
    assert_eq!(
        console_doc.text().to_string(),
        ">>> ",
        "the console must be untouched by Enter pressed elsewhere"
    );

    Ok(())
}

/// Accepting a completion replaces the qualifier the adapter reported and
/// leaves the cursor after the new text, ready to keep typing.
#[tokio::test(flavor = "multi_thread")]
async fn accepting_a_completion_replaces_the_qualifier() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    send_keys(&mut app, ":debug-console<ret>").await?;
    let console = app.editor.dap_console.unwrap();
    let view_id = app.editor.tree.focus;

    send_keys(&mut app, "iitems.ap<esc>").await?;
    assert_eq!(
        app.editor.document(console).unwrap().text().to_string(),
        ">>> items.ap"
    );

    // debugpy answers `items.ap` with start 6, length 2 -- offsets into the
    // submitted line, which is anchored just past the 4-character prompt.
    let anchor = 4;
    assert!(app
        .editor
        .dap_console_insert(anchor + 6, anchor + 6 + 2, "append"));

    let doc = app.editor.document(console).unwrap();
    assert_eq!(doc.text().to_string(), ">>> items.append");
    assert_eq!(
        doc.selection(view_id).primary().anchor,
        doc.text().len_chars(),
        "the cursor should sit after the completion"
    );
    assert!(!doc.is_modified());

    Ok(())
}

/// A stale range cannot panic the editor: the buffer may have moved on between
/// asking for completions and accepting one.
#[tokio::test(flavor = "multi_thread")]
async fn a_stale_completion_range_is_clamped() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    send_keys(&mut app, ":debug-console<ret>").await?;
    let console = app.editor.dap_console.unwrap();

    assert!(app.editor.dap_console_insert(999, 1200, "append"));
    assert_eq!(
        app.editor.document(console).unwrap().text().to_string(),
        ">>> append"
    );

    // A reversed range is refused rather than applied.
    assert!(!app.editor.dap_console_insert(8, 2, "nope"));

    Ok(())
}

/// The console is an ordinary buffer, so ordinary editing works in it:
/// selection, yank and paste.
#[tokio::test(flavor = "multi_thread")]
async fn supports_selection_yank_and_paste() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    send_keys(&mut app, ":debug-console<ret>").await?;
    let console = app.editor.dap_console.unwrap();

    send_keys(&mut app, "ipayload<esc>").await?;
    assert_eq!(
        app.editor.document(console).unwrap().text().to_string(),
        ">>> payload"
    );

    // Select the word with a textobject, yank it, and paste it back.
    send_keys(&mut app, "miwy").await?;
    send_keys(&mut app, "p").await?;

    let text = app.editor.document(console).unwrap().text().to_string();
    assert_eq!(
        text.matches("payload").count(),
        2,
        "yank and paste should duplicate the word, got {:?}",
        text
    );

    Ok(())
}

/// A bracketed paste -- what a terminal sends when you paste into it -- lands in
/// the console like any other buffer, newlines and indentation intact, and the
/// result is submittable as one block.
#[tokio::test(flavor = "multi_thread")]
async fn accepts_a_bracketed_paste() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    send_keys(&mut app, ":debug-console<ret>").await?;
    let console = app.editor.dap_console.unwrap();

    send_keys(&mut app, "i").await?;
    send_paste(&mut app, "for i in items:\n    print(i)").await?;

    assert_eq!(
        app.editor.document(console).unwrap().text().to_string(),
        ">>> for i in items:\n    print(i)",
        "a pasted block should arrive intact"
    );

    Ok(())
}
