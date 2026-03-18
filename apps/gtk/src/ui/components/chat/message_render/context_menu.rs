fn create_message_context_menu() -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(true);
    popover.set_autohide(true);
    popover.add_css_class("actions-popover");

    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu_box.add_css_class("chat-message-context-menu");
    menu_box.set_margin_start(6);
    menu_box.set_margin_end(6);
    menu_box.set_margin_top(6);
    menu_box.set_margin_bottom(6);

    menu_box.append(&create_context_action_row(
        "edit-copy-symbolic",
        "Copy Selected",
        "copy-selected",
    ));
    menu_box.append(&create_context_action_row(
        "edit-copy-symbolic",
        "Copy",
        "copy",
    ));
    menu_box.append(&create_context_action_row(
        "terminal-symbolic",
        "Copy Command",
        "copy-command",
    ));
    menu_box.append(&create_context_action_row(
        "edit-copy-symbolic",
        "Copy Output",
        "copy-output",
    ));
    menu_box.append(&create_context_action_row(
        "fork-right-symbolic",
        "Fork",
        "fork",
    ));

    popover.set_child(Some(&menu_box));
    popover
}

fn create_context_action_row(icon_name: &str, label: &str, action_id: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.set_has_frame(false);
    button.add_css_class("app-flat-button");
    button.add_css_class("chat-message-context-item");
    button.set_halign(gtk::Align::Fill);
    button.set_widget_name(action_id);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(13);
    icon.add_css_class("chat-message-context-icon");

    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.add_css_class("chat-message-context-label");

    row.append(&icon);
    row.append(&text);
    button.set_child(Some(&row));

    button
}

#[derive(Default)]
struct ContextMenuSelection {
    content_widget: gtk::glib::WeakRef<gtk::Widget>,
    command_context_widget: gtk::glib::WeakRef<gtk::Widget>,
    is_user: bool,
}

thread_local! {
    static SHARED_MESSAGE_CONTEXT_MENU_REGISTRY: RefCell<HashMap<usize, gtk::Popover>> =
        RefCell::new(HashMap::new());
}

pub(super) fn ensure_shared_message_context_menu(messages_box: &gtk::Box) {
    if messages_box.widget_name() != "chat-messages-box" {
        return;
    }
    let key = messages_box_registry_key(messages_box);
    let should_init = SHARED_MESSAGE_CONTEXT_MENU_REGISTRY.with(|registry| {
        !registry.borrow().contains_key(&key)
    });
    if !should_init {
        return;
    }

    let selection = Rc::new(RefCell::new(ContextMenuSelection::default()));
    let popover = create_message_context_menu();
    wire_message_context_actions(&popover, selection.clone());
    popover.set_parent(messages_box);

    {
        let selection = selection.clone();
        popover.connect_hide(move |_| {
            *selection.borrow_mut() = ContextMenuSelection::default();
        });
    }

    {
        let messages_box_weak = messages_box.downgrade();
        let popover_weak = popover.downgrade();
        let selection = selection.clone();
        let right_click = gtk::GestureClick::builder().button(3).build();
        right_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        right_click.connect_pressed(move |gesture, _, x, y| {
            let Some(messages_box) = messages_box_weak.upgrade() else {
                return;
            };
            let Some(popover) = popover_weak.upgrade() else {
                return;
            };
            let Some(picked) = messages_box.pick(x, y, gtk::PickFlags::DEFAULT) else {
                return;
            };
            let Some(row_widget) = find_ancestor_with_css_class(&picked, "chat-message-row") else {
                return;
            };
            let Ok(row) = row_widget.clone().downcast::<gtk::Box>() else {
                return;
            };
            let Some(shell_widget) = row.first_child() else {
                return;
            };
            let Ok(shell) = shell_widget.downcast::<gtk::Box>() else {
                return;
            };
            let Some(content) = shell.first_child() else {
                return;
            };

            let command_context = find_ancestor_with_css_class(&picked, "chat-command-card")
                .unwrap_or_else(|| picked.clone());
            let is_user = row.halign() == gtk::Align::End;
            {
                let mut current = selection.borrow_mut();
                current.content_widget = content.downgrade();
                current.command_context_widget = command_context.downgrade();
                current.is_user = is_user;
            }
            let anchor: gtk::Widget = messages_box.clone().upcast();
            if anchor.root().is_none() || !anchor.is_mapped() {
                return;
            }
            let needs_reparent = popover
                .parent()
                .map(|parent| parent.as_ptr() != anchor.as_ptr())
                .unwrap_or(true);
            if needs_reparent {
                if popover.parent().is_some() {
                    popover.unparent();
                }
                popover.set_parent(&messages_box);
            }
            if popover.parent().and_then(|parent| parent.root()).is_none() {
                return;
            }
            update_context_menu_visibility(&popover, &selection.borrow());
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        messages_box.add_controller(right_click);
    }

    {
        let messages_box = messages_box.clone();
        if let Some(window) = messages_box.root().and_downcast::<gtk::Window>() {
            let key_controller = gtk::EventControllerKey::new();
            key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
            key_controller.connect_key_pressed(move |_, key, _, state| {
                let is_copy = (key == gtk::gdk::Key::c || key == gtk::gdk::Key::C)
                    && state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
                if !is_copy {
                    return gtk::glib::Propagation::Proceed;
                }

                let root: gtk::Widget = messages_box.clone().upcast();
                if let Some(selected) = extract_selected_text_from_widget(&root) {
                    copy_text_to_clipboard(&selected);
                    gtk::glib::Propagation::Stop
                } else {
                    gtk::glib::Propagation::Proceed
                }
            });
            window.add_controller(key_controller);
        }
    }

    messages_box.connect_destroy(move |_| {
        SHARED_MESSAGE_CONTEXT_MENU_REGISTRY.with(|registry| {
            registry.borrow_mut().remove(&key);
        });
    });

    SHARED_MESSAGE_CONTEXT_MENU_REGISTRY.with(|registry| {
        registry.borrow_mut().insert(key, popover);
    });
}

fn wire_message_context_actions(
    popover: &gtk::Popover,
    selection: Rc<RefCell<ContextMenuSelection>>,
) {
    let Some(menu_box) = popover.child().and_downcast::<gtk::Box>() else {
        return;
    };

    let mut child = menu_box.first_child();
    while let Some(widget) = child {
        if let Ok(button) = widget.clone().downcast::<gtk::Button>() {
            let action_id = button.widget_name();
            let popover_weak = popover.downgrade();
            let selection = selection.clone();
            button.connect_clicked(move |_| {
                let (content_ref, command_context, _is_user) = {
                    let current = selection.borrow();
                    (
                        current.content_widget.upgrade(),
                        current
                            .command_context_widget
                            .upgrade()
                            .or_else(|| current.content_widget.upgrade()),
                        current.is_user,
                    )
                };
                match action_id.as_str() {
                    "copy-selected" => {
                        if let Some(content_ref) = content_ref.as_ref() {
                            if let Some(selected) = extract_selected_text_from_widget(content_ref) {
                                copy_text_to_clipboard(&selected);
                            }
                        }
                    }
                    "copy" => {
                        if let Some(content_ref) = content_ref.as_ref() {
                            if let Some(full_text) = extract_full_text_from_widget(content_ref) {
                                copy_text_to_clipboard(&full_text);
                            }
                        }
                    }
                    "copy-command" => {
                        if let Some(command_context) = command_context.as_ref() {
                            if let Some(command_text) =
                                extract_command_text_from_widget(command_context)
                            {
                                copy_text_to_clipboard(&command_text);
                            }
                        }
                    }
                    "copy-output" => {
                        if let Some(command_context) = command_context.as_ref() {
                            if let Some(output_text) =
                                extract_command_output_text_from_widget(command_context)
                            {
                                copy_text_to_clipboard(&output_text);
                            }
                        }
                    }
                    "fork" => {
                        if let Some(content_ref) = content_ref.as_ref() {
                            start_fork_from_context(content_ref);
                        }
                    }
                    _ => {}
                }
                if let Some(popover_ref) = popover_weak.upgrade() {
                    popover_ref.popdown();
                }
            });
        }
        child = widget.next_sibling();
    }
}

fn update_context_menu_visibility(
    popover: &gtk::Popover,
    selection: &ContextMenuSelection,
) {
    let content = selection.content_widget.upgrade();
    let command_context = selection
        .command_context_widget
        .upgrade()
        .or_else(|| content.clone());
    let has_selected_text = content
        .as_ref()
        .and_then(extract_selected_text_from_widget)
        .is_some();
    let has_command_text = command_context
        .as_ref()
        .and_then(extract_command_text_from_widget)
        .is_some();
    let has_command_output = command_context
        .as_ref()
        .and_then(extract_command_output_text_from_widget)
        .is_some();
    let Some(menu_box) = popover.child().and_downcast::<gtk::Box>() else {
        return;
    };

    let mut child = menu_box.first_child();
    while let Some(widget) = child {
        if let Ok(button) = widget.clone().downcast::<gtk::Button>() {
            match button.widget_name().as_str() {
                "copy-selected" => button.set_visible(has_selected_text),
                "copy" => button.set_visible(content.is_some()),
                "copy-command" => button.set_visible(has_command_text),
                "copy-output" => button.set_visible(has_command_output),
                "fork" => button.set_visible(!selection.is_user && content.is_some()),
                _ => {}
            }
        }
        child = widget.next_sibling();
    }
}

fn resolve_messages_box_for_widget(root: &gtk::Widget) -> Option<gtk::Box> {
    let mut node = Some(root.clone());
    while let Some(current) = node {
        if current.widget_name() == "chat-messages-box" {
            return current.downcast::<gtk::Box>().ok();
        }
        node = current.parent();
    }
    None
}

fn resolve_chat_handles(root: &gtk::Widget) -> Option<(Rc<AppDb>, Rc<CodexProfileManager>)> {
    let messages_box = resolve_messages_box_for_widget(root)?;
    chat_handles_for_messages_box(&messages_box)
}

fn current_messages_box_thread_id(root: &gtk::Widget) -> Option<String> {
    let messages_box = resolve_messages_box_for_widget(root)?;
    chat_thread_id_for_messages_box(&messages_box)
}

fn fallback_active_thread_id(db: &AppDb) -> Option<String> {
    let local_thread_id = db
        .get_setting("last_active_thread_id")
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())?;
    db.get_thread_record(local_thread_id)
        .ok()
        .flatten()
        .and_then(|thread| thread.remote_thread_id_owned())
}

fn find_message_row(root: &gtk::Widget) -> Option<gtk::Widget> {
    let mut node = Some(root.clone());
    while let Some(current) = node {
        if current.has_css_class("chat-message-row") {
            return Some(current);
        }
        node = current.parent();
    }
    None
}

fn extract_turn_id_from_assistant_context(root: &gtk::Widget) -> Option<String> {
    let row = find_message_row(root)?;
    let mut sibling = row.prev_sibling();
    while let Some(node) = sibling {
        let marker = node.widget_name();
        if let Some(turn_id) = marker.strip_prefix("turn-user-row:") {
            let trimmed = turn_id.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        sibling = node.prev_sibling();
    }
    None
}

fn parse_turn_timestamp_opt(turn: &Value) -> Option<i64> {
    let raw = turn
        .get("completedAt")
        .or_else(|| turn.get("createdAt"))
        .or_else(|| turn.get("timestamp"))?;
    if let Some(value) = raw.as_i64() {
        return Some(value);
    }
    let iso = raw.as_str()?;
    gtk::glib::DateTime::from_iso8601(iso, None)
        .ok()
        .map(|dt| dt.to_unix())
}

fn rollback_count_after_target_turn(thread: &Value, target_turn_id: &str) -> Option<usize> {
    let turns = thread.get("turns").and_then(Value::as_array)?;
    let target_index = turns
        .iter()
        .position(|turn| turn.get("id").and_then(Value::as_str) == Some(target_turn_id))?;
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
    Some(turns.len().saturating_sub(chrono_target_pos + 1))
}

fn start_fork_from_context(content: &gtk::Widget) {
    let Some((db, manager)) = resolve_chat_handles(content) else {
        eprintln!("fork action ignored: chat context unavailable");
        return;
    };
    let Some(source_thread_id) =
        current_messages_box_thread_id(content).or_else(|| fallback_active_thread_id(db.as_ref()))
    else {
        eprintln!("fork action ignored: source thread id unavailable");
        return;
    };
    let Some(source_thread) = db
        .get_thread_record_by_remote_thread_id(&source_thread_id)
        .ok()
        .flatten()
    else {
        eprintln!("fork action ignored: source local thread record not found");
        return;
    };
    let Some(client) = manager.resolve_client_for_thread_id(&source_thread_id) else {
        eprintln!("fork action ignored: runtime client unavailable for source thread");
        return;
    };
    if !client.capabilities().supports_fork {
        eprintln!("fork action ignored: runtime does not support thread fork");
        return;
    }
    let target_turn_id = extract_turn_id_from_assistant_context(content);
    let source_thread_id_for_worker = source_thread_id.clone();
    let target_turn_id_for_worker = target_turn_id.clone();
    let (tx, rx) = mpsc::channel::<Result<(String, Value), String>>();
    thread::spawn(move || {
        let result = (|| -> Result<(String, Value), String> {
            let forked_thread_id = client.thread_fork(&source_thread_id_for_worker)?;
            if let Some(target_turn_id) = target_turn_id_for_worker.as_deref() {
                if !client.capabilities().supports_rollback {
                    return Err(
                        "runtime does not support rollback for fork-at-turn".to_string(),
                    );
                }
                let fork_thread = client.thread_read(&forked_thread_id, true)?;
                if let Some(rollback_count) =
                    rollback_count_after_target_turn(&fork_thread, target_turn_id)
                {
                    if rollback_count > 0 {
                        let _ = client.thread_rollback(&forked_thread_id, rollback_count)?;
                    }
                }
            }
            let final_thread = client.thread_read(&forked_thread_id, true)?;
            Ok((forked_thread_id, final_thread))
        })();
        let _ = tx.send(result);
    });

    let ui_root = content.root().map(|root| root.upcast::<gtk::Widget>());
    gtk::glib::timeout_add_local(Duration::from_millis(30), move || match rx.try_recv() {
        Ok(Ok((forked_thread_id, forked_thread))) => {
            let fork_title = source_thread.title.clone();
            let new_thread = match db.create_thread_with_remote_identity(
                source_thread.workspace_id,
                source_thread.profile_id,
                Some(source_thread.id),
                &fork_title,
                Some(&forked_thread_id),
                source_thread.remote_account_type(),
                source_thread.remote_account_email(),
            ) {
                Ok(record) => record,
                Err(err) => {
                    eprintln!("failed to create local forked thread: {err}");
                    return gtk::glib::ControlFlow::Break;
                }
            };

            if let Err(err) = super::history::sync_completed_turns_from_thread(
                db.as_ref(),
                &forked_thread_id,
                &forked_thread,
            ) {
                eprintln!("failed to sync forked thread local history: {err}");
            } else {
                super::history::prune_cached_state_for_thread(
                    db.as_ref(),
                    &forked_thread_id,
                    &forked_thread,
                );
            }

            let mut inserted = false;
            if let Some(root) = ui_root.as_ref() {
                inserted = thread_list::append_thread_under_parent_from_root(
                    root,
                    source_thread.id,
                    new_thread.clone(),
                );
            }
            if !inserted {
                eprintln!("fork created but sidebar insertion under parent failed");
            }
            let _ = db.set_setting("last_active_thread_id", &new_thread.id.to_string());
            let _ = db.set_setting("pending_profile_thread_id", "");
            gtk::glib::ControlFlow::Break
        }
        Ok(Err(err)) => {
            eprintln!("failed to fork thread: {err}");
            gtk::glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => gtk::glib::ControlFlow::Break,
    });
}

fn find_ancestor_with_css_class(root: &gtk::Widget, css_class: &str) -> Option<gtk::Widget> {
    let mut node = Some(root.clone());
    while let Some(current) = node {
        if current.has_css_class(css_class) {
            return Some(current);
        }
        node = current.parent();
    }
    None
}

fn copy_text_to_clipboard(text: &str) {
    if let Some(display) = gtk::gdk::Display::default() {
        let clipboard = display.clipboard();
        clipboard.set_text(text);
    }
}

fn extract_selected_text_from_widget(root: &gtk::Widget) -> Option<String> {
    collect_labels(root).into_iter().find_map(|label| {
        let (start, end) = label.selection_bounds()?;
        if end <= start {
            return None;
        }
        let text = label.text().to_string();
        slice_by_char_bounds(&text, start as usize, end as usize)
    })
}

fn extract_full_text_from_widget(root: &gtk::Widget) -> Option<String> {
    let lines: Vec<String> = collect_labels(root)
        .into_iter()
        .filter_map(|label| {
            let text = label.text().to_string();
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect();

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n\n"))
    }
}

fn extract_command_text_from_widget(root: &gtk::Widget) -> Option<String> {
    let command_card = find_ancestor_with_css_class(root, "chat-command-card").or_else(|| {
        root.has_css_class("chat-command-card")
            .then_some(root.clone())
    })?;
    command_card
        .tooltip_text()
        .map(|text| text.to_string())
        .filter(|text| !text.trim().is_empty())
}

fn extract_command_output_text_from_widget(root: &gtk::Widget) -> Option<String> {
    let command_card = find_ancestor_with_css_class(root, "chat-command-card").or_else(|| {
        root.has_css_class("chat-command-card")
            .then_some(root.clone())
    })?;
    collect_labels(&command_card).into_iter().find_map(|label| {
        if !label.has_css_class("chat-command-output") {
            return None;
        }
        let text = label.text().to_string();
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn collect_labels(root: &gtk::Widget) -> Vec<gtk::Label> {
    let mut labels = Vec::new();
    collect_labels_recursive(root, &mut labels);
    labels
}

fn collect_labels_recursive(widget: &gtk::Widget, out: &mut Vec<gtk::Label>) {
    if let Ok(label) = widget.clone().downcast::<gtk::Label>() {
        out.push(label);
    }

    let mut child = widget.first_child();
    while let Some(node) = child {
        collect_labels_recursive(&node, out);
        child = node.next_sibling();
    }
}

fn slice_by_char_bounds(text: &str, start: usize, end: usize) -> Option<String> {
    if start >= end {
        return None;
    }

    let chars: Vec<char> = text.chars().collect();
    if start >= chars.len() {
        return None;
    }

    let end = end.min(chars.len());
    let selected: String = chars[start..end].iter().collect();
    if selected.trim().is_empty() {
        None
    } else {
        Some(selected)
    }
}
