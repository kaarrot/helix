use std::path::Path;

use helix_core::diagnostic::Severity;
use helix_core::Transaction;
use helix_stdx::path;
use helix_term::application::Application;
use helix_view::editor::Action;

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

fn view_anchor_line(
    app: &Application,
    doc_id: helix_view::DocumentId,
    view_id: helix_view::ViewId,
) -> usize {
    let doc = app.editor.document(doc_id).unwrap();
    doc.text()
        .char_to_line(doc.view_offset(view_id).anchor.min(doc.text().len_chars()))
}

fn split_diff_anchor_lines(app: &Application) -> (usize, usize) {
    let diff_state = main_diff_state(app);
    (
        view_anchor_line(app, diff_state.base_doc_id, diff_state.base_view_id),
        view_anchor_line(app, diff_state.working_doc_id, diff_state.working_view_id),
    )
}

fn focused_and_other_anchor_lines(app: &Application) -> (usize, usize) {
    let view_id = app.editor.tree.focus;
    let diff_state = app.editor.diff.views.get(&view_id).unwrap();
    let (focused_doc, focused_view, other_doc, other_view) =
        if view_id == diff_state.working_view_id {
            (
                diff_state.working_doc_id,
                diff_state.working_view_id,
                diff_state.base_doc_id,
                diff_state.base_view_id,
            )
        } else {
            (
                diff_state.base_doc_id,
                diff_state.base_view_id,
                diff_state.working_doc_id,
                diff_state.working_view_id,
            )
        };
    (
        view_anchor_line(app, focused_doc, focused_view),
        view_anchor_line(app, other_doc, other_view),
    )
}

fn conflicted_repo() -> anyhow::Result<GitRepoFixture> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("conflict.txt", "base\n")?;
    repo.commit_all("base")?;
    repo.checkout_new_branch("feature")?;
    repo.write_file("conflict.txt", "theirs\n")?;
    repo.commit_all("feature")?;
    repo.checkout("main")?;
    repo.write_file("conflict.txt", "ours\n")?;
    repo.commit_all("main")?;
    repo.merge_expect_conflict("feature")?;
    Ok(repo)
}

#[tokio::test(flavor = "multi_thread")]
async fn regression_opening_a_diff_replaces_the_merge_session() -> anyhow::Result<()> {
    let repo = conflicted_repo()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    for split in [true, false] {
        for target in [None, Some("feature")] {
            let file = repo.file("conflict.txt");
            let mut app = AppBuilder::new().with_file(&file, None).build()?;
            app.editor.open_merge_view(&file)?;
            assert_eq!(app.editor.diff.merge_views.len(), 3);
            match target {
                None => app.editor.open_diff_view(&file, "HEAD", Some(split))?,
                Some(target) => {
                    app.editor
                        .open_diff_view_range(&file, "HEAD", Some(target), Some(split))?
                }
            }
            assert!(
                app.editor.diff.merge_views.is_empty(),
                "split={split}, target={target:?}"
            );
            let count = if split { 2 } else { 1 };
            assert_eq!(app.editor.tree.views().count(), count);
            assert_eq!(app.editor.diff.views.len(), count);
            for (view, _) in app.editor.tree.views() {
                assert!(app.editor.document(view.doc).is_some());
            }
            app.editor.close_diff_view(app.editor.tree.focus);
            AppTestHarness::new().close(&mut app).await?;
        }
    }
    Ok(())
}

async fn check_merge_result_close(command: &str) -> anyhow::Result<()> {
    let repo = conflicted_repo()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("conflict.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    assert!(harness.send_keys(&mut app, command).await?, "{command}");
    assert!(app.editor.diff.merge_views.is_empty(), "{command}");
    assert!(app.editor.tree.views().count() > 0);
    for (view, _) in app.editor.tree.views() {
        assert!(app.editor.document(view.doc).is_some(), "{command}");
    }
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_merge_result_without_history_keeps_valid_views() -> anyhow::Result<()> {
    check_merge_result_close(":bc<ret>").await
}

#[tokio::test(flavor = "multi_thread")]
async fn force_closing_merge_result_without_history_keeps_valid_views() -> anyhow::Result<()> {
    check_merge_result_close(":bc!<ret>").await
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_all_merge_buffers_keeps_valid_views() -> anyhow::Result<()> {
    check_merge_result_close(":buffer-close-all<ret>").await
}

#[tokio::test(flavor = "multi_thread")]
async fn writing_and_closing_merge_result_keeps_valid_views() -> anyhow::Result<()> {
    check_merge_result_close(":write-buffer-close<ret>").await
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_conflicts_cannot_be_accepted_or_staged() -> anyhow::Result<()> {
    let repo = conflicted_repo()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("conflict.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    let unmerged = repo.unmerged_entries()?;
    for input in [
        "=======\ntheirs\n>>>>>>> theirs\n",
        "<<<<<<< ours\nours\n=======\ntheirs\n",
        "<<<<<<< ours\nours\n=======\ntheirs\n>>>>>>> quoted\nmore\n>>>>>>> theirs\n",
    ] {
        let (view, doc) = helix_view::current!(app.editor);
        let tx = Transaction::change(
            doc.text(),
            [(0, doc.text().len_chars(), Some(input.into()))].into_iter(),
        );
        assert!(doc.apply(&tx, view.id));
        assert!(harness.send_keys(&mut app, "<space>mo").await?);
        assert!(app.editor.is_err());
        assert_eq!(current_doc_text(&app), input);
        assert!(harness.send_keys(&mut app, "<space>mf").await?);
        assert!(app.editor.is_err());
        assert_eq!(repo.unmerged_entries()?, unmerged);
    }
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn scratch_buffers_can_be_diffed_and_update_live() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    {
        let (view, doc) = helix_view::current!(app.editor);
        let tx = Transaction::change(
            doc.text(),
            std::iter::once((0, doc.text().len_chars(), Some("base\nsame\n".into()))),
        );
        doc.apply(&tx, view.id);
    }
    let base_doc_id = app.editor.tree.get(app.editor.tree.focus).doc;
    let working_doc_id = app.editor.new_file(Action::Replace);
    {
        let (view, doc) = helix_view::current!(app.editor);
        let tx = Transaction::change(
            doc.text(),
            std::iter::once((0, doc.text().len_chars(), Some("working\nsame\n".into()))),
        );
        doc.apply(&tx, view.id);
    }

    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":diff-buffer<ret>").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    assert!(harness.wait_for_idle(&mut app).await?);

    {
        let diff_state = main_diff_state(&app);
        assert!(matches!(
            diff_state.source,
            helix_view::diff_view::DiffViewSource::Buffers
        ));
        assert_eq!(diff_state.base_doc_id, base_doc_id);
        assert_eq!(diff_state.working_doc_id, working_doc_id);
        assert_eq!(app.editor.diff.views.len(), 2);

        let base_doc = app.editor.document(base_doc_id).unwrap();
        let working_doc = app.editor.document(working_doc_id).unwrap();
        assert!(base_doc.path().is_none());
        assert!(working_doc.path().is_none());
        let base_diff = base_doc
            .diff_handle_for_view(diff_state.base_view_id)
            .unwrap()
            .load();
        let working_diff = working_doc
            .diff_handle_for_view(diff_state.working_view_id)
            .unwrap()
            .load();
        assert!(base_diff.is_inverted());
        assert!(!working_diff.is_inverted());
        assert!(!working_diff.is_empty());
    }

    let diff_state = main_diff_state(&app);
    app.editor.focus(diff_state.base_view_id);

    assert!(harness.send_keys(&mut app, "Goleft-only<esc>").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    let expected_base = app.editor.document(base_doc_id).unwrap().text().to_string();
    let actual_base = app
        .editor
        .document(working_doc_id)
        .unwrap()
        .diff_handle_for_view(diff_state.working_view_id)
        .unwrap()
        .load()
        .diff_base()
        .to_string();
    assert_eq!(actual_base, expected_base);

    let doc_count = app.editor.documents.len();
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    assert!(app.editor.diff.views.is_empty());
    assert_eq!(app.editor.documents.len(), doc_count);
    assert!(app
        .editor
        .document(base_doc_id)
        .unwrap()
        .diff_handle_for_view(diff_state.base_view_id)
        .is_none());
    assert!(app
        .editor
        .document(working_doc_id)
        .unwrap()
        .diff_handle_for_view(diff_state.working_view_id)
        .is_none());

    harness.close(&mut app).await?;

    Ok(())
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

    let single_line_c = format!("#[{newest} newest commit|]#\n");
    let mut app = AppBuilder::new()
        .with_file(repo.file("log.txt"), None)
        .with_input_text(single_line_c)
        .build()?;

    test_key_sequence(
        &mut app,
        Some("<space>mc"),
        Some(&|app| {
            assert_status(
                app,
                &format!(
                    "Diff set: {newest} (HEAD) vs working tree: uncommitted changes only, space m C shows the commit"
                ),
                Severity::Warning,
            );
            assert_diff_range(app, &newest, None);
        }),
        false,
    )
    .await?;

    let single_older_c = format!("#[{oldest} older commit|]#\n");
    let mut app = AppBuilder::new()
        .with_file(repo.file("log.txt"), None)
        .with_input_text(single_older_c)
        .build()?;

    test_key_sequence(
        &mut app,
        Some("<space>mc"),
        Some(&|app| {
            assert_status(
                app,
                &format!("Diff set: {oldest} vs working tree"),
                Severity::Info,
            );
            assert_diff_range(app, &oldest, None);
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

    // Closing a pane ends the session and keeps one view; the harness's own
    // `:q!` then quits.
    assert!(harness.send_keys(&mut app, ":q!<ret>").await?);
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

    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    assert_status(&app, "Split view off", Severity::Info);
    assert_eq!(app.editor.diff.views.len(), 1);
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_eq!(current_cursor_line(&app), 1);

    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    assert_status(&app, "Split view on", Severity::Info);
    assert_eq!(app.editor.diff.views.len(), 2);
    assert_eq!(app.editor.tree.views().count(), 2);
    assert_split_diff_cursor_line(&app, 1);

    {
        let diff_state = main_diff_state(&app);
        app.editor.focus(diff_state.base_view_id);
    }
    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    assert_status(&app, "Split view off", Severity::Info);
    assert_eq!(app.editor.diff.views.len(), 1);
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_eq!(current_cursor_line(&app), 1);

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
async fn range_diff_can_toggle_split_view_off() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\n")?;
    repo.commit_all("initial")?;
    let older = repo.rev_parse("HEAD")?;
    repo.write_file("tracked.txt", "two\n")?;
    repo.commit_all("second")?;
    let newer = repo.rev_parse("HEAD")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    let mut harness = AppTestHarness::new();

    let command = format!(":diff-commit {older}..{newer}<ret>");
    assert!(harness.send_keys(&mut app, &command).await?);
    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);

    {
        let diff_state = main_diff_state(&app);
        assert_eq!(diff_state.base_ref, older);
        assert_eq!(diff_state.target_ref.as_deref(), Some(newer.as_str()));
        assert!(diff_state.close_base_doc_on_close);
        assert!(diff_state.close_working_doc_on_close);
        assert_eq!(app.editor.diff.views.len(), 1);
        assert_eq!(app.editor.tree.views().count(), 1);
    }

    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    assert_status(&app, "Split view on", Severity::Info);
    assert_eq!(app.editor.diff.views.len(), 2);
    assert_eq!(app.editor.tree.views().count(), 2);

    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    assert_status(&app, "Split view off", Severity::Info);
    assert_eq!(app.editor.diff.views.len(), 1);
    assert_eq!(app.editor.tree.views().count(), 1);
    let _view = app.editor.tree.get(app.editor.tree.focus);

    harness.close(&mut app).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_working_tree_path_keeps_revision_buffers() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\n")?;
    repo.commit_all("initial")?;
    let older = repo.rev_parse("HEAD")?;
    repo.write_file("tracked.txt", "two\n")?;
    repo.commit_all("second")?;
    let newer = repo.rev_parse("HEAD")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().build()?;

    app.editor
        .open_diff_view_range(&tracked_path, &older, Some(&newer), Some(false))?;

    let diff_state = main_diff_state(&app);
    let base_doc_id = diff_state.base_doc_id;
    let target_doc_id = diff_state.working_doc_id;

    {
        let target_doc = app.editor.document(target_doc_id).unwrap();
        assert!(target_doc.url().is_some());
        assert_eq!(
            target_doc
                .git_revision
                .as_ref()
                .map(|revision| revision.path.as_path()),
            Some(tracked_path.as_path())
        );
    }

    let working_doc_id = app.editor.open(&tracked_path, Action::Replace)?;

    assert_current_doc_path(&app, &tracked_path);
    assert_ne!(working_doc_id, target_doc_id);
    assert!(app.editor.diff.views.is_empty());
    assert_eq!(
        app.editor.document(base_doc_id).unwrap().text().to_string(),
        "one\n"
    );
    assert_eq!(
        app.editor
            .document(target_doc_id)
            .unwrap()
            .text()
            .to_string(),
        "two\n"
    );

    let errs = app.close().await;
    assert!(errs.is_empty(), "errors closing app: {errs:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn same_file_at_different_commits_are_separate_buffers() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\n")?;
    repo.commit_all("initial")?;
    let first = repo.rev_parse("HEAD")?;
    repo.write_file("tracked.txt", "two\n")?;
    repo.commit_all("second")?;
    let second = repo.rev_parse("HEAD")?;
    repo.write_file("tracked.txt", "three\n")?;
    repo.commit_all("third")?;
    let third = repo.rev_parse("HEAD")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().build()?;

    app.editor
        .open_diff_view_range(&tracked_path, &first, Some(&second), Some(false))?;
    let first_diff = main_diff_state(&app);
    let first_doc = first_diff.base_doc_id;
    let second_doc = first_diff.working_doc_id;
    assert_eq!(
        app.editor.document(first_doc).unwrap().text().to_string(),
        "one\n"
    );
    assert_eq!(
        app.editor.document(second_doc).unwrap().text().to_string(),
        "two\n"
    );

    app.editor
        .open_diff_view_range(&tracked_path, &second, Some(&third), Some(false))?;
    let second_diff = main_diff_state(&app);
    let third_doc = second_diff.working_doc_id;
    assert_eq!(second_diff.base_doc_id, second_doc);
    assert_ne!(third_doc, second_doc);
    assert_ne!(third_doc, first_doc);
    assert_eq!(
        app.editor.document(first_doc).unwrap().text().to_string(),
        "one\n"
    );
    assert_eq!(
        app.editor.document(second_doc).unwrap().text().to_string(),
        "two\n"
    );
    assert_eq!(
        app.editor.document(third_doc).unwrap().text().to_string(),
        "three\n"
    );

    app.editor
        .open_diff_view_range(&tracked_path, &second, Some(&third), Some(false))?;
    let reused = main_diff_state(&app);
    assert_eq!(reused.base_doc_id, second_doc);
    assert_eq!(reused.working_doc_id, third_doc);

    let working_doc_id = app.editor.open(&tracked_path, Action::Replace)?;
    assert_current_doc_path(&app, &tracked_path);
    assert_ne!(working_doc_id, first_doc);
    assert_ne!(working_doc_id, second_doc);
    assert_ne!(working_doc_id, third_doc);
    assert!(app.editor.document(first_doc).is_some());
    assert!(app.editor.document(second_doc).is_some());
    assert!(app.editor.document(third_doc).is_some());

    let errs = app.close().await;
    assert!(errs.is_empty(), "errors closing app: {errs:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn split_diff_keyboard_and_search_sync_scroll() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    let mut base = String::new();
    let mut working = String::new();
    for i in 0..250 {
        if i == 1 {
            base.push_str("one\n");
            working.push_str("one changed\n");
        } else if i == 200 || i == 240 {
            base.push_str("needle\n");
            working.push_str("needle\n");
        } else {
            base.push_str(&format!("line {i}\n"));
            working.push_str(&format!("line {i}\n"));
        }
    }
    repo.write_file("tracked.txt", &base)?;
    repo.commit_all("initial")?;
    repo.write_file("tracked.txt", &working)?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    // The two search matches must be farther apart than one viewport.
    app.resize_test_terminal(120, 24);
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    assert_status(&app, "Split view on", Severity::Info);
    assert_eq!(app.editor.tree.views().count(), 2);
    assert_eq!(split_diff_anchor_lines(&app), (0, 0));

    assert!(harness.send_keys(&mut app, "200j").await?);
    let (base_line, working_line) = split_diff_anchor_lines(&app);
    assert!(
        base_line > 0 && working_line > 0,
        "j/k movement should scroll both split-diff panes, got base={base_line} working={working_line}"
    );
    assert_eq!(base_line, working_line);

    assert!(harness.send_keys(&mut app, "gg").await?);
    assert_eq!(split_diff_anchor_lines(&app), (0, 0));

    assert!(harness.send_keys(&mut app, "<space>ms").await?);
    assert_status(&app, "Sync scroll disabled", Severity::Info);
    assert!(harness.send_keys(&mut app, "200j").await?);
    let (focused, other) = focused_and_other_anchor_lines(&app);
    assert!(
        focused > 0,
        "focused pane should still scroll with sync off"
    );
    assert_eq!(
        other, 0,
        "unfocused pane should not follow when sync is off"
    );

    assert!(harness.send_keys(&mut app, "gg").await?);
    assert!(harness.send_keys(&mut app, "<space>ms").await?);
    assert_status(&app, "Sync scroll enabled", Severity::Info);
    // Re-enable does not itself remap; gg already put the focused pane at the top,
    // so one more movement/ensure is needed to pull the other pane back.
    assert!(harness.send_keys(&mut app, "gg").await?);
    assert_eq!(split_diff_anchor_lines(&app), (0, 0));

    assert!(harness.send_keys(&mut app, "/needle<ret>").await?);
    let (base_line, working_line) = split_diff_anchor_lines(&app);
    assert!(
        base_line > 0 && working_line > 0,
        "search should scroll both split-diff panes, got base={base_line} working={working_line}"
    );
    assert_eq!(base_line, working_line);
    let first_match_line = base_line;

    assert!(harness.send_keys(&mut app, "n").await?);
    let (base_line, working_line) = split_diff_anchor_lines(&app);
    assert!(
        base_line > first_match_line && working_line > first_match_line,
        "n should scroll both split-diff panes to the next match, first={first_match_line} base={base_line} working={working_line}"
    );
    assert_eq!(base_line, working_line);

    assert!(harness.send_keys(&mut app, "N").await?);
    let (base_line, working_line) = split_diff_anchor_lines(&app);
    assert_eq!(base_line, first_match_line);
    assert_eq!(working_line, first_match_line);

    assert!(harness.send_keys(&mut app, "gg").await?);
    assert_eq!(split_diff_anchor_lines(&app), (0, 0));
    assert!(harness.send_keys(&mut app, "/needle<esc>").await?);
    assert_eq!(split_diff_anchor_lines(&app), (0, 0));

    // Closing a pane ends the session and keeps one view; the harness's own
    // `:q!` then quits.
    assert!(harness.send_keys(&mut app, ":q!<ret>").await?);
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
                    assert_status(app, "Conflict 2/2", Severity::Info);
                    assert_eq!(current_cursor_line(app), 10);
                }),
            ),
            (
                Some("<space>mp"),
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
            // Closing a pane ends the session and keeps one view; the helper's
            // own `:q!` then quits.
            (Some(":q!<ret>"), None),
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

#[tokio::test(flavor = "multi_thread")]
async fn diff_base_compares_against_ref_without_reloading() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "version one\n")?;
    repo.commit_all("initial")?;
    repo.write_file("tracked.txt", "version two\n")?;
    repo.commit_all("second")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("tracked.txt"), None)
        .build()?;

    let mut harness = AppTestHarness::new();
    assert!(
        harness
            .send_keys(&mut app, "iversion two dirty<esc>")
            .await?
    );
    assert!(harness.send_keys(&mut app, ":diff-base HEAD^<ret>").await?);
    assert!(harness.wait_for_idle(&mut app).await?);

    {
        let view = app.editor.tree.get(app.editor.tree.focus);
        let doc = app.editor.document(view.doc).unwrap();
        assert_eq!(doc.diff_base_ref(), Some("HEAD^"));
        assert!(
            doc.text().to_string().contains("version two dirty"),
            "unsaved buffer must not be reloaded from disk"
        );
        let diff_base = doc
            .diff_handle()
            .expect("differ should be set")
            .load()
            .diff_base()
            .to_string();
        assert!(
            diff_base.contains("version one"),
            "gutter base should be HEAD^, got {diff_base:?}"
        );
        assert!(
            !diff_base.contains("version two"),
            "gutter base should not be HEAD, got {diff_base:?}"
        );
    }

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn git_split_diff_stays_in_sync_after_edits() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "before\n")?;
    repo.commit_all("initial")?;
    repo.write_file("tracked.txt", "after\n")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    assert!(harness.wait_for_idle(&mut app).await?);

    let diff_state = main_diff_state(&app);
    {
        let base_doc = app.editor.document(diff_state.base_doc_id).unwrap();
        assert!(base_doc.is_virtual_base);
        assert!(base_doc.readonly);
        assert!(base_doc.git_revision.is_some());
        assert_ne!(base_doc.path(), Some(&tracked_path));
        assert!(base_doc.url().is_some());
    }

    app.editor.focus(diff_state.working_view_id);
    assert!(harness.send_keys(&mut app, "Goedited<esc>").await?);
    assert!(harness.wait_for_idle(&mut app).await?);

    let expected_working = app
        .editor
        .document(diff_state.working_doc_id)
        .unwrap()
        .text()
        .to_string();
    let actual_from_base = app
        .editor
        .document(diff_state.base_doc_id)
        .unwrap()
        .diff_handle_for_view(diff_state.base_view_id)
        .unwrap()
        .load()
        .diff_base()
        .to_string();
    assert_eq!(actual_from_base, expected_working);

    // Closing a pane ends the session and keeps one view; the harness's own
    // `:q!` then quits.
    assert!(harness.send_keys(&mut app, ":q!<ret>").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_diff_base_pane_does_not_panic() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "before\n")?;
    repo.commit_all("initial")?;
    repo.write_file("tracked.txt", "after\n")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    assert!(harness.send_keys(&mut app, "<space>mv").await?);

    let diff_state = main_diff_state(&app);
    app.editor.focus(diff_state.base_view_id);
    assert!(harness.send_keys(&mut app, "<C-w>q").await?);

    assert!(app.editor.diff.views.is_empty());
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_current_doc_path(&app, &tracked_path);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_commands_target_result_from_ours_pane() -> anyhow::Result<()> {
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

    test_key_sequences(
        &mut app,
        vec![
            (
                Some(":merge<ret>"),
                Some(&|app| {
                    assert_eq!(app.editor.diff.merge_views.len(), 3);
                    let state = app
                        .editor
                        .diff
                        .merge_views
                        .get(&app.editor.tree.focus)
                        .cloned()
                        .expect("merge session");
                    for doc_id in [state.ours_doc_id, state.theirs_doc_id] {
                        let side = app.editor.document(doc_id).unwrap();
                        assert!(side.is_virtual_base);
                        assert!(side.readonly);
                        assert_ne!(side.path(), Some(&conflict_path));
                        assert_eq!(
                            side.git_revision.as_ref().map(|revision| &revision.path),
                            Some(&conflict_path)
                        );
                    }
                }),
            ),
            (
                Some("<C-w>w"),
                Some(&|app| {
                    let state = app
                        .editor
                        .diff
                        .merge_views
                        .get(&app.editor.tree.focus)
                        .cloned()
                        .expect("merge session");
                    assert_ne!(
                        app.editor.tree.focus, state.result_view_id,
                        "expected to leave the RESULT pane"
                    );
                }),
            ),
            (
                Some("<space>mo"),
                Some(&|app| {
                    assert_status(app, "Accepted HEAD version", Severity::Info);
                    let state = app
                        .editor
                        .diff
                        .merge_views
                        .values()
                        .next()
                        .cloned()
                        .expect("merge session");
                    let result_text = app
                        .editor
                        .document(state.result_doc_id)
                        .unwrap()
                        .text()
                        .to_string();
                    assert!(
                        result_text.contains("ours"),
                        "accept from OURS pane must edit RESULT, got {result_text:?}"
                    );
                    assert!(
                        !result_text.contains("<<<<<<<"),
                        "RESULT should have no markers, got {result_text:?}"
                    );
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
            (
                Some("<space>mq"),
                Some(&|app| {
                    assert!(app.editor.diff.merge_views.is_empty());
                }),
            ),
        ],
        false,
    )
    .await?;

    assert_eq!(
        std::fs::read_to_string(&conflict_path)?,
        LineFeedHandling::Native.apply("ours\n"),
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_merge_ours_pane_does_not_panic() -> anyhow::Result<()> {
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

    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    let state = app
        .editor
        .diff
        .merge_views
        .get(&app.editor.tree.focus)
        .cloned()
        .expect("merge session");

    app.editor.focus(state.ours_view_id);
    assert!(harness.send_keys(&mut app, "<C-w>q").await?);

    assert!(app.editor.diff.merge_views.is_empty());
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_current_doc_path(&app, &conflict_path);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn quit_in_split_diff_working_pane_keeps_unsaved_buffer() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "before\n")?;
    repo.commit_all("initial")?;
    repo.write_file("tracked.txt", "after\n")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    let diff_state = main_diff_state(&app);
    app.editor.focus(diff_state.working_view_id);
    assert!(harness.send_keys(&mut app, "ihello<esc>").await?);

    // Ends the diff and keeps the edited buffer on screen.
    assert!(harness.send_keys(&mut app, ":q<ret>").await?);
    assert!(app.editor.diff.views.is_empty());
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_current_doc_path(&app, &tracked_path);
    assert!(helix_view::doc!(app.editor).is_modified());

    // Now it is the last view, so the unsaved check applies.
    assert!(harness.send_keys(&mut app, ":q<ret>").await?);
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_eq!(app.editor.get_status().unwrap().1, &Severity::Error);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn window_only_in_split_diff_base_pane_keeps_a_view() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "before\n")?;
    repo.commit_all("initial")?;
    repo.write_file("tracked.txt", "after\n")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    let diff_state = main_diff_state(&app);
    app.editor.focus(diff_state.base_view_id);

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    assert!(app.editor.diff.views.is_empty());
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_current_doc_path(&app, &tracked_path);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn quit_in_merge_result_pane_keeps_result() -> anyhow::Result<()> {
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

    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    assert!(harness.send_keys(&mut app, ":q<ret>").await?);

    assert!(app.editor.diff.merge_views.is_empty());
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_current_doc_path(&app, &conflict_path);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn quit_in_buffer_diff_keeps_a_view() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().build()?;
    {
        let (view, doc) = helix_view::current!(app.editor);
        let tx = Transaction::change(
            doc.text(),
            std::iter::once((0, doc.text().len_chars(), Some("base\n".into()))),
        );
        doc.apply(&tx, view.id);
    }
    let working_doc_id = app.editor.new_file(Action::Replace);
    {
        let (view, doc) = helix_view::current!(app.editor);
        let tx = Transaction::change(
            doc.text(),
            std::iter::once((0, doc.text().len_chars(), Some("working\n".into()))),
        );
        doc.apply(&tx, view.id);
    }

    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":diff-buffer<ret>").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    let diff_state = main_diff_state(&app);
    assert_eq!(diff_state.working_doc_id, working_doc_id);
    app.editor.focus(diff_state.working_view_id);

    assert!(harness.send_keys(&mut app, ":q<ret>").await?);
    assert!(app.editor.diff.views.is_empty());
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_eq!(
        app.editor.tree.get(app.editor.tree.focus).doc,
        working_doc_id
    );

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn changed_file_picker_opens_deleted_file_read_only() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "tracked\n")?;
    repo.write_file("doomed.txt", "bye\n")?;
    repo.commit_all("initial")?;
    let doomed_path = repo.file("doomed.txt");
    std::fs::remove_file(&doomed_path)?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    let mut harness = AppTestHarness::new();

    // The deleted file is the only change, so Enter opens it.
    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);

    let diff_state = main_diff_state(&app);
    assert!(diff_state.close_working_doc_on_close);
    {
        let working = app.editor.document(diff_state.working_doc_id).unwrap();
        assert!(working.path().is_none());
        assert!(working.is_virtual_base);
        assert!(working.text().len_chars() == 0);
        let base = app.editor.document(diff_state.base_doc_id).unwrap();
        assert_eq!(base.text(), &LineFeedHandling::Native.apply("bye\n"));
    }

    assert!(harness.send_keys(&mut app, ":w<ret>").await?);
    assert!(!doomed_path.exists(), ":w recreated the deleted file");

    // Toggling the split reopens both virtual panes.
    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    let diff_state = main_diff_state(&app);
    assert_ne!(diff_state.base_view_id, diff_state.working_view_id);
    assert!(app
        .editor
        .document(diff_state.working_doc_id)
        .unwrap()
        .path()
        .is_none());

    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    assert!(app.editor.diff.views.is_empty());
    assert!(app.editor.document(diff_state.working_doc_id).is_none());
    assert!(!doomed_path.exists());

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_view_focuses_result_and_keeps_sides_read_only() -> anyhow::Result<()> {
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

    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    let state = app
        .editor
        .diff
        .merge_views
        .get(&app.editor.tree.focus)
        .cloned()
        .expect("merge session");
    assert_eq!(app.editor.tree.focus, state.result_view_id);

    // THEIRS is the last pane opened; RESULT must take the focus from it.
    let focused_at = |doc_id| app.editor.document(doc_id).unwrap().focused_at;
    assert!(focused_at(state.result_doc_id) > focused_at(state.theirs_doc_id));
    assert!(focused_at(state.result_doc_id) > focused_at(state.ours_doc_id));

    app.editor.focus(state.ours_view_id);
    let before = app
        .editor
        .document(state.ours_doc_id)
        .unwrap()
        .text()
        .clone();
    assert!(harness.send_keys(&mut app, "ihello<esc>").await?);
    let ours = app.editor.document(state.ours_doc_id).unwrap();
    assert_eq!(ours.text(), &before);
    assert!(!ours.is_modified());

    // Leave only RESULT so the harness's `:q!` exits.
    assert!(harness.send_keys(&mut app, "<C-w>q").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_close_in_merge_side_pane_does_not_panic() -> anyhow::Result<()> {
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

    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    let state = app
        .editor
        .diff
        .merge_views
        .get(&app.editor.tree.focus)
        .cloned()
        .expect("merge session");

    app.editor.focus(state.ours_view_id);
    assert!(harness.send_keys(&mut app, ":bc<ret>").await?);

    assert!(app.editor.diff.merge_views.is_empty());
    assert!(app.editor.document(state.ours_doc_id).is_none());
    assert!(app.editor.document(state.theirs_doc_id).is_none());
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_current_doc_path(&app, &conflict_path);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_close_in_range_diff_pane_does_not_panic() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\n")?;
    repo.commit_all("initial")?;
    let older = repo.rev_parse("HEAD")?;
    repo.write_file("tracked.txt", "two\n")?;
    repo.commit_all("second")?;
    let newer = repo.rev_parse("HEAD")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    let mut harness = AppTestHarness::new();

    let command = format!(":diff-commit {older}..{newer}<ret>");
    assert!(harness.send_keys(&mut app, &command).await?);
    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    assert!(harness.send_keys(&mut app, "<space>mv").await?);

    let diff_state = main_diff_state(&app);
    assert_ne!(diff_state.base_view_id, diff_state.working_view_id);
    app.editor.focus(diff_state.working_view_id);
    assert!(harness.send_keys(&mut app, ":bc<ret>").await?);

    assert!(app.editor.diff.views.is_empty());
    assert!(app.editor.document(diff_state.base_doc_id).is_none());
    assert!(app.editor.document(diff_state.working_doc_id).is_none());
    assert_eq!(app.editor.tree.views().count(), 1);

    harness.close(&mut app).await?;
    Ok(())
}

/// A single-pane diff shows only the working document, so there is no other
/// pane to keep in step. Syncing it anyway read the base document's scroll for
/// that view, as left by the last time the view showed it, and every key put
/// the working document back there.
#[tokio::test(flavor = "multi_thread")]
async fn single_pane_diff_keeps_its_own_scroll() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    let text: String = (0..2000).map(|n| format!("line {n}\n")).collect();
    repo.write_file("tracked.txt", &text)?;
    repo.commit_all("initial")?;
    repo.write_file("tracked.txt", &text.replace("line 1000\n", "changed\n"))?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    let diff_state = main_diff_state(&app);
    assert!(!diff_state.is_split());
    let view_id = diff_state.working_view_id;
    {
        let base = app.editor.document_mut(diff_state.base_doc_id).unwrap();
        base.ensure_view_init(view_id);
        base.set_view_offset(view_id, helix_view::view::ViewPosition::default());
    }

    assert!(harness.send_keys(&mut app, "1500gg").await?);
    let anchor = view_anchor_line(&app, diff_state.working_doc_id, view_id);
    assert!(
        (1000..1500).contains(&anchor),
        "the view should follow the cursor to line 1500, not start at line {anchor}"
    );

    harness.close(&mut app).await?;
    Ok(())
}

/// Showing another buffer in a split's base pane takes that pane out of the
/// diff and leaves the file under review open beside it, as a plain view
/// without the diff's colouring.
#[tokio::test(flavor = "multi_thread")]
async fn leaving_a_split_from_its_base_pane_keeps_the_working_file() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "before\n")?;
    repo.commit_all("initial")?;
    repo.write_file("tracked.txt", "after\n")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked_path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked_path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    let diff_state = main_diff_state(&app);
    app.editor.focus(diff_state.base_view_id);

    assert!(harness.send_keys(&mut app, ":new<ret>").await?);
    assert!(app.editor.diff.views.is_empty());
    assert_eq!(app.editor.tree.views().count(), 2);
    let working = app.editor.tree.get(diff_state.working_view_id);
    assert_eq!(working.doc, diff_state.working_doc_id);
    let working_doc = app.editor.document(diff_state.working_doc_id).unwrap();
    assert!(
        !working_doc.char_diff_enabled,
        "the working file kept the diff's colouring"
    );

    // `close` quits one view, so leave only one.
    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

/// A repository with a tracked file of numbered lines, one of them changed in
/// the working tree, and a second file to jump to.
fn repo_with_a_file_to_jump_to() -> anyhow::Result<GitRepoFixture> {
    let repo = GitRepoFixture::new()?;
    let text: String = (0..60).map(|n| format!("line {n}\n")).collect();
    repo.write_file("tracked.txt", &text)?;
    repo.write_file("other.txt", "other\n")?;
    repo.commit_all("initial")?;
    repo.write_file("tracked.txt", &text.replace("line 30\n", "changed\n"))?;
    Ok(repo)
}

async fn open_changed_file_diff(
    app: &mut Application,
    harness: &mut AppTestHarness,
    split: bool,
) -> anyhow::Result<()> {
    assert!(harness.send_keys(app, "<space>g").await?);
    assert!(harness.wait_for_idle(app).await?);
    assert!(harness.send_keys(app, "<ret>").await?);
    if split {
        assert!(harness.send_keys(app, "<space>mv").await?);
    }
    Ok(())
}

async fn open_other(
    app: &mut Application,
    harness: &mut AppTestHarness,
    other: &Path,
) -> anyhow::Result<()> {
    assert!(
        harness
            .send_keys(app, &format!(":o {}<ret>", other.display()))
            .await?
    );
    Ok(())
}

/// Going to another file with the working pane of a split diff takes the diff
/// apart. Jumping back to the file used to leave it a plain buffer, to be
/// diffed again from the top with `<space>g`. It brings the diff back instead,
/// around the same view, so the place in the file is not lost.
#[tokio::test(flavor = "multi_thread")]
async fn jumping_back_to_a_split_diff_restores_it() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("tracked.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();

    open_changed_file_diff(&mut app, &mut harness, true).await?;
    assert!(harness.send_keys(&mut app, "40gg").await?);
    let before = main_diff_state(&app);
    let working_view = before.working_view_id;
    assert_eq!(app.editor.tree.focus, working_view);

    open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
    assert!(app.editor.diff.views.is_empty());
    assert_eq!(app.editor.tree.views().count(), 1);

    assert!(harness.send_keys(&mut app, "<C-o>").await?);
    assert_eq!(app.editor.diff.views.len(), 2, "the diff should be back");
    assert_eq!(app.editor.tree.views().count(), 2);
    assert_eq!(
        app.editor.tree.focus, working_view,
        "the view the user was in keeps the focus"
    );
    let diff_state = main_diff_state(&app);
    assert!(diff_state.is_split());
    assert_eq!(diff_state.working_view_id, working_view);
    assert_eq!(diff_state.working_doc_id, before.working_doc_id);
    assert_eq!(
        app.editor.tree.get(working_view).doc,
        before.working_doc_id,
        "the working pane shows the file again"
    );
    assert_eq!(
        app.editor.tree.get(diff_state.base_view_id).doc,
        diff_state.base_doc_id
    );
    assert!(
        app.editor.tree.get(diff_state.base_view_id).area.x
            < app.editor.tree.get(working_view).area.x,
        "the base pane sits on the left"
    );
    assert!(app
        .editor
        .document(diff_state.working_doc_id)
        .unwrap()
        .char_diff_enabled_for_view(working_view));
    assert_eq!(
        current_cursor_line(&app),
        39,
        "the cursor stayed on its line"
    );
    assert_split_diff_cursor_line(&app, 39);

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

/// The jump out of a restored diff takes it apart again, as the first one did,
/// and the next jump back finds it once more.
#[tokio::test(flavor = "multi_thread")]
async fn a_diff_can_be_left_and_restored_by_jumping_back_and_forth() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let other = repo.file("other.txt");
    let mut app = AppBuilder::new()
        .with_file(repo.file("tracked.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();

    open_changed_file_diff(&mut app, &mut harness, true).await?;
    open_other(&mut app, &mut harness, &other).await?;
    assert!(harness.send_keys(&mut app, "<C-o>").await?);
    assert_eq!(app.editor.diff.views.len(), 2);

    assert!(harness.send_keys(&mut app, "<C-i>").await?);
    assert_current_doc_path(&app, &other);
    assert!(
        app.editor.diff.views.is_empty(),
        "a jump to another file ended the diff only in the view it was left from"
    );
    assert_eq!(app.editor.tree.views().count(), 1);

    assert!(harness.send_keys(&mut app, "<C-o>").await?);
    assert_eq!(app.editor.diff.views.len(), 2);
    assert_eq!(app.editor.tree.views().count(), 2);
    assert_current_doc_path(&app, &repo.file("tracked.txt"));

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn jumping_back_to_a_single_pane_diff_restores_it() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("tracked.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();

    open_changed_file_diff(&mut app, &mut harness, false).await?;
    let view = app.editor.tree.focus;
    assert!(harness.send_keys(&mut app, "40gg").await?);
    open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
    assert!(app.editor.diff.views.is_empty());

    assert!(harness.send_keys(&mut app, "<C-o>").await?);
    assert_eq!(app.editor.tree.views().count(), 1);
    assert_eq!(app.editor.tree.focus, view);
    let diff_state = main_diff_state(&app);
    assert!(!diff_state.is_split());
    assert_eq!(current_cursor_line(&app), 39);

    harness.close(&mut app).await?;
    Ok(())
}

/// A base pane that goes to another file leaves the working pane beside it, and
/// finds it there when it comes back.
#[tokio::test(flavor = "multi_thread")]
async fn jumping_back_in_the_base_pane_restores_the_diff() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("tracked.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();

    open_changed_file_diff(&mut app, &mut harness, true).await?;
    let before = main_diff_state(&app);
    app.editor.focus(before.base_view_id);

    open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
    assert!(app.editor.diff.views.is_empty());
    assert_eq!(app.editor.tree.views().count(), 2);

    assert!(harness.send_keys(&mut app, "<C-o>").await?);
    assert_eq!(app.editor.diff.views.len(), 2, "the diff should be back");
    assert_eq!(app.editor.tree.views().count(), 2);
    assert_eq!(app.editor.tree.focus, before.base_view_id);
    let diff_state = main_diff_state(&app);
    assert_eq!(diff_state.base_view_id, before.base_view_id);
    assert_eq!(diff_state.working_view_id, before.working_view_id);

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

/// Only the diff a view was last taken out of is worth jumping back to. One
/// opened or closed since is the user's newer choice, which a jump back must
/// not undo.
#[tokio::test(flavor = "multi_thread")]
async fn a_diff_opened_or_closed_since_is_not_brought_back() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    open_changed_file_diff(&mut app, &mut harness, true).await?;
    open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
    assert_eq!(app.editor.diff.suspended.len(), 1);

    // Diffing the file again from where the view is, ...
    app.editor.open_diff_view(&tracked, "HEAD", Some(false))?;
    assert!(app.editor.diff.suspended.is_empty());
    // ... and leaving that, is a diff of its own to jump back to.
    open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
    assert_eq!(app.editor.diff.suspended.len(), 1);

    // Closing the view ends it. The split takes the focus, so go back to
    // the view that was left before closing it.
    assert!(harness.send_keys(&mut app, ":vsplit<ret>").await?);
    assert_eq!(app.editor.diff.suspended.len(), 1);
    assert!(harness.send_keys(&mut app, "<C-w>h<C-w>q").await?);
    assert!(app.editor.diff.suspended.is_empty());

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ordinary_buffer_navigation_restores_diff() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    for split in [false, true] {
        for (keys, line) in [
            ("ga", 39),
            (":o tracked.txt<ret>", 0),
            (":o tracked.txt:17<ret>", 16),
            ("<space>btracked<ret>", 39),
            ("gn", 39),
            ("2gp", 39),
            (":bc<ret>", 39),
        ] {
            app.editor.open_diff_view(&tracked, "HEAD", Some(split))?;
            assert!(harness.send_keys(&mut app, "40gg").await?);
            let before = main_diff_state(&app);
            open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
            assert!(app.editor.diff.views.is_empty());

            assert!(harness.send_keys(&mut app, keys).await?);
            let restored = main_diff_state(&app);
            assert_eq!(restored.working_doc_id, before.working_doc_id, "{keys}");
            assert_eq!(restored.working_view_id, before.working_view_id, "{keys}");
            assert_eq!(restored.is_split(), split, "{keys}");
            assert_eq!(current_cursor_line(&app), line, "{keys}");
            assert!(app.editor.diff.suspended.is_empty(), "{keys}");
        }
    }

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_back_in_the_base_pane_restores_diff() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    app.editor.open_diff_view(&tracked, "HEAD", Some(true))?;
    let before = main_diff_state(&app);
    app.editor.focus(before.base_view_id);
    assert!(harness.send_keys(&mut app, "40gg").await?);
    open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;

    assert!(harness.send_keys(&mut app, "ga").await?);
    let restored = main_diff_state(&app);
    assert_eq!(restored.base_view_id, before.base_view_id);
    assert_eq!(restored.working_view_id, before.working_view_id);
    assert_eq!(current_cursor_line(&app), 39);
    assert!(app.editor.diff.suspended.is_empty());

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn returning_both_diff_panes_restores_diff_in_either_order() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    repo.write_file("third.txt", "third\n")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    for base_returns_first in [true, false] {
        for keys in ["ga", "<C-o>"] {
            app.editor.open_diff_view(&tracked, "HEAD", Some(true))?;
            let before = main_diff_state(&app);
            // This test preserves independent pane positions. Synchronized
            // comparisons intentionally map the cursor when focus changes.
            for state in app.editor.diff.views.values_mut() {
                state.sync_scroll = false;
            }
            app.editor.focus(before.base_view_id);
            assert!(harness.send_keys(&mut app, "20gg").await?);
            open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
            app.editor.focus(before.working_view_id);
            assert!(harness.send_keys(&mut app, "40gg").await?);
            open_other(&mut app, &mut harness, &repo.file("third.txt")).await?;

            let (first, last) = if base_returns_first {
                (before.base_view_id, before.working_view_id)
            } else {
                (before.working_view_id, before.base_view_id)
            };
            app.editor.focus(first);
            assert!(harness.send_keys(&mut app, keys).await?);
            assert!(app.editor.diff.views.is_empty());
            assert_eq!(app.editor.diff.suspended.len(), 1);

            app.editor.focus(last);
            assert!(harness.send_keys(&mut app, keys).await?);
            let restored = main_diff_state(&app);
            assert_eq!(restored.base_view_id, before.base_view_id);
            assert_eq!(restored.working_view_id, before.working_view_id);
            assert_eq!(app.editor.tree.views().count(), 2);
            assert_eq!(app.editor.tree.focus, last);
            assert!(app.editor.diff.suspended.is_empty());
            app.editor.focus(before.base_view_id);
            assert_eq!(current_cursor_line(&app), 19);
            app.editor.focus(before.working_view_id);
            assert_eq!(current_cursor_line(&app), 39);
        }
    }

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_diff_supersedes_its_partners_suspended_diff() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    app.editor.open_diff_view(&tracked, "HEAD", Some(true))?;
    let before = main_diff_state(&app);
    app.editor.focus(before.base_view_id);
    open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
    assert_eq!(app.editor.diff.suspended.len(), 1);

    app.editor.focus(before.working_view_id);
    app.editor.open_diff_view(&tracked, "HEAD", Some(false))?;
    assert!(app.editor.diff.suspended.is_empty());
    app.editor.focus(before.base_view_id);
    assert!(harness.send_keys(&mut app, "ga").await?);
    assert!(!app.editor.diff.views.contains_key(&before.base_view_id));
    assert_eq!(app.editor.diff.views.len(), 1);

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_diff_keeps_snapshots_used_by_another_diff() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let older = repo.rev_parse("HEAD")?;
    repo.commit_all("second")?;
    let newer = repo.rev_parse("HEAD")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    for target in [None, Some(newer.as_str())] {
        app.editor
            .open_diff_view_range(&tracked, &older, target, Some(false))?;
        let first = main_diff_state(&app);
        app.editor
            .switch(first.working_doc_id, Action::VerticalSplit);
        app.editor
            .open_diff_view_range(&tracked, &older, target, Some(false))?;
        let second = main_diff_state(&app);
        assert_eq!(first.base_doc_id, second.base_doc_id);
        assert_eq!(first.working_doc_id, second.working_doc_id);

        assert!(app.editor.close_diff_view(second.working_view_id));
        assert!(app.editor.diff.views.contains_key(&first.working_view_id));
        assert!(app.editor.document(first.base_doc_id).is_some());
        assert!(app.editor.document(first.working_doc_id).is_some());
        assert!(harness.wait_for_idle(&mut app).await?);
        let base = app.editor.document(first.base_doc_id).unwrap();
        assert_eq!(
            view_diff_base_text(&app, first.working_doc_id, first.working_view_id).as_deref(),
            Some(base.text().to_string().as_str())
        );

        assert!(app.editor.close_diff_view(first.working_view_id));
        assert!(app.editor.document(first.base_doc_id).is_none());
        if target.is_some() {
            assert!(app.editor.document(first.working_doc_id).is_none());
        }
    }

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_diff_keeps_snapshots_used_by_a_suspended_diff() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let older = repo.rev_parse("HEAD")?;
    repo.commit_all("second")?;
    let newer = repo.rev_parse("HEAD")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    app.editor
        .open_diff_view_range(&tracked, &older, Some(&newer), Some(false))?;
    let first = main_diff_state(&app);
    app.editor
        .switch(first.working_doc_id, Action::VerticalSplit);
    app.editor
        .open_diff_view_range(&tracked, &older, Some(&newer), Some(false))?;
    let second = main_diff_state(&app);

    app.editor.focus(first.working_view_id);
    open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
    assert_eq!(app.editor.diff.suspended.len(), 1);
    app.editor.focus(second.working_view_id);
    assert!(app.editor.close_diff_view(second.working_view_id));
    assert!(app.editor.document(first.base_doc_id).is_some());
    assert!(app.editor.document(first.working_doc_id).is_some());

    app.editor.focus(first.working_view_id);
    assert!(harness.send_keys(&mut app, "ga").await?);
    let restored = main_diff_state(&app);
    assert_eq!(restored.base_doc_id, first.base_doc_id);
    assert_eq!(restored.working_doc_id, first.working_doc_id);
    assert!(app.editor.close_diff_view(first.working_view_id));
    assert!(app.editor.document(first.base_doc_id).is_none());
    assert!(app.editor.document(first.working_doc_id).is_none());

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_comparisons_can_toggle_to_a_split_diff() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let older = repo.rev_parse("HEAD")?;
    repo.commit_all("second")?;
    let newer = repo.rev_parse("HEAD")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    for target in [None, Some(newer.as_str())] {
        app.editor
            .open_diff_view_range(&tracked, &older, target, Some(false))?;
        let first = main_diff_state(&app);
        app.editor
            .switch(first.working_doc_id, Action::VerticalSplit);
        app.editor
            .open_diff_view_range(&tracked, &older, target, Some(false))?;

        let views_before_split = app.editor.tree.views().count();
        assert!(harness.send_keys(&mut app, "<space>mv").await?);
        let split = main_diff_state(&app);
        assert!(split.is_split());
        assert_eq!(app.editor.tree.views().count(), views_before_split + 1);
        assert!(harness.wait_for_idle(&mut app).await?);
        let base = app.editor.document(split.base_doc_id).unwrap();
        assert_eq!(
            diff_base_text(&app, split.working_doc_id).as_deref(),
            Some(base.text().to_string().as_str())
        );
        assert!(app.editor.close_diff_view(split.working_view_id));
    }

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn split_buffer_diff_keeps_revisions_from_other_comparisons() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let older = repo.rev_parse("HEAD")?;
    repo.commit_all("second")?;
    let newer = repo.rev_parse("HEAD")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    app.editor
        .open_diff_view_range(&tracked, &older, Some(&newer), Some(false))?;
    let first = main_diff_state(&app);
    app.editor
        .switch(first.working_doc_id, Action::VerticalSplit);
    app.editor
        .open_buffer_diff_view(first.base_doc_id, first.working_doc_id, Some(true))?;

    let split = main_diff_state(&app);
    assert!(split.is_split());
    assert_eq!(split.base_doc_id, first.base_doc_id);
    assert_eq!(split.working_doc_id, first.working_doc_id);
    assert!(harness.wait_for_idle(&mut app).await?);
    let base = app.editor.document(split.base_doc_id).unwrap();
    assert_eq!(
        diff_base_text(&app, split.working_doc_id).as_deref(),
        Some(base.text().to_string().as_str())
    );

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn diff_reset_forgets_suspended_comparisons() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let older = repo.rev_parse("HEAD")?;
    repo.commit_all("second")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    for keys in [":diff-reset<ret>", "<space>mr"] {
        app.editor.open_diff_view(&tracked, &older, Some(false))?;
        open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
        assert_eq!(app.editor.diff.suspended.len(), 1);

        assert!(harness.send_keys(&mut app, keys).await?);
        assert!(app.editor.diff.suspended.is_empty());
        assert!(harness.send_keys(&mut app, "<C-o>").await?);
        assert_current_doc_path(&app, &tracked);
        assert!(app.editor.diff.views.is_empty());
        assert!(
            !app.editor
                .document(app.editor.tree.get(app.editor.tree.focus).doc)
                .unwrap()
                .char_diff_enabled
        );
    }

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn navigating_from_an_empty_scratch_diff_preserves_its_buffer() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text("base#[|\n]#").build()?;
    let base = app.editor.tree.get(app.editor.tree.focus).doc;
    let working = app.editor.new_file(Action::Replace);
    app.editor
        .open_buffer_diff_view(base, working, Some(true))?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, ":new<ret>").await?);
    assert!(app.editor.document(working).is_some());
    assert!(harness.send_keys(&mut app, "ga").await?);
    let restored = main_diff_state(&app);
    assert_eq!(restored.base_doc_id, base);
    assert_eq!(restored.working_doc_id, working);
    assert!(restored.is_split());

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

/// How many diff markers a view's gutter shows. The diff gutter is the last
/// column of the layout used here.
fn diff_gutter_marks(app: &Application, view_id: helix_view::ViewId) -> usize {
    let view = app.editor.tree.get(view_id);
    let doc = app.editor.document(view.doc).unwrap();
    let x = view.area.left() + view.gutter_offset(doc) - 1;
    (view.area.top()..view.area.bottom())
        .filter(|&y| {
            matches!(
                app.screen().get(x, y).map(|cell| cell.symbol.as_str()),
                Some("▍" | "▔" | "▁")
            )
        })
        .count()
}

/// Auto-reload is meant for files that change on disk. A revision's snapshot is
/// read from a cache file that is written after the snapshot is loaded, so it
/// looks changed, and reloading it dropped the diff its gutter shows. The left
/// pane's markers went out as soon as it got the focus.
#[tokio::test(flavor = "multi_thread")]
async fn auto_reload_leaves_the_base_pane_gutter_alone() -> anyhow::Result<()> {
    use helix_view::editor::GutterType;

    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut config = Config::default();
    config.editor.auto_reload = true;
    config.editor.gutters = vec![
        GutterType::Diagnostics,
        GutterType::Spacer,
        GutterType::Diff,
    ]
    .into();
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(repo.file("tracked.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();

    open_changed_file_diff(&mut app, &mut harness, true).await?;
    let diff_state = main_diff_state(&app);
    assert!(diff_gutter_marks(&app, diff_state.base_view_id) > 0);

    // The cache file is stamped after the document loads, by however much the
    // filesystem's clock allows. Make it unmistakable.
    let cache_file = app
        .editor
        .document(diff_state.base_doc_id)
        .unwrap()
        .path()
        .unwrap()
        .to_owned();
    std::fs::OpenOptions::new()
        .write(true)
        .open(cache_file)?
        .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))?;

    assert!(harness.send_keys(&mut app, "<C-w><C-w>").await?);
    assert_eq!(app.editor.tree.focus, diff_state.base_view_id);
    assert!(harness.wait_for_idle(&mut app).await?);

    assert!(
        diff_gutter_marks(&app, diff_state.base_view_id) > 0,
        "the base pane lost its diff markers"
    );
    assert!(diff_gutter_marks(&app, diff_state.working_view_id) > 0);

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

/// `:diff-buffer` in a split's base pane compares that pane's revision. The
/// old diff closed the revision on its way out, and the new one then read a
/// buffer that was gone and panicked.
#[tokio::test(flavor = "multi_thread")]
async fn diff_buffer_from_the_base_pane_keeps_its_revision() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let other = repo.file("other.txt");
    let mut app = AppBuilder::new()
        .with_file(repo.file("tracked.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();

    open_changed_file_diff(&mut app, &mut harness, true).await?;
    let other_doc_id = app.editor.open(&other, Action::Load)?;
    let before = main_diff_state(&app);
    app.editor.focus(before.base_view_id);

    let command = format!(":diff-buffer {}<ret>", other.display());
    assert!(harness.send_keys(&mut app, &command).await?);
    let diff_state = main_diff_state(&app);
    assert_eq!(diff_state.base_doc_id, other_doc_id);
    assert_eq!(diff_state.working_doc_id, before.base_doc_id);
    assert!(app.editor.document(before.base_doc_id).is_some());
    assert_eq!(app.editor.diff.views.len(), 2);

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

/// The base pane's jumplist holds the working file it showed before the diff
/// opened. Jumping there showed the working file in both panes, with the left
/// one still drawn as the base. It leaves the diff instead, and jumping
/// forward again brings it back.
#[tokio::test(flavor = "multi_thread")]
async fn jumping_the_base_pane_to_the_working_file_leaves_the_diff() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("tracked.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();

    open_changed_file_diff(&mut app, &mut harness, true).await?;
    let before = main_diff_state(&app);
    app.editor.focus(before.base_view_id);

    assert!(harness.send_keys(&mut app, "<C-o>").await?);
    assert_eq!(
        app.editor.tree.get(before.base_view_id).doc,
        before.working_doc_id
    );
    assert!(
        app.editor.diff.views.is_empty(),
        "a pane showing the working file is not the base pane"
    );
    assert_eq!(app.editor.tree.views().count(), 2);

    assert!(harness.send_keys(&mut app, "<C-i>").await?);
    assert_eq!(app.editor.diff.views.len(), 2, "the diff should be back");
    let diff_state = main_diff_state(&app);
    assert_eq!(diff_state.base_view_id, before.base_view_id);
    assert_eq!(
        app.editor.tree.get(before.base_view_id).doc,
        before.base_doc_id
    );

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

/// `:bc` on the revision in a split's base pane used to put the working file in
/// that pane while it stayed registered as the base. Closing the revision ends
/// the diff and leaves the working file in one plain pane.
#[tokio::test(flavor = "multi_thread")]
async fn closing_the_base_revision_ends_the_split_diff() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("tracked.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();

    open_changed_file_diff(&mut app, &mut harness, true).await?;
    let before = main_diff_state(&app);
    app.editor.focus(before.base_view_id);

    assert!(harness.send_keys(&mut app, ":bc<ret>").await?);
    assert!(app.editor.diff.views.is_empty());
    assert!(app.editor.document(before.base_doc_id).is_none());
    assert!(app
        .editor
        .tree
        .views()
        .all(|(view, _)| view.doc == before.working_doc_id));
    let working_doc = app.editor.document(before.working_doc_id).unwrap();
    assert!(!working_doc.char_diff_enabled);
    assert!(working_doc.linked_diff_doc.is_none());

    harness.close(&mut app).await?;
    Ok(())
}

/// `:bc` on the file of a single-pane diff falls back to the buffer shown
/// before it. That buffer used to inherit the diff: its spacers, gutter and
/// `]c`, and `<space>mv` reopened the closed file's diff.
#[tokio::test(flavor = "multi_thread")]
async fn closing_the_working_file_ends_a_single_pane_diff() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let other = repo.file("other.txt");
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&other, None).build()?;
    let mut harness = AppTestHarness::new();

    open_other(&mut app, &mut harness, &tracked).await?;
    open_changed_file_diff(&mut app, &mut harness, false).await?;
    let before = main_diff_state(&app);
    assert!(!before.is_split());

    assert!(harness.send_keys(&mut app, ":bc<ret>").await?);
    assert_current_doc_path(&app, &other);
    assert!(app.editor.diff.views.is_empty());
    assert!(
        app.editor.document(before.base_doc_id).is_none(),
        "the revision goes with the diff"
    );

    harness.close(&mut app).await?;
    Ok(())
}

fn diff_base_text(app: &Application, doc_id: helix_view::DocumentId) -> Option<String> {
    let view_id =
        app.editor
            .diff
            .views
            .get(&app.editor.tree.focus)
            .map_or(app.editor.tree.focus, |state| {
                if state.base_doc_id == doc_id {
                    state.base_view_id
                } else {
                    state.working_view_id
                }
            });
    view_diff_base_text(app, doc_id, view_id)
}

fn view_diff_base_text(
    app: &Application,
    doc_id: helix_view::DocumentId,
    view_id: helix_view::ViewId,
) -> Option<String> {
    let doc = app.editor.document(doc_id).unwrap();
    doc.diff_handle_for_view(view_id)
        .map(|handle| handle.load().diff_base().to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn comparisons_of_one_buffer_keep_their_own_baselines_and_edits() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    let older_text = "first\nold-a\nmiddle\nold-b\nlast\n";
    let head_text = "first\nnew-a\nmiddle\nold-b\nlast\n";
    let working_text = "first\nnew-a\nmiddle\nnew-b\nlast\n";
    repo.write_file("tracked.txt", older_text)?;
    repo.commit_all("first")?;
    let older = repo.rev_parse("HEAD")?;
    repo.write_file("tracked.txt", head_text)?;
    repo.commit_all("second")?;
    repo.write_file("tracked.txt", working_text)?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    app.editor.open_diff_view(&tracked, &older, Some(false))?;
    let first = main_diff_state(&app);
    app.editor
        .switch(first.working_doc_id, Action::VerticalSplit);
    app.editor.open_diff_view(&tracked, "HEAD", Some(false))?;
    let second = main_diff_state(&app);
    app.editor
        .switch(first.working_doc_id, Action::HorizontalSplit);
    let plain_view = app.editor.tree.focus;
    assert!(harness.wait_for_idle(&mut app).await?);

    assert_eq!(
        view_diff_base_text(&app, first.working_doc_id, first.working_view_id).as_deref(),
        Some(older_text)
    );
    assert_eq!(
        view_diff_base_text(&app, first.working_doc_id, second.working_view_id).as_deref(),
        Some(head_text)
    );
    assert_eq!(
        view_diff_base_text(&app, first.working_doc_id, plain_view).as_deref(),
        Some(head_text)
    );
    assert!(!app
        .editor
        .document(first.working_doc_id)
        .unwrap()
        .char_diff_enabled_for_view(plain_view));

    app.editor.focus(first.working_view_id);
    assert!(
        harness
            .send_keys(&mut app, ":toggle-char-diff<ret>")
            .await?
    );
    let doc = app.editor.document(first.working_doc_id).unwrap();
    assert!(!doc.char_diff_enabled_for_view(first.working_view_id));
    assert!(doc.char_diff_enabled_for_view(second.working_view_id));
    assert!(harness.send_keys(&mut app, "gg]gmig").await?);
    assert_eq!(current_cursor_line(&app), 1);
    let (view, doc) = helix_view::current_ref!(app.editor);
    let selection = doc.selection(view.id).primary();
    assert_eq!(
        doc.text()
            .slice(selection.from()..selection.to())
            .to_string(),
        "new-a\n"
    );

    app.editor.focus(second.working_view_id);
    assert!(harness.send_keys(&mut app, "gg]g").await?);
    assert_eq!(current_cursor_line(&app), 3);
    app.editor.focus(first.working_view_id);
    assert!(
        harness
            .send_keys(&mut app, ":reset-diff-change<ret>")
            .await?
    );
    let edited = "first\nold-a\nmiddle\nnew-b\nlast\n";
    assert_eq!(current_doc_text(&app), edited);
    assert!(harness.wait_for_idle(&mut app).await?);
    for view in [first.working_view_id, second.working_view_id, plain_view] {
        assert_eq!(
            app.editor
                .document(first.working_doc_id)
                .unwrap()
                .diff_handle_for_view(view)
                .unwrap()
                .load()
                .doc()
                .to_string(),
            edited
        );
    }

    assert!(app.editor.close_diff_view(second.working_view_id));
    assert_eq!(
        view_diff_base_text(&app, first.working_doc_id, first.working_view_id).as_deref(),
        Some(older_text)
    );
    assert_eq!(
        view_diff_base_text(&app, first.working_doc_id, plain_view).as_deref(),
        Some(head_text)
    );
    app.editor.focus(first.working_view_id);
    assert!(harness.send_keys(&mut app, "gg]g").await?);
    assert_eq!(current_cursor_line(&app), 3);

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_update_comparisons_using_a_buffer_on_opposite_sides() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    let c = dir.path().join("c.txt");
    std::fs::write(&a, "a\n")?;
    std::fs::write(&b, "b\n")?;
    std::fs::write(&c, "c\n")?;
    let mut app = AppBuilder::new().with_file(&a, None).build()?;
    let mut harness = AppTestHarness::new();
    let a_id = app.editor.tree.get(app.editor.tree.focus).doc;
    let b_id = app.editor.open(&b, Action::Load)?;
    let c_id = app.editor.open(&c, Action::Load)?;
    app.editor.open_buffer_diff_view(b_id, a_id, Some(false))?;
    let first = main_diff_state(&app);
    app.editor.switch(b_id, Action::VerticalSplit);
    app.editor.open_buffer_diff_view(c_id, b_id, Some(false))?;
    let second = main_diff_state(&app);

    assert!(harness.send_keys(&mut app, "ggirevised-<esc>").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert_eq!(
        view_diff_base_text(&app, a_id, first.working_view_id).as_deref(),
        Some("revised-b\n")
    );
    assert_eq!(
        view_diff_base_text(&app, b_id, second.working_view_id).as_deref(),
        Some("c\n")
    );
    assert_eq!(
        app.editor
            .document(b_id)
            .unwrap()
            .diff_handle_for_view(second.working_view_id)
            .unwrap()
            .load()
            .doc()
            .to_string(),
        "revised-b\n"
    );
    assert!(app.editor.close_diff_view(first.working_view_id));
    assert!(harness.send_keys(&mut app, "ggimore-<esc>").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert_eq!(
        app.editor
            .document(b_id)
            .unwrap()
            .diff_handle_for_view(second.working_view_id)
            .unwrap()
            .load()
            .doc()
            .to_string(),
        "more-revised-b\n"
    );

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn suspending_one_comparison_keeps_the_other_baseline() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let older = repo.rev_parse("HEAD")?;
    repo.commit_all("second")?;
    let head_text = std::fs::read_to_string(repo.file("tracked.txt"))?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();
    app.editor.open_diff_view(&tracked, &older, Some(false))?;
    let first = main_diff_state(&app);
    let old_text = app
        .editor
        .document(first.base_doc_id)
        .unwrap()
        .text()
        .to_string();
    app.editor
        .switch(first.working_doc_id, Action::VerticalSplit);
    app.editor.open_diff_view(&tracked, "HEAD", Some(false))?;
    let second = main_diff_state(&app);
    app.editor.focus(first.working_view_id);
    open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
    assert_eq!(
        view_diff_base_text(&app, first.working_doc_id, second.working_view_id).as_deref(),
        Some(head_text.as_str())
    );
    assert!(harness.send_keys(&mut app, "ga").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert_eq!(
        view_diff_base_text(&app, first.working_doc_id, first.working_view_id).as_deref(),
        Some(old_text.as_str())
    );
    assert_eq!(
        view_diff_base_text(&app, first.working_doc_id, second.working_view_id).as_deref(),
        Some(head_text.as_str())
    );

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_diff_open_preserves_the_current_comparison() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let older = repo.rev_parse("HEAD")?;
    repo.commit_all("second")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    for (split, base_focused) in [(false, false), (true, false), (true, true)] {
        app.editor.open_diff_view(&tracked, "HEAD", Some(split))?;
        let before = main_diff_state(&app);
        if base_focused {
            app.editor.focus(before.base_view_id);
        }
        assert!(harness.send_keys(&mut app, "40gg").await?);
        let focus = app.editor.tree.focus;
        let views: Vec<_> = app
            .editor
            .tree
            .views()
            .map(|(view, _)| (view.id, view.doc, view.area))
            .collect();
        let documents: Vec<_> = app.editor.documents.keys().copied().collect();

        for failure in 0..4 {
            let result = match failure {
                0 => app
                    .editor
                    .open_diff_view(&tracked, "missing-ref", Some(!split)),
                1 => app.editor.open_diff_view_range(
                    &tracked,
                    &older,
                    Some("missing-ref"),
                    Some(!split),
                ),
                2 => app.editor.open_diff_view_range(
                    &tracked,
                    "missing-ref",
                    Some(&older),
                    Some(!split),
                ),
                _ => app.editor.open_diff_view_with_paths(
                    &tracked,
                    repo.path(),
                    &older,
                    Some(!split),
                ),
            };
            assert!(result.is_err(), "failure case {failure}");
            assert_eq!(app.editor.tree.focus, focus);
            assert_eq!(
                app.editor
                    .tree
                    .views()
                    .map(|(view, _)| (view.id, view.doc, view.area))
                    .collect::<Vec<_>>(),
                views
            );
            assert_eq!(
                app.editor.documents.keys().copied().collect::<Vec<_>>(),
                documents
            );
            let after = main_diff_state(&app);
            assert_eq!(after.base_doc_id, before.base_doc_id);
            assert_eq!(after.working_doc_id, before.working_doc_id);
            assert_eq!(after.is_split(), split);
            assert_eq!(current_cursor_line(&app), 39);
            assert!(app.editor.diff.suspended.is_empty());
        }
    }

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_diff_open_keeps_a_suspended_comparison() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();
    app.editor.open_diff_view(&tracked, "HEAD", Some(true))?;
    let before = main_diff_state(&app);
    open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
    assert!(app
        .editor
        .open_diff_view(&tracked, "missing-ref", Some(false))
        .is_err());
    assert_eq!(app.editor.diff.suspended.len(), 1);
    assert!(harness.send_keys(&mut app, "ga").await?);
    let restored = main_diff_state(&app);
    assert_eq!(restored.working_view_id, before.working_view_id);
    assert_eq!(restored.working_doc_id, before.working_doc_id);
    assert!(restored.is_split());

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_split_toggle_keeps_the_original_comparison() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    repo.git(&["branch", "review-base"])?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();
    app.editor
        .open_diff_view(&tracked, "review-base", Some(false))?;
    let before = main_diff_state(&app);
    let preference = app.editor.diff.split_view_override;
    repo.git(&["branch", "-D", "review-base"])?;

    assert!(harness.send_keys(&mut app, "<space>mv").await?);
    let after = main_diff_state(&app);
    assert_eq!(after.base_doc_id, before.base_doc_id);
    assert_eq!(after.working_doc_id, before.working_doc_id);
    assert!(!after.is_split());
    assert_eq!(app.editor.diff.split_view_override, preference);
    assert_eq!(*app.editor.get_status().unwrap().1, Severity::Error);
    assert_eq!(app.editor.tree.views().count(), 1);

    harness.close(&mut app).await?;
    Ok(())
}

/// A buffer diff takes over the file's own diff against HEAD. Ending it
/// dropped the diff altogether, so the gutter stayed blank until `:reload`.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_gets_its_own_diff_back_after_a_buffer_diff() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let other = repo.file("other.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();
    let tracked_doc_id = app.editor.tree.get(app.editor.tree.focus).doc;
    let head: String = (0..60).map(|n| format!("line {n}\n")).collect();
    app.editor.open(&other, Action::Load)?;

    let command = format!(":diff-buffer {}<ret>", other.display());
    assert!(harness.send_keys(&mut app, &command).await?);
    assert_eq!(app.editor.diff.views.len(), 2);
    assert_eq!(
        diff_base_text(&app, tracked_doc_id).as_deref(),
        Some("other\n")
    );

    let diff_state = main_diff_state(&app);
    app.editor.focus(diff_state.working_view_id);
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    assert!(app.editor.diff.views.is_empty());
    assert_eq!(diff_base_text(&app, tracked_doc_id), Some(head));

    harness.close(&mut app).await?;
    Ok(())
}

/// Leaving a diff against an older commit left the file diffed against that
/// commit, with nothing on screen to say so, and the revision kept its
/// character-diff colouring.
#[tokio::test(flavor = "multi_thread")]
async fn leaving_a_diff_against_an_older_commit_goes_back_to_head() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\n")?;
    repo.write_file("other.txt", "other\n")?;
    repo.commit_all("initial")?;
    let older = repo.rev_parse("HEAD")?;
    repo.write_file("tracked.txt", "two\n")?;
    repo.commit_all("second")?;
    repo.write_file("tracked.txt", "three\n")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    app.editor.open_diff_view(&tracked, &older, Some(true))?;
    let diff_state = main_diff_state(&app);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert_eq!(
        diff_base_text(&app, diff_state.working_doc_id).as_deref(),
        Some("one\n")
    );

    open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
    assert!(app.editor.diff.views.is_empty());
    assert_eq!(
        diff_base_text(&app, diff_state.working_doc_id).as_deref(),
        Some("two\n")
    );
    let revision = app.editor.document(diff_state.base_doc_id).unwrap();
    assert!(!revision.char_diff_enabled);
    assert!(!revision.char_diff_minus_side);
    assert!(revision.diff_handle().is_none());

    // Jumping back still finds the diff, against the same commit.
    assert!(harness.send_keys(&mut app, "<C-o>").await?);
    assert_eq!(app.editor.diff.views.len(), 2);
    assert_eq!(
        diff_base_text(&app, diff_state.working_doc_id).as_deref(),
        Some("one\n")
    );

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

/// A snapshot of `HEAD` stays open when its diff is left, to be jumped back to.
/// Once `HEAD` moved on, the jump back, or diffing the file again, reused it
/// and compared the file with the commit before.
#[tokio::test(flavor = "multi_thread")]
async fn a_diff_against_head_follows_a_new_commit() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    open_changed_file_diff(&mut app, &mut harness, true).await?;
    let before = main_diff_state(&app);
    let old_head = repo.rev_parse("HEAD")?;
    open_other(&mut app, &mut harness, &repo.file("other.txt")).await?;
    assert!(app.editor.document(before.base_doc_id).is_some());

    repo.commit_all("second")?;
    let new_head = repo.rev_parse("HEAD")?;
    assert_ne!(old_head, new_head);

    assert!(harness.send_keys(&mut app, "<C-o>").await?);
    assert_eq!(app.editor.diff.views.len(), 2, "the diff should be back");
    let diff_state = main_diff_state(&app);
    assert_ne!(diff_state.base_doc_id, before.base_doc_id);
    let base = app.editor.document(diff_state.base_doc_id).unwrap();
    assert_eq!(
        base.git_revision.as_ref().unwrap().commit.as_deref(),
        Some(new_head.as_str())
    );
    assert!(base.text().to_string().contains("changed\n"));
    assert!(
        app.editor.document(before.base_doc_id).is_none(),
        "the snapshot of the old HEAD is gone"
    );
    assert_ne!(
        base.path(),
        None,
        "the new snapshot has a cache file of its own"
    );

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

/// Reloading the file of a diff against an older commit, as `:reload` or
/// auto-reload do after an outside edit, rebased the worker both panes share
/// on HEAD. The base pane still showed the older commit, while both gutters
/// and the spacers were worked out against HEAD.
#[tokio::test(flavor = "multi_thread")]
async fn reloading_a_diffed_file_keeps_the_diff_s_base() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\n")?;
    repo.commit_all("initial")?;
    let older = repo.rev_parse("HEAD")?;
    repo.write_file("tracked.txt", "two\n")?;
    repo.commit_all("second")?;
    repo.write_file("tracked.txt", "three\n")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();

    app.editor.open_diff_view(&tracked, &older, Some(true))?;
    let diff_state = main_diff_state(&app);
    app.editor.focus(diff_state.working_view_id);

    repo.write_file("tracked.txt", "four\n")?;
    assert!(harness.send_keys(&mut app, ":reload<ret>").await?);
    assert_eq!(current_doc_text(&app), "four\n");
    assert_eq!(app.editor.diff.views.len(), 2);
    assert_eq!(
        diff_base_text(&app, diff_state.working_doc_id).as_deref(),
        Some("one\n"),
        "the diff is still against the older commit"
    );

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

/// A file outside any repository has no base of its own, so reloading it in
/// a buffer diff dropped the diff altogether.
#[tokio::test(flavor = "multi_thread")]
async fn reloading_a_file_outside_a_repository_keeps_its_buffer_diff() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let working = dir.path().join("working.txt");
    let base = dir.path().join("base.txt");
    std::fs::write(&working, "working\n")?;
    std::fs::write(&base, "base\n")?;

    let mut app = AppBuilder::new().with_file(&working, None).build()?;
    let mut harness = AppTestHarness::new();
    let working_doc_id = app.editor.tree.get(app.editor.tree.focus).doc;
    app.editor.open(&base, Action::Load)?;

    let command = format!(":diff-buffer {}<ret>", base.display());
    assert!(harness.send_keys(&mut app, &command).await?);
    let diff_state = main_diff_state(&app);
    assert_eq!(diff_state.working_doc_id, working_doc_id);
    app.editor.focus(diff_state.working_view_id);

    std::fs::write(&working, "changed\n")?;
    assert!(harness.send_keys(&mut app, ":reload<ret>").await?);
    assert_eq!(current_doc_text(&app), "changed\n");
    assert_eq!(
        diff_base_text(&app, working_doc_id).as_deref(),
        Some("base\n"),
        "the buffer diff is still there"
    );

    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

/// A `:diff-base` ref outlasts a reload, as it outlasts a diff, rather than
/// going back to HEAD while the status still names the ref.
#[tokio::test(flavor = "multi_thread")]
async fn reloading_keeps_the_diff_base_ref() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\n")?;
    repo.commit_all("initial")?;
    let older = repo.rev_parse("HEAD")?;
    repo.write_file("tracked.txt", "two\n")?;
    repo.commit_all("second")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let tracked = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&tracked, None).build()?;
    let mut harness = AppTestHarness::new();
    let doc_id = app.editor.tree.get(app.editor.tree.focus).doc;

    let command = format!(":diff-base {older}<ret>");
    assert!(harness.send_keys(&mut app, &command).await?);
    assert_eq!(diff_base_text(&app, doc_id).as_deref(), Some("one\n"));

    repo.write_file("tracked.txt", "three\n")?;
    assert!(harness.send_keys(&mut app, ":reload<ret>").await?);
    assert_eq!(diff_base_text(&app, doc_id).as_deref(), Some("one\n"));
    let doc = app.editor.document(doc_id).unwrap();
    assert_eq!(doc.diff_base_ref(), Some(older.as_str()));

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn comparisons_preserve_neighboring_unsaved_buffers() -> anyhow::Result<()> {
    let repo = conflicted_repo()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("conflict.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    let original_view = app.editor.tree.focus;
    let scratch = app.editor.new_file(Action::VerticalSplit);
    let neighbor = app.editor.tree.focus;
    assert!(
        harness
            .send_keys(&mut app, "iunsaved neighbor<esc>")
            .await?
    );
    let expected = current_doc_text(&app);
    app.editor.focus(original_view);
    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    assert_eq!(app.editor.tree.views().count(), 4);
    assert_eq!(app.editor.tree.get(neighbor).doc, scratch);
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    assert_eq!(app.editor.tree.views().count(), 2);
    assert!(harness.send_keys(&mut app, ":bc<ret>").await?);
    assert_eq!(
        app.editor.document(scratch).unwrap().text().to_string(),
        expected
    );
    assert!(app.editor.document(scratch).unwrap().is_modified());
    assert!(app.editor.tree.contains(neighbor));
    assert!(harness.send_keys(&mut app, "<C-w>o").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_ranges_preserve_the_previous_comparison_scope() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    repo.commit_all("second")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("tracked.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(
        harness
            .send_keys(&mut app, ":diff-commit HEAD~1..HEAD<ret>")
            .await?
    );
    assert_diff_range(&app, "HEAD~1", Some("HEAD"));
    for invalid in ["nosuchref", "HEAD~99", "main...nosuchref"] {
        assert!(
            harness
                .send_keys(&mut app, &format!(":diff-commit {invalid}<ret>"))
                .await?
        );
        assert!(app.editor.is_err());
        assert_diff_range(&app, "HEAD~1", Some("HEAD"));
    }
    assert!(
        harness
            .send_keys(&mut app, ":diff-commit HEAD^!<ret>")
            .await?
    );
    assert_diff_range(&app, "HEAD^", Some("HEAD"));
    assert!(
        harness
            .send_keys(&mut app, ":diff-commit HEAD~1...HEAD<ret>")
            .await?
    );
    assert_diff_range(&app, &repo.rev_parse("HEAD~1")?, Some("HEAD"));
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn next_file_and_picker_follow_original_paths_in_ranges() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    for name in ["a.txt", "b.txt", "c.txt"] {
        repo.write_file(name, "one\n")?;
    }
    repo.commit_all("base")?;
    for name in ["a.txt", "b.txt", "c.txt"] {
        repo.write_file(name, "two\n")?;
    }
    repo.commit_all("changes")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("a.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(
        harness
            .send_keys(&mut app, ":diff-commit HEAD^!<ret><space>g")
            .await?
    );
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>]F").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    let doc = app
        .editor
        .document(main_diff_state(&app).working_doc_id)
        .unwrap();
    assert_eq!(doc.git_revision.as_ref().unwrap().path, repo.file("b.txt"));
    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);
    let doc = app
        .editor
        .document(main_diff_state(&app).working_doc_id)
        .unwrap();
    assert_eq!(doc.git_revision.as_ref().unwrap().path, repo.file("b.txt"));
    assert!(harness.send_keys(&mut app, "[F").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    let doc = app
        .editor
        .document(main_diff_state(&app).working_doc_id)
        .unwrap();
    assert_eq!(doc.git_revision.as_ref().unwrap().path, repo.file("a.txt"));
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn moving_head_flags_active_comparisons_until_refresh() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/example/project.git",
    ])?;
    let old_head = repo.rev_parse("HEAD")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut config = test_config();
    config.editor.clipboard_provider = helix_view::clipboard::ClipboardProvider::None;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_file(repo.file("tracked.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    app.editor
        .open_diff_view(&repo.file("tracked.txt"), "HEAD", Some(true))?;
    let before = main_diff_state(&app);
    let result_before = current_doc_text(&app);
    repo.commit_all("second")?;
    app.editor.refresh_comparison_state();
    assert!(main_diff_state(&app).stale);
    app.editor.focus(before.base_view_id);
    assert!(harness.send_keys(&mut app, ":copy-path url<ret>").await?);
    let url = app
        .editor
        .registers
        .first('+', &app.editor)
        .unwrap()
        .into_owned();
    assert!(url.contains(&format!("/{old_head}/tracked.txt")), "{url}");
    app.editor.focus(before.working_view_id);
    assert!(app
        .editor
        .comparison_label(before.working_view_id)
        .contains("STALE"));
    assert!(harness.send_keys(&mut app, "<space>md").await?);
    assert!(app.editor.is_err());
    assert_eq!(current_doc_text(&app), result_before);
    assert!(harness.send_keys(&mut app, ":diff-refresh<ret>").await?);
    let after = main_diff_state(&app);
    assert!(!after.stale);
    assert_eq!(
        app.editor
            .document(after.base_doc_id)
            .unwrap()
            .git_revision
            .as_ref()
            .unwrap()
            .commit
            .as_deref(),
        Some(repo.rev_parse("HEAD")?.as_str())
    );
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn aborting_a_merge_blocks_stale_accepts_and_keeps_result_edits() -> anyhow::Result<()> {
    let repo = conflicted_repo()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("conflict.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    let expected = current_doc_text(&app);
    repo.git(&["merge", "--abort"])?;
    assert!(harness.send_keys(&mut app, "<space>mo").await?);
    assert!(app.editor.is_err());
    assert_eq!(current_doc_text(&app), expected);
    let state = app
        .editor
        .diff
        .merge_views
        .get(&app.editor.tree.focus)
        .unwrap();
    assert!(state.stale);
    assert!(harness.send_keys(&mut app, ":diff-refresh<ret>").await?);
    assert!(app.editor.is_err());
    assert!(!app.editor.diff.merge_views.is_empty());
    assert_eq!(current_doc_text(&app), expected);
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn diff3_base_can_be_viewed_and_accepted_from_any_pane() -> anyhow::Result<()> {
    let repo = conflicted_repo()?;
    repo.git(&["checkout", "--conflict=diff3", "--", "conflict.txt"])?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("conflict.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    let original = current_doc_text(&app);
    let state = app
        .editor
        .diff
        .merge_views
        .get(&app.editor.tree.focus)
        .unwrap()
        .clone();
    assert!(harness.send_keys(&mut app, "<space>mB").await?);
    assert_eq!(current_doc_text(&app), "base\n");
    assert!(app
        .editor
        .comparison_label(app.editor.tree.focus)
        .contains("BASE"));
    assert!(harness.send_keys(&mut app, "<space>ma").await?);
    assert_eq!(
        app.editor
            .document(state.result_doc_id)
            .unwrap()
            .text()
            .to_string(),
        "base\n"
    );
    app.editor.focus(state.result_view_id);
    assert!(harness.send_keys(&mut app, "u").await?);
    assert_eq!(current_doc_text(&app), original);
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    assert_eq!(app.editor.tree.views().count(), 1);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_modify_conflicts_can_stage_an_explicit_deletion() -> anyhow::Result<()> {
    let repo = conflicted_repo()?;
    repo.git(&["merge", "--abort"])?;
    repo.git(&["rm", "conflict.txt"])?;
    repo.commit_all("delete")?;
    repo.merge_expect_conflict("feature")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("conflict.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":merge<ret><space>mo").await?);
    assert!(!app.editor.is_err(), "{:?}", app.editor.get_status());
    assert!(harness.send_keys(&mut app, "<space>mf").await?);
    assert!(!app.editor.is_err(), "{:?}", app.editor.get_status());
    assert!(!repo.file("conflict.txt").exists());
    assert!(repo.unmerged_entries()?.is_empty());
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn finish_refuses_to_stage_an_ordinary_modified_file() -> anyhow::Result<()> {
    let repo = repo_with_a_file_to_jump_to()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("tracked.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    let index_before = repo.git_output(&["write-tree"])?;
    assert!(harness.send_keys(&mut app, "<space>mf").await?);
    assert!(app.editor.is_err());
    assert_eq!(repo.git_output(&["write-tree"])?, index_before);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn short_custom_markers_cannot_bypass_the_finish_guard() -> anyhow::Result<()> {
    let repo = conflicted_repo()?;
    repo.write_file(".gitattributes", "*.txt conflict-marker-size=3\n")?;
    repo.git(&["checkout", "--conflict=merge", "--", "conflict.txt"])?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("conflict.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    let state = app
        .editor
        .diff
        .merge_views
        .get(&app.editor.tree.focus)
        .unwrap();
    assert_eq!(state.marker_size, 3);
    let entries = repo.unmerged_entries()?;
    let (view, doc) = helix_view::current!(app.editor);
    let tx = Transaction::change(
        doc.text(),
        [(
            0,
            doc.text().len_chars(),
            Some("===\ntheirs\n>>> incoming\n".into()),
        )]
        .into_iter(),
    );
    assert!(doc.apply(&tx, view.id));
    assert!(harness.send_keys(&mut app, "<space>mf").await?);
    assert!(app.editor.is_err());
    assert_eq!(repo.unmerged_entries()?, entries);
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn rebase_panes_name_upstream_and_the_replayed_commit() -> anyhow::Result<()> {
    let repo = conflicted_repo()?;
    repo.git(&["merge", "--abort"])?;
    repo.checkout("feature")?;
    assert!(repo.git(&["rebase", "main"]).is_err());
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("conflict.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    let state = app
        .editor
        .diff
        .merge_views
        .get(&app.editor.tree.focus)
        .unwrap();
    assert!(
        state.ours_label.contains("upstream HEAD"),
        "{}",
        state.ours_label
    );
    assert!(
        state.theirs_label.contains("replayed commit"),
        "{}",
        state.theirs_label
    );
    assert!(harness.send_keys(&mut app, "<space>mt").await?);
    assert_eq!(current_doc_text(&app), "theirs\n");
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn merge_reference_panes_follow_a_distant_conflict() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    let base = (0..200).map(|i| format!("line {i}\n")).collect::<String>();
    repo.write_file("conflict.txt", &base)?;
    repo.commit_all("base")?;
    repo.checkout_new_branch("feature")?;
    repo.write_file("conflict.txt", &base.replace("line 140\n", "theirs\n"))?;
    repo.commit_all("feature")?;
    repo.checkout("main")?;
    repo.write_file("conflict.txt", &base.replace("line 140\n", "ours\n"))?;
    repo.commit_all("main")?;
    repo.merge_expect_conflict("feature")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("conflict.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "j").await?);
    let state = app
        .editor
        .diff
        .merge_views
        .get(&app.editor.tree.focus)
        .unwrap();
    assert!(current_cursor_line(&app) >= 140);
    for (doc_id, view_id) in [
        (state.ours_doc_id, state.ours_view_id),
        (state.theirs_doc_id, state.theirs_view_id),
    ] {
        let doc = app.editor.document(doc_id).unwrap();
        let view = app.editor.tree.get(view_id);
        let pos = doc.text().line_to_char(140);
        assert!(
            view.screen_coords_at_pos(doc, doc.text().slice(..), pos)
                .is_some(),
            "side conflict is off screen: {:?}",
            doc.view_offset(view_id)
        );
    }
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_diff_can_reuse_merge_snapshots_without_closing_them() -> anyhow::Result<()> {
    let repo = conflicted_repo()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("conflict.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":merge<ret>").await?);
    let state = app
        .editor
        .diff
        .merge_views
        .get(&app.editor.tree.focus)
        .unwrap()
        .clone();
    app.editor.focus(state.ours_view_id);
    app.editor
        .open_buffer_diff_view(state.theirs_doc_id, state.ours_doc_id, Some(true))?;
    assert!(app.editor.diff.merge_views.is_empty());
    assert_eq!(main_diff_state(&app).working_doc_id, state.ours_doc_id);
    for (view, _) in app.editor.tree.views() {
        assert!(app.editor.document(view.doc).is_some());
    }
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn accepting_index_base_resolves_only_the_selected_marker_block() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    let base = (0..50).map(|i| format!("line {i}\n")).collect::<String>();
    repo.write_file("conflict.txt", &base)?;
    repo.commit_all("base")?;
    repo.checkout_new_branch("feature")?;
    repo.write_file(
        "conflict.txt",
        &base
            .replace("line 5\n", "theirs five\n")
            .replace("line 40\n", "theirs forty\n"),
    )?;
    repo.commit_all("feature")?;
    repo.checkout("main")?;
    repo.write_file(
        "conflict.txt",
        &base
            .replace("line 5\n", "ours five\n")
            .replace("line 40\n", "ours forty\n"),
    )?;
    repo.commit_all("main")?;
    repo.merge_expect_conflict("feature")?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("conflict.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":merge<ret><space>ma").await?);
    let after_first = current_doc_text(&app);
    assert!(
        after_first.contains("line 4\nline 5\nline 6\n"),
        "{after_first}"
    );
    assert_eq!(
        helix_view::merge_view::conflict_count(&helix_core::Rope::from(after_first.as_str())),
        1
    );
    assert!(harness.send_keys(&mut app, "<space>mn<space>ma").await?);
    assert_eq!(current_doc_text(&app), base);
    assert!(harness.send_keys(&mut app, "u").await?);
    assert_eq!(current_doc_text(&app), after_first);
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn sticky_merge_menu_can_switch_sides_and_leave_for_undo() -> anyhow::Result<()> {
    let repo = conflicted_repo()?;
    let _cwd = CwdGuard::enter(repo.path()).await?;
    let mut app = AppBuilder::new()
        .with_file(repo.file("conflict.txt"), None)
        .build()?;
    let mut harness = AppTestHarness::new();
    assert!(harness.send_keys(&mut app, ":merge<ret><space>Mo").await?);
    assert_eq!(current_doc_text(&app), "ours\n");
    assert!(harness.send_keys(&mut app, "t").await?);
    assert_eq!(current_doc_text(&app), "theirs\n");
    assert!(harness.send_keys(&mut app, "<esc>u").await?);
    assert_eq!(current_doc_text(&app), "ours\n");
    assert!(harness.send_keys(&mut app, "<space>mq").await?);
    harness.close(&mut app).await?;
    Ok(())
}
