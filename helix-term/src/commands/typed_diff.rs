use super::*;

use helix_core::command_line::Args;

pub(crate) fn diff_base(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let scrolloff = cx.editor.config().scrolloff;
    let (view, doc) = current!(cx.editor);

    let commit_ref = if args.is_empty() {
        None
    } else {
        Some(args.get(0).unwrap().to_string())
    };

    let result = match commit_ref.clone() {
        Some(ref_name) => doc.set_diff_base_from_ref(ref_name),
        None => doc.reset_diff_base(),
    };
    if let Err(e) = result {
        cx.editor.set_error(format!("failed to set diff base: {e}"));
        return Ok(());
    }

    doc.reload(view, &cx.editor.diff_providers).map(|_| {
        view.ensure_cursor_in_view(doc, scrolloff);
    })?;

    let msg = if let Some(ref commit) = commit_ref {
        format!("Diff base set to: {}", commit)
    } else {
        "Diff base reset to HEAD".to_string()
    };
    cx.editor.set_status(msg);

    Ok(())
}

pub(crate) fn toggle_char_diff(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let (_view, doc) = current!(cx.editor);
    doc.char_diff_enabled = !doc.char_diff_enabled;

    let status = if doc.char_diff_enabled {
        "Character-level diff highlighting enabled"
    } else {
        "Character-level diff highlighting disabled"
    };

    cx.editor.set_status(status);
    cx.editor.clear_idle_timer();

    Ok(())
}

#[derive(Clone)]
struct DiffBufferMeta {
    id: DocumentId,
    name: String,
    is_modified: bool,
    focused_at: std::time::Instant,
}

fn diff_buffer_name(doc: &Document) -> String {
    doc.relative_path()
        .and_then(|path| path.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| doc.display_name().into_owned())
}

fn resolve_diff_buffer_arg(
    editor: &Editor,
    arg: &str,
    working_doc_id: DocumentId,
) -> anyhow::Result<DocumentId> {
    let mut matches = editor
        .documents
        .values()
        .filter(|doc| doc.id() != working_doc_id)
        .filter(|doc| {
            doc.id().to_string() == arg
                || doc
                    .relative_path()
                    .and_then(|path| path.to_str())
                    .is_some_and(|path| path == arg)
                || doc
                    .path()
                    .and_then(|path| path.to_str())
                    .is_some_and(|path| path == arg)
                || doc.display_name().as_ref() == arg
        })
        .map(Document::id);

    let Some(first) = matches.next() else {
        anyhow::bail!("No matching buffer: {arg}");
    };
    if matches.next().is_some() {
        anyhow::bail!("Ambiguous buffer '{arg}', use a buffer id");
    }
    Ok(first)
}

fn open_diff_buffer_picker(
    cx: &mut compositor::Context,
    working_doc_id: DocumentId,
) -> anyhow::Result<()> {
    let mut items = cx
        .editor
        .documents
        .values()
        .filter(|doc| doc.id() != working_doc_id)
        .map(|doc| DiffBufferMeta {
            id: doc.id(),
            name: diff_buffer_name(doc),
            is_modified: doc.is_modified(),
            focused_at: doc.focused_at,
        })
        .collect::<Vec<_>>();

    if items.is_empty() {
        cx.editor.set_error("No other buffer to diff");
        return Ok(());
    }

    items.sort_unstable_by_key(|item| std::cmp::Reverse(item.focused_at));

    let callback = async move {
        let call: job::Callback = job::Callback::EditorCompositor(Box::new(
            move |_editor: &mut Editor, compositor: &mut Compositor| {
                let columns = [
                    ui::PickerColumn::new("id", |meta: &DiffBufferMeta, _| {
                        meta.id.to_string().into()
                    }),
                    ui::PickerColumn::new("flags", |meta: &DiffBufferMeta, _| {
                        if meta.is_modified { "+" } else { "" }.into()
                    }),
                    ui::PickerColumn::new("buffer", |meta: &DiffBufferMeta, _| {
                        meta.name.as_str().into()
                    }),
                ];

                let picker = ui::Picker::new(columns, 2, items, (), move |cx, meta, _action| {
                    if let Err(e) =
                        cx.editor
                            .open_buffer_diff_view(meta.id, working_doc_id, Some(true))
                    {
                        cx.editor
                            .set_error(format!("Failed to open buffer diff: {e}"));
                    } else {
                        cx.editor.set_status(format!(
                            "Diffing buffer {} against current buffer",
                            meta.id
                        ));
                    }
                })
                .with_preview(|_editor, meta| Some((meta.id.into(), None)));

                compositor.push(Box::new(overlaid(picker)));
            },
        ));
        Ok(call)
    };

    cx.jobs.callback(callback);
    Ok(())
}

pub(crate) fn diff_buffer(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let working_doc_id = view!(cx.editor).doc;
    let base_doc_id = if let Some(arg) = args.first() {
        resolve_diff_buffer_arg(cx.editor, arg, working_doc_id)?
    } else {
        return open_diff_buffer_picker(cx, working_doc_id);
    };

    cx.editor
        .open_buffer_diff_view(base_doc_id, working_doc_id, Some(true))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    cx.editor.set_status(format!(
        "Diffing buffer {base_doc_id} against current buffer"
    ));

    Ok(())
}

pub(crate) fn diff_commit(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let range_str = args.first().context("missing git reference argument")?;

    let diff_range = DiffRange::parse(range_str)
        .with_context(|| format!("failed to parse diff range: {}", range_str))?;
    let display = diff_range.display();
    cx.editor.diff.range = Some(diff_range);
    cx.editor.set_status(format!(
        "Diff range set to: {} — use space+g to browse files",
        display
    ));

    Ok(())
}

pub(crate) fn open_merge_view(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }
    let path = doc!(cx.editor)
        .path()
        .map(|p| p.to_path_buf())
        .context("Current buffer has no file path")?;
    cx.editor
        .open_merge_view(&path)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

pub(crate) fn diff_reset_typed(
    cx: &mut compositor::Context,
    _args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    if event != PromptEvent::Validate {
        return Ok(());
    }

    let view_id = view!(cx.editor).id;
    if cx.editor.diff.views.contains_key(&view_id) {
        cx.editor.close_diff_view(view_id);
    }
    cx.editor.diff.range = None;

    let diff_docs: Vec<_> = cx
        .editor
        .documents
        .iter()
        .filter_map(|(id, doc)| doc.path().map(|_| *id))
        .collect();

    for (_, doc) in cx.editor.documents.iter_mut() {
        doc.char_diff_enabled = false;
        doc.char_diff_minus_side = false;
        doc.linked_diff_doc = None;
        if doc.path().is_none() {
            doc.clear_diff_handle();
        }
    }
    for id in diff_docs {
        if let Some(doc) = cx.editor.documents.get_mut(&id) {
            let _ = doc.reset_diff_base();
        }
    }

    cx.editor.set_status("Diff reset to HEAD");

    Ok(())
}

pub(crate) fn diff_files(
    cx: &mut compositor::Context,
    args: Args,
    event: PromptEvent,
) -> anyhow::Result<()> {
    #[cfg(not(feature = "git"))]
    {
        let _ = (cx, args, event);
        anyhow::bail!("git support not compiled in");
    }

    #[cfg(feature = "git")]
    {
        if event != PromptEvent::Validate {
            return Ok(());
        }

        let base_ref = args
            .first()
            .context("missing git reference argument")?
            .to_string();
        // `:diff-files <ref>` browses the changes between <ref> and the working
        // tree (target = None), i.e. the same listing as `space g` with that
        // base. This shares `changed_file_picker`'s implementation so both get
        // the instant cache + background refresh.
        let target_ref: Option<String> = None;

        let cwd = helix_stdx::env::current_working_dir();
        if !cwd.exists() {
            cx.editor
                .set_error("Current working directory does not exist");
            return Ok(());
        }

        cx.editor.diff.changed_file_request = cx.editor.diff.changed_file_request.wrapping_add(1);
        #[cfg(not(feature = "integration"))]
        let request = cx.editor.diff.changed_file_request;

        let seed = diff::changed_file_seed(cx.editor, &cwd, &base_ref, target_ref.as_deref());

        // The picker isn't `Send`, and a typed command has no direct compositor,
        // so build and push it on the main thread via a compositor callback.
        // The background refresh is spawned from the same callback, after the
        // picker is on the compositor stack, so even an instant scan can't
        // finish before the picker it reconciles exists.
        cx.jobs.callback(async move {
            Ok(job::Callback::EditorCompositor(Box::new(
                move |editor: &mut Editor, compositor: &mut Compositor| {
                    let (component, injector) = diff::changed_file_picker_component(
                        editor,
                        cwd.clone(),
                        base_ref.clone(),
                        target_ref.clone(),
                        seed.clone(),
                    );
                    compositor.push(component);

                    #[cfg(not(feature = "integration"))]
                    diff::spawn_changed_file_refresh(
                        cwd,
                        base_ref,
                        target_ref,
                        seed.clone(),
                        request,
                        // Stream into the empty picker on a cache miss;
                        // reconcile the visible cached listing on a hit.
                        seed.is_empty().then_some(injector),
                    );
                    #[cfg(feature = "integration")]
                    let _ = injector;
                },
            )))
        });

        Ok(())
    }
}
