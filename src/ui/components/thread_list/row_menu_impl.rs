fn thread_row(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
    workspace_path: String,
    show_profile_icons: Rc<Cell<bool>>,
    show_backend_icons: Rc<Cell<bool>>,
    thread: ThreadRecord,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.add_css_class("thread-row");
    row.set_selectable(false);
    row.set_activatable(false);
    let local_thread_id = thread.id;
    let runtime_workspace_path = thread_runtime_workspace_path(&thread, &workspace_path);
    let thread_has_account_identity =
        thread.remote_account_type().is_some() || thread.remote_account_email().is_some();

    row.set_widget_name(&format!("thread-{}", local_thread_id));

    let current_thread_id: Rc<RefCell<Option<String>>> =
        Rc::new(RefCell::new(thread.remote_thread_id_owned()));

    let inner = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    inner.add_css_class("thread-row-content");
    inner.set_hexpand(true);
    inner.set_halign(gtk::Align::Fill);
    let is_fork = thread.parent_thread_id.is_some();
    inner.set_margin_start(if is_fork { 12 } else { 8 });
    inner.set_margin_end(0);
    inner.set_margin_top(0);
    inner.set_margin_bottom(0);

    if is_fork {
        let fork_icon = gtk::Image::from_icon_name("fork-right-symbolic");
        fork_icon.set_pixel_size(11);
        fork_icon.add_css_class("thread-fork-icon");
        fork_icon.add_css_class("thread-leading-icon");
        inner.append(&fork_icon);
    }

    if thread.worktree_active {
        let worktree_icon = gtk::Image::from_icon_name("git-symbolic");
        worktree_icon.set_pixel_size(11);
        worktree_icon.add_css_class("thread-worktree-icon");
        worktree_icon.add_css_class("thread-leading-icon");
        inner.append(&worktree_icon);
    }

    let lock_icon = gtk::Image::from_icon_name("padlock-closed-symbolic");
    lock_icon.set_pixel_size(11);
    lock_icon.add_css_class("thread-lock-icon");
    lock_icon.add_css_class("thread-leading-icon");
    lock_icon.set_visible(db.is_local_thread_locked(local_thread_id).unwrap_or(false));
    inner.append(&lock_icon);

    let completion_icon = gtk::Image::from_icon_name("bell-symbolic");
    completion_icon.set_pixel_size(11);
    completion_icon.add_css_class("thread-complete-icon");
    completion_icon.set_visible(false);
    inner.append(&completion_icon);

    let backend_icon_name = backend_icon_name_for_thread(db.as_ref(), &thread);
    let backend_revealer = gtk::Revealer::new();
    backend_revealer.add_css_class("thread-backend-revealer");
    backend_revealer.add_css_class("thread-leading-icon");
    backend_revealer.set_transition_type(gtk::RevealerTransitionType::SlideRight);
    backend_revealer.set_transition_duration(150);
    backend_revealer.set_reveal_child(false);
    backend_revealer.set_visible(show_backend_icons.get() && backend_icon_name.is_some());
    let backend_icon = gtk::Image::from_icon_name(backend_icon_name.unwrap_or("provider-codex"));
    backend_icon.set_pixel_size(11);
    backend_icon.add_css_class("thread-backend-icon");
    backend_revealer.set_child(Some(&backend_icon));
    inner.append(&backend_revealer);

    let profile_icon = gtk::Image::from_icon_name(
        profile_icon_name_for_thread(db.as_ref(), &thread)
            .as_deref()
            .unwrap_or("person-symbolic"),
    );
    profile_icon.set_pixel_size(11);
    profile_icon.add_css_class("thread-profile-icon");
    profile_icon.add_css_class("thread-leading-icon");
    profile_icon.set_visible(
        show_profile_icons.get() && profile_icon_name_for_thread(db.as_ref(), &thread).is_some(),
    );
    inner.append(&profile_icon);

    let label = gtk::Label::new(Some(&thread.title));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_halign(gtk::Align::Fill);
    label.set_width_chars(1);
    label.set_max_width_chars(30);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.add_css_class("thread-title");
    inner.append(&label);
    let thread_title_text: Rc<RefCell<String>> = Rc::new(RefCell::new(thread.title.clone()));

    let time_label = gtk::Label::new(Some(
        &db.thread_relative_time_by_id(thread.id, thread.created_at),
    ));
    time_label.set_halign(gtk::Align::End);
    time_label.add_css_class("thread-time");
    inner.append(&time_label);

    {
        let db = db.clone();
        let time_label = time_label.clone();
        let thread_id = thread.id;
        let fallback_created_at = thread.created_at;
        gtk::glib::timeout_add_local(Duration::from_secs(30), move || {
            if time_label.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            time_label.set_text(&db.thread_relative_time_by_id(thread_id, fallback_created_at));
            gtk::glib::ControlFlow::Continue
        });
    }

    row.set_child(Some(&inner));

    {
        let thread_title = thread.title.clone();
        let remote_thread_id = thread.remote_thread_id_owned();
        let workspace_path = runtime_workspace_path.clone();
        let drag_source = gtk::DragSource::builder()
            .actions(gtk::gdk::DragAction::COPY)
            .build();
        drag_source.connect_drag_begin(move |source, _| {
            let icon = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            icon.add_css_class("thread-drag-chip");
            let title = gtk::Label::new(Some(&thread_title));
            title.add_css_class("thread-drag-chip-label");
            title.set_xalign(0.0);
            icon.append(&title);
            let paintable = gtk::WidgetPaintable::new(Some(&icon));
            source.set_icon(Some(&paintable), 12, 10);
        });
        drag_source.connect_prepare(move |_, _, _| {
            let payload = json!({
                "localThreadId": local_thread_id,
                "threadId": remote_thread_id,
                "codexThreadId": remote_thread_id,
                "workspacePath": workspace_path,
            })
            .to_string();
            Some(gtk::gdk::ContentProvider::for_value(&payload.to_value()))
        });
        row.add_controller(drag_source);
    }

    {
        let db = db.clone();
        let lock_icon = lock_icon.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(500), move || {
            lock_icon.set_visible(db.is_local_thread_locked(local_thread_id).unwrap_or(false));
            gtk::glib::ControlFlow::Continue
        });
    }
    {
        let db = db.clone();
        let profile_icon = profile_icon.clone();
        let show_profile_icons = show_profile_icons.clone();
        let local_thread_id = local_thread_id;
        gtk::glib::timeout_add_local(Duration::from_millis(900), move || {
            if profile_icon.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let icon_name = db
                .get_thread_record(local_thread_id)
                .ok()
                .flatten()
                .and_then(|thread| profile_icon_name_for_thread(db.as_ref(), &thread));
            profile_icon.set_icon_name(icon_name.as_deref());
            profile_icon.set_visible(show_profile_icons.get() && icon_name.is_some());
            gtk::glib::ControlFlow::Continue
        });
    }
    {
        let db = db.clone();
        let completion_icon = completion_icon.clone();
        let label = label.clone();
        let time_label = time_label.clone();
        let inner = inner.clone();
        let current_thread_id = current_thread_id.clone();
        let thread_title_text = thread_title_text.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(90), move || {
            if label.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }

            // Keep cached title in sync with direct UI title updates (e.g. first-message rename).
            let observed_title = label.text().to_string();
            if !observed_title.trim().is_empty() && observed_title != *thread_title_text.borrow() {
                thread_title_text.replace(observed_title);
            }
            let text = thread_title_text.borrow().clone();
            let mut known_thread_id = current_thread_id.borrow().clone();
            if known_thread_id.is_none() {
                known_thread_id = db
                    .get_thread_record(local_thread_id)
                    .ok()
                    .flatten()
                    .and_then(|record| record.remote_thread_id_owned());
                if known_thread_id.is_some() {
                    current_thread_id.replace(known_thread_id.clone());
                }
            }

            let has_active_turn = known_thread_id
                .as_deref()
                .map(crate::ui::components::chat::thread_has_active_turn)
                .unwrap_or(false);
            let has_completed_unseen = known_thread_id
                .as_deref()
                .map(crate::ui::components::chat::thread_has_completed_unseen)
                .unwrap_or(false);
            completion_icon.set_visible(has_completed_unseen && !has_active_turn);
            if has_active_turn {
                let wave_phase = gtk::glib::monotonic_time() as f64 / 90_000.0;
                label.set_use_markup(true);
                label.set_markup(&crate::ui::components::chat::sidebar_wave_status_markup(
                    &text, wave_phase,
                ));
                time_label.add_css_class("thread-wave-active");
                let mut child = inner.first_child();
                while let Some(node) = child {
                    if node.has_css_class("thread-leading-icon") {
                        node.add_css_class("thread-wave-active");
                    }
                    child = node.next_sibling();
                }
            } else {
                label.set_use_markup(false);
                label.set_text(&text);
                time_label.remove_css_class("thread-wave-active");
                let mut child = inner.first_child();
                while let Some(node) = child {
                    if node.has_css_class("thread-leading-icon") {
                        node.remove_css_class("thread-wave-active");
                    }
                    child = node.next_sibling();
                }
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    {
        let row_for_select = row.clone();
        let db = db.clone();
        let manager = manager.clone();
        let active_thread_id = active_thread_id.clone();
        let current_thread_id = current_thread_id.clone();
        let active_workspace_path_for_select = active_workspace_path.clone();
        let workspace_path = runtime_workspace_path.clone();
        let thread_profile_id = thread.profile_id;
        let completion_icon = completion_icon.clone();
        let select_click = gtk::GestureClick::builder().button(1).build();
        select_click.connect_released(move |_, _, _, _| {
            active_workspace_path_for_select.replace(Some(workspace_path.clone()));

            if let Some(root) = row_for_select.root() {
                let root_widget: gtk::Widget = root.upcast();
                if let Some(stack_widget) =
                    widget_tree::find_widget_by_name(&root_widget, "main-content-view-stack")
                {
                    if let Ok(stack) = stack_widget.downcast::<adw::ViewStack>() {
                        stack.set_visible_child_name("chat");
                    }
                }
                clear_thread_list_selections(&root_widget);
            }

            row_for_select.add_css_class("thread-row-selected");
            if let Some(known_thread_id) = current_thread_id.borrow().clone() {
                active_thread_id.replace(Some(known_thread_id));
            }
            if let Some(thread_id) = current_thread_id.borrow().clone() {
                crate::ui::components::chat::clear_thread_completed_unseen(&thread_id);
                completion_icon.set_visible(false);
            } else if let Ok(Some(record)) = db.get_thread_record(local_thread_id) {
                if let Some(thread_id) = record.remote_thread_id() {
                    crate::ui::components::chat::clear_thread_completed_unseen(thread_id);
                }
                completion_icon.set_visible(false);
            }

            let db = db.clone();
            let manager = manager.clone();
            let active_thread_id = active_thread_id.clone();
            let current_thread_id = current_thread_id.clone();
            let workspace_path = workspace_path.clone();
            gtk::glib::timeout_add_local_once(Duration::from_millis(8), move || {
                let _ = db.set_runtime_profile_id(thread_profile_id);
                let _ = db.set_active_profile_id(thread_profile_id);
                if let Some(profile) = db.get_codex_profile(thread_profile_id).ok().flatten() {
                    let _ = db.set_current_thread_account(
                        profile.last_account_type.as_deref(),
                        profile.last_email.as_deref(),
                    );
                }
                let _ = db.set_setting("last_active_thread_id", &local_thread_id.to_string());
                let _ = db.set_setting("last_active_workspace_path", &workspace_path);

                let is_locked = db.is_local_thread_locked(local_thread_id).unwrap_or(false);
                let latest_thread = db.get_thread_record(local_thread_id).ok().flatten();
                let existing_thread_id = latest_thread
                    .as_ref()
                    .and_then(|record| record.remote_thread_id_owned())
                    .or_else(|| current_thread_id.borrow().clone());
                current_thread_id.replace(existing_thread_id.clone());
                let needs_account_backfill = latest_thread
                    .as_ref()
                    .map(|record| {
                        record.remote_account_type().is_none()
                            && record.remote_account_email().is_none()
                    })
                    .unwrap_or(!thread_has_account_identity);
                if let Some(thread_id) = existing_thread_id {
                    active_thread_id.replace(Some(thread_id.clone()));
                    if db
                        .get_setting("pending_profile_thread_id")
                        .ok()
                        .flatten()
                        .and_then(|value| value.parse::<i64>().ok())
                        == Some(local_thread_id)
                    {
                        let _ = db.set_setting("pending_profile_thread_id", "");
                    }
                    if is_locked {
                        return;
                    }
                    let manager_for_resume = manager.clone();
                    let db_for_resume = db.clone();
                    let current_for_resume = current_thread_id.clone();
                    let active_for_resume = active_thread_id.clone();
                    let workspace_path_for_resume = workspace_path.clone();
                    gtk::glib::timeout_add_local_once(Duration::from_millis(24), move || {
                        manager_for_resume.switch_runtime_to_thread(&thread_id);
                        if let Some(client) =
                            manager_for_resume.resolve_running_client_for_thread_id(&thread_id)
                        {
                            let client = client.clone();
                            let workspace_path_bg = workspace_path_for_resume.clone();
                            let remote_thread_id_bg = thread_id.clone();
                            let (tx, rx) = mpsc::channel::<Result<String, String>>();
                            thread::spawn(move || {
                                let result = client.thread_resume(
                                    &remote_thread_id_bg,
                                    Some(&workspace_path_bg),
                                    Some("gpt-5.3-codex"),
                                );
                                let _ = tx.send(result);
                            });

                            let db_for_result = db_for_resume.clone();
                            let current_for_result = current_for_resume.clone();
                            let active_for_result = active_for_resume.clone();
                            gtk::glib::timeout_add_local(Duration::from_millis(30), move || {
                                match rx.try_recv() {
                                    Ok(Ok(resolved_id)) => {
                                        let account =
                                            db_for_result.current_thread_account().ok().flatten();
                                        let account_type =
                                            account.as_ref().map(|(kind, _)| kind.as_str());
                                        let account_email =
                                            account.as_ref().and_then(|(_, email)| email.as_deref());
                                        if resolved_id != thread_id {
                                            eprintln!(
                                                "thread_resume returned mismatched thread id for local_thread_id={} expected={} got={}, keeping original mapping",
                                                local_thread_id, thread_id, resolved_id
                                            );
                                            if active_for_result.borrow().as_deref()
                                                == Some(thread_id.as_str())
                                            {
                                                active_for_result
                                                    .replace(Some(thread_id.clone()));
                                            }
                                            current_for_result
                                                .replace(Some(thread_id.clone()));
                                        } else if needs_account_backfill {
                                            let _ = db_for_result.set_thread_account_identity(
                                                local_thread_id,
                                                account_type,
                                                account_email,
                                            );
                                        }
                                        gtk::glib::ControlFlow::Break
                                    }
                                    Ok(Err(err)) => {
                                        if !is_expected_pre_materialization_error(&err) {
                                            eprintln!(
                                                "failed to resume Codex thread on select (using local history fallback): {err}"
                                            );
                                        }
                                        gtk::glib::ControlFlow::Break
                                    }
                                    Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                                    Err(mpsc::TryRecvError::Disconnected) => gtk::glib::ControlFlow::Break,
                                }
                            });
                        }
                    });
                    return;
                }
                if db
                    .get_setting("pending_profile_thread_id")
                    .ok()
                    .flatten()
                    .and_then(|value| value.parse::<i64>().ok())
                    == Some(local_thread_id)
                {
                    active_thread_id.replace(None);
                    return;
                }
                if is_locked {
                    active_thread_id.replace(None);
                    return;
                }

                if db
                    .local_thread_has_remote_chat_turns(local_thread_id)
                    .unwrap_or(false)
                {
                    eprintln!(
                        "refusing to create new codex thread for local_thread_id={} because local chat history already exists",
                        local_thread_id
                    );
                    return;
                }

                if let Some(refreshed_thread_id) = db
                    .get_thread_record(local_thread_id)
                    .ok()
                    .flatten()
                    .and_then(|record| record.remote_thread_id_owned())
                {
                    current_thread_id.replace(Some(refreshed_thread_id.clone()));
                    active_thread_id.replace(Some(refreshed_thread_id));
                    return;
                }

                let _ = db.set_runtime_profile_id(thread_profile_id);
                if let Some(client) = manager.client_for_profile(thread_profile_id) {
                    let client = client.clone();
                    let sandbox_policy = if client.backend_kind().eq_ignore_ascii_case("opencode")
                    {
                        let default_access_mode =
                            crate::ui::components::chat::composer::default_composer_setting_value(
                                db.as_ref(),
                                "opencode_access_mode",
                            )
                            .unwrap_or_else(|| "workspaceWrite".to_string());
                        let default_command_mode =
                            crate::ui::components::chat::composer::default_composer_setting_value(
                                db.as_ref(),
                                "opencode_command_mode",
                            )
                            .unwrap_or_else(|| "allowAll".to_string());
                        Some(
                            crate::ui::components::chat::runtime_controls::opencode_session_policy_for(
                                &default_access_mode,
                                &default_command_mode,
                            ),
                        )
                    } else {
                        None
                    };
                    let workspace_path_bg = workspace_path.clone();
                    let (tx, rx) = mpsc::channel::<Result<String, String>>();
                    thread::spawn(move || {
                        let _ = tx.send(
                            client.thread_start(
                                Some(&workspace_path_bg),
                                None,
                                sandbox_policy,
                            )
                        );
                    });

                    let db_for_result = db.clone();
                    let current_for_result = current_thread_id.clone();
                    let active_for_result = active_thread_id.clone();
                    gtk::glib::timeout_add_local(Duration::from_millis(30), move || {
                        match rx.try_recv() {
                            Ok(Ok(new_thread_id)) => {
                                let account = db_for_result.current_thread_account().ok().flatten();
                                let account_type = account.as_ref().map(|(kind, _)| kind.as_str());
                                let account_email =
                                    account.as_ref().and_then(|(_, email)| email.as_deref());
                                let _ = db_for_result.set_thread_codex_id_with_account(
                                    local_thread_id,
                                    &new_thread_id,
                                    account_type,
                                    account_email,
                                );
                                current_for_result.replace(Some(new_thread_id.clone()));
                                active_for_result.replace(Some(new_thread_id));
                                if db_for_result
                                    .get_setting("pending_profile_thread_id")
                                    .ok()
                                    .flatten()
                                    .and_then(|value| value.parse::<i64>().ok())
                                    == Some(local_thread_id)
                                {
                                    let _ = db_for_result.set_setting("pending_profile_thread_id", "");
                                }
                                gtk::glib::ControlFlow::Break
                            }
                            Ok(Err(err)) => {
                                eprintln!("failed to start Codex thread on select: {err}");
                                gtk::glib::ControlFlow::Break
                            }
                            Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                            Err(mpsc::TryRecvError::Disconnected) => gtk::glib::ControlFlow::Break,
                        }
                    });
                }
            });
        });
        row.add_controller(select_click);
    }

    let menu = build_thread_context_menu(
        &row,
        &label,
        thread_title_text.clone(),
        db.clone(),
        manager.clone(),
        active_thread_id.clone(),
        current_thread_id.clone(),
        active_workspace_path.clone(),
        runtime_workspace_path.clone(),
        &thread,
    );
    let right_click = gtk::GestureClick::builder().button(3).build();
    {
        let row = row.clone();
        let menu = menu.clone();
        right_click.connect_pressed(move |gesture, _, x, y| {
            if let Some(listbox) = row.parent().and_downcast::<gtk::ListBox>() {
                if menu.parent().is_none() {
                    menu.set_parent(&listbox);
                }
                let point = gtk::graphene::Point::new(x as f32, y as f32);
                let rect = row
                    .compute_point(&listbox, &point)
                    .map(|p| {
                        gtk::gdk::Rectangle::new(p.x().round() as i32, p.y().round() as i32, 1, 1)
                    })
                    .unwrap_or_else(|| gtk::gdk::Rectangle::new(0, 0, 1, 1));
                menu.set_pointing_to(Some(&rect));
            } else {
                if menu.parent().is_none() {
                    menu.set_parent(&row);
                }
                menu.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            }
            menu.popup();
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
    }
    row.add_controller(right_click);

    row
}

fn build_thread_context_menu(
    row: &gtk::ListBoxRow,
    label: &gtk::Label,
    thread_title_text: Rc<RefCell<String>>,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    current_thread_id: Rc<RefCell<Option<String>>>,
    _active_workspace_path: Rc<RefCell<Option<String>>>,
    workspace_path: String,
    thread: &ThreadRecord,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(true);
    popover.set_autohide(true);
    popover.set_offset(0, 0);
    popover.add_css_class("actions-popover");

    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu_box.add_css_class("chat-message-context-menu");
    menu_box.set_margin_start(6);
    menu_box.set_margin_end(6);
    menu_box.set_margin_top(6);
    menu_box.set_margin_bottom(6);

    let rename_button = build_context_item("document-edit-symbolic", "Rename");
    {
        let popover = popover.clone();
        let row = row.clone();
        let label = label.clone();
        let thread_title_text = thread_title_text.clone();
        let db = db.clone();
        let thread_id = thread.id;
        rename_button.connect_clicked(move |_| {
            popover.popdown();

            let parent_window = row
                .root()
                .and_then(|root| root.downcast::<gtk::Window>().ok());
            let dialog = gtk::Window::builder()
                .title("Rename Thread")
                .modal(true)
                .resizable(false)
                .default_width(380)
                .default_height(120)
                .build();
            if let Some(parent) = parent_window.as_ref() {
                dialog.set_transient_for(Some(parent));
            }

            let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
            content.set_margin_start(14);
            content.set_margin_end(14);
            content.set_margin_top(14);
            content.set_margin_bottom(14);

            let entry = gtk::Entry::new();
            entry.set_placeholder_text(Some("Thread title"));
            entry.set_text(&label.text());
            content.append(&entry);

            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            actions.set_halign(gtk::Align::End);
            let cancel_button = gtk::Button::with_label("Cancel");
            let rename_button = gtk::Button::with_label("Rename");
            actions.append(&cancel_button);
            actions.append(&rename_button);
            content.append(&actions);
            dialog.set_child(Some(&content));

            {
                let dialog = dialog.clone();
                cancel_button.connect_clicked(move |_| {
                    dialog.close();
                });
            }

            {
                let dialog = dialog.clone();
                let db = db.clone();
                let label = label.clone();
                let thread_title_text = thread_title_text.clone();
                let entry = entry.clone();
                rename_button.connect_clicked(move |_| {
                    let next_title = entry.text().trim().to_string();
                    if !next_title.is_empty() {
                        if let Err(err) = db.rename_thread(thread_id, &next_title) {
                            eprintln!("failed to rename thread: {err}");
                        } else {
                            thread_title_text.replace(next_title.clone());
                            label.set_text(&next_title);
                        }
                    }
                    dialog.close();
                });
            }

            {
                let rename_button = rename_button.clone();
                entry.connect_activate(move |_| {
                    rename_button.emit_clicked();
                });
            }

            dialog.present();
            entry.grab_focus();
            entry.set_position(-1);
        });
    }

    let close_button = build_context_item("window-close-symbolic", "Close");
    {
        let popover = popover.clone();
        let row = row.clone();
        let db = db.clone();
        let manager = manager.clone();
        let active_thread_id = active_thread_id.clone();
        let thread_id = thread.id;
        let thread_worktree_path = thread.worktree_path.clone();
        let thread_has_worktree = thread.worktree_active;
        let current_thread_id = current_thread_id.clone();
        close_button.connect_clicked(move |_| {
            let scroll_state = row.parent().and_then(|parent| {
                let widget: gtk::Widget = parent.upcast();
                crate::ui::widget_tree::capture_ancestor_vscroll(&widget)
            });
            let remote_thread_id = current_thread_id.borrow().clone();
            if let Some(thread_id) = remote_thread_id.as_deref() {
                if let Some(client) = manager.resolve_client_for_thread_id(thread_id) {
                    let _ = client.thread_archive(thread_id);
                }
            }
            if let Some(path) = thread_worktree_path
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                if let Err(err) = crate::worktree::stop_worktree_checkout(path) {
                    eprintln!("failed to clean up worktree for closed thread: {err}");
                }
                let _ = db.set_thread_worktree_info(thread_id, None, None, false);
            } else if thread_has_worktree {
                let _ = db.set_thread_worktree_info(thread_id, None, None, false);
            }
            if let Err(err) = crate::restore::clear_thread_restore_data(db.as_ref(), thread_id) {
                eprintln!("failed to clear restore history for closed thread: {err}");
            }
            if let Err(err) = db.close_thread(thread_id) {
                eprintln!("failed to close thread: {err}");
            } else if let Some(listbox) = row.parent().and_downcast::<gtk::ListBox>() {
                if let Some(thread_id) = remote_thread_id.as_deref() {
                    layout::remove_thread_from_multiview_layout(db.as_ref(), thread_id);
                }
                listbox.remove(&row);
                let _ = with_thread_list_for_listbox(&listbox, |thread_list| {
                    thread_list.refresh_profile_icon_visibility();
                });
                let selected_local_thread = db
                    .get_setting("last_active_thread_id")
                    .ok()
                    .flatten()
                    .and_then(|value| value.parse::<i64>().ok());
                if selected_local_thread == Some(thread_id) {
                    let _ = db.set_setting("last_active_thread_id", "");
                    let _ = db.set_setting("pending_profile_thread_id", "");
                    active_thread_id.replace(None);
                }
                let active_id = active_thread_id.borrow().clone();
                if let Some(active_id) = active_id {
                    if remote_thread_id.as_deref() == Some(active_id.as_str()) {
                        active_thread_id.replace(None);
                    }
                }
                if let Some((scroll, value)) = scroll_state {
                    crate::ui::widget_tree::restore_vscroll_position(&scroll, value);
                }
            }
            popover.popdown();
        });
    }

    let restore_button = build_context_item("document-revert-symbolic", "Restore…");
    {
        let popover = popover.clone();
        let row = row.clone();
        let db = db.clone();
        let manager = manager.clone();
        let current_thread_id = current_thread_id.clone();
        let active_thread_id = active_thread_id.clone();
        restore_button.connect_clicked(move |_| {
            let Some(thread_id) = current_thread_id.borrow().clone() else {
                eprintln!("restore preview unavailable: thread has no Codex id yet");
                popover.popdown();
                return;
            };

            let parent_window = row
                .root()
                .and_then(|root| root.downcast::<gtk::Window>().ok());
            super::restore_preview::open_restore_preview_dialog(
                parent_window,
                db.clone(),
                manager.resolve_client_for_thread_id(&thread_id),
                thread_id,
                active_thread_id.clone(),
                workspace_path.clone(),
                None,
            );
            popover.popdown();
        });
    }

    menu_box.append(&rename_button);
    menu_box.append(&restore_button);
    menu_box.append(&close_button);
    popover.set_child(Some(&menu_box));

    popover
}

fn build_context_item(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.set_has_frame(false);
    button.add_css_class("app-flat-button");
    button.add_css_class("chat-message-context-item");
    button.set_halign(gtk::Align::Fill);

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
