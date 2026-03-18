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

let handle_loop_action: Rc<dyn Fn(String, crate::services::enzim_agent::LoopDriverAction)> = {
    let db = db.clone();
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    let enzim_agent_question_label = enzim_agent_question_label.clone();
    let enzim_agent_summary_label = enzim_agent_summary_label.clone();
    let dispatch_enzim_message = dispatch_enzim_message.clone();
    Rc::new(move |remote_thread_id, action| match action {
        crate::services::enzim_agent::LoopDriverAction::Continue {
            loop_id,
            event_id,
            message,
        } => {
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
        crate::services::enzim_agent::LoopDriverAction::AskUser { question, .. } => {
            if active_thread_id.borrow().as_deref() == Some(remote_thread_id.as_str()) {
                enzim_agent_status.set_text("");
                if !question.trim().is_empty() {
                    enzim_agent_question_label.set_text(&question);
                }
            }
        }
        crate::services::enzim_agent::LoopDriverAction::Finish { summary, .. } => {
            if active_thread_id.borrow().as_deref() == Some(remote_thread_id.as_str()) {
                enzim_agent_status.set_text("Loop finished.");
                enzim_agent_summary_label.set_text(&summary);
                enzim_agent_summary_label.set_visible(!summary.trim().is_empty());
            }
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

{
    let enzim_agent_popover = enzim_agent_popover.clone();
    enzim_agent_button.connect_clicked(move |_| {
        if enzim_agent_popover.is_visible() {
            enzim_agent_popover.popdown();
        } else {
            enzim_agent_popover.popup();
        }
    });
}

{
    let enzim_agent_popover = enzim_agent_popover.clone();
    enzim_agent_cancel.connect_clicked(move |_| {
        enzim_agent_popover.popdown();
    });
}

{
    let db = db.clone();
    let manager = manager.clone();
    let enzim_agent_button = enzim_agent_button.clone();
    enzim_agent_settings.connect_clicked(move |_| {
        let parent = enzim_agent_button
            .root()
            .and_then(|root| root.downcast::<gtk::Window>().ok());
        crate::ui::components::settings_dialog::show(
            parent.as_ref(),
            db.clone(),
            manager.clone(),
            crate::ui::components::settings_dialog::SettingsPage::EnzimAgent,
        );
    });
}

{
    let db = db.clone();
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_prompt_view = enzim_agent_prompt_view.clone();
    let enzim_agent_instructions_view = enzim_agent_instructions_view.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    let enzim_agent_popover = enzim_agent_popover.clone();
    let dispatch_enzim_message = dispatch_enzim_message.clone();
    let thread_locked = thread_locked.clone();
    enzim_agent_start.connect_clicked(move |_| {
        let Some(remote_thread_id) = active_thread_id.borrow().clone() else {
            enzim_agent_status.set_text("Select a thread first.");
            return;
        };
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
        let Some(local_thread_id) = db
            .get_thread_record_by_remote_thread_id(&remote_thread_id)
            .ok()
            .flatten()
            .map(|thread| thread.id)
        else {
            enzim_agent_status.set_text("Thread record not found.");
            return;
        };

        match crate::services::enzim_agent::start_loop(
            db.as_ref(),
            local_thread_id,
            &prompt,
            &instructions,
        ) {
            Ok(loop_record) => {
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
    let db = db.clone();
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    enzim_agent_stop.connect_clicked(move |_| {
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
        match crate::services::enzim_agent::cancel_active_loop_for_local_thread(
            db.as_ref(),
            local_thread_id,
        ) {
            Ok(()) => enzim_agent_status.set_text("Loop stopped."),
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
    let enzim_agent_button = enzim_agent_button.clone();
    let enzim_agent_popover = enzim_agent_popover.clone();
    let enzim_agent_idle_label = enzim_agent_idle_label.clone();
    let enzim_agent_question_box = enzim_agent_question_box.clone();
    let enzim_agent_question_label = enzim_agent_question_label.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    let enzim_agent_answer_submit = enzim_agent_answer_submit.clone();
    let enzim_agent_start = enzim_agent_start.clone();
    let enzim_agent_stop = enzim_agent_stop.clone();
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

        enzim_agent_loop_status.set_text(&(format_loop_status)(latest_loop.as_ref()));

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
            enzim_agent_summary_label.set_visible(true);
        } else {
            enzim_agent_summary_label.set_text("");
            enzim_agent_summary_label.set_visible(false);
        }

        let has_active_loop = latest_loop.as_ref().is_some_and(|loop_record| {
            matches!(
                loop_record.status.as_str(),
                "active" | "waiting_runtime" | "evaluating" | "waiting_user"
            )
        });
        enzim_agent_start
            .set_sensitive(active_thread_id.borrow().is_some() && !has_active_loop);
        enzim_agent_stop.set_sensitive(has_active_loop);

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
            enzim_agent_idle_label.set_visible(true);
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

        gtk::glib::ControlFlow::Continue
    });
}

{
    let db = db.clone();
    let enzim_agent_in_flight_loops = enzim_agent_in_flight_loops.clone();
    let handle_loop_action = handle_loop_action.clone();
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(90), move || {
        while let Ok((remote_thread_id, loop_id, action, error)) = enzim_agent_worker_rx.try_recv() {
            enzim_agent_in_flight_loops.borrow_mut().remove(&loop_id);
            if let Some(error) = error {
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
                handle_loop_action(remote_thread_id, action);
            }
        }
        gtk::glib::ControlFlow::Continue
    });
}

{
    let db = db.clone();
    let active_thread_id = active_thread_id.clone();
    let enzim_agent_status = enzim_agent_status.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(90), move || {
        while let Ok((remote_thread_id, loop_id, event_id, turn_id, error)) =
            enzim_agent_dispatch_rx.try_recv()
        {
            if let Some(error) = error {
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
    gtk::glib::timeout_add_local(Duration::from_millis(320), move || {
        if enzim_agent_button.root().is_none() {
            return gtk::glib::ControlFlow::Break;
        }
        let active_loops = db.list_active_enzim_agent_loops().unwrap_or_default();
        for loop_record in active_loops {
            if loop_record.status != "waiting_runtime" {
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
                match crate::services::enzim_agent::process_waiting_runtime_turn(
                    &detached_db,
                    &remote_thread_id,
                ) {
                    Ok(result) => {
                        let _ = worker_tx.send((
                            remote_thread_id,
                            loop_record.id,
                            result.map(|processed| processed.action),
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
