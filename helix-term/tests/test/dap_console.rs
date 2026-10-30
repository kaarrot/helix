use super::*;

use std::time::Duration;

use helix_term::application::Application;
use helix_term::config::{Config, ConfigLoadError};
use helix_view::{current_ref, document::Mode, input::parse_macro, Editor};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;

mod adapter;

#[cfg(windows)]
use crossterm::event::{Event, KeyEvent};
#[cfg(not(windows))]
use termina::event::{Event, KeyEvent};

/// Drives one editor session through several rounds of keys, with the editor
/// open to inspection -- and to changes -- in between. `test_key_sequences` quits
/// the editor after every call, and its checks only get to look.
struct Session {
    app: Application,
    tx: UnboundedSender<std::io::Result<Event>>,
    rx: UnboundedReceiverStream<std::io::Result<Event>>,
}

impl Session {
    fn new() -> anyhow::Result<(Self, tempfile::NamedTempFile)> {
        Self::with_config(helpers::test_config())
    }

    /// A session whose keymap has `keys`, a `config.toml` fragment, over the defaults.
    fn with_keys(keys: &str) -> anyhow::Result<(Self, tempfile::NamedTempFile)> {
        let loaded = Config::load(Ok(keys.to_string()), Err(ConfigLoadError::default()))
            .map_err(|err| anyhow::anyhow!("{err}"))?;
        Self::with_config(Config {
            keys: loaded.keys,
            ..helpers::test_config()
        })
    }

    fn with_config(config: Config) -> anyhow::Result<(Self, tempfile::NamedTempFile)> {
        let file = tempfile::NamedTempFile::new()?;
        let app = helpers::AppBuilder::new()
            .with_config(config)
            .with_file(file.path(), None)
            .build()?;
        let (tx, rx) = unbounded_channel();
        let rx = UnboundedReceiverStream::new(rx);
        Ok((Self { app, tx, rx }, file))
    }

    async fn keys(&mut self, keys: &str) -> anyhow::Result<&Editor> {
        for key in parse_macro(keys)? {
            self.tx.send(Ok(Event::Key(KeyEvent::from(key))))?;
        }
        self.app.event_loop_until_idle(&mut self.rx).await;
        Ok(&self.app.editor)
    }

    fn editor(&mut self) -> &mut Editor {
        &mut self.app.editor
    }

    async fn paste(&mut self, text: &str) -> anyhow::Result<&Editor> {
        self.tx.send(Ok(Event::Paste(text.to_string())))?;
        self.app.event_loop_until_idle(&mut self.rx).await;
        Ok(&self.app.editor)
    }

    fn attach_adapter(&mut self, gates: &std::path::Path) -> anyhow::Result<()> {
        let path = helix_view::doc!(self.app.editor).path().cloned().unwrap();
        let editor = &mut self.app.editor;
        let id = editor
            .debug_adapters
            .start_client(None, &adapter::config(gates)?)?;
        let debugger = editor.debug_adapters.get_client_mut(id).unwrap();
        let thread = serde_json::from_value(serde_json::json!(1))?;
        debugger.thread_id = Some(thread);
        debugger.active_frame = Some(0);
        debugger.stack_frames.insert(
            thread,
            vec![
                serde_json::from_value(serde_json::json!({
                    "id": 7, "name": "demo", "source": {"path": path}, "line": 1, "column": 1,
                }))?,
                serde_json::from_value(serde_json::json!({
                    "id": 8, "name": "caller", "source": {"path": path}, "line": 1, "column": 1,
                }))?,
            ],
        );
        editor.debug_adapters.set_active_client(id);
        Ok(())
    }

    async fn finish_requests(&mut self) -> anyhow::Result<()> {
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.app.editor.dap_console_busy().is_some() {
                self.app.editor.reset_idle_timer();
                self.keys("").await?;
            }
            anyhow::Ok(())
        })
        .await?
    }

    /// Leaves the debug menu and quits.
    async fn quit(mut self) -> anyhow::Result<()> {
        for key in parse_macro("<esc><esc><esc>:qa!<ret>")? {
            self.tx.send(Ok(Event::Key(KeyEvent::from(key))))?;
        }
        tokio::time::timeout(
            Duration::from_millis(500),
            self.app.event_loop(&mut self.rx),
        )
        .await?;
        let errs = self.app.close().await;
        anyhow::ensure!(errs.is_empty(), "errors closing the editor: {errs:?}");
        Ok(())
    }
}

fn console_text(editor: &Editor) -> String {
    let doc_id = editor.dap_console_doc().expect("the console exists");
    editor.document(doc_id).unwrap().text().to_string()
}

fn cursor(editor: &Editor) -> usize {
    let (view, doc) = current_ref!(editor);
    doc.selection(view.id)
        .primary()
        .cursor(doc.text().slice(..))
}

const NO_SESSION: &str = "*** No debug session: start one from the debug menu (<space>G l)\n";

#[tokio::test(flavor = "multi_thread")]
async fn console_opens_from_the_debug_menu_and_keeps_it() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;

    let editor = session.keys("<space>G<C-d>").await?;
    assert!(editor.is_dap_console_focused());
    assert_eq!(Mode::Insert, editor.mode());
    assert_eq!(2, editor.tree.views().count());
    assert_eq!(format!("{NO_SESSION}(hx) "), console_text(editor));
    // The debug menu is set aside while typing in the console.
    assert!(editor.autoinfo.is_none());

    // `b`, `c` and `n` would be debug commands in the menu; here they are text.
    let editor = session.keys("bcn").await?;
    assert_eq!(format!("{NO_SESSION}(hx) bcn"), console_text(editor));

    // Output arriving mid-command goes above the input line, and leaves what was
    // typed, and the cursor, where they were.
    session.editor().dap_console_print("stdout line");
    let editor = session.editor();
    assert_eq!(
        format!("{NO_SESSION}stdout line\n(hx) bcn"),
        console_text(editor)
    );
    assert_eq!(console_text(editor).chars().count(), cursor(editor));

    let editor = session.keys("<ret>").await?;
    assert_eq!(
        format!("{NO_SESSION}stdout line\n(hx) bcn\n{NO_SESSION}(hx) "),
        console_text(editor)
    );
    assert_eq!(Mode::Insert, editor.mode());
    let console = editor.dap_console_doc().unwrap();
    assert!(!editor.document(console).unwrap().is_modified());

    // Up recalls the command just run; Down clears the line again.
    let editor = session.keys("<up>").await?;
    assert!(console_text(editor).ends_with("\n(hx) bcn"));
    let editor = session.keys("<down>").await?;
    assert!(console_text(editor).ends_with("\n(hx) "));

    // Esc leaves insert mode within the console; a second Esc returns to the
    // source, back in the debug menu, with the console still on screen.
    let editor = session.keys("<esc><esc>").await?;
    assert!(!editor.is_dap_console_focused());
    assert_eq!(Mode::Normal, editor.mode());
    assert_eq!(2, editor.tree.views().count());
    assert!(editor.autoinfo.is_some());

    // Ctrl-d switches focus without changing the split layout.
    let editor = session.keys("<C-d>").await?;
    assert!(editor.is_dap_console_focused());
    assert_eq!(Mode::Insert, editor.mode());

    let editor = session.keys("<C-d>").await?;
    assert!(!editor.is_dap_console_focused());
    assert_eq!(Mode::Normal, editor.mode());
    assert_eq!(2, editor.tree.views().count());
    assert!(editor.dap_console_doc().is_some());
    assert!(editor.autoinfo.is_some());

    // Shown again, the transcript is where it was left.
    let editor = session.keys("<C-d>").await?;
    assert!(console_text(editor).starts_with(&format!("{NO_SESSION}stdout line\n")));

    let editor = session.keys("<esc>:debug-console-hide<ret>").await?;
    assert!(!editor.is_dap_console_focused());
    assert_eq!(1, editor.tree.views().count());
    assert!(editor.dap_console_doc().is_some());

    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn console_opens_and_closes_from_the_command_line() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;

    let editor = session.keys(":debug-console<ret>").await?;
    assert!(editor.is_dap_console_focused());
    assert_eq!(Mode::Insert, editor.mode());
    assert_eq!(2, editor.tree.views().count());
    assert_eq!(format!("{NO_SESSION}(hx) "), console_text(editor));

    // Opening an open console only focuses it again.
    let editor = session.keys("<esc>gg:debug-console<ret>").await?;
    assert!(editor.is_dap_console_focused());
    assert_eq!(Mode::Insert, editor.mode());
    assert_eq!(2, editor.tree.views().count());
    assert_eq!(console_text(editor).chars().count(), cursor(editor));

    let editor = session.keys("<esc>:debug-console-toggle<ret>").await?;
    assert!(!editor.is_dap_console_focused());
    assert_eq!(Mode::Normal, editor.mode());
    assert_eq!(1, editor.tree.views().count());
    assert!(editor.dap_console_doc().is_some());

    let editor = session.keys(":debug-console-toggle<ret>").await?;
    assert!(editor.is_dap_console_focused());
    assert_eq!(Mode::Insert, editor.mode());
    assert_eq!(format!("{NO_SESSION}(hx) "), console_text(editor));

    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_command_line_works_from_the_debug_menu() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;

    let editor = session.keys("<space>G:debug-console<ret>").await?;
    assert!(editor.is_dap_console_focused());
    assert_eq!(Mode::Insert, editor.mode());
    // The menu is set aside at once, not on the next key.
    assert!(editor.autoinfo.is_none());

    let editor = session.keys("bcn").await?;
    assert_eq!(format!("{NO_SESSION}(hx) bcn"), console_text(editor));

    // Back at the source, the debug menu is still there.
    let editor = session.keys("<esc><esc>").await?;
    assert!(!editor.is_dap_console_focused());
    assert!(editor.autoinfo.is_some());

    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_toggle_bound_to_a_letter_types_it_in_the_console() -> anyhow::Result<()> {
    let (mut session, _file) =
        Session::with_keys("[keys.normal.space.G]\nx = \"dap_console_toggle\"\n")?;

    let editor = session.keys("<space>Gx").await?;
    assert!(editor.is_dap_console_focused());
    assert_eq!(Mode::Insert, editor.mode());

    let editor = session.keys("x * 2").await?;
    assert!(editor.is_dap_console_focused());
    assert_eq!(format!("{NO_SESSION}(hx) x * 2"), console_text(editor));

    // From normal mode the letter closes the console again.
    let editor = session.keys("<esc>x").await?;
    assert!(!editor.is_dap_console_focused());
    assert_eq!(1, editor.tree.views().count());
    assert!(editor.autoinfo.is_some());

    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn typing_in_the_transcript_happens_on_the_input_line() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;

    session.keys("<space>G<C-d>x<esc>").await?;

    // Up in the transcript, starting to type again lands after the prompt rather
    // than in the output.
    let editor = session.keys("ggiy").await?;
    assert_eq!(format!("{NO_SESSION}(hx) xy"), console_text(editor));

    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_transcript_is_never_unsaved_and_never_thrown_away() -> anyhow::Result<()> {
    let (mut session, file) = Session::new()?;

    // Typing into the console and leaving insert mode records the edit, but a
    // transcript has nothing to save, so `:q` is not held up by it.
    let editor = session.keys("<space>G<C-d>half typed<esc>").await?;
    let console = editor.dap_console_doc().unwrap();
    assert!(!editor.document(console).unwrap().is_modified());

    // Opening another file in the console's view replaces it there, but the
    // transcript survives, as an empty scratch buffer would not.
    let other = tempfile::NamedTempFile::new()?;
    let editor = session
        .keys(&format!(":e {}<ret>", other.path().display()))
        .await?;
    assert!(!editor.is_dap_console_focused());
    assert_eq!(Some(console), editor.dap_console_doc());
    assert!(console_text(editor).ends_with("(hx) half typed"));

    // Nothing stops the editor from closing: the console is not an unsaved buffer.
    let editor = session.keys(":q<ret>").await?;
    assert!(
        !editor
            .get_status()
            .is_some_and(|(msg, _)| msg.contains("unsaved")),
        "{:?}",
        editor.get_status()
    );
    drop(file);
    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_debug_menu_opened_inside_the_console_works() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;

    session.keys("<space>G<C-d><esc>").await?;
    // `<space>G` from the console enters the debug menu, and `v` is the menu's
    // variables command -- not select mode, as it would be once the menu was set
    // aside.
    let editor = session.keys("<space>G").await?;
    assert!(editor.autoinfo.is_some(), "the debug menu is up");
    let editor = session.keys("v").await?;
    assert_eq!(Mode::Normal, editor.mode());
    assert!(editor.is_dap_console_focused());

    // Esc leaves the menu, and a second Esc the console.
    let editor = session.keys("<esc>").await?;
    assert!(editor.autoinfo.is_none());
    assert!(editor.is_dap_console_focused());
    let editor = session.keys("<esc>").await?;
    assert!(!editor.is_dap_console_focused());

    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn enter_on_an_empty_line_repeats_only_what_the_console_ran() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;

    // The shared history already holds an entry, from the eval prompt or from an
    // earlier day; an empty line must not run it.
    session
        .editor()
        .registers
        .write('=', vec!["items.pop()".to_string()])?;
    let editor = session.keys("<space>G<C-d><ret>").await?;
    assert_eq!(format!("{NO_SESSION}(hx) "), console_text(editor));

    // Once the console has run something, an empty line runs it again.
    let editor = session.keys("x<ret><ret>").await?;
    assert_eq!(
        format!("{NO_SESSION}(hx) x\n{NO_SESSION}(hx) x\n{NO_SESSION}(hx) "),
        console_text(editor)
    );

    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn pasting_types_on_the_input_line() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;

    // With the cursor up in the transcript, a paste still goes to the input line,
    // and each line break in it runs the line so far.
    session.keys("<space>G<C-d><esc>gg").await?;
    let editor = session.paste("a = 1\nb = 2\nc").await?;
    assert_eq!(
        format!("{NO_SESSION}(hx) a = 1\n{NO_SESSION}(hx) b = 2\n{NO_SESSION}(hx) c"),
        console_text(editor)
    );

    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn output_waits_while_the_completion_menu_is_open() -> anyhow::Result<()> {
    use helix_core::completion::CompletionProvider;
    use helix_view::handlers::completion::ResponseContext;

    let (mut session, _file) = Session::new()?;
    session.keys("<space>G<C-d>obj.").await?;

    // Stand in for the menu Tab opens: while it is up, picking an item rewinds the
    // document to where the menu opened, taking any output added since with it.
    let editor = session.editor();
    let (view, doc) = helix_view::current!(editor);
    let savepoint = doc.savepoint(view);
    editor.handlers.completions.active_completions.insert(
        CompletionProvider::Debugger,
        ResponseContext {
            is_incomplete: false,
            priority: 0,
            savepoint,
        },
    );
    editor.dap_console_print("stdout while picking");
    editor.dap_console_write_unwritten();
    assert_eq!(format!("{NO_SESSION}(hx) obj."), console_text(editor));

    // Once the menu is gone the output is written, in order.
    editor.handlers.completions.active_completions.clear();
    editor.dap_console_print("next");
    assert_eq!(
        format!("{NO_SESSION}stdout while picking\nnext\n(hx) obj."),
        console_text(editor)
    );

    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn program_output_is_written_per_frame_and_unfinished_lines_are_kept() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;
    session.keys("<space>G<C-d>").await?;

    // Whole lines are gathered and written with the next frame.
    let editor = session.editor();
    editor.dap_console_output("stdout", "one\ntw");
    assert_eq!(format!("{NO_SESSION}(hx) "), console_text(editor));
    editor.reset_idle_timer();
    let editor = session.keys("").await?;
    assert_eq!(format!("{NO_SESSION}one\n(hx) "), console_text(editor));

    // A line the program never finished is written when it stops or exits.
    let editor = session.editor();
    editor.dap_console_output("stderr", "boom");
    editor.dap_console_flush_output();
    assert_eq!(
        format!("{NO_SESSION}one\ntw\nstderr: boom\n(hx) "),
        console_text(editor)
    );

    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn breakpoints_can_be_set_from_the_console_before_a_session() -> anyhow::Result<()> {
    let (mut session, file) = Session::new()?;
    std::fs::write(file.path(), "a = 1\nb = 2\n")?;
    let path = helix_stdx::path::canonicalize(file.path());

    session.keys("<space>G<C-d>").await?;
    let editor = session
        .keys(&format!("b {}:2<ret>", file.path().display()))
        .await?;
    assert!(
        console_text(editor).contains(&format!("Breakpoint set at {}:2\n", path.display())),
        "{}",
        console_text(editor)
    );
    assert_eq!(1, editor.breakpoints[&path][0].line);

    let editor = session.keys("b<ret>").await?;
    assert!(console_text(editor).contains(&format!("(hx) b\n{}:2 (unverified)\n", path.display())));

    let editor = session
        .keys(&format!("b {}:2<ret>", file.path().display()))
        .await?;
    assert!(console_text(editor).contains(&format!("Breakpoint cleared at {}:2\n", path.display())));
    assert!(editor.breakpoints[&path].is_empty());

    // Line 0 does not exist, and a frame file needs a session.
    let editor = session.keys("b 0<ret>").await?;
    assert!(console_text(editor).contains("*** The selected frame has no source file"));

    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn gf_shows_a_file_line_from_the_transcript_beside_the_console() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;
    let other = tempfile::NamedTempFile::new()?;
    let text: String = (1..=400).map(|i| format!("    line {i}\n")).collect();
    std::fs::write(other.path(), text)?;
    let other_path = helix_stdx::path::canonicalize(other.path());

    session.keys("<space>G<C-d>").await?;
    let console_view = session.editor().tree.focus;
    let source_view = session
        .editor()
        .tree
        .views()
        .map(|(view, _)| view.id)
        .find(|&id| id != console_view)
        .unwrap();
    // A traceback line and a `w` line, as the console prints them.
    let traceback = format!("  File \"{}\", line 300, in run", other.path().display());
    let frame = format!("  #1  main  {}:20", other.path().display());
    session
        .editor()
        .dap_console_print(&format!("{traceback}\n{frame}"));

    // `gF` on the traceback, two lines up from the input line.
    let editor = session.keys("<esc>kkgF").await?;
    assert_eq!(console_view, editor.tree.focus);
    assert_eq!(Mode::Normal, editor.mode());
    assert_eq!(2, editor.tree.views().count());
    let shows_line = |editor: &Editor, line: usize| {
        let view = editor.tree.get(source_view);
        let doc = editor.document(view.doc).unwrap();
        let text = doc.text().slice(..);
        assert_eq!(Some(&other_path), doc.path());
        let cursor = doc.selection(source_view).primary().cursor(text);
        // On the line's first non-blank, in the middle of the view.
        assert_eq!(text.line_to_char(line) + 4, cursor);
        let top = text.char_to_line(doc.view_offset(source_view).anchor);
        assert!(top < line && line < top + view.inner_height(), "{top}");
        assert_eq!(Some((source_view, view.doc, line)), editor.peek);
        assert_eq!(Some(line), editor.peeked_line(view, doc));
    };
    shows_line(editor, 299);

    // The next one reuses the same split.
    let editor = session.keys("jgF").await?;
    assert_eq!(console_view, editor.tree.focus);
    assert_eq!(2, editor.tree.views().count());
    shows_line(editor, 19);

    // Nothing to show on the input line.
    let editor = session.keys("jgF").await?;
    assert_eq!(2, editor.tree.views().count());
    assert_eq!(console_view, editor.tree.focus);

    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn focus_and_window_navigation_keep_both_panes_open() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;
    session.keys("<space>G<C-d>draft<C-d>").await?;

    // Ordinary window navigation also works while the source's debug menu is up.
    let editor = session.keys("<C-w>j").await?;
    assert!(editor.is_dap_console_focused());
    assert_eq!(Mode::Normal, editor.mode());
    assert!(console_text(editor).ends_with("(hx) draft"));

    let editor = session.keys("<C-d>").await?;
    assert!(!editor.is_dap_console_focused());
    assert_eq!(2, editor.tree.views().count());
    // Leave the sticky menu: the visible console still has the same focus key.
    let editor = session.keys("<esc><C-d>").await?;
    assert!(editor.is_dap_console_focused());
    assert_eq!(Mode::Insert, editor.mode());
    assert!(console_text(editor).ends_with("(hx) draft"));

    // Explicitly hide from the source, retaining the transcript.
    let editor = session.keys("<C-d>:debug-console-hide<ret>").await?;
    assert_eq!(1, editor.tree.views().count());
    assert!(editor.dap_console_doc().is_some());
    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn history_restores_the_draft_and_its_cursor() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;
    session
        .editor()
        .registers
        .write('=', vec!["previous".into()])?;
    session.keys("<space>G<C-d>tax + 🦀  <left><left>").await?;
    let before = cursor(session.editor());
    session.keys("<up><C-d><C-d>").await?;
    let editor = session.keys("<down>").await?;
    assert!(console_text(editor).ends_with("(hx) tax + 🦀  "));
    assert_eq!(before, cursor(editor));
    assert!(editor.dap_console.history_draft.is_none());
    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn console_help_works_without_a_session() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;
    let editor = session.keys(":debug-console<ret>h<ret>").await?;
    assert_eq!("[Debug Console]", helix_view::doc!(editor).display_name());
    assert!(console_text(editor).contains("(hx) h\nDebug Console\n"));
    assert!(console_text(editor).contains(":debug-console-hide"));
    assert!(!console_text(editor).contains("NameError"));
    let editor = session.keys("help<ret>").await?;
    assert!(console_text(editor).contains("(hx) help\nDebug Console\n"));
    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_evaluations_allow_navigation_and_keep_pasted_commands_in_order(
) -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;
    let gates = tempfile::tempdir()?;
    session.attach_adapter(gates.path())?;
    session.keys(":debug-console<ret>").await?;
    session.paste("slow\nfast\ndraft").await?;
    adapter::wait_for(gates.path().join("evaluate-started")).await?;
    assert_eq!(Some("evaluating…"), session.editor().dap_console_busy());
    assert!(!console_text(session.editor()).contains("(hx) fast\n"));

    // The adapter is still waiting. Navigation and hiding must finish now,
    // without waiting for the evaluation to return.
    let editor = tokio::time::timeout(Duration::from_secs(1), session.keys("<C-d>")).await??;
    assert!(!editor.is_dap_console_focused());
    assert_eq!(2, editor.tree.views().count());
    let editor = session.keys(":debug-console-hide<ret>").await?;
    assert_eq!(1, editor.tree.views().count());
    let source = editor.tree.focus;
    let source_cursor = cursor(editor);
    std::fs::write(gates.path().join("release-evaluate"), "")?;
    session.finish_requests().await?;
    assert_eq!(source, session.editor().tree.focus);
    assert_eq!(source_cursor, cursor(session.editor()));
    assert_eq!(Mode::Normal, session.editor().mode());
    assert_eq!(1, session.editor().tree.views().count());
    assert!(console_text(session.editor()).contains("(hx) slow\n11\n(hx) fast\n22\n(hx) draft"));
    assert_eq!(
        "slow:7\nfast:7\n",
        std::fs::read_to_string(gates.path().join("evaluated"))?
    );
    session.keys(":debug-console<ret>").await?;
    assert!(session.editor().is_dap_console_focused());
    assert_eq!(Mode::Insert, session.editor().mode());
    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn queued_evaluations_use_the_frame_selected_when_submitted() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;
    let gates = tempfile::tempdir()?;
    session.attach_adapter(gates.path())?;
    session.keys(":debug-console<ret>slow<ret>").await?;
    adapter::wait_for(gates.path().join("evaluate-started")).await?;

    // Changing the selected frame must affect the next submission, while each
    // pending evaluation retains its own frame even when it starts later.
    session.keys("f 1<ret>fast<ret>f 0<ret>draft").await?;
    std::fs::write(gates.path().join("release-evaluate"), "")?;
    session.finish_requests().await?;
    assert_eq!(
        "slow:7\nfast:8\n",
        std::fs::read_to_string(gates.path().join("evaluated"))?
    );
    assert!(console_text(session.editor()).ends_with("(hx) draft"));
    assert_eq!(
        Some(0),
        session
            .editor()
            .debug_adapters
            .get_active_client()
            .unwrap()
            .active_frame
    );
    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn variable_inspection_allows_navigation_and_guards_execution() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;
    let gates = tempfile::tempdir()?;
    session.attach_adapter(gates.path())?;
    session.keys(":debug-console<ret>?<ret>draft").await?;
    adapter::wait_for(gates.path().join("inspection-started")).await?;
    assert_eq!(Some("inspecting…"), session.editor().dap_console_busy());

    let editor =
        tokio::time::timeout(Duration::from_secs(1), session.keys("<C-d><space>Gn")).await??;
    assert!(!editor.is_dap_console_focused());
    assert!(editor
        .status_msg
        .as_ref()
        .is_some_and(|(message, _)| message.contains("pending inspection")));
    std::fs::write(gates.path().join("release-inspection"), "")?;
    session.finish_requests().await?;
    assert!(!session.editor().is_dap_console_focused());
    assert!(console_text(session.editor()).contains("Locals:\n  answer: int = 42\n(hx) draft"));
    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn late_completion_does_not_overwrite_newer_input() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;
    let gates = tempfile::tempdir()?;
    session.attach_adapter(gates.path())?;
    session.keys(":debug-console<ret>obj.<tab>").await?;
    adapter::wait_for(gates.path().join("completion-started")).await?;
    session.keys("new").await?;
    std::fs::write(gates.path().join("release-completion"), "")?;
    session.finish_requests().await?;
    assert!(console_text(session.editor()).ends_with("(hx) obj.new"));
    session.quit().await
}

async fn discard_completion_after(change: &str) -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;
    let gates = tempfile::tempdir()?;
    session.attach_adapter(gates.path())?;
    session.keys(":debug-console<ret>obj.<tab>").await?;
    adapter::wait_for(gates.path().join("completion-started")).await?;
    match change {
        "cursor" => {
            session.keys("<left>").await?;
        }
        "focus" => {
            session.keys("<C-d>").await?;
        }
        "frame" => {
            session
                .editor()
                .debug_adapters
                .get_active_client_mut()
                .unwrap()
                .active_frame = Some(1);
        }
        _ => unreachable!(),
    }
    let focus = session.editor().tree.focus;
    let before = cursor(session.editor());
    std::fs::write(gates.path().join("release-completion"), "")?;
    session.finish_requests().await?;
    assert!(console_text(session.editor()).ends_with("(hx) obj."));
    assert_eq!(focus, session.editor().tree.focus);
    assert_eq!(before, cursor(session.editor()));
    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn late_completion_is_discarded_after_moving_the_cursor() -> anyhow::Result<()> {
    discard_completion_after("cursor").await
}

#[tokio::test(flavor = "multi_thread")]
async fn late_completion_is_discarded_after_switching_focus() -> anyhow::Result<()> {
    discard_completion_after("focus").await
}

#[tokio::test(flavor = "multi_thread")]
async fn late_completion_is_discarded_after_switching_frames() -> anyhow::Result<()> {
    discard_completion_after("frame").await
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_console_discards_pending_results_and_queued_evaluations() -> anyhow::Result<()>
{
    let (mut session, _file) = Session::new()?;
    let gates = tempfile::tempdir()?;
    session.attach_adapter(gates.path())?;
    session.keys(":debug-console<ret>").await?;
    session.paste("slow\nfast\n").await?;
    adapter::wait_for(gates.path().join("evaluate-started")).await?;
    let console = session.editor().dap_console_doc().unwrap();
    session
        .keys("<esc>:buffer-close!<ret>:debug-console<ret>fresh draft")
        .await?;
    assert_ne!(Some(console), session.editor().dap_console_doc());
    std::fs::write(gates.path().join("release-evaluate"), "")?;
    session.finish_requests().await?;
    assert_eq!("(hx) fresh draft", console_text(session.editor()));
    assert_eq!(
        "slow:7\n",
        std::fs::read_to_string(gates.path().join("evaluated"))?
    );
    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_the_session_discards_pending_results_and_queued_evaluations() -> anyhow::Result<()>
{
    let (mut session, _file) = Session::new()?;
    let gates = tempfile::tempdir()?;
    session.attach_adapter(gates.path())?;
    session.keys(":debug-console<ret>").await?;
    session.paste("slow\nfast\ndraft").await?;
    adapter::wait_for(gates.path().join("evaluate-started")).await?;
    let before = console_text(session.editor());

    let adapter = session
        .editor()
        .debug_adapters
        .get_active_client()
        .unwrap()
        .id();
    // Ending a session can leave its client registered until the adapter sends
    // its termination event. Wait on the queue itself since the busy indicator
    // correctly stops referring to this inactive adapter.
    let (ticket, previous, done) = session
        .editor()
        .dap_console_begin_request(adapter, "waiting…");
    let mut previous = previous.unwrap();
    session.editor().debug_adapters.unset_active_client();
    std::fs::write(gates.path().join("release-evaluate"), "")?;
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            session.editor().reset_idle_timer();
            session.keys("").await?;
            if previous.try_recv().is_ok() {
                break;
            }
        }
        anyhow::Ok(())
    })
    .await??;
    session.editor().reset_idle_timer();
    session.keys("").await?;
    session.editor().dap_console_finish_request(ticket);
    drop(done);

    assert_eq!(before, console_text(session.editor()));
    assert_eq!(
        "slow:7\n",
        std::fs::read_to_string(gates.path().join("evaluated"))?
    );
    assert!(session
        .editor()
        .debug_adapters
        .get_active_client()
        .is_none());
    session.quit().await
}

#[tokio::test(flavor = "multi_thread")]
async fn completion_survives_output_arriving_before_the_response() -> anyhow::Result<()> {
    let (mut session, _file) = Session::new()?;
    let gates = tempfile::tempdir()?;
    session.attach_adapter(gates.path())?;
    session.keys(":debug-console<ret>obj.<tab>").await?;
    adapter::wait_for(gates.path().join("completion-started")).await?;
    session
        .editor()
        .dap_console_print("stdout during completion");
    std::fs::write(gates.path().join("release-completion"), "")?;
    session.finish_requests().await?;
    assert!(console_text(session.editor()).ends_with("stdout during completion\n(hx) obj.member"));
    session.quit().await
}
