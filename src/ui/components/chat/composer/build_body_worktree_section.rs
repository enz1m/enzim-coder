{
    let worktree_popover = gtk::Popover::new();
    worktree_popover.set_has_arrow(true);
    worktree_popover.set_autohide(true);
    worktree_popover.set_position(gtk::PositionType::Top);
    worktree_popover.set_parent(&worktree_button);
    worktree_popover.add_css_class("composer-worktree-popover");

    let worktree_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    worktree_box.add_css_class("composer-worktree-popover-box");
    worktree_box.set_margin_start(10);
    worktree_box.set_margin_end(10);
    worktree_box.set_margin_top(10);
    worktree_box.set_margin_bottom(10);
    worktree_box.set_size_request(300, -1);

    let worktree_title = gtk::Label::new(Some("Create Worktree Variants"));
    worktree_title.set_xalign(0.0);
    worktree_title.add_css_class("composer-worktree-title");
    worktree_box.append(&worktree_title);

    let worktree_subtitle = gtk::Label::new(Some(
        "Create forked child threads with isolated Git worktree folders.",
    ));
    worktree_subtitle.set_wrap(true);
    worktree_subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    worktree_subtitle.set_xalign(0.0);
    worktree_subtitle.add_css_class("composer-worktree-subtitle");
    worktree_box.append(&worktree_subtitle);

    let count_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    count_row.add_css_class("composer-worktree-count-row");
    let count_label = gtk::Label::new(Some("Variants"));
    count_label.add_css_class("composer-worktree-count-label");
    count_label.set_xalign(0.0);
    count_label.set_hexpand(true);
    let worktree_count = gtk::SpinButton::with_range(1.0, 8.0, 1.0);
    worktree_count.set_value(2.0);
    worktree_count.set_numeric(true);
    worktree_count.add_css_class("composer-worktree-count");
    worktree_count.set_width_chars(2);
    count_row.append(&count_label);
    count_row.append(&worktree_count);
    worktree_box.append(&count_row);

    let worktree_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    worktree_actions.add_css_class("composer-worktree-actions");
    worktree_actions.set_halign(gtk::Align::End);
    let worktree_cancel = gtk::Button::with_label("Cancel");
    worktree_cancel.add_css_class("composer-worktree-cancel");
    let worktree_create = gtk::Button::with_label("Create");
    worktree_create.add_css_class("suggested-action");
    worktree_create.add_css_class("composer-worktree-create");
    worktree_actions.append(&worktree_cancel);
    worktree_actions.append(&worktree_create);
    worktree_box.append(&worktree_actions);
    worktree_popover.set_child(Some(&worktree_box));

    {
        let db = db.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        let messages_box = messages_box.clone();
        let messages_scroll = messages_scroll.clone();
        let conversation_stack = conversation_stack.clone();
        let worktree_button = worktree_button.clone();
        let worktree_button_for_parent = worktree_button.clone();
        let worktree_popover = worktree_popover.clone();
        worktree_button.connect_clicked(move |_| {
            let active_thread_id = active_codex_thread_id.borrow().clone();
            if let Some(codex_thread_id) = active_thread_id.as_deref() {
                if let Some(thread) = db
                    .get_thread_record_by_codex_thread_id(codex_thread_id)
                    .ok()
                    .flatten()
                {
                    let is_worktree_thread = thread.worktree_active
                        && thread
                            .worktree_path
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .is_some();
                    if is_worktree_thread {
                        let worktree_path = thread.worktree_path.clone().unwrap_or_default();
                        let Some(live_workspace_path) =
                            db.workspace_path_for_local_thread(thread.id).ok().flatten()
                        else {
                            super::message_render::append_message(
                                &messages_box,
                                Some(&messages_scroll),
                                &conversation_stack,
                                "Unable to resolve live workspace path for this worktree thread.",
                                false,
                                std::time::SystemTime::now(),
                            );
                            return;
                        };
                        let parent = worktree_button_for_parent
                            .root()
                            .and_then(|root| root.downcast::<gtk::Window>().ok());
                        open_worktree_merge_popup(
                            parent,
                            db.clone(),
                            active_workspace_path.clone(),
                            &messages_box,
                            &messages_scroll,
                            &conversation_stack,
                            thread.id,
                            &worktree_path,
                            &live_workspace_path,
                        );
                        return;
                    }
                }
            }

            if worktree_popover.is_visible() {
                worktree_popover.popdown();
            } else {
                worktree_popover.popup();
            }
        });
    }

    {
        let worktree_popover = worktree_popover.clone();
        worktree_cancel.connect_clicked(move |_| {
            worktree_popover.popdown();
        });
    }

    {
        let db = db.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        let worktree_button = worktree_button.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(220), move || {
            if worktree_button.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let is_active_worktree = active_codex_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| db.get_thread_record_by_codex_thread_id(thread_id).ok())
                .flatten()
                .map(|thread| {
                    thread.worktree_active
                        && thread
                            .worktree_path
                            .as_deref()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .is_some()
                })
                .unwrap_or(false);
            if is_active_worktree {
                worktree_button.set_icon_name("merge-symbolic");
                worktree_button.set_tooltip_text(Some("Merge changes and stop this worktree"));
                worktree_button.add_css_class("composer-worktree-button-merge-active");
            } else {
                worktree_button.set_icon_name("git-symbolic");
                worktree_button.set_tooltip_text(Some("Create worktree variants"));
                worktree_button.remove_css_class("composer-worktree-button-merge-active");
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    {
        let db = db.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        let thread_lock_note = thread_lock_note.clone();
        let input_view = input_view.clone();
        let add_file = add_file.clone();
        let mic = mic.clone();
        let worktree_button = worktree_button.clone();
        let send = send.clone();
        let selected_images = selected_images.clone();
        let thread_locked = thread_locked.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(180), move || {
            if send.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let is_locked = active_codex_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| db.is_codex_thread_locked(thread_id).ok())
                .or_else(|| {
                    db.get_setting("last_active_thread_id")
                        .ok()
                        .flatten()
                        .and_then(|value| value.parse::<i64>().ok())
                        .and_then(|thread_id| db.is_local_thread_locked(thread_id).ok())
                })
                .unwrap_or(false);
            if *thread_locked.borrow() != is_locked {
                thread_locked.replace(is_locked);
                if is_locked {
                    thread_lock_note.set_text(
                        "This thread was started with a different Codex account. History can't be loaded from Codex. Start a new thread.",
                    );
                    thread_lock_note.set_visible(true);
                    input_view.set_editable(false);
                    input_view.set_cursor_visible(false);
                    add_file.set_sensitive(false);
                    mic.set_sensitive(false);
                    worktree_button.set_sensitive(false);
                    send.set_icon_name("padlock-closed-symbolic");
                    send.set_tooltip_text(Some("Thread locked to another Codex account"));
                } else {
                    thread_lock_note.set_visible(false);
                    input_view.set_editable(true);
                    input_view.set_cursor_visible(true);
                    add_file.set_sensitive(true);
                    mic.set_sensitive(true);
                    worktree_button.set_sensitive(true);
                    send.set_tooltip_text(None);
                }
            }
            update_send_button_active_state(
                &send,
                &input_view,
                &selected_images,
                *thread_locked.borrow(),
            );
            gtk::glib::ControlFlow::Continue
        });
    }

    {
        let send = send.clone();
        let db = db.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        let was_in_progress = Rc::new(RefCell::new(false));
        let was_in_progress_for_timer = was_in_progress.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(80), move || {
            if send.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let active_thread = active_codex_thread_id.borrow().clone();
            let thread_locked = active_thread
                .as_deref()
                .and_then(|thread_id| db.is_codex_thread_locked(thread_id).ok())
                .or_else(|| {
                    db.get_setting("last_active_thread_id")
                        .ok()
                        .flatten()
                        .and_then(|value| value.parse::<i64>().ok())
                        .and_then(|thread_id| db.is_local_thread_locked(thread_id).ok())
                })
                .unwrap_or(false);
            if thread_locked {
                send.set_icon_name("padlock-closed-symbolic");
                send.set_tooltip_text(Some("Thread locked to another Codex account"));
                was_in_progress_for_timer.replace(false);
                return gtk::glib::ControlFlow::Continue;
            }

            let in_progress_for_active = active_thread
                .as_deref()
                .and_then(super::codex_runtime::active_turn_for_thread)
                .is_some();

            if *was_in_progress_for_timer.borrow() != in_progress_for_active {
                send.set_icon_name(if in_progress_for_active {
                    "media-playback-stop-symbolic"
                } else {
                    "satnav-symbolic"
                });
                was_in_progress_for_timer.replace(in_progress_for_active);
            }

            gtk::glib::ControlFlow::Continue
        });
    }

    {
        let db = db.clone();
        let manager = manager.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        let messages_box = messages_box.clone();
        let messages_scroll = messages_scroll.clone();
        let conversation_stack = conversation_stack.clone();
        let suggestion_row = suggestion_row.clone();
        let worktree_popover = worktree_popover.clone();
        let worktree_count = worktree_count.clone();
        let thread_locked = thread_locked.clone();
        worktree_create.connect_clicked(move |_| {
            worktree_popover.popdown();

            if *thread_locked.borrow() {
                super::message_render::append_message(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    "This thread is locked to another Codex account. Worktree variants are unavailable.",
                    false,
                    std::time::SystemTime::now(),
                );
                return;
            }

            let Some(source_codex_thread_id) = active_codex_thread_id.borrow().clone() else {
                super::message_render::append_message(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    "Select a thread first before creating worktree variants.",
                    false,
                    std::time::SystemTime::now(),
                );
                return;
            };

            let Some(source_thread) = db
                .get_thread_record_by_codex_thread_id(&source_codex_thread_id)
                .ok()
                .flatten()
            else {
                super::message_render::append_message(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    "Unable to resolve the selected thread for worktree creation.",
                    false,
                    std::time::SystemTime::now(),
                );
                return;
            };

            let source_workspace_path = source_thread
                .worktree_path
                .as_deref()
                .map(str::trim)
                .filter(|path| !path.is_empty() && source_thread.worktree_active)
                .map(|path| path.to_string())
                .or_else(|| active_workspace_path.borrow().clone())
                .or_else(|| {
                    db.workspace_path_for_codex_thread(&source_codex_thread_id)
                        .ok()
                        .flatten()
                });

            let Some(source_workspace_path) = source_workspace_path else {
                super::message_render::append_message(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    "Could not determine workspace path for worktree creation.",
                    false,
                    std::time::SystemTime::now(),
                );
                return;
            };

            let Some(client) = manager
                .resolve_client_for_thread_id(&source_codex_thread_id)
                .or_else(|| {
                    db.runtime_profile_id()
                        .ok()
                        .flatten()
                        .and_then(|profile_id| manager.client_for_profile(profile_id))
                })
            else {
                super::message_render::append_message(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    "Codex app-server is not available for this thread.",
                    false,
                    std::time::SystemTime::now(),
                );
                return;
            };

            let count = worktree_count.value_as_int().clamp(1, 8) as usize;
            suggestion_row.set_visible(false);
            super::message_render::append_message(
                &messages_box,
                Some(&messages_scroll),
                &conversation_stack,
                &format!("Creating {count} worktree variant thread(s)..."),
                false,
                std::time::SystemTime::now(),
            );

            let (tx, rx) = mpsc::channel::<WorktreeBatchResult>();
            let source_thread_bg = source_thread.clone();
            thread::spawn(move || {
                let mut batch = WorktreeBatchResult {
                    entries: Vec::new(),
                    errors: Vec::new(),
                };
                for idx in 1..=count {
                    let forked_codex_thread_id = match client.thread_fork(&source_codex_thread_id) {
                        Ok(thread_id) => thread_id,
                        Err(err) => {
                            batch
                                .errors
                                .push(format!("Variant {idx}: failed to fork thread ({err})"));
                            continue;
                        }
                    };
                    match crate::worktree::create_thread_worktree(
                        &source_workspace_path,
                        source_thread_bg.id,
                        idx,
                    ) {
                        Ok(created) => batch.entries.push(WorktreeBatchEntry {
                            forked_codex_thread_id,
                            worktree_path: created.path,
                            worktree_branch: created.branch,
                        }),
                        Err(err) => batch.errors.push(format!(
                            "Variant {idx}: failed to create worktree ({err})"
                        )),
                    }
                }
                let _ = tx.send(batch);
            });

            let db_for_ui = db.clone();
            let messages_box_for_ui = messages_box.clone();
            let messages_scroll_for_ui = messages_scroll.clone();
            let conversation_stack_for_ui = conversation_stack.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(40), move || {
                if messages_box_for_ui.root().is_none() {
                    return gtk::glib::ControlFlow::Break;
                }
                let Ok(batch) = rx.try_recv() else {
                    return gtk::glib::ControlFlow::Continue;
                };
                let mut created_count = 0usize;
                let mut errors = batch.errors;
                for entry in batch.entries {
                    let created_thread = match db_for_ui.create_thread(
                        source_thread.workspace_id,
                        source_thread.profile_id,
                        Some(source_thread.id),
                        &source_thread.title,
                        Some(&entry.forked_codex_thread_id),
                        source_thread.codex_account_type.as_deref(),
                        source_thread.codex_account_email.as_deref(),
                    ) {
                        Ok(thread) => thread,
                        Err(err) => {
                            errors.push(format!("Failed to persist child thread: {err}"));
                            continue;
                        }
                    };

                    if let Err(err) = db_for_ui.set_thread_worktree_info(
                        created_thread.id,
                        Some(&entry.worktree_path),
                        Some(&entry.worktree_branch),
                        true,
                    ) {
                        errors.push(format!(
                            "Failed to attach worktree metadata for thread {}: {}",
                            created_thread.id, err
                        ));
                    }

                    let thread_for_ui = db_for_ui
                        .get_thread_record(created_thread.id)
                        .ok()
                        .flatten()
                        .unwrap_or(created_thread);
                    if let Some(root) = messages_box_for_ui.root() {
                        let root_widget: gtk::Widget = root.upcast();
                        let inserted =
                            crate::ui::components::thread_list::append_thread_under_parent_from_root_passive(
                                &root_widget,
                                source_thread.id,
                                thread_for_ui,
                            );
                        if !inserted {
                            errors.push("Failed to insert new worktree thread in sidebar.".to_string());
                        }
                    }
                    created_count += 1;
                }

                let mut summary = format!("Created {created_count} worktree variant thread(s).");
                if !errors.is_empty() {
                    let preview = errors.into_iter().take(3).collect::<Vec<String>>().join("\n");
                    summary.push_str(&format!("\n\nSome variants failed:\n{preview}"));
                }
                super::message_render::append_message(
                    &messages_box_for_ui,
                    Some(&messages_scroll_for_ui),
                    &conversation_stack_for_ui,
                    &summary,
                    false,
                    std::time::SystemTime::now(),
                );
                gtk::glib::ControlFlow::Break
            });
        });
    }
}
