{
        while let Ok((thread_id, log_source, render_policy, history_result)) = history_rx.try_recv() {
            if loading_history_thread_id.borrow().as_deref() == Some(thread_id.as_str()) {
                loading_history_thread_id.replace(None);
            }
            if active_thread_id.borrow().as_deref() != Some(thread_id.as_str()) {
                continue;
            }

            match history_result {
                Ok(thread_value) => {
                    clear_history_unavailable_for_thread(&thread_id);
                    clear_history_dirty_for_thread(&thread_id);
                    let turns = thread_value
                        .get("turns")
                        .and_then(Value::as_array)
                        .map(|turns| turns.len())
                        .unwrap_or(0);
                    log_history_load_step(
                        &thread_id,
                        &format!("appserver history received ({turns} turns)"),
                    );
                    eprintln!(
                        "[runtime:{log_source}] loaded history thread_id={} turns={}",
                        thread_id, turns
                    );
                    let was_already_loaded = loaded_history_thread_id.borrow().as_deref()
                        == Some(thread_id.as_str());
                    match super::codex_history::sync_completed_turns_from_thread(
                        &db,
                        &thread_id,
                        &thread_value,
                    ) {
                        Err(err) => {
                            eprintln!(
                                "[runtime:{log_source}] failed to sync local turns thread_id={}: {}",
                                thread_id, err
                            );
                        }
                        Ok(synced_turn_count) => {
                            log_history_load_step(
                                &thread_id,
                                &format!("synced {synced_turn_count} completed turns into sqlite"),
                            );
                            if synced_turn_count > 0 {
                                cached_history_snapshots.borrow_mut().remove(&thread_id);
                            }
                            let has_in_progress = {
                                let turn_threads_ref = turn_threads.borrow();
                                turn_uis.borrow().iter().any(|(turn_id, turn_ui)| {
                                    turn_ui.in_progress
                                        && turn_threads_ref
                                            .get(turn_id)
                                            .map(|owner| owner == &thread_id)
                                            .unwrap_or(false)
                                })
                            };
                            let has_live_turn_ui = {
                                let turn_threads_ref = turn_threads.borrow();
                                turn_uis.borrow().keys().any(|turn_id| {
                                    turn_threads_ref
                                        .get(turn_id)
                                        .map(|owner| owner == &thread_id)
                                        .unwrap_or(false)
                                })
                            };
                            let has_shared_active_turn =
                                active_turn_for_thread(&thread_id).is_some();
                            if has_in_progress || (has_shared_active_turn && !has_live_turn_ui) {
                                loaded_history_thread_id.replace(Some(thread_id));
                            continue;
                        }
                        super::codex_history::prune_cached_state_for_thread(
                            &db,
                            &thread_id,
                            &thread_value,
                        );
                        cached_commands_for_thread
                            .replace(super::codex_history::load_cached_commands(&db, &thread_id));
                        cached_file_changes_for_thread.replace(
                            super::codex_history::load_cached_file_changes(&db, &thread_id),
                        );
                        cached_tool_items_for_thread.replace(
                            super::codex_history::load_cached_tool_items(&db, &thread_id),
                        );
                        cached_pending_requests_for_thread.replace(
                            super::codex_history::load_cached_pending_requests(&db, &thread_id),
                        );
                        cached_turn_errors_for_thread.replace(
                            super::codex_history::load_cached_turn_errors(&db, &thread_id),
                        );
                            let needs_rerender = matches!(
                                render_policy,
                                HistorySyncRenderPolicy::AllowRerender
                            ) && !has_live_turn_ui
                                && (!was_already_loaded || synced_turn_count > 0);
                            if needs_rerender {
                                log_history_load_step(
                                    &thread_id,
                                    "scheduling sqlite re-render after sync",
                                );
                                let _ = super::codex_history::render_local_thread_history_from_db(
                                    &db,
                                    Some(manager.clone()),
                                    &messages_box,
                                    &messages_scroll,
                                    &conversation_stack,
                                    &suggestion_row,
                                    &thread_id,
                                    None,
                                );
                            }
                        }
                    }
                    loaded_history_thread_id.replace(Some(thread_id));
                }
                Err(err) => {
                    let no_runtime_error = err.contains("No runtime available for thread");
                    let pre_materialization_error = (err.contains("not materialized yet")
                        && err.contains("includeTurns is unavailable"))
                        || err.contains("thread not loaded");
                    let is_unavailable_error = no_runtime_error;
                    let already_unavailable =
                        is_unavailable_error && history_is_unavailable_for_thread(&thread_id);
                    if !already_unavailable && !pre_materialization_error {
                        eprintln!(
                            "[runtime:{log_source}] failed history load thread_id={}: {}",
                            thread_id, err
                        );
                    }
                    if no_runtime_error {
                        mark_history_unavailable_for_thread(&thread_id);
                        loaded_history_thread_id.replace(Some(thread_id));
                        continue;
                    }
                    if pre_materialization_error {
                    }
                    loaded_history_thread_id.replace(Some(thread_id));
                }
            }
        }
}
