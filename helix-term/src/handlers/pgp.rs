//! Passphrase-encrypted (`gpg -c`) files: asks for the passphrase as one
//! opens, and closes its markdown preview when it closes.
//!
//! One passphrase prompt shows at a time. When it is done, the files still
//! locked are tried with the passphrases remembered so far before the next
//! prompt, so files sharing a passphrase ask for it once.

use helix_event::register_hook;
use helix_view::events::{DocumentDidClose, DocumentDidOpen};
use helix_view::handlers::Handlers;
use helix_view::pgp::Passphrase;
use helix_view::{DocumentId, Editor};

use crate::compositor::{Compositor, Context};
use crate::job::{self, Callback};
use crate::ui::{MarkdownPreview, Prompt, PromptEvent};

pub(super) fn register_hooks(_handlers: &Handlers) {
    register_hook!(move |event: &mut DocumentDidOpen<'_>| {
        let doc_id = event.doc;
        if event
            .editor
            .documents
            .get(&doc_id)
            .is_some_and(|doc| doc.is_pgp_locked())
        {
            job::dispatch_blocking(move |editor, compositor| unlock(editor, compositor, doc_id));
        }
        Ok(())
    });

    register_hook!(move |event: &mut DocumentDidClose<'_>| {
        if event.doc.is_encrypted() {
            let doc_id = event.doc.id();
            job::dispatch_blocking(move |_, compositor| {
                if compositor
                    .find_id::<MarkdownPreview>(MarkdownPreview::ID)
                    .is_some_and(|preview| preview.source_doc() == doc_id)
                {
                    compositor.remove(MarkdownPreview::ID);
                }
            });
        }
        Ok(())
    });
}

/// Runs `f` once the prompt handling the current event has closed.
fn later(cx: &mut Context, f: impl FnOnce(&mut Editor, &mut Compositor) + Send + 'static) {
    cx.jobs
        .callback(async move { Ok(Callback::EditorCompositor(Box::new(f))) });
}

/// After a passphrase prompt is done: unlocks the files still locked, or
/// asks for the next one.
fn next(cx: &mut Context) {
    later(cx, |editor, compositor| {
        let locked: Vec<DocumentId> = editor
            .documents
            .values()
            .filter(|doc| doc.is_pgp_locked())
            .map(|doc| doc.id())
            .collect();
        for doc_id in locked {
            unlock(editor, compositor, doc_id);
        }
    });
}

/// Opens a locked document with a passphrase remembered this session, or
/// asks for one.
fn unlock(editor: &mut Editor, compositor: &mut Compositor, doc_id: DocumentId) {
    // The open prompt takes this file in turn when it is done.
    if compositor.find_id::<Prompt>(Prompt::SECRET_ID).is_some() {
        return;
    }
    let Some(doc) = editor.documents.get_mut(&doc_id) else {
        return;
    };
    let Some(state) = doc.pgp.as_ref().filter(|state| state.is_locked()) else {
        return;
    };
    let name = doc.display_name().into_owned();
    if state.is_new_file() {
        ask_new(compositor, doc_id, name, None, None);
        return;
    }
    // A wrong passphrase costs one key derivation, a fraction of a second.
    let unlocked = editor
        .pgp_passphrases
        .iter()
        .any(|passphrase| doc.pgp_unlock(passphrase.clone()).is_ok());
    if unlocked {
        editor.set_status(format!("Decrypted {name}"));
    } else {
        ask(compositor, doc_id, name, false);
    }
}

/// Asks for the passphrase of an existing file, again after a `wrong` one.
/// Esc closes it.
///
/// The reason for asking again is in the prompt: it covers the status line.
fn ask(compositor: &mut Compositor, doc_id: DocumentId, name: String, wrong: bool) {
    let label = if wrong {
        format!("Wrong passphrase. Passphrase for {name}: ")
    } else {
        format!("Passphrase for {name}: ")
    };
    let prompt = Prompt::new(
        label.into(),
        None,
        |_, _| Vec::new(),
        move |cx, input, event| match event {
            PromptEvent::Update => {}
            PromptEvent::Abort => {
                close(cx.editor, doc_id);
                next(cx);
            }
            PromptEvent::Validate => {
                let Some(doc) = cx
                    .editor
                    .documents
                    .get_mut(&doc_id)
                    .filter(|doc| doc.is_pgp_locked())
                else {
                    next(cx);
                    return;
                };
                let passphrase = Passphrase::new(input);
                match doc.pgp_unlock(passphrase.clone()) {
                    Ok(()) => {
                        cx.editor.pgp_passphrases.remember(passphrase);
                        cx.editor.set_status(format!("Decrypted {name}"));
                        next(cx);
                    }
                    Err(_) => {
                        let name = name.clone();
                        later(cx, move |_, compositor| ask(compositor, doc_id, name, true));
                    }
                }
            }
        },
    )
    .with_secret();
    compositor.push(Box::new(prompt));
}

/// Asks twice for the passphrase of a new file, starting over after an
/// `error`. Esc closes it.
fn ask_new(
    compositor: &mut Compositor,
    doc_id: DocumentId,
    name: String,
    first: Option<Passphrase>,
    error: Option<&'static str>,
) {
    let label = match (&first, error) {
        (Some(_), _) => "Repeat passphrase: ".to_string(),
        (None, Some(error)) => format!("{error} New passphrase for {name}: "),
        (None, None) => format!("New passphrase for {name}: "),
    };
    let prompt = Prompt::new(
        label.into(),
        None,
        |_, _| Vec::new(),
        move |cx, input, event| match event {
            PromptEvent::Update => {}
            PromptEvent::Abort => {
                close(cx.editor, doc_id);
                next(cx);
            }
            PromptEvent::Validate => {
                let name = name.clone();
                if input.is_empty() {
                    later(cx, move |_, compositor| {
                        ask_new(compositor, doc_id, name, None, Some("It can't be empty."))
                    });
                    return;
                }
                let passphrase = Passphrase::new(input);
                match &first {
                    None => later(cx, move |_, compositor| {
                        ask_new(compositor, doc_id, name, Some(passphrase), None)
                    }),
                    Some(first) if *first == passphrase => {
                        if let Some(doc) = cx.editor.documents.get_mut(&doc_id) {
                            if doc.pgp_unlock(passphrase.clone()).is_ok() {
                                cx.editor.pgp_passphrases.remember(passphrase);
                                cx.editor
                                    .set_status(format!("{name} will be saved encrypted"));
                            }
                        }
                        next(cx);
                    }
                    Some(_) => {
                        later(cx, move |_, compositor| {
                            ask_new(compositor, doc_id, name, None, Some("They didn't match."))
                        });
                    }
                }
            }
        },
    )
    .with_secret();
    compositor.push(Box::new(prompt));
}

/// Closes a document whose passphrase prompt was dismissed. It is still
/// locked and empty, so there is nothing to keep.
fn close(editor: &mut Editor, doc_id: DocumentId) {
    if editor
        .documents
        .get(&doc_id)
        .is_some_and(|doc| doc.is_pgp_locked())
    {
        let _ = editor.close_document(doc_id, true);
    }
}
