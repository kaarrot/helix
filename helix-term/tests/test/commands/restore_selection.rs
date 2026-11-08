use super::*;

use helix_core::test::print;
use helix_view::document::Mode;

async fn assert_restored(input: &str, keys: &str, expected: &str) -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text(input).build()?;
    let (text, selection) = print(expected);
    test_key_sequence(
        &mut app,
        Some(keys),
        Some(&|app| {
            let (view, doc) = current_ref!(app.editor);
            assert_eq!(doc.text(), &text);
            assert_eq!(doc.selection(view.id), &selection);
            assert_eq!(app.editor.mode(), Mode::Select);
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_after_escape_and_movement() -> anyhow::Result<()> {
    assert_restored("#[a|]#bcdef\n", "vll<esc>llgv", "#[abc|]#def\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_after_toggling_select_mode() -> anyhow::Result<()> {
    assert_restored("#[a|]#bcdef\n", "vllvllgv", "#[abc|]#def\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_preserves_direction() -> anyhow::Result<()> {
    assert_restored("abc#[d|]#ef\n", "vhh<esc>llgv", "a#[|bcd]#ef\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_after_normal_mode_line_selection() -> anyhow::Result<()> {
    assert_restored("#[a|]#bc\ndef\nghi\n", "xjgv", "#[abc\n|]#def\nghi\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_after_repeated_collapse() -> anyhow::Result<()> {
    assert_restored("ab#[cde|]#fg\n", ";llhh;;gv", "ab#[cde|]#fg\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_after_collapse_in_select_mode() -> anyhow::Result<()> {
    assert_restored("#[a|]#bcdef\n", "vll;gv", "#[abc|]#def\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_swaps_in_select_mode() -> anyhow::Result<()> {
    assert_restored("#[a|]#bcdefgh\n", "vll<esc>;lllvlgv", "#[abc|]#defgh\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_swaps_back_in_select_mode() -> anyhow::Result<()> {
    assert_restored("#[a|]#bcdefgh\n", "vll<esc>;lllvlgvgv", "abcde#[fg|]#h\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_can_be_repeated_after_leaving() -> anyhow::Result<()> {
    assert_restored(
        "#[a|]#bcdefgh\n",
        "vll<esc>;llgv<esc>;llgv",
        "#[abc|]#defgh\n",
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_ignores_unicode_cursor_movement() -> anyhow::Result<()> {
    assert_restored(
        "#[a|]#b e\u{301}👩‍💻 z\n",
        "vll<esc>glhhhgv",
        "#[ab |]#e\u{301}👩‍💻 z\n",
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_follows_insertions() -> anyhow::Result<()> {
    assert_restored("ab#[cde|]#fg\n", ";ggiXX<esc>gv", "XXab#[cde|]#fg\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_after_entering_insert_mode() -> anyhow::Result<()> {
    assert_restored("#[a|]#bcdef\n", "vllaX<esc>gv", "#[abc|]#Xdef\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_follows_undo() -> anyhow::Result<()> {
    assert_restored("ab#[cde|]#fg\n", ";ggiXX<esc>ugv", "ab#[cde|]#fg\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_follows_redo() -> anyhow::Result<()> {
    assert_restored("ab#[cde|]#fg\n", ";ggiXX<esc>uUgv", "XXab#[cde|]#fg\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_after_deleting_selected_text() -> anyhow::Result<()> {
    assert_restored("ab#[cde|]#fg\n", "dllgv", "ab#[f|]#g\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_after_deleting_entire_document() -> anyhow::Result<()> {
    assert_restored("#[abc|]#", "dgv", "#[|]#").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_preserves_multiple_ranges_and_primary() -> anyhow::Result<()> {
    assert_restored("#(|ab)#cd#[ef|]#gh\n", ";,llgv", "#(|ab)#cd#[ef|]#gh\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_preserves_multiple_cursors() -> anyhow::Result<()> {
    assert_restored("#[a|]#b#(c|)#d\n", ",llgv", "#[a|]#b#(c|)#d\n").await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_without_saved_selection() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text("#[a|]#bc\n").build()?;
    test_key_sequence(
        &mut app,
        Some("gv"),
        Some(&|app| {
            let (view, doc) = current_ref!(app.editor);
            assert_eq!(doc.selection(view.id), &Selection::single(0, 1));
            assert_eq!(app.editor.mode(), Mode::Normal);
            assert_eq!(app.editor.get_status().unwrap().0, "No previous selection");
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_is_separate_for_each_buffer() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text("#[abc|]#def\n").build()?;
    let original = doc!(app.editor).id();
    send_key_sequence(&mut app, ";ll").await?;
    app.editor.new_file(Action::Replace);
    send_key_sequence(&mut app, "gv").await?;
    assert_eq!(app.editor.mode(), Mode::Normal);
    assert_eq!(app.editor.get_status().unwrap().0, "No previous selection");
    app.editor.switch(original, Action::Replace);
    test_key_sequence(
        &mut app,
        Some("gv"),
        Some(&|app| {
            let (view, doc) = current_ref!(app.editor);
            assert_eq!(doc.selection(view.id), &Selection::single(0, 3));
            assert_eq!(app.editor.mode(), Mode::Select);
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_selection_is_separate_for_each_view() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text("#[abc|]#def\n").build()?;
    send_key_sequence(&mut app, ";ll").await?;
    let first_view = app.editor.tree.focus;
    let original = doc!(app.editor).id();
    app.editor.switch(original, Action::VerticalSplit);
    send_key_sequence(&mut app, "gv").await?;
    assert_eq!(app.editor.mode(), Mode::Normal);
    assert_eq!(app.editor.get_status().unwrap().0, "No previous selection");
    let second_view = app.editor.tree.focus;
    app.editor.close(second_view);
    assert!(doc!(app.editor).previous_selection(second_view).is_none());
    assert_eq!(app.editor.tree.focus, first_view);
    test_key_sequence(
        &mut app,
        Some("gv"),
        Some(&|app| {
            let (view, doc) = current_ref!(app.editor);
            assert_eq!(doc.selection(view.id), &Selection::single(0, 3));
            assert_eq!(app.editor.mode(), Mode::Select);
        }),
        false,
    )
    .await
}
