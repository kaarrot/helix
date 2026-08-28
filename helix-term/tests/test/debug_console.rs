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
