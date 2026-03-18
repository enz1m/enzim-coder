{
    let voice_capture_state: Rc<RefCell<voice::VoiceCaptureState>> =
        Rc::new(RefCell::new(voice::VoiceCaptureState::default()));

    let open_voice_settings: Rc<dyn Fn()> = {
        let db = db.clone();
        let manager = manager.clone();
        let mic = mic.clone();
        Rc::new(move || {
            let parent = mic.root().and_then(|root| root.downcast::<gtk::Window>().ok());
            voice::open_voice_settings_dialog(parent, db.clone(), manager.clone());
        })
    };

    {
        let open_voice_settings = open_voice_settings.clone();
        let menu = gtk::Popover::new();
        menu.set_has_arrow(false);
        menu.set_autohide(true);
        menu.set_offset(0, 0);
        menu.set_parent(&mic);
        let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        menu_box.add_css_class("chat-message-context-menu");
        menu_box.set_margin_start(6);
        menu_box.set_margin_end(6);
        menu_box.set_margin_top(6);
        menu_box.set_margin_bottom(6);
        let settings = gtk::Button::with_label("Voice Settings");
        settings.add_css_class("app-flat-button");
        settings.add_css_class("chat-message-context-item");
        {
            let menu = menu.clone();
            settings.connect_clicked(move |_| {
                (open_voice_settings)();
                menu.popdown();
            });
        }
        menu_box.append(&settings);
        menu.set_child(Some(&menu_box));

        let right_click = gtk::GestureClick::builder().button(3).build();
        {
            let menu = menu.clone();
            right_click.connect_pressed(move |gesture, _, x, y| {
                menu.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                menu.popup();
                gesture.set_state(gtk::EventSequenceState::Claimed);
            });
        }
        mic.add_controller(right_click);
    }

    {
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        let voice_capture_state = voice_capture_state.clone();
        let mic = mic.clone();
        let input_view = input_view.clone();
        let input_scroll = input_scroll.clone();
        let placeholder = placeholder.clone();
        let voice_activity_label = voice_activity_label.clone();
        let messages_box = messages_box.clone();
        let messages_scroll = messages_scroll.clone();
        let conversation_stack = conversation_stack.clone();
        let open_voice_settings = open_voice_settings.clone();
        let voice_activity_text: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        let voice_activity_source: Rc<RefCell<Option<gtk::glib::SourceId>>> =
            Rc::new(RefCell::new(None));
        let start_voice_activity: Rc<dyn Fn(&str)> = {
            let input_view = input_view.clone();
            let voice_activity_label = voice_activity_label.clone();
            let placeholder = placeholder.clone();
            let voice_activity_text = voice_activity_text.clone();
            let voice_activity_source = voice_activity_source.clone();
            Rc::new(move |status: &str| {
                *voice_activity_text.borrow_mut() = status.to_string();
                placeholder.set_visible(false);
                input_view.set_opacity(0.0);
                voice_activity_label.set_visible(true);
                if voice_activity_source.borrow().is_some() {
                    return;
                }
                let voice_activity_label = voice_activity_label.clone();
                let voice_activity_text = voice_activity_text.clone();
                let source = gtk::glib::timeout_add_local(Duration::from_millis(70), move || {
                    if voice_activity_label.root().is_none() {
                        return gtk::glib::ControlFlow::Break;
                    }
                    let status_text = voice_activity_text.borrow().clone();
                    let phase = gtk::glib::monotonic_time() as f64 / 90_000.0;
                    voice_activity_label.set_use_markup(true);
                    voice_activity_label.set_markup(
                        &crate::ui::components::chat::sidebar_wave_status_markup(
                            &status_text,
                            phase,
                        ),
                    );
                    gtk::glib::ControlFlow::Continue
                });
                *voice_activity_source.borrow_mut() = Some(source);
            })
        };
        let stop_voice_activity: Rc<dyn Fn()> = {
            let input_view = input_view.clone();
            let placeholder = placeholder.clone();
            let voice_activity_label = voice_activity_label.clone();
            let voice_activity_source = voice_activity_source.clone();
            Rc::new(move || {
                if let Some(source) = voice_activity_source.borrow_mut().take() {
                    source.remove();
                }
                input_view.set_opacity(1.0);
                voice_activity_label.set_use_markup(false);
                voice_activity_label.set_text("");
                voice_activity_label.set_visible(false);
                let buf = input_view.buffer();
                let start = buf.start_iter();
                let end = buf.end_iter();
                let is_empty = buf.text(&start, &end, true).trim().is_empty();
                placeholder.set_visible(is_empty);
            })
        };
        let mic_for_click = mic.clone();
        let mic_for_click_handler = mic.clone();
        mic_for_click.connect_clicked(move |_| {
            let is_locked = active_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| db.is_remote_thread_locked(thread_id).ok())
                .or_else(|| {
                    db.get_setting("last_active_thread_id")
                        .ok()
                        .flatten()
                        .and_then(|value| value.parse::<i64>().ok())
                        .and_then(|thread_id| db.is_local_thread_locked(thread_id).ok())
                })
                .unwrap_or(false);
            if is_locked {
                super::message_render::append_message(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    "This thread is locked to another account. Voice input is unavailable.",
                    false,
                    std::time::SystemTime::now(),
                );
                return;
            }

            if voice_capture_state.borrow().transcribing {
                return;
            }

            if voice_capture_state.borrow().recording_child.is_none() {
                if voice::ensure_ffmpeg_available().is_err() {
                    (open_voice_settings)();
                    return;
                }
                let config = db.voice_to_text_config().ok().flatten();
                let Some(config) = config else {
                    (open_voice_settings)();
                    return;
                };
                if !config.is_valid() {
                    (open_voice_settings)();
                    return;
                }
                let recording_dir = crate::services::app::chat::default_app_data_dir().join("voice_tmp");
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let audio_path = recording_dir.join(format!("recording-{now}.wav"));
                match voice::start_voice_recording(&audio_path) {
                    Ok(child) => {
                        let mut state = voice_capture_state.borrow_mut();
                        state.recording_child = Some(child);
                        state.recording_path = Some(audio_path);
                        mic_for_click_handler.set_icon_name("media-playback-stop-symbolic");
                        mic_for_click_handler.set_tooltip_text(Some("Stop voice recording"));
                        (start_voice_activity)("Listening...");
                    }
                    Err(err) => {
                        (stop_voice_activity)();
                        super::message_render::append_message(
                            &messages_box,
                            Some(&messages_scroll),
                            &conversation_stack,
                            &format!("Voice recording failed to start: {err}"),
                            false,
                            std::time::SystemTime::now(),
                        );
                    }
                }
                return;
            }

            let (mut child, audio_path) = {
                let mut state = voice_capture_state.borrow_mut();
                let Some(child) = state.recording_child.take() else {
                    return;
                };
                let Some(path) = state.recording_path.take() else {
                    return;
                };
                (child, path)
            };
            if let Err(err) = voice::stop_voice_recording(&mut child) {
                (stop_voice_activity)();
                super::message_render::append_message(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    &format!("Voice recording failed to finalize: {err}"),
                    false,
                    std::time::SystemTime::now(),
                );
                mic_for_click_handler.set_icon_name("audio-input-microphone-symbolic");
                mic_for_click_handler.set_tooltip_text(Some("Voice input"));
                let _ = std::fs::remove_file(&audio_path);
                return;
            }
            let audio_size = std::fs::metadata(&audio_path).map(|meta| meta.len()).unwrap_or(0);
            if audio_size < 128 {
                (stop_voice_activity)();
                super::message_render::append_message(
                    &messages_box,
                    Some(&messages_scroll),
                    &conversation_stack,
                    "Voice recording is empty or too short. Hold the microphone button a bit longer and try again.",
                    false,
                    std::time::SystemTime::now(),
                );
                mic_for_click_handler.set_icon_name("audio-input-microphone-symbolic");
                mic_for_click_handler.set_tooltip_text(Some("Voice input"));
                let _ = std::fs::remove_file(&audio_path);
                return;
            }

            let config = db
                .voice_to_text_config()
                .ok()
                .flatten()
                .unwrap_or_default();
            if !config.is_valid() {
                (stop_voice_activity)();
                (open_voice_settings)();
                mic_for_click_handler.set_icon_name("audio-input-microphone-symbolic");
                mic_for_click_handler.set_tooltip_text(Some("Voice input"));
                return;
            }

            (start_voice_activity)("Transcribing...");
            voice_capture_state.borrow_mut().transcribing = true;
            mic_for_click_handler.set_sensitive(false);
            mic_for_click_handler.set_icon_name("view-refresh-symbolic");
            mic_for_click_handler.set_tooltip_text(Some("Transcribing..."));

            let (tx, rx) = mpsc::channel::<Result<String, String>>();
            thread::spawn(move || {
                let _ = tx.send(voice::transcribe_audio(&config, &audio_path));
                let _ = std::fs::remove_file(&audio_path);
            });

            let mic_for_result = mic_for_click_handler.clone();
            let voice_capture_state_for_result = voice_capture_state.clone();
            let input_view_for_result = input_view.clone();
            let input_scroll_for_result = input_scroll.clone();
            let placeholder_for_result = placeholder.clone();
            let stop_voice_activity_for_result = stop_voice_activity.clone();
            let messages_box_for_result = messages_box.clone();
            let messages_scroll_for_result = messages_scroll.clone();
            let conversation_stack_for_result = conversation_stack.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(40), move || match rx.try_recv() {
                Ok(Ok(text)) => {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        let buf = input_view_for_result.buffer();
                        let mut insert_at = buf.end_iter();
                        let start = buf.start_iter();
                        let end = buf.end_iter();
                        let existing = buf.text(&start, &end, true).to_string();
                        if !existing.trim().is_empty() {
                            buf.insert(&mut insert_at, "\n\n");
                        }
                        buf.insert(&mut insert_at, trimmed);
                        placeholder_for_result.set_visible(false);
                        update_input_height(
                            &input_scroll_for_result,
                            &input_view_for_result,
                            min_height,
                            max_height,
                        );
                    }
                    (stop_voice_activity_for_result)();
                    let mut state = voice_capture_state_for_result.borrow_mut();
                    state.transcribing = false;
                    mic_for_result.set_sensitive(true);
                    mic_for_result.set_icon_name("audio-input-microphone-symbolic");
                    mic_for_result.set_tooltip_text(Some("Voice input"));
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    super::message_render::append_message(
                        &messages_box_for_result,
                        Some(&messages_scroll_for_result),
                        &conversation_stack_for_result,
                        &format!("Voice transcription failed: {err}"),
                        false,
                        std::time::SystemTime::now(),
                    );
                    (stop_voice_activity_for_result)();
                    let mut state = voice_capture_state_for_result.borrow_mut();
                    state.transcribing = false;
                    mic_for_result.set_sensitive(true);
                    mic_for_result.set_icon_name("audio-input-microphone-symbolic");
                    mic_for_result.set_tooltip_text(Some("Voice input"));
                    gtk::glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    (stop_voice_activity_for_result)();
                    let mut state = voice_capture_state_for_result.borrow_mut();
                    state.transcribing = false;
                    mic_for_result.set_sensitive(true);
                    mic_for_result.set_icon_name("audio-input-microphone-symbolic");
                    mic_for_result.set_tooltip_text(Some("Voice input"));
                    gtk::glib::ControlFlow::Break
                }
            });
        });
    }
}
