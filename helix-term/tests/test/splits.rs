use super::*;

use helix_stdx::path;

#[tokio::test(flavor = "multi_thread")]
async fn test_split_write_quit_all() -> anyhow::Result<()> {
    let mut file1 = tempfile::NamedTempFile::new()?;
    let mut file2 = tempfile::NamedTempFile::new()?;
    let mut file3 = tempfile::NamedTempFile::new()?;

    let mut app = helpers::AppBuilder::new()
        .with_file(file1.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            (
                Some(&format!(
                    "ihello1<esc>:sp<ret>:o {}<ret>ihello2<esc>:sp<ret>:o {}<ret>ihello3<esc>",
                    file2.path().to_string_lossy(),
                    file3.path().to_string_lossy()
                )),
                Some(&|app| {
                    let docs: Vec<_> = app.editor.documents().collect();
                    assert_eq!(3, docs.len());

                    let doc1 = docs
                        .iter()
                        .find(|doc| doc.path().unwrap() == &path::normalize(file1.path()))
                        .unwrap();

                    assert_eq!("hello1", doc1.text().to_string());

                    let doc2 = docs
                        .iter()
                        .find(|doc| doc.path().unwrap() == &path::normalize(file2.path()))
                        .unwrap();

                    assert_eq!("hello2", doc2.text().to_string());

                    let doc3 = docs
                        .iter()
                        .find(|doc| doc.path().unwrap() == &path::normalize(file3.path()))
                        .unwrap();

                    assert_eq!("hello3", doc3.text().to_string());

                    helpers::assert_status_not_error(&app.editor);
                    assert_eq!(3, app.editor.tree.views().count());
                }),
            ),
            (
                Some(":wqa<ret>"),
                Some(&|app| {
                    helpers::assert_status_not_error(&app.editor);
                    assert_eq!(0, app.editor.tree.views().count());
                }),
            ),
        ],
        true,
    )
    .await?;

    helpers::assert_file_has_content(&mut file1, &LineFeedHandling::Native.apply("hello1"))?;
    helpers::assert_file_has_content(&mut file2, &LineFeedHandling::Native.apply("hello2"))?;
    helpers::assert_file_has_content(&mut file3, &LineFeedHandling::Native.apply("hello3"))?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_split_write_quit_same_file() -> anyhow::Result<()> {
    let mut file = tempfile::NamedTempFile::new()?;
    let mut app = helpers::AppBuilder::new()
        .with_file(file.path(), None)
        .build()?;

    test_key_sequences(
        &mut app,
        vec![
            (
                Some("O<esc>ihello<esc>:sp<ret>ogoodbye<esc>"),
                Some(&|app| {
                    assert_eq!(2, app.editor.tree.views().count());
                    helpers::assert_status_not_error(&app.editor);

                    let mut docs: Vec<_> = app.editor.documents().collect();
                    assert_eq!(1, docs.len());

                    let doc = docs.pop().unwrap();

                    assert_eq!(
                        LineFeedHandling::Native.apply("hello\ngoodbye"),
                        doc.text().to_string()
                    );

                    assert!(doc.is_modified());
                }),
            ),
            (
                Some(":wq<ret>"),
                Some(&|app| {
                    helpers::assert_status_not_error(&app.editor);
                    assert_eq!(1, app.editor.tree.views().count());

                    let mut docs: Vec<_> = app.editor.documents().collect();
                    assert_eq!(1, docs.len());

                    let doc = docs.pop().unwrap();

                    assert_eq!(
                        LineFeedHandling::Native.apply("hello\ngoodbye"),
                        doc.text().to_string()
                    );

                    assert!(!doc.is_modified());
                }),
            ),
        ],
        false,
    )
    .await?;

    helpers::assert_file_has_content(&mut file, &LineFeedHandling::Native.apply("hello\ngoodbye"))?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_changes_in_splits_apply_to_all_views() -> anyhow::Result<()> {
    // See <https://github.com/helix-editor/helix/issues/4732>.
    // Transactions must be applied to any view that has the changed document open.
    // This sequence would panic since the jumplist entry would be modified in one
    // window but not the other. Attempting to update the changelist in the other
    // window would cause a panic since it would point outside of the document.

    // The key sequence here:
    // * <C-w>v       Create a vertical split of the current buffer.
    //                Both views look at the same doc.
    // * [<space>     Add a line ending to the beginning of the document.
    //                The cursor is now at line 2 in window 2.
    // * <C-s>        Save that selection to the jumplist in window 2.
    // * <C-w>w       Switch to window 1.
    // * kd           Delete line 1 in window 1.
    // * <C-w>q       Close window 1, focusing window 2.
    // * d            Delete line 1 in window 2.
    //
    // This panicked in the past because the jumplist entry on line 2 of window 2
    // was not updated and after the `kd` step, pointed outside of the document.
    test((
        "#[|]#",
        "<C-w>v[<space><C-s><C-w>wkd<C-w>qd",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    // Transactions are applied to the views for windows lazily when they are focused.
    // This case panics if the transactions and inversions are not applied in the
    // correct order as we switch between windows.
    test((
        "#[|]#",
        "[<space>[<space>[<space><C-w>vuuu<C-w>wUUU<C-w>quuu",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    // See <https://github.com/helix-editor/helix/issues/4957>.
    // This sequence undoes part of the history and then adds new changes, creating a
    // new branch in the history tree. `View::sync_changes` applies transactions down
    // and up to the lowest common ancestor in the path between old and new revision
    // numbers. If we apply these up/down transactions in the wrong order, this case
    // panics.
    // The key sequence:
    // * 3[<space>    Create three empty lines so we are at the end of the document.
    // * <C-w>v<C-s>  Create a split and save that point at the end of the document
    //                in the jumplist.
    // * <C-w>w       Switch back to the first window.
    // * uu           Undo twice (not three times which would bring us back to the
    //                root of the tree).
    // * 3[<space>    Create three empty lines. Now the end of the document is past
    //                where it was on step 1.
    // * <C-w>q       Close window 1, focusing window 2 and causing a sync. This step
    //                panics if we don't apply in the right order.
    // * %d           Clean up the buffer.
    test((
        "#[|]#",
        "3[<space><C-w>v<C-s><C-w>wuu3[<space><C-w>q%d",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    Ok(())
}

#[cfg(not(windows))]
#[tokio::test(flavor = "multi_thread")]
async fn test_coalesced_click_and_enter_cancels_prompt() -> anyhow::Result<()> {
    use helix_view::input::parse_macro;
    use termina::event::{
        Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, Modifiers, MouseButton, MouseEvent,
        MouseEventKind,
    };
    use tokio_stream::wrappers::UnboundedReceiverStream;

    let file_a = tempfile::NamedTempFile::new()?;
    let file_b = tempfile::NamedTempFile::new()?;
    std::fs::write(file_a.path(), "alpha\n")?;
    std::fs::write(file_b.path(), "bravo\n")?;

    let mut app = helpers::AppBuilder::new()
        .with_file(file_a.path(), None)
        .build()?;

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let mut rx_stream = UnboundedReceiverStream::new(rx);

    let setup = format!(
        ":vsplit {}<ret>A UNSAVED<esc><C-w>h:buffer-close!",
        file_b.path().to_string_lossy()
    );
    for key_event in parse_macro(&setup)? {
        tx.send(Ok(Event::Key(KeyEvent::from(key_event))))?;
    }
    assert!(app.event_loop_until_idle(&mut rx_stream).await);
    assert_eq!(2, app.editor.tree.views().count());

    let right_view_area = app
        .editor
        .tree
        .views()
        .find(|(view, _)| {
            app.editor
                .document(view.doc)
                .and_then(|doc| doc.path())
                .is_some_and(|p| p == &path::normalize(file_b.path()))
        })
        .map(|(view, _)| view.area)
        .expect("right split showing B.txt");

    // Enqueue click on B.txt and Enter together so both arrive in one coalesced batch.
    tx.send(Ok(Event::Mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: right_view_area.x + 1,
        row: right_view_area.y + 1,
        modifiers: Modifiers::NONE,
    })))?;
    tx.send(Ok(Event::Key(KeyEvent {
        code: KeyCode::Enter,
        modifiers: Modifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    })))?;

    assert!(app.event_loop_until_idle(&mut rx_stream).await);

    // Focus moved to B.txt, and the pending `:buffer-close!` prompt from A.txt
    // was dismissed before Enter could discard B.txt's unsaved edit.
    assert_eq!(2, app.editor.tree.views().count());
    let doc_b = app
        .editor
        .documents()
        .find(|doc| doc.path().is_some_and(|p| p == &path::normalize(file_b.path())))
        .expect("B.txt should still be open");
    assert_eq!("bravo UNSAVED\n", doc_b.text().to_string());
    assert!(doc_b.is_modified());

    app.close().await;
    Ok(())
}
