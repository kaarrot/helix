use std::path::Path;

use helix_core::diagnostic::Severity;
use helix_stdx::path;
use helix_term::application::Application;

use super::*;

fn assert_status(app: &Application, expected: &str, severity: Severity) {
    let (status, actual_severity) = app.editor.get_status().unwrap();
    assert_eq!(status.as_ref(), expected);
    assert_eq!(*actual_severity, severity);
}

fn assert_current_doc_path(app: &Application, expected: &Path) {
    let view = app.editor.tree.get(app.editor.tree.focus);
    let doc = app.editor.document(view.doc).unwrap();
    assert_eq!(doc.path().unwrap(), &path::normalize(expected));
}

fn current_doc_text(app: &Application) -> String {
    let view = app.editor.tree.get(app.editor.tree.focus);
    let doc = app.editor.document(view.doc).unwrap();
    doc.text().to_string()
}

fn current_cursor_line(app: &Application) -> usize {
    let view = app.editor.tree.get(app.editor.tree.focus);
    let doc = app.editor.document(view.doc).unwrap();
    doc.selection(view.id)
        .primary()
        .cursor_line(doc.text().slice(..))
}

fn assert_diff_range(app: &Application, base_ref: &str, target_ref: Option<&str>) {
    let range = app
        .editor
        .diff
        .range
        .as_ref()
        .expect("diff range should be set");
    assert_eq!(range.base_ref, base_ref);
    assert_eq!(range.target_ref.as_deref(), target_ref);
}

fn assert_split_diff_cursor_line(app: &Application, expected_line: usize) {
    let view_id = app.editor.tree.focus;
    let diff_state = app.editor.diff.views.get(&view_id).unwrap().clone();

    let base_doc = app.editor.document(diff_state.base_doc_id).unwrap();
    let working_doc = app.editor.document(diff_state.working_doc_id).unwrap();

    let base_line = base_doc
        .selection(diff_state.base_view_id)
        .primary()
        .cursor_line(base_doc.text().slice(..));
    let working_line = working_doc
        .selection(diff_state.working_view_id)
        .primary()
        .cursor_line(working_doc.text().slice(..));

    assert_eq!(base_line, expected_line);
    assert_eq!(working_line, expected_line);
}

fn main_diff_state(app: &Application) -> helix_view::diff_view::DiffViewState {
    let view_id = app.editor.tree.focus;
    app.editor.diff.views.get(&view_id).unwrap().clone()
}

#[tokio::test(flavor = "multi_thread")]
async fn repo_backed_diff_base_toggle_and_reset() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "version one\n")?;
    repo.commit_all("initial")?;
    repo.write_file("tracked.txt", "version two\n")?;
    repo.commit_all("second")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("tracked.txt"), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":diff-base HEAD^<ret>"),
                Some(&|app| {
                    assert_status(app, "Diff base set to: HEAD^", Severity::Info);
                    let view = app.editor.tree.get(app.editor.tree.focus);
                    let doc = app.editor.document(view.doc).unwrap();
                    assert_eq!(doc.diff_base_ref(), Some("HEAD^"));
                }),
            ),
            (
                Some(":toggle-char-diff<ret>"),
                Some(&|app| {
                    assert_status(
                        app,
                        "Character-level diff highlighting enabled",
                        Severity::Info,
                    );
                    let view = app.editor.tree.get(app.editor.tree.focus);
                    let doc = app.editor.document(view.doc).unwrap();
                    assert!(doc.char_diff_enabled);
                }),
            ),
            (
                Some(":diff-reset<ret>"),
                Some(&|app| {
                    assert_status(app, "Diff reset to HEAD", Severity::Info);
                    let view = app.editor.tree.get(app.editor.tree.focus);
                    let doc = app.editor.document(view.doc).unwrap();
                    assert_eq!(doc.diff_base_ref(), None);
                    assert!(!doc.char_diff_enabled);
                    assert!(app.editor.diff.range.is_none());
                }),
            ),
        ],
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_commit_typable_sets_explicit_range() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\n")?;
    repo.commit_all("initial")?;
    let older = repo.rev_parse("HEAD")?;
    repo.write_file("tracked.txt", "two\n")?;
    repo.commit_all("second")?;
    let newer = repo.rev_parse("HEAD")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("tracked.txt"), None)
        .build()?;

    let command = format!(":diff-commit {older}..{newer}<ret>");
    let expected_status =
        format!("Diff range set to: {older}..{newer} — use space+g to browse files");

    test_key_sequence(
        &mut app,
        Some(&command),
        Some(&|app| {
            assert_status(app, &expected_status, Severity::Info);
            assert_diff_range(app, &older, Some(&newer));
        }),
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_mode_commit_commands_use_selected_git_log_lines() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("log.txt", "placeholder\n")?;
    repo.commit_all("initial")?;
    let oldest = repo.rev_parse_short("HEAD")?;
    repo.write_file("log.txt", "second\n")?;
    repo.commit_all("second")?;
    let newest = repo.rev_parse_short("HEAD")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;

    let multi_line_log = format!("#[{newest} newer commit\n{oldest} older commit|]#\n");
    let mut app = AppBuilder::new()
        .with_file(repo.file("log.txt"), None)
        .with_input_text(multi_line_log)
        .build()?;

    test_key_sequence(
        &mut app,
        Some("<space>mc"),
        Some(&|app| {
            assert_status(
                app,
                &format!("Diff set: {oldest}..{newest}"),
                Severity::Info,
            );
            assert_diff_range(app, &oldest, Some(&newest));
        }),
        false,
    )
    .await?;

    let single_line_log = format!("#[{newest} newest commit|]#\n");
    let mut app = AppBuilder::new()
        .with_file(repo.file("log.txt"), None)
        .with_input_text(single_line_log)
        .build()?;

    test_key_sequence(
        &mut app,
        Some("<space>mC"),
        Some(&|app| {
            assert_status(
                app,
                &format!("Diff set: showing changes in {newest}"),
                Severity::Info,
            );
            assert_diff_range(app, &format!("{newest}^"), Some(&newest));
        }),
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_and_changed_file_pickers_open_diff_views() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "before\n")?;
    repo.commit_all("initial")?;
    let base = repo.rev_parse("HEAD")?;
    repo.write_file("tracked.txt", "after\n")?;
    repo.commit_all("second")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");

    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    let mut harness = AppTestHarness::new();
    let diff_files_command = format!(":diff-files {base}<ret>");
    assert!(harness.send_keys(&mut app, &diff_files_command).await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    {
        let diff_state = main_diff_state(&app);
        assert_eq!(diff_state.base_ref, base);
        assert_eq!(diff_state.target_ref, None);
        assert_eq!(app.editor.diff.views.len(), 1);
        assert_current_doc_path(&app, &tracked_path);
    }
    harness.close(&mut app).await?;

    repo.write_file("tracked.txt", "working tree change\n")?;
    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    {
        let diff_state = main_diff_state(&app);
        assert_eq!(diff_state.base_ref, "HEAD");
        assert_eq!(diff_state.target_ref, None);
        assert_eq!(app.editor.diff.views.len(), 1);
        assert_current_doc_path(&app, &tracked_path);
    }
    harness.close(&mut app).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_file_picker_opens_merge_view_for_conflicts() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("conflict.txt", "base\n")?;
    repo.commit_all("base")?;

    repo.checkout_new_branch("feature")?;
    repo.write_file("conflict.txt", "theirs\n")?;
    repo.commit_all("feature change")?;

    repo.checkout("main")?;
    repo.write_file("conflict.txt", "ours\n")?;
    repo.commit_all("main change")?;
    repo.merge_expect_conflict("feature")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let conflict_path = repo.file("conflict.txt");
    let mut app = AppBuilder::new().with_file(&conflict_path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);

    assert_eq!(app.editor.diff.merge_views.len(), 3);
    assert_eq!(app.editor.tree.views().count(), 3);
    assert_current_doc_path(&app, &conflict_path);

    harness.close(&mut app).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn split_diff_controls_and_cross_pane_navigation_work() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file(
        "tracked.txt",
        indoc! {"
            zero
            one
            two
            three
            four
            five
        "},
    )?;
    repo.commit_all("initial")?;
    repo.write_file(
        "tracked.txt",
        indoc! {"
            zero
            one changed
            two
            three
            four changed
            five
        "},
    )?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    assert_eq!(app.editor.diff.views.len(), 1);
    assert_eq!(app.editor.tree.views().count(), 1);

    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    assert_status(&app, "Split view on", Severity::Info);
    assert_eq!(app.editor.diff.views.len(), 2);
    assert_eq!(app.editor.tree.views().count(), 2);

    assert!(harness.send_keys(&mut app, "<space>ms").await?);
    assert_status(&app, "Sync scroll disabled", Severity::Info);
    {
        let diff_state = main_diff_state(&app);
        assert!(!app.editor.diff.views[&diff_state.base_view_id].sync_scroll);
        assert!(!app.editor.diff.views[&diff_state.working_view_id].sync_scroll);
    }

    assert!(harness.send_keys(&mut app, "]g").await?);
    assert_split_diff_cursor_line(&app, 1);

    assert!(harness.send_keys(&mut app, "]g").await?);
    assert_split_diff_cursor_line(&app, 4);

    assert!(harness.send_keys(&mut app, "[g").await?);
    assert_split_diff_cursor_line(&app, 1);

    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    assert!(app.editor.diff.views.is_empty());
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_current_doc_path(&app, &tracked_path);
    {
        let view = app.editor.tree.get(app.editor.tree.focus);
        let doc = app.editor.document(view.doc).unwrap();
        assert!(!doc.char_diff_enabled);
    }

    harness.close(&mut app).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_workflow_resolves_conflicts_and_stages_file() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file(
        "conflict.txt",
        indoc! {"
            start
            first base
            keep one
            keep two
            keep three
            keep four
            second base
            end
        "},
    )?;
    repo.commit_all("base")?;

    repo.checkout_new_branch("feature")?;
    repo.write_file(
        "conflict.txt",
        indoc! {"
            start
            first theirs
            keep one
            keep two
            keep three
            keep four
            second theirs
            end
        "},
    )?;
    repo.commit_all("feature change")?;

    repo.checkout("main")?;
    repo.write_file(
        "conflict.txt",
        indoc! {"
            start
            first ours
            keep one
            keep two
            keep three
            keep four
            second ours
            end
        "},
    )?;
    repo.commit_all("main change")?;
    repo.merge_expect_conflict("feature")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let conflict_path = repo.file("conflict.txt");
    let mut app = AppBuilder::new().with_file(&conflict_path, None).build()?;

    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":merge<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.diff.merge_views.len(), 3);
                    assert_eq!(app.editor.tree.views().count(), 3);
                    assert_current_doc_path(app, &conflict_path);
                }),
            ),
            (
                Some("<space>mn"),
                Some(&|app| {
                    assert_status(app, "Conflict 1/2", Severity::Info);
                    assert_eq!(current_cursor_line(app), 1);
                }),
            ),
            (
                Some("<space>mp"),
                Some(&|app| {
                    assert_status(app, "Conflict 2/2", Severity::Info);
                    assert_eq!(current_cursor_line(app), 10);
                }),
            ),
            (
                Some("<space>mn"),
                Some(&|app| {
                    assert_status(app, "Conflict 1/2", Severity::Info);
                    assert_eq!(current_cursor_line(app), 1);
                }),
            ),
            (
                Some("<space>mo"),
                Some(&|app| {
                    assert_status(app, "Accepted HEAD version", Severity::Info);
                    let text = current_doc_text(app);
                    assert!(text.contains(
                        "start\nfirst ours\nkeep one\nkeep two\nkeep three\nkeep four\n<<<<<<<"
                    ));
                }),
            ),
            (
                Some("<space>mt"),
                Some(&|app| {
                    assert_status(app, "Switched to incoming version", Severity::Info);
                    let text = current_doc_text(app);
                    assert!(text.contains(
                        "start\nfirst theirs\nkeep one\nkeep two\nkeep three\nkeep four\n<<<<<<<"
                    ));
                }),
            ),
            (
                Some("<space>mn"),
                Some(&|app| {
                    assert_status(app, "Conflict 1/1", Severity::Info);
                    assert_eq!(current_cursor_line(app), 6);
                }),
            ),
            (
                Some("<space>mf"),
                Some(&|app| {
                    assert_status(app, "1 conflict(s) still unresolved", Severity::Error);
                }),
            ),
            (
                Some("<space>mb"),
                Some(&|app| {
                    assert_status(app, "Accepted both version", Severity::Info);
                    let text = current_doc_text(app);
                    assert!(text.contains("second ours\nsecond theirs\n"));
                }),
            ),
            (
                Some("<space>mf"),
                Some(&|app| {
                    assert!(
                        !app.editor.is_err(),
                        "status: {:?}",
                        app.editor.get_status()
                    );
                }),
            ),
        ],
        false,
    )
    .await?;

    assert_eq!(
        std::fs::read_to_string(&conflict_path)?,
        LineFeedHandling::Native.apply(indoc! {"
                start
                first theirs
                keep one
                keep two
                keep three
                keep four
                second ours
                second theirs
                end
            "},),
    );
    assert!(repo.unmerged_entries()?.is_empty());
    assert_eq!(repo.diff_cached_names()?, vec!["conflict.txt".to_string()]);

    Ok(())
}
