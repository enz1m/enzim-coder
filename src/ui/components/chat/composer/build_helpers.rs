fn install_turn_started_retagger(
    messages_box: &gtk::Box,
    turn_started_ui_rx: mpsc::Receiver<(String, String)>,
) {
    let messages_box = messages_box.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        if messages_box.root().is_none() {
            return gtk::glib::ControlFlow::Break;
        }
        while let Ok((pending_marker, turn_id)) = turn_started_ui_rx.try_recv() {
            let _ = super::message_render::retag_message_row(
                &messages_box,
                &pending_marker,
                &format!("turn-user-row:{turn_id}"),
            );
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn install_send_error_poller(
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    conversation_stack: &gtk::Stack,
    send_error_rx: mpsc::Receiver<String>,
) {
    let messages_box = messages_box.clone();
    let messages_scroll = messages_scroll.clone();
    let conversation_stack = conversation_stack.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        if messages_box.root().is_none() {
            return gtk::glib::ControlFlow::Break;
        }
        while let Ok(message) = send_error_rx.try_recv() {
            super::message_render::append_message(
                &messages_box,
                Some(&messages_scroll),
                &conversation_stack,
                &message,
                false,
                std::time::SystemTime::now(),
            );
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn install_steer_note_poller(
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    steer_note_rx: mpsc::Receiver<String>,
) {
    let messages_box = messages_box.clone();
    let messages_scroll = messages_scroll.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(50), move || {
        if messages_box.root().is_none() {
            return gtk::glib::ControlFlow::Break;
        }
        while let Ok(note) = steer_note_rx.try_recv() {
            let _ = super::message_render::append_steer_note_to_last_user_message(
                &messages_box,
                &messages_scroll,
                &note,
            );
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn build_default_suggestion_row() -> (gtk::Box, i32) {
    let suggestion_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    suggestion_row.set_halign(gtk::Align::Center);
    for item in [
        "Explain this project",
        "Find and fix bugs in my code",
        "Update my Readme.md",
    ] {
        let chip = create_suggestion_chip(item);
        suggestion_row.append(&chip);
    }
    let (_, natural_width, _, _) = suggestion_row.measure(gtk::Orientation::Horizontal, -1);
    (suggestion_row, natural_width)
}
