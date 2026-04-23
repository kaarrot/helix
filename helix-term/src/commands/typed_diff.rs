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

    cx.editor.diff.range = None;

    let diff_docs: Vec<_> = cx
        .editor
        .documents
        .iter()
        .filter_map(|(id, doc)| doc.path().map(|_| *id))
        .collect();

    for (_, doc) in cx.editor.documents.iter_mut() {
        doc.char_diff_enabled = false;
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
        use helix_vcs::FileChange;
        use helix_view::theme::Style;

        if event != PromptEvent::Validate {
            return Ok(());
        }

        let ref_name = args.first().context("missing git reference argument")?;
        let ref_name_owned = ref_name.to_string();

        let cwd = helix_stdx::env::current_working_dir();
        if !cwd.exists() {
            cx.editor
                .set_error("Current working directory does not exist");
            return Ok(());
        }

        let modified = cx.editor.theme.get("diff.delta");
        let deleted = cx.editor.theme.get("diff.minus");
        let renamed = cx.editor.theme.get("diff.delta.moved");

        let callback = async move {
            use tui::text::Span;

            #[derive(Clone)]
            pub struct DiffFileChangeData {
                cwd: PathBuf,
                style_modified: Style,
                style_deleted: Style,
                style_renamed: Style,
            }

            let call: job::Callback = job::Callback::EditorCompositor(Box::new(
                move |_editor: &mut Editor, compositor: &mut Compositor| {
                    let columns = [
                        ui::PickerColumn::new(
                            "change",
                            |change: &FileChange, data: &DiffFileChangeData| {
                                match change {
                                    FileChange::Modified { .. } => {
                                        Span::styled("~ modified", data.style_modified)
                                    }
                                    FileChange::Deleted { .. } => {
                                        Span::styled("- deleted", data.style_deleted)
                                    }
                                    FileChange::Renamed { .. } => {
                                        Span::styled("> renamed", data.style_renamed)
                                    }
                                    _ => Span::raw("  changed"),
                                }
                                .into()
                            },
                        ),
                        ui::PickerColumn::new(
                            "path",
                            |change: &FileChange, data: &DiffFileChangeData| {
                                let display_path = |path: &PathBuf| {
                                    path.strip_prefix(&data.cwd)
                                        .unwrap_or(path)
                                        .display()
                                        .to_string()
                                };
                                match change {
                                    FileChange::Modified { path } => display_path(path),
                                    FileChange::Deleted { path } => display_path(path),
                                    FileChange::Renamed { from_path, to_path } => {
                                        format!(
                                            "{} -> {}",
                                            display_path(from_path),
                                            display_path(to_path)
                                        )
                                    }
                                    FileChange::Untracked { path } => display_path(path),
                                    FileChange::Conflict { path } => display_path(path),
                                }
                                .into()
                            },
                        ),
                    ];

                    let ref_name_for_callback = ref_name_owned.clone();
                    let picker = ui::Picker::new(
                        columns,
                        1, // path column
                        [],
                        DiffFileChangeData {
                            cwd: cwd.clone(),
                            style_modified: modified,
                            style_deleted: deleted,
                            style_renamed: renamed,
                        },
                        move |cx, meta: &FileChange, _action| {
                            let path = meta.path();
                            if let Err(e) =
                                cx.editor.open_diff_view(path, &ref_name_for_callback, None)
                            {
                                cx.editor
                                    .set_error(format!("Failed to open diff view: {e}"));
                            }
                        },
                    )
                    .with_preview(|_editor, meta| Some((meta.path().into(), None)));

                    let injector = picker.injector();
                    let ref_name_for_thread = ref_name_owned.clone();

                    #[cfg(feature = "integration")]
                    {
                        use helix_vcs::git;
                        let result = git::for_each_changed_file_between_commits(
                            &cwd,
                            &ref_name_for_thread,
                            move |change| match change {
                                Ok(change) => injector.push(change).is_ok(),
                                Err(_err) => true,
                            },
                        );

                        if let Err(err) = result {
                            log::error!("Failed to get diff files: {}", err);
                        }
                    }
                    #[cfg(not(feature = "integration"))]
                    std::thread::spawn(move || {
                        use helix_vcs::git;
                        let result = git::for_each_changed_file_between_commits(
                            &cwd,
                            &ref_name_for_thread,
                            move |change| match change {
                                Ok(change) => injector.push(change).is_ok(),
                                Err(_err) => true,
                            },
                        );

                        if let Err(err) = result {
                            log::error!("Failed to get diff files: {}", err);
                        }
                    });

                    compositor.push(Box::new(overlaid(picker)));
                },
            ));
            Ok(call)
        };

        cx.jobs.callback(callback);
        Ok(())
    }
}
