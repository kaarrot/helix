use helix_term::application::Application;
use helix_view::review::{DiffSide, ThreadId};

use super::*;

fn thread_count(app: &Application) -> usize {
    app.editor.diff.reviews.len()
}

fn only_thread_side(app: &Application) -> DiffSide {
    app.editor
        .diff
        .reviews
        .iter()
        .next()
        .expect("expected a review thread")
        .side
}

fn only_thread_draft(app: &Application) -> Option<&str> {
    app.editor
        .diff
        .reviews
        .iter()
        .next()
        .expect("expected a review thread")
        .draft
        .as_deref()
}

fn only_thread_line(app: &Application) -> u32 {
    app.editor
        .diff
        .reviews
        .iter()
        .next()
        .expect("expected a review thread")
        .line
}

fn focused_review_side(app: &Application) -> DiffSide {
    let view = app.editor.tree.get(app.editor.tree.focus);
    let doc = app.editor.document(view.doc).unwrap();
    view.review_identity(doc, &app.editor.diff.views)
        .expect("focused view should have a review identity")
        .1
}

/// The virtual-row plan for the focused view, or `None` when it wants no rows.
fn focused_plan_row_count(app: &Application) -> usize {
    let view = app.editor.tree.get(app.editor.tree.focus);
    let doc = app.editor.document(view.doc).unwrap();
    view.virtual_row_plan(
        doc,
        &app.editor.diff.views,
        &app.editor.documents,
        &app.editor.diff.reviews,
        None,
    )
    .map_or(0, |plan| {
        (0..doc.text().len_lines())
            .map(|line| plan.rows_at(line).len())
            .sum()
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn comment_renders_in_a_plain_buffer_with_no_diff() -> anyhow::Result<()> {
    // The row plan used to be gated on the view being a diff pane, which would
    // have made commenting impossible on a clean worktree.
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "alpha\nbeta\ngamma\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    assert_eq!(thread_count(&app), 0);
    assert_eq!(focused_plan_row_count(&app), 0);

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why this branch?<C-s>").await?);

    assert_eq!(thread_count(&app), 1, "comment was not stored");
    assert!(
        focused_plan_row_count(&app) > 0,
        "comment reserved no rows in a buffer with no diff view"
    );

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn single_pane_diff_keeps_working_side_comments() -> anyhow::Result<()> {
    // Single-pane Space-g reuses the working view for both ids; comments
    // made in the file must stay Working and visible there.
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.rs", "fn one() {}\n")?;
    repo.commit_all("initial")?;
    repo.write_file("tracked.rs", "fn one() { changed }\n")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("tracked.rs");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why this line?<C-s>").await?);
    assert_eq!(thread_count(&app), 1);
    assert_eq!(only_thread_side(&app), DiffSide::Working);
    assert!(focused_plan_row_count(&app) > 0);

    assert!(harness.send_keys(&mut app, "<space>g").await?);
    assert!(harness.wait_for_idle(&mut app).await?);
    assert!(harness.send_keys(&mut app, "<ret>").await?);

    assert_eq!(
        app.editor.diff.views.len(),
        1,
        "default diff is single-pane"
    );
    let view_id = app.editor.tree.focus;
    let diff_state = &app.editor.diff.views[&view_id];
    assert_eq!(
        diff_state.base_view_id, diff_state.working_view_id,
        "single-pane reuses the working view for both ids"
    );
    assert_eq!(
        thread_count(&app),
        1,
        "opening the diff must not create a second thread"
    );
    assert_eq!(only_thread_side(&app), DiffSide::Working);
    assert_eq!(focused_review_side(&app), DiffSide::Working);
    assert!(
        focused_plan_row_count(&app) > 0,
        "in-file comments must still render in a single-pane diff"
    );

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn split_diff_base_pane_claims_a_session() -> anyhow::Result<()> {
    // Virtual base docs have path == None, so the first comment from the left
    // pane used to skip claiming a session, never persist, and seed its
    // anchors onto the working document.
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\ntwo\nthree\n")?;
    repo.commit_all("initial")?;
    repo.checkout_new_branch("base-pane")?;
    repo.write_file("tracked.txt", "one\ntwo changed\nthree\n")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    app.editor
        .open_diff_view(&path, "HEAD", Some(true))
        .expect("split diff should open");
    // The differ can keep requesting redraws, so wait_for_idle is unusable
    // until it settles; pump instead of requiring idle.
    harness
        .pump(&mut app, std::time::Duration::from_millis(400))
        .await;
    assert_eq!(app.editor.diff.views.len(), 2, "expected a split diff");

    let (base_view, base_doc, working_doc) = {
        let view_id = app.editor.tree.focus;
        let state = &app.editor.diff.views[&view_id];
        assert_ne!(
            state.base_view_id, state.working_view_id,
            "split panes must be distinct views"
        );
        (state.base_view_id, state.base_doc_id, state.working_doc_id)
    };
    assert!(
        app.editor.document(base_doc).unwrap().path().is_none(),
        "the bug is that the virtual base document has no path"
    );

    app.editor.focus(base_view);
    assert_eq!(focused_review_side(&app), DiffSide::Base);

    harness
        .send_keys_pumping(
            &mut app,
            "<space>mRc",
            std::time::Duration::from_millis(300),
        )
        .await?;
    harness
        .send_keys_pumping(
            &mut app,
            "old two<C-s>",
            std::time::Duration::from_millis(300),
        )
        .await?;

    let session = app
        .editor
        .diff
        .session
        .as_ref()
        .expect("commenting from the base pane must claim a session");
    assert_eq!(session.name, "base-pane");
    let uuid = session.uuid.clone();
    assert_eq!(thread_count(&app), 1);
    assert_eq!(only_thread_side(&app), DiffSide::Base);
    assert_eq!(
        app.editor.document(base_doc).unwrap().review_anchors.len(),
        1,
        "the comment must be anchored on the base document"
    );
    assert!(
        app.editor
            .document(working_doc)
            .unwrap()
            .review_anchors
            .is_empty(),
        "a base-side comment must not seed onto the working document"
    );
    assert!(focused_plan_row_count(&app) > 0);

    app.editor.save_reviews();
    let dir = helix_view::review::session::review_dir();
    assert!(
        dir.join(format!("{uuid}.threads.json")).exists(),
        "claiming the session is what makes the draft persist"
    );

    app.editor
        .document_mut(base_doc)
        .unwrap()
        .review_anchors
        .clear();
    app.editor
        .document_mut(working_doc)
        .unwrap()
        .review_anchors
        .clear();
    app.editor.seed_review_anchors(base_doc);
    app.editor.seed_review_anchors(working_doc);
    assert_eq!(
        app.editor.document(base_doc).unwrap().review_anchors.len(),
        1,
        "reseed must put base-side threads back on the base document"
    );
    assert!(
        app.editor
            .document(working_doc)
            .unwrap()
            .review_anchors
            .is_empty(),
        "reseed must not copy base-side threads onto the working document"
    );

    // :q! via harness.close waits for the event loop to exit; a split differ
    // that is still redrawing never goes idle, so the 2s close timeout fires.
    let _ = app.close().await;
    let _ = std::fs::remove_file(dir.join(format!("{uuid}.threads.json")));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn split_diff_prompts_quote_the_matching_side() -> anyhow::Result<()> {
    // compose_prompt used to ignore thread.side and always quote the working
    // tree, so a comment on the old pane sent the new text at that line.
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\ntwo\nthree\n")?;
    repo.commit_all("initial")?;
    repo.write_file("tracked.txt", "one\ntwo changed\nthree\n")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    app.editor
        .open_diff_view(&path, "HEAD", Some(true))
        .expect("split diff should open");
    harness
        .pump(&mut app, std::time::Duration::from_millis(400))
        .await;

    let (base_view, working_view) = {
        let view_id = app.editor.tree.focus;
        let state = &app.editor.diff.views[&view_id];
        (state.base_view_id, state.working_view_id)
    };

    app.editor.focus(base_view);
    harness
        .send_keys_pumping(&mut app, "ggj", std::time::Duration::from_millis(200))
        .await?;
    harness
        .send_keys_pumping(
            &mut app,
            "<space>mRc",
            std::time::Duration::from_millis(300),
        )
        .await?;
    harness
        .send_keys_pumping(
            &mut app,
            "old two<C-s>",
            std::time::Duration::from_millis(300),
        )
        .await?;

    app.editor.focus(working_view);
    harness
        .send_keys_pumping(&mut app, "ggj", std::time::Duration::from_millis(200))
        .await?;
    harness
        .send_keys_pumping(
            &mut app,
            "<space>mRc",
            std::time::Duration::from_millis(300),
        )
        .await?;
    harness
        .send_keys_pumping(
            &mut app,
            "new two<C-s>",
            std::time::Duration::from_millis(300),
        )
        .await?;

    assert_eq!(app.editor.diff.reviews.pending_count(), 2);
    let fake = FakeAgent::default();
    let sent = fake.sent.clone();
    app.editor.diff.agent = Some(Box::new(fake));

    harness
        .send_keys_pumping(
            &mut app,
            "<space>mRS",
            std::time::Duration::from_millis(400),
        )
        .await?;

    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 2, "both drafts should be sent");

    let base_prompt = sent
        .iter()
        .find(|(_, prompt)| prompt.contains("old two"))
        .map(|(_, prompt)| prompt.as_str())
        .expect("base comment was sent");
    let working_prompt = sent
        .iter()
        .find(|(_, prompt)| prompt.contains("new two"))
        .map(|(_, prompt)| prompt.as_str())
        .expect("working comment was sent");

    assert!(
        base_prompt.contains("side: base"),
        "base comment must be tagged as the old side:\n{base_prompt}"
    );
    assert!(
        !base_prompt.contains("two changed"),
        "base comment must quote the old text, not the working tree:\n{base_prompt}"
    );
    assert!(
        base_prompt.contains("two"),
        "base comment should still quote the old line:\n{base_prompt}"
    );

    assert!(
        working_prompt.contains("side: working"),
        "working comment must be tagged as the working side:\n{working_prompt}"
    );
    assert!(
        working_prompt.contains("two changed"),
        "working comment must quote the working-tree text:\n{working_prompt}"
    );

    let _ = app.close().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ctrl_s_sends_a_saved_draft_from_the_comment_line() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.rs", "fn one() {}\n")?;
    repo.commit_all("initial")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("tracked.rs");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    let fake = FakeAgent::default();
    let sent = fake.sent.clone();
    app.editor.diff.agent = Some(Box::new(fake));

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-s>").await?);
    assert_eq!(app.editor.diff.reviews.pending_count(), 1);
    assert!(sent.lock().unwrap().is_empty(), "saving must not send");

    // Closed the box: Ctrl-S / Ctrl-Shift-S now land in normal mode.
    assert!(app.editor.diff.reviews.composing.is_none());
    assert!(harness.send_keys(&mut app, "<C-S-s>").await?);
    assert_eq!(sent.lock().unwrap().len(), 1);
    assert_eq!(app.editor.diff.reviews.pending_count(), 0);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn collapsing_a_thread_shrinks_it_to_one_row() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "alpha\nbeta\ngamma\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(
        harness
            .send_keys(
                &mut app,
                "a comment long enough to wrap over several rows<C-s>"
            )
            .await?
    );
    let expanded = focused_plan_row_count(&app);
    assert!(expanded >= 1);

    assert!(harness.send_keys(&mut app, "<space>mRt").await?);
    assert_eq!(
        focused_plan_row_count(&app),
        1,
        "a collapsed thread should occupy exactly one summary row"
    );

    assert!(harness.send_keys(&mut app, "<space>mRt").await?);
    assert_eq!(
        focused_plan_row_count(&app),
        expanded,
        "toggle should restore"
    );

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn editing_above_a_comment_moves_it_and_deleting_orphans_it() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "alpha\nbeta\ngamma\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    // Comment on line 2 ("beta"), then insert a line above it.
    assert!(harness.send_keys(&mut app, "jj").await?);
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "about gamma<C-s>").await?);

    let anchored_line = |app: &Application| {
        let view = app.editor.tree.get(app.editor.tree.focus);
        let doc = app.editor.document(view.doc).unwrap();
        doc.review_anchors[0].line(doc.text())
    };
    let before = anchored_line(&app);

    assert!(harness.send_keys(&mut app, "ggO").await?);
    assert!(harness.send_keys(&mut app, "inserted<esc>").await?);
    assert_eq!(
        anchored_line(&app),
        before + 1,
        "inserting a line above should push the comment down"
    );

    // Now delete the anchored line entirely.
    let line = anchored_line(&app);
    let goto = format!("{}gg", line + 1);
    assert!(harness.send_keys(&mut app, &goto).await?);
    assert!(harness.send_keys(&mut app, "xd").await?);

    let view = app.editor.tree.get(app.editor.tree.focus);
    let doc = app.editor.document(view.doc).unwrap();
    assert!(
        doc.review_anchors[0].orphaned,
        "deleting the anchored line should orphan the thread, not lose it"
    );
    assert_eq!(
        app.editor.diff.reviews.len(),
        1,
        "an orphaned thread must still exist"
    );

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn remapped_comment_line_survives_save_and_reseed() -> anyhow::Result<()> {
    // Anchors track inserts, but thread.line used to stay at the original
    // number. Restart reseeds from that snapshot, so the box jumped back.
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\ntwo\nthree\n")?;
    repo.commit_all("initial")?;
    repo.checkout_new_branch("remap-save")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    // Comment on line 1 ("two").
    assert!(harness.send_keys(&mut app, "j").await?);
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "about two<C-s>").await?);
    assert_eq!(only_thread_line(&app), 1);

    assert!(harness.send_keys(&mut app, "ggO").await?);
    assert!(harness.send_keys(&mut app, "inserted<esc>").await?);

    let view = app.editor.tree.get(app.editor.tree.focus);
    let doc_id = view.doc;
    let remapped = {
        let doc = app.editor.document(doc_id).unwrap();
        doc.review_anchors[0].line(doc.text())
    };
    assert_eq!(
        remapped, 2,
        "inserting a line above should push the comment down"
    );

    app.editor.save_reviews();
    assert_eq!(
        only_thread_line(&app),
        remapped as u32,
        "save must write the remapped line back into the store"
    );

    app.editor
        .document_mut(doc_id)
        .unwrap()
        .review_anchors
        .clear();
    app.editor.seed_review_anchors(doc_id);
    let reseeded = {
        let doc = app.editor.document(doc_id).unwrap();
        doc.review_anchors[0].line(doc.text())
    };
    assert_eq!(
        reseeded, remapped,
        "reseed must use the remapped stored line"
    );
    assert!(focused_plan_row_count(&app) > 0);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn remapped_comment_line_survives_closing_the_document() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "one\ntwo\nthree\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "j").await?);
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "about two<C-s>").await?);

    assert!(harness.send_keys(&mut app, "ggO").await?);
    assert!(harness.send_keys(&mut app, "inserted<esc>").await?);

    let view = app.editor.tree.get(app.editor.tree.focus);
    let doc_id = view.doc;
    let remapped = {
        let doc = app.editor.document(doc_id).unwrap();
        doc.review_anchors[0].line(doc.text())
    };
    assert_eq!(remapped, 2);

    assert!(
        app.editor.close_document(doc_id, true).is_ok(),
        "force-close should succeed"
    );
    assert_eq!(
        only_thread_line(&app),
        remapped as u32,
        "closing must write the remapped line back before anchors are dropped"
    );

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn reply_after_insert_composes_on_the_remapped_line() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "one\ntwo\nthree\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "j").await?);
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "about two<C-s>").await?);

    assert!(harness.send_keys(&mut app, "ggO").await?);
    assert!(harness.send_keys(&mut app, "inserted<esc>").await?);

    let remapped = {
        let view = app.editor.tree.get(app.editor.tree.focus);
        let doc = app.editor.document(view.doc).unwrap();
        doc.review_anchors[0].line(doc.text()) as u32
    };
    assert_eq!(remapped, 2);

    assert!(harness.send_keys(&mut app, "]C").await?);
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);

    let composing = app
        .editor
        .diff
        .reviews
        .composing
        .as_ref()
        .expect("reply should open a composing box");
    assert_eq!(
        composing.line, remapped,
        "the reply box must sit on the remapped line, not the stale stored line"
    );

    assert!(harness.send_keys(&mut app, "<esc>").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_comment_claims_a_session_named_for_the_branch() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\ntwo\n")?;
    repo.commit_all("initial")?;
    repo.checkout_new_branch("feature-x")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(
        app.editor.diff.session.is_none(),
        "no session before commenting"
    );

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-s>").await?);

    let session = app
        .editor
        .diff
        .session
        .as_ref()
        .expect("first comment should claim a session");
    assert_eq!(session.name, "feature-x");
    let uuid = session.uuid.clone();

    // Switching by name gives a different conversation, deterministically.
    assert!(
        harness
            .send_keys(&mut app, ":review-session spike<ret>")
            .await?
    );
    let switched = app.editor.diff.session.as_ref().unwrap();
    assert_eq!(switched.name, "spike");
    assert_ne!(switched.uuid, uuid);

    // And switching back resumes the original one.
    assert!(
        harness
            .send_keys(&mut app, ":review-session feature-x<ret>")
            .await?
    );
    assert_eq!(app.editor.diff.session.as_ref().unwrap().uuid, uuid);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_review_sessions_isolates_their_stores() -> anyhow::Result<()> {
    // :review-session used to keep the in-memory threads and only change the
    // UUID, so the next save wrote them over the conversation just switched to.
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\ntwo\n")?;
    repo.commit_all("initial")?;
    repo.checkout_new_branch("feature-x")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-s>").await?);
    assert_eq!(thread_count(&app), 1);
    assert_eq!(only_thread_draft(&app), Some("why?"));

    assert!(
        harness
            .send_keys(&mut app, ":review-session spike<ret>")
            .await?
    );
    assert_eq!(
        thread_count(&app),
        0,
        "switching must not keep the previous conversation's threads"
    );
    assert_eq!(focused_plan_row_count(&app), 0);
    {
        let view = app.editor.tree.get(app.editor.tree.focus);
        let doc = app.editor.document(view.doc).unwrap();
        assert!(
            doc.review_anchors.is_empty(),
            "anchors belong to the previous conversation"
        );
    }

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "spike note<C-s>").await?);
    assert_eq!(thread_count(&app), 1);
    assert_eq!(only_thread_draft(&app), Some("spike note"));

    assert!(
        harness
            .send_keys(&mut app, ":review-session feature-x<ret>")
            .await?
    );
    assert_eq!(thread_count(&app), 1);
    assert_eq!(
        only_thread_draft(&app),
        Some("why?"),
        "the original conversation must come back, not spike's threads"
    );
    assert!(focused_plan_row_count(&app) > 0);

    assert!(
        harness
            .send_keys(&mut app, ":review-session spike<ret>")
            .await?
    );
    assert_eq!(thread_count(&app), 1);
    assert_eq!(only_thread_draft(&app), Some("spike note"));

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn review_session_picks_the_agent_without_renaming() -> anyhow::Result<()> {
    use helix_view::review::agent::ReviewAgentKind;

    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.txt", "one\ntwo\n")?;
    repo.commit_all("initial")?;
    repo.checkout_new_branch("feature-x")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("tracked.txt");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-s>").await?);
    assert_eq!(app.editor.diff.session.as_ref().unwrap().name, "feature-x");
    assert_eq!(app.editor.diff.agent_kind, ReviewAgentKind::Claude);

    assert!(
        harness
            .send_keys(&mut app, ":review-session grok<ret>")
            .await?
    );
    assert_eq!(app.editor.diff.session.as_ref().unwrap().name, "feature-x");
    assert_eq!(app.editor.diff.agent_kind, ReviewAgentKind::Grok);

    assert!(
        harness
            .send_keys(&mut app, ":review-session spike claude<ret>")
            .await?
    );
    assert_eq!(app.editor.diff.session.as_ref().unwrap().name, "spike");
    assert_eq!(app.editor.diff.agent_kind, ReviewAgentKind::Claude);

    harness.close(&mut app).await?;
    Ok(())
}

/// Records what it was asked, and answers on demand, so the send path can be
/// exercised without spawning a real agent.
#[derive(Debug, Default, Clone)]
struct FakeAgent {
    sent: std::sync::Arc<std::sync::Mutex<Vec<(helix_view::review::ThreadId, String)>>>,
}

impl helix_view::review::agent::ReviewAgent for FakeAgent {
    fn send(&mut self, thread: helix_view::review::ThreadId, prompt: String) -> anyhow::Result<()> {
        self.sent.lock().unwrap().push((thread, prompt));
        Ok(())
    }
    fn shutdown(&mut self) {}
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_send_quotes_each_comment_s_own_file() -> anyhow::Result<()> {
    // Regression: the prompt used to be composed from the focused document, so
    // a batch spanning several files attributed every comment to whichever one
    // happened to be on screen.
    let repo = GitRepoFixture::new()?;
    repo.write_file("alpha.rs", "fn alpha_one() {}\nfn alpha_two() {}\n")?;
    repo.write_file("beta.rs", "fn beta_one() {}\nfn beta_two() {}\n")?;
    repo.commit_all("initial")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let alpha = repo.file("alpha.rs");
    let beta = repo.file("beta.rs");

    let mut app = AppBuilder::new().with_file(&alpha, None).build()?;
    let mut harness = AppTestHarness::new();

    // A comment in alpha.rs, then another in beta.rs.
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "about alpha<C-s>").await?);
    let open_beta = format!(":open {}<ret>", beta.display());
    assert!(harness.send_keys(&mut app, &open_beta).await?);
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "about beta<C-s>").await?);

    assert_eq!(app.editor.diff.reviews.pending_count(), 2);

    let fake = FakeAgent::default();
    let sent = fake.sent.clone();
    app.editor.diff.agent = Some(Box::new(fake));

    assert!(harness.send_keys(&mut app, "<space>mRS").await?);

    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 2, "both drafts should be sent");
    assert_eq!(app.editor.diff.reviews.pending_count(), 0);

    let alpha_prompt = sent
        .iter()
        .find(|(_, prompt)| prompt.contains("about alpha"))
        .expect("alpha comment was sent");
    let beta_prompt = sent
        .iter()
        .find(|(_, prompt)| prompt.contains("about beta"))
        .expect("beta comment was sent");

    assert!(
        alpha_prompt.1.contains("alpha.rs") && !alpha_prompt.1.contains("beta.rs"),
        "alpha's comment must quote alpha.rs:\n{}",
        alpha_prompt.1
    );
    assert!(
        beta_prompt.1.contains("beta.rs") && !beta_prompt.1.contains("alpha.rs"),
        "beta's comment must quote beta.rs:\n{}",
        beta_prompt.1
    );
    // The quoted context must come from the right file too, not just the path.
    assert!(alpha_prompt.1.contains("alpha_one"), "{}", alpha_prompt.1);
    assert!(beta_prompt.1.contains("beta_one"), "{}", beta_prompt.1);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_lands_on_the_thread_that_asked() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.rs", "fn one() {}\nfn two() {}\n")?;
    repo.commit_all("initial")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("tracked.rs");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-s>").await?);

    let fake = FakeAgent::default();
    let sent = fake.sent.clone();
    app.editor.diff.agent = Some(Box::new(fake));
    assert!(harness.send_keys(&mut app, "<space>mRS").await?);

    let id = sent.lock().unwrap()[0].0;
    use helix_view::review::agent::AgentEvent;
    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Started(id));
    assert!(app.editor.diff.reviews.get(id).unwrap().awaiting);

    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Chunk(id, "because ".into()));
    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Completed(id, "because it is".into()));

    let thread = app.editor.diff.reviews.get(id).unwrap();
    assert!(!thread.awaiting);
    assert_eq!(thread.entry_count(), 2);
    assert_eq!(thread.entry(1).unwrap().text, "because it is");

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thread_grows_through_replies_and_can_be_navigated() -> anyhow::Result<()> {
    use helix_view::review::agent::AgentEvent;

    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.rs", "fn one() {}\nfn two() {}\n")?;
    repo.commit_all("initial")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("tracked.rs");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    let fake = FakeAgent::default();
    let sent = fake.sent.clone();
    app.editor.diff.agent = Some(Box::new(fake));

    // Question, answer.
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why this?<C-s>").await?);
    assert!(harness.send_keys(&mut app, "<space>mRS").await?);
    let id = sent.lock().unwrap()[0].0;
    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Completed(id, "because X".into()));

    // Replying on the same line must extend the thread, not open a new one.
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(
        harness
            .send_keys(&mut app, "but what about Y?<C-s>")
            .await?
    );
    assert_eq!(
        app.editor.diff.reviews.len(),
        1,
        "a reply must extend the thread, not start another"
    );

    assert!(harness.send_keys(&mut app, "<space>mRS").await?);
    let second = sent.lock().unwrap()[1].clone();
    assert_eq!(second.0, id, "the follow-up belongs to the same thread");
    assert!(
        second.1.contains("follow-up"),
        "a follow-up should not resend the whole context: {}",
        second.1
    );

    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Completed(id, "Y is handled".into()));

    // you, agent, you, agent
    let thread = app.editor.diff.reviews.get(id).unwrap();
    assert_eq!(thread.entry_count(), 4);
    assert_eq!(thread.view_index(), 3, "the newest entry is shown");
    assert_eq!(thread.entry(3).unwrap().text, "Y is handled");

    // And the history is navigable, once the box has been stopped on: walking
    // a thread is an action on the box, not on being near it.
    assert!(harness.send_keys(&mut app, "j").await?);
    assert!(app.editor.diff.reviews.focused.is_some());
    assert!(harness.send_keys(&mut app, "<C-left>").await?);
    assert!(harness.send_keys(&mut app, "<C-left>").await?);
    assert_eq!(app.editor.diff.reviews.get(id).unwrap().view_index(), 1);
    assert_eq!(
        app.editor
            .diff
            .reviews
            .get(id)
            .unwrap()
            .entry(1)
            .unwrap()
            .text,
        "because X"
    );
    assert!(harness.send_keys(&mut app, "<C-right>").await?);
    assert_eq!(app.editor.diff.reviews.get(id).unwrap().view_index(), 2);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn ctrl_left_walks_the_thread_while_the_comment_box_is_open() -> anyhow::Result<()> {
    use helix_view::review::agent::AgentEvent;

    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.rs", "fn one() {}\nfn two() {}\n")?;
    repo.commit_all("initial")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("tracked.rs");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    let fake = FakeAgent::default();
    let sent = fake.sent.clone();
    app.editor.diff.agent = Some(Box::new(fake));

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-s>").await?);
    assert!(harness.send_keys(&mut app, "<space>mRS").await?);
    let id = sent.lock().unwrap()[0].0;
    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Completed(id, "because".into()));
    assert_eq!(app.editor.diff.reviews.get(id).unwrap().view_index(), 1);

    // Open a reply. Ctrl-left must walk the thread, not move the caret in the box.
    assert!(harness.send_keys(&mut app, "c").await?);
    assert!(app.editor.diff.reviews.composing.is_some());
    assert!(harness.send_keys(&mut app, "<C-left>").await?);
    assert_eq!(
        app.editor.diff.reviews.get(id).unwrap().view_index(),
        0,
        "Ctrl-left while composing should show the previous entry"
    );

    assert!(harness.send_keys(&mut app, "<esc>").await?);
    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_conversation_comes_back_after_a_restart() -> anyhow::Result<()> {
    use helix_view::review::agent::AgentEvent;

    let repo = GitRepoFixture::new()?;
    repo.write_file("tracked.rs", "fn one() {}\nfn two() {}\nfn three() {}\n")?;
    repo.commit_all("initial")?;
    repo.checkout_new_branch("persist-me")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("tracked.rs");

    let uuid = {
        let mut app = AppBuilder::new().with_file(&path, None).build()?;
        let mut harness = AppTestHarness::new();

        let fake = FakeAgent::default();
        let sent = fake.sent.clone();
        app.editor.diff.agent = Some(Box::new(fake));

        // An answered thread, and a draft that was never sent.
        assert!(harness.send_keys(&mut app, "<space>mRc").await?);
        assert!(harness.send_keys(&mut app, "why one?<C-s>").await?);
        assert!(harness.send_keys(&mut app, "<space>mRS").await?);
        let id = sent.lock().unwrap()[0].0;
        app.editor
            .diff
            .reviews
            .apply_agent_event(AgentEvent::Completed(id, "because one".into()));

        assert!(harness.send_keys(&mut app, "jj").await?);
        assert!(harness.send_keys(&mut app, "<space>mRc").await?);
        assert!(harness.send_keys(&mut app, "never sent<C-s>").await?);

        let uuid = app.editor.diff.session.as_ref().unwrap().uuid.clone();
        // The write is debounced in normal use; force it rather than sleeping.
        app.editor.save_reviews();
        harness.close(&mut app).await?;
        uuid
    };

    // A fresh editor on the same branch: same derived uuid, same conversation.
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    // Commenting is what claims the session, and claiming is what reloads.
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "<esc>").await?);

    assert_eq!(
        app.editor.diff.session.as_ref().map(|s| s.uuid.clone()),
        Some(uuid.clone()),
        "the same branch must derive the same conversation"
    );
    assert_eq!(
        app.editor.diff.reviews.len(),
        2,
        "both threads should come back"
    );
    assert_eq!(
        app.editor.diff.reviews.pending_count(),
        1,
        "the unsent draft is the thing most worth not losing"
    );
    assert!(
        app.editor
            .diff
            .reviews
            .iter()
            .any(|thread| thread.entry(1).is_some_and(|e| e.text == "because one")),
        "the agent's answer should come back too"
    );
    assert!(!app.editor.diff.reviews.any_awaiting());

    // Reloaded threads must be anchored, or they would stop tracking edits.
    let view = app.editor.tree.get(app.editor.tree.focus);
    let doc = app.editor.document(view.doc).unwrap();
    assert_eq!(
        doc.review_anchors.len(),
        2,
        "reloaded threads must be re-anchored in the open document"
    );

    // Commenting on a reloaded thread's line must edit it, not open a second
    // one on top. This used to look for a thread before claiming the session,
    // so on a fresh editor the store was still empty and the existing draft was
    // invisible.
    assert!(harness.send_keys(&mut app, "]c").await?);
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "still editing<C-s>").await?);
    assert_eq!(
        app.editor.diff.reviews.len(),
        2,
        "commenting on a reloaded thread must extend it, not duplicate it"
    );

    harness.close(&mut app).await?;

    let dir = helix_view::review::session::review_dir();
    let _ = std::fs::remove_file(dir.join(format!("{uuid}.threads.json")));
    Ok(())
}

/// Exercises the real `claude` child: spawning it, the flags it is given, and
/// the parsing of its actual output.
///
/// Ignored by default — it needs the CLI installed, credentials and the
/// network, and it costs tokens. Run it deliberately:
///
/// ```sh
/// HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1 cargo test --features integration \
///     --profile integration --workspace --test integration \
///     -- --ignored --nocapture answers_for_real
/// ```
///
/// This is the one seam the other tests cannot cover: every event shape is
/// parsed from a live process rather than from a fixture written by hand.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "spawns a real agent: needs the CLI, credentials, network and tokens"]
async fn the_agent_answers_for_real() -> anyhow::Result<()> {
    let repo = GitRepoFixture::new()?;
    repo.write_file("src.rs", "fn alpha() {}\nfn beta() {}\nfn gamma() {}\n")?;
    repo.commit_all("initial")?;

    let _cwd = CwdGuard::enter(repo.path()).await?;
    let path = repo.file("src.rs");
    let mut app = AppBuilder::new().with_file(&path, None).build()?;
    let mut harness = AppTestHarness::new();

    // Ask something only answerable by reading the file, so a wrong answer
    // means the context Helix composed never arrived.
    assert!(harness.send_keys(&mut app, "j").await?);
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(
        harness
            .send_keys(
                &mut app,
                "Reply with only the name of the function on this line.<C-s>"
            )
            .await?
    );
    assert_eq!(app.editor.diff.reviews.pending_count(), 1);

    // Not `send_keys`: sending starts the spinner, and an animation means the
    // editor never goes idle, which is what `send_keys` waits for.
    harness
        .send_keys_pumping(
            &mut app,
            "<space>mRS",
            std::time::Duration::from_millis(500),
        )
        .await?;
    assert!(
        app.editor.diff.agent.is_some(),
        "sending should have spawned an agent"
    );

    let id = app.editor.diff.reviews.iter().next().unwrap().id;

    // Pump the event loop until the reply lands. Jobs dispatched by the reader
    // are applied here, which is what makes this exercise the real path.
    //
    // `wait_for_idle` cannot be used: under the integration feature the idle
    // timer is reset after every event, and the spinner's redraw ticker means
    // events never stop while a reply is in flight, so it would never return.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    let mut answer = None;
    while std::time::Instant::now() < deadline {
        harness
            .pump(&mut app, std::time::Duration::from_millis(300))
            .await;
        let thread = app.editor.diff.reviews.get(id).unwrap();
        if !thread.awaiting && thread.entry_count() >= 2 {
            answer = thread.entry(1).map(|entry| entry.text.to_string());
            break;
        }
    }

    let answer = answer.expect("the agent never replied within the deadline");
    println!("agent replied: {answer:?}");
    assert!(
        answer.to_lowercase().contains("beta"),
        "the agent should have seen the line Helix quoted, got: {answer:?}"
    );

    // The conversation is on disk, so a restart would bring it back.
    let uuid = app.editor.diff.session.as_ref().unwrap().uuid.clone();
    app.editor.save_reviews();
    let dir = helix_view::review::session::review_dir();
    assert!(dir.join(format!("{uuid}.threads.json")).exists());

    harness.close(&mut app).await?;
    let _ = std::fs::remove_file(dir.join(format!("{uuid}.threads.json")));
    let _ = std::fs::remove_file(dir.join(format!("{uuid}.started")));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn c_replies_on_a_thread_and_still_changes_elsewhere() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "alpha\nbeta\ngamma\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    // No thread here yet, so `c` must behave exactly as it always has.
    assert!(harness.send_keys(&mut app, "c").await?);
    assert!(harness.send_keys(&mut app, "X<esc>").await?);
    let text = {
        let view = app.editor.tree.get(app.editor.tree.focus);
        app.editor.document(view.doc).unwrap().text().to_string()
    };
    assert!(
        text.starts_with('X'),
        "c must still change the selection off a thread, got {text:?}"
    );
    assert_eq!(app.editor.diff.reviews.len(), 0);

    // Now put a thread on the line and try again.
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-s>").await?);
    assert_eq!(app.editor.diff.reviews.len(), 1);
    let before = {
        let view = app.editor.tree.get(app.editor.tree.focus);
        app.editor.document(view.doc).unwrap().text().to_string()
    };

    assert!(harness.send_keys(&mut app, "c").await?);
    assert!(harness.send_keys(&mut app, "a reply<C-s>").await?);

    let after = {
        let view = app.editor.tree.get(app.editor.tree.focus);
        app.editor.document(view.doc).unwrap().text().to_string()
    };
    assert_eq!(before, after, "c on a thread must not edit the buffer");
    assert_eq!(
        app.editor.diff.reviews.len(),
        1,
        "c on a thread replies to it rather than starting another"
    );
    let thread = app.editor.diff.reviews.iter().next().unwrap();
    assert_eq!(thread.draft.as_deref(), Some("a reply"));

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn vertical_motion_stops_on_a_comment_box() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "one\ntwo\nthree\nfour\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    let line = |app: &Application| {
        let view = app.editor.tree.get(app.editor.tree.focus);
        let doc = app.editor.document(view.doc).unwrap();
        doc.selection(view.id)
            .primary()
            .cursor_line(doc.text().slice(..))
    };

    // Comment on line 2 (index 1).
    assert!(harness.send_keys(&mut app, "j").await?);
    assert_eq!(line(&app), 1);
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-s>").await?);
    assert_eq!(line(&app), 1);

    // First `j` stops on the box: the cursor stays put and the box takes focus.
    assert!(harness.send_keys(&mut app, "j").await?);
    assert_eq!(line(&app), 1, "the first press should stop on the box");
    assert!(app.editor.diff.reviews.focused.is_some());

    // The second carries on, and focus is dropped.
    assert!(harness.send_keys(&mut app, "j").await?);
    assert_eq!(line(&app), 2);
    assert!(app.editor.diff.reviews.focused.is_none());

    // Coming back up lands on the line with its box focused.
    assert!(harness.send_keys(&mut app, "k").await?);
    assert_eq!(line(&app), 1);
    assert!(
        app.editor.diff.reviews.focused.is_some(),
        "going up should stop on the box too"
    );

    // And the next press leaves.
    assert!(harness.send_keys(&mut app, "k").await?);
    assert_eq!(line(&app), 0);
    assert!(app.editor.diff.reviews.focused.is_none());

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_count_travels_without_stopping_at_boxes() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "one\ntwo\nthree\nfour\nfive\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    assert!(harness.send_keys(&mut app, "j").await?);
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-s>").await?);
    assert!(harness.send_keys(&mut app, "gg").await?);

    // `3j` should land three lines down, not be absorbed by the box on the way.
    assert!(harness.send_keys(&mut app, "3j").await?);
    let view = app.editor.tree.get(app.editor.tree.focus);
    let doc = app.editor.document(view.doc).unwrap();
    assert_eq!(
        doc.selection(view.id)
            .primary()
            .cursor_line(doc.text().slice(..)),
        3,
        "a count should travel rather than stop at every box"
    );

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn replying_from_an_older_entry_rewinds_the_thread() -> anyhow::Result<()> {
    use helix_view::review::agent::AgentEvent;

    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "one\ntwo\nthree\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    let fake = FakeAgent::default();
    let sent = fake.sent.clone();
    app.editor.diff.agent = Some(Box::new(fake));

    // Build four entries: you, agent, you, agent.
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "first<C-s>").await?);
    assert!(harness.send_keys(&mut app, "<space>mRS").await?);
    let id = sent.lock().unwrap()[0].0;
    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Completed(id, "answer one".into()));
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "second<C-s>").await?);
    assert!(harness.send_keys(&mut app, "<space>mRS").await?);
    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Completed(id, "answer two".into()));
    assert_eq!(app.editor.diff.reviews.get(id).unwrap().entry_count(), 4);

    // Stop on the box, then walk back to entry 2 of 4.
    assert!(harness.send_keys(&mut app, "j").await?);
    assert!(harness.send_keys(&mut app, "<C-left>").await?);
    assert!(harness.send_keys(&mut app, "<C-left>").await?);
    assert_eq!(app.editor.diff.reviews.get(id).unwrap().view_index(), 1);

    // `c` from there continues the conversation from that point.
    assert!(harness.send_keys(&mut app, "c").await?);
    assert!(harness.send_keys(&mut app, "different tack<C-s>").await?);

    let thread = app.editor.diff.reviews.get(id).unwrap();
    assert_eq!(
        thread.entry_count(),
        3,
        "the two entries after the one being replied to should be gone"
    );
    assert_eq!(thread.entry(0).unwrap().text, "first");
    assert_eq!(thread.entry(1).unwrap().text, "answer one");
    assert_eq!(thread.entry(2).unwrap().text, "different tack");
    assert!(
        thread.rewound,
        "the agent still holds the dropped replies and has to be told"
    );

    // And the agent is told, once.
    assert!(harness.send_keys(&mut app, "<space>mRS").await?);
    let last = sent.lock().unwrap().last().unwrap().1.clone();
    assert!(
        last.contains("removed part of this thread"),
        "the follow-up should say the thread was edited: {last}"
    );
    assert!(!app.editor.diff.reviews.get(id).unwrap().rewound);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn pasting_into_the_comment_box_does_not_edit_the_buffer() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "one\ntwo\nthree\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    let before = {
        let view = app.editor.tree.get(app.editor.tree.focus);
        app.editor.document(view.doc).unwrap().text().to_string()
    };

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    // A paste that is ignored by the box lands in the document instead.
    harness.paste(&mut app, "pasted line\nsecond line").await?;
    assert!(harness.send_keys(&mut app, "<C-s>").await?);

    let after = {
        let view = app.editor.tree.get(app.editor.tree.focus);
        app.editor.document(view.doc).unwrap().text().to_string()
    };
    assert_eq!(
        before, after,
        "a paste into the box must not reach the buffer"
    );

    let thread = app.editor.diff.reviews.iter().next().expect("a thread");
    assert_eq!(
        thread.draft.as_deref(),
        Some("pasted line\nsecond line"),
        "the pasted text should be the comment, newlines included"
    );

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tall_reply_is_capped_and_scrolled_inside_its_box() -> anyhow::Result<()> {
    use helix_view::review::agent::AgentEvent;

    // Short file: the code fits, so there is nothing for the editor to scroll.
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "one\ntwo\nthree\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    let fake = FakeAgent::default();
    let sent = fake.sent.clone();
    app.editor.diff.agent = Some(Box::new(fake));

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-s>").await?);
    assert!(harness.send_keys(&mut app, "<space>mRS").await?);
    let id = sent.lock().unwrap()[0].0;

    let long: String = (0..200)
        .map(|n| format!("line {n} of a very long explanation"))
        .collect::<Vec<_>>()
        .join("\n");
    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Completed(id, long));

    let view_height = app.editor.tree.get(app.editor.tree.focus).inner_height();
    let rows = focused_plan_row_count(&app);
    assert!(
        rows <= view_height,
        "a box must never outgrow the window: {rows} rows in a window of {view_height}"
    );

    // The code it is about is still on screen.
    assert!(rows < view_height, "the code should still have room");

    // Where the box settles is decided while drawing, since only drawing knows
    // how tall the box was allowed to be.
    let scroll_of = |app: &Application| {
        focused_plan_row_count(app);
        app.editor.diff.reviews.get(id).unwrap().scroll.get()
    };
    let cursor_of = |app: &Application| app.editor.diff.reviews.get(id).unwrap().cursor;

    // Stopping on the box, C-down walks a cursor down the reply.
    assert!(harness.send_keys(&mut app, "j").await?);
    assert!(app.editor.diff.reviews.focused.is_some());
    assert_eq!(cursor_of(&app), 0);
    assert_eq!(scroll_of(&app), 0);

    assert!(harness.send_keys(&mut app, "<C-down>").await?);
    assert_eq!(cursor_of(&app), 1);
    assert_eq!(
        scroll_of(&app),
        0,
        "a cursor still inside the box must not scroll it"
    );

    // Walked past the bottom, the box follows rather than losing the cursor.
    let body = (view_height / 2).max(3);
    for _ in 0..body {
        assert!(harness.send_keys(&mut app, "<C-down>").await?);
    }
    assert_eq!(cursor_of(&app), body + 1);
    assert_eq!(
        scroll_of(&app),
        2,
        "the box should scroll only as far as the cursor made it"
    );

    // Coming back up, the box stays put while the cursor is still inside it ...
    assert!(harness.send_keys(&mut app, "<C-up>").await?);
    assert_eq!(cursor_of(&app), body);
    assert_eq!(
        scroll_of(&app),
        2,
        "a box should not move for a cursor that is still in it"
    );

    // ... and follows again once the cursor would leave the top.
    for _ in 0..body {
        assert!(harness.send_keys(&mut app, "<C-up>").await?);
    }
    assert_eq!(cursor_of(&app), 0);
    assert_eq!(scroll_of(&app), 0);

    // Moving to another entry starts it at the top rather than mid-scroll.
    assert!(harness.send_keys(&mut app, "<C-left>").await?);
    assert_eq!(cursor_of(&app), 0);
    assert_eq!(scroll_of(&app), 0);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(not(windows))]
async fn clicking_below_a_box_lands_on_the_line_clicked() -> anyhow::Result<()> {
    // Rows held for a box are part of the layout. Reserving them for drawing
    // but not for the coordinate maths puts everything below a box out by the
    // box's height -- a click lands that many lines too far down.
    use helix_view::review::agent::AgentEvent;
    use termina::event::{MouseButton, MouseEventKind};

    let file = tempfile::NamedTempFile::new()?;
    let text: String = (0..20).map(|n| format!("line {n}\n")).collect();
    std::fs::write(file.path(), &text)?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    let fake = FakeAgent::default();
    let sent = fake.sent.clone();
    app.editor.diff.agent = Some(Box::new(fake));

    // A thread on the first line, with a reply tall enough that ignoring its
    // rows could not be mistaken for a rounding error.
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-S-s>").await?);
    let id = sent.lock().unwrap()[0].0;
    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Completed(id, "alpha\nbeta\ngamma".into()));
    assert!(harness.send_keys(&mut app, "<esc>").await?);

    // The painter is the authority on where the box ended: the first document
    // line after it is drawn on the row below its last.
    let (last_box_row, column) = {
        let hits = app.editor.diff.reviews.hits.borrow();
        let last = hits
            .iter()
            .filter(|hit| hit.thread == id)
            .max_by_key(|hit| hit.row)
            .expect("the box should have been painted");
        (last.row, last.x + 2)
    };

    // Line 0 carries the box, so the row after it is line 1 and the next is 2.
    harness
        .mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            last_box_row + 2,
            column,
        )
        .await?;

    let line = {
        let view = app.editor.tree.get(app.editor.tree.focus);
        let doc = app.editor.document(view.doc).unwrap();
        doc.selection(view.id)
            .primary()
            .cursor_line(doc.text().slice(..))
    };
    assert_eq!(line, 2, "the click should land on the line it was over");

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[cfg(not(windows))]
async fn part_of_a_reply_is_selected_with_the_mouse_and_copied() -> anyhow::Result<()> {
    use helix_view::review::agent::AgentEvent;
    use termina::event::{MouseButton, MouseEventKind};

    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "one\ntwo\nthree\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    let fake = FakeAgent::default();
    let sent = fake.sent.clone();
    app.editor.diff.agent = Some(Box::new(fake));

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-S-s>").await?);
    let id = sent.lock().unwrap()[0].0;
    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Completed(id, "alpha\nbeta\ngamma".into()));
    assert!(harness.send_keys(&mut app, "<esc>").await?);

    // Where the rows actually landed is the renderer's answer, not the test's:
    // asking it is the same question the mouse asks.
    let row_of = |app: &Application, body: usize| -> (u16, u16) {
        let hits = app.editor.diff.reviews.hits.borrow();
        let hit = hits
            .iter()
            .find(|hit| hit.thread == id && hit.body == Some(body))
            .unwrap_or_else(|| panic!("no painted row for body line {body}"));
        (hit.row, hit.x + 2)
    };
    let (first_row, column) = row_of(&app, 0);
    let (second_row, _) = row_of(&app, 1);

    let width = {
        let view = app.editor.tree.get(app.editor.tree.focus);
        view.inner_width(app.editor.document(view.doc).unwrap()) as usize
    };

    // A press alone points at the box without selecting anything in it.
    harness
        .mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            first_row,
            column,
        )
        .await?;
    assert_eq!(app.editor.diff.reviews.focused, Some(id));
    assert_eq!(app.editor.diff.reviews.get(id).unwrap().cursor, 0);
    assert_eq!(
        app.editor
            .diff
            .reviews
            .get(id)
            .unwrap()
            .selected_rows(width),
        None
    );

    // Dragging over a second row is what makes it a selection.
    harness
        .mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            second_row,
            column,
        )
        .await?;
    assert_eq!(
        app.editor
            .diff
            .reviews
            .get(id)
            .unwrap()
            .selected_rows(width),
        Some((0, 1))
    );
    harness
        .mouse(
            &mut app,
            MouseEventKind::Up(MouseButton::Left),
            second_row,
            column,
        )
        .await?;

    let before = app
        .editor
        .document(app.editor.tree.get(app.editor.tree.focus).doc)
        .unwrap()
        .text()
        .to_string();
    assert!(harness.send_keys(&mut app, "y").await?);

    let clipboard = |app: &Application| -> Vec<String> {
        app.editor
            .registers
            .read('+', &app.editor)
            .map(|values| values.map(|value| value.to_string()).collect())
            .unwrap_or_default()
    };
    assert_eq!(clipboard(&app), vec!["alpha\nbeta".to_string()]);
    assert!(
        app.editor.registers.read('"', &app.editor).is_none(),
        "`y` on a box must not also yank the document"
    );
    assert_eq!(
        app.editor
            .document(app.editor.tree.get(app.editor.tree.focus).doc)
            .unwrap()
            .text()
            .to_string(),
        before,
        "clicking a box must not move the text cursor into the code under it"
    );
    assert!(
        app.editor.diff.reviews.get(id).unwrap().select.is_none(),
        "the selection has served its purpose once it is copied"
    );

    // Nothing selected copies the whole entry, not nothing.
    assert!(harness.send_keys(&mut app, "y").await?);
    assert_eq!(clipboard(&app), vec!["alpha\nbeta\ngamma".to_string()]);

    // And off the thread's line `y` is still an ordinary yank. A count travels
    // straight through the box rather than stopping on it.
    assert!(harness.send_keys(&mut app, "2j").await?);
    assert!(harness.send_keys(&mut app, "y").await?);
    assert!(
        app.editor.registers.read('"', &app.editor).is_some(),
        "`y` away from a box should yank the document as it always did"
    );

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn an_open_box_takes_the_place_of_the_thread_it_is_replying_to() -> anyhow::Result<()> {
    use helix_view::review::agent::AgentEvent;

    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "one\ntwo\nthree\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    let fake = FakeAgent::default();
    let sent = fake.sent.clone();
    app.editor.diff.agent = Some(Box::new(fake));

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-s>").await?);
    assert!(harness.send_keys(&mut app, "<space>mRS").await?);
    let id = sent.lock().unwrap()[0].0;

    // A tall answer, so the difference is unmistakable.
    let long: String = (0..40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Completed(id, long));
    let with_answer = focused_plan_row_count(&app);
    assert!(with_answer > 5, "the answer should occupy real space");

    // Opening a reply replaces that space rather than stacking on top of it.
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(app.editor.diff.reviews.composing.is_some());
    let while_composing = focused_plan_row_count(&app);
    assert!(
        while_composing < with_answer,
        "an open box should take the thread's place, not sit on top of it: \
         {while_composing} rows while composing vs {with_answer} before"
    );

    // Typing grows it, so the code below keeps its distance.
    assert!(harness.send_keys(&mut app, "a<ret>b").await?);
    assert!(focused_plan_row_count(&app) > while_composing);

    // Cancelling gives the thread its space back.
    assert!(harness.send_keys(&mut app, "<esc>").await?);
    assert!(app.editor.diff.reviews.composing.is_none());
    assert_eq!(focused_plan_row_count(&app), with_answer);

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn d_deletes_the_focused_entry_and_still_deletes_text_elsewhere() -> anyhow::Result<()> {
    use helix_view::review::agent::AgentEvent;

    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "one\ntwo\nthree\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    let text_of = |app: &Application| {
        let view = app.editor.tree.get(app.editor.tree.focus);
        app.editor.document(view.doc).unwrap().text().to_string()
    };

    let fake = FakeAgent::default();
    let sent = fake.sent.clone();
    app.editor.diff.agent = Some(Box::new(fake));

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "first<C-s>").await?);
    assert!(harness.send_keys(&mut app, "<space>mRS").await?);
    let id = sent.lock().unwrap()[0].0;
    app.editor
        .diff
        .reviews
        .apply_agent_event(AgentEvent::Completed(id, "answer".into()));
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "second<C-s>").await?);
    assert_eq!(app.editor.diff.reviews.get(id).unwrap().entry_count(), 3);

    // Not focused: `d` must still delete text, as it always has.
    let before = text_of(&app);
    assert!(app.editor.diff.reviews.focused.is_none());
    assert!(harness.send_keys(&mut app, "d").await?);
    assert_ne!(text_of(&app), before, "d off a box must still delete text");
    assert_eq!(app.editor.diff.reviews.get(id).unwrap().entry_count(), 3);

    // Focused, looking at the middle entry: that one goes, the others stay.
    assert!(harness.send_keys(&mut app, "j").await?);
    assert!(app.editor.diff.reviews.focused.is_some());
    assert!(harness.send_keys(&mut app, "<C-left>").await?);
    assert_eq!(app.editor.diff.reviews.get(id).unwrap().view_index(), 1);

    let text_before = text_of(&app);
    assert!(harness.send_keys(&mut app, "d").await?);
    assert_eq!(
        text_of(&app),
        text_before,
        "d on a box must not touch the buffer"
    );

    let thread = app.editor.diff.reviews.get(id).unwrap();
    assert_eq!(thread.entry_count(), 2, "only the viewed entry should go");
    assert_eq!(thread.entry(0).unwrap().text, "first");
    assert_eq!(thread.entry(1).unwrap().text, "second");
    assert!(
        thread.rewound,
        "the agent still holds the removed turn and has to be told"
    );

    harness.close(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn review_boxes_can_be_hidden_and_shown() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;
    std::fs::write(file.path(), "one\ntwo\nthree\n")?;

    let mut app = AppBuilder::new().with_file(file.path(), None).build()?;
    let mut harness = AppTestHarness::new();

    let text_of = |app: &Application| {
        let view = app.editor.tree.get(app.editor.tree.focus);
        app.editor.document(view.doc).unwrap().text().to_string()
    };

    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(harness.send_keys(&mut app, "why?<C-s>").await?);
    let shown = focused_plan_row_count(&app);
    assert!(shown > 0);

    // Hidden: nothing is drawn.
    assert!(harness.send_keys(&mut app, "<space>mRh").await?);
    assert!(app.editor.diff.reviews.hidden);
    assert_eq!(
        focused_plan_row_count(&app),
        0,
        "a hidden box must not be drawn"
    );
    assert_eq!(
        app.editor.diff.reviews.len(),
        1,
        "hiding must not touch the conversation"
    );

    // And the keys that act on boxes fall back to what they normally do.
    let before = text_of(&app);
    assert!(harness.send_keys(&mut app, "d").await?);
    assert_ne!(
        text_of(&app),
        before,
        "d should delete text while boxes are hidden"
    );
    assert_eq!(
        app.editor
            .diff
            .reviews
            .get(ThreadId(0))
            .unwrap()
            .entry_count(),
        1
    );

    // Shown again, unchanged.
    assert!(harness.send_keys(&mut app, "<space>mRh").await?);
    assert!(!app.editor.diff.reviews.hidden);
    assert_eq!(focused_plan_row_count(&app), shown);

    // Commenting while hidden brings them back, since it is asking to see them.
    assert!(harness.send_keys(&mut app, "<space>mRh").await?);
    assert!(app.editor.diff.reviews.hidden);
    assert!(harness.send_keys(&mut app, "<space>mRc").await?);
    assert!(!app.editor.diff.reviews.hidden);
    assert!(harness.send_keys(&mut app, "<esc>").await?);

    harness.close(&mut app).await?;
    Ok(())
}
