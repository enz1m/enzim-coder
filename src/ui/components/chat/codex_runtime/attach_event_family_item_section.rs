{
    match method {
                "item/tool/call" => {
                    let Some(request_id) = event.request_id else {
                        continue;
                    };
                    let turn_id = super::codex_events::extract_turn_id(&event.params)
                        .or_else(|| active_turn.borrow().clone());
                    let resolved_thread_id = thread_id
                        .or_else(|| {
                            turn_id
                                .as_ref()
                                .and_then(|id| turn_threads.borrow().get(id).cloned())
                        })
                        .or_else(|| active_turn_thread.borrow().clone())
                        .or_else(|| active_thread_id.clone());
                    let should_render_active = super::codex_events::should_render_for_active(
                        resolved_thread_id.as_deref(),
                        active_thread_id.as_deref(),
                    );
                    if !should_render_active {
                        // Multiple runtime listeners can receive the same server request event.
                        // Inactive listeners must not auto-respond, or they can race the active UI
                        // and clear the request before the user answers.
                        continue;
                    }

                    let Some(turn_id) = turn_id else {
                        if let Some(client) = event_client.as_ref() {
                            let _ = client.respond_to_server_request(
                                request_id,
                                build_tool_call_failure_payload(
                                    "Tool call is missing turn context",
                                ),
                            );
                        }
                        continue;
                    };
                    let Some(resolved_thread_id) = resolved_thread_id else {
                        continue;
                    };
                    let Some(client) = event_client.as_ref() else {
                        continue;
                    };
                    show_pending_request_card(
                        client,
                        &turn_uis,
                        &pending_server_requests_by_id,
                        &messages_box,
                        &messages_scroll,
                        &conversation_stack,
                        &resolved_thread_id,
                        &turn_id,
                        request_id,
                        event.method.as_str(),
                        &event.params,
                        should_render_active,
                    );
                    pending_request_thread_by_id
                        .borrow_mut()
                        .insert(request_id, resolved_thread_id.clone());
                    if should_render_active {
                        persist_pending_request_entry(
                            &db,
                            &cached_pending_requests_for_thread,
                            &resolved_thread_id,
                            request_id,
                            &turn_id,
                            event.method.as_str(),
                            &event.params,
                        );
                    }
                }
                "item/commandExecution/requestApproval"
                | "item/fileChange/requestApproval"
                | "item/tool/requestUserInput" => {
                    let Some(request_id) = event.request_id else {
                        continue;
                    };
                    let turn_id = super::codex_events::extract_turn_id(&event.params)
                        .or_else(|| active_turn.borrow().clone());
                    let resolved_thread_id = thread_id
                        .or_else(|| {
                            turn_id
                                .as_ref()
                                .and_then(|id| turn_threads.borrow().get(id).cloned())
                        })
                        .or_else(|| active_turn_thread.borrow().clone())
                        .or_else(|| active_thread_id.clone());
                    let should_render_active = super::codex_events::should_render_for_active(
                        resolved_thread_id.as_deref(),
                        active_thread_id.as_deref(),
                    );
                    if !should_render_active {
                        // Multiple runtime listeners can receive the same server request event.
                        // Inactive listeners must not auto-respond, or they can race the active UI
                        // and clear the request before the user answers.
                        continue;
                    }

                    let Some(turn_id) = turn_id else {
                        continue;
                    };
                    let Some(resolved_thread_id) = resolved_thread_id else {
                        continue;
                    };
                    let Some(client) = event_client.as_ref() else {
                        continue;
                    };
                    show_pending_request_card(
                        client,
                        &turn_uis,
                        &pending_server_requests_by_id,
                        &messages_box,
                        &messages_scroll,
                        &conversation_stack,
                        &resolved_thread_id,
                        &turn_id,
                        request_id,
                        event.method.as_str(),
                        &event.params,
                        should_render_active,
                    );
                    pending_request_thread_by_id
                        .borrow_mut()
                        .insert(request_id, resolved_thread_id.clone());
                    if should_render_active {
                        persist_pending_request_entry(
                            &db,
                            &cached_pending_requests_for_thread,
                            &resolved_thread_id,
                            request_id,
                            &turn_id,
                            event.method.as_str(),
                            &event.params,
                        );
                    }
                }
                "item/started" => {
                    let Some(item) = event.params.get("item") else {
                        continue;
                    };
                    let Some(item_id) = super::codex_events::extract_item_id(&event.params) else {
                        continue;
                    };
                    let item_kind = item
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string();
                    let resolved_thread_id = thread_id
                        .or_else(|| active_turn_thread.borrow().clone())
                        .or_else(|| active_thread_id.clone());
                    let turn_id = super::codex_events::extract_turn_id(&event.params)
                        .or_else(|| active_turn.borrow().clone());

                    if let Some(thread_id) = resolved_thread_id.clone() {
                        item_threads.borrow_mut().insert(item_id.clone(), thread_id);
                    }
                    if let Some(turn_id) = turn_id.clone() {
                        item_turns.borrow_mut().insert(item_id.clone(), turn_id);
                    }
                    item_kinds
                        .borrow_mut()
                        .insert(item_id.clone(), item_kind.clone());

                    if item_kind == "fileChange" {
                        if let Some(thread_id) = resolved_thread_id.as_deref() {
                            if let Some(preimages) =
                                crate::restore::capture_preimages_for_item(&db, thread_id, item)
                            {
                                staged_preimages_by_item
                                    .borrow_mut()
                                    .insert(item_id.clone(), preimages);
                            }
                        }
                    }

                    let should_render_active = super::codex_events::should_render_for_active(
                        resolved_thread_id.as_deref(),
                        active_thread_id.as_deref(),
                    );
                    if !should_render_active {
                        continue;
                    }
                    let Some(turn_id) = turn_id else {
                        continue;
                    };

                    let mut turns = turn_uis.borrow_mut();
                    let turn_ui = turns.entry(turn_id).or_insert_with(|| {
                        super::create_turn_ui(&messages_box, &messages_scroll, &conversation_stack)
                    });
                    turn_ui.in_progress = true;

                    turn_ui
                        .pending_items
                        .insert(item_id.clone(), item_kind.clone());

                    match item_kind.as_str() {
                        "agentMessage" => {
                            turn_ui.agent_message_item_ids.insert(item_id.clone());
                            if !turn_ui.text_widgets.contains_key(&item_id) {
                                let label =
                                    super::message_render::create_text_segment_revealed(&turn_ui.body_box);
                                turn_ui.text_widgets.insert(item_id.clone(), label);
                                turn_ui.text_buffers.insert(item_id.clone(), String::new());
                                turn_ui
                                    .text_pending_deltas
                                    .insert(item_id.clone(), String::new());
                            }
                        }
                        "commandExecution" => {
                            let command = item
                                .get("command")
                                .and_then(Value::as_str)
                                .unwrap_or("command");
                            let command_paths = extract_write_paths_from_command(command);
                            if !command_paths.is_empty() {
                                staged_command_paths_by_item
                                    .borrow_mut()
                                    .insert(item_id.clone(), command_paths.clone());
                                if let Some(thread_id) = resolved_thread_id.as_deref() {
                                    let synthetic_item = json!({
                                        "changes": command_paths
                                            .iter()
                                            .map(|path| json!({ "path": path }))
                                            .collect::<Vec<Value>>()
                                    });
                                    if let Some(preimages) =
                                        crate::restore::capture_preimages_for_item(
                                            &db,
                                            thread_id,
                                            &synthetic_item,
                                        )
                                    {
                                        staged_command_preimages_by_item
                                            .borrow_mut()
                                            .insert(item_id.clone(), preimages);
                                    }
                                }
                            }
                            if !turn_ui.command_widgets.contains_key(&item_id) {
                                let (widget, command_ui) =
                                    super::message_render::create_command_widget(command);
                                super::message_render::append_action_widget_with_reveal(
                                    &turn_ui.body_box,
                                    "commandExecution",
                                    &widget,
                                );
                                turn_ui.command_widgets.insert(item_id.clone(), command_ui);
                            }
                        }
                        "fileChange" => {}
                        "dynamicToolCall" => {
                            let (tool_name, arguments, _, _) =
                                super::codex_events::extract_dynamic_tool_call_fields(item);
                            if !turn_ui.tool_call_widgets.contains_key(&item_id) {
                                let (widget, tool_ui) =
                                    super::message_render::create_tool_call_widget(
                                        &tool_name, &arguments,
                                    );
                                super::message_render::append_action_widget_with_reveal(
                                    &turn_ui.body_box,
                                    "dynamicToolCall",
                                    &widget,
                                );
                                turn_ui.tool_call_widgets.insert(item_id.clone(), tool_ui);
                            }
                        }
                        kind if is_generic_item_kind(kind) => {
                            let (section, mut title, summary, _, _) =
                                super::codex_events::extract_generic_item_fields(item);
                            if kind == "contextCompaction" {
                                title = "Context compaction".to_string();
                            }
                            if !turn_ui.generic_item_widgets.contains_key(&item_id) {
                                let (widget, generic_ui) =
                                    super::message_render::create_generic_item_widget(
                                        &section, &title, &summary,
                                    );
                                super::message_render::append_action_widget_with_reveal(
                                    &turn_ui.body_box,
                                    kind,
                                    &widget,
                                );
                                turn_ui
                                    .generic_item_widgets
                                    .insert(item_id.clone(), generic_ui);
                            }
                            if let Some(generic_ui) = turn_ui.generic_item_widgets.get_mut(&item_id) {
                                generic_ui.set_title(&title);
                                generic_ui.set_details(&summary, "");
                                generic_ui.set_running(true);
                            }
                        }
                        _ => {}
                    }
                    super::refresh_turn_status(turn_ui);
                    if should_render_active {
                        super::message_render::scroll_to_bottom(&messages_scroll);
                    }
                }
                "item/agentMessage/delta"
                | "item/plan/delta"
                | "item/reasoning/summaryTextDelta"
                | "item/reasoning/summaryPartAdded"
                | "item/reasoning/textDelta"
                | "item/commandExecution/outputDelta"
                | "item/fileChange/outputDelta"
                | "item/dynamicToolCall/outputDelta"
                | "item/dynamicToolCall/textDelta"
                | "item/webSearch/outputDelta"
                | "item/webSearch/textDelta"
                | "item/mcpToolCall/outputDelta"
                | "item/mcpToolCall/textDelta"
                | "item/collabToolCall/outputDelta"
                | "item/collabToolCall/textDelta"
                | "item/imageView/outputDelta"
                | "item/imageView/textDelta"
                | "item/contextCompaction/outputDelta"
                | "item/contextCompaction/textDelta" => {
                    let Some(item_id) = super::codex_events::extract_item_id(&event.params) else {
                        continue;
                    };
                    let delta = if event.method == "item/reasoning/summaryPartAdded" {
                        "\n".to_string()
                    } else {
                        let Some(delta) = super::codex_events::extract_delta_text(&event.params)
                        else {
                            continue;
                        };
                        delta
                    };

                    let turn_id = super::codex_events::extract_turn_id(&event.params)
                        .or_else(|| item_turns.borrow().get(&item_id).cloned())
                        .or_else(|| active_turn.borrow().clone());
                    let resolved_thread_id = thread_id
                        .or_else(|| {
                            turn_id
                                .as_ref()
                                .and_then(|id| turn_threads.borrow().get(id).cloned())
                        })
                        .or_else(|| item_threads.borrow().get(&item_id).cloned())
                        .or_else(|| active_turn_thread.borrow().clone())
                        .or_else(|| active_thread_id.clone());
                    let should_render_active = super::codex_events::should_render_for_active(
                        resolved_thread_id.as_deref(),
                        active_thread_id.as_deref(),
                    );
                    if !should_render_active {
                        continue;
                    }
                    let Some(turn_id) = turn_id else {
                        continue;
                    };

                    let mut turns = turn_uis.borrow_mut();
                    let turn_ui = turns.entry(turn_id).or_insert_with(|| {
                        super::create_turn_ui(&messages_box, &messages_scroll, &conversation_stack)
                    });
                    turn_ui.in_progress = true;

                    let item_kind = item_kinds
                        .borrow()
                        .get(&item_id)
                        .cloned()
                        .or_else(|| {
                            super::codex_events::item_kind_for_delta_method(event.method.as_str())
                                .map(|s| s.to_string())
                        })
                        .unwrap_or_else(|| "unknown".to_string());

                    match item_kind.as_str() {
                        "agentMessage" => {
                            turn_ui.agent_message_item_ids.insert(item_id.clone());
                            if !turn_ui.text_widgets.contains_key(&item_id) {
                                let label =
                                    super::message_render::create_text_segment_revealed(&turn_ui.body_box);
                                turn_ui.text_widgets.insert(item_id.clone(), label);
                                turn_ui.text_buffers.insert(item_id.clone(), String::new());
                                turn_ui
                                    .text_pending_deltas
                                    .insert(item_id.clone(), String::new());
                            }
                            let pending = turn_ui
                                .text_pending_deltas
                                .entry(item_id.clone())
                                .or_default();
                            pending.push_str(&delta);

                            let flush_at = pending
                                .rfind('\n')
                                .map(|last_newline| last_newline + 1)
                                .or_else(|| {
                                    const STREAM_FLUSH_TARGET_CHARS: usize = 120;
                                    const STREAM_FLUSH_MIN_BOUNDARY_CHARS: usize = 48;

                                    if pending.chars().count() < STREAM_FLUSH_TARGET_CHARS {
                                        return None;
                                    }

                                    let mut candidate = None;
                                    for (idx, ch) in pending.char_indices() {
                                        let next = idx + ch.len_utf8();
                                        if next < STREAM_FLUSH_MIN_BOUNDARY_CHARS {
                                            continue;
                                        }
                                        if ch.is_whitespace()
                                            || matches!(
                                                ch,
                                                ',' | '.' | ';' | ':' | '!' | '?' | ')' | ']' | '}'
                                            )
                                        {
                                            candidate = Some(next);
                                        }
                                    }
                                    candidate
                                });

                            if let Some(flush_at) = flush_at {
                                let flush_chunk = pending[..flush_at].to_string();
                                pending.drain(..flush_at);
                                let buffer = turn_ui.text_buffers.entry(item_id.clone()).or_default();
                                buffer.push_str(&flush_chunk);
                                if let Some(label) = turn_ui.text_widgets.get(&item_id) {
                                    super::markdown::set_markdown(label, buffer);
                                }
                            }
                        }
                        "commandExecution" => {
                            if !turn_ui.command_widgets.contains_key(&item_id) {
                                let (widget, command_ui) =
                                    super::message_render::create_command_widget("command");
                                super::message_render::append_action_widget_with_reveal(
                                    &turn_ui.body_box,
                                    "commandExecution",
                                    &widget,
                                );
                                turn_ui.command_widgets.insert(item_id.clone(), command_ui);
                            }
                            if let Some(command_ui) = turn_ui.command_widgets.get_mut(&item_id) {
                                command_ui.output_text.borrow_mut().push_str(&delta);
                                let full_output = command_ui.output_text.borrow().clone();
                                command_ui.set_command_output(&full_output);
                            }
                        }
                        "fileChange" => {}
                        "dynamicToolCall" => {
                            if !turn_ui.tool_call_widgets.contains_key(&item_id) {
                                let (widget, tool_ui) =
                                    super::message_render::create_tool_call_widget("tool", "{}");
                                super::message_render::append_action_widget_with_reveal(
                                    &turn_ui.body_box,
                                    "dynamicToolCall",
                                    &widget,
                                );
                                turn_ui.tool_call_widgets.insert(item_id.clone(), tool_ui);
                            }
                            if let Some(tool_ui) = turn_ui.tool_call_widgets.get_mut(&item_id) {
                                tool_ui.append_output_delta(&delta);
                                tool_ui.status_label.set_text("Running...");
                            }
                        }
                        kind if is_generic_item_kind(kind) => {
                            if !turn_ui.generic_item_widgets.contains_key(&item_id) {
                                let (widget, generic_ui) =
                                    super::message_render::create_generic_item_widget(
                                        "Tool", kind, "",
                                    );
                                super::message_render::append_action_widget_with_reveal(
                                    &turn_ui.body_box,
                                    kind,
                                    &widget,
                                );
                                turn_ui
                                    .generic_item_widgets
                                    .insert(item_id.clone(), generic_ui);
                            }
                            if let Some(generic_ui) = turn_ui.generic_item_widgets.get_mut(&item_id)
                            {
                                let current = generic_ui.output_text();
                                let mut merged = current;
                                merged.push_str(&delta);
                                let summary = generic_ui.summary_label.text().to_string();
                                generic_ui.set_details(&summary, &merged);
                                generic_ui.set_running(true);
                            }
                        }
                        "reasoning" => {
                            let status_buffer = turn_ui.status_buffers.entry(item_id.clone()).or_default();
                            status_buffer.push_str(&delta);
                            let latest_line = status_buffer
                                .lines()
                                .rev()
                                .find(|line| !line.trim().is_empty())
                                .map(str::trim)
                                .unwrap_or("");
                            let status_text = if latest_line.is_empty() {
                                "Thinking...".to_string()
                            } else {
                                super::codex_events::sanitize_stream_status_text(latest_line)
                            };
                            let status_text = if status_text.trim().is_empty() {
                                "Thinking...".to_string()
                            } else {
                                status_text
                            };
                            let now_micros = gtk::glib::monotonic_time();
                            let can_replace = turn_ui.status_last_text.is_empty()
                                || turn_ui.status_last_text == status_text
                                || (now_micros - turn_ui.status_last_updated_micros) >= 500_000;
                            if can_replace {
                                turn_ui.status_row.set_visible(true);
                                turn_ui.status_label.set_text(&status_text);
                                turn_ui.status_last_text = status_text;
                                turn_ui.status_last_updated_micros = now_micros;
                            }
                        }
                        "plan" => {
                            let status_buffer = turn_ui.status_buffers.entry(item_id.clone()).or_default();
                            status_buffer.push_str(&delta);
                            let latest_line = status_buffer
                                .lines()
                                .rev()
                                .find(|line| !line.trim().is_empty())
                                .map(str::trim)
                                .unwrap_or("");
                            let status_text = if latest_line.is_empty() {
                                "Thinking...".to_string()
                            } else {
                                super::codex_events::sanitize_stream_status_text(latest_line)
                            };
                            let status_text = if status_text.trim().is_empty() {
                                "Thinking...".to_string()
                            } else {
                                status_text
                            };
                            let now_micros = gtk::glib::monotonic_time();
                            let can_replace = turn_ui.status_last_text.is_empty()
                                || turn_ui.status_last_text == status_text
                                || (now_micros - turn_ui.status_last_updated_micros) >= 500_000;
                            if can_replace {
                                turn_ui.status_row.set_visible(true);
                                turn_ui.status_label.set_text(&status_text);
                                turn_ui.status_last_text = status_text;
                                turn_ui.status_last_updated_micros = now_micros;
                            }
                        }
                        _ => {}
                    }
                    if should_render_active {
                        super::message_render::scroll_to_bottom(&messages_scroll);
                    }
                }
                "item/completed" => {
                    let Some(item) = event.params.get("item") else {
                        continue;
                    };
                    let Some(item_id) = super::codex_events::extract_item_id(&event.params) else {
                        continue;
                    };

                    let turn_id = super::codex_events::extract_turn_id(&event.params)
                        .or_else(|| item_turns.borrow().get(&item_id).cloned())
                        .or_else(|| active_turn.borrow().clone());
                    let resolved_thread_id = thread_id
                        .or_else(|| {
                            turn_id
                                .as_ref()
                                .and_then(|id| turn_threads.borrow().get(id).cloned())
                        })
                        .or_else(|| item_threads.borrow().get(&item_id).cloned())
                        .or_else(|| active_turn_thread.borrow().clone())
                        .or_else(|| active_thread_id.clone());
                    let should_render_active = super::codex_events::should_render_for_active(
                        resolved_thread_id.as_deref(),
                        active_thread_id.as_deref(),
                    );
                    if !should_render_active {
                        continue;
                    }
                    let Some(turn_id) = turn_id else {
                        continue;
                    };

                    let kind = item
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown")
                        .to_string();

                    let mut turns = turn_uis.borrow_mut();
                    let turn_ui = turns.entry(turn_id.clone()).or_insert_with(|| {
                        super::create_turn_ui(&messages_box, &messages_scroll, &conversation_stack)
                    });

                    match kind.as_str() {
                        "agentMessage" => {
                            turn_ui.agent_message_item_ids.insert(item_id.clone());
                            let completed_text = super::codex_events::extract_agent_message_text(item)
                                .or_else(|| {
                                    item.get("text")
                                        .and_then(Value::as_str)
                                        .map(|s| s.to_string())
                                })
                                .unwrap_or_default();
                            let streamed_prefix =
                                turn_ui.text_buffers.get(&item_id).cloned().unwrap_or_default();
                            let streamed_suffix = turn_ui
                                .text_pending_deltas
                                .remove(&item_id)
                                .unwrap_or_default();
                            let streamed_text = format!("{streamed_prefix}{streamed_suffix}");
                            let final_text = if streamed_text.trim().is_empty() {
                                completed_text
                            } else if completed_text.trim().is_empty() {
                                streamed_text
                            } else if completed_text.len() >= streamed_text.len() {
                                completed_text
                            } else {
                                streamed_text
                            };
                            if !turn_ui.text_widgets.contains_key(&item_id) {
                                let label =
                                    super::message_render::create_text_segment_revealed(&turn_ui.body_box);
                                turn_ui.text_widgets.insert(item_id.clone(), label);
                            }
                            if let Some(label) = turn_ui.text_widgets.get(&item_id) {
                                super::markdown::set_markdown(label, &final_text);
                            }
                            turn_ui
                                .text_buffers
                                .insert(item_id.clone(), final_text.clone());
                        }
                        "commandExecution" => {
                            let command = item
                                .get("command")
                                .and_then(Value::as_str)
                                .unwrap_or("command");
                            let status = item
                                .get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("completed");
                            let exit_code = item.get("exitCode").and_then(Value::as_i64);
                            let duration_ms = item.get("durationMs").and_then(Value::as_i64);
                            let command_actions_md =
                                super::codex_events::format_command_actions_markdown(item);
                            if !turn_ui.command_widgets.contains_key(&item_id) {
                                let (widget, command_ui) =
                                    super::message_render::create_command_widget(command);
                                super::message_render::append_action_widget_with_reveal(
                                    &turn_ui.body_box,
                                    "commandExecution",
                                    &widget,
                                );
                                turn_ui.command_widgets.insert(item_id.clone(), command_ui);
                            }
                            if let Some(command_ui) = turn_ui.command_widgets.get_mut(&item_id) {
                                command_ui.set_command_headline(command);
                                command_ui.set_command_status_label(
                                    &super::codex_events::format_command_status_label(
                                        status,
                                        exit_code,
                                        duration_ms,
                                    ),
                                );
                                if let Some(output) =
                                    item.get("aggregatedOutput").and_then(Value::as_str)
                                {
                                    let full_output =
                                        if let Some(actions_md) = command_actions_md.as_deref() {
                                            if output.trim().is_empty() {
                                                actions_md.to_string()
                                            } else {
                                                format!("{output}\n\n{actions_md}")
                                            }
                                        } else {
                                            output.to_string()
                                        };
                                    command_ui.set_command_output(&full_output);
                                } else if let Some(actions_md) = command_actions_md.as_deref() {
                                    command_ui.set_command_output(actions_md);
                                }
                                command_ui.revealer.set_reveal_child(false);
                            }

                            if should_render_active {
                                if let Some(thread_id) = resolved_thread_id.as_deref() {
                                let after_text_count = {
                                    turn_ui
                                        .text_buffers
                                        .iter()
                                        .filter(|(buffer_item_id, content)| {
                                            !content.trim().is_empty()
                                                && turn_ui
                                                    .agent_message_item_ids
                                                    .contains(buffer_item_id.as_str())
                                        })
                                        .count()
                                };
                                let output = item
                                    .get("aggregatedOutput")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                let output = if let Some(actions_md) = command_actions_md.as_deref()
                                {
                                    if output.trim().is_empty() {
                                        actions_md.to_string()
                                    } else {
                                        format!("{output}\n\n{actions_md}")
                                    }
                                } else {
                                    output
                                };
                                let entry = json!({
                                    "turnId": turn_id,
                                    "itemId": item_id,
                                    "command": command,
                                    "status": status,
                                    "exitCode": exit_code,
                                    "durationMs": duration_ms,
                                    "output": output,
                                    "writePaths": staged_command_paths_by_item
                                        .borrow_mut()
                                        .remove(&item_id)
                                        .unwrap_or_else(|| extract_write_paths_from_command(command)),
                                    "preimages": staged_command_preimages_by_item
                                        .borrow_mut()
                                        .remove(&item_id)
                                        .unwrap_or(Value::Null),
                                    "afterTextCount": after_text_count,
                                    "afterTextCountMode": "agentMessageOnly"
                                });
                                let mut cached = cached_commands_for_thread.borrow_mut();
                                super::codex_history::upsert_cached_command(&mut cached, entry);
                                super::codex_history::save_cached_commands(&db, thread_id, &cached);
                            }
                            }
                            if should_render_active && is_probably_file_write_command(command) {
                                turns_with_write_like_commands
                                    .borrow_mut()
                                    .insert(turn_id.clone());
                            }
                        }
                        "fileChange" => {
                            if !turn_ui.file_change_widgets.contains_key(&item_id) {
                                let widget = super::message_render::create_file_change_widget(item);
                                super::message_render::append_action_widget_with_reveal(
                                    &turn_ui.body_box,
                                    "fileChange",
                                    &widget,
                                );
                                turn_ui.file_change_widgets.insert(item_id.clone(), widget);
                            }
                            if should_render_active {
                                if let Some(thread_id) = resolved_thread_id.as_deref() {
                                let after_text_count = {
                                    turn_ui
                                        .text_buffers
                                        .iter()
                                        .filter(|(buffer_item_id, content)| {
                                            !content.trim().is_empty()
                                                && turn_ui
                                                    .agent_message_item_ids
                                                    .contains(buffer_item_id.as_str())
                                        })
                                        .count()
                                };
                                let status = item
                                    .get("status")
                                    .and_then(Value::as_str)
                                    .unwrap_or("completed");
                                let changes = item
                                    .get("changes")
                                    .and_then(Value::as_array)
                                    .cloned()
                                    .unwrap_or_default();
                                let preimages = staged_preimages_by_item
                                    .borrow_mut()
                                    .remove(&item_id)
                                    .unwrap_or(Value::Null);
                                let entry = json!({
                                    "turnId": turn_id,
                                    "itemId": item_id,
                                    "status": status,
                                    "changes": changes,
                                    "preimages": preimages,
                                    "afterTextCount": after_text_count,
                                    "afterTextCountMode": "agentMessageOnly"
                                });
                                let mut cached = cached_file_changes_for_thread.borrow_mut();
                                super::codex_history::upsert_cached_file_change(&mut cached, entry);
                                super::codex_history::save_cached_file_changes(
                                    &db, thread_id, &cached,
                                );
                            }
                            }
                        }
                        "dynamicToolCall" => {
                            let (tool_name, arguments, status, output) =
                                super::codex_events::extract_dynamic_tool_call_fields(item);
                            if !turn_ui.tool_call_widgets.contains_key(&item_id) {
                                let (widget, tool_ui) =
                                    super::message_render::create_tool_call_widget(
                                        &tool_name, &arguments,
                                    );
                                super::message_render::append_action_widget_with_reveal(
                                    &turn_ui.body_box,
                                    "dynamicToolCall",
                                    &widget,
                                );
                                turn_ui.tool_call_widgets.insert(item_id.clone(), tool_ui);
                            }
                            if let Some(tool_ui) = turn_ui.tool_call_widgets.get_mut(&item_id) {
                                tool_ui.tool_label.set_text(&tool_name);
                                super::message_render::set_plain_label_text(
                                    &tool_ui.args_label,
                                    &arguments,
                                );
                                tool_ui.status_label.set_text(if status == "failed" {
                                    "Failed"
                                } else {
                                    "Completed"
                                });
                                tool_ui.set_output(&output);
                            }
                            if should_render_active {
                                if let Some(thread_id) = resolved_thread_id.as_deref() {
                                let after_text_count = {
                                    turn_ui
                                        .text_buffers
                                        .iter()
                                        .filter(|(buffer_item_id, content)| {
                                            !content.trim().is_empty()
                                                && turn_ui
                                                    .agent_message_item_ids
                                                    .contains(buffer_item_id.as_str())
                                        })
                                        .count()
                                };
                                let entry = json!({
                                    "turnId": turn_id,
                                    "itemId": item_id,
                                    "type": "dynamicToolCall",
                                    "toolName": tool_name,
                                    "arguments": arguments,
                                    "status": status,
                                    "output": output,
                                    "afterTextCount": after_text_count,
                                    "afterTextCountMode": "agentMessageOnly"
                                });
                                let mut cached = cached_tool_items_for_thread.borrow_mut();
                                super::codex_history::upsert_cached_tool_item(&mut cached, entry);
                                super::codex_history::save_cached_tool_items(
                                    &db, thread_id, &cached,
                                );
                            }
                            }
                        }
                        kind if is_generic_item_kind(kind) => {
                            let (section, title, summary, status, output) =
                                super::codex_events::extract_generic_item_fields(item);
                            if !turn_ui.generic_item_widgets.contains_key(&item_id) {
                                let (widget, generic_ui) =
                                    super::message_render::create_generic_item_widget(
                                        &section, &title, &summary,
                                    );
                                super::message_render::append_action_widget_with_reveal(
                                    &turn_ui.body_box,
                                    kind,
                                    &widget,
                                );
                                turn_ui
                                    .generic_item_widgets
                                    .insert(item_id.clone(), generic_ui);
                            }
                            if let Some(generic_ui) = turn_ui.generic_item_widgets.get_mut(&item_id)
                            {
                                generic_ui.set_title(&title);
                                generic_ui.set_details(&summary, &output);
                                generic_ui.set_running(status == "running");
                            }
                            if should_render_active {
                                if let Some(thread_id) = resolved_thread_id.as_deref() {
                                let after_text_count = {
                                    turn_ui
                                        .text_buffers
                                        .iter()
                                        .filter(|(buffer_item_id, content)| {
                                            !content.trim().is_empty()
                                                && turn_ui
                                                    .agent_message_item_ids
                                                    .contains(buffer_item_id.as_str())
                                        })
                                        .count()
                                };
                                let entry = json!({
                                    "turnId": turn_id,
                                    "itemId": item_id,
                                    "type": kind,
                                    "title": title,
                                    "summary": summary,
                                    "status": status,
                                    "output": output,
                                    "afterTextCount": after_text_count,
                                    "afterTextCountMode": "agentMessageOnly"
                                });
                                let mut cached = cached_tool_items_for_thread.borrow_mut();
                                super::codex_history::upsert_cached_tool_item(&mut cached, entry);
                                super::codex_history::save_cached_tool_items(
                                    &db, thread_id, &cached,
                                );
                            }
                            }
                        }
                        _ => {}
                    }
                    turn_ui.status_buffers.remove(&item_id);
                    turn_ui.pending_items.remove(&item_id);
                    super::refresh_turn_status(turn_ui);
                    item_turns.borrow_mut().remove(&item_id);
                    item_kinds.borrow_mut().remove(&item_id);
                    if should_render_active {
                        super::message_render::scroll_to_bottom(&messages_scroll);
                    }
                }
        _ => {}
    }
}
