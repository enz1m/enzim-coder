{
let (enzim_agent_worker_tx, enzim_agent_worker_rx) = mpsc::channel::<(
    String,
    i64,
    Option<crate::services::enzim_agent::LoopDriverAction>,
    Option<String>,
)>();
let (enzim_agent_dispatch_tx, enzim_agent_dispatch_rx) =
    mpsc::channel::<(String, Option<i64>, Option<i64>, Option<String>, Option<String>)>();
let enzim_agent_in_flight_loops: Rc<RefCell<std::collections::HashSet<i64>>> =
    Rc::new(RefCell::new(std::collections::HashSet::new()));
let enzim_agent_last_thread_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
let enzim_agent_dismissed_finished_loops: Rc<RefCell<std::collections::HashMap<String, i64>>> =
    Rc::new(RefCell::new(std::collections::HashMap::new()));

let format_loop_status: Rc<
    dyn Fn(Option<&crate::services::app::chat::EnzimAgentLoopRecord>) -> String,
> =
    Rc::new(move |loop_record| match loop_record {
        None => "No active loop on this thread.".to_string(),
        Some(loop_record) => {
            let base = match loop_record.status.as_str() {
                "waiting_runtime" => format!(
                    "Loop running via {}. Waiting for the coding agent to finish the current turn.",
                    loop_record.backend_kind
                ),
                "evaluating" => "Enzim Agent is evaluating the latest agent response.".to_string(),
                "waiting_user" => "Enzim Agent needs your answer to continue the loop.".to_string(),
                "finished" => "Loop finished.".to_string(),
                "cancelled" => "Loop stopped.".to_string(),
                "paused_error" => "Loop paused because of an error.".to_string(),
                "active" => "Loop active.".to_string(),
                other => format!("Loop status: {other}."),
            };
            let mut details = vec![base];
            details.push(format!("Iterations: {}", loop_record.iteration_count));
            if loop_record.error_count > 0 {
                details.push(format!("Errors: {}", loop_record.error_count));
            }
            if let Some(error) = loop_record
                .last_error_text
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                details.push(format!("Last error: {error}"));
            }
            details.join(" ")
        }
    });

let save_enzim_drafts_for_thread: Rc<dyn Fn(&str)> = {
    let db = db.clone();
    let enzim_agent_prompt_view = enzim_agent_prompt_view.clone();
    let enzim_agent_instructions_view = enzim_agent_instructions_view.clone();
    let enzim_agent_telegram_questions_switch = enzim_agent_telegram_questions_switch.clone();
    Rc::new(move |thread_id| {
        let thread_id = thread_id.trim();
        if thread_id.is_empty() {
            return;
        }
        let prompt_buffer = enzim_agent_prompt_view.buffer();
        let prompt_text = prompt_buffer
            .text(&prompt_buffer.start_iter(), &prompt_buffer.end_iter(), true)
            .to_string();
        save_thread_setting(&db, thread_id, "enzim_agent_prompt", &prompt_text);

        let instructions_buffer = enzim_agent_instructions_view.buffer();
        let instructions_text = instructions_buffer
            .text(
                &instructions_buffer.start_iter(),
                &instructions_buffer.end_iter(),
                true,
            )
            .to_string();
        save_thread_setting(
            &db,
            thread_id,
            "enzim_agent_instructions",
            &instructions_text,
        );
        save_thread_setting(
            &db,
            thread_id,
            "enzim_agent_telegram_questions",
            if enzim_agent_telegram_questions_switch.is_active() {
                "1"
            } else {
                "0"
            },
        );
    })
};

let load_enzim_drafts_for_thread: Rc<dyn Fn(Option<&str>)> = {
    let db = db.clone();
    let enzim_agent_prompt_view = enzim_agent_prompt_view.clone();
    let enzim_agent_instructions_view = enzim_agent_instructions_view.clone();
    let enzim_agent_telegram_questions_switch = enzim_agent_telegram_questions_switch.clone();
    Rc::new(move |thread_id| {
        let defaults = crate::services::enzim_agent::effective_loop_draft_defaults(db.as_ref());
        let prompt_text = thread_id
            .and_then(|thread_id| thread_setting_value(&db, thread_id, "enzim_agent_prompt"))
            .unwrap_or_else(|| defaults.prompt_text.clone());
        let instructions_text = thread_id
            .and_then(|thread_id| {
                thread_setting_value(&db, thread_id, "enzim_agent_instructions")
            })
            .unwrap_or_else(|| defaults.instructions_text.clone());
        let telegram_questions_enabled = thread_id
            .and_then(|thread_id| {
                thread_setting_value(&db, thread_id, "enzim_agent_telegram_questions")
            })
            .as_deref()
            == Some("1");

        let prompt_buffer = enzim_agent_prompt_view.buffer();
        let current_prompt = prompt_buffer
            .text(&prompt_buffer.start_iter(), &prompt_buffer.end_iter(), true)
            .to_string();
        if current_prompt != prompt_text {
            prompt_buffer.set_text(&prompt_text);
        }

        let instructions_buffer = enzim_agent_instructions_view.buffer();
        let current_instructions = instructions_buffer
            .text(
                &instructions_buffer.start_iter(),
                &instructions_buffer.end_iter(),
                true,
            )
            .to_string();
        if current_instructions != instructions_text {
            instructions_buffer.set_text(&instructions_text);
        }
        if enzim_agent_telegram_questions_switch.is_active() != telegram_questions_enabled {
            enzim_agent_telegram_questions_switch.set_active(telegram_questions_enabled);
        }
    })
};

{
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_last_thread_id = enzim_agent_last_thread_id.clone();
    let save_enzim_drafts_for_thread = save_enzim_drafts_for_thread.clone();
    let load_enzim_drafts_for_thread = load_enzim_drafts_for_thread.clone();
    let enzim_agent_prompt_view = enzim_agent_prompt_view.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(140), move || {
        if enzim_agent_prompt_view.root().is_none() {
            return gtk::glib::ControlFlow::Break;
        }

        let next_thread_id = active_thread_id.borrow().clone();
        let previous_thread_id = enzim_agent_last_thread_id.borrow().clone();
        if next_thread_id == previous_thread_id {
            return gtk::glib::ControlFlow::Continue;
        }

        if let Some(previous_thread_id) = previous_thread_id.as_deref() {
            save_enzim_drafts_for_thread(previous_thread_id);
        }

        enzim_agent_last_thread_id.replace(next_thread_id.clone());
        load_enzim_drafts_for_thread(next_thread_id.as_deref());
        gtk::glib::ControlFlow::Continue
    });
}

{
    let active_thread_id = active_thread_id.clone();
    let save_enzim_drafts_for_thread = save_enzim_drafts_for_thread.clone();
    let prompt_buffer = enzim_agent_prompt_view.buffer();
    prompt_buffer.connect_changed(move |_| {
        if let Some(thread_id) = active_thread_id.borrow().as_deref() {
            save_enzim_drafts_for_thread(thread_id);
        }
    });
}

{
    let active_thread_id = active_thread_id.clone();
    let save_enzim_drafts_for_thread = save_enzim_drafts_for_thread.clone();
    let instructions_buffer = enzim_agent_instructions_view.buffer();
    instructions_buffer.connect_changed(move |_| {
        if let Some(thread_id) = active_thread_id.borrow().as_deref() {
            save_enzim_drafts_for_thread(thread_id);
        }
    });
}

{
    let active_thread_id = active_thread_id.clone();
    let save_enzim_drafts_for_thread = save_enzim_drafts_for_thread.clone();
    let enzim_agent_telegram_questions_switch = enzim_agent_telegram_questions_switch.clone();
    enzim_agent_telegram_questions_switch.connect_active_notify(move |_| {
        if let Some(thread_id) = active_thread_id.borrow().as_deref() {
            save_enzim_drafts_for_thread(thread_id);
        }
    });
}

let dispatch_enzim_message: Rc<
    dyn Fn(String, String, Option<String>, Option<i64>, Option<i64>, bool) -> Result<(), String>,
> = {
    let db = db.clone();
    let active_thread_id = active_thread_id.clone();
    let messages_box = messages_box.clone();
    let messages_scroll = messages_scroll.clone();
    let conversation_stack = conversation_stack.clone();
    let resolve_client_for_thread = resolve_client_for_thread.clone();
    let selected_mode_id = selected_mode_id.clone();
    let selected_model_id = selected_model_id.clone();
    let selected_effort = selected_effort.clone();
    let selected_variant = selected_variant.clone();
    let selected_opencode_command_mode = selected_opencode_command_mode.clone();
    let selected_access_mode = selected_access_mode.clone();
    let send_error_tx = send_error_tx.clone();
    let turn_started_ui_tx = turn_started_ui_tx.clone();
    let enzim_agent_dispatch_tx = enzim_agent_dispatch_tx.clone();
    Rc::new(
        move |remote_thread_id, text, badge, loop_id, event_id, rename_on_first_prompt| {
            let client = resolve_client_for_thread(&remote_thread_id).ok_or_else(|| {
                "Runtime backend is not available for this thread. Start the selected profile runtime and retry."
                    .to_string()
            })?;
            let is_active_thread = active_thread_id.borrow().as_deref() == Some(remote_thread_id.as_str());

            let pending_row_marker = if is_active_thread {
                let image_paths: Vec<String> = Vec::new();
                let user_content = super::message_render::append_user_message_with_images_badged(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    &text,
                    &image_paths,
                    badge.as_deref(),
                    SystemTime::now(),
                );
                let pending_row_marker = next_pending_user_row_marker();
                let _ = super::message_render::set_message_row_marker(
                    &user_content,
                    &pending_row_marker,
                );
                Some(pending_row_marker)
            } else {
                None
            };

            if rename_on_first_prompt && !text.trim().is_empty() {
                if let Some(next_title) = title_from_first_prompt(&text) {
                    match db.rename_thread_if_new_by_remote_id(&remote_thread_id, &next_title) {
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
                        Err(err) => eprintln!("failed to rename thread on Enzim prompt send: {err}"),
                    }
                }
            }

            let is_opencode = client.backend_kind().eq_ignore_ascii_case("opencode");
            let current_thread_id = active_thread_id.borrow().clone();
            let model_id = if current_thread_id.as_deref() == Some(remote_thread_id.as_str()) {
                selected_model_id.borrow().clone()
            } else {
                thread_setting_value(&db, &remote_thread_id, "model")
                    .or_else(|| default_composer_setting_value(&db, "model"))
                    .unwrap_or_default()
            };
            let effort = if is_opencode {
                if current_thread_id.as_deref() == Some(remote_thread_id.as_str()) {
                    selected_variant.borrow().clone()
                } else {
                    thread_setting_value(&db, &remote_thread_id, "variant")
                        .or_else(|| default_composer_setting_value(&db, "variant"))
                        .unwrap_or_default()
                }
            } else if current_thread_id.as_deref() == Some(remote_thread_id.as_str()) {
                selected_effort.borrow().clone()
            } else {
                thread_setting_value(&db, &remote_thread_id, "effort")
                    .or_else(|| default_composer_setting_value(&db, "effort"))
                    .unwrap_or_else(|| "medium".to_string())
            };
            let access_mode = if current_thread_id.as_deref() == Some(remote_thread_id.as_str()) {
                selected_access_mode.borrow().clone()
            } else {
                thread_setting_value(&db, &remote_thread_id, "access_mode")
                    .or_else(|| default_composer_setting_value(&db, "access_mode"))
                    .unwrap_or_default()
            };
            let collaboration_mode = if current_thread_id.as_deref() == Some(remote_thread_id.as_str()) {
                selected_mode_id.borrow().clone()
            } else {
                thread_setting_value(&db, &remote_thread_id, "collaboration_mode")
                    .or_else(|| default_composer_setting_value(&db, "collaboration_mode"))
                    .unwrap_or_default()
            };
            let command_mode = if is_opencode {
                Some(
                    if current_thread_id.as_deref() == Some(remote_thread_id.as_str()) {
                        selected_opencode_command_mode.borrow().clone()
                    } else {
                        thread_setting_value(&db, &remote_thread_id, "opencode_command_mode")
                            .or_else(|| default_composer_setting_value(&db, "opencode_command_mode"))
                            .unwrap_or_else(|| "allowAll".to_string())
                    },
                )
            } else {
                None
            };
            let sandbox_policy = super::runtime_controls::sandbox_policy_for(&access_mode);
            let collaboration_mode_for_turn =
                collaboration_mode_payload(&collaboration_mode, &model_id, &effort);

            let send_error_tx_for_thread = send_error_tx.clone();
            let turn_started_ui_tx_for_thread = turn_started_ui_tx.clone();
            let enzim_agent_dispatch_tx_for_thread = enzim_agent_dispatch_tx.clone();
            std::thread::spawn(move || {
                let workspace_path_for_turn =
                    crate::services::app::chat::BackgroundRepo::workspace_path_for_remote_thread(
                        &remote_thread_id,
                    );
                let model_value = model_id.trim().to_string();
                let model_for_turn = if model_value.is_empty() {
                    None
                } else {
                    Some(model_value)
                };
                let effort_value = effort.trim().to_string();
                let effort_for_turn = if effort_value.is_empty() {
                    None
                } else {
                    Some(effort_value)
                };

                if let Some(command_mode) = command_mode
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    if let Err(err) = client.thread_set_command_mode(&remote_thread_id, command_mode) {
                        let message =
                            format!("Failed to configure OpenCode command mode: {err}");
                        let _ = enzim_agent_dispatch_tx_for_thread.send((
                            remote_thread_id.clone(),
                            loop_id,
                            None,
                            None,
                            Some(message.clone()),
                        ));
                        let _ = send_error_tx_for_thread.send(message);
                        return;
                    }
                }

                if let Some(workspace_path) = workspace_path_for_turn.as_deref() {
                    match client.thread_resume(
                        &remote_thread_id,
                        Some(workspace_path),
                        model_for_turn.as_deref(),
                    ) {
                        Ok(resolved_thread_id) => {
                            if resolved_thread_id != remote_thread_id {
                                let message = format!(
                                    "Turn failed: thread resume mismatch (expected {remote_thread_id}, got {resolved_thread_id})"
                                );
                                let _ = enzim_agent_dispatch_tx_for_thread.send((
                                    remote_thread_id.clone(),
                                    loop_id,
                                    None,
                                    None,
                                    Some(message.clone()),
                                ));
                                let _ = send_error_tx_for_thread.send(message);
                                return;
                            }
                        }
                        Err(err) => {
                            if !is_expected_pre_materialization_error(&err) {
                                let message = format!(
                                    "Turn failed: could not resume thread in workspace ({err})"
                                );
                                let _ = enzim_agent_dispatch_tx_for_thread.send((
                                    remote_thread_id.clone(),
                                    loop_id,
                                    None,
                                    None,
                                    Some(message.clone()),
                                ));
                                let _ = send_error_tx_for_thread.send(message);
                                return;
                            }
                        }
                    }
                }

                match client.turn_start(
                    &remote_thread_id,
                    &text,
                    &[],
                    &[],
                    model_for_turn.as_deref(),
                    effort_for_turn.as_deref(),
                    sandbox_policy,
                    None,
                    collaboration_mode_for_turn,
                    workspace_path_for_turn.as_deref(),
                ) {
                    Ok(turn_id) => {
                        if let Some(marker) = pending_row_marker {
                            let _ = turn_started_ui_tx_for_thread.send((marker, turn_id.clone()));
                        }
                        let _ = enzim_agent_dispatch_tx_for_thread.send((
                            remote_thread_id,
                            loop_id,
                            event_id,
                            Some(turn_id),
                            None,
                        ));
                    }
                    Err(err) => {
                        let message = format!("Turn failed: {err}");
                        let _ = enzim_agent_dispatch_tx_for_thread.send((
                            remote_thread_id.clone(),
                            loop_id,
                            None,
                            None,
                            Some(message.clone()),
                        ));
                        let _ = send_error_tx_for_thread.send(message);
                    }
                }
            });
            Ok(())
        },
    )
};

let loop_still_active_for_thread: Rc<dyn Fn(&str, i64) -> bool> = {
    let db = db.clone();
    Rc::new(move |remote_thread_id, loop_id| {
        crate::services::enzim_agent::active_loop_for_remote_thread(db.as_ref(), remote_thread_id)
            .map(|loop_record| loop_record.id == loop_id)
            .unwrap_or(false)
    })
};

let pending_loop_request_for_thread: Rc<
    dyn Fn(&str) -> Option<crate::services::enzim_agent::LoopPendingRequest>,
> = {
    let db = db.clone();
    Rc::new(move |remote_thread_id| {
        let raw = db
            .get_setting(&format!("thread_pending_requests:{remote_thread_id}"))
            .ok()
            .flatten()?;
        let entries = serde_json::from_str::<Vec<serde_json::Value>>(&raw).ok()?;
        let entry = entries.into_iter().find(|entry| {
            entry.get("method").and_then(serde_json::Value::as_str).is_some()
        })?;
        let request_id = entry.get("requestId").and_then(serde_json::Value::as_i64)?;
        let method = entry
            .get("method")
            .and_then(serde_json::Value::as_str)?
            .to_string();
        let params = entry.get("params")?.clone();

        let title = match method.as_str() {
            "item/commandExecution/requestApproval" => "Approve Command Execution".to_string(),
            "item/fileChange/requestApproval" => "Approve File Changes".to_string(),
            "item/tool/requestUserInput" => "Tool Needs Input".to_string(),
            _ => "Action Requires Input".to_string(),
        };
        let questions = params
            .get("questions")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let details = if method == "item/commandExecution/requestApproval" {
            let command = params
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<command unavailable>");
            let cwd = params
                .get("cwd")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<cwd unavailable>");
            let reason = params
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("No reason provided.");
            format!("Command: {command}\nCWD: {cwd}\nReason: {reason}")
        } else if method == "item/fileChange/requestApproval" {
            let reason = params
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("No reason provided.");
            let grant_root = params
                .get("grantRoot")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            if grant_root.is_empty() {
                format!("Reason: {reason}")
            } else {
                format!("Reason: {reason}\nGrant root: {grant_root}")
            }
        } else if method == "item/tool/requestUserInput" && !questions.is_empty() {
            let mut detail_lines = Vec::new();
            if let Some(prompt) = params.get("prompt").and_then(serde_json::Value::as_str) {
                let prompt = prompt.trim();
                if !prompt.is_empty() {
                    detail_lines.push(prompt.to_string());
                }
            }
            for question in &questions {
                let prompt = question
                    .get("question")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Choose an option");
                let options = question
                    .get("options")
                    .and_then(serde_json::Value::as_array)
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|option| option.get("label").and_then(serde_json::Value::as_str))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                if options.is_empty() {
                    detail_lines.push(prompt.to_string());
                } else {
                    detail_lines.push(format!("{prompt} Options: {options}"));
                }
            }
            detail_lines.join("\n")
        } else {
            params
                .get("prompt")
                .and_then(serde_json::Value::as_str)
                .or_else(|| params.get("reason").and_then(serde_json::Value::as_str))
                .unwrap_or("The server requested user input for a tool action.")
                .to_string()
        };

        let options = if method == "item/tool/requestUserInput" && questions.len() == 1 {
            let question = &questions[0];
            let question_id = question
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("question")
                .to_string();
            question
                .get("options")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| {
                    let label = option
                        .get("label")
                        .and_then(serde_json::Value::as_str)?
                        .trim()
                        .to_string();
                    if label.is_empty() {
                        return None;
                    }
                    let option_id = format!(
                        "{}:{}",
                        question_id,
                        label
                            .chars()
                            .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '-' })
                            .collect::<String>()
                            .split('-')
                            .filter(|part| !part.is_empty())
                            .collect::<Vec<_>>()
                            .join("-")
                    );
                    Some(crate::services::enzim_agent::LoopPendingRequestOption {
                        id: option_id,
                        label: label.clone(),
                        payload: serde_json::json!({
                            "answers": {
                                question_id.clone(): {
                                    "answers": [label]
                                }
                            }
                        }),
                    })
                })
                .collect::<Vec<_>>()
        } else {
            let mut raw_decisions = params
                .get("availableDecisions")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_default();
            if raw_decisions.is_empty() {
                raw_decisions = if method == "item/tool/requestUserInput" {
                    vec![
                        serde_json::Value::String("accept".to_string()),
                        serde_json::Value::String("decline".to_string()),
                        serde_json::Value::String("cancel".to_string()),
                    ]
                } else {
                    vec![
                        serde_json::Value::String("accept".to_string()),
                        serde_json::Value::String("acceptForSession".to_string()),
                        serde_json::Value::String("decline".to_string()),
                        serde_json::Value::String("cancel".to_string()),
                    ]
                };
            }
            let has_key = |items: &[serde_json::Value], key: &str| {
                items.iter().any(|decision| {
                    decision.as_str() == Some(key)
                        || decision
                            .as_object()
                            .and_then(|obj| obj.keys().next().map(|value| value.as_str() == key))
                            .unwrap_or(false)
                })
            };
            if method == "item/commandExecution/requestApproval"
                && params.get("proposedExecpolicyAmendment").is_some()
                && !has_key(&raw_decisions, "acceptWithExecpolicyAmendment")
            {
                raw_decisions.push(serde_json::Value::String(
                    "acceptWithExecpolicyAmendment".to_string(),
                ));
            }
            if method == "item/commandExecution/requestApproval"
                && (params.get("proposedNetworkPolicyAmendments").is_some()
                    || params.get("networkApprovalContext").is_some())
                && !has_key(&raw_decisions, "applyNetworkPolicyAmendment")
            {
                raw_decisions.push(serde_json::Value::String(
                    "applyNetworkPolicyAmendment".to_string(),
                ));
            }

            raw_decisions
                .into_iter()
                .filter_map(|decision| {
                    let key = if let Some(value) = decision.as_str() {
                        value.to_string()
                    } else if let Some(obj) = decision.as_object() {
                        obj.keys().next()?.to_string()
                    } else {
                        return None;
                    };
                    let label = match key.as_str() {
                        "acceptForSession" => "Accept for session".to_string(),
                        "accept" => "Accept".to_string(),
                        "decline" => "Decline".to_string(),
                        "cancel" => "Cancel".to_string(),
                        "acceptWithExecpolicyAmendment" => {
                            "Accept with policy amendment".to_string()
                        }
                        "applyNetworkPolicyAmendment" => {
                            "Apply network policy amendment".to_string()
                        }
                        other => other.to_string(),
                    };
                    let payload = match key.as_str() {
                        "acceptWithExecpolicyAmendment" => serde_json::json!({
                            "decision": {
                                "acceptWithExecpolicyAmendment": {
                                    "execpolicy_amendment": params
                                        .get("proposedExecpolicyAmendment")
                                        .cloned()
                                        .unwrap_or_else(|| serde_json::json!([]))
                                }
                            }
                        }),
                        "applyNetworkPolicyAmendment" => serde_json::json!({
                            "decision": {
                                "applyNetworkPolicyAmendment": {
                                    "network_policy_amendment": {
                                        "host": params
                                            .get("networkApprovalContext")
                                            .and_then(|value| value.get("host"))
                                            .and_then(serde_json::Value::as_str)
                                            .unwrap_or(""),
                                        "action": "allow"
                                    }
                                }
                            }
                        }),
                        _ => serde_json::json!({ "decision": decision.clone() }),
                    };
                    Some(crate::services::enzim_agent::LoopPendingRequestOption {
                        id: key,
                        label,
                        payload,
                    })
                })
                .collect::<Vec<_>>()
        };
        if options.is_empty() {
            return None;
        }
        if let Some(loop_record) =
            crate::services::enzim_agent::active_loop_for_remote_thread(db.as_ref(), remote_thread_id)
        {
            let request_key = format!("request:{request_id}");
            if db
                .list_enzim_agent_loop_events(loop_record.id)
                .ok()
                .into_iter()
                .flatten()
                .any(|event| {
                    event.event_kind == "agent_request_response"
                        && event.external_turn_id.as_deref() == Some(request_key.as_str())
                })
            {
                return None;
            }
        }
        Some(crate::services::enzim_agent::LoopPendingRequest {
            request_id,
            method,
            title,
            details,
            options,
        })
    })
};

let handle_loop_action: Rc<dyn Fn(String, crate::services::enzim_agent::LoopDriverAction)> = {
    let db = db.clone();
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    let enzim_agent_question_label = enzim_agent_question_label.clone();
    let enzim_agent_summary_label = enzim_agent_summary_label.clone();
    let dispatch_enzim_message = dispatch_enzim_message.clone();
    let loop_still_active_for_thread = loop_still_active_for_thread.clone();
    let resolve_client_for_thread = resolve_client_for_thread.clone();
    Rc::new(move |remote_thread_id, action| match action {
        crate::services::enzim_agent::LoopDriverAction::Continue {
            loop_id,
            event_id,
            message,
        } => {
            if !loop_still_active_for_thread(&remote_thread_id, loop_id) {
                return;
            }
            if let Err(err) = dispatch_enzim_message(
                remote_thread_id.clone(),
                message,
                Some("Enzim Agent".to_string()),
                Some(loop_id),
                Some(event_id),
                false,
            ) {
                let _ = crate::services::enzim_agent::record_dispatch_error(
                    db.as_ref(),
                    loop_id,
                    &err,
                );
                if active_thread_id.borrow().as_deref() == Some(remote_thread_id.as_str()) {
                    enzim_agent_status.set_text(&err);
                }
            }
        }
        crate::services::enzim_agent::LoopDriverAction::AskUser { loop_id, question } => {
            if !loop_still_active_for_thread(&remote_thread_id, loop_id) {
                return;
            }
            let mut telegram_error: Option<String> = None;
            if thread_setting_value(&db, &remote_thread_id, "enzim_agent_telegram_questions")
                .as_deref()
                == Some("1")
            {
                if let Err(err) = crate::services::enzim_agent::start_telegram_question_session(
                    db.as_ref(),
                    loop_id,
                    &remote_thread_id,
                    &question,
                ) {
                    telegram_error =
                        Some(format!("{err} The question is still waiting in the popup."));
                }
            }
            if active_thread_id.borrow().as_deref() == Some(remote_thread_id.as_str()) {
                enzim_agent_status.set_text(telegram_error.as_deref().unwrap_or(""));
                if !question.trim().is_empty() {
                    enzim_agent_question_label.set_text(&question);
                }
            }
        }
        crate::services::enzim_agent::LoopDriverAction::Finish { loop_id, summary } => {
            if !loop_still_active_for_thread(&remote_thread_id, loop_id) {
                return;
            }
            if active_thread_id.borrow().as_deref() == Some(remote_thread_id.as_str()) {
                enzim_agent_status.set_text("Loop finished.");
                enzim_agent_summary_label.set_text(&summary);
                enzim_agent_summary_label.set_visible(!summary.trim().is_empty());
            }
        }
        crate::services::enzim_agent::LoopDriverAction::Respond {
            loop_id,
            request_id,
            option_label,
            payload,
        } => {
            if !loop_still_active_for_thread(&remote_thread_id, loop_id) {
                return;
            }
            let Some(client) = resolve_client_for_thread(&remote_thread_id) else {
                if active_thread_id.borrow().as_deref() == Some(remote_thread_id.as_str()) {
                    enzim_agent_status.set_text(
                        "Runtime backend is not available to answer the pending request.",
                    );
                }
                return;
            };
            if active_thread_id.borrow().as_deref() == Some(remote_thread_id.as_str()) {
                enzim_agent_status.set_text(&format!("Enzim selected: {option_label}"));
            }
            std::thread::spawn(move || {
                if let Err(err) = client.respond_to_server_request(request_id, payload) {
                    eprintln!(
                        "failed to send Enzim request response thread={} request_id={}: {}",
                        remote_thread_id, request_id, err
                    );
                }
            });
        }
    })
};

let submit_loop_answer_fn: Rc<dyn Fn(String, String, String) -> Result<(), String>> = {
    let db = db.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    let enzim_agent_in_flight_loops = enzim_agent_in_flight_loops.clone();
    let enzim_agent_worker_tx = enzim_agent_worker_tx.clone();
    Rc::new(move |remote_thread_id, answer, source| {
        let loop_record =
            crate::services::enzim_agent::active_loop_for_remote_thread(db.as_ref(), &remote_thread_id)
                .ok_or_else(|| "No active Enzim Agent loop for this thread.".to_string())?;
        if loop_record.status != "waiting_user" {
            return Err("Enzim Agent is not waiting for your answer on this thread.".to_string());
        }
        if !enzim_agent_in_flight_loops.borrow_mut().insert(loop_record.id) {
            return Err("Enzim Agent is already processing this loop.".to_string());
        }
        enzim_agent_status.set_text("Enzim Agent is processing your answer.");
        let worker_tx = enzim_agent_worker_tx.clone();
        std::thread::spawn(move || {
            let detached_db = match AppDb::open_detached() {
                Ok(db) => db,
                Err(err) => {
                    let _ = worker_tx.send((
                        remote_thread_id,
                        loop_record.id,
                        None,
                        Some(err.to_string()),
                    ));
                    return;
                }
            };
            match crate::services::enzim_agent::process_user_answer(
                &detached_db,
                &remote_thread_id,
                &answer,
                &source,
            ) {
                Ok(action) => {
                    let _ = worker_tx.send((remote_thread_id, loop_record.id, Some(action), None));
                }
                Err(err) => {
                    let _ = worker_tx.send((remote_thread_id, loop_record.id, None, Some(err)));
                }
            }
        });
        Ok(())
    })
};
submit_loop_answer.replace(Some(submit_loop_answer_fn.clone()));

let open_enzim_settings: Rc<dyn Fn()> = {
    let db = db.clone();
    let manager = manager.clone();
    let enzim_agent_button = enzim_agent_button.clone();
    Rc::new(move || {
        let parent = enzim_agent_button
            .root()
            .and_then(|root| root.downcast::<gtk::Window>().ok());
        crate::ui::components::settings_dialog::show(
            parent.as_ref(),
            db.clone(),
            manager.clone(),
            crate::ui::components::settings_dialog::SettingsPage::EnzimAgent,
        );
    })
};

let open_enzim_turn_details: Rc<dyn Fn()> = {
    let db = db.clone();
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_turn_details = enzim_agent_turn_details.clone();
    Rc::new(move || {
        let Some(remote_thread_id) = active_thread_id.borrow().clone() else {
            return;
        };
        let Some(local_thread_id) = db
            .get_thread_record_by_remote_thread_id(&remote_thread_id)
            .ok()
            .flatten()
            .map(|thread| thread.id)
        else {
            return;
        };
        let Some(loop_record) = db
            .latest_enzim_agent_loop_for_local_thread(local_thread_id)
            .ok()
            .flatten()
        else {
            return;
        };
        let events = db
            .list_enzim_agent_loop_events(loop_record.id)
            .unwrap_or_default();

        let parent = enzim_agent_turn_details
            .root()
            .and_then(|root| root.downcast::<gtk::Window>().ok());
        let dialog = gtk::Window::builder()
            .title("Loop Turn Details")
            .default_width(760)
            .default_height(680)
            .modal(true)
            .build();
        if let Some(parent) = parent.as_ref() {
            dialog.set_transient_for(Some(parent));
        }
        dialog.add_css_class("settings-window");
        dialog.add_css_class("enzim-loop-details-window");

        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.set_margin_start(14);
        root.set_margin_end(14);
        root.set_margin_top(14);
        root.set_margin_bottom(14);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let title = gtk::Label::new(Some("Loop Turn Details"));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.add_css_class("composer-enzim-agent-title");
        header.append(&title);

        let header_meta = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header_meta.set_halign(gtk::Align::End);

        let status_text = match loop_record.status.as_str() {
            "waiting_runtime" => "Waiting For Agent",
            "evaluating" => "Enzim Evaluating",
            "waiting_user" => "Waiting For User",
            "finished" => "Finished",
            "cancelled" => "Stopped",
            "paused_error" => "Paused On Error",
            "active" => "Running",
            other => other,
        };
        let status_chip = gtk::Label::new(Some(status_text));
        status_chip.add_css_class("enzim-loop-details-pill");
        status_chip.add_css_class("enzim-loop-details-pill-strong");
        header_meta.append(&status_chip);

        let backend_chip = gtk::Label::new(Some(&loop_record.backend_kind));
        backend_chip.add_css_class("enzim-loop-details-pill");
        header_meta.append(&backend_chip);

        let iteration_chip =
            gtk::Label::new(Some(&format!("{} iterations", loop_record.iteration_count)));
        iteration_chip.add_css_class("enzim-loop-details-pill");
        header_meta.append(&iteration_chip);

        let event_chip = gtk::Label::new(Some(&format!("{} events", events.len())));
        event_chip.add_css_class("enzim-loop-details-pill");
        header_meta.append(&event_chip);

        header.append(&header_meta);
        root.append(&header);

        let summary_card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        summary_card.add_css_class("enzim-loop-details-summary-card");

        let setup_toggle = gtk::Button::with_label("Loop Setup ▸");
        setup_toggle.set_has_frame(false);
        setup_toggle.set_halign(gtk::Align::Start);
        setup_toggle.add_css_class("app-flat-button");
        setup_toggle.add_css_class("enzim-loop-details-section-toggle");
        summary_card.append(&setup_toggle);

        let setup_revealer = gtk::Revealer::new();
        setup_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        setup_revealer.set_reveal_child(false);
        let setup_box = gtk::Box::new(gtk::Orientation::Vertical, 8);

        let prompt_title = gtk::Label::new(Some("Prompt"));
        prompt_title.set_xalign(0.0);
        prompt_title.add_css_class("composer-enzim-agent-question-title");
        setup_box.append(&prompt_title);
        let prompt_value = gtk::Label::new(Some(&loop_record.prompt_text));
        prompt_value.set_xalign(0.0);
        prompt_value.set_wrap(true);
        prompt_value.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        prompt_value.add_css_class("composer-enzim-agent-brief-card");
        setup_box.append(&prompt_value);

        let instructions_title = gtk::Label::new(Some("Looping Instructions"));
        instructions_title.set_xalign(0.0);
        instructions_title.add_css_class("composer-enzim-agent-question-title");
        setup_box.append(&instructions_title);
        let instructions_value = gtk::Label::new(Some(&loop_record.instructions_text));
        instructions_value.set_xalign(0.0);
        instructions_value.set_wrap(true);
        instructions_value.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        instructions_value.add_css_class("composer-enzim-agent-brief-card");
        setup_box.append(&instructions_value);
        setup_revealer.set_child(Some(&setup_box));
        summary_card.append(&setup_revealer);
        {
            let setup_revealer = setup_revealer.clone();
            let setup_toggle_for_click = setup_toggle.clone();
            setup_toggle.connect_clicked(move |_| {
                let next = !setup_revealer.reveals_child();
                setup_revealer.set_reveal_child(next);
                setup_toggle_for_click.set_label(if next {
                    "Loop Setup ▾"
                } else {
                    "Loop Setup ▸"
                });
            });
        }

        if let Some(summary) = loop_record
            .final_summary_text
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let final_title = gtk::Label::new(Some("Final Summary"));
            final_title.set_xalign(0.0);
            final_title.add_css_class("composer-enzim-agent-question-title");
            summary_card.append(&final_title);

            let final_value = gtk::Label::new(Some(summary));
            final_value.set_xalign(0.0);
            final_value.set_wrap(true);
            final_value.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            final_value.add_css_class("composer-enzim-agent-question");
            summary_card.append(&final_value);
        }
        root.append(&summary_card);

        let timeline_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build();
        timeline_scroll.set_has_frame(false);
        timeline_scroll.add_css_class("enzim-loop-details-scroll");

        let timeline_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
        timeline_box.add_css_class("enzim-loop-details-timeline");
        timeline_box.set_margin_end(4);

        for (idx, event) in events.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("enzim-loop-details-row");

            let rail = gtk::Box::new(gtk::Orientation::Vertical, 0);
            rail.add_css_class("enzim-loop-details-rail");
            rail.set_valign(gtk::Align::Fill);

            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.set_size_request(12, 12);
            dot.add_css_class("enzim-loop-details-dot");
            dot.set_halign(gtk::Align::Center);
            rail.append(&dot);

            if idx + 1 < events.len() {
                let line = gtk::Box::new(gtk::Orientation::Vertical, 0);
                line.set_vexpand(true);
                line.set_halign(gtk::Align::Center);
                line.set_width_request(2);
                line.add_css_class("enzim-loop-details-line");
                rail.append(&line);
            }

            row.append(&rail);

            let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
            card.set_hexpand(true);
            card.add_css_class("enzim-loop-details-card");

            let runtime_detail_text = if event.event_kind == "runtime_error" {
                event.external_turn_id.as_deref().and_then(|turn_id| {
                    crate::services::enzim_agent::detailed_runtime_error_for_turn(
                        db.as_ref(),
                        &remote_thread_id,
                        local_thread_id,
                        turn_id,
                    )
                })
            } else {
                None
            };

            let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let kind_text = match event.event_kind.as_str() {
                "initial_prompt" => "Initial Prompt",
                "assistant_reply" => "Coding Agent Reply",
                "agent_followup" => "Enzim Follow-up",
                "agent_question" => "Question To User",
                "runtime_request" => "Runtime Approval Request",
                "agent_request_response" => "Enzim Request Decision",
                "user_answer" => "User Answer",
                "agent_finish" => "Loop Finished",
                "runtime_error" => "Runtime Error",
                other => other,
            };
            let kind_label = gtk::Label::new(Some(kind_text));
            kind_label.set_xalign(0.0);
            kind_label.set_hexpand(true);
            kind_label.add_css_class("enzim-loop-details-kind");
            header.append(&kind_label);

            let meta = if let Some(turn_id) = event.external_turn_id.as_deref() {
                format!(
                    "#{}  •  {}  •  {}  •  {}",
                    event.sequence_no,
                    event.author_kind,
                    enzimcoder::data::format_relative_age(event.created_at),
                    &turn_id.chars().take(8).collect::<String>()
                )
            } else {
                format!(
                    "#{}  •  {}  •  {}",
                    event.sequence_no,
                    event.author_kind,
                    enzimcoder::data::format_relative_age(event.created_at)
                )
            };
            let meta_label = gtk::Label::new(Some(&meta));
            meta_label.set_xalign(1.0);
            meta_label.add_css_class("enzim-loop-details-meta");
            header.append(&meta_label);
            card.append(&header);

            let display_text = runtime_detail_text
                .as_deref()
                .or(event.full_text.as_deref())
                .unwrap_or("")
                .trim()
                .to_string();
            if !display_text.trim().is_empty() {
                let truncated_text = {
                    let limit = 260usize;
                    let mut preview = String::new();
                    for (idx, ch) in display_text.chars().enumerate() {
                        if idx >= limit {
                            preview.push('…');
                            break;
                        }
                        preview.push(ch);
                    }
                    preview
                };
                let is_truncated = truncated_text != display_text;
                let compact_label = gtk::Label::new(Some(&truncated_text));
                compact_label.set_xalign(0.0);
                compact_label.set_wrap(true);
                compact_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                compact_label.add_css_class("enzim-loop-details-compact");
                card.append(&compact_label);
                if is_truncated {
                    let expand_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    expand_row.set_halign(gtk::Align::Center);
                    expand_row.add_css_class("enzim-loop-details-inline-toggle-row");
                    let expand_toggle = gtk::Button::builder()
                        .icon_name("disclose-arrow-down-symbolic")
                        .build();
                    expand_toggle.set_has_frame(false);
                    expand_toggle.add_css_class("app-flat-button");
                    expand_toggle.add_css_class("enzim-loop-details-inline-toggle");
                    expand_row.append(&expand_toggle);
                    card.append(&expand_row);

                    let compact_label_for_click = compact_label.clone();
                    let display_text_for_click = display_text.clone();
                    let truncated_text_for_click = truncated_text.clone();
                    let expand_toggle_for_click = expand_toggle.clone();
                    let expanded = std::rc::Rc::new(std::cell::RefCell::new(false));
                    let expanded_for_click = expanded.clone();
                    expand_toggle.connect_clicked(move |_| {
                        let next = !*expanded_for_click.borrow();
                        expanded_for_click.replace(next);
                        compact_label_for_click.set_text(if next {
                            &display_text_for_click
                        } else {
                            &truncated_text_for_click
                        });
                        expand_toggle_for_click.set_icon_name(if next {
                            "disclose-arrow-up-symbolic"
                        } else {
                            "disclose-arrow-down-symbolic"
                        });
                    });
                }
            }

            if let Some(decision_json) = event
                .decision_json
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                let json_expander = gtk::Expander::new(Some("Decision JSON"));
                json_expander.add_css_class("enzim-loop-details-json-expander");
                let pretty_decision = serde_json::from_str::<serde_json::Value>(decision_json)
                    .ok()
                    .and_then(|value| serde_json::to_string_pretty(&value).ok())
                    .unwrap_or_else(|| decision_json.to_string());
                let decision_label = gtk::Label::new(Some(&pretty_decision));
                decision_label.set_xalign(0.0);
                decision_label.set_wrap(true);
                decision_label.set_wrap_mode(gtk::pango::WrapMode::Char);
                decision_label.set_selectable(true);
                decision_label.add_css_class("enzim-loop-details-json");
                json_expander.set_child(Some(&decision_label));
                card.append(&json_expander);
            }

            row.append(&card);
            timeline_box.append(&row);
        }

        timeline_scroll.set_child(Some(&timeline_box));
        root.append(&timeline_scroll);

        dialog.set_child(Some(&root));
        dialog.present();
    })
};

{
    let db = db.clone();
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_popover = enzim_agent_popover.clone();
    let open_enzim_settings = open_enzim_settings.clone();
    enzim_agent_button.connect_clicked(move |_| {
        if enzim_agent_popover.is_visible() {
            enzim_agent_popover.popdown();
        } else {
            let has_active_loop = active_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| {
                    db.get_thread_record_by_remote_thread_id(thread_id)
                        .ok()
                        .flatten()
                        .map(|thread| thread.id)
                })
                .and_then(|thread_id| {
                    db.active_enzim_agent_loop_for_local_thread(thread_id)
                        .ok()
                        .flatten()
                })
                .is_some();
            let config = crate::services::enzim_agent::load_config(db.as_ref());
            let api_key_missing = config
                .api_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none();
            if api_key_missing && !has_active_loop {
                open_enzim_settings();
                return;
            }
            enzim_agent_popover.popup();
        }
    });
}

{
    let open_enzim_settings = open_enzim_settings.clone();
    enzim_agent_settings.connect_clicked(move |_| {
        open_enzim_settings();
    });
}

{
    let open_enzim_turn_details = open_enzim_turn_details.clone();
    enzim_agent_turn_details.connect_clicked(move |_| {
        open_enzim_turn_details();
    });
}

{
    let db = db.clone();
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_prompt_view = enzim_agent_prompt_view.clone();
    let enzim_agent_instructions_view = enzim_agent_instructions_view.clone();
    let enzim_agent_dismissed_finished_loops = enzim_agent_dismissed_finished_loops.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    let enzim_agent_popover = enzim_agent_popover.clone();
    let dispatch_enzim_message = dispatch_enzim_message.clone();
    let selected_access_mode = selected_access_mode.clone();
    let thread_locked = thread_locked.clone();
    enzim_agent_loop_toggle.connect_clicked(move |_| {
        let Some(remote_thread_id) = active_thread_id.borrow().clone() else {
            enzim_agent_status.set_text("Select a thread first.");
            return;
        };
        let Some(local_thread_id) = db
            .get_thread_record_by_remote_thread_id(&remote_thread_id)
            .ok()
            .flatten()
            .map(|thread| thread.id)
        else {
            enzim_agent_status.set_text("Thread record not found.");
            return;
        };
        let has_active_loop = db
            .active_enzim_agent_loop_for_local_thread(local_thread_id)
            .ok()
            .flatten()
            .is_some();
        if has_active_loop {
            match crate::services::enzim_agent::cancel_active_loop_for_local_thread(
                db.as_ref(),
                local_thread_id,
            ) {
                Ok(()) => enzim_agent_status.set_text("Loop stopped."),
                Err(err) => enzim_agent_status.set_text(&err),
            }
            return;
        }
        if let Some(latest_loop) = db
            .latest_enzim_agent_loop_for_local_thread(local_thread_id)
            .ok()
            .flatten()
        {
            let finished_screen_visible = latest_loop.status == "finished"
                && enzim_agent_dismissed_finished_loops
                    .borrow()
                    .get(&remote_thread_id)
                    .copied()
                    != Some(latest_loop.id);
            if finished_screen_visible {
                enzim_agent_dismissed_finished_loops
                    .borrow_mut()
                    .insert(remote_thread_id.clone(), latest_loop.id);
                enzim_agent_status.set_text("");
                return;
            }
        }
        if *thread_locked.borrow() {
            enzim_agent_status
                .set_text("This thread is locked to another account. Start a new thread to use Enzim Agent.");
            return;
        }
        if super::codex_runtime::active_turn_for_thread(&remote_thread_id).is_some() {
            enzim_agent_status.set_text(
                "Wait for the current turn to finish before starting an Enzim Agent loop.",
            );
            return;
        }
        if selected_access_mode.borrow().as_str() != "dangerFullAccess" {
            enzim_agent_status.set_text(
                "Enzim Agent looping requires Full access. Switch the composer permission level to Full access first.",
            );
            return;
        }

        let prompt_buffer = enzim_agent_prompt_view.buffer();
        let prompt = prompt_buffer
            .text(&prompt_buffer.start_iter(), &prompt_buffer.end_iter(), true)
            .to_string();
        let instructions_buffer = enzim_agent_instructions_view.buffer();
        let instructions = instructions_buffer
            .text(
                &instructions_buffer.start_iter(),
                &instructions_buffer.end_iter(),
                true,
            )
            .to_string();
        match crate::services::enzim_agent::start_loop(
            db.as_ref(),
            local_thread_id,
            &prompt,
            &instructions,
        ) {
            Ok(loop_record) => {
                enzim_agent_dismissed_finished_loops
                    .borrow_mut()
                    .remove(&remote_thread_id);
                enzim_agent_status.set_text("Loop started.");
                if let Err(err) = dispatch_enzim_message(
                    remote_thread_id,
                    prompt,
                    None,
                    Some(loop_record.id),
                    None,
                    true,
                ) {
                    let _ = crate::services::enzim_agent::record_dispatch_error(
                        db.as_ref(),
                        loop_record.id,
                        &err,
                    );
                    enzim_agent_status.set_text(&err);
                    return;
                }
                enzim_agent_popover.popdown();
            }
            Err(err) => enzim_agent_status.set_text(&err),
        }
    });
}

{
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_answer_view = enzim_agent_answer_view.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    let enzim_agent_popover = enzim_agent_popover.clone();
    let submit_loop_answer_fn = submit_loop_answer_fn.clone();
    enzim_agent_answer_submit.connect_clicked(move |_| {
        let Some(remote_thread_id) = active_thread_id.borrow().clone() else {
            enzim_agent_status.set_text("Select a thread first.");
            return;
        };
        let buffer = enzim_agent_answer_view.buffer();
        let answer = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), true)
            .to_string();
        if answer.trim().is_empty() {
            enzim_agent_status.set_text("Answer is empty.");
            return;
        }
        match submit_loop_answer_fn(remote_thread_id, answer, "popup".to_string()) {
            Ok(()) => {
                buffer.set_text("");
                enzim_agent_popover.popdown();
            }
            Err(err) => enzim_agent_status.set_text(&err),
        }
    });
}

{
    let db = db.clone();
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_dismissed_finished_loops = enzim_agent_dismissed_finished_loops.clone();
    let enzim_agent_button = enzim_agent_button.clone();
    let enzim_agent_popover = enzim_agent_popover.clone();
    let enzim_agent_header_note = enzim_agent_header_note.clone();
    let enzim_agent_header_status = enzim_agent_header_status.clone();
    let enzim_agent_header_backend = enzim_agent_header_backend.clone();
    let enzim_agent_header_iterations = enzim_agent_header_iterations.clone();
    let enzim_agent_header_errors = enzim_agent_header_errors.clone();
    let enzim_agent_loop_prompt_title = enzim_agent_loop_prompt_title.clone();
    let enzim_agent_prompt_scroll = enzim_agent_prompt_scroll.clone();
    let enzim_agent_prompt_view = enzim_agent_prompt_view.clone();
    let enzim_agent_loop_instructions_row = enzim_agent_loop_instructions_row.clone();
    let enzim_agent_instructions_revealer = enzim_agent_instructions_revealer.clone();
    let enzim_agent_instructions_scroll = enzim_agent_instructions_scroll.clone();
    let enzim_agent_instructions_view = enzim_agent_instructions_view.clone();
    let enzim_agent_telegram_questions_row = enzim_agent_telegram_questions_row.clone();
    let enzim_agent_running_box = enzim_agent_running_box.clone();
    let enzim_agent_running_state_value = enzim_agent_running_state_value.clone();
    let enzim_agent_running_backend_value = enzim_agent_running_backend_value.clone();
    let enzim_agent_running_iterations_value = enzim_agent_running_iterations_value.clone();
    let enzim_agent_running_errors_value = enzim_agent_running_errors_value.clone();
    let enzim_agent_running_prompt_value = enzim_agent_running_prompt_value.clone();
    let enzim_agent_running_instructions_value = enzim_agent_running_instructions_value.clone();
    let enzim_agent_finished_box = enzim_agent_finished_box.clone();
    let enzim_agent_finished_backend_value = enzim_agent_finished_backend_value.clone();
    let enzim_agent_finished_iterations_value = enzim_agent_finished_iterations_value.clone();
    let enzim_agent_finished_errors_value = enzim_agent_finished_errors_value.clone();
    let enzim_agent_finished_summary = enzim_agent_finished_summary.clone();
    let enzim_agent_idle_label = enzim_agent_idle_label.clone();
    let enzim_agent_question_box = enzim_agent_question_box.clone();
    let enzim_agent_question_label = enzim_agent_question_label.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    let enzim_agent_answer_submit = enzim_agent_answer_submit.clone();
    let enzim_agent_loop_toggle = enzim_agent_loop_toggle.clone();
    let enzim_agent_turn_details = enzim_agent_turn_details.clone();
    let selected_access_mode = selected_access_mode.clone();
    let enzim_agent_telegram_questions_switch = enzim_agent_telegram_questions_switch.clone();
    let enzim_agent_telegram_questions_label = enzim_agent_telegram_questions_label.clone();
    let enzim_agent_loop_status = enzim_agent_loop_status.clone();
    let enzim_agent_summary_label = enzim_agent_summary_label.clone();
    let format_loop_status = format_loop_status.clone();
    let wiggle_step: Rc<RefCell<u8>> = Rc::new(RefCell::new(0));
    let wiggle_step_for_timer = wiggle_step.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(150), move || {
        if enzim_agent_button.root().is_none() {
            return gtk::glib::ControlFlow::Break;
        }

        let (latest_loop, pending_question) = active_thread_id
            .borrow()
            .as_deref()
            .and_then(|thread_id| {
                let local_thread_id = db
                    .get_thread_record_by_remote_thread_id(thread_id)
                    .ok()
                    .flatten()
                    .map(|thread| thread.id)?;
                Some((
                    db.latest_enzim_agent_loop_for_local_thread(local_thread_id)
                        .ok()
                        .flatten(),
                    crate::services::enzim_agent::pending_question(db.as_ref(), thread_id),
                ))
            })
            .unwrap_or((None, None));
        let finished_loop_dismissed = active_thread_id
            .borrow()
            .as_deref()
            .and_then(|thread_id| {
                latest_loop.as_ref().and_then(|loop_record| {
                    enzim_agent_dismissed_finished_loops
                        .borrow()
                        .get(thread_id)
                        .copied()
                        .map(|dismissed_id| dismissed_id == loop_record.id)
                })
            })
            .unwrap_or(false);

        enzim_agent_loop_status.set_text(&(format_loop_status)(latest_loop.as_ref()));
        let header_note_text = enzim_agent_status.text().to_string();
        let header_note_text = header_note_text.trim().to_string();
        enzim_agent_header_note.set_text(&header_note_text);
        enzim_agent_header_note.set_tooltip_text(if header_note_text.is_empty() {
            None
        } else {
            Some(&header_note_text)
        });
        enzim_agent_header_note.set_visible(!header_note_text.is_empty());
        enzim_agent_header_status.set_text(
            latest_loop
                .as_ref()
                .map(|loop_record| match loop_record.status.as_str() {
                    "waiting_runtime" => "Waiting For Agent",
                    "evaluating" => "Enzim Evaluating",
                    "waiting_user" => "Waiting For User",
                    "finished" => "Finished",
                    "cancelled" => "Stopped",
                    "paused_error" => "Paused",
                    "active" => "Running",
                    other => other,
                })
                .unwrap_or("Idle"),
        );
        enzim_agent_header_status.set_visible(latest_loop.is_some() && !finished_loop_dismissed);
        enzim_agent_header_backend.set_text(
            latest_loop
                .as_ref()
                .map(|loop_record| loop_record.backend_kind.as_str())
                .unwrap_or(""),
        );
        enzim_agent_header_backend.set_visible(latest_loop.is_some() && !finished_loop_dismissed);
        enzim_agent_header_iterations.set_text(
            &latest_loop
                .as_ref()
                .map(|loop_record| format!("{} iterations", loop_record.iteration_count))
                .unwrap_or_default(),
        );
        enzim_agent_header_iterations
            .set_visible(latest_loop.is_some() && !finished_loop_dismissed);
        enzim_agent_header_errors.set_text(
            &latest_loop
                .as_ref()
                .map(|loop_record| format!("{} errors", loop_record.error_count))
                .unwrap_or_default(),
        );
        enzim_agent_header_errors.set_visible(
            latest_loop
                .as_ref()
                .map(|loop_record| loop_record.error_count > 0)
                .unwrap_or(false)
                && !finished_loop_dismissed,
        );

        let latest_summary = latest_loop
            .as_ref()
            .and_then(|loop_record| loop_record.final_summary_text.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                active_thread_id
                    .borrow()
                    .as_deref()
                    .and_then(|thread_id| {
                        db.get_thread_record_by_remote_thread_id(thread_id)
                            .ok()
                            .flatten()
                            .and_then(|thread| {
                                crate::services::enzim_agent::latest_finished_summary(
                                    db.as_ref(),
                                    thread.id,
                                )
                            })
                    })
            });
        if let Some(summary) = latest_summary {
            enzim_agent_summary_label.set_text(&summary);
        } else {
            enzim_agent_summary_label.set_text("");
        }

        let has_active_loop = latest_loop.as_ref().is_some_and(|loop_record| {
            matches!(
                loop_record.status.as_str(),
                "active" | "waiting_runtime" | "evaluating" | "waiting_user"
            )
        });
        let show_finished_screen = latest_loop.as_ref().is_some_and(|loop_record| {
            loop_record.status == "finished" && !finished_loop_dismissed
        });
        let show_turn_details = latest_loop.is_some();
        enzim_agent_turn_details.set_visible(show_turn_details);

        enzim_agent_loop_prompt_title.set_visible(!has_active_loop && !show_finished_screen);
        enzim_agent_prompt_scroll.set_visible(!has_active_loop && !show_finished_screen);
        enzim_agent_loop_instructions_row.set_visible(!has_active_loop && !show_finished_screen);
        enzim_agent_instructions_revealer.set_visible(!has_active_loop && !show_finished_screen);
        enzim_agent_telegram_questions_row.set_visible(!has_active_loop && !show_finished_screen);
        enzim_agent_running_box.set_visible(has_active_loop);
        enzim_agent_finished_box.set_visible(show_finished_screen);

        enzim_agent_prompt_view.set_editable(!has_active_loop);
        enzim_agent_prompt_view.set_cursor_visible(!has_active_loop);
        enzim_agent_prompt_scroll.set_sensitive(!has_active_loop);
        enzim_agent_instructions_view.set_editable(!has_active_loop);
        enzim_agent_instructions_view.set_cursor_visible(!has_active_loop);
        enzim_agent_instructions_scroll.set_sensitive(!has_active_loop);
        let has_linked_telegram = db.remote_telegram_active_account().ok().flatten().is_some();
        enzim_agent_telegram_questions_switch
            .set_sensitive(!has_active_loop && has_linked_telegram);
        enzim_agent_telegram_questions_label.set_opacity(if has_linked_telegram { 1.0 } else { 0.6 });
        if has_linked_telegram {
            enzim_agent_telegram_questions_label.set_tooltip_text(None);
        } else {
            enzim_agent_telegram_questions_label.set_tooltip_text(Some(
                "Link a Telegram bot in Remote settings to enable Telegram questions.",
            ));
        }

        if let Some(loop_record) = latest_loop.as_ref().filter(|_| has_active_loop) {
            let state_text = match loop_record.status.as_str() {
                "waiting_runtime" => "Waiting for coding agent",
                "evaluating" => "Enzim evaluating",
                "waiting_user" => "Waiting for your answer",
                "active" => "Running",
                other => other,
            };
            enzim_agent_running_state_value.set_text(state_text);
            enzim_agent_running_backend_value.set_text(&loop_record.backend_kind);
            enzim_agent_running_iterations_value
                .set_text(&loop_record.iteration_count.to_string());
            enzim_agent_running_errors_value.set_text(&loop_record.error_count.to_string());
            enzim_agent_running_prompt_value.set_text(&loop_record.prompt_text);
            enzim_agent_running_instructions_value
                .set_text(&loop_record.instructions_text);
        } else {
            enzim_agent_running_state_value.set_text("Idle");
            enzim_agent_running_backend_value.set_text("-");
            enzim_agent_running_iterations_value.set_text("0");
            enzim_agent_running_errors_value.set_text("0");
            enzim_agent_running_prompt_value.set_text("");
            enzim_agent_running_instructions_value.set_text("");
        }
        if let Some(loop_record) = latest_loop.as_ref().filter(|_| show_finished_screen) {
            enzim_agent_finished_backend_value.set_text(&loop_record.backend_kind);
            enzim_agent_finished_iterations_value
                .set_text(&loop_record.iteration_count.to_string());
            enzim_agent_finished_errors_value.set_text(&loop_record.error_count.to_string());
            enzim_agent_finished_summary.set_text(
                loop_record
                    .final_summary_text
                    .as_deref()
                    .unwrap_or("Loop finished."),
            );
        } else {
            enzim_agent_finished_backend_value.set_text("-");
            enzim_agent_finished_iterations_value.set_text("0");
            enzim_agent_finished_errors_value.set_text("0");
            enzim_agent_finished_summary.set_text("");
        }

        let can_toggle = active_thread_id.borrow().is_some()
            && (has_active_loop || selected_access_mode.borrow().as_str() == "dangerFullAccess");
        enzim_agent_loop_toggle.set_sensitive(can_toggle);
        if has_active_loop {
            enzim_agent_loop_toggle.set_label("Stop Loop");
            enzim_agent_loop_toggle.remove_css_class("composer-enzim-agent-action-primary");
            enzim_agent_loop_toggle.add_css_class("composer-enzim-agent-action-stop");
        } else if latest_loop.is_some() && !finished_loop_dismissed {
            enzim_agent_loop_toggle.set_label("Start New");
            enzim_agent_loop_toggle.remove_css_class("composer-enzim-agent-action-stop");
            enzim_agent_loop_toggle.add_css_class("composer-enzim-agent-action-primary");
        } else {
            enzim_agent_loop_toggle.set_label("Start Loop");
            enzim_agent_loop_toggle.remove_css_class("composer-enzim-agent-action-stop");
            enzim_agent_loop_toggle.add_css_class("composer-enzim-agent-action-primary");
        }

        if let Some(pending_question) = pending_question {
            enzim_agent_idle_label.set_visible(false);
            enzim_agent_question_box.set_visible(true);
            enzim_agent_question_label.set_text(&pending_question.question);
            enzim_agent_answer_submit.set_sensitive(true);
            enzim_agent_button.set_tooltip_text(Some("Enzim Agent needs your answer"));
            enzim_agent_button.add_css_class("composer-enzim-agent-button-active");

            if !enzim_agent_popover.is_visible() {
                enzim_agent_button.add_css_class("composer-enzim-agent-button-waiting");
                let next = (*wiggle_step_for_timer.borrow()).wrapping_add(1) % 4;
                wiggle_step_for_timer.replace(next);
                match next {
                    0 => {
                        enzim_agent_button.set_margin_start(0);
                        enzim_agent_button.set_margin_end(2);
                    }
                    1 => {
                        enzim_agent_button.set_margin_start(2);
                        enzim_agent_button.set_margin_end(0);
                    }
                    2 => {
                        enzim_agent_button.set_margin_start(1);
                        enzim_agent_button.set_margin_end(1);
                    }
                    _ => {
                        enzim_agent_button.set_margin_start(0);
                        enzim_agent_button.set_margin_end(0);
                    }
                }
            } else {
                enzim_agent_button.remove_css_class("composer-enzim-agent-button-waiting");
                enzim_agent_button.set_margin_start(0);
                enzim_agent_button.set_margin_end(0);
            }
        } else {
            enzim_agent_idle_label.set_visible(!show_finished_screen);
            enzim_agent_question_box.set_visible(false);
            enzim_agent_answer_submit.set_sensitive(false);
            enzim_agent_idle_label.set_text("No Enzim Agent question is waiting on this thread.");
            enzim_agent_button.set_tooltip_text(Some("Enzim Agent"));
            enzim_agent_button.remove_css_class("composer-enzim-agent-button-active");
            enzim_agent_button.remove_css_class("composer-enzim-agent-button-waiting");
            enzim_agent_button.set_margin_start(0);
            enzim_agent_button.set_margin_end(0);
            if latest_loop
                .as_ref()
                .map(|loop_record| loop_record.status.as_str())
                != Some("paused_error")
            {
                enzim_agent_status.set_text("");
            } else if let Some(loop_record) = latest_loop.as_ref() {
                enzim_agent_status.set_text(
                    loop_record
                        .last_error_text
                        .as_deref()
                        .unwrap_or("Loop paused because of an error."),
                );
            }
        }
        enzim_agent_summary_label.set_visible(false);

        gtk::glib::ControlFlow::Continue
    });
}

{
    let db = db.clone();
    let enzim_agent_in_flight_loops = enzim_agent_in_flight_loops.clone();
    let handle_loop_action = handle_loop_action.clone();
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    let loop_still_active_for_thread = loop_still_active_for_thread.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(90), move || {
        while let Ok((remote_thread_id, loop_id, action, error)) = enzim_agent_worker_rx.try_recv() {
            enzim_agent_in_flight_loops.borrow_mut().remove(&loop_id);
            let is_stale = !loop_still_active_for_thread(&remote_thread_id, loop_id);
            if let Some(error) = error {
                if is_stale {
                    continue;
                }
                if active_thread_id.borrow().as_deref() == Some(remote_thread_id.as_str()) {
                    enzim_agent_status.set_text(&error);
                }
                let _ = crate::services::enzim_agent::record_dispatch_error(
                    db.as_ref(),
                    loop_id,
                    &error,
                );
                continue;
            }
            if let Some(action) = action {
                if is_stale {
                    continue;
                }
                handle_loop_action(remote_thread_id, action);
            }
        }
        gtk::glib::ControlFlow::Continue
    });
}

{
    let db = db.clone();
    let enzim_agent_in_flight_loops = enzim_agent_in_flight_loops.clone();
    let enzim_agent_worker_tx = enzim_agent_worker_tx.clone();
    let enzim_agent_button = enzim_agent_button.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(260), move || {
        if enzim_agent_button.root().is_none() {
            return gtk::glib::ControlFlow::Break;
        }
        let active_loops = db.list_active_enzim_agent_loops().unwrap_or_default();
        for loop_record in active_loops {
            if loop_record.status != "waiting_user" {
                continue;
            }
            let Some(remote_thread_id) = loop_record.remote_thread_id_snapshot.clone() else {
                continue;
            };
            let pending_answer = db
                .list_remote_pending_prompts_for_local_thread(loop_record.local_thread_id, 8)
                .unwrap_or_default()
                .into_iter()
                .find(|prompt| prompt.source == "telegram-loop-answer");
            let Some(pending_answer) = pending_answer else {
                continue;
            };
            if !enzim_agent_in_flight_loops
                .borrow_mut()
                .insert(loop_record.id)
            {
                continue;
            }
            let _ = db.mark_remote_pending_prompt_consumed(pending_answer.id);
            let worker_tx = enzim_agent_worker_tx.clone();
            let answer_text = pending_answer.text.clone();
            std::thread::spawn(move || {
                let detached_db = match AppDb::open_detached() {
                    Ok(db) => db,
                    Err(err) => {
                        let _ = worker_tx.send((
                            remote_thread_id,
                            loop_record.id,
                            None,
                            Some(err.to_string()),
                        ));
                        return;
                    }
                };
                match crate::services::enzim_agent::process_user_answer(
                    &detached_db,
                    &remote_thread_id,
                    &answer_text,
                    "telegram",
                ) {
                    Ok(action) => {
                        let _ = worker_tx.send((remote_thread_id, loop_record.id, Some(action), None));
                    }
                    Err(err) => {
                        let _ = worker_tx.send((remote_thread_id, loop_record.id, None, Some(err)));
                    }
                }
            });
        }
        gtk::glib::ControlFlow::Continue
    });
}

{
    let db = db.clone();
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    let loop_still_active_for_thread = loop_still_active_for_thread.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(90), move || {
        while let Ok((remote_thread_id, loop_id, event_id, turn_id, error)) =
            enzim_agent_dispatch_rx.try_recv()
        {
            let is_stale = loop_id
                .map(|loop_id| !loop_still_active_for_thread(&remote_thread_id, loop_id))
                .unwrap_or(false);
            if let Some(error) = error {
                if is_stale {
                    continue;
                }
                if let Some(loop_id) = loop_id {
                    let _ = crate::services::enzim_agent::record_dispatch_error(
                        db.as_ref(),
                        loop_id,
                        &error,
                    );
                }
                if active_thread_id.borrow().as_deref() == Some(remote_thread_id.as_str()) {
                    enzim_agent_status.set_text(&error);
                }
                continue;
            }

            if let (Some(loop_id), Some(event_id), Some(turn_id)) = (loop_id, event_id, turn_id) {
                if is_stale {
                    continue;
                }
                if let Err(err) = crate::services::enzim_agent::mark_followup_dispatched(
                    db.as_ref(),
                    loop_id,
                    event_id,
                    &remote_thread_id,
                    &turn_id,
                ) {
                    let _ = crate::services::enzim_agent::record_dispatch_error(
                        db.as_ref(),
                        loop_id,
                        &err,
                    );
                    if active_thread_id.borrow().as_deref() == Some(remote_thread_id.as_str()) {
                        enzim_agent_status.set_text(&err);
                    }
                }
            }
        }
        gtk::glib::ControlFlow::Continue
    });
}

{
    let db = db.clone();
    let enzim_agent_in_flight_loops = enzim_agent_in_flight_loops.clone();
    let enzim_agent_worker_tx = enzim_agent_worker_tx.clone();
    let pending_loop_request_for_thread = pending_loop_request_for_thread.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(320), move || {
        if enzim_agent_button.root().is_none() {
            return gtk::glib::ControlFlow::Break;
        }
        let active_loops = db.list_active_enzim_agent_loops().unwrap_or_default();
        for loop_record in active_loops {
            if loop_record.status != "waiting_runtime" && loop_record.status != "active" {
                continue;
            }
            let Some(remote_thread_id) = loop_record.remote_thread_id_snapshot.clone() else {
                continue;
            };
            if !enzim_agent_in_flight_loops
                .borrow_mut()
                .insert(loop_record.id)
            {
                continue;
            }
            let worker_tx = enzim_agent_worker_tx.clone();
            let pending_request = pending_loop_request_for_thread(&remote_thread_id);
            std::thread::spawn(move || {
                let detached_db = match AppDb::open_detached() {
                    Ok(db) => db,
                    Err(err) => {
                        let _ = worker_tx.send((
                            remote_thread_id,
                            loop_record.id,
                            None,
                            Some(err.to_string()),
                        ));
                        return;
                    }
                };
                let result = if let Some(pending_request) = pending_request.as_ref() {
                    crate::services::enzim_agent::process_pending_request(
                        &detached_db,
                        &remote_thread_id,
                        pending_request,
                    )
                    .map(|action| Some(action))
                } else {
                    crate::services::enzim_agent::process_waiting_runtime_turn(
                        &detached_db,
                        &remote_thread_id,
                    )
                    .map(|result| result.map(|processed| processed.action))
                };
                match result {
                    Ok(result) => {
                        let _ = worker_tx.send((
                            remote_thread_id,
                            loop_record.id,
                            result,
                            None,
                        ));
                    }
                    Err(err) => {
                        let _ = worker_tx.send((remote_thread_id, loop_record.id, None, Some(err)));
                    }
                }
            });
        }
        gtk::glib::ControlFlow::Continue
    });
}
}
