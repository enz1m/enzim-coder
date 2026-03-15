fn build_multi_chat_content_inner(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    codex: Option<Arc<RuntimeClient>>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_vexpand(true);

    let panes_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    panes_row.add_css_class("multi-chat-panes-row");
    panes_row.set_margin_start(0);
    panes_row.set_margin_end(0);
    panes_row.set_margin_top(0);
    panes_row.set_margin_bottom(0);
    panes_row.set_vexpand(true);

    let panes_scroll = build_panes_scroll(&panes_row);
    let panes_overlay = gtk::Overlay::new();
    panes_overlay.set_vexpand(true);
    panes_overlay.set_hexpand(true);
    panes_overlay.set_child(Some(&panes_scroll));
    let drop_slot = build_drop_slot();

    let (initial_panes, initial_columns, restored_focus, raw_saved_layout) = load_initial_layout(
        db.as_ref(),
        active_thread_id.borrow().clone(),
        active_workspace_path.borrow().clone(),
    );

    let panes_state: Rc<RefCell<Vec<PaneUi>>> = Rc::new(RefCell::new(Vec::new()));
    for persisted in &initial_panes {
        let thread_state = Rc::new(RefCell::new(persisted.thread_id.clone()));
        let workspace_state = Rc::new(RefCell::new(persisted.workspace_path.clone()));
        if let Some(pane) = build_pane_ui(
            persisted.id,
            db.clone(),
            manager.clone(),
            codex.clone(),
            thread_state,
            workspace_state,
        ) {
            set_pane_tab(&pane, &persisted.tab);
            panes_state.borrow_mut().push(pane);
        }
    }

    if panes_state.borrow().is_empty() {
        let thread_state = Rc::new(RefCell::new(active_thread_id.borrow().clone()));
        let workspace_state = Rc::new(RefCell::new(active_workspace_path.borrow().clone()));
        if let Some(pane) = build_pane_ui(
            1,
            db.clone(),
            manager.clone(),
            codex.clone(),
            thread_state,
            workspace_state,
        ) {
            panes_state.borrow_mut().push(pane);
        }
    }

    let pane_ids: Vec<u64> = panes_state.borrow().iter().map(|pane| pane.id).collect();
    let columns_state: Rc<RefCell<Vec<Vec<u64>>>> = Rc::new(RefCell::new(
        normalize_columns_for_ids(initial_columns, &pane_ids),
    ));

    let focused_pane_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(
        if panes_state
            .borrow()
            .iter()
            .any(|pane| pane.id == restored_focus)
        {
            restored_focus
        } else {
            panes_state
                .borrow()
                .first()
                .map(|pane| pane.id)
                .unwrap_or(1)
        },
    ));

    let next_pane_id = Rc::new(RefCell::new(
        panes_state
            .borrow()
            .iter()
            .map(|pane| pane.id)
            .max()
            .unwrap_or(0)
            + 1,
    ));

    let show_drop_slot: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let dragging_pane_id: Rc<RefCell<Option<u64>>> = Rc::new(RefCell::new(None));
    let active_gap_name: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let reorder_for_gaps: Rc<RefCell<Option<Rc<dyn Fn(u64, u64, bool)>>>> =
        Rc::new(RefCell::new(None));
    let thread_drop_for_slots: Rc<
        RefCell<Option<Rc<dyn Fn(Option<String>, Option<String>, InsertTarget)>>>,
    > = Rc::new(RefCell::new(None));

    let bottom_base = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bottom_base.add_css_class("multi-chat-bottom-base");
    bottom_base.set_halign(gtk::Align::Fill);
    bottom_base.set_valign(gtk::Align::End);
    bottom_base.set_hexpand(true);
    bottom_base.set_vexpand(false);
    bottom_base.set_can_target(false);
    panes_overlay.add_overlay(&bottom_base);

    let composer_holder = build_shared_composer_holder();
    panes_overlay.add_overlay(&composer_holder);

    let set_reorder_drag_active: Rc<dyn Fn(bool)> = {
        let panes_row = panes_row.clone();
        Rc::new(move |active| {
            if active {
                panes_row.add_css_class("multi-chat-reordering");
            } else {
                panes_row.remove_css_class("multi-chat-reordering");
            }
        })
    };

    let sync_global_active: Rc<dyn Fn()> = {
        let db = db.clone();
        let panes_state = panes_state.clone();
        let focused_pane_id = focused_pane_id.clone();
        let global_active_thread_id = active_thread_id.clone();
        let global_active_workspace_path = active_workspace_path.clone();
        Rc::new(move || {
            let panes = panes_state.borrow();
            let target = panes
                .iter()
                .find(|pane| pane.id == *focused_pane_id.borrow())
                .or_else(|| panes.first());
            if let Some(target) = target {
                let next_thread_id = target.active_thread_id.borrow().clone();
                global_active_thread_id.replace(next_thread_id.clone());
                global_active_workspace_path.replace(target.active_workspace_path.borrow().clone());
                if let Some(thread_id) =
                    next_thread_id.filter(|value| !value.trim().is_empty())
                {
                    if let Ok(Some(thread)) = db.get_thread_record_by_remote_thread_id(&thread_id)
                    {
                        if let Some(root) = target.root.root() {
                            let root_widget: gtk::Widget = root.upcast();
                            let _ = crate::ui::widget_tree::select_thread_row(
                                &root_widget,
                                thread.id,
                            );
                        }
                    }
                }
            }
        })
    };

    let last_layout_saved: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(raw_saved_layout));
    let persist_layout: Rc<dyn Fn()> = {
        let db = db.clone();
        let panes_state = panes_state.clone();
        let columns_state = columns_state.clone();
        let focused_pane_id = focused_pane_id.clone();
        let last_layout_saved = last_layout_saved.clone();
        Rc::new(move || {
            let panes = panes_state.borrow();
            let columns = columns_state.borrow();
            let serialized =
                serialize_persisted_layout(&panes, &columns, *focused_pane_id.borrow());
            if last_layout_saved.borrow().as_deref() == Some(serialized.as_str()) {
                return;
            }
            if db.set_setting(SETTING_PANE_LAYOUT_V1, &serialized).is_ok() {
                last_layout_saved.replace(Some(serialized));
            }
        })
    };

    let rebuild_layout: Rc<dyn Fn()> = {
        let db = db.clone();
        let panes_row = panes_row.clone();
        let panes_state = panes_state.clone();
        let columns_state = columns_state.clone();
        let drop_slot = drop_slot.clone();
        let show_drop_slot = show_drop_slot.clone();
        let reorder_for_gaps = reorder_for_gaps.clone();
        let thread_drop_for_slots = thread_drop_for_slots.clone();
        Rc::new(move || {
            while let Some(child) = panes_row.first_child() {
                panes_row.remove(&child);
            }
            let panes = panes_state.borrow();
            let mut by_id = std::collections::HashMap::new();
            for pane in panes.iter() {
                by_id.insert(pane.id, pane);
            }
            let pane_ids: Vec<u64> = panes.iter().map(|pane| pane.id).collect();
            {
                let mut cols = columns_state.borrow_mut();
                let normalized = normalize_columns_for_ids(cols.clone(), &pane_ids);
                if *cols != normalized {
                    *cols = normalized;
                }
            }
            let cols = columns_state.borrow();
            let profile_icon_visibility_by_workspace =
                sidebar_profile_icon_visibility_by_workspace_id(db.as_ref());
            let can_close = panes.len() > 1;
            if !cols.is_empty() {
                let gap = gtk::Box::new(gtk::Orientation::Vertical, 0);
                gap.add_css_class("multi-chat-between-slot");
                gap.set_vexpand(true);
                gap.set_hexpand(false);
                gap.set_size_request(4, -1);
                gap.set_widget_name(edge_gap_widget_name(true));
                let indicator = gtk::Box::new(gtk::Orientation::Vertical, 0);
                indicator.add_css_class("multi-chat-between-slot-indicator");
                indicator.set_halign(gtk::Align::Center);
                indicator.set_valign(gtk::Align::Fill);
                indicator.set_hexpand(false);
                indicator.set_vexpand(true);
                gap.append(&indicator);

                let drop_target = gtk::DropTarget::new(
                    String::static_type(),
                    gtk::gdk::DragAction::MOVE | gtk::gdk::DragAction::COPY,
                );
                {
                    let gap = gap.clone();
                    drop_target.connect_enter(move |_, _, _| {
                        gap.add_css_class("multi-chat-between-slot-hover");
                        gtk::gdk::DragAction::MOVE | gtk::gdk::DragAction::COPY
                    });
                }
                {
                    let gap = gap.clone();
                    drop_target.connect_leave(move |_| {
                        gap.remove_css_class("multi-chat-between-slot-hover");
                    });
                }
                if let Some(reorder_panes) = reorder_for_gaps.borrow().clone() {
                    let gap = gap.clone();
                    let first_id = cols[0][0];
                    let thread_drop_for_slots = thread_drop_for_slots.clone();
                    drop_target.connect_drop(move |_, value, _, _| {
                        let Ok(raw) = value.get::<String>() else {
                            gap.remove_css_class("multi-chat-between-slot-hover");
                            return false;
                        };
                        if let Some(dragged_pane_id) = parse_pane_reorder_payload(&raw) {
                            reorder_panes(dragged_pane_id, first_id, false);
                        } else if let Some((codex_thread, workspace_path)) =
                            parse_thread_drop_payload(&raw)
                        {
                            if let Some(handler) = thread_drop_for_slots.borrow().clone() {
                                handler(
                                    codex_thread,
                                    workspace_path,
                                    InsertTarget::Horizontal {
                                        target_pane_id: first_id,
                                        after: false,
                                    },
                                );
                            } else {
                                gap.remove_css_class("multi-chat-between-slot-hover");
                                return false;
                            }
                        } else {
                            gap.remove_css_class("multi-chat-between-slot-hover");
                            return false;
                        }
                        gap.remove_css_class("multi-chat-between-slot-hover");
                        true
                    });
                }
                gap.add_controller(drop_target);
                panes_row.append(&gap);
            }
            for (idx, col) in cols.iter().enumerate() {
                let col_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
                col_box.add_css_class("multi-chat-column");
                col_box.set_vexpand(true);
                col_box.set_hexpand(false);
                col_box.set_homogeneous(col.len() == 2);
                for pane_id in col {
                    if let Some(pane) = by_id.get(pane_id).copied() {
                        let thread_record = pane
                            .active_thread_id
                            .borrow()
                            .as_deref()
                            .and_then(|thread_id| {
                                db.get_thread_record_by_remote_thread_id(thread_id)
                                    .ok()
                                    .flatten()
                            });
                        let thread_title = thread_record
                            .as_ref()
                            .map(|thread| thread.title.clone())
                            .filter(|title| !title.trim().is_empty())
                            .unwrap_or_else(|| "New thread".to_string());
                        pane.title_label.set_text(&thread_title);
                        if let Some(thread) = thread_record {
                            let icon_name = profile_icon_name_for_profile(db.as_ref(), thread.profile_id);
                            pane.profile_icon.set_icon_name(Some(&icon_name));
                            let show_icon = profile_icon_visibility_by_workspace
                                .get(&thread.workspace_id)
                                .copied()
                                .unwrap_or(false);
                            pane.profile_icon.set_visible(show_icon);
                        } else {
                            pane.profile_icon.set_visible(false);
                        }
                        let workspace_name =
                            workspace_display_name(pane.active_workspace_path.borrow().as_deref());
                        pane.workspace_label.set_text(&workspace_name);
                        pane.close_button.set_visible(can_close);
                        if let Some(parent) = pane.shell.parent() {
                            if let Ok(parent_box) = parent.downcast::<gtk::Box>() {
                                parent_box.remove(&pane.shell);
                            }
                        }
                        col_box.append(&pane.shell);
                    }
                }
                panes_row.append(&col_box);

                if idx + 1 < cols.len() {
                    let left_id = col.first().copied().unwrap_or(0);
                    let right_id = cols[idx + 1].first().copied().unwrap_or(0);
                    let gap = gtk::Box::new(gtk::Orientation::Vertical, 0);
                    gap.add_css_class("multi-chat-between-slot");
                    gap.set_vexpand(true);
                    gap.set_hexpand(false);
                    gap.set_size_request(4, -1);
                    gap.set_widget_name(&gap_widget_name(left_id, right_id));
                    let indicator = gtk::Box::new(gtk::Orientation::Vertical, 0);
                    indicator.add_css_class("multi-chat-between-slot-indicator");
                    indicator.set_halign(gtk::Align::Center);
                    indicator.set_valign(gtk::Align::Fill);
                    indicator.set_hexpand(false);
                    indicator.set_vexpand(true);
                    gap.append(&indicator);

                    let drop_target = gtk::DropTarget::new(
                        String::static_type(),
                        gtk::gdk::DragAction::MOVE | gtk::gdk::DragAction::COPY,
                    );
                    {
                        let gap = gap.clone();
                        drop_target.connect_enter(move |_, _, _| {
                            gap.add_css_class("multi-chat-between-slot-hover");
                            gtk::gdk::DragAction::MOVE | gtk::gdk::DragAction::COPY
                        });
                    }
                    {
                        let gap = gap.clone();
                        drop_target.connect_leave(move |_| {
                            gap.remove_css_class("multi-chat-between-slot-hover");
                        });
                    }
                    if let Some(reorder_panes) = reorder_for_gaps.borrow().clone() {
                        let gap = gap.clone();
                        let thread_drop_for_slots = thread_drop_for_slots.clone();
                        drop_target.connect_drop(move |_, value, _, _| {
                            let Ok(raw) = value.get::<String>() else {
                                gap.remove_css_class("multi-chat-between-slot-hover");
                                return false;
                            };
                            if let Some(dragged_pane_id) = parse_pane_reorder_payload(&raw) {
                                if left_id != 0 && right_id != 0 {
                                    if dragged_pane_id == left_id {
                                        reorder_panes(dragged_pane_id, right_id, false);
                                    } else {
                                        reorder_panes(dragged_pane_id, left_id, true);
                                    }
                                } else if left_id != 0 {
                                    reorder_panes(dragged_pane_id, left_id, true);
                                }
                            } else if let Some((codex_thread, workspace_path)) =
                                parse_thread_drop_payload(&raw)
                            {
                                if left_id != 0 {
                                    if let Some(handler) = thread_drop_for_slots.borrow().clone() {
                                        handler(
                                            codex_thread,
                                            workspace_path,
                                            InsertTarget::Horizontal {
                                                target_pane_id: left_id,
                                                after: true,
                                            },
                                        );
                                    } else {
                                        gap.remove_css_class("multi-chat-between-slot-hover");
                                        return false;
                                    }
                                }
                            } else {
                                gap.remove_css_class("multi-chat-between-slot-hover");
                                return false;
                            }
                            gap.remove_css_class("multi-chat-between-slot-hover");
                            true
                        });
                    }
                    gap.add_controller(drop_target);
                    panes_row.append(&gap);
                }
            }
            if !cols.is_empty() {
                let gap = gtk::Box::new(gtk::Orientation::Vertical, 0);
                gap.add_css_class("multi-chat-between-slot");
                gap.set_vexpand(true);
                gap.set_hexpand(false);
                gap.set_size_request(4, -1);
                gap.set_widget_name(edge_gap_widget_name(false));
                let indicator = gtk::Box::new(gtk::Orientation::Vertical, 0);
                indicator.add_css_class("multi-chat-between-slot-indicator");
                indicator.set_halign(gtk::Align::Center);
                indicator.set_valign(gtk::Align::Fill);
                indicator.set_hexpand(false);
                indicator.set_vexpand(true);
                gap.append(&indicator);

                let drop_target = gtk::DropTarget::new(
                    String::static_type(),
                    gtk::gdk::DragAction::MOVE | gtk::gdk::DragAction::COPY,
                );
                {
                    let gap = gap.clone();
                    drop_target.connect_enter(move |_, _, _| {
                        gap.add_css_class("multi-chat-between-slot-hover");
                        gtk::gdk::DragAction::MOVE | gtk::gdk::DragAction::COPY
                    });
                }
                {
                    let gap = gap.clone();
                    drop_target.connect_leave(move |_| {
                        gap.remove_css_class("multi-chat-between-slot-hover");
                    });
                }
                if let Some(reorder_panes) = reorder_for_gaps.borrow().clone() {
                    let gap = gap.clone();
                    let last_id = cols[cols.len() - 1][0];
                    let thread_drop_for_slots = thread_drop_for_slots.clone();
                    drop_target.connect_drop(move |_, value, _, _| {
                        let Ok(raw) = value.get::<String>() else {
                            gap.remove_css_class("multi-chat-between-slot-hover");
                            return false;
                        };
                        if let Some(dragged_pane_id) = parse_pane_reorder_payload(&raw) {
                            reorder_panes(dragged_pane_id, last_id, true);
                        } else if let Some((codex_thread, workspace_path)) =
                            parse_thread_drop_payload(&raw)
                        {
                            if let Some(handler) = thread_drop_for_slots.borrow().clone() {
                                handler(
                                    codex_thread,
                                    workspace_path,
                                    InsertTarget::Horizontal {
                                        target_pane_id: last_id,
                                        after: true,
                                    },
                                );
                            } else {
                                gap.remove_css_class("multi-chat-between-slot-hover");
                                return false;
                            }
                        } else {
                            gap.remove_css_class("multi-chat-between-slot-hover");
                            return false;
                        }
                        gap.remove_css_class("multi-chat-between-slot-hover");
                        true
                    });
                }
                gap.add_controller(drop_target);
                panes_row.append(&gap);
            }
            if *show_drop_slot.borrow() {
                panes_row.append(&drop_slot);
            }
        })
    };

    let initial_bottom_seeded: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let rebuild_shared_composer: Rc<dyn Fn()> = {
        let db = db.clone();
        let manager = manager.clone();
        let codex = codex.clone();
        let panes_state = panes_state.clone();
        let focused_pane_id = focused_pane_id.clone();
        let composer_holder = composer_holder.clone();
        let initial_bottom_seeded = initial_bottom_seeded.clone();
        Rc::new(move || {
            let carried_text = composer_holder.first_child().and_then(|existing| {
                let existing_widget: gtk::Widget = existing.upcast();
                crate::ui::widget_tree::find_widget_by_name(&existing_widget, "composer-input-view")
                    .and_then(|widget| widget.downcast::<gtk::TextView>().ok())
                    .map(|input_view| {
                        let buffer = input_view.buffer();
                        let start = buffer.start_iter();
                        let end = buffer.end_iter();
                        buffer.text(&start, &end, true).to_string()
                    })
            });
            while let Some(child) = composer_holder.first_child() {
                composer_holder.remove(&child);
            }
            let panes = panes_state.borrow();
            let target = panes
                .iter()
                .find(|pane| pane.id == *focused_pane_id.borrow())
                .or_else(|| panes.first())
                .cloned();
            drop(panes);
            if let Some(target) = target {
                if !*initial_bottom_seeded.borrow() {
                    initial_bottom_seeded.replace(true);
                    for pane in panes_state.borrow().iter() {
                        let messages_scroll = pane.chat.messages_scroll.clone();
                        let deadline = gtk::glib::monotonic_time() + 900_000;
                        gtk::glib::timeout_add_local(Duration::from_millis(45), move || {
                            let adj = messages_scroll.vadjustment();
                            let lower = adj.lower();
                            let target = (adj.upper() - adj.page_size()).max(lower);
                            adj.set_value(target);
                            if gtk::glib::monotonic_time() < deadline {
                                gtk::glib::ControlFlow::Continue
                            } else {
                                gtk::glib::ControlFlow::Break
                            }
                        });
                    }
                }
                if target.active_thread_id.borrow().is_none() {
                    composer_holder.set_visible(false);
                    return;
                }
                let composer = chat::build_shared_composer_for_chat_target(
                    db.clone(),
                    manager.clone(),
                    codex.clone(),
                    target.active_thread_id.clone(),
                    target.active_workspace_path.clone(),
                    target.chat.messages_box.clone(),
                    target.chat.messages_scroll.clone(),
                    target.chat.conversation_stack.clone(),
                );
                composer_holder.append(&composer);
                if let Some(carried_text) = carried_text {
                    let composer_widget: gtk::Widget = composer.clone().upcast();
                    if let Some(input_view) = crate::ui::widget_tree::find_widget_by_name(
                        &composer_widget,
                        "composer-input-view",
                    )
                    .and_then(|widget| widget.downcast::<gtk::TextView>().ok())
                    {
                        let buffer = input_view.buffer();
                        buffer.set_text(&carried_text);
                        let end = buffer.end_iter();
                        buffer.place_cursor(&end);
                    }
                }
                composer_holder.set_visible(true);
            } else {
                composer_holder.set_visible(false);
            }
        })
    };

    let apply_focus_styles: Rc<dyn Fn()> = {
        let panes_state = panes_state.clone();
        let focused_pane_id = focused_pane_id.clone();
        Rc::new(move || {
            for pane in panes_state.borrow().iter() {
                if pane.id == *focused_pane_id.borrow() {
                    pane.root.add_css_class("multi-chat-pane-focused");
                } else {
                    pane.root.remove_css_class("multi-chat-pane-focused");
                }
            }
        })
    };

    let set_insert_target: Rc<dyn Fn(Option<InsertTarget>)> = {
        let panes_row = panes_row.clone();
        let panes_state = panes_state.clone();
        let columns_state = columns_state.clone();
        let active_gap_name = active_gap_name.clone();
        Rc::new(move |next_target| {
            for pane in panes_state.borrow().iter() {
                pane.root.remove_css_class("multi-chat-pane-drop-before");
                pane.root.remove_css_class("multi-chat-pane-drop-after");
                pane.shell.remove_css_class("multi-chat-pane-drop-top");
                pane.shell.remove_css_class("multi-chat-pane-drop-bottom");
            }

            let desired_gap = next_target.as_ref().and_then(|target| match target {
                InsertTarget::Horizontal {
                    target_pane_id,
                    after,
                } => {
                    let cols = columns_state.borrow();
                    let target_col = pane_position(&cols, *target_pane_id).map(|(idx, _)| idx)?;
                    if *after && target_col + 1 >= cols.len() {
                        Some(edge_gap_widget_name(false).to_string())
                    } else if !*after && target_col == 0 {
                        Some(edge_gap_widget_name(true).to_string())
                    } else if *after {
                        let right_col = cols.get(target_col + 1)?;
                        Some(gap_widget_name(cols[target_col][0], right_col[0]))
                    } else if target_col > 0 {
                        Some(gap_widget_name(
                            cols[target_col - 1][0],
                            cols[target_col][0],
                        ))
                    } else {
                        None
                    }
                }
                InsertTarget::Vertical {
                    target_pane_id,
                    below,
                } => {
                    if let Some(pane) = panes_state
                        .borrow()
                        .iter()
                        .find(|pane| pane.id == *target_pane_id)
                    {
                        if *below {
                            pane.shell.add_css_class("multi-chat-pane-drop-bottom");
                        } else {
                            pane.shell.add_css_class("multi-chat-pane-drop-top");
                        }
                    }
                    None
                }
            });

            if active_gap_name.borrow().as_ref() == desired_gap.as_ref() {
                return;
            }

            let mut child = panes_row.first_child();
            while let Some(node) = child {
                if node.has_css_class("multi-chat-between-slot") {
                    node.remove_css_class("multi-chat-between-slot-active");
                    node.remove_css_class("multi-chat-between-slot-hover");
                    if desired_gap
                        .as_deref()
                        .map(|name| node.widget_name() == name)
                        .unwrap_or(false)
                    {
                        node.add_css_class("multi-chat-between-slot-active");
                    }
                }
                child = node.next_sibling();
            }
            active_gap_name.replace(desired_gap);
        })
    };

    let clear_drop_markers: Rc<dyn Fn()> = {
        let panes_state = panes_state.clone();
        let panes_row = panes_row.clone();
        let active_gap_name = active_gap_name.clone();
        Rc::new(move || {
            for pane in panes_state.borrow().iter() {
                pane.root.remove_css_class("multi-chat-pane-drop-before");
                pane.root.remove_css_class("multi-chat-pane-drop-after");
                pane.shell.remove_css_class("multi-chat-pane-drop-top");
                pane.shell.remove_css_class("multi-chat-pane-drop-bottom");
            }
            let mut child = panes_row.first_child();
            while let Some(node) = child {
                if node.has_css_class("multi-chat-between-slot") {
                    node.remove_css_class("multi-chat-between-slot-active");
                    node.remove_css_class("multi-chat-between-slot-hover");
                }
                child = node.next_sibling();
            }
            active_gap_name.replace(None);
        })
    };

    let can_vertical_drop: Rc<dyn Fn(u64, u64) -> bool> = {
        let columns_state = columns_state.clone();
        Rc::new(move |dragged_id: u64, target_id: u64| {
            let cols = columns_state.borrow();
            let Some((drag_col_idx, _)) = pane_position(&cols, dragged_id) else {
                return false;
            };
            let Some((target_col_idx, _)) = pane_position(&cols, target_id) else {
                return false;
            };
            if drag_col_idx == target_col_idx {
                return true;
            }
            cols[target_col_idx].len() < 2
        })
    };

    let can_vertical_thread_drop: Rc<dyn Fn(u64) -> bool> = {
        let columns_state = columns_state.clone();
        Rc::new(move |target_id: u64| {
            let cols = columns_state.borrow();
            let Some((target_col_idx, _)) = pane_position(&cols, target_id) else {
                return false;
            };
            cols[target_col_idx].len() < 2
        })
    };

    let move_pane_horizontal: Rc<dyn Fn(u64, u64, bool)> = {
        let panes_state = panes_state.clone();
        let columns_state = columns_state.clone();
        let focused_pane_id = focused_pane_id.clone();
        let rebuild_layout = rebuild_layout.clone();
        let apply_focus_styles = apply_focus_styles.clone();
        let rebuild_shared_composer = rebuild_shared_composer.clone();
        let sync_global_active = sync_global_active.clone();
        let persist_layout = persist_layout.clone();
        let clear_drop_markers = clear_drop_markers.clone();
        Rc::new(move |dragged_id: u64, target_id: u64, insert_after: bool| {
            if dragged_id == target_id {
                clear_drop_markers();
                return;
            }
            let panes = panes_state.borrow();
            let has_dragged = panes.iter().any(|pane| pane.id == dragged_id);
            let has_target = panes.iter().any(|pane| pane.id == target_id);
            drop(panes);
            if !has_dragged || !has_target {
                clear_drop_markers();
                return;
            }

            let mut cols = columns_state.borrow_mut();
            let Some((from_col_idx, from_row_idx)) = pane_position(&cols, dragged_id) else {
                clear_drop_markers();
                return;
            };
            cols[from_col_idx].remove(from_row_idx);
            if cols[from_col_idx].is_empty() {
                cols.remove(from_col_idx);
            }
            let Some((target_col_idx, _)) = pane_position(&cols, target_id) else {
                cols.push(vec![dragged_id]);
                clear_drop_markers();
                drop(cols);
                rebuild_layout();
                apply_focus_styles();
                rebuild_shared_composer();
                sync_global_active();
                persist_layout();
                return;
            };
            let insert_at = if insert_after {
                target_col_idx + 1
            } else {
                target_col_idx
            };
            let cols_len = cols.len();
            cols.insert(insert_at.min(cols_len), vec![dragged_id]);
            drop(cols);

            focused_pane_id.replace(dragged_id);

            clear_drop_markers();
            rebuild_layout();
            apply_focus_styles();
            rebuild_shared_composer();
            sync_global_active();
            persist_layout();
        })
    };
    reorder_for_gaps.replace(Some(move_pane_horizontal.clone()));

    let move_pane_vertical: Rc<dyn Fn(u64, u64, bool)> = {
        let columns_state = columns_state.clone();
        let focused_pane_id = focused_pane_id.clone();
        let rebuild_layout = rebuild_layout.clone();
        let apply_focus_styles = apply_focus_styles.clone();
        let rebuild_shared_composer = rebuild_shared_composer.clone();
        let sync_global_active = sync_global_active.clone();
        let persist_layout = persist_layout.clone();
        let clear_drop_markers = clear_drop_markers.clone();
        Rc::new(move |dragged_id: u64, target_id: u64, below: bool| {
            if dragged_id == target_id {
                clear_drop_markers();
                return;
            }
            let mut cols = columns_state.borrow_mut();
            let Some((from_col_idx, from_row_idx)) = pane_position(&cols, dragged_id) else {
                clear_drop_markers();
                return;
            };
            cols[from_col_idx].remove(from_row_idx);
            if cols[from_col_idx].is_empty() {
                cols.remove(from_col_idx);
            }

            let Some((target_col_idx, target_row_idx)) = pane_position(&cols, target_id) else {
                cols.push(vec![dragged_id]);
                drop(cols);
                clear_drop_markers();
                rebuild_layout();
                apply_focus_styles();
                rebuild_shared_composer();
                sync_global_active();
                persist_layout();
                return;
            };

            if cols[target_col_idx].len() < 2 {
                let insert_at = if below {
                    (target_row_idx + 1).min(cols[target_col_idx].len())
                } else {
                    target_row_idx
                };
                cols[target_col_idx].insert(insert_at, dragged_id);
            } else {
                let insert_col = if below {
                    target_col_idx + 1
                } else {
                    target_col_idx
                };
                let cols_len = cols.len();
                cols.insert(insert_col.min(cols_len), vec![dragged_id]);
            }
            drop(cols);
            focused_pane_id.replace(dragged_id);

            clear_drop_markers();
            rebuild_layout();
            apply_focus_styles();
            rebuild_shared_composer();
            sync_global_active();
            persist_layout();
        })
    };

    let close_pane: Rc<dyn Fn(u64)> = {
        let panes_state = panes_state.clone();
        let columns_state = columns_state.clone();
        let focused_pane_id = focused_pane_id.clone();
        let rebuild_layout = rebuild_layout.clone();
        let apply_focus_styles = apply_focus_styles.clone();
        let rebuild_shared_composer = rebuild_shared_composer.clone();
        let sync_global_active = sync_global_active.clone();
        let persist_layout = persist_layout.clone();
        Rc::new(move |pane_id: u64| {
            let mut panes = panes_state.borrow_mut();
            if panes.len() <= 1 {
                return;
            }
            let removed_idx = panes.iter().position(|pane| pane.id == pane_id);
            let Some(removed_idx) = removed_idx else {
                return;
            };
            panes.remove(removed_idx);
            {
                let mut cols = columns_state.borrow_mut();
                for col in cols.iter_mut() {
                    col.retain(|id| *id != pane_id);
                }
                cols.retain(|col| !col.is_empty());
                let pane_ids: Vec<u64> = panes.iter().map(|pane| pane.id).collect();
                *cols = normalize_columns_for_ids(cols.clone(), &pane_ids);
            }
            if *focused_pane_id.borrow() == pane_id {
                if let Some(next) = panes
                    .get(
                        removed_idx
                            .saturating_sub(1)
                            .min(panes.len().saturating_sub(1)),
                    )
                    .or_else(|| panes.first())
                {
                    focused_pane_id.replace(next.id);
                }
            }
            drop(panes);
            rebuild_layout();
            apply_focus_styles();
            rebuild_shared_composer();
            sync_global_active();
            persist_layout();
        })
    };

    for pane in panes_state.borrow().iter() {
        attach_pane_handlers(
            pane,
            pane.id,
            db.clone(),
            focused_pane_id.clone(),
            dragging_pane_id.clone(),
            clear_drop_markers.clone(),
            set_insert_target.clone(),
            can_vertical_drop.clone(),
            can_vertical_thread_drop.clone(),
            thread_drop_for_slots.clone(),
            set_reorder_drag_active.clone(),
            move_pane_horizontal.clone(),
            move_pane_vertical.clone(),
            apply_focus_styles.clone(),
            rebuild_shared_composer.clone(),
            sync_global_active.clone(),
            persist_layout.clone(),
            close_pane.clone(),
        );
    }

    let open_or_focus_thread: Rc<dyn Fn(Option<String>, Option<String>)> = {
        let panes_state = panes_state.clone();
        let columns_state = columns_state.clone();
        let focused_pane_id = focused_pane_id.clone();
        let next_pane_id = next_pane_id.clone();
        let db = db.clone();
        let manager = manager.clone();
        let codex = codex.clone();
        let apply_focus_styles = apply_focus_styles.clone();
        let rebuild_shared_composer = rebuild_shared_composer.clone();
        let rebuild_layout = rebuild_layout.clone();
        let sync_global_active = sync_global_active.clone();
        let persist_layout = persist_layout.clone();
        let close_pane = close_pane.clone();
        let dragging_pane_id = dragging_pane_id.clone();
        let clear_drop_markers = clear_drop_markers.clone();
        let move_pane_horizontal = move_pane_horizontal.clone();
        let move_pane_vertical = move_pane_vertical.clone();
        let thread_drop_for_slots = thread_drop_for_slots.clone();
        Rc::new(
            move |codex_thread: Option<String>, workspace_path: Option<String>| {
                let Some(codex_thread) = codex_thread.filter(|value| !value.trim().is_empty())
                else {
                    sync_global_active();
                    rebuild_shared_composer();
                    apply_focus_styles();
                    persist_layout();
                    return;
                };

                {
                    let panes = panes_state.borrow();
                    if let Some(existing) = panes.iter().find(|pane| {
                        pane.active_thread_id.borrow().as_deref()
                            == Some(codex_thread.as_str())
                    }) {
                        let was_focused = existing.id == *focused_pane_id.borrow();
                        let workspace_changed = if let Some(next_workspace) = workspace_path.clone() {
                            let changed = existing.active_workspace_path.borrow().as_deref()
                                != Some(next_workspace.as_str());
                            if changed {
                                existing.active_workspace_path.replace(Some(next_workspace));
                            }
                            changed
                        } else {
                            false
                        };
                        if was_focused && !workspace_changed {
                            return;
                        }
                        focused_pane_id.replace(existing.id);
                        drop(panes);
                        if workspace_changed {
                            rebuild_layout();
                        }
                        apply_focus_styles();
                        rebuild_shared_composer();
                        sync_global_active();
                        persist_layout();
                        return;
                    }
                }

                let pane_id = *next_pane_id.borrow();
                next_pane_id.replace(pane_id + 1);
                let thread_state = Rc::new(RefCell::new(Some(codex_thread.clone())));
                let workspace_state = Rc::new(RefCell::new(resolve_workspace_path(
                    db.as_ref(),
                    Some(codex_thread.as_str()),
                    workspace_path,
                    None,
                )));

                if let Some(pane) = build_pane_ui(
                    pane_id,
                    db.clone(),
                    manager.clone(),
                    codex.clone(),
                    thread_state,
                    workspace_state,
                ) {
                    attach_pane_handlers(
                        &pane,
                        pane.id,
                        db.clone(),
                        focused_pane_id.clone(),
                        dragging_pane_id.clone(),
                        clear_drop_markers.clone(),
                        set_insert_target.clone(),
                        can_vertical_drop.clone(),
                        can_vertical_thread_drop.clone(),
                        thread_drop_for_slots.clone(),
                        set_reorder_drag_active.clone(),
                        move_pane_horizontal.clone(),
                        move_pane_vertical.clone(),
                        apply_focus_styles.clone(),
                        rebuild_shared_composer.clone(),
                        sync_global_active.clone(),
                        persist_layout.clone(),
                        close_pane.clone(),
                    );
                    panes_state.borrow_mut().push(pane);
                    columns_state.borrow_mut().push(vec![pane_id]);
                    focused_pane_id.replace(pane_id);
                    rebuild_layout();
                    apply_focus_styles();
                    rebuild_shared_composer();
                    sync_global_active();
                    persist_layout();
                }
            },
        )
    };

    let place_thread_at_target: Rc<dyn Fn(Option<String>, Option<String>, InsertTarget)> = {
        let open_or_focus_thread = open_or_focus_thread.clone();
        let focused_pane_id = focused_pane_id.clone();
        let move_pane_horizontal = move_pane_horizontal.clone();
        let move_pane_vertical = move_pane_vertical.clone();
        Rc::new(move |codex_thread, workspace_path, target| {
            open_or_focus_thread(codex_thread, workspace_path);
            let moved_id = *focused_pane_id.borrow();
            match target {
                InsertTarget::Horizontal {
                    target_pane_id,
                    after,
                } => {
                    if moved_id != target_pane_id {
                        move_pane_horizontal(moved_id, target_pane_id, after);
                    }
                }
                InsertTarget::Vertical {
                    target_pane_id,
                    below,
                } => {
                    if moved_id != target_pane_id {
                        move_pane_vertical(moved_id, target_pane_id, below);
                    }
                }
            }
        })
    };
    thread_drop_for_slots.replace(Some(place_thread_at_target.clone()));

    include!("build_content_drop_slot_section.rs");

    include!("build_content_focus_poll_section.rs");

    rebuild_layout();
    apply_focus_styles();
    rebuild_shared_composer();
    sync_global_active();
    persist_layout();

    root.append(&panes_overlay);
    root
}
