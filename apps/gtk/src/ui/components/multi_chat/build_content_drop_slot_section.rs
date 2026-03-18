{
    {
        let show_drop_slot = show_drop_slot.clone();
        let rebuild_layout = rebuild_layout.clone();
        let dragging_pane_id = dragging_pane_id.clone();
        let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::COPY);
        let show_drop_slot_enter = show_drop_slot.clone();
        let rebuild_layout_enter = rebuild_layout.clone();
        let dragging_pane_id_enter = dragging_pane_id.clone();
        drop_target.connect_enter(move |_, _, _| {
            if dragging_pane_id_enter.borrow().is_some() {
                return gtk::gdk::DragAction::empty();
            }
            show_drop_slot_enter.replace(true);
            rebuild_layout_enter();
            gtk::gdk::DragAction::COPY
        });
        let show_drop_slot_leave = show_drop_slot.clone();
        let rebuild_layout_leave = rebuild_layout.clone();
        drop_target.connect_leave(move |_| {
            show_drop_slot_leave.replace(false);
            rebuild_layout_leave();
        });
        let show_drop_slot_drop = show_drop_slot.clone();
        let rebuild_layout_drop = rebuild_layout.clone();
        let open_or_focus_thread = open_or_focus_thread.clone();
        let dragging_pane_id_drop = dragging_pane_id.clone();
        let db_for_drop = db.clone();
        drop_target.connect_drop(move |_, value, _, _| {
            if dragging_pane_id_drop.borrow().is_some() {
                show_drop_slot_drop.replace(false);
                rebuild_layout_drop();
                return false;
            }
            let Ok(raw) = value.get::<String>() else {
                show_drop_slot_drop.replace(false);
                rebuild_layout_drop();
                return false;
            };
            let Some((codex_thread, workspace_path)) = parse_thread_drop_payload(&raw) else {
                show_drop_slot_drop.replace(false);
                rebuild_layout_drop();
                return false;
            };
            if codex_thread
                .as_deref()
                .map(|id| !thread_exists(db_for_drop.as_ref(), id))
                .unwrap_or(true)
            {
                show_drop_slot_drop.replace(false);
                rebuild_layout_drop();
                return false;
            }
            show_drop_slot_drop.replace(false);
            rebuild_layout_drop();
            open_or_focus_thread(codex_thread, workspace_path);
            true
        });
        panes_row.add_controller(drop_target);
    }
}
