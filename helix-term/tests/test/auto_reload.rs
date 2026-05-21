use std::{
    io::{Seek, SeekFrom, Write},
    time::Duration,
};

use anyhow::bail;
use helix_core::{diagnostic::Severity, Transaction};
use helix_term::{application::Application, config::Config};
use helix_view::{current, current_ref, doc};
use tempfile::NamedTempFile;

use super::*;

fn config_with_auto_reload(auto_reload: bool) -> Config {
    let mut config = test_config();
    config.editor.auto_reload = auto_reload;
    config
}

fn current_text(app: &Application) -> String {
    doc!(app.editor).text().to_string()
}

fn overwrite_file(file: &mut NamedTempFile, content: &str) -> anyhow::Result<()> {
    let file = file.as_file_mut();
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(content.as_bytes())?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn overwrite_until_newer(
    app: &Application,
    file: &mut NamedTempFile,
    content: &str,
) -> anyhow::Result<()> {
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(20));
        overwrite_file(file, content)?;

        let (_, doc) = current_ref!(app.editor);
        if doc.has_newer_file_on_disk()? {
            return Ok(());
        }
    }

    bail!("external file write did not produce a newer mtime")
}

async fn close_app(app: &mut Application) -> anyhow::Result<()> {
    let errs = app.close().await;
    if !errs.is_empty() {
        bail!("error closing app: {errs:?}");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_reload_enabled_reloads_current_buffer_on_idle() -> anyhow::Result<()> {
    let mut file = temp_file_with_contents("before\n")?;
    let mut app = AppBuilder::new()
        .with_config(config_with_auto_reload(true))
        .with_file(file.path(), None)
        .build()?;

    overwrite_until_newer(&app, &mut file, "after\n")?;
    app.handle_idle_timeout().await;

    assert_eq!("after\n", current_text(&app));
    assert!(!app.editor.is_err(), "error: {:?}", app.editor.get_status());

    close_app(&mut app).await
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_reload_disabled_keeps_current_buffer_on_idle() -> anyhow::Result<()> {
    let mut file = temp_file_with_contents("before\n")?;
    let mut app = AppBuilder::new()
        .with_config(config_with_auto_reload(false))
        .with_file(file.path(), None)
        .build()?;

    overwrite_until_newer(&app, &mut file, "after\n")?;
    app.handle_idle_timeout().await;

    assert_eq!("before\n", current_text(&app));

    close_app(&mut app).await
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_reload_warns_and_keeps_unsaved_buffer() -> anyhow::Result<()> {
    let mut file = temp_file_with_contents("before\n")?;
    let mut app = AppBuilder::new()
        .with_config(config_with_auto_reload(true))
        .with_file(file.path(), None)
        .build()?;

    {
        let (view, doc) = current!(app.editor);
        let selection = doc.selection(view.id).clone();
        let transaction = Transaction::change_by_selection(doc.text(), &selection, |_| {
            (0, 0, Some("local ".into()))
        });
        doc.apply(&transaction, view.id);
    }

    overwrite_until_newer(&app, &mut file, "after\n")?;
    app.handle_idle_timeout().await;

    assert_eq!("local before\n", current_text(&app));
    let (status, severity) = app.editor.get_status().unwrap();
    assert_eq!(&Severity::Warning, severity);
    assert!(status.contains(":reload"));

    close_app(&mut app).await
}

#[tokio::test(flavor = "multi_thread")]
async fn auto_reload_ignores_nonexistent_mapped_path() -> anyhow::Result<()> {
    let temp_dir = tempfile::tempdir()?;
    let path = temp_dir.path().join("missing.md");
    assert!(!path.exists());

    let mut app = AppBuilder::new()
        .with_config(config_with_auto_reload(true))
        .with_file(&path, None)
        .build()?;

    app.handle_idle_timeout().await;

    assert!(!app.editor.is_err(), "error: {:?}", app.editor.get_status());

    close_app(&mut app).await
}
