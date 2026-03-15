{
    match method {
                "error" => {
                    if let Some(message) = super::codex_events::error_event_message(&event.params) {
                        let target_turn_id = super::codex_events::extract_turn_id(&event.params)
                            .or_else(|| active_turn.borrow().clone());
                        if let Some(turn_id) = target_turn_id {
                            let resolved_thread_id = thread_id
                                .or_else(|| turn_threads.borrow().get(&turn_id).cloned())
                                .or_else(|| active_turn_thread.borrow().clone())
                                .or_else(|| active_thread_id.clone());
                            let message = maybe_replace_profile_auth_error_message(
                                &db,
                                &manager,
                                resolved_thread_id.as_deref(),
                                &message,
                            );
                            if let Some(thread_id) = resolved_thread_id.as_deref() {
                                clear_active_turn_for_thread(thread_id, Some(turn_id.as_str()));
                            }

                            if let Some(thread_id) = resolved_thread_id.as_deref() {
                                let entry = json!({
                                    "turnId": turn_id,
                                    "message": message
                                });
                                let mut cached = cached_turn_errors_for_thread.borrow_mut();
                                super::history::upsert_cached_turn_error(&mut cached, entry);
                                super::history::save_cached_turn_errors(
                                    &db, thread_id, &cached,
                                );
                            }

                            if let Some(turn_ui) = turn_uis.borrow_mut().get_mut(&turn_id) {
                                turn_ui.status_row.set_visible(true);
                                turn_ui.status_label.set_text(&message);
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
                                        "Error", &message,
                                    );
                                    turn_ui.body_box.append(&widget);
                                    turn_ui
                                        .text_buffers
                                        .insert(format!("error:{turn_id}"), message.clone());
                                    turn_ui.body_box.set_visible(true);
                                    turn_ui.bubble.remove_css_class("chat-turn-bubble-initial");
                                }
                                super::message_render::scroll_to_bottom(&messages_scroll);
                            }
                        }
                    }
                }
        _ => {}
    }
}
