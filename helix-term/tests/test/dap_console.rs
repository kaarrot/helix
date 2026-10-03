use super::*;

use std::time::Duration;

use helix_term::application::Application;
use helix_term::config::{Config, ConfigLoadError};
use helix_view::{current_ref, document::Mode, input::parse_macro, Editor};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;

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

    // The same key opens and closes the console. Closing only hides it.
    let editor = session.keys("<C-d>").await?;
    assert!(editor.is_dap_console_focused());
    assert_eq!(Mode::Insert, editor.mode());

    let editor = session.keys("<C-d>").await?;
    assert!(!editor.is_dap_console_focused());
    assert_eq!(Mode::Normal, editor.mode());
    assert_eq!(1, editor.tree.views().count());
    assert!(editor.dap_console_doc().is_some());
    assert!(editor.autoinfo.is_some());

    // Shown again, the transcript is where it was left.
    let editor = session.keys("<C-d>").await?;
    assert!(console_text(editor).starts_with(&format!("{NO_SESSION}stdout line\n")));

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
