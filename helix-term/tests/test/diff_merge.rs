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
        assert!(base_doc.diff_handle().is_some());
        assert!(working_doc.diff_handle().is_some());
        assert!(base_doc.char_diff_minus_side);
        assert!(!working_doc.char_diff_minus_side);
        assert!(!working_doc.diff_handle().unwrap().load().is_empty());
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
        .diff_handle()
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
        .diff_handle()
        .is_none());
    assert!(app
        .editor
        .document(working_doc_id)
        .unwrap()
        .diff_handle()
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
                &format!("Diff set: {oldest}^..{newest}"),
                Severity::Info,
            );
            assert_diff_range(app, &format!("{oldest}^"), Some(&newest));
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
                &format!("Diff set: {newest}^ vs working tree"),
                Severity::Info,
            );
            assert_diff_range(app, &format!("{newest}^"), None);
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
async fn split_diff_keyboard_and_search_sync_scroll() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    let mut base = String::new();
    let mut working = String::new();
    for i in 0..250 {
        if i == 1 {
            base.push_str("one\n");
            working.push_str("one changed\n");
        } else if i == 200 {
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

    assert!(harness.send_keys(&mut app, "gg").await?);
    assert_eq!(split_diff_anchor_lines(&app), (0, 0));
    assert!(harness.send_keys(&mut app, "/needle<esc>").await?);
    assert_eq!(split_diff_anchor_lines(&app), (0, 0));

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
    assert!(app
        .editor
        .document(diff_state.base_doc_id)
        .unwrap()
        .path()
        .is_none());
    assert!(
        app.editor
            .document(diff_state.base_doc_id)
            .unwrap()
            .readonly
    );

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
        .diff_handle()
        .unwrap()
        .load()
        .diff_base()
        .to_string();
    assert_eq!(actual_from_base, expected_working);

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
                    assert!(app
                        .editor
                        .document(state.ours_doc_id)
                        .unwrap()
                        .path()
                        .is_none());
                    assert!(app
                        .editor
                        .document(state.theirs_doc_id)
                        .unwrap()
                        .path()
                        .is_none());
                    assert!(app.editor.document(state.ours_doc_id).unwrap().readonly);
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
