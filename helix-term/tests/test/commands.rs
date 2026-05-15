use helix_term::application::Application;
use helix_view::current_ref;
use helix_view::doc;
use helix_view::editor::Action;
use once_cell::sync::Lazy;
use std::sync::Arc;

use super::*;

mod insert;
mod movement;
mod reverse_selection_contents;
mod rotate_selection_contents;
mod write;

static STREAM_TEST_LOCK: Lazy<Arc<tokio::sync::Mutex<()>>> =
    Lazy::new(|| Arc::new(tokio::sync::Mutex::new(())));

#[cfg(unix)]
async fn assert_stream_output_contains(
    input: &str,
    keys: &str,
    expected: &str,
) -> anyhow::Result<String> {
    let mut config = helpers::test_config();
    config.editor.word_completion.enable = false;
    let mut app = AppBuilder::new()
        .with_config(config)
        .with_input_text(input)
        .build()?;

    send_key_sequence(&mut app, keys).await?;
    wait_for_condition(&mut app, expected, |app| {
        doc!(app.editor).text().to_string().contains(expected)
            && app
                .editor
                .get_status()
                .is_some_and(|(status, _)| status.as_ref().starts_with("Stream completed in "))
    })
    .await?;

    let text = doc!(app.editor).text().to_string();
    assert!(
        text.contains(expected),
        "expected streamed output to contain {expected:?}, got {text:?}"
    );

    close_app(&mut app).await?;
    Ok(text)
}

#[tokio::test(flavor = "multi_thread")]
async fn search_selection_detect_word_boundaries_at_eof() -> anyhow::Result<()> {
    // <https://github.com/helix-editor/helix/issues/12609>
    test((
        indoc! {"\
            #[o|]#ne
            two
            three"},
        "gej*h",
        indoc! {"\
            one
            two
            three#[
            |]#"},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_selection_duplication() -> anyhow::Result<()> {
    // Forward
    test((
        indoc! {"\
            #[lo|]#rem
            ipsum
            dolor
            "},
        "CC",
        indoc! {"\
            #(lo|)#rem
            #(ip|)#sum
            #[do|]#lor
            "},
    ))
    .await?;

    // Backward
    test((
        indoc! {"\
            #[|lo]#rem
            ipsum
            dolor
            "},
        "CC",
        indoc! {"\
            #(|lo)#rem
            #(|ip)#sum
            #[|do]#lor
            "},
    ))
    .await?;

    // Copy the selection to previous line, skipping the first line in the file
    test((
        indoc! {"\
            test
            #[testitem|]#
            "},
        "<A-C>",
        indoc! {"\
            test
            #[testitem|]#
            "},
    ))
    .await?;

    // Copy the selection to previous line, including the first line in the file
    test((
        indoc! {"\
            test
            #[test|]#
            "},
        "<A-C>",
        indoc! {"\
            #[test|]#
            #(test|)#
            "},
    ))
    .await?;

    // Copy the selection to next line, skipping the last line in the file
    test((
        indoc! {"\
            #[testitem|]#
            test
            "},
        "C",
        indoc! {"\
            #[testitem|]#
            test
            "},
    ))
    .await?;

    // Copy the selection to next line, including the last line in the file
    test((
        indoc! {"\
            #[test|]#
            test
            "},
        "C",
        indoc! {"\
            #(test|)#
            #[test|]#
            "},
    ))
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_goto_file_impl() -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new()?;

    fn match_paths(app: &Application, matches: Vec<&str>) -> usize {
        app.editor
            .documents()
            .filter_map(|d| d.path()?.file_name())
            .filter(|n| matches.iter().any(|m| *m == n.to_string_lossy()))
            .count()
    }

    // Single selection
    test_key_sequence(
        &mut AppBuilder::new().with_file(file.path(), None).build()?,
        Some("ione.js<esc>%gf"),
        Some(&|app| {
            assert_eq!(1, match_paths(app, vec!["one.js"]));
        }),
        false,
    )
    .await?;

    // Multiple selection
    test_key_sequence(
        &mut AppBuilder::new().with_file(file.path(), None).build()?,
        Some("ione.js<ret>two.js<esc>%<A-s>gf"),
        Some(&|app| {
            assert_eq!(2, match_paths(app, vec!["one.js", "two.js"]));
        }),
        false,
    )
    .await?;

    // Cursor on first quote
    test_key_sequence(
        &mut AppBuilder::new().with_file(file.path(), None).build()?,
        Some("iimport 'one.js'<esc>B;gf"),
        Some(&|app| {
            assert_eq!(1, match_paths(app, vec!["one.js"]));
        }),
        false,
    )
    .await?;

    // Cursor on last quote
    test_key_sequence(
        &mut AppBuilder::new().with_file(file.path(), None).build()?,
        Some("iimport 'one.js'<esc>bgf"),
        Some(&|app| {
            assert_eq!(1, match_paths(app, vec!["one.js"]));
        }),
        false,
    )
    .await?;

    // ';' is behind the path
    test_key_sequence(
        &mut AppBuilder::new().with_file(file.path(), None).build()?,
        Some("iimport 'one.js';<esc>B;gf"),
        Some(&|app| {
            assert_eq!(1, match_paths(app, vec!["one.js"]));
        }),
        false,
    )
    .await?;

    // allow numeric values in path
    test_key_sequence(
        &mut AppBuilder::new().with_file(file.path(), None).build()?,
        Some("iimport 'one123.js'<esc>B;gf"),
        Some(&|app| {
            assert_eq!(1, match_paths(app, vec!["one123.js"]));
        }),
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_multi_selection_paste() -> anyhow::Result<()> {
    test((
        indoc! {"\
            #[|lorem]#
            #(|ipsum)#
            #(|dolor)#
            "},
        "yp",
        indoc! {"\
            lorem#[|lorem]#
            ipsum#(|ipsum)#
            dolor#(|dolor)#
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_multi_selection_shell_commands() -> anyhow::Result<()> {
    // pipe
    test((
        indoc! {"\
            #[|lorem]#
            #(|ipsum)#
            #(|dolor)#
            "},
        "|echo foo<ret>",
        indoc! {"\
            #[|foo]#
            #(|foo)#
            #(|foo)#"
        },
    ))
    .await?;

    // insert-output
    test((
        indoc! {"\
            #[|lorem]#
            #(|ipsum)#
            #(|dolor)#
            "},
        "!echo foo<ret>",
        indoc! {"\
            #[|foo]#lorem
            #(|foo)#ipsum
            #(|foo)#dolor
            "},
    ))
    .await?;

    // append-output
    test((
        indoc! {"\
            #[|lorem]#
            #(|ipsum)#
            #(|dolor)#
            "},
        "<A-!>echo foo<ret>",
        indoc! {"\
            lorem#[|foo]#
            ipsum#(|foo)#
            dolor#(|foo)#
            "},
    ))
    .await?;

    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn test_insert_stream_output_appends_selection_with_spaces() -> anyhow::Result<()> {
    let _lock = Arc::clone(&STREAM_TEST_LOCK).lock_owned().await;

    assert_stream_output_contains(
        "#[hello world|]#\n",
        ":: printf '[%%s]'<ret>",
        "[hello world]",
    )
    .await?;

    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn test_insert_stream_output_shell_quotes_appended_selection() -> anyhow::Result<()> {
    let _lock = Arc::clone(&STREAM_TEST_LOCK).lock_owned().await;

    assert_stream_output_contains(
        "#[don't|]#\n",
        ":insert-stream-output printf '[%%s]'<ret>",
        "[don't]",
    )
    .await?;

    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn test_insert_stream_output_trims_linewise_selection_line_ending() -> anyhow::Result<()> {
    let _lock = Arc::clone(&STREAM_TEST_LOCK).lock_owned().await;

    let text =
        assert_stream_output_contains("#[|]#comment\n", "x:: printf '[%%s]'<ret>", "[comment]")
            .await?;
    assert!(
        !text.contains("[comment\n]"),
        "linewise selection should not append its trailing line ending: {text:?}"
    );

    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn test_insert_stream_output_trims_multiline_linewise_selection_line_ending(
) -> anyhow::Result<()> {
    let _lock = Arc::clone(&STREAM_TEST_LOCK).lock_owned().await;

    let text =
        assert_stream_output_contains("#[|]#a\nb\n", "2x:: printf '[%%s]'<ret>", "[a\nb]").await?;
    assert!(
        !text.contains("[a\nb\n]"),
        "multi-line linewise selection should append one argument without the final line ending: {text:?}"
    );

    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn test_insert_stream_output_empty_selection_does_not_append_arg() -> anyhow::Result<()> {
    let _lock = Arc::clone(&STREAM_TEST_LOCK).lock_owned().await;

    let text =
        assert_stream_output_contains("#[|]#", ":: printf foo<ret>", "printf foo\nfoo").await?;
    assert!(
        !text.contains("''"),
        "empty primary selection should not append a shell argument: {text:?}"
    );

    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn test_insert_stream_output_sends_input_without_selection_arg() -> anyhow::Result<()> {
    let _lock = Arc::clone(&STREAM_TEST_LOCK).lock_owned().await;
    let mut app = AppBuilder::new().build()?;

    send_key_sequence(&mut app, ":insert-stream-output printf ready; cat<ret>").await?;
    wait_for_condition(&mut app, "initial streamed output", |app| {
        doc!(app.editor)
            .text()
            .to_string()
            .contains("printf ready; cat\nready")
    })
    .await?;

    send_key_sequence(&mut app, "ggx:insert-stream-output hello<ret>").await?;
    wait_for_condition(&mut app, "stream input echo", |app| {
        let text = doc!(app.editor).text().to_string();
        text.contains(">>> hello\n") && text.matches("hello\n").count() >= 2
    })
    .await?;

    {
        let text = doc!(app.editor).text().to_string();
        assert!(text.contains(">>> hello\n"));
        assert!(
            !text.contains(">>> hello printf ready; cat"),
            "running-stream input should not append the editor selection: {text:?}"
        );
    }

    send_key_sequence(&mut app, ":::<ret>").await?;
    wait_for_condition(&mut app, "stream cancellation", |app| {
        app.editor
            .get_status()
            .is_some_and(|(status, _)| status.as_ref() == "Stream cancelled (buffer: [scratch])")
    })
    .await?;

    close_app(&mut app).await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn test_insert_stream_commands_and_aliases() -> anyhow::Result<()> {
    let _lock = Arc::clone(&STREAM_TEST_LOCK).lock_owned().await;
    let mut app = AppBuilder::new().build()?;

    send_key_sequence(&mut app, ":: printf foo<ret>").await?;
    wait_for_condition(&mut app, "stream alias output", |app| {
        let text = doc!(app.editor).text().to_string();
        text.contains("printf foo\nfoo")
            && app
                .editor
                .get_status()
                .is_some_and(|(status, _)| status.as_ref().starts_with("Stream completed in "))
    })
    .await?;

    {
        let doc = doc!(app.editor);
        assert!(doc.text().to_string().contains("printf foo\nfoo"));
        assert_eq!(
            app.editor.get_status().unwrap().0.as_ref(),
            "Stream completed in '[scratch]'"
        );
    }

    send_key_sequence(&mut app, ":new<ret>").await?;
    wait_for_condition(&mut app, "fresh scratch buffer", |app| {
        let doc = doc!(app.editor);
        doc.text().to_string() == "\n" && doc.display_name().as_ref() == "[scratch]"
    })
    .await?;

    send_key_sequence(&mut app, ":insert-stream-output printf one; cat<ret>").await?;
    wait_for_condition(&mut app, "initial streamed output", |app| {
        doc!(app.editor).text().to_string() == "printf one; cat\none\n"
    })
    .await?;

    {
        let doc = doc!(app.editor);
        assert_eq!(doc.display_name().as_ref(), "printf one; cat");
    }

    send_key_sequence(&mut app, ":insert-stream-output hello<ret>").await?;
    wait_for_condition(&mut app, "stream input echo", |app| {
        let text = doc!(app.editor).text().to_string();
        text.contains(">>> hello\n") && text.matches("hello\n").count() >= 2
    })
    .await?;

    {
        let doc = doc!(app.editor);
        let text = doc.text().to_string();

        assert_eq!(doc.display_name().as_ref(), "printf one; cat");
        assert!(text.starts_with("printf one; cat\none"));
        assert!(text.contains(">>> hello\n"));
        assert!(text.matches("hello\n").count() >= 2);
        assert_eq!(
            app.editor.get_status().unwrap().0.as_ref(),
            "Input sent to '[scratch]'"
        );
    }

    send_key_sequence(&mut app, ":::<ret>").await?;
    wait_for_condition(&mut app, "stream cancellation", |app| {
        app.editor
            .get_status()
            .is_some_and(|(status, _)| status.as_ref() == "Stream cancelled (buffer: [scratch])")
    })
    .await?;

    {
        let doc = doc!(app.editor);
        let text = doc.text().to_string();

        assert_eq!(doc.display_name().as_ref(), "printf one; cat");
        assert!(text.contains(">>> hello\n"));
        assert!(text.matches("hello\n").count() >= 2);
        assert_eq!(
            app.editor.get_status().unwrap().0.as_ref(),
            "Stream cancelled (buffer: [scratch])"
        );
    }

    close_app(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_insert_stream_output_reuses_last_stream_buffer() -> anyhow::Result<()> {
    let _lock = Arc::clone(&STREAM_TEST_LOCK).lock_owned().await;
    let source_file = helpers::temp_file_with_contents("source\n")?;
    let source_path = source_file.path().to_path_buf();
    let mut app = AppBuilder::new().with_file(&source_path, None).build()?;

    let source_doc_id = app
        .editor
        .documents()
        .find(|doc| doc.path().is_some_and(|path| path == &source_path))
        .expect("source file should be open")
        .id();

    send_key_sequence(&mut app, ":new<ret>").await?;
    wait_for_condition(&mut app, "fresh scratch buffer", |app| {
        let doc = doc!(app.editor);
        doc.display_name().as_ref() == "[scratch]"
    })
    .await?;

    send_key_sequence(&mut app, ":insert-stream-output echo one<ret>").await?;
    wait_for_condition(&mut app, "initial stream output", |app| {
        let text = doc!(app.editor).text().to_string();
        text.contains("echo one\none")
    })
    .await?;

    let output_doc_id = doc!(app.editor).id();
    assert_ne!(output_doc_id, source_doc_id);

    app.editor.switch(source_doc_id, Action::Replace);
    wait_for_condition(&mut app, "source buffer focused", |app| {
        let doc = doc!(app.editor);
        doc.id() == source_doc_id && doc.path().is_some_and(|path| path == &source_path)
    })
    .await?;

    {
        let view_id = current_ref!(app.editor).0.id;
        let doc = app.editor.document_mut(source_doc_id).unwrap();
        let end = doc.text().len_chars();
        doc.set_selection(view_id, helix_core::Selection::point(end));
        let (view, doc) = current_ref!(app.editor);
        assert!(doc.selection(view.id).primary().is_empty());
    }

    send_key_sequence(&mut app, ":insert-stream-output echo two<ret>").await?;
    wait_for_condition(&mut app, "reused stream output buffer", |app| {
        let doc = doc!(app.editor);
        let text = doc.text().to_string();
        doc.id() == output_doc_id
            && text.contains("echo one\none")
            && text.contains("echo two\ntwo")
            && text.find("echo one") < text.find("echo two")
    })
    .await?;

    {
        let (view, doc) = current_ref!(app.editor);
        let text = doc.text().to_string();
        assert_eq!(doc.id(), output_doc_id);
        assert!(text.contains("echo one\none"));
        assert!(text.contains("echo two\ntwo"));
        assert!(text.find("echo one") < text.find("echo two"));
        assert_eq!(
            doc.selection(view.id)
                .primary()
                .cursor(doc.text().slice(..)),
            doc.text().len_chars()
        );
    }

    assert_eq!(
        app.editor
            .document(source_doc_id)
            .unwrap()
            .text()
            .to_string(),
        "source\n"
    );

    close_app(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_insert_stream_output_reuses_last_stream_buffer_from_scratch() -> anyhow::Result<()> {
    let _lock = Arc::clone(&STREAM_TEST_LOCK).lock_owned().await;
    let mut app = AppBuilder::new().build()?;

    send_key_sequence(&mut app, ":new<ret>").await?;
    wait_for_condition(&mut app, "first scratch buffer", |app| {
        doc!(app.editor).display_name().as_ref() == "[scratch]"
    })
    .await?;

    send_key_sequence(&mut app, ":insert-stream-output echo one<ret>").await?;
    wait_for_condition(&mut app, "first stream output", |app| {
        let text = doc!(app.editor).text().to_string();
        text.contains("echo one\none")
    })
    .await?;

    let output_doc_id = doc!(app.editor).id();

    send_key_sequence(&mut app, ":new<ret>").await?;
    send_key_sequence(&mut app, "itest<esc>").await?;
    wait_for_condition(&mut app, "second scratch buffer", |app| {
        let doc = doc!(app.editor);
        doc.id() != output_doc_id && doc.text().to_string() == "test\n"
    })
    .await?;

    let other_scratch_id = doc!(app.editor).id();

    send_key_sequence(&mut app, ":insert-stream-output echo two<ret>").await?;
    wait_for_condition(
        &mut app,
        "output appended in original scratch buffer",
        |app| {
            let doc = doc!(app.editor);
            let text = doc.text().to_string();
            doc.id() == output_doc_id
                && text.contains("echo one\none")
                && text.contains("echo two\ntwo")
                && text.find("echo one") < text.find("echo two")
        },
    )
    .await?;

    assert_eq!(
        app.editor
            .document(other_scratch_id)
            .unwrap()
            .text()
            .to_string(),
        "test\n"
    );

    close_app(&mut app).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_undo_redo() -> anyhow::Result<()> {
    // A jumplist selection is created at a point which is undone.
    //
    // * 2[<space>   Add two newlines at line start. We're now on line 3.
    // * <C-s>       Save the selection on line 3 in the jumplist.
    // * u           Undo the two newlines. We're now on line 1.
    // * <C-o><C-i>  Jump forward an back again in the jumplist. This would panic
    //               if the jumplist were not being updated correctly.
    test((
        "#[|]#",
        "2[<space><C-s>u<C-o><C-i>",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    // A jumplist selection is passed through an edit and then an undo and then a redo.
    //
    // * [<space>    Add a newline at line start. We're now on line 2.
    // * <C-s>       Save the selection on line 2 in the jumplist.
    // * kd          Delete line 1. The jumplist selection should be adjusted to the new line 1.
    // * uU          Undo and redo the `kd` edit.
    // * <C-o>       Jump back in the jumplist. This would panic if the jumplist were not being
    //               updated correctly.
    // * <C-i>       Jump forward to line 1.
    test((
        "#[|]#",
        "[<space><C-s>kduU<C-o><C-i>",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    // In this case we 'redo' manually to ensure that the transactions are composing correctly.
    test((
        "#[|]#",
        "[<space>u[<space>u",
        "#[|]#",
        LineFeedHandling::AsIs,
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_extend_line() -> anyhow::Result<()> {
    // extend with line selected then count
    test((
        indoc! {"\
            #[l|]#orem
            ipsum
            dolor
            
            "},
        "x2x",
        indoc! {"\
            #[lorem
            ipsum
            dolor\n|]#
            
            "},
    ))
    .await?;

    // extend with count on partial selection
    test((
        indoc! {"\
            #[l|]#orem
            ipsum
            
            "},
        "2x",
        indoc! {"\
            #[lorem
            ipsum\n|]#
            
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_character_info() -> anyhow::Result<()> {
    // UTF-8, single byte
    test_key_sequence(
        &mut helpers::AppBuilder::new().build()?,
        Some("ih<esc>h:char<ret>"),
        Some(&|app| {
            assert_eq!(
                r#""h" (U+0068) Dec 104 Hex 68"#,
                app.editor.get_status().unwrap().0
            );
        }),
        false,
    )
    .await?;

    // UTF-8, multi-byte
    test_key_sequence(
        &mut helpers::AppBuilder::new().build()?,
        Some("ië<esc>h:char<ret>"),
        Some(&|app| {
            assert_eq!(
                r#""ë" (U+0065 U+0308) Hex 65 + cc 88"#,
                app.editor.get_status().unwrap().0
            );
        }),
        false,
    )
    .await?;

    // Multiple characters displayed as one, escaped characters
    test_key_sequence(
        &mut helpers::AppBuilder::new().build()?,
        Some(":line<minus>ending crlf<ret>:char<ret>"),
        Some(&|app| {
            assert_eq!(
                r#""\r\n" (U+000d U+000a) Hex 0d + 0a"#,
                app.editor.get_status().unwrap().0
            );
        }),
        false,
    )
    .await?;

    // Non-UTF-8
    test_key_sequence(
        &mut helpers::AppBuilder::new().build()?,
        Some(":encoding ascii<ret>ih<esc>h:char<ret>"),
        Some(&|app| {
            assert_eq!(r#""h" Dec 104 Hex 68"#, app.editor.get_status().unwrap().0);
        }),
        false,
    )
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_delete_char_backward() -> anyhow::Result<()> {
    // don't panic when deleting overlapping ranges
    test(("#(x|)# #[x|]#", "c<space><backspace><esc>", "#[\n|]#")).await?;
    test((
        "#( |)##( |)#a#( |)#axx#[x|]#a",
        "li<backspace><esc>",
        "#(a|)##(|a)#xx#[|a]#",
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_delete_word_backward() -> anyhow::Result<()> {
    // don't panic when deleting overlapping ranges
    test(("fo#[o|]#ba#(r|)#", "a<C-w><esc>", "#[\n|]#")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_delete_word_forward() -> anyhow::Result<()> {
    // don't panic when deleting overlapping ranges
    test(("fo#[o|]#b#(|ar)#", "i<A-d><esc>", "fo#[\n|]#")).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_delete_char_forward() -> anyhow::Result<()> {
    test((
        indoc! {"\
                #[abc|]#def
                #(abc|)#ef
                #(abc|)#f
                #(abc|)#
            "},
        "a<del><esc>",
        indoc! {"\
                #[abc|]#ef
                #(abc|)#f
                #(abc|)#
                #(abc|)#
            "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_insert_with_indent() -> anyhow::Result<()> {
    const INPUT: &str = indoc! { "
        #[f|]#n foo() {
            if let Some(_) = None {

            }
         
        }

        fn bar() {

        }
        "
    };

    // insert_at_line_start
    test((
        INPUT,
        ":lang rust<ret>%<A-s>I",
        indoc! { "
            #[f|]#n foo() {
                #(i|)#f let Some(_) = None {
                    #(\n|)#
                #(}|)#
            #( |)#
            #(}|)#
            #(\n|)#
            #(f|)#n bar() {
                #(\n|)#
            #(}|)#
            "
        },
    ))
    .await?;

    // insert_at_line_end
    test((
        INPUT,
        ":lang rust<ret>%<A-s>A",
        indoc! { "
            fn foo() {#[\n|]#
                if let Some(_) = None {#(\n|)#
                    #(\n|)#
                }#(\n|)#
             #(\n|)#
            }#(\n|)#
            #(\n|)#
            fn bar() {#(\n|)#
                #(\n|)#
            }#(\n|)#
            "
        },
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_join_selections() -> anyhow::Result<()> {
    // normal join
    test((
        indoc! {"\
            #[a|]#bc
            def
        "},
        "J",
        indoc! {"\
            #[a|]#bc def
        "},
    ))
    .await?;

    // join with empty line
    test((
        indoc! {"\
            #[a|]#bc

            def
        "},
        "JJ",
        indoc! {"\
            #[a|]#bc def
        "},
    ))
    .await?;

    // join with additional space in non-empty line
    test((
        indoc! {"\
            #[a|]#bc

                def
        "},
        "JJ",
        indoc! {"\
            #[a|]#bc def
        "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_join_selections_space() -> anyhow::Result<()> {
    // join with empty lines panic
    test((
        indoc! {"\
            #[a

            b

            c

            d

            e|]#
        "},
        "<A-J>",
        indoc! {"\
            a#[ |]#b#( |)#c#( |)#d#( |)#e
        "},
    ))
    .await?;

    // normal join
    test((
        indoc! {"\
            #[a|]#bc
            def
        "},
        "<A-J>",
        indoc! {"\
            abc#[ |]#def
        "},
    ))
    .await?;

    // join with empty line
    test((
        indoc! {"\
            #[a|]#bc

            def
        "},
        "<A-J>",
        indoc! {"\
            #[a|]#bc
            def
        "},
    ))
    .await?;

    // join with additional space in non-empty line
    test((
        indoc! {"\
            #[a|]#bc

                def
        "},
        "<A-J><A-J>",
        indoc! {"\
            abc#[ |]#def
        "},
    ))
    .await?;

    // join with retained trailing spaces
    test((
        indoc! {"\
            #[aaa   

            bb  

            c |]#
        "},
        "<A-J>",
        indoc! {"\
            aaa   #[ |]#bb  #( |)#c 
        "},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_join_selections_comment() -> anyhow::Result<()> {
    test((
        indoc! {"\
            /// #[a|]#bc
            /// def
        "},
        ":lang rust<ret>J",
        indoc! {"\
            /// #[a|]#bc def
        "},
    ))
    .await?;

    // Only join if the comment token matches the previous line.
    test((
        indoc! {"\
            #[| // a
            // b
            /// c
            /// d
            e
            /// f
            // g]#
        "},
        ":lang rust<ret>J",
        indoc! {"\
            #[| // a b /// c d e f // g]#
        "},
    ))
    .await?;

    test((
        "#[|\t// Join comments
\t// with indent]#",
        ":lang go<ret>J",
        "#[|\t// Join comments with indent]#",
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_read_file() -> anyhow::Result<()> {
    let mut file = tempfile::NamedTempFile::new()?;
    let contents_to_read = "some contents";
    let output_file = helpers::temp_file_with_contents(contents_to_read)?;

    test_key_sequence(
        &mut helpers::AppBuilder::new()
            .with_file(file.path(), None)
            .build()?,
        Some(&format!(":r {:?}<ret><esc>:w<ret>", output_file.path())),
        Some(&|app| {
            assert!(!app.editor.is_err(), "error: {:?}", app.editor.get_status());
        }),
        false,
    )
    .await?;

    let expected_contents = LineFeedHandling::Native.apply(contents_to_read);
    helpers::assert_file_has_content(&mut file, &expected_contents)?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_delete() -> anyhow::Result<()> {
    // Test `surround_delete` when head < anchor
    test(("(#[|  ]#)", "mdm", "#[|  ]#")).await?;
    test(("(#[|  ]#)", "md(", "#[|  ]#")).await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn surround_replace_ts() -> anyhow::Result<()> {
    const INPUT: &str = r#"\
fn foo() {
    if let Some(_) = None {
        testing!("f#[|o]#o)");
    }
}
"#;
    test((
        INPUT,
        ":lang rust<ret>mrm'",
        r#"\
fn foo() {
    if let Some(_) = None {
        testing!('f#[|o]#o)');
    }
}
"#,
    ))
    .await?;

    test((
        INPUT,
        ":lang rust<ret>3mrm[",
        r#"\
fn foo() {
    if let Some(_) = None [
        testing!("f#[|o]#o)");
    ]
}
"#,
    ))
    .await?;

    test((
        INPUT,
        ":lang rust<ret>2mrm{",
        r#"\
fn foo() {
    if let Some(_) = None {
        testing!{"f#[|o]#o)"};
    }
}
"#,
    ))
    .await?;

    test((
        indoc! {"\
            #[a
            b
            c
            d
            e|]#
            f
            "},
        "s\\n<ret>r,",
        "a#[,|]#b#(,|)#c#(,|)#d#(,|)#e\nf\n",
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn macro_play_within_macro_record() -> anyhow::Result<()> {
    // <https://github.com/helix-editor/helix/issues/12697>
    //
    // * `"aQihello<esc>Q` record a macro to register 'a' which inserts "hello"
    // * `Q"aq<space>world<esc>Q` record a macro to the default macro register which plays the
    //   macro in register 'a' and then inserts " world"
    // * `%d` clear the buffer
    // * `q` replay the macro in the default macro register
    // * `i<ret>` add a newline at the end
    //
    // The inner macro in register 'a' should replay within the outer macro exactly once to insert
    // "hello world".
    test((
        indoc! {"\
            #[|]#
        "},
        r#""aQihello<esc>QQ"aqi<space>world<esc>Q%dqi<ret>"#,
        indoc! {"\
            hello world
            #[|]#"},
    ))
    .await?;

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn global_search_with_multibyte_chars() -> anyhow::Result<()> {
    // Assert that `helix_term::commands::global_search` handles multibyte characters correctly.
    test((
        indoc! {"\
            // Hello world!
            // #[|
            ]#
            "},
        // start global search
        " /«十分に長い マルチバイトキャラクター列» で検索<ret><esc>",
        indoc! {"\
            // Hello world!
            // #[|
            ]#
            "},
    ))
    .await?;

    Ok(())
}
