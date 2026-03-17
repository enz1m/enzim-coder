{
    match method {
                "turn/started" => {
                    if let Some(turn_id) = super::codex_events::extract_turn_id(&event.params) {
                        let resolved_thread = thread_id.or_else(|| active_thread_id.clone());
                        if let Some(thread_id) = resolved_thread.clone() {
                            clear_history_unavailable_for_thread(&thread_id);
                            turn_threads.borrow_mut().insert(turn_id.clone(), thread_id);
                        }
                        if let Some(thread_id) = resolved_thread.as_deref() {
                            set_active_turn_for_thread(thread_id, &turn_id);
                        }
                        active_turn.replace(Some(turn_id.clone()));
                        active_turn_thread.replace(resolved_thread.clone());

                        if super::codex_events::should_render_for_active(
                            resolved_thread.as_deref(),
                            active_thread_id.as_deref(),
                        ) {
                            let mut turns = turn_uis.borrow_mut();
                            let turn_ui = turns.entry(turn_id).or_insert_with(|| {
                                super::create_turn_ui(
                                    &messages_box,
                                    &messages_scroll,
                                    &conversation_stack,
                                )
                            });
                            turn_ui.in_progress = true;
                            turn_ui.runtime_status_text = None;
                            super::refresh_turn_status(turn_ui);
                            super::message_render::scroll_to_bottom(&messages_scroll);
                        }
                    }
                }
                "turn/status" => {
                    let Some(turn_id) = super::codex_events::extract_turn_id(&event.params) else {
                        continue;
                    };
                    let status_text = event
                        .params
                        .get("statusText")
                        .and_then(Value::as_str)
                        .or_else(|| {
                            event.params
                                .get("status")
                                .and_then(|status| status.get("message"))
                                .and_then(Value::as_str)
                        })
                        .map(super::codex_events::sanitize_stream_status_text)
                        .unwrap_or_default();
                    if status_text.trim().is_empty() {
                        continue;
                    }

                    let resolved_thread_id = thread_id
                        .or_else(|| turn_threads.borrow().get(&turn_id).cloned())
                        .or_else(|| active_turn_thread.borrow().clone())
                        .or_else(|| active_thread_id.clone());
                    let should_render_active = super::codex_events::should_render_for_active(
                        resolved_thread_id.as_deref(),
                        active_thread_id.as_deref(),
                    );
                    if !should_render_active {
                        continue;
                    }

                    let mut turns = turn_uis.borrow_mut();
                    let turn_ui = turns.entry(turn_id).or_insert_with(|| {
                        super::create_turn_ui(&messages_box, &messages_scroll, &conversation_stack)
                    });
                    turn_ui.in_progress = true;
                    turn_ui.runtime_status_text = Some(status_text);
                    super::refresh_turn_status(turn_ui);
                    super::message_render::scroll_to_bottom(&messages_scroll);
                }
                "turn/plan/updated" => {
                    let Some(turn_id) = super::codex_events::extract_turn_id(&event.params) else {
                        continue;
                    };
                    let Some(plan_text) = super::codex_events::format_plan_update(&event.params)
                    else {
                        continue;
                    };

                    let resolved_thread_id = thread_id
                        .or_else(|| turn_threads.borrow().get(&turn_id).cloned())
                        .or_else(|| active_turn_thread.borrow().clone())
                        .or_else(|| active_thread_id.clone());
                    let should_render_active = super::codex_events::should_render_for_active(
                        resolved_thread_id.as_deref(),
                        active_thread_id.as_deref(),
                    );
                    if !should_render_active {
                        continue;
                    }

                    let mut turns = turn_uis.borrow_mut();
                    let turn_ui = turns.entry(turn_id).or_insert_with(|| {
                        super::create_turn_ui(&messages_box, &messages_scroll, &conversation_stack)
                    });
                    turn_ui.in_progress = true;
                    if !plan_text.trim().is_empty() {
                        turn_ui.status_row.set_visible(true);
                        turn_ui.status_label.set_text("Thinking...");
                        if should_render_active {
                            super::message_render::scroll_to_bottom(&messages_scroll);
                        }
                    }
                }
                "turn/completed" => {
                    if let Some(turn_id) = super::codex_events::extract_turn_id(&event.params) {
                        let resolved_thread_id = thread_id
                            .or_else(|| turn_threads.borrow().get(&turn_id).cloned())
                            .or_else(|| active_turn_thread.borrow().clone())
                            .or_else(|| active_thread_id.clone());
                        let is_active_thread = super::codex_events::should_render_for_active(
                            resolved_thread_id.as_deref(),
                            active_thread_id.as_deref(),
                        );
                        turn_threads.borrow_mut().remove(&turn_id);
                        if let Some(thread_id) = resolved_thread_id.as_deref() {
                            clear_active_turn_for_thread(thread_id, Some(turn_id.as_str()));
                            clear_history_unavailable_for_thread(thread_id);
                            if track_background_completion {
                                if is_active_thread {
                                    super::clear_thread_completed_unseen(thread_id);
                                } else {
                                    super::mark_thread_completed_unseen(thread_id);
                                }
                            }
                        }
                        let mut remote_assistant_text = String::new();
                        let mut remote_command_count = 0usize;
                        let mut remote_file_edit_count = 0usize;
                        let mut remote_other_action_count = 0usize;
                        if let Some(turn_ui) = turn_uis.borrow_mut().get_mut(&turn_id) {
                            turn_ui.in_progress = false;
                            turn_ui.runtime_status_text = None;
                            turn_ui.pending_items.clear();
                            super::message_render::set_active_action_section_wave(
                                &turn_ui.body_box,
                                false,
                            );
                            let completed_at = event
                                .params
                                .get("turn")
                                .and_then(|turn| turn.get("completedAt"))
                                .and_then(|value| {
                                    if let Some(raw) = value.as_i64() {
                                        return Some(raw);
                                    }
                                    value.as_str().and_then(|raw| {
                                        gtk::glib::DateTime::from_iso8601(raw, None)
                                            .ok()
                                            .map(|dt| dt.to_unix())
                                    })
                                })
                                .unwrap_or_else(|| {
                                    std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs() as i64
                                });
                            let completed_time = if completed_at > 1_000_000_000_000 {
                                std::time::UNIX_EPOCH
                                    + std::time::Duration::from_millis(completed_at as u64)
                            } else {
                                std::time::UNIX_EPOCH
                                    + std::time::Duration::from_secs(completed_at as u64)
                            };
                            super::message_render::reveal_assistant_turn_completion_timestamp(
                                &turn_ui.timestamp_label,
                                &turn_ui.timestamp_revealer,
                                completed_time,
                            );
                            super::refresh_turn_status(turn_ui);
                            let status = event
                                .params
                                .get("turn")
                                .and_then(|turn| turn.get("status"))
                                .and_then(Value::as_str);
                            let failed_message =
                                super::codex_events::turn_failed_message(&event.params)
                                    .or_else(|| {
                                        super::codex_events::turn_error_message(&event.params)
                                    })
                                    .or_else(|| {
                                        if status == Some("failed") {
                                            Some("Turn failed.".to_string())
                                        } else {
                                            None
                                        }
                                    });

                            if let Some(message) = failed_message {
                                let message = maybe_replace_profile_auth_error_message(
                                    &db,
                                    &manager,
                                    resolved_thread_id.as_deref(),
                                    &message,
                                );
                                if is_active_thread {
                                    if let Some(thread_id) = resolved_thread_id.as_deref() {
                                    let entry = json!({
                                        "turnId": turn_id,
                                        "message": message
                                    });
                                    let mut cached = cached_turn_errors_for_thread.borrow_mut();
                                    super::history::upsert_cached_turn_error(
                                        &mut cached,
                                        entry,
                                    );
                                    super::history::save_cached_turn_errors_async(
                                        thread_id, &cached,
                                    );
                                }
                                }

                                let has_text = turn_ui
                                    .text_buffers
                                    .values()
                                    .any(|content| !content.trim().is_empty());
                                let has_commands = !turn_ui.command_widgets.is_empty();
                                let has_file_changes = !turn_ui.file_change_widgets.is_empty();
                                let has_tool_calls = !turn_ui.tool_call_widgets.is_empty();
                                let has_generic_items = !turn_ui.generic_item_widgets.is_empty();
                                if !has_text
                                    && !has_commands
                                    && !has_file_changes
                                    && !has_tool_calls
                                    && !has_generic_items
                                {
                                    let widget = super::message_render::create_error_widget(
                                        "Turn failed",
                                        &message,
                                    );
                                    turn_ui.body_box.append(&widget);
                                    turn_ui
                                        .text_buffers
                                        .insert(format!("error:{turn_id}"), message.clone());
                                    turn_ui.body_box.set_visible(true);
                                    turn_ui.bubble.remove_css_class("chat-turn-bubble-initial");
                                }
                            }

                            let mut text_segments = turn_ui
                                .text_buffers
                                .values()
                                .map(|value| value.trim().to_string())
                                .filter(|value| !value.is_empty())
                                .collect::<Vec<_>>();
                            text_segments.sort();
                            remote_assistant_text = text_segments.join("\n\n");
                            remote_command_count = turn_ui.command_widgets.len();
                            remote_file_edit_count = turn_ui.file_change_widgets.len();
                            remote_other_action_count =
                                turn_ui.tool_call_widgets.len()
                                    + turn_ui
                                        .generic_item_widgets
                                        .keys()
                                        .filter(|item_id| {
                                            !turn_ui.reasoning_item_ids.contains((*item_id).as_str())
                                        })
                                        .count();

                            if is_active_thread {
                                super::message_render::scroll_to_bottom(&messages_scroll);
                            }
                        }

                        if let Some(thread_id) = resolved_thread_id.as_deref() {
                            crate::remote::runtime::forward_turn_completion_if_enabled(
                                db.as_ref(),
                                thread_id,
                                &turn_id,
                                &remote_assistant_text,
                                remote_command_count,
                                remote_file_edit_count,
                                remote_other_action_count,
                            );
                        }

                        let request_ids_for_turn: Vec<i64> = pending_server_requests_by_id
                            .borrow()
                            .iter()
                            .filter_map(|(request_id, pending)| {
                                if pending.turn_id == turn_id {
                                    Some(*request_id)
                                } else {
                                    None
                                }
                            })
                            .collect();
                        for request_id in request_ids_for_turn {
                            if let Some(pending) = pending_server_requests_by_id
                                .borrow_mut()
                                .remove(&request_id)
                            {
                                remove_request_card(&turn_uis, &pending.turn_id, &pending.card);
                                if is_active_thread {
                                    remove_persisted_pending_request(
                                        &db,
                                        &cached_pending_requests_for_thread,
                                        &pending.thread_id,
                                        request_id,
                                    );
                                }
                            }
                            pending_request_thread_by_id
                                .borrow_mut()
                                .remove(&request_id);
                        }
                        if is_active_thread {
                            if let Some(thread_id) = resolved_thread_id.as_deref() {
                                let mut cached = cached_pending_requests_for_thread.borrow_mut();
                                if super::history::remove_cached_pending_requests_for_turn(
                                    &mut cached,
                                    &turn_id,
                                ) {
                                    super::history::save_cached_pending_requests_async(
                                        thread_id, &cached,
                                    );
                                }
                            }
                        }

                        if is_active_thread {
                            if let Some(thread_id) = resolved_thread_id.as_deref() {
                                let file_changes: Vec<Value> = cached_file_changes_for_thread
                                    .borrow()
                                    .iter()
                                    .filter(|entry| {
                                        entry.get("turnId").and_then(Value::as_str)
                                            == Some(turn_id.as_str())
                                            && entry
                                                .get("status")
                                                .and_then(Value::as_str)
                                                .map(|status| status == "completed")
                                                .unwrap_or(true)
                                    })
                                    .cloned()
                                    .collect();
                                let command_file_changes: Vec<Value> = cached_commands_for_thread
                                    .borrow()
                                    .iter()
                                    .filter_map(|entry| {
                                        let same_turn = entry
                                            .get("turnId")
                                            .and_then(Value::as_str)
                                            == Some(turn_id.as_str());
                                        let completed = entry
                                            .get("status")
                                            .and_then(Value::as_str)
                                            .map(|status| status == "completed")
                                            .unwrap_or(true);
                                        if !same_turn || !completed {
                                            return None;
                                        }
                                        let paths: Vec<String> = entry
                                            .get("writePaths")
                                            .and_then(Value::as_array)
                                            .map(|items| {
                                                items
                                                    .iter()
                                                    .filter_map(Value::as_str)
                                                    .map(|s| s.to_string())
                                                    .collect()
                                            })
                                            .unwrap_or_default();
                                        if paths.is_empty() {
                                            return None;
                                        }
                                        let changes = paths
                                            .into_iter()
                                            .map(|path| json!({ "path": path }))
                                            .collect::<Vec<Value>>();
                                        let preimages = entry
                                            .get("preimages")
                                            .cloned()
                                            .unwrap_or(Value::Null);
                                        Some(json!({
                                            "turnId": turn_id,
                                            "itemId": entry.get("itemId").cloned().unwrap_or(Value::Null),
                                            "status": "completed",
                                            "changes": changes,
                                            "preimages": preimages
                                        }))
                                    })
                                    .collect();
                                let mut all_file_changes = file_changes;
                                all_file_changes.extend(command_file_changes);
                                let has_write_like_command = turns_with_write_like_commands
                                    .borrow()
                                    .contains(turn_id.as_str());
                                let checkpoint_id = crate::restore::capture_turn_checkpoint(
                                    &db,
                                    thread_id,
                                    &turn_id,
                                    &all_file_changes,
                                );
                                if let Some(checkpoint_id) = checkpoint_id {
                                    let active_thread = active_thread_id.clone();
                                    append_checkpoint_strip_for_turn(
                                        &manager,
                                        &messages_box,
                                        &messages_scroll,
                                        &conversation_stack,
                                        active_thread.as_deref(),
                                        thread_id,
                                        &turn_id,
                                        checkpoint_id,
                                    );
                                }
                                if checkpoint_id.is_none() && has_write_like_command {
                                    let thread_id_for_checkpoint = thread_id.to_string();
                                    let turn_id_for_checkpoint = turn_id.clone();
                                    let checkpoint_tx = checkpoint_tx.clone();
                                    thread::spawn(move || {
                                        let checkpoint_id = crate::data::background_repo::BackgroundRepo::capture_workspace_delta_checkpoint_for_remote_thread(
                                            &thread_id_for_checkpoint,
                                            &turn_id_for_checkpoint,
                                        );
                                        if let Some(checkpoint_id) = checkpoint_id {
                                            let _ = checkpoint_tx.send((
                                                thread_id_for_checkpoint.clone(),
                                                turn_id_for_checkpoint.clone(),
                                                checkpoint_id,
                                            ));
                                            eprintln!(
                                                "[restore] captured command-write checkpoint thread_id={} turn_id={}",
                                                thread_id_for_checkpoint, turn_id_for_checkpoint
                                            );
                                        }
                                    });
                                }
                                let _ = checkpoint_id;

                                let client = manager.resolve_running_client_for_thread_id(thread_id);
                                if client.is_none() {
                                    mark_history_unavailable_for_thread(thread_id);
                                    continue;
                                }
                                let history_tx = history_tx.clone();
                                let thread_id = thread_id.to_string();
                                let log_source =
                                    db.get_thread_record_by_remote_thread_id(&thread_id)
                                        .ok()
                                        .flatten()
                                        .and_then(|thread| {
                                            db.get_codex_profile(thread.profile_id).ok().flatten().map(
                                                |profile| format!("{}#{}", profile.name, profile.id),
                                            )
                                        })
                                        .unwrap_or_else(|| "unknown".to_string());
                                thread::spawn(move || {
                                    let result = client
                                        .map(|client| client.thread_read(&thread_id, true))
                                        .unwrap_or_else(|| {
                                            Err("No runtime available for thread".to_string())
                                        });
                                    let _ = history_tx.send((
                                        thread_id,
                                        log_source,
                                        HistorySyncRenderPolicy::DbOnly,
                                        result,
                                    ));
                                });
                            }
                        }
                        turns_with_write_like_commands
                            .borrow_mut()
                            .remove(turn_id.as_str());

                        if active_turn.borrow().as_deref() == Some(turn_id.as_str()) {
                            active_turn.replace(None);
                            active_turn_thread.replace(None);
                        }
                    }
                }
                "turn/diff/updated" => {
                }
        _ => {}
    }
}
