use helix_term::application::Application;
use helix_view::editor::{CompletionDisplay, StatusLineElement};

use super::*;

const COMPLETION_PREFIX: &str = "status";
const COMPLETION_LABELS: [&str; 3] = ["statusalpha", "statusbeta", "statusgamma"];
const OFFSCREEN_BLANK_LINES: usize = 220;

#[cfg(not(windows))]
fn left_click_events(row: u16, column: u16) -> [termina::event::Event; 2] {
    use termina::event::{Event, Modifiers, MouseButton, MouseEvent, MouseEventKind};

    [
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: Modifiers::NONE,
        }),
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column,
            row,
            modifiers: Modifiers::NONE,
        }),
    ]
}

#[cfg(windows)]
fn left_click_events(row: u16, column: u16) -> [crossterm::event::Event; 2] {
    use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    [
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::empty(),
        }),
        Event::Mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::empty(),
        }),
    ]
}

fn statusline_completion_config() -> Config {
    let mut config = test_config();
    config.editor.completion_display = CompletionDisplay::Statusline;
    config.editor.statusline.left = vec![
        StatusLineElement::Mode,
        StatusLineElement::FileName,
        StatusLineElement::CompletionSuggestions,
    ];
    config.editor.statusline.center = vec![];
    config.editor.statusline.right = vec![StatusLineElement::Position];
    config
}

const DIAGNOSTIC_WORD: &str = "statux";

fn completion_source_text() -> String {
    let mut text = String::from("\n");

    for _ in 0..OFFSCREEN_BLANK_LINES {
        text.push('\n');
    }

    for label in COMPLETION_LABELS {
        text.push_str(label);
        text.push('\n');
    }

    text
}

fn diagnostic_source_text() -> String {
    let mut text = String::from(DIAGNOSTIC_WORD);
    text.push('\n');

    for _ in 0..OFFSCREEN_BLANK_LINES {
        text.push('\n');
    }

    for label in COMPLETION_LABELS {
        text.push_str(label);
        text.push('\n');
    }

    text
}

fn insert_error_diagnostic(app: &mut Application, start: usize, end: usize, line: usize) {
    use helix_core::diagnostic::{
        Diagnostic, DiagnosticProvider, LanguageServerId, Range, Severity,
    };

    let (_, doc) = helix_view::current!(app.editor);
    doc.replace_diagnostics(
        [Diagnostic {
            range: Range { start, end },
            ends_at_word: true,
            starts_at_word: true,
            zero_width: false,
            line,
            message: "undefined".into(),
            severity: Some(Severity::Error),
            code: None,
            provider: DiagnosticProvider::Lsp {
                server_id: LanguageServerId::default(),
                identifier: None,
            },
            tags: Vec::new(),
            source: None,
            data: None,
        }],
        &[],
        None,
    );
}

fn click_screen_text(app: &Application, needle: &str, char_offset: usize) -> [u16; 2] {
    let lines = screen_lines(app);
    let (row, line) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains(needle))
        .unwrap_or_else(|| panic!("expected {needle:?} to be visible"));
    let column = line.find(needle).expect("expected needle column") + char_offset;
    [row as u16, column as u16]
}

#[tokio::test(flavor = "multi_thread")]
async fn statusline_completion_renders_without_popup() -> anyhow::Result<()> {
    let file = temp_file_with_contents(completion_source_text())?;
    let file_name = file
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let expected_position = format!("1:{}", COMPLETION_PREFIX.chars().count() + 1);
    let mut app = AppBuilder::new()
        .with_config(statusline_completion_config())
        .with_file(file.path(), None)
        .build()?;

    run_event_loop_until_idle(&mut app).await;

    test_key_sequence(
        &mut app,
        Some(&format!("i{COMPLETION_PREFIX}<C-x>")),
        Some(&|app| {
            let lines = screen_lines(app);
            let statusline = lines
                .iter()
                .rev()
                .find(|line| !line.is_empty())
                .expect("expected a rendered statusline row");

            for label in COMPLETION_LABELS {
                assert!(
                    statusline.contains(label),
                    "statusline row {statusline:?} did not contain completion label {label:?}"
                );
                assert_eq!(
                    1,
                    count_screen_occurrences(app, label),
                    "expected completion label {label:?} to appear exactly once in the rendered screen"
                );
            }

            assert!(
                !statusline.contains(&file_name),
                "statusline row {statusline:?} still showed file name {file_name:?} while completion was active"
            );
            assert!(
                !statusline.contains(&expected_position),
                "statusline row {statusline:?} still showed position {expected_position:?} while completion was active"
            );
        }),
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn statusline_completion_accepts_mouse_click() -> anyhow::Result<()> {
    let file = temp_file_with_contents(completion_source_text())?;
    let mut app = AppBuilder::new()
        .with_config(statusline_completion_config())
        .with_file(file.path(), None)
        .build()?;

    run_event_loop_until_idle(&mut app).await;
    dispatch_key_sequence(&mut app, &format!("i{COMPLETION_PREFIX}<C-x>")).await?;

    let lines = screen_lines(&app);
    let (row, statusline) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains(COMPLETION_LABELS[1]))
        .expect("expected statusline completion label to render");
    let column = statusline
        .find(COMPLETION_LABELS[1])
        .expect("expected label column") as u16;

    dispatch_events(&mut app, left_click_events(row as u16, column)).await?;

    let (_, doc) = helix_view::current_ref!(app.editor);
    assert_eq!(
        format!("{}\n", COMPLETION_LABELS[1]),
        doc.text().line(0).to_string()
    );

    test_key_sequence(&mut app, Some("<esc>:q!<ret>"), None, true).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn statusline_completion_accepts_mouse_click_on_padding_row() -> anyhow::Result<()> {
    let file = temp_file_with_contents(completion_source_text())?;
    let mut app = AppBuilder::new()
        .with_config(statusline_completion_config())
        .with_file(file.path(), None)
        .build()?;

    run_event_loop_until_idle(&mut app).await;
    dispatch_key_sequence(&mut app, &format!("i{COMPLETION_PREFIX}<C-x>")).await?;

    let lines = screen_lines(&app);
    let (row, statusline) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains(COMPLETION_LABELS[1]))
        .expect("expected statusline completion label to render");
    assert!(row > 0, "expected a padding row above the completion label");
    let column = statusline
        .find(COMPLETION_LABELS[1])
        .expect("expected label column") as u16;

    dispatch_events(&mut app, left_click_events((row - 1) as u16, column)).await?;

    let (_, doc) = helix_view::current_ref!(app.editor);
    assert_eq!(
        format!("{}\n", COMPLETION_LABELS[1]),
        doc.text().line(0).to_string()
    );

    test_key_sequence(&mut app, Some("<esc>:q!<ret>"), None, true).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn popup_completion_accepts_mouse_click() -> anyhow::Result<()> {
    let file = temp_file_with_contents(completion_source_text())?;
    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;

    run_event_loop_until_idle(&mut app).await;
    dispatch_key_sequence(&mut app, &format!("i{COMPLETION_PREFIX}<C-x>")).await?;

    let lines = screen_lines(&app);
    let (row, line) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.contains(COMPLETION_LABELS[1]))
        .expect("expected popup completion label to render");
    let column = line
        .find(COMPLETION_LABELS[1])
        .expect("expected label column") as u16;

    dispatch_events(&mut app, left_click_events(row as u16, column)).await?;

    let (_, doc) = helix_view::current_ref!(app.editor);
    assert_eq!(
        format!("{}\n", COMPLETION_LABELS[1]),
        doc.text().line(0).to_string()
    );

    test_key_sequence(&mut app, Some("<esc>:q!<ret>"), None, true).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn popup_completion_closes_on_buffer_click() -> anyhow::Result<()> {
    let file = temp_file_with_contents(completion_source_text())?;
    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;

    run_event_loop_until_idle(&mut app).await;
    dispatch_key_sequence(&mut app, &format!("i{COMPLETION_PREFIX}<C-x>")).await?;
    assert_eq!(1, count_screen_occurrences(&app, COMPLETION_LABELS[0]));

    let lines = screen_lines(&app);
    let row = lines
        .iter()
        .enumerate()
        .find(|(row, line)| *row > 5 && line.is_empty())
        .map(|(row, _)| row)
        .expect("expected a blank buffer row outside the popup");

    dispatch_events(&mut app, left_click_events(row as u16, 0)).await?;

    for label in COMPLETION_LABELS {
        assert_eq!(0, count_screen_occurrences(&app, label));
    }

    test_key_sequence(&mut app, Some("<esc>:q!<ret>"), None, true).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn statusline_completion_opens_on_diagnostic_click_in_insert_mode() -> anyhow::Result<()> {
    let file = temp_file_with_contents(diagnostic_source_text())?;
    let mut config = statusline_completion_config();
    config.editor.auto_completion = false;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(file.path(), None)
        .build()?;

    run_event_loop_until_idle(&mut app).await;
    insert_error_diagnostic(&mut app, 0, DIAGNOSTIC_WORD.chars().count(), 0);
    dispatch_key_sequence(&mut app, "i").await?;

    for label in COMPLETION_LABELS {
        assert_eq!(
            0,
            count_screen_occurrences(&app, label),
            "completion should not open before clicking the diagnostic"
        );
    }

    // Click a prefix of the underlined word so word completion has a filter.
    let click_offset = 3;
    let [row, column] = click_screen_text(&app, DIAGNOSTIC_WORD, click_offset);
    dispatch_events(&mut app, left_click_events(row, column)).await?;

    let (view, doc) = helix_view::current_ref!(app.editor);
    let cursor = doc
        .selection(view.id)
        .primary()
        .cursor(doc.text().slice(..));
    assert_eq!(
        click_offset, cursor,
        "clicking the underline should move the insert cursor onto that character"
    );
    assert_eq!(helix_view::document::Mode::Insert, app.editor.mode);

    for label in COMPLETION_LABELS {
        assert!(
            count_screen_occurrences(&app, label) >= 1,
            "expected statusline completion label {label:?} after clicking the diagnostic"
        );
    }

    test_key_sequence(&mut app, Some("<esc>:q!<ret>"), None, true).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn statusline_completion_does_not_open_on_diagnostic_click_in_normal_mode(
) -> anyhow::Result<()> {
    let file = temp_file_with_contents(diagnostic_source_text())?;
    let mut config = statusline_completion_config();
    config.editor.auto_completion = false;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(file.path(), None)
        .build()?;

    run_event_loop_until_idle(&mut app).await;
    insert_error_diagnostic(&mut app, 0, DIAGNOSTIC_WORD.chars().count(), 0);
    // Enter and leave insert so the buffer is drawn before we sample click coords.
    dispatch_key_sequence(&mut app, "i<esc>").await?;

    let [row, column] = click_screen_text(&app, DIAGNOSTIC_WORD, 3);
    dispatch_events(&mut app, left_click_events(row, column)).await?;

    assert_eq!(helix_view::document::Mode::Normal, app.editor.mode);
    for label in COMPLETION_LABELS {
        assert_eq!(
            0,
            count_screen_occurrences(&app, label),
            "normal-mode diagnostic clicks should not open statusline completions"
        );
    }

    test_key_sequence(&mut app, Some("<esc>:q!<ret>"), None, true).await?;

    Ok(())
}
