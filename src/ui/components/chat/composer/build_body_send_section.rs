{
    let refresh_queue_ui: Rc<dyn Fn()> = {
        let queued_entries = queued_entries.clone();
        let queued_box = queued_box.clone();
        let queued_merge_button = queued_merge_button.clone();
        let queue_summary_box = queue_summary_box.clone();
        let queue_summary_label = queue_summary_label.clone();
        let queue_summary_toggle = queue_summary_toggle.clone();
        let queue_expanded = queue_expanded.clone();
        Rc::new(move || {
            let len = queued_entries.borrow().len();
            let mut expanded = *queue_expanded.borrow();
            if len == 0 && expanded {
                queue_expanded.replace(false);
                expanded = false;
            }

            queue_summary_box.set_visible(len > 0);
            if len > 0 {
                let summary = if len == 1 {
                    "1 queued message".to_string()
                } else {
                    format!("{len} queued messages")
                };
                queue_summary_label.set_text(&summary);
            } else {
                queue_summary_label.set_text("");
            }
            queue_summary_toggle.set_text(if expanded { "↓" } else { "↑" });

            queued_box.set_visible(len > 0 && expanded);
            queued_merge_button.set_visible(len > 1 && expanded);
        })
    };
    (refresh_queue_ui)();

    {
        let queue_expanded = queue_expanded.clone();
        let refresh_queue_ui = refresh_queue_ui.clone();
        let queue_summary_toggle_for_click = queue_summary_toggle.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            let next = !*queue_expanded.borrow();
            queue_expanded.replace(next);
            (refresh_queue_ui)();
        });
        queue_summary_toggle_for_click.add_controller(click);

        let queue_summary_toggle_for_hover_enter = queue_summary_toggle.clone();
        let queue_summary_toggle_for_hover_leave = queue_summary_toggle.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |_, _, _| {
            queue_summary_toggle_for_hover_enter.add_css_class("is-hover");
        });
        motion.connect_leave(move |_| {
            queue_summary_toggle_for_hover_leave.remove_css_class("is-hover");
        });
        queue_summary_toggle.add_controller(motion);
    }

    {
        let queued_entries = queued_entries.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            let first_steer_button = queued_entries
                .borrow()
                .front()
                .map(|entry| entry.steer_button.clone());
            if let Some(button) = first_steer_button {
                button.emit_clicked();
            }
        });
        queue_summary_steer.add_controller(click);

        let queue_summary_steer_for_hover_enter = queue_summary_steer.clone();
        let queue_summary_steer_for_hover_leave = queue_summary_steer.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |_, _, _| {
            queue_summary_steer_for_hover_enter.add_css_class("is-hover");
        });
        motion.connect_leave(move |_| {
            queue_summary_steer_for_hover_leave.remove_css_class("is-hover");
        });
        queue_summary_steer.add_controller(motion);
    }

    {
        let db = db.clone();
        let input_view = input_view.clone();
        let input_scroll = input_scroll.clone();
        let placeholder = placeholder.clone();
        let manager = manager.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        let selected_mode_id = selected_mode_id.clone();
        let selected_model_id = selected_model_id.clone();
        let selected_effort = selected_effort.clone();
        let selected_access_mode = selected_access_mode.clone();
        let selected_mentions = selected_mentions.clone();
        let selected_images = selected_images.clone();
        let mention_popover = mention_popover.clone();
        let image_preview_scroll = image_preview_scroll.clone();
        let image_preview_strip = image_preview_strip.clone();
        let send_button = send.clone();
        let send_error_tx = send_error_tx.clone();
        let messages_box = messages_box.clone();
        let messages_scroll = messages_scroll.clone();
        let conversation_stack = conversation_stack.clone();
        let suggestion_row = suggestion_row.clone();
        let queued_box = queued_box.clone();
        let queued_entries = queued_entries.clone();
        let queued_next_id = queued_next_id.clone();
        let queued_dispatch_state = queued_dispatch_state.clone();
        let refresh_queue_ui = refresh_queue_ui.clone();
        let thread_locked = thread_locked.clone();
        send.connect_clicked(move |_| {
            let skills_allowed_for_text = |text: &str| -> Result<(), String> {
                if text.trim().is_empty() {
                    return Ok(());
                }
                let profile_id = db.runtime_profile_id().ok().flatten().unwrap_or(1);
                let catalog = crate::skill_mcp::load_catalog(db.as_ref());
                let assignments =
                    crate::skill_mcp::load_profile_assignments(db.as_ref(), profile_id);
                let blocked = crate::skill_mcp::disabled_skill_markers(
                    text,
                    &catalog,
                    &assignments,
                );
                if blocked.is_empty() {
                    Ok(())
                } else {
                    Err(format!(
                        "Blocked by profile Skill assignment: {}",
                        blocked
                            .into_iter()
                            .map(|name| format!("${name}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            };

            let buf = input_view.buffer();
            let start = buf.start_iter();
            let end = buf.end_iter();
            let text = buf.text(&start, &end, true).to_string();
            let attached_images = selected_images.borrow().clone();
            let has_images = !attached_images.is_empty();

            if *thread_locked.borrow() {
                super::message_render::append_message(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    "This thread was started with a different Codex account. Start a new thread to continue.",
                    false,
                    std::time::SystemTime::now(),
                );
                return;
            }

            let active_thread = active_codex_thread_id.borrow().clone();
            let turn_id = active_thread
                .as_deref()
                .and_then(super::codex_runtime::active_turn_for_thread);
            let is_turn_in_progress = turn_id.is_some();

            if is_turn_in_progress {
                if !text.trim().is_empty() || has_images {
                    if let Err(err) = skills_allowed_for_text(&text) {
                        super::message_render::append_message(
                            &messages_box,
                            Some(&messages_scroll),
                            &conversation_stack,
                            &err,
                            false,
                            std::time::SystemTime::now(),
                        );
                        return;
                    }
                    let queued_mode = selected_mode_id.borrow().clone();
                    let queued_model_id = selected_model_id.borrow().clone();
                    let queued_effort = selected_effort.borrow().clone();
                    let queued_access_mode = selected_access_mode.borrow().clone();
                    let queued_sandbox_policy =
                        super::codex_controls::sandbox_policy_for(&queued_access_mode);
                    let queued_collaboration_mode = collaboration_mode_payload(
                        &queued_mode,
                        &queued_model_id,
                        &queued_effort,
                    );
                    let mentions_for_turn: Vec<(String, String)> = selected_mentions
                        .borrow()
                        .iter()
                        .filter(|mention| text.contains(&format!("@{}", mention.display)))
                        .map(|mention| (mention.display.clone(), mention.path.clone()))
                        .collect();
                    let queued_image_paths: Vec<String> =
                        attached_images.iter().map(|image| image.path.clone()).collect();
                    let queued_summary =
                        send_payload_summary_from_paths(&text, &queued_image_paths);
                    let queued_id = {
                        let mut next_id = queued_next_id.borrow_mut();
                        let current = *next_id;
                        *next_id = next_id.saturating_add(1);
                        current
                    };
                    let queued_payload = Rc::new(RefCell::new(QueuedPayload {
                        remote_prompt_id: None,
                        text: text.clone(),
                        summary: queued_summary.clone(),
                        mentions: mentions_for_turn.clone(),
                        images: queued_image_paths.clone(),
                        expected_thread_id: active_thread.clone(),
                        model_id: queued_model_id.clone(),
                        effort: queued_effort.clone(),
                        sandbox_policy: queued_sandbox_policy.clone(),
                        collaboration_mode: queued_collaboration_mode.clone(),
                    }));

                    let queued_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
                    queued_row.add_css_class("chat-queued-card");

                    let queued_text = gtk::Label::new(Some(&queued_summary));
                    queued_text.set_xalign(0.0);
                    queued_text.set_yalign(0.0);
                    queued_text.set_wrap(true);
                    queued_text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    queued_text.set_hexpand(true);
                    queued_text.add_css_class("chat-queued-text");
                    let queued_text_scroll = gtk::ScrolledWindow::builder()
                        .hscrollbar_policy(gtk::PolicyType::Never)
                        .vscrollbar_policy(gtk::PolicyType::Automatic)
                        .propagate_natural_height(true)
                        .min_content_height(20)
                        .max_content_height(96)
                        .child(&queued_text)
                        .build();
                    queued_text_scroll.set_has_frame(false);
                    queued_text_scroll.set_hexpand(true);
                    queued_text_scroll.add_css_class("chat-queued-text-scroll");
                    queued_row.append(&queued_text_scroll);

                    let queued_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    queued_actions.add_css_class("chat-queued-actions");
                    queued_actions.set_halign(gtk::Align::End);

                    let cancel_button = gtk::Button::with_label("Cancel");
                    cancel_button.add_css_class("app-flat-button");
                    cancel_button.add_css_class("chat-queued-action");
                    cancel_button.add_css_class("chat-queued-cancel");
                    queued_actions.append(&cancel_button);

                    let steer_button = gtk::Button::with_label("Steer");
                    steer_button.add_css_class("app-flat-button");
                    steer_button.add_css_class("chat-queued-action");
                    steer_button.add_css_class("chat-queued-steer");
                    queued_actions.append(&steer_button);

                    queued_row.append(&queued_actions);
                    queued_box.append(&queued_row);
                    queued_entries.borrow_mut().push_back(QueuedUiEntry {
                        id: queued_id,
                        row: queued_row.clone(),
                        preview_label: queued_text.clone(),
                        steer_button: steer_button.clone(),
                        payload: queued_payload.clone(),
                    });
                    (refresh_queue_ui)();

                    let queued_entries_for_steer = queued_entries.clone();
                    let queued_entries_for_cancel = queued_entries.clone();
                    let manager_for_steer = manager.clone();
                    let db_for_steer = db.clone();
                    let active_codex_thread_id_for_steer = active_codex_thread_id.clone();
                    let queued_dispatch_state_for_steer = queued_dispatch_state.clone();
                    let send_error_tx_for_steer = send_error_tx.clone();
                    let steer_note_tx_for_steer = steer_note_tx.clone();
                    let turn_started_ui_tx_for_steer = turn_started_ui_tx.clone();
                    let queued_box_for_steer = queued_box.clone();
                    let messages_box_for_steer = messages_box.clone();
                    let messages_scroll_for_steer = messages_scroll.clone();
                    let conversation_stack_for_steer = conversation_stack.clone();
                    let suggestion_row_for_steer = suggestion_row.clone();
                    let queued_box_for_cancel = queued_box.clone();
                    let refresh_queue_ui_for_cancel = refresh_queue_ui.clone();
                    let db_for_cancel = db.clone();
                    let input_view_for_cancel = input_view.clone();
                    let input_scroll_for_cancel = input_scroll.clone();
                    let placeholder_for_cancel = placeholder.clone();
                    let send_button_for_cancel = send_button.clone();
                    let refresh_queue_ui_for_steer = refresh_queue_ui.clone();
                    cancel_button.connect_clicked(move |_| {
                        let removed = {
                            let mut entries = queued_entries_for_cancel.borrow_mut();
                            entries
                                .iter()
                                .position(|entry| entry.id == queued_id)
                                .and_then(|idx| entries.remove(idx))
                        };
                        let Some(removed) = removed else {
                            return;
                        };
                        if removed.row.parent().is_some() {
                            queued_box_for_cancel.remove(&removed.row);
                        }
                        if let Some(remote_prompt_id) = removed.payload.borrow().remote_prompt_id {
                            let _ = db_for_cancel.mark_remote_pending_prompt_consumed(remote_prompt_id);
                        }
                        (refresh_queue_ui_for_cancel)();

                        let buf = input_view_for_cancel.buffer();
                        let start = buf.start_iter();
                        let end = buf.end_iter();
                        let existing = buf.text(&start, &end, true).to_string();
                        let queued_text_value_for_cancel = removed.payload.borrow().text.clone();
                        let restored = if existing.trim().is_empty() {
                            queued_text_value_for_cancel
                        } else {
                            format!("{existing}\n\n{}", queued_text_value_for_cancel)
                        };
                        buf.set_text(&restored);
                        placeholder_for_cancel.set_visible(restored.trim().is_empty());
                        send_button_for_cancel.set_sensitive(true);
                        update_input_height(
                            &input_scroll_for_cancel,
                            &input_view_for_cancel,
                            min_height,
                            max_height,
                        );
                        input_view_for_cancel.grab_focus();
                    });

                    steer_button.connect_clicked(move |_| {
                        let queued_entry = {
                            let mut entries = queued_entries_for_steer.borrow_mut();
                            entries
                                .iter()
                                .position(|entry| entry.id == queued_id)
                                .and_then(|idx| entries.remove(idx))
                        };
                        let Some(queued_entry) = queued_entry else {
                            return;
                        };
                        if queued_entry.row.parent().is_some() {
                            queued_box_for_steer.remove(&queued_entry.row);
                        }
                        if let Some(remote_prompt_id) = queued_entry.payload.borrow().remote_prompt_id {
                            let _ = db_for_steer.mark_remote_pending_prompt_consumed(remote_prompt_id);
                        }
                        (refresh_queue_ui_for_steer)();

                        let payload = queued_entry.payload.borrow().clone();
                        let thread_id = payload
                            .expected_thread_id
                            .or_else(|| active_codex_thread_id_for_steer.borrow().clone());
                        let Some(thread_id) = thread_id else {
                            let _ = send_error_tx_for_steer.send(
                                "Create or select a thread from the sidebar first.".to_string(),
                            );
                            return;
                        };
                        let Some(client) = manager_for_steer
                            .resolve_client_for_thread_id(&thread_id)
                            .or_else(|| {
                                db_for_steer
                                    .runtime_profile_id()
                                    .ok()
                                    .flatten()
                                    .and_then(|profile_id| {
                                        manager_for_steer.client_for_profile(profile_id)
                                    })
                            })
                        else {
                            let _ = send_error_tx_for_steer
                                .send("Codex app-server is not available.".to_string());
                            return;
                        };

                        let auto_dispatched = queued_dispatch_state_for_steer
                            .borrow()
                            .as_ref()
                            .map(|(dispatch_thread_id, _, _)| dispatch_thread_id == &thread_id)
                            .unwrap_or(false);
                        let current_turn_id = if auto_dispatched {
                            None
                        } else {
                            super::codex_runtime::active_turn_for_thread(&thread_id)
                        };

                        let queued_text_for_thread = payload.text.clone();
                        let queued_summary_for_thread = payload.summary.clone();
                        let queued_mentions_for_thread = payload.mentions.clone();
                        let queued_images_for_thread = payload.images.clone();
                        let send_error_tx_for_thread = send_error_tx_for_steer.clone();
                        let steer_note_tx_for_thread = steer_note_tx_for_steer.clone();
                        let steer_turn_id = current_turn_id.clone();
                        let queued_model_id_for_turn = payload.model_id.clone();
                        let queued_effort_for_turn = payload.effort.clone();
                        let queued_sandbox_policy_for_turn = payload.sandbox_policy.clone();
                        let queued_collaboration_mode_for_turn = payload.collaboration_mode.clone();
                        let pending_row_marker_for_turn = if current_turn_id.is_none() {
                            let pending_row_marker = next_pending_user_row_marker();
                            let user_content =
                                super::message_render::append_user_message_with_images(
                                    &messages_box_for_steer,
                                    Some(&messages_scroll_for_steer),
                                    &conversation_stack_for_steer,
                                    &queued_text_for_thread,
                                    &queued_images_for_thread,
                                    std::time::SystemTime::now(),
                                );
                            let _ = super::message_render::set_message_row_marker(
                                &user_content,
                                &pending_row_marker,
                            );
                            suggestion_row_for_steer.set_visible(false);
                            super::message_render::scroll_to_bottom(&messages_scroll_for_steer);
                            Some(pending_row_marker)
                        } else {
                            None
                        };
                        let turn_started_ui_tx_for_thread = turn_started_ui_tx_for_steer.clone();
                        thread::spawn(move || {
                            if let Some(turn_id) = steer_turn_id.clone() {
                                match client.turn_steer(
                                    &thread_id,
                                    &turn_id,
                                    &queued_text_for_thread,
                                    &queued_images_for_thread,
                                    &queued_mentions_for_thread,
                                ) {
                                    Ok(_) => {
                                        let _ = steer_note_tx_for_thread
                                            .send(queued_summary_for_thread.clone());
                                        return;
                                    }
                                    Err(err) => {
                                        eprintln!("turn/steer failed, falling back to send: {err}");
                                    }
                                }
                            }

                            if let Some(turn_id) = current_turn_id.clone() {
                                let _ = client.turn_interrupt(&thread_id, &turn_id);
                                thread::sleep(Duration::from_millis(200));
                            }

                            let _ = crate::data::background_repo::BackgroundRepo::ensure_thread_baseline_checkpoint(&thread_id);
                            let workspace_path_for_turn =
                                crate::data::background_repo::BackgroundRepo::workspace_path_for_codex_thread(&thread_id);
                            if let Some(workspace_path) = workspace_path_for_turn.as_deref() {
                                match client.thread_resume(
                                    &thread_id,
                                    Some(workspace_path),
                                    Some(&queued_model_id_for_turn),
                                ) {
                                    Ok(resolved_thread_id) => {
                                        if resolved_thread_id != thread_id {
                                            let _ = send_error_tx_for_thread.send(format!(
                                                "Failed to send queued prompt: thread resume mismatch (expected {thread_id}, got {resolved_thread_id})"
                                            ));
                                            return;
                                        }
                                    }
                                    Err(err) => {
                                        if !is_expected_pre_materialization_error(&err) {
                                            let _ = send_error_tx_for_thread.send(format!(
                                                "Failed to send queued prompt: could not resume thread in workspace ({err})"
                                            ));
                                            return;
                                        }
                                    }
                                }
                            }

                            for attempt in 0..8 {
                                match client.turn_start(
                                    &thread_id,
                                    &queued_text_for_thread,
                                    &queued_images_for_thread,
                                    &queued_mentions_for_thread,
                                    Some(&queued_model_id_for_turn),
                                    Some(&queued_effort_for_turn),
                                    queued_sandbox_policy_for_turn.clone(),
                                    None,
                                    queued_collaboration_mode_for_turn.clone(),
                                    workspace_path_for_turn.as_deref(),
                                ) {
                                    Ok(turn_id) => {
                                        if let Some(pending_marker) =
                                            pending_row_marker_for_turn.clone()
                                        {
                                            let _ = turn_started_ui_tx_for_thread
                                                .send((pending_marker, turn_id));
                                        }
                                        return;
                                    }
                                    Err(err) => {
                                        let retryable = err.contains("active turn")
                                            || err.contains("in progress")
                                            || err.contains("already active")
                                            || err.contains("thread is active");
                                        if retryable && attempt < 7 {
                                            thread::sleep(Duration::from_millis(200));
                                            continue;
                                        }
                                        let _ = send_error_tx_for_thread
                                            .send(format!("Failed to send queued prompt: {err}"));
                                        return;
                                    }
                                }
                            }
                        });
                    });

                    buf.set_text("");
                    selected_mentions.borrow_mut().clear();
                    selected_images.borrow_mut().clear();
                    refresh_image_preview_strip(
                        &image_preview_scroll,
                        &image_preview_strip,
                        &selected_images,
                        &send_button,
                        &input_view,
                        &thread_locked,
                    );
                    mention_popover.popdown();
                    placeholder.set_visible(true);
                    update_input_height(&input_scroll, &input_view, min_height, max_height);
                    return;
                }

                let Some(thread_id) = active_thread else {
                    return;
                };
                let Some(client) = resolve_client_for_thread(&thread_id) else {
                    return;
                };
                let Some(turn_id) = turn_id else {
                    return;
                };

                let send_error_tx_for_interrupt = send_error_tx.clone();
                thread::spawn(move || {
                    if let Err(err) = client.turn_interrupt(&thread_id, &turn_id) {
                        let _ =
                            send_error_tx_for_interrupt.send(format!("Failed to stop turn: {err}"));
                        eprintln!("failed to interrupt turn: {err}");
                    }
                });
                return;
            }

            if text.trim().is_empty() && !has_images {
                return;
            }
            if let Err(err) = skills_allowed_for_text(&text) {
                super::message_render::append_message(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    &err,
                    false,
                    std::time::SystemTime::now(),
                );
                return;
            }

            let pending_row_marker = next_pending_user_row_marker();
            let image_paths_for_preview: Vec<String> =
                attached_images.iter().map(|image| image.path.clone()).collect();
            let user_content = super::message_render::append_user_message_with_images(
                &messages_box,
                Some(&messages_scroll),
                &conversation_stack,
                &text,
                &image_paths_for_preview,
                std::time::SystemTime::now(),
            );
            let _ = super::message_render::set_message_row_marker(
                &user_content,
                &pending_row_marker,
            );
            suggestion_row.set_visible(false);
            super::message_render::scroll_to_bottom(&messages_scroll);

            let Some(thread_id) = active_codex_thread_id.borrow().clone() else {
                super::message_render::append_message(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    "Create or select a thread from the sidebar first.",
                    false,
                    std::time::SystemTime::now(),
                );
                return;
            };
            let Some(client) = resolve_client_for_thread(&thread_id) else {
                super::message_render::append_message(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    "Codex app-server is not available. Start `codex app-server` and retry.",
                    false,
                    std::time::SystemTime::now(),
                );
                return;
            };

            if !text.trim().is_empty() {
                if let Some(next_title) = title_from_first_prompt(&text) {
                    match db.rename_thread_if_new_by_codex_id(&thread_id, &next_title) {
                        Ok(Some(local_thread_id)) => {
                            if let Some(root) = messages_box.root() {
                                let root_widget: gtk::Widget = root.upcast();
                                let _ = crate::ui::components::thread_list::update_thread_row_title(
                                    &root_widget,
                                    local_thread_id,
                                    &next_title,
                                );
                            }
                        }
                        Ok(None) => {}
                        Err(err) => eprintln!("failed to rename thread on first prompt: {err}"),
                    }
                }
            }

            let collaboration_mode = selected_mode_id.borrow().clone();
            let model_id = selected_model_id.borrow().clone();
            let effort = selected_effort.borrow().clone();
            let access_mode = selected_access_mode.borrow().clone();
            let sandbox_policy = super::codex_controls::sandbox_policy_for(&access_mode);
            let collaboration_mode_for_turn =
                collaboration_mode_payload(&collaboration_mode, &model_id, &effort);
            let send_error_tx_for_thread = send_error_tx.clone();
            let turn_started_ui_tx = turn_started_ui_tx.clone();
            let mentions_for_turn: Vec<(String, String)> = selected_mentions
                .borrow()
                .iter()
                .filter(|mention| text.contains(&format!("@{}", mention.display)))
                .map(|mention| (mention.display.clone(), mention.path.clone()))
                .collect();
            let image_paths_for_turn: Vec<String> =
                attached_images.iter().map(|image| image.path.clone()).collect();

            thread::spawn(move || {
                let _ = crate::data::background_repo::BackgroundRepo::ensure_thread_baseline_checkpoint(&thread_id);
                let workspace_path_for_turn =
                    crate::data::background_repo::BackgroundRepo::workspace_path_for_codex_thread(&thread_id);
                if let Some(workspace_path) = workspace_path_for_turn.as_deref() {
                    match client.thread_resume(&thread_id, Some(workspace_path), Some(&model_id)) {
                        Ok(resolved_thread_id) => {
                            if resolved_thread_id != thread_id {
                                let _ = send_error_tx_for_thread.send(format!(
                                    "Turn failed: thread resume mismatch (expected {thread_id}, got {resolved_thread_id})"
                                ));
                                return;
                            }
                        }
                        Err(err) => {
                            if !is_expected_pre_materialization_error(&err) {
                                let _ = send_error_tx_for_thread.send(format!(
                                    "Turn failed: could not resume thread in workspace ({err})"
                                ));
                                return;
                            }
                        }
                    }
                }

                match client.turn_start(
                    &thread_id,
                    &text,
                    &image_paths_for_turn,
                    &mentions_for_turn,
                    Some(&model_id),
                    Some(&effort),
                    sandbox_policy,
                    None,
                    collaboration_mode_for_turn,
                    workspace_path_for_turn.as_deref(),
                ) {
                    Ok(turn_id) => {
                        let _ = turn_started_ui_tx.send((pending_row_marker, turn_id));
                    }
                    Err(err) => {
                        let _ = send_error_tx_for_thread.send(format!("Turn failed: {err}"));
                        eprintln!("failed to start turn: {err}");
                    }
                }
            });

            buf.set_text("");
            selected_mentions.borrow_mut().clear();
            selected_images.borrow_mut().clear();
            refresh_image_preview_strip(
                &image_preview_scroll,
                &image_preview_strip,
                &selected_images,
                &send_button,
                &input_view,
                &thread_locked,
            );
            mention_popover.popdown();
            placeholder.set_visible(true);
            update_input_height(&input_scroll, &input_view, min_height, max_height);
        });
    }

    {
        let db = db.clone();
        let queued_entries = queued_entries.clone();
        let queued_box = queued_box.clone();
        let queued_next_id = queued_next_id.clone();
        let refresh_queue_ui = refresh_queue_ui.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        let selected_mode_id = selected_mode_id.clone();
        let selected_model_id = selected_model_id.clone();
        let selected_effort = selected_effort.clone();
        let selected_access_mode = selected_access_mode.clone();
        let send = send.clone();
        let input_view = input_view.clone();
        let input_scroll = input_scroll.clone();
        let placeholder = placeholder.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(260), move || {
            if send.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let Some(active_thread_id) = active_codex_thread_id.borrow().clone() else {
                return gtk::glib::ControlFlow::Continue;
            };
            let Some(local_thread_id) = db
                .get_thread_record_by_codex_thread_id(&active_thread_id)
                .ok()
                .flatten()
                .map(|thread| thread.id)
            else {
                return gtk::glib::ControlFlow::Continue;
            };

            let pending_prompts = db
                .list_remote_pending_prompts_for_local_thread(local_thread_id, 24)
                .unwrap_or_default();
            if pending_prompts.is_empty() {
                return gtk::glib::ControlFlow::Continue;
            }

            for prompt in pending_prompts {
                let already_queued = queued_entries.borrow().iter().any(|entry| {
                    entry.payload.borrow().remote_prompt_id == Some(prompt.id)
                });
                if already_queued {
                    continue;
                }

                let queued_mode = selected_mode_id.borrow().clone();
                let queued_model_id = selected_model_id.borrow().clone();
                let queued_effort = selected_effort.borrow().clone();
                let queued_access_mode = selected_access_mode.borrow().clone();
                let queued_sandbox_policy =
                    super::codex_controls::sandbox_policy_for(&queued_access_mode);
                let queued_collaboration_mode =
                    collaboration_mode_payload(&queued_mode, &queued_model_id, &queued_effort);

                let queued_summary =
                    format!("[telegram] {}", send_payload_summary_from_paths(&prompt.text, &[]));
                let queued_id = {
                    let mut next_id = queued_next_id.borrow_mut();
                    let current = *next_id;
                    *next_id = next_id.saturating_add(1);
                    current
                };
                let queued_payload = Rc::new(RefCell::new(QueuedPayload {
                    remote_prompt_id: Some(prompt.id),
                    text: prompt.text.clone(),
                    summary: queued_summary.clone(),
                    mentions: Vec::new(),
                    images: Vec::new(),
                    expected_thread_id: Some(active_thread_id.clone()),
                    model_id: queued_model_id.clone(),
                    effort: queued_effort.clone(),
                    sandbox_policy: queued_sandbox_policy.clone(),
                    collaboration_mode: queued_collaboration_mode.clone(),
                }));

                let queued_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
                queued_row.add_css_class("chat-queued-card");

                let queued_text = gtk::Label::new(Some(&queued_summary));
                queued_text.set_xalign(0.0);
                queued_text.set_yalign(0.0);
                queued_text.set_wrap(true);
                queued_text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                queued_text.set_hexpand(true);
                queued_text.add_css_class("chat-queued-text");

                let queued_text_scroll = gtk::ScrolledWindow::builder()
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .vscrollbar_policy(gtk::PolicyType::Automatic)
                    .propagate_natural_height(true)
                    .min_content_height(20)
                    .max_content_height(96)
                    .child(&queued_text)
                    .build();
                queued_text_scroll.set_has_frame(false);
                queued_text_scroll.set_hexpand(true);
                queued_text_scroll.add_css_class("chat-queued-text-scroll");
                queued_row.append(&queued_text_scroll);

                let queued_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                queued_actions.add_css_class("chat-queued-actions");
                queued_actions.set_halign(gtk::Align::End);

                let cancel_button = gtk::Button::with_label("Cancel");
                cancel_button.add_css_class("app-flat-button");
                cancel_button.add_css_class("chat-queued-action");
                cancel_button.add_css_class("chat-queued-cancel");
                queued_actions.append(&cancel_button);

                let steer_button = gtk::Button::with_label("Steer");
                steer_button.add_css_class("app-flat-button");
                steer_button.add_css_class("chat-queued-action");
                steer_button.add_css_class("chat-queued-steer");
                queued_actions.append(&steer_button);

                queued_row.append(&queued_actions);
                queued_box.append(&queued_row);
                queued_entries.borrow_mut().push_back(QueuedUiEntry {
                    id: queued_id,
                    row: queued_row.clone(),
                    preview_label: queued_text.clone(),
                    steer_button: steer_button.clone(),
                    payload: queued_payload.clone(),
                });
                (refresh_queue_ui)();

                let db_for_cancel = db.clone();
                let queued_entries_for_cancel = queued_entries.clone();
                let queued_box_for_cancel = queued_box.clone();
                let refresh_queue_ui_for_cancel = refresh_queue_ui.clone();
                let input_view_for_cancel = input_view.clone();
                let input_scroll_for_cancel = input_scroll.clone();
                let placeholder_for_cancel = placeholder.clone();
                let send_for_cancel = send.clone();
                cancel_button.connect_clicked(move |_| {
                    let removed = {
                        let mut entries = queued_entries_for_cancel.borrow_mut();
                        entries
                            .iter()
                            .position(|entry| entry.id == queued_id)
                            .and_then(|index| entries.remove(index))
                    };
                    let Some(removed) = removed else {
                        return;
                    };
                    if removed.row.parent().is_some() {
                        queued_box_for_cancel.remove(&removed.row);
                    }
                    if let Some(remote_prompt_id) = removed.payload.borrow().remote_prompt_id {
                        let _ = db_for_cancel.mark_remote_pending_prompt_consumed(remote_prompt_id);
                    }
                    (refresh_queue_ui_for_cancel)();

                    let buf = input_view_for_cancel.buffer();
                    let start = buf.start_iter();
                    let end = buf.end_iter();
                    let existing = buf.text(&start, &end, true).to_string();
                    let restored = if existing.trim().is_empty() {
                        removed.payload.borrow().text.clone()
                    } else {
                        format!("{}\n\n{}", existing, removed.payload.borrow().text)
                    };
                    buf.set_text(&restored);
                    placeholder_for_cancel.set_visible(restored.trim().is_empty());
                    send_for_cancel.set_sensitive(true);
                    update_input_height(
                        &input_scroll_for_cancel,
                        &input_view_for_cancel,
                        min_height,
                        max_height,
                    );
                    input_view_for_cancel.grab_focus();
                });

                let db_for_steer = db.clone();
                let queued_entries_for_steer = queued_entries.clone();
                let queued_box_for_steer = queued_box.clone();
                let refresh_queue_ui_for_steer = refresh_queue_ui.clone();
                let input_view_for_steer = input_view.clone();
                let input_scroll_for_steer = input_scroll.clone();
                let placeholder_for_steer = placeholder.clone();
                let send_for_steer = send.clone();
                steer_button.connect_clicked(move |_| {
                    let removed = {
                        let mut entries = queued_entries_for_steer.borrow_mut();
                        entries
                            .iter()
                            .position(|entry| entry.id == queued_id)
                            .and_then(|index| entries.remove(index))
                    };
                    let Some(removed) = removed else {
                        return;
                    };
                    if removed.row.parent().is_some() {
                        queued_box_for_steer.remove(&removed.row);
                    }
                    if let Some(remote_prompt_id) = removed.payload.borrow().remote_prompt_id {
                        let _ = db_for_steer.mark_remote_pending_prompt_consumed(remote_prompt_id);
                    }
                    (refresh_queue_ui_for_steer)();

                    let text = removed.payload.borrow().text.clone();
                    let buf = input_view_for_steer.buffer();
                    let start = buf.start_iter();
                    let end = buf.end_iter();
                    let existing = buf.text(&start, &end, true).to_string();
                    let composed = if existing.trim().is_empty() {
                        text
                    } else {
                        format!("{existing}\n\n{text}")
                    };
                    buf.set_text(&composed);
                    placeholder_for_steer.set_visible(composed.trim().is_empty());
                    update_input_height(
                        &input_scroll_for_steer,
                        &input_view_for_steer,
                        min_height,
                        max_height,
                    );
                    send_for_steer.emit_clicked();
                });
            }

            gtk::glib::ControlFlow::Continue
        });
    }

    {
        let queued_entries = queued_entries.clone();
        let queued_box = queued_box.clone();
        let queued_merge_button = queued_merge_button.clone();
        let refresh_queue_ui = refresh_queue_ui.clone();
        let db = db.clone();
        queued_merge_button.connect_clicked(move |_| {
            let (
                rows_to_remove,
                remote_ids_to_consume,
                merged_label,
                merged_text,
                merged_mentions,
                merged_images,
            ) = {
                let mut entries = queued_entries.borrow_mut();
                if entries.len() < 2 {
                    return;
                }
                let Some(first) = entries.front().cloned() else {
                    return;
                };

                let mut merged_text = String::new();
                let mut merged_mentions: Vec<(String, String)> = Vec::new();
                let mut merged_images: Vec<String> = Vec::new();
                let mut rows_to_remove: Vec<gtk::Box> = Vec::new();
                let mut remote_ids_to_consume: Vec<i64> = Vec::new();

                for (idx, entry) in entries.iter().enumerate() {
                    let payload = entry.payload.borrow();
                    if !payload.text.trim().is_empty() {
                        if !merged_text.is_empty() {
                            merged_text.push_str("\n\n");
                        }
                        merged_text.push_str(payload.text.trim_end());
                    }
                    for mention in &payload.mentions {
                        if !merged_mentions.iter().any(|existing| existing == mention) {
                            merged_mentions.push(mention.clone());
                        }
                    }
                    for image in &payload.images {
                        if !merged_images.iter().any(|existing| existing == image) {
                            merged_images.push(image.clone());
                        }
                    }
                    if idx > 0 {
                        rows_to_remove.push(entry.row.clone());
                        if let Some(remote_prompt_id) = payload.remote_prompt_id {
                            remote_ids_to_consume.push(remote_prompt_id);
                        }
                    }
                }

                entries.truncate(1);
                (
                    rows_to_remove,
                    remote_ids_to_consume,
                    first.preview_label.clone(),
                    merged_text,
                    merged_mentions,
                    merged_images,
                )
            };

            for row in rows_to_remove {
                if row.parent().is_some() {
                    queued_box.remove(&row);
                }
            }
            for remote_prompt_id in remote_ids_to_consume {
                let _ = db.mark_remote_pending_prompt_consumed(remote_prompt_id);
            }

            if let Some(first) = queued_entries.borrow().front().cloned() {
                let mut payload = first.payload.borrow_mut();
                payload.text = merged_text;
                payload.mentions = merged_mentions;
                payload.images = merged_images;
                payload.summary = send_payload_summary_from_paths(&payload.text, &payload.images);
                merged_label.set_text(&payload.summary);
            }
            (refresh_queue_ui)();
        });
    }

    {
        let queued_entries = queued_entries.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        let queued_dispatch_state = queued_dispatch_state.clone();
        let send = send.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(180), move || {
            if send.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            {
                let mut state = queued_dispatch_state.borrow_mut();
                if let Some((thread_id, started_micros, seen_active)) = state.as_mut() {
                    if super::codex_runtime::active_turn_for_thread(thread_id).is_some() {
                        *seen_active = true;
                        return gtk::glib::ControlFlow::Continue;
                    }

                    let elapsed_micros = gtk::glib::monotonic_time() - *started_micros;
                    if *seen_active || elapsed_micros > 5_000_000 {
                        state.take();
                    } else {
                        return gtk::glib::ControlFlow::Continue;
                    }
                }
            }

            let Some(front) = queued_entries.borrow().front().cloned() else {
                return gtk::glib::ControlFlow::Continue;
            };
            let thread_id = front
                .payload
                .borrow()
                .expected_thread_id
                .clone()
                .or_else(|| active_codex_thread_id.borrow().clone());
            let Some(thread_id) = thread_id else {
                return gtk::glib::ControlFlow::Continue;
            };
            if super::codex_runtime::active_turn_for_thread(&thread_id).is_none() {
                queued_dispatch_state
                    .borrow_mut()
                    .replace((thread_id, gtk::glib::monotonic_time(), false));
                front.steer_button.emit_clicked();
            }
            gtk::glib::ControlFlow::Continue
        });
    }
}
