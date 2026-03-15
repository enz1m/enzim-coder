{
        while let Ok((event_profile_id, event)) = event_rx.try_recv() {
            let event_client = manager.client_for_profile(event_profile_id);
            let active_thread_id = active_thread_id.borrow().clone();
            let event_turn_id = super::codex_events::extract_turn_id(&event.params);
            let event_item_id = super::codex_events::extract_item_id(&event.params);
            let thread_id = super::codex_events::extract_thread_id(&event.params, None)
                .or_else(|| {
                    event_turn_id
                        .as_ref()
                        .and_then(|turn_id| turn_threads.borrow().get(turn_id).cloned())
                })
                .or_else(|| {
                    event_item_id
                        .as_ref()
                        .and_then(|item_id| item_threads.borrow().get(item_id).cloned())
                });
            if let Some(event_thread_id) = thread_id.as_deref() {
                let is_active_thread = active_thread_id
                    .as_deref()
                    .map(|active| active == event_thread_id)
                    .unwrap_or(false);
                if !is_active_thread {
                    mark_history_dirty_for_thread(event_thread_id);
                }
            }

            let method = event.method.as_str();
            match method {
                m if m.starts_with("turn/") => include!("attach_event_family_turn_section.rs"),
                m if m.starts_with("item/") => include!("attach_event_family_item_section.rs"),
                m if m.starts_with("thread/") => {
                    include!("attach_event_family_thread_section.rs")
                }
                "error" => include!("attach_event_family_error_section.rs"),
                "serverRequest/resolved" => {
                    include!("attach_event_family_server_request_section.rs")
                }
                _ => {}
            }
        }
}
