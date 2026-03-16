{
    {
        let panes_row = panes_row.clone();
        let mut last_seen_thread = active_thread_id.borrow().clone();
        let mut last_seen_workspace = active_workspace_path.borrow().clone();
        let mut last_seen_pending_profile = pending_profile_thread_local_id(db.as_ref());
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        let open_or_focus_thread = open_or_focus_thread.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(120), move || {
            if panes_row.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let now_thread = active_thread_id.borrow().clone();
            let now_workspace = active_workspace_path.borrow().clone();
            let now_pending_profile = pending_profile_thread_local_id(db.as_ref());
            if now_thread != last_seen_thread
                || now_workspace != last_seen_workspace
                || now_pending_profile != last_seen_pending_profile
            {
                open_or_focus_thread(now_thread.clone(), now_workspace.clone());
                last_seen_thread = now_thread;
                last_seen_workspace = now_workspace;
                last_seen_pending_profile = now_pending_profile;
            }
            gtk::glib::ControlFlow::Continue
        });
    }
}
