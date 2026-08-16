//! Rendering of files with lines wider than the viewport, with soft wrap off.
//!
//! With soft wrap disabled the renderer discards the off-screen tail of a line
//! by skipping it rather than formatting and throwing away every grapheme. What
//! ends up on screen must be byte for byte what the unclipped renderer drew, so
//! these tests assert on the rendered surface rather than on editor state.

use std::io::Write;

use helix_term::application::Application;
use helix_term::config::Config;
use helix_view::current_ref;
use tempfile::NamedTempFile;

use super::*;

/// The document rows of the rendered surface, trailing blank rows dropped.
///
/// Gutters are disabled in these tests so a row is the document text itself.
fn document_rows(app: &Application) -> Vec<String> {
    let mut rows = app.rendered_text();
    // drop the statusline first, then the blank filler above it
    rows.pop();
    while rows.last().is_some_and(|row| row.is_empty()) {
        rows.pop();
    }
    rows
}

async fn render_file(contents: &str, keys: &str) -> anyhow::Result<Vec<String>> {
    let mut file = NamedTempFile::new()?;
    write!(file, "{contents}")?;
    file.flush()?;

    // no gutters, so the rendered row is exactly the document text
    let mut editor = test_editor_config();
    editor.gutters = helix_view::editor::GutterConfig {
        layout: Vec::new(),
        ..Default::default()
    };
    let config = Config {
        editor,
        ..test_config()
    };

    let mut app = AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;

    let rendered = std::cell::RefCell::new(Vec::new());
    test_key_sequence(
        &mut app,
        Some(keys),
        Some(&|app: &Application| {
            *rendered.borrow_mut() = document_rows(app);
        }),
        false,
    )
    .await?;

    Ok(rendered.into_inner())
}

/// The viewport is 120 columns wide. A 400 char line must render its first
/// screenful and nothing beyond it, and the lines after it must still be there
/// -- a skip that overshoots would swallow them.
#[tokio::test(flavor = "multi_thread")]
async fn long_lines_render_visible_prefix() -> anyhow::Result<()> {
    let long: String = ('a'..='z').cycle().take(400).collect();
    let contents = format!("{long}\nsecond line\n{long}\nfourth line\n");

    let rows = render_file(&contents, "<esc>").await?;

    // every document line is present, in order
    assert!(
        rows.iter().any(|row| row.contains("second line")),
        "line after a long line went missing: {rows:#?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("fourth line")),
        "line after the second long line went missing: {rows:#?}"
    );

    // the long line is truncated to the viewport, not wrapped onto extra rows
    let long_rows = rows.iter().filter(|row| row.starts_with("abcdefg")).count();
    assert_eq!(
        long_rows, 2,
        "expected exactly two truncated long lines, got {long_rows}: {rows:#?}"
    );

    // and it renders the true prefix of the line
    let first_long = rows.iter().find(|row| row.starts_with("abcdefg")).unwrap();
    assert!(
        long.starts_with(first_long.as_str()),
        "rendered text is not a prefix of the document line:\n  rendered {first_long:?}"
    );

    Ok(())
}

/// A line whose width lands exactly on the viewport edge is the case where the
/// line break itself is the first grapheme at or past the clip column. Skipping
/// there must not consume the following line.
#[tokio::test(flavor = "multi_thread")]
async fn line_ending_exactly_at_viewport_edge() -> anyhow::Result<()> {
    for width in 118..=122 {
        let line: String = "x".repeat(width);
        let contents = format!("{line}\nSENTINEL\nlast\n");
        let rows = render_file(&contents, "<esc>").await?;
        assert!(
            rows.iter().any(|row| row.contains("SENTINEL")),
            "line after a {width}-char line went missing: {rows:#?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("last")),
            "last line went missing for width {width}: {rows:#?}"
        );
    }
    Ok(())
}

/// Horizontal scrolling still shows the right window into the line.
#[tokio::test(flavor = "multi_thread")]
async fn horizontally_scrolled_long_line() -> anyhow::Result<()> {
    let long: String = ('a'..='z').cycle().take(400).collect();
    let contents = format!("{long}\ntrailer\n");

    // move to the end of the line, forcing the view to scroll right
    let rows = render_file(&contents, "gl").await?;

    let scrolled = rows.first().expect("expected a rendered long line");
    assert!(
        !scrolled.is_empty(),
        "scrolled long line rendered blank: {rows:#?}"
    );
    assert!(
        long.contains(scrolled.as_str()),
        "horizontally scrolled text is not a substring of the document line:\n  rendered {scrolled:?}"
    );
    // the window has to be the *tail* of the line, since the cursor is at its end
    assert!(
        long.ends_with(scrolled.as_str()),
        "expected the end of the line to be in view:\n  rendered {scrolled:?}"
    );
    // and the short line is legitimately blank at this horizontal offset
    assert!(
        rows.get(1).is_none_or(|row| row.is_empty()),
        "short line should have nothing in view when scrolled right: {rows:#?}"
    );

    Ok(())
}

/// Not an assertion, a stopwatch for the whole render path. Run with:
///
/// ```text
/// cargo test --features integration --release --test integration -- \
///     --ignored --nocapture bench_scroll_long_lines
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "timing measurement, not a correctness test"]
async fn bench_scroll_long_lines() -> anyhow::Result<()> {
    use std::time::Instant;

    let line: String = ('a'..='z').cycle().take(2000).collect();
    let contents: String = std::iter::repeat(line.as_str())
        .take(5000)
        .map(|l| format!("{l}\n"))
        .collect();

    let mut file = NamedTempFile::new()?;
    write!(file, "{contents}")?;
    file.flush()?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;

    let scrolls = 200;
    let keys = "<C-d>".repeat(scrolls);
    let start = Instant::now();
    test_key_sequence(&mut app, Some(&keys), None, false).await?;
    let elapsed = start.elapsed();

    println!(
        "{scrolls} x Ctrl-d over 5000 lines of 2000 chars: {:?} total, {:?}/scroll",
        elapsed,
        elapsed / scrolls as u32
    );

    Ok(())
}

/// Tabs and double-width graphemes straddling the clip column must not shift
/// what is drawn.
#[tokio::test(flavor = "multi_thread")]
async fn wide_graphemes_at_clip_column() -> anyhow::Result<()> {
    let cjk: String = "\u{4f60}\u{597d}".repeat(200);
    let contents = format!("{cjk}\nSENTINEL\n\t\t{}\nlast\n", "y".repeat(300));

    let rows = render_file(&contents, "<esc>").await?;

    assert!(
        rows.iter().any(|row| row.contains("SENTINEL")),
        "line after a CJK line went missing: {rows:#?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("last")),
        "line after a tab indented long line went missing: {rows:#?}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Soft wrap
//
// A soft wrapped line longer than `BLOCK_CHARS` is divided into blocks so that
// the formatter can start near any position rather than walking there from the
// line start. Each block boundary is a forced visual line break, so the risk is
// that motion and rendering disagree about where a row begins -- which shows up
// as the cursor drifting away from the character it is drawn on.
// ---------------------------------------------------------------------------

/// What a soft wrap test needs to see, all captured *inside* the render
/// callback: after `test_key_sequence` returns the view tree is gone.
struct SoftWrapped {
    rows: Vec<String>,
    /// char index of the primary cursor
    cursor_char: usize,
    /// where the renderer actually drew that cursor, if on screen
    cursor_screen: Option<helix_core::Position>,
}

async fn render_soft_wrapped(contents: &str, keys: &str) -> anyhow::Result<SoftWrapped> {
    let mut file = NamedTempFile::new()?;
    write!(file, "{contents}")?;
    file.flush()?;

    let mut editor = test_editor_config();
    editor.gutters = helix_view::editor::GutterConfig {
        layout: Vec::new(),
        ..Default::default()
    };
    editor.soft_wrap.enable = Some(true);
    let config = Config {
        editor,
        ..test_config()
    };

    let mut app = AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;

    let captured = std::cell::RefCell::new(None);
    test_key_sequence(
        &mut app,
        Some(keys),
        Some(&|app: &Application| {
            let (view, doc) = current_ref!(app.editor);
            *captured.borrow_mut() = Some(SoftWrapped {
                rows: document_rows(app),
                cursor_char: doc
                    .selection(view.id)
                    .primary()
                    .cursor(doc.text().slice(..)),
                cursor_screen: app.editor.cursor().0,
            });
        }),
        false,
    )
    .await?;

    captured
        .into_inner()
        .ok_or_else(|| anyhow::anyhow!("render callback never ran"))
}

/// A line spanning many blocks must still wrap onto the viewport and show the
/// document text, and every rendered row must fit the viewport.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrapped_multi_block_line_renders() -> anyhow::Result<()> {
    // comfortably more than BLOCK_CHARS (4096) so several boundaries are crossed
    let contents = format!("{}\n", "lorem ipsum dolor sit amet ".repeat(1000));

    let rendered = render_soft_wrapped(&contents, "<esc>").await?;
    let rows = rendered.rows;

    assert!(
        rows.len() > 1,
        "a soft wrapped long line should occupy several rows: {rows:#?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("lorem ipsum")),
        "document text missing from a soft wrapped long line: {rows:#?}"
    );
    // no row may exceed the viewport
    for (i, row) in rows.iter().enumerate() {
        assert!(
            row.chars().count() <= 120,
            "row {i} is wider than the viewport: {row:?}"
        );
    }

    Ok(())
}

/// The load bearing one. Deep inside a multi-block line the cursor must remain
/// exactly where the renderer draws it -- a boundary that motion and rendering
/// disagree about shows up here and nowhere else.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrapped_cursor_agrees_with_render() -> anyhow::Result<()> {
    // a one char sentinel, well past several block boundaries and unique in the
    // document. One char so the search leaves the cursor *on* it -- a search
    // selects the whole match and puts the cursor at its last char.
    let filler = "lorem ipsum dolor sit amet ".repeat(1000);
    let contents = format!("{filler}Z{filler}\n");

    let rendered = render_soft_wrapped(&contents, "<esc>/Z<ret>").await?;

    assert_eq!(
        rendered.cursor_char,
        filler.chars().count(),
        "search did not land on the sentinel"
    );

    let screen = rendered
        .cursor_screen
        .expect("cursor should be on screen after a search");
    let row = rendered.rows.get(screen.row).unwrap_or_else(|| {
        panic!(
            "cursor row {} not rendered: {:#?}",
            screen.row, rendered.rows
        )
    });
    assert!(
        row.chars().nth(screen.col).is_some_and(|c| c == 'Z'),
        "cursor at ({}, {}) is not on the sentinel it selected; row is {row:?}",
        screen.row,
        screen.col
    );

    Ok(())
}

/// `j` then `k` deep inside a multi-block line must return to the same char,
/// including when the pair straddles a forced block break.
#[tokio::test(flavor = "multi_thread")]
async fn soft_wrapped_vertical_motion_round_trips() -> anyhow::Result<()> {
    let filler = "lorem ipsum dolor sit amet ".repeat(1000);
    let contents = format!("{filler}Z{filler}\n");

    let before = render_soft_wrapped(&contents, "<esc>/Z<ret>").await?;
    let after = render_soft_wrapped(&contents, "<esc>/Z<ret>jjjkkk").await?;

    assert_eq!(
        after.cursor_char, before.cursor_char,
        "jjjkkk did not return to the starting char"
    );

    Ok(())
}

/// Not an assertion, a stopwatch for soft wrapped vertical motion through the
/// whole editor, render included. Run with:
///
/// ```text
/// cargo test --features integration --release --test integration -- \
///     --ignored --nocapture bench_soft_wrapped
/// ```
#[tokio::test(flavor = "multi_thread")]
#[ignore = "timing measurement, not a correctness test"]
async fn bench_soft_wrapped_vertical_motion() -> anyhow::Result<()> {
    use std::cell::Cell;
    use std::time::Instant;

    let contents = format!("{}\n", "lorem ipsum dolor sit amet ".repeat(80_000));

    let mut file = NamedTempFile::new()?;
    write!(file, "{contents}")?;
    file.flush()?;

    let mut editor = test_editor_config();
    editor.soft_wrap.enable = Some(true);
    let config = Config {
        editor,
        ..test_config()
    };
    let mut app = AppBuilder::new()
        .with_file(file.path(), None)
        .with_config(config)
        .build()?;

    let presses = 100usize;
    let motion = "jk".repeat(presses / 2);

    // The harness closes the app after one sequence, so setup and the timed run
    // have to be two entries of the same sequence rather than two calls.
    let started = Cell::new(None);
    let elapsed = Cell::new(None);
    test_key_sequences(
        &mut app,
        vec![
            // park the cursor deep inside the line, then start the clock
            (
                Some("<esc>1000000l"),
                Some(&|_: &Application| started.set(Some(Instant::now()))),
            ),
            (
                Some(motion.as_str()),
                Some(&|_: &Application| elapsed.set(started.get().map(|start| start.elapsed()))),
            ),
        ],
        false,
    )
    .await?;

    let elapsed = elapsed.get().expect("timed section never ran");
    println!(
        "{presses} x j/k inside a {}-char soft wrapped line: {:?} total, {:?}/keypress",
        contents.len(),
        elapsed,
        elapsed / presses as u32
    );

    Ok(())
}
