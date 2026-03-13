fn append_history_assistant_turn(
    messages_box: &gtk::Box,
    conversation_stack: &gtk::Stack,
    items: &[Value],
    cached_commands: &[Value],
    cached_file_changes: &[Value],
    cached_tool_items: &[Value],
    turn_error: Option<&str>,
    timestamp: SystemTime,
) -> bool {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("chat-message-row");
    row.set_halign(gtk::Align::Fill);
    row.set_hexpand(true);

    let bubble = gtk::Box::new(gtk::Orientation::Vertical, 8);
    bubble.add_css_class("chat-assistant-surface");

    let body_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    body_box.add_css_class("chat-command-list");
    bubble.append(&body_box);

    let mut has_content = false;
    let total_text_count = items
        .iter()
        .filter_map(|item| {
            if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
                return None;
            }
            super::codex_events::extract_agent_message_text(item)
                .map(|text| !text.trim().is_empty())
        })
        .filter(|is_non_empty| *is_non_empty)
        .count();
    let legacy_non_agent_text_bias = items
        .iter()
        .filter(|item| {
            matches!(
                item.get("type").and_then(Value::as_str).unwrap_or("unknown"),
                "reasoning" | "plan"
            )
        })
        .count();
    let resolve_after_text_count = |cached: &Value| -> Option<usize> {
        let raw = cached
            .get("afterTextCount")
            .and_then(Value::as_u64)
            .map(|count| count as usize)?;
        let mode = cached.get("afterTextCountMode").and_then(Value::as_str);
        if mode == Some("agentMessageOnly") {
            Some(raw)
        } else if legacy_non_agent_text_bias > 0 {
            Some(raw.saturating_sub(legacy_non_agent_text_bias))
        } else {
            Some(raw)
        }
    };

    let cached_command_item_ids: HashSet<&str> = cached_commands
        .iter()
        .filter_map(|cached| cached.get("itemId").and_then(Value::as_str))
        .collect();
    let cached_file_change_item_ids: HashSet<&str> = cached_file_changes
        .iter()
        .filter_map(|cached| cached.get("itemId").and_then(Value::as_str))
        .collect();
    let cached_tool_item_ids: HashSet<&str> = cached_tool_items
        .iter()
        .filter_map(|cached| cached.get("itemId").and_then(Value::as_str))
        .collect();

    let mut ordered_cached_commands: Vec<&Value> = cached_commands.iter().collect();
    let mut ordered_cached_file_changes: Vec<&Value> = cached_file_changes.iter().collect();
    let mut ordered_cached_tool_items: Vec<&Value> = cached_tool_items.iter().collect();
    ordered_cached_commands.sort_by_key(|cached| {
        cached
            .get("afterTextCount")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
    });
    ordered_cached_file_changes.sort_by_key(|cached| {
        cached
            .get("afterTextCount")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
    });
    ordered_cached_tool_items.sort_by_key(|cached| {
        cached
            .get("afterTextCount")
            .and_then(Value::as_u64)
            .unwrap_or(u64::MAX)
    });

    let mut cached_by_text_count: HashMap<usize, Vec<&Value>> = HashMap::new();
    let mut trailing_cached_commands: Vec<&Value> = Vec::new();
    let mut cached_file_changes_by_text_count: HashMap<usize, Vec<&Value>> = HashMap::new();
    let mut trailing_cached_file_changes: Vec<&Value> = Vec::new();
    let mut cached_tool_items_by_text_count: HashMap<usize, Vec<&Value>> = HashMap::new();
    let mut trailing_cached_tool_items: Vec<&Value> = Vec::new();
    for cached in ordered_cached_commands {
        let fallback_after_text_count = if total_text_count > 0 {
            Some(total_text_count)
        } else {
            None
        };
        if let Some(after_text_count) = resolve_after_text_count(cached).or(fallback_after_text_count)
        {
            cached_by_text_count
                .entry(after_text_count)
                .or_default()
                .push(cached);
        } else {
            trailing_cached_commands.push(cached);
        }
    }
    for cached in ordered_cached_file_changes {
        let fallback_after_text_count = if total_text_count > 0 {
            Some(total_text_count)
        } else {
            None
        };
        if let Some(after_text_count) = resolve_after_text_count(cached).or(fallback_after_text_count)
        {
            cached_file_changes_by_text_count
                .entry(after_text_count)
                .or_default()
                .push(cached);
        } else {
            trailing_cached_file_changes.push(cached);
        }
    }
    for cached in ordered_cached_tool_items {
        let fallback_after_text_count = if total_text_count > 0 {
            Some(total_text_count)
        } else {
            None
        };
        if let Some(after_text_count) = resolve_after_text_count(cached).or(fallback_after_text_count)
        {
            cached_tool_items_by_text_count
                .entry(after_text_count)
                .or_default()
                .push(cached);
        } else {
            trailing_cached_tool_items.push(cached);
        }
    }

    let mut emitted_text_count: usize = 0;
    append_cached_commands_for_text_count(
        &body_box,
        &mut cached_by_text_count,
        0,
        &mut has_content,
    );
    if let Some(cached_entries) = cached_file_changes_by_text_count.remove(&0) {
        for cached in cached_entries {
            let widget = super::message_render::create_file_change_widget(cached);
            super::message_render::append_action_widget(&body_box, "fileChange", &widget);
            has_content = true;
        }
    }
    append_cached_tool_items_for_text_count(
        &body_box,
        &mut cached_tool_items_by_text_count,
        0,
        &mut has_content,
    );

    for item in items {
        match item
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
        {
            "agentMessage" => {
                if let Some(text) = super::codex_events::extract_agent_message_text(item) {
                    if !text.trim().is_empty() {
                        let label = super::message_render::create_text_segment(&body_box);
                        super::markdown::set_markdown(&label, &text);
                        has_content = true;
                        emitted_text_count += 1;
                        append_cached_commands_for_text_count(
                            &body_box,
                            &mut cached_by_text_count,
                            emitted_text_count,
                            &mut has_content,
                        );
                        if let Some(cached_entries) =
                            cached_file_changes_by_text_count.remove(&emitted_text_count)
                        {
                            for cached in cached_entries {
                                let widget = super::message_render::create_file_change_widget(cached);
                                super::message_render::append_action_widget(
                                    &body_box,
                                    "fileChange",
                                    &widget,
                                );
                                has_content = true;
                            }
                        }
                        append_cached_tool_items_for_text_count(
                            &body_box,
                            &mut cached_tool_items_by_text_count,
                            emitted_text_count,
                            &mut has_content,
                        );
                    }
                }
            }
            "commandExecution" => {
                if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                    if cached_command_item_ids.contains(item_id) {
                        continue;
                    }
                }
                append_command_from_value(&body_box, item);
                has_content = true;
            }
            "fileChange" => {
                if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                    if cached_file_change_item_ids.contains(item_id) {
                        continue;
                    }
                }
                let widget = super::message_render::create_file_change_widget(item);
                super::message_render::append_action_widget(&body_box, "fileChange", &widget);
                has_content = true;
            }
            "dynamicToolCall" => {
                if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                    if cached_tool_item_ids.contains(item_id) {
                        continue;
                    }
                }
                let (tool_name, arguments, status, output) =
                    super::codex_events::extract_dynamic_tool_call_fields(item);
                let (widget, tool_ui) =
                    super::message_render::create_tool_call_widget(&tool_name, &arguments);
                tool_ui.status_label.set_text(if status == "failed" {
                    "Failed"
                } else {
                    "Completed"
                });
                tool_ui.set_output(&output);
                super::message_render::append_action_widget(
                    &body_box,
                    "dynamicToolCall",
                    &widget,
                );
                has_content = true;
            }
            "webSearch" | "mcpToolCall" | "collabToolCall" | "imageView" | "enteredReviewMode"
            | "exitedReviewMode" | "contextCompaction" => {
                if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                    if cached_tool_item_ids.contains(item_id) {
                        continue;
                    }
                }
                let (section, title, summary, status, output) =
                    super::codex_events::extract_generic_item_fields(item);
                let (widget, generic_ui) =
                    super::message_render::create_generic_item_widget(&section, &title, &summary);
                generic_ui.set_title(&title);
                generic_ui.set_details(&summary, &output);
                generic_ui.set_running(status == "running");
                let action_kind = item
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or(section.as_str());
                super::message_render::append_action_widget(&body_box, action_kind, &widget);
                has_content = true;
            }
            _ => {}
        }
    }

    let mut remaining_counts: Vec<usize> = cached_by_text_count
        .keys()
        .chain(cached_file_changes_by_text_count.keys())
        .chain(cached_tool_items_by_text_count.keys())
        .copied()
        .collect();
    remaining_counts.sort_unstable();
    remaining_counts.dedup();
    for count in remaining_counts {
        append_cached_commands_for_text_count(
            &body_box,
            &mut cached_by_text_count,
            count,
            &mut has_content,
        );
        if let Some(cached_entries) = cached_file_changes_by_text_count.remove(&count) {
            for cached in cached_entries {
                let widget = super::message_render::create_file_change_widget(cached);
                super::message_render::append_action_widget(&body_box, "fileChange", &widget);
                has_content = true;
            }
        }
        append_cached_tool_items_for_text_count(
            &body_box,
            &mut cached_tool_items_by_text_count,
            count,
            &mut has_content,
        );
    }
    for cached in &trailing_cached_commands {
        append_command_from_value(&body_box, cached);
        has_content = true;
    }
    for cached in &trailing_cached_file_changes {
        let widget = super::message_render::create_file_change_widget(cached);
        super::message_render::append_action_widget(&body_box, "fileChange", &widget);
        has_content = true;
    }
    for cached in &trailing_cached_tool_items {
        if append_tool_item_from_value(&body_box, cached) {
            has_content = true;
        }
    }

    if !has_content {
        if let Some(message) = turn_error {
            let label = super::message_render::create_text_segment(&body_box);
            super::markdown::set_markdown(&label, &format!("**Turn failed**\n\n{message}"));
            has_content = true;
        }
    }

    if !has_content {
        return false;
    }

    conversation_stack.set_visible_child_name("messages");
    super::message_render::append_hover_timestamp(messages_box, &row, &bubble, false, timestamp);
    super::message_render::make_assistant_row_full_width(&row);
    messages_box.append(&row);
    true
}
