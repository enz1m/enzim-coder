fn create_inline_approval_card(
    title: &str,
    details: &str,
    method: &str,
    params: &Value,
    options: &[Value],
    on_submit: Rc<dyn Fn(Value, String)>,
) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("chat-command-card");
    card.add_css_class("chat-approval-card");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let details_label = gtk::Label::new(Some(details));
    details_label.set_xalign(0.0);
    details_label.set_wrap(true);
    details_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    details_label.add_css_class("chat-approval-details");
    let title_label = gtk::Label::new(Some(title));
    title_label.set_xalign(0.0);
    title_label.add_css_class("chat-command-section-title");
    root.append(&title_label);
    root.append(&details_label);

    let has_execpolicy = options
        .iter()
        .filter_map(decision_key)
        .any(|key| key == "acceptWithExecpolicyAmendment");
    let has_network_policy = options
        .iter()
        .filter_map(decision_key)
        .any(|key| key == "applyNetworkPolicyAmendment")
        || params.get("networkApprovalContext").is_some();
    let has_questions = method == "item/tool/requestUserInput"
        && params.get("questions").and_then(Value::as_array).is_some();

    let execpolicy_entry = gtk::Entry::new();
    if has_execpolicy {
        let proposed = params
            .get("proposedExecpolicyAmendment")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        execpolicy_entry.set_placeholder_text(Some("execpolicy amendment command prefix"));
        execpolicy_entry.set_text(&proposed);
        execpolicy_entry.set_editable(false);
        execpolicy_entry.set_can_focus(false);
        execpolicy_entry.add_css_class("chat-approval-command-preview");
        root.append(&execpolicy_entry);
    }

    let network_host_entry = gtk::Entry::new();
    let network_action = gtk::DropDown::from_strings(&["allow", "deny"]);
    if has_network_policy {
        let host = params
            .get("networkApprovalContext")
            .and_then(|v| v.get("host"))
            .and_then(Value::as_str)
            .unwrap_or("");
        network_host_entry.set_placeholder_text(Some("network policy host"));
        network_host_entry.set_text(host);
        root.append(&network_host_entry);
        root.append(&network_action);
    }

    let mut question_inputs: Vec<(String, gtk::DropDown, Vec<String>, gtk::Entry)> = Vec::new();
    if has_questions {
        if let Some(questions) = params.get("questions").and_then(Value::as_array) {
            for question in questions {
                let qid = question
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("question")
                    .to_string();
                let prompt = question
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or("Choose an option");
                let prompt_label = gtk::Label::new(Some(prompt));
                prompt_label.set_xalign(0.0);
                prompt_label.set_wrap(true);
                prompt_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                root.append(&prompt_label);

                let mut option_labels = Vec::<String>::new();
                if let Some(options) = question.get("options").and_then(Value::as_array) {
                    for option in options {
                        let label = option
                            .get("label")
                            .and_then(Value::as_str)
                            .unwrap_or("Option")
                            .to_string();
                        option_labels.push(label);
                    }
                }
                if option_labels.is_empty() {
                    option_labels.push("Accept".to_string());
                    option_labels.push("Decline".to_string());
                }
                option_labels.push("Other".to_string());
                let string_refs = option_labels.iter().map(String::as_str).collect::<Vec<_>>();
                let dropdown = gtk::DropDown::from_strings(&string_refs);
                dropdown.set_selected(0);
                root.append(&dropdown);

                let other_entry = gtk::Entry::new();
                other_entry.set_placeholder_text(Some("Other response"));
                other_entry.set_visible(false);
                root.append(&other_entry);

                {
                    let other_entry = other_entry.clone();
                    let options_len = option_labels.len();
                    dropdown.connect_selected_notify(move |dd| {
                        other_entry.set_visible((dd.selected() as usize) + 1 == options_len);
                    });
                }

                question_inputs.push((qid, dropdown, option_labels, other_entry));
            }
        }
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);

    if has_questions {
        let submit = gtk::Button::with_label("Submit");
        let on_submit = on_submit.clone();
        submit.connect_clicked(move |_| {
            let mut answers = serde_json::Map::<String, Value>::new();
            for (qid, dropdown, labels, other_entry) in &question_inputs {
                let selected_idx = dropdown.selected() as usize;
                let chosen = labels
                    .get(selected_idx)
                    .cloned()
                    .unwrap_or_else(|| "Other".to_string());
                let value = if chosen == "Other" {
                    other_entry.text().to_string()
                } else {
                    chosen
                };
                answers.insert(qid.clone(), json!({ "answers": [value] }));
            }
            on_submit(json!({ "answers": answers }), "Submit".to_string());
        });
        actions.append(&submit);
    } else {
        for decision_payload in options {
            let label = approval_decision_label(decision_payload);
            let key = decision_key(decision_payload).unwrap_or_default();
            let decision_payload = decision_payload.clone();
            let on_submit = on_submit.clone();
            let execpolicy_entry = execpolicy_entry.clone();
            let network_host_entry = network_host_entry.clone();
            let network_action = network_action.clone();
            let button = gtk::Button::with_label(&label);
            button.connect_clicked(move |_| {
                let final_decision = if key == "acceptWithExecpolicyAmendment" {
                    let tokens = parse_execpolicy_tokens(execpolicy_entry.text().as_str());
                    json!({
                        "acceptWithExecpolicyAmendment": {
                            "execpolicy_amendment": tokens
                        }
                    })
                } else if key == "applyNetworkPolicyAmendment" {
                    let host = network_host_entry.text().to_string();
                    let action = if network_action.selected() == 1 {
                        "deny"
                    } else {
                        "allow"
                    };
                    json!({
                        "applyNetworkPolicyAmendment": {
                            "network_policy_amendment": {
                                "host": host,
                                "action": action
                            }
                        }
                    })
                } else {
                    decision_payload.clone()
                };
                on_submit(
                    json!({ "decision": final_decision }),
                    label.clone(),
                );
            });
            actions.append(&button);
        }
    }

    root.append(&actions);
    card.append(&root);
    card
}

fn set_request_card_submission_state(
    card: &gtk::Box,
    title_text: &str,
    detail_text: &str,
    is_error: bool,
) {
    while let Some(child) = card.first_child() {
        card.remove(&child);
    }

    card.add_css_class("chat-approval-card-submitted");
    if is_error {
        card.add_css_class("chat-approval-card-error");
    } else {
        card.remove_css_class("chat-approval-card-error");
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
    root.add_css_class("chat-approval-summary");
    root.set_margin_start(10);
    root.set_margin_end(10);
    root.set_margin_top(8);
    root.set_margin_bottom(8);

    let title = gtk::Label::new(Some(title_text));
    title.set_xalign(0.0);
    title.add_css_class("chat-approval-summary-title");
    root.append(&title);

    let detail = gtk::Label::new(Some(detail_text));
    detail.set_xalign(0.0);
    detail.set_wrap(true);
    detail.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    detail.add_css_class("chat-approval-summary-detail");
    root.append(&detail);

    card.append(&root);
}

fn remove_request_card(
    turn_uis: &Rc<RefCell<HashMap<String, super::TurnUi>>>,
    turn_id: &str,
    card: &gtk::Box,
) {
    if let Some(turn_ui) = turn_uis.borrow_mut().get_mut(turn_id) {
        turn_ui.body_box.remove(card);
        super::refresh_turn_status(turn_ui);
    }
}

fn default_tool_decisions() -> Vec<Value> {
    vec![
        Value::String("accept".to_string()),
        Value::String("decline".to_string()),
        Value::String("cancel".to_string()),
    ]
}

fn command_file_decisions_or_defaults(method: &str, params: &Value) -> Vec<Value> {
    let mut decisions = approval_decision_options_for_event(method, params);
    if decisions.is_empty() {
        decisions = vec![
            Value::String("accept".to_string()),
            Value::String("acceptForSession".to_string()),
            Value::String("decline".to_string()),
            Value::String("cancel".to_string()),
        ];
    }
    if method == "item/commandExecution/requestApproval" {
        if params.get("proposedExecpolicyAmendment").is_some()
            && !decisions
                .iter()
                .filter_map(decision_key)
                .any(|key| key == "acceptWithExecpolicyAmendment")
        {
            decisions.push(Value::String("acceptWithExecpolicyAmendment".to_string()));
        }
        if (params.get("proposedNetworkPolicyAmendments").is_some()
            || params.get("networkApprovalContext").is_some())
            && !decisions
                .iter()
                .filter_map(decision_key)
                .any(|key| key == "applyNetworkPolicyAmendment")
        {
            decisions.push(Value::String("applyNetworkPolicyAmendment".to_string()));
        }
    }
    decisions
}

fn request_details_text(method: &str, params: &Value) -> String {
    if method == "item/commandExecution/requestApproval" {
        let command = params
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("<command unavailable>");
        let cwd = params
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or("<cwd unavailable>");
        let reason = params
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("No reason provided.");
        return format!("Command: {command}\nCWD: {cwd}\nReason: {reason}");
    }
    if method == "item/fileChange/requestApproval" {
        let reason = params
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("No reason provided.");
        let grant_root = params
            .get("grantRoot")
            .and_then(Value::as_str)
            .unwrap_or("");
        if grant_root.is_empty() {
            return format!("Reason: {reason}");
        }
        return format!("Reason: {reason}\nGrant root: {grant_root}");
    }

    params
        .get("prompt")
        .and_then(Value::as_str)
        .map(|s| s.to_string())
        .or_else(|| {
            params
                .get("reason")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "The server requested user input for a tool action.".to_string())
}

fn request_title(method: &str) -> &'static str {
    match method {
        "item/commandExecution/requestApproval" => "Approve Command Execution",
        "item/fileChange/requestApproval" => "Approve File Changes",
        "item/tool/requestUserInput" => "Tool Needs Input",
        _ => "Action Requires Input",
    }
}

fn sanitize_cancel_payload(method: &str) -> Value {
    let _ = method;
    json!({"decision": "cancel"})
}

fn create_request_submit_payload(method: &str, params: &Value) -> (String, String, Vec<Value>) {
    let title = request_title(method).to_string();
    let details = request_details_text(method, params);
    let decisions = if method == "item/tool/requestUserInput" {
        let listed = approval_decision_options_for_event(method, params);
        if listed.is_empty() {
            default_tool_decisions()
        } else {
            listed
        }
    } else {
        command_file_decisions_or_defaults(method, params)
    };
    (title, details, decisions)
}

fn is_tool_question_flow(method: &str, params: &Value) -> bool {
    method == "item/tool/requestUserInput"
        && params
            .get("questions")
            .and_then(Value::as_array)
            .map(|items| !items.is_empty())
            .unwrap_or(false)
}

fn tool_no_question_decision_payload(decision_payload: Value) -> Value {
    json!({ "decision": decision_payload })
}

fn is_generic_item_kind(kind: &str) -> bool {
    matches!(
        kind,
        "webSearch"
            | "mcpToolCall"
            | "collabToolCall"
            | "imageView"
            | "enteredReviewMode"
            | "exitedReviewMode"
            | "contextCompaction"
    )
}

fn extract_token_usage_summary(params: &Value) -> Option<String> {
    let usage = params
        .get("tokenUsage")
        .or_else(|| params.get("usage"))
        .or_else(|| params.get("thread").and_then(|v| v.get("tokenUsage")))?;

    let input_opt = usage
        .get("inputTokens")
        .or_else(|| usage.get("input"))
        .or_else(|| usage.get("promptTokens"))
        .and_then(Value::as_i64);
    let output_opt = usage
        .get("outputTokens")
        .or_else(|| usage.get("output"))
        .or_else(|| usage.get("completionTokens"))
        .and_then(Value::as_i64);
    let total_opt = usage
        .get("totalTokens")
        .or_else(|| usage.get("total"))
        .and_then(Value::as_i64);

    let input = input_opt.unwrap_or(0);
    let output = output_opt.unwrap_or(0);
    let total = total_opt.unwrap_or(input + output);
    if input <= 0 && output <= 0 && total <= 0 {
        return None;
    }

    Some(format!(
        "Token usage updated: input {input}, output {output}, total {total}."
    ))
}

fn append_event_note_to_turn(
    turn_ui: &mut super::TurnUi,
    messages_scroll: &gtk::ScrolledWindow,
    key: String,
    text: &str,
) {
    let label = super::message_render::create_text_segment(&turn_ui.body_box);
    super::markdown::set_markdown(&label, text);
    turn_ui.text_widgets.insert(key.clone(), label);
    turn_ui.text_buffers.insert(key, text.to_string());
    super::refresh_turn_status(turn_ui);
    super::message_render::scroll_to_bottom(messages_scroll);
}

fn build_tool_call_success_payload(text: &str) -> Value {
    json!({
        "success": true,
        "contentItems": [
            {
                "type": "output_text",
                "text": text
            }
        ]
    })
}

fn build_tool_call_failure_payload(message: &str) -> Value {
    json!({
        "success": false,
        "error": {
            "message": message
        }
    })
}

fn create_tool_call_request_card(
    tool_name: &str,
    arguments: &str,
    prompt: &str,
    on_submit: Rc<dyn Fn(Value, String)>,
) -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("chat-command-card");
    card.add_css_class("chat-approval-card");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let title = gtk::Label::new(Some("Tool Call Request"));
    title.set_xalign(0.0);
    title.add_css_class("chat-command-section-title");
    root.append(&title);

    let details = gtk::Label::new(Some(prompt));
    details.set_xalign(0.0);
    details.set_wrap(true);
    details.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    root.append(&details);

    let tool_label = gtk::Label::new(Some(&format!("Tool: {tool_name}")));
    tool_label.set_xalign(0.0);
    tool_label.set_wrap(true);
    tool_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    root.append(&tool_label);

    let args_label = gtk::Label::new(Some(&format!("Arguments: {arguments}")));
    args_label.set_xalign(0.0);
    args_label.set_wrap(true);
    args_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    args_label.set_selectable(true);
    root.append(&args_label);

    let response_entry = gtk::Entry::new();
    response_entry.set_placeholder_text(Some("Tool response text"));
    root.append(&response_entry);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);

    let send_button = gtk::Button::with_label("Send result");
    {
        let response_entry = response_entry.clone();
        let on_submit = on_submit.clone();
        send_button.connect_clicked(move |_| {
            let text = response_entry.text().to_string();
            let payload = build_tool_call_success_payload(&text);
            on_submit(payload, "Send result".to_string());
        });
    }
    actions.append(&send_button);

    let fail_button = gtk::Button::with_label("Fail");
    {
        let response_entry = response_entry.clone();
        let on_submit = on_submit.clone();
        fail_button.connect_clicked(move |_| {
            let message = response_entry.text().to_string();
            let message = if message.trim().is_empty() {
                "Tool call failed".to_string()
            } else {
                message
            };
            on_submit(build_tool_call_failure_payload(&message), "Fail".to_string());
        });
    }
    actions.append(&fail_button);

    let cancel_button = gtk::Button::with_label("Cancel");
    {
        let on_submit = on_submit.clone();
        cancel_button.connect_clicked(move |_| {
            on_submit(build_tool_call_failure_payload(
                "Tool call cancelled by user",
            ), "Cancel".to_string());
        });
    }
    actions.append(&cancel_button);

    root.append(&actions);
    card.append(&root);
    card
}

fn persist_pending_request_entry(
    db: &AppDb,
    cached_pending_requests_for_thread: &Rc<RefCell<Vec<Value>>>,
    thread_id: &str,
    request_id: i64,
    turn_id: &str,
    method: &str,
    params: &Value,
) {
    let entry = json!({
        "requestId": request_id,
        "turnId": turn_id,
        "method": method,
        "params": params
    });
    let mut cached = cached_pending_requests_for_thread.borrow_mut();
    super::codex_history::upsert_cached_pending_request(&mut cached, entry);
    super::codex_history::save_cached_pending_requests(db, thread_id, &cached);
}

fn remove_persisted_pending_request(
    db: &AppDb,
    cached_pending_requests_for_thread: &Rc<RefCell<Vec<Value>>>,
    thread_id: &str,
    request_id: i64,
) {
    let mut cached = cached_pending_requests_for_thread.borrow_mut();
    if super::codex_history::remove_cached_pending_request(&mut cached, request_id) {
        super::codex_history::save_cached_pending_requests(db, thread_id, &cached);
    }
}

#[allow(clippy::too_many_arguments)]
fn show_pending_request_card(
    client: &Arc<CodexAppServer>,
    turn_uis: &Rc<RefCell<HashMap<String, super::TurnUi>>>,
    pending_server_requests_by_id: &Rc<RefCell<HashMap<i64, PendingServerRequestUi>>>,
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    conversation_stack: &gtk::Stack,
    thread_id: &str,
    turn_id: &str,
    request_id: i64,
    method: &str,
    params: &Value,
    auto_scroll: bool,
) {
    if let Some(existing) = pending_server_requests_by_id
        .borrow_mut()
        .remove(&request_id)
    {
        remove_request_card(turn_uis, &existing.turn_id, &existing.card);
    }

    let card = if method == "item/tool/call" {
        let (tool_name, arguments, prompt) =
            super::codex_events::extract_tool_request_call_fields(params);
        let client_for_submit = client.clone();
        let turn_uis_for_submit = turn_uis.clone();
        let pending_map_for_submit = pending_server_requests_by_id.clone();
        create_tool_call_request_card(
            &tool_name,
            &arguments,
            &prompt,
            Rc::new(move |payload, action_label| {
                if let Some(pending_ui) = pending_map_for_submit.borrow().get(&request_id).cloned()
                {
                    if let Some(turn_ui) = turn_uis_for_submit
                        .borrow_mut()
                        .get_mut(&pending_ui.turn_id)
                    {
                        turn_ui.status_row.set_visible(true);
                        turn_ui.status_label.set_text("Waiting for agent...");
                    }
                    set_request_card_submission_state(
                        &pending_ui.card,
                        &format!("{action_label} selected"),
                        "Response sent. Waiting for agent...",
                        false,
                    );
                }
                if let Err(err) = client_for_submit.respond_to_server_request(request_id, payload) {
                    if let Some(pending_ui) = pending_map_for_submit.borrow().get(&request_id) {
                        set_request_card_submission_state(
                            &pending_ui.card,
                            "Failed to send response",
                            &err,
                            true,
                        );
                    }
                    if let Some(pending_ui) = pending_map_for_submit.borrow().get(&request_id) {
                        if let Some(turn_ui) = turn_uis_for_submit
                            .borrow_mut()
                            .get_mut(&pending_ui.turn_id)
                        {
                            turn_ui.status_row.set_visible(true);
                            turn_ui.status_label.set_text("Response submit failed");
                        }
                    }
                    eprintln!("failed to send tool/call response request_id={request_id}: {err}");
                }
            }),
        )
    } else {
        let (title, details, mut options) = create_request_submit_payload(method, params);
        if method == "item/tool/requestUserInput"
            && !is_tool_question_flow(method, params)
            && options.is_empty()
        {
            options = default_tool_decisions();
        }
        let client_for_submit = client.clone();
        let turn_uis_for_submit = turn_uis.clone();
        let pending_map_for_submit = pending_server_requests_by_id.clone();
        let method = method.to_string();
        let method_for_submit = method.clone();
        create_inline_approval_card(
            &title,
            &details,
            method.as_str(),
            params,
            &options,
            Rc::new(move |decision_payload, action_label| {
                if let Some(pending_ui) = pending_map_for_submit.borrow().get(&request_id).cloned()
                {
                    if let Some(turn_ui) = turn_uis_for_submit
                        .borrow_mut()
                        .get_mut(&pending_ui.turn_id)
                    {
                        turn_ui.status_row.set_visible(true);
                        turn_ui.status_label.set_text("Waiting for agent...");
                    }
                    set_request_card_submission_state(
                        &pending_ui.card,
                        &format!("{action_label} selected"),
                        "Decision sent. Waiting for agent...",
                        false,
                    );
                }
                let final_payload = if method_for_submit == "item/tool/requestUserInput"
                    && decision_payload.get("decision").is_none()
                {
                    decision_payload
                } else if method_for_submit == "item/tool/requestUserInput" {
                    tool_no_question_decision_payload(
                        decision_payload
                            .get("decision")
                            .cloned()
                            .unwrap_or_else(|| Value::String("cancel".to_string())),
                    )
                } else {
                    decision_payload
                };
                if let Err(err) =
                    client_for_submit.respond_to_server_request(request_id, final_payload)
                {
                    if let Some(pending_ui) = pending_map_for_submit.borrow().get(&request_id) {
                        set_request_card_submission_state(
                            &pending_ui.card,
                            "Failed to send decision",
                            &err,
                            true,
                        );
                    }
                    if let Some(pending_ui) = pending_map_for_submit.borrow().get(&request_id) {
                        if let Some(turn_ui) = turn_uis_for_submit
                            .borrow_mut()
                            .get_mut(&pending_ui.turn_id)
                        {
                            turn_ui.status_row.set_visible(true);
                            turn_ui.status_label.set_text("Decision submit failed");
                        }
                    }
                    eprintln!("failed to send approval response request_id={request_id}: {err}");
                }
            }),
        )
    };

    let mut turns = turn_uis.borrow_mut();
    let turn_ui = turns.entry(turn_id.to_string()).or_insert_with(|| {
        super::create_turn_ui(messages_box, messages_scroll, conversation_stack)
    });
    turn_ui.in_progress = true;
    turn_ui.status_row.set_visible(true);
    if method == "item/tool/call" {
        turn_ui.status_label.set_text("Tool call needs response...");
    } else {
        turn_ui.status_label.set_text("Waiting for approval...");
    }
    turn_ui.body_box.append(&card);
    super::refresh_turn_status(turn_ui);
    if auto_scroll {
        super::message_render::scroll_to_bottom(messages_scroll);
    }
    pending_server_requests_by_id.borrow_mut().insert(
        request_id,
        PendingServerRequestUi {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.to_string(),
            card,
        },
    );
}
