use helix_view::editor::{CompletionDisplay, StatusLineElement};

use super::*;

const COMPLETION_PREFIX: &str = "status";
const COMPLETION_LABELS: [&str; 3] = ["statusalpha", "statusbeta", "statusgamma"];
const OFFSCREEN_BLANK_LINES: usize = 220;

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
