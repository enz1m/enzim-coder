pub(crate) fn apply_restore_with_chat_sync(
    db: &AppDb,
    codex: Option<Arc<RuntimeClient>>,
    active_thread_id: Option<Rc<RefCell<Option<String>>>>,
    workspace_path: Option<&str>,
    codex_thread_id: &str,
    checkpoint_id: i64,
    target_turn_id: Option<&str>,
    selected_paths: &[String],
    forced_paths: &[String],
    parent_window: Option<&gtk::Window>,
    prefill_prompt: Option<&str>,
) -> Result<(crate::restore::RestoreApplyResult, String), String> {
    let apply = crate::restore::apply_restore_to_checkpoint_by_remote_id(
        db,
        codex_thread_id,
        checkpoint_id,
        selected_paths,
        forced_paths,
    )?;
    let Some(result) = apply else {
        return Err("Restore apply unavailable for this checkpoint.".to_string());
    };

    let mut allow_prefill = false;
    let rollback_status = if let Some(target_turn_id) = target_turn_id {
        let Some(client) = codex.as_ref() else {
            return Err(
                "Restore applied, but chat trim failed: runtime client unavailable.".to_string(),
            );
        };
        if !client.capabilities().supports_rollback {
            let status = " • Chat trim skipped (runtime does not support rollback)".to_string();
            if allow_prefill {
                if let (Some(parent_window), Some(prompt)) = (parent_window, prefill_prompt) {
                    if !prompt.trim().is_empty() {
                        set_composer_input_text(parent_window, prompt);
                    }
                }
            }
            return Ok((result, status));
        }
        if let Some(active) = active_thread_id.as_ref() {
            if active.borrow().as_deref() != Some(codex_thread_id) {
                active.replace(Some(codex_thread_id.to_string()));
            }
        }

        let thread_read_result = match client.thread_read(codex_thread_id, true) {
            Ok(thread) => Ok(thread),
            Err(err) if err.contains("thread not loaded") || err.contains("no rollout found") => {
                if let Some(workspace_path) = workspace_path {
                    match client.thread_resume(
                        codex_thread_id,
                        Some(workspace_path),
                        Some("gpt-5.3-codex"),
                    ) {
                        Ok(_) => client.thread_read(codex_thread_id, true),
                        Err(resume_err) => Err(format!(
                            "failed to materialize thread for rollback: {resume_err}"
                        )),
                    }
                } else {
                    Err(err)
                }
            }
            Err(err) => Err(err),
        };

        match thread_read_result {
            Ok(thread) => {
                let Some(turns) = thread.get("turns").and_then(Value::as_array) else {
                    return Err(
                        "Restore applied, but chat trim failed: thread has no turns payload."
                            .to_string(),
                    );
                };

                let Some(target_index) = turns.iter().position(|turn| {
                    turn.get("id").and_then(Value::as_str) == Some(target_turn_id)
                }) else {
                    return Err(
                        "Restore applied, but chat trim failed: selected turn was not found in the remote thread."
                            .to_string(),
                    );
                };

                let mut indexed: Vec<(usize, i64)> = turns
                    .iter()
                    .enumerate()
                    .map(|(idx, turn)| {
                        let ts = parse_turn_timestamp_opt(turn).unwrap_or(idx as i64);
                        (idx, ts)
                    })
                    .collect();
                indexed.sort_by_key(|(_, ts)| *ts);
                let chrono_target_pos = indexed
                    .iter()
                    .position(|(idx, _)| *idx == target_index)
                    .unwrap_or(target_index);
                let rollback_count = indexed.len().saturating_sub(chrono_target_pos);
                eprintln!(
                    "[restore] rollback request thread_id={} target_turn_id={} total_turns={} target_index={} chrono_target_pos={} rollback_count={}",
                    codex_thread_id,
                    target_turn_id,
                    turns.len(),
                    target_index,
                    chrono_target_pos,
                    rollback_count
                );
                if rollback_count > 0 {
                    let rollback_result =
                        match client.thread_rollback(codex_thread_id, rollback_count) {
                            Ok(thread) => Ok(thread),
                            Err(err)
                                if err.contains("thread not found")
                                    || err.contains("thread not loaded")
                                    || err.contains("no rollout found") =>
                            {
                                if let Some(workspace_path) = workspace_path {
                                    match client.thread_resume(
                                        codex_thread_id,
                                        Some(workspace_path),
                                        Some("gpt-5.3-codex"),
                                    ) {
                                        Ok(_) => {
                                            client.thread_rollback(codex_thread_id, rollback_count)
                                        }
                                        Err(resume_err) => Err(format!(
                                            "{err}; resume before rollback failed: {resume_err}"
                                        )),
                                    }
                                } else {
                                    Err(err)
                                }
                            }
                            Err(err) => Err(err),
                        };

                    match rollback_result {
                        Ok(rolled_thread) => {
                            let remaining_turns = rolled_thread
                                .get("turns")
                                .and_then(Value::as_array)
                                .map(|v| v.len())
                                .unwrap_or(0);
                            eprintln!(
                                "[restore] rollback applied thread_id={} removed_turns={} remaining_turns={}",
                                codex_thread_id, rollback_count, remaining_turns
                            );
                            if let Some(workspace_path) = workspace_path {
                                if let Err(err) = client.thread_resume(
                                    codex_thread_id,
                                    Some(workspace_path),
                                    Some("gpt-5.3-codex"),
                                ) {
                                    eprintln!(
                                        "[restore] warning: thread/resume after rollback failed thread_id={}: {}",
                                        codex_thread_id, err
                                    );
                                }
                            }
                            let sync_thread = client
                                .thread_read(codex_thread_id, true)
                                .unwrap_or(rolled_thread);
                            let sync_turns = sync_thread
                                .get("turns")
                                .and_then(Value::as_array)
                                .map(|v| v.len())
                                .unwrap_or(0);
                            eprintln!(
                                "[restore] post-resume sync thread_id={} turns={}",
                                codex_thread_id, sync_turns
                            );
                            if let Some(parent_window) = parent_window {
                                let refreshed = super::chat::refresh_visible_history_for_thread(
                                    db,
                                    parent_window,
                                    codex_thread_id,
                                    &sync_thread,
                                );
                                if !refreshed {
                                    return Err(
                                        "Restore applied and chat trimmed, but failed to refresh visible chat UI."
                                            .to_string(),
                                    );
                                }
                            } else {
                                let synced = super::chat::sync_local_history_for_thread(
                                    db,
                                    codex_thread_id,
                                    &sync_thread,
                                );
                                if !synced {
                                    return Err(
                                        "Restore applied and chat trimmed, but failed to persist local chat history."
                                            .to_string(),
                                    );
                                }
                            }
                            super::chat::request_runtime_history_reload(codex_thread_id);
                            allow_prefill = true;
                            format!(" • Chat trimmed: {} turn(s)", rollback_count)
                        }
                        Err(err) => {
                            return Err(format!("Restore applied, but chat trim failed: {err}"));
                        }
                    }
                } else {
                    " • Chat already at restored point".to_string()
                }
            }
            Err(err) => {
                return Err(format!("Restore applied, but chat trim failed: {err}"));
            }
        }
    } else {
        " • Chat trim skipped (no remote turn context)".to_string()
    };

    if allow_prefill {
        if let (Some(parent_window), Some(prompt)) = (parent_window, prefill_prompt) {
            if !prompt.trim().is_empty() {
                set_composer_input_text(parent_window, prompt);
            }
        }
    }

    Ok((result, rollback_status))
}
