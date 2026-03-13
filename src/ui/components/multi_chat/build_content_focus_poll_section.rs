{
    {
        let panes_row = panes_row.clone();
        let mut last_seen_thread = active_codex_thread_id.borrow().clone();
        let mut last_seen_workspace = active_workspace_path.borrow().clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        let open_or_focus_thread = open_or_focus_thread.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(120), move || {
            if panes_row.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let now_thread = active_codex_thread_id.borrow().clone();
            let now_workspace = active_workspace_path.borrow().clone();
            if now_thread != last_seen_thread || now_workspace != last_seen_workspace {
                open_or_focus_thread(now_thread.clone(), now_workspace.clone());
                last_seen_thread = now_thread;
                last_seen_workspace = now_workspace;
            }
            gtk::glib::ControlFlow::Continue
        });
    }
}
