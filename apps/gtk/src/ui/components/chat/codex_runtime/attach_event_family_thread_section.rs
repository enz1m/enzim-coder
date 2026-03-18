{
    match method {
                "thread/tokenUsage/updated" => {
                    let resolved_thread_id =
                        thread_id.or_else(|| active_turn_thread.borrow().clone());
                    if !super::codex_events::should_render_for_active(
                        resolved_thread_id.as_deref(),
                        active_thread_id.as_deref(),
                    ) {
                        continue;
                    }
                    let Some(note) = extract_token_usage_summary(&event.params) else {
                        continue;
                    };
                    let turn_id = super::codex_events::extract_turn_id(&event.params)
                        .or_else(|| active_turn.borrow().clone())
                        .or_else(|| turn_uis.borrow().keys().last().cloned());
                    let Some(turn_id) = turn_id else {
                        continue;
                    };
                    let mut turns = turn_uis.borrow_mut();
                    let turn_ui = turns.entry(turn_id).or_insert_with(|| {
                        super::create_turn_ui(&messages_box, &messages_scroll, &conversation_stack)
                    });
                    let mut seq = event_note_counter.borrow_mut();
                    *seq += 1;
                    let key = format!("event:token-usage:{}", *seq);
                    append_event_note_to_turn(turn_ui, &messages_scroll, key, &note);
                }
                "thread/status/changed" => {
                    let resolved_thread_id =
                        thread_id.or_else(|| active_turn_thread.borrow().clone());
                    if !super::codex_events::should_render_for_active(
                        resolved_thread_id.as_deref(),
                        active_thread_id.as_deref(),
                    ) {
                        continue;
                    }
                    let status = event
                        .params
                        .get("status")
                        .and_then(Value::as_str)
                        .or_else(|| {
                            event
                                .params
                                .get("thread")
                                .and_then(|v| v.get("status"))
                                .and_then(Value::as_str)
                        })
                        .unwrap_or("unknown");
                    if status == "unknown" || status.trim().is_empty() {
                        continue;
                    }
                    let turn_id = super::codex_events::extract_turn_id(&event.params)
                        .or_else(|| active_turn.borrow().clone())
                        .or_else(|| turn_uis.borrow().keys().last().cloned());
                    let Some(turn_id) = turn_id else {
                        continue;
                    };
                    let mut turns = turn_uis.borrow_mut();
                    let turn_ui = turns.entry(turn_id).or_insert_with(|| {
                        super::create_turn_ui(&messages_box, &messages_scroll, &conversation_stack)
                    });
                    let mut seq = event_note_counter.borrow_mut();
                    *seq += 1;
                    let key = format!("event:thread-status:{}", *seq);
                    append_event_note_to_turn(
                        turn_ui,
                        &messages_scroll,
                        key,
                        &format!("_Thread status changed: `{status}`._"),
                    );
                }
                "thread/archived" | "thread/unarchived" | "thread/closed" => {
                    let resolved_thread_id =
                        thread_id.or_else(|| active_turn_thread.borrow().clone());
                    if !super::codex_events::should_render_for_active(
                        resolved_thread_id.as_deref(),
                        active_thread_id.as_deref(),
                    ) {
                        continue;
                    }
                    let note = match event.method.as_str() {
                        "thread/archived" => "_Thread archived._",
                        "thread/unarchived" => "_Thread unarchived._",
                        _ => "_Thread closed._",
                    };
                    let turn_id = super::codex_events::extract_turn_id(&event.params)
                        .or_else(|| active_turn.borrow().clone())
                        .or_else(|| turn_uis.borrow().keys().last().cloned());
                    let Some(turn_id) = turn_id else {
                        continue;
                    };
                    let mut turns = turn_uis.borrow_mut();
                    let turn_ui = turns.entry(turn_id).or_insert_with(|| {
                        super::create_turn_ui(&messages_box, &messages_scroll, &conversation_stack)
                    });
                    let mut seq = event_note_counter.borrow_mut();
                    *seq += 1;
                    let key = format!("event:thread-lifecycle:{}", *seq);
                    append_event_note_to_turn(turn_ui, &messages_scroll, key, note);
                }
        _ => {}
    }
}
