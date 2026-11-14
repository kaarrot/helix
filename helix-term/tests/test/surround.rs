use super::*;

use helix_core::test::print;
use helix_view::{current_ref, document::Mode};

fn assert_state(app: &helix_term::application::Application, expected: &str) {
    let (text, selection) = print(expected);
    let (view, doc) = current_ref!(app.editor);
    assert_eq!(doc.text(), &text);
    assert_eq!(doc.selection(view.id), &selection);
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_prompt_waits_for_enter() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text("#[hello|]#\n").build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("ms\"\"\""),
                Some(&|app| assert_state(app, "#[hello|]#\n")),
            ),
            (
                Some("<ret>"),
                Some(&|app| assert_state(app, "#[\"\"\"hello\"\"\"|]#\n")),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_prompt_delimiters() -> anyhow::Result<()> {
    for (keys, expected) in [
        ("ms\"<ret>", "#[\"hello\"|]#\n"),
        ("ms'''<ret>", "#['''hello'''|]#\n"),
        ("ms((<ret>", "#[((hello))|]#\n"),
        ("ms))<ret>", "#[((hello))|]#\n"),
        ("ms**<ret>", "#[**hello**|]#\n"),
        ("msab<ret>", "#[abhelloab|]#\n"),
        ("ms<space><ret>", "#[ hello |]#\n"),
        ("ms「「<ret>", "#[「「hello」」|]#\n"),
    ] {
        test(("#[hello|]#\n", keys, expected)).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_prompt_escapes() -> anyhow::Result<()> {
    // The annotation parser consumes one repeated line ending after a selection.
    for (keys, expected) in [
        (r"ms\n<ret>", "#[\nhello\n|]#\n\n"),
        (r"ms\n\n<ret>", "#[\n\nhello\n\n|]#\n\n"),
        (r"ms\\<ret>", "#[\\hello\\|]#\n"),
        (r"ms\\n<ret>", "#[\\nhello\\n|]#\n"),
        (r"ms\q<ret>", "#[\\qhello\\q|]#\n"),
        (r"ms\<ret>", "#[\\hello\\|]#\n"),
    ] {
        test(("#[hello|]#\n", keys, expected)).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_prompt_uses_document_line_endings() -> anyhow::Result<()> {
    test((
        "#[hello|]#\r\n",
        r":line-ending crlf<ret>ms\n<ret>",
        "#[\r\nhello\r\n|]#\r\n\r\n",
        LineFeedHandling::AsIs,
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_prompt_works_in_keybinding_macros() -> anyhow::Result<()> {
    let config = Config::load(
        Ok("[keys.normal.m]\nQ = '@ms\"\"\"<ret>'\n".into()),
        Ok(String::new()),
    )
    .expect("valid macro keybinding configuration");
    test_with_config(
        AppBuilder::new().with_config(config),
        ("#[hello|]#\n", "mQ", "#[\"\"\"hello\"\"\"|]#\n"),
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_prompt_edits_and_cancels() -> anyhow::Result<()> {
    for (keys, expected) in [
        ("ms\"\"\"<backspace><ret>", "#[\"\"hello\"\"|]#\n"),
        ("msé<backspace>'<ret>", "#['hello'|]#\n"),
        ("ms\"\"\"<esc>", "#[hello|]#\n"),
        ("ms<ret>", "#[hello|]#\n"),
        ("ms\"<backspace><ret>", "#[hello|]#\n"),
    ] {
        test(("#[hello|]#\n", keys, expected)).await?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_prompt_preserves_selections() -> anyhow::Result<()> {
    test((
        "#(|café)# #[λ|]#\n",
        "ms\"\"\"<ret>",
        "#(|\"\"\"café\"\"\")# #[\"\"\"λ\"\"\"|]#\n",
    ))
    .await?;
    test((
        "#[a|]##(b|)#\n",
        "ms\"\"\"<ret>",
        "#[\"\"\"a\"\"\"|]##(\"\"\"b\"\"\"|)#\n",
    ))
    .await?;
    test((
        "#[|]#",
        "ms\"\"\"<ret>",
        "#[\"\"\"\"\"\"|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;
    test((
        "#[one\ntwo|]#\n",
        "ms\"\"\"<ret>",
        "#[\"\"\"one\ntwo\"\"\"|]#\n",
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_prompt_commits_one_undo_step() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text("#[hello|]#\n").build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("ms\"\"\"<ret>"),
                Some(&|app| assert_state(app, "#[\"\"\"hello\"\"\"|]#\n")),
            ),
            (Some("u"), Some(&|app| assert_state(app, "#[hello|]#\n"))),
            (
                Some("U"),
                Some(&|app| assert_state(app, "#[\"\"\"hello\"\"\"|]#\n")),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_prompt_keeps_cursor_in_view() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text("#[hello|]#\n").build()?;
    let (view, _) = current_ref!(app.editor);
    let delimiter = r"\n".repeat(view.inner_height() + 2);
    test_key_sequence(
        &mut app,
        Some(&format!("ms{delimiter}<ret>")),
        Some(&|app| {
            let (view, doc) = current_ref!(app.editor);
            let cursor = doc
                .selection(view.id)
                .primary()
                .cursor(doc.text().slice(..));
            assert!(view
                .screen_coords_at_pos(doc, doc.text().slice(..), cursor)
                .is_some());
        }),
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_prompt_preserves_select_mode_on_cancel() -> anyhow::Result<()> {
    let mut app = AppBuilder::new().with_input_text("#[hello|]#\n").build()?;
    test_key_sequences(
        &mut app,
        vec![
            (
                Some("vms\"\"\"<esc>"),
                Some(&|app| {
                    assert_state(app, "#[hello|]#\n");
                    assert_eq!(app.editor.mode(), Mode::Select);
                }),
            ),
            (
                Some("ms<ret>"),
                Some(&|app| {
                    assert_state(app, "#[hello|]#\n");
                    assert_eq!(app.editor.mode(), Mode::Select);
                }),
            ),
            (
                Some("ms\"\"\"<ret>"),
                Some(&|app| {
                    assert_state(app, "#[\"\"\"hello\"\"\"|]#\n");
                    assert_eq!(app.editor.mode(), Mode::Normal);
                }),
            ),
        ],
        false,
    )
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn space_textobject_stops_at_line_boundaries() -> anyhow::Result<()> {
    for (input, expected) in [
        (
            "left word\nhe#[l|]#lo\nnext word\n",
            "left word\n#[hello|]#\nnext word\n",
        ),
        ("he#[l|]#lo world\n", "#[hello|]# world\n"),
        ("hello wo#[r|]#ld\n", "hello #[world|]#\n"),
        ("a b#[c|]#d e\n", "a #[bcd|]# e\n"),
        ("he#[|l]#lo\n", "#[|hello]#\n"),
        ("#[\n|]#next\n", "#[\n|]#next\n"),
    ] {
        test((input, "mi<space>", expected)).await?;
    }
    test(("a b#[c|]#d e\n", "2mi<space>", "#[a bcd e|]#\n")).await?;
    test(("a b#[c|]#d e\n", "vmi<space>", "a #[bcd|]# e\n")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn space_textobject_excludes_crlf() -> anyhow::Result<()> {
    test((
        "hello#[\r\n|]#next\r\n",
        "mi<space>",
        "#[hello|]#\r\nnext\r\n",
        LineFeedHandling::AsIs,
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn space_textobject_preserves_multiple_selections() -> anyhow::Result<()> {
    test(("ca#(f|)#é\nλ#[x|]#\n", "mi<space>", "#(café|)#\n#[λx|]#\n")).await
}

#[tokio::test(flavor = "multi_thread")]
async fn space_textobject_multiline_keeps_previous_behavior() -> anyhow::Result<()> {
    for keys in ["mI<space>", "vmI<space>"] {
        test((
            "left word\nhe#[l|]#lo\nnext word\n",
            keys,
            "left #[word\nhello\nnext|]# word\n",
        ))
        .await?;
    }
    test((
        "left word\nhe#[|l]#lo\nnext word\n",
        "mI<space>",
        "left #[|word\nhello\nnext]# word\n",
    ))
    .await?;
    test(("he#[l|]#lo\n", "mI<space>", "he#[l|]#lo\n")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn other_textobjects_keep_multiline_matching() -> anyhow::Result<()> {
    for keys in ["mi'", "mI'"] {
        test((
            "'first\nhe#[l|]#lo\nlast'\n",
            keys,
            "'#[first\nhello\nlast|]#'\n",
        ))
        .await?;
    }
    test(("he#[l|]#lo\n", "mIw", "#[hello|]#\n")).await?;
    test((
        "left word\nhe#[l|]#lo\nnext word\n",
        "ma<space>",
        "left#[ word\nhello\nnext |]#word\n",
    ))
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn space_textobject_and_surround_keep_newline_outside() -> anyhow::Result<()> {
    test((
        "left word\nhe#[l|]#lo\nnext word\n",
        "mi<space>ms<space><ret>",
        "left word\n#[ hello |]#\nnext word\n",
    ))
    .await
}
