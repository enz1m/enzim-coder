fn build_attach_menu_item(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.set_has_frame(false);
    button.set_halign(gtk::Align::Fill);
    button.set_hexpand(true);
    button.add_css_class("app-flat-button");
    button.add_css_class("composer-attach-menu-item");

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.add_css_class("composer-attach-menu-item-row");

    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(14);
    icon.add_css_class("composer-attach-menu-item-icon");
    row.append(&icon);

    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.set_hexpand(true);
    text.add_css_class("composer-attach-menu-item-label");
    row.append(&text);

    button.set_child(Some(&row));
    button
}

fn build_inner(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    codex: Option<Arc<RuntimeClient>>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
    messages_box: gtk::Box,
    messages_scroll: gtk::ScrolledWindow,
    conversation_stack: gtk::Stack,
    _active_turn: Rc<RefCell<Option<String>>>,
    _active_turn_thread: Rc<RefCell<Option<String>>>,
) -> ComposerSection {
    let (send_error_tx, send_error_rx) = mpsc::channel::<String>();
    let (steer_note_tx, steer_note_rx) = mpsc::channel::<String>();
    let (turn_started_ui_tx, turn_started_ui_rx) = mpsc::channel::<(String, String)>();
    let resolve_client_for_thread: Rc<dyn Fn(&str) -> Option<Arc<RuntimeClient>>> = {
        let db = db.clone();
        let manager = manager.clone();
        Rc::new(move |thread_id: &str| {
            manager.resolve_client_for_thread_id(thread_id).or_else(|| {
                db.runtime_profile_id()
                    .ok()
                    .flatten()
                    .and_then(|profile_id| manager.client_for_profile(profile_id))
            })
        })
    };
    let queue_steer_allowed_for_thread: Rc<dyn Fn(Option<&str>) -> bool> = {
        let db = db.clone();
        let manager = manager.clone();
        let resolve_client_for_thread = resolve_client_for_thread.clone();
        Rc::new(move |thread_id: Option<&str>| {
            thread_id
                .and_then(|id| resolve_client_for_thread(id))
                .or_else(|| {
                    db.runtime_profile_id()
                        .ok()
                        .flatten()
                        .and_then(|profile_id| manager.client_for_profile(profile_id))
                })
                .map(|client| !client.backend_kind().eq_ignore_ascii_case("opencode"))
                .unwrap_or(true)
        })
    };

    install_turn_started_retagger(&messages_box, turn_started_ui_rx);
    install_send_error_poller(
        &messages_box,
        &messages_scroll,
        &conversation_stack,
        send_error_rx,
    );
    install_steer_note_poller(&messages_box, &messages_scroll, steer_note_rx);

    let lower_content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    lower_content.add_css_class("composer-floating-shell");

    let (suggestion_row, suggestion_row_natural_width) = build_default_suggestion_row();
    lower_content.append(&suggestion_row);

    let queued_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    queued_box.add_css_class("chat-queued-list");
    queued_box.set_visible(false);
    lower_content.append(&queued_box);
    let queued_entries: Rc<RefCell<VecDeque<QueuedUiEntry>>> =
        Rc::new(RefCell::new(VecDeque::new()));
    let queued_next_id: Rc<RefCell<u64>> = Rc::new(RefCell::new(1));
    let queued_dispatch_state: Rc<RefCell<Option<(String, i64, bool)>>> =
        Rc::new(RefCell::new(None));
    let queue_expanded: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));

    let queued_merge_button = gtk::Button::with_label("Merge queued");
    queued_merge_button.add_css_class("app-flat-button");
    queued_merge_button.add_css_class("chat-queued-merge");
    queued_merge_button.set_halign(gtk::Align::End);
    queued_merge_button.set_visible(false);
    lower_content.append(&queued_merge_button);

    let live_turn_status_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    live_turn_status_row.add_css_class("chat-live-status-row");
    live_turn_status_row.set_halign(gtk::Align::Fill);
    live_turn_status_row.set_hexpand(true);

    let timer_box = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    timer_box.add_css_class("chat-live-status-timer");
    let timer_icon = gtk::Image::from_icon_name("alarm-symbolic");
    timer_icon.add_css_class("chat-live-status-icon");
    timer_icon.set_pixel_size(12);
    timer_box.append(&timer_icon);
    let live_turn_timer_label = gtk::Label::new(Some("00:00"));
    live_turn_timer_label.add_css_class("chat-live-status-timer-label");
    live_turn_timer_label.set_xalign(0.0);
    timer_box.append(&live_turn_timer_label);
    live_turn_status_row.append(&timer_box);

    let queue_summary_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    queue_summary_box.add_css_class("chat-queue-summary-box");
    queue_summary_box.set_halign(gtk::Align::Center);
    queue_summary_box.set_valign(gtk::Align::Center);
    queue_summary_box.set_visible(false);

    let queue_summary_label = gtk::Label::new(None);
    queue_summary_label.add_css_class("chat-queue-summary-label");
    queue_summary_label.set_xalign(0.5);
    queue_summary_box.append(&queue_summary_label);

    let queue_summary_sep_left = gtk::Label::new(Some("-"));
    queue_summary_sep_left.add_css_class("chat-queue-summary-separator");
    queue_summary_box.append(&queue_summary_sep_left);

    let queue_summary_steer = gtk::Label::new(Some("Steer"));
    queue_summary_steer.add_css_class("chat-queue-summary-action");
    queue_summary_steer.set_selectable(false);
    queue_summary_box.append(&queue_summary_steer);

    let queue_summary_sep_right = gtk::Label::new(Some("-"));
    queue_summary_sep_right.add_css_class("chat-queue-summary-separator");
    queue_summary_box.append(&queue_summary_sep_right);

    let queue_summary_toggle = gtk::Label::new(Some("↑"));
    queue_summary_toggle.add_css_class("chat-queue-summary-action");
    queue_summary_toggle.add_css_class("chat-queue-summary-toggle");
    queue_summary_toggle.set_selectable(false);
    queue_summary_box.append(&queue_summary_toggle);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    live_turn_status_row.append(&spacer);

    let live_turn_status_label = gtk::Label::new(Some("Working..."));
    live_turn_status_label.add_css_class("chat-live-status-text");
    live_turn_status_label.set_xalign(1.0);
    live_turn_status_label.set_wrap(false);
    live_turn_status_label.set_single_line_mode(true);
    live_turn_status_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    live_turn_status_row.append(&live_turn_status_label);

    let live_turn_status_overlay = gtk::Overlay::new();
    live_turn_status_overlay.set_child(Some(&live_turn_status_row));
    live_turn_status_overlay.add_overlay(&queue_summary_box);

    let live_turn_status_revealer = gtk::Revealer::new();
    live_turn_status_revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
    live_turn_status_revealer.set_transition_duration(180);
    live_turn_status_revealer.set_reveal_child(false);
    live_turn_status_revealer.set_visible(false);
    live_turn_status_revealer.set_child(Some(&live_turn_status_overlay));

    let composer_cluster = gtk::Box::new(gtk::Orientation::Vertical, 4);
    composer_cluster.append(&live_turn_status_revealer);

    let composer = gtk::Box::new(gtk::Orientation::Vertical, 8);
    composer.add_css_class("composer");
    composer.add_css_class("composer-floating");

    let thread_lock_note = gtk::Label::new(None);
    thread_lock_note.add_css_class("composer-lock-note");
    thread_lock_note.set_xalign(0.0);
    thread_lock_note.set_wrap(true);
    thread_lock_note.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    thread_lock_note.set_visible(false);

    let input_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .build();
    input_scroll.set_has_frame(false);
    input_scroll.set_widget_name("composer-input-scroll");
    input_scroll.add_css_class("composer-input");

    let input_view = gtk::TextView::new();
    input_view.set_widget_name("composer-input-view");
    input_view.add_css_class("composer-input-view");
    input_view.set_wrap_mode(gtk::WrapMode::WordChar);
    input_view.set_accepts_tab(false);
    input_view.set_monospace(false);
    input_view.set_top_margin(8);
    input_view.set_bottom_margin(8);
    input_view.set_left_margin(12);
    input_view.set_right_margin(40);
    input_view.set_vexpand(false);
    input_view.set_hexpand(true);
    input_view.set_cursor_visible(true);
    input_scroll.set_child(Some(&input_view));

    let mention_popover = gtk::Popover::new();
    mention_popover.set_has_arrow(false);
    mention_popover.set_autohide(true);
    mention_popover.set_position(gtk::PositionType::Top);
    mention_popover.set_parent(&input_view);
    mention_popover.set_focusable(false);

    let mention_listbox = gtk::ListBox::new();
    mention_listbox.add_css_class("navigation-sidebar");
    mention_listbox.add_css_class("composer-attach-picker-list");
    mention_listbox.set_selection_mode(gtk::SelectionMode::Single);
    mention_listbox.set_focusable(false);

    let mention_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_width(280)
        .min_content_height(160)
        .max_content_height(280)
        .child(&mention_listbox)
        .build();
    mention_scroll.set_has_frame(false);
    mention_scroll.set_focusable(false);
    mention_scroll.add_css_class("composer-attach-picker-scroll");
    mention_scroll.set_overflow(gtk::Overflow::Hidden);
    mention_popover.set_child(Some(&mention_scroll));

    let selected_mentions: Rc<RefCell<Vec<MentionAttachment>>> = Rc::new(RefCell::new(Vec::new()));
    let selected_images: Rc<RefCell<Vec<ImageAttachment>>> = Rc::new(RefCell::new(Vec::new()));
    let mention_files: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let mention_files_root: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let filtered_mentions: Rc<RefCell<Vec<(String, String)>>> = Rc::new(RefCell::new(Vec::new()));

    {
        let mention_popover = mention_popover.clone();
        let mention_listbox = mention_listbox.clone();
        let input_view = input_view.clone();
        let selected_mentions = selected_mentions.clone();
        let filtered_mentions = filtered_mentions.clone();
        mention_listbox.connect_row_activated(move |_, row| {
            let index = row.index();
            if index < 0 {
                return;
            }
            let selected = { filtered_mentions.borrow().get(index as usize).cloned() };
            let Some((display, path)) = selected else {
                return;
            };

            if insert_selected_mention(&input_view, &display) {
                if !selected_mentions.borrow().iter().any(|m| m.path == path) {
                    selected_mentions
                        .borrow_mut()
                        .push(MentionAttachment { display, path });
                }
            }

            mention_popover.popdown();
            input_view.grab_focus();
        });
    }

    {
        let mention_popover = mention_popover.clone();
        let mention_popover_for_keys = mention_popover.clone();
        let mention_listbox = mention_listbox.clone();
        let mention_scroll = mention_scroll.clone();
        let filtered_mentions = filtered_mentions.clone();
        let mention_files = mention_files.clone();
        let input_view = input_view.clone();
        let selected_mentions = selected_mentions.clone();
        let popup_key_controller = gtk::EventControllerKey::new();
        popup_key_controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        popup_key_controller.connect_key_pressed(move |_, key, _, state| {
            if key == gtk::gdk::Key::Down {
                move_mention_selection(&mention_listbox, &mention_scroll, 1);
                return gtk::glib::Propagation::Stop;
            }
            if key == gtk::gdk::Key::Up {
                move_mention_selection(&mention_listbox, &mention_scroll, -1);
                return gtk::glib::Propagation::Stop;
            }
            if key == gtk::gdk::Key::Escape {
                mention_popover_for_keys.popdown();
                return gtk::glib::Propagation::Stop;
            }

            let is_enter = key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter;
            if is_enter {
                let row = mention_listbox
                    .selected_row()
                    .or_else(|| mention_listbox.row_at_index(0));
                if let Some(row) = row {
                    let index = row.index();
                    let selected = { filtered_mentions.borrow().get(index as usize).cloned() };
                    if let Some((display, path)) = selected {
                        let _ = insert_selected_mention(&input_view, &display);
                        mention_popover_for_keys.popdown();
                        if !selected_mentions.borrow().iter().any(|m| m.path == path) {
                            selected_mentions
                                .borrow_mut()
                                .push(MentionAttachment { display, path });
                        }
                    }
                }
                return gtk::glib::Propagation::Stop;
            }

            if apply_mention_key_input(&input_view, key, state) {
                refresh_mention_popup(
                    &mention_popover_for_keys,
                    &mention_listbox,
                    &filtered_mentions,
                    &mention_files,
                    &input_view,
                );
                return gtk::glib::Propagation::Stop;
            }

            gtk::glib::Propagation::Proceed
        });
        mention_popover.add_controller(popup_key_controller);
    }

    let local_css = gtk::CssProvider::new();
    local_css.load_from_string(
        r#"
        #composer-input-scroll,
        #composer-input-scroll > viewport,
        #composer-input-scroll > viewport > textview.view,
        textview#composer-input-view,
        textview#composer-input-view text {
          background: transparent;
          background-color: transparent;
          background-image: none;
          color: @window_fg_color;
          border-width: 0;
          border-style: none;
          border-color: transparent;
          box-shadow: unset;
        }

        scrolledwindow#composer-input-scroll > scrollbar.vertical,
        scrolledwindow#composer-input-scroll > scrollbar.vertical > range,
        scrolledwindow#composer-input-scroll > scrollbar.vertical > range > trough,
        scrolledwindow#composer-input-scroll > scrollbar.vertical > range > trough > slider {
          min-width: 0;
          min-height: 0;
          margin: 0;
          padding: 0;
          border: none;
          box-shadow: none;
          background: transparent;
          background-color: transparent;
          background-image: none;
          opacity: 0;
        }

        button.composer-selector-button,
        button#composer-selector-button,
        button.composer-input-mic,
        button.composer-enzim-agent-button,
        button.composer-worktree-button {
          background-color: transparent;
          background-image: none;
          border-color: transparent;
          box-shadow: none;
        }

        button.composer-selector-button:hover,
        button#composer-selector-button:hover,
        button.composer-input-mic:hover,
        button.composer-enzim-agent-button:hover,
        button.composer-worktree-button:hover {
          background-color: transparent;
        }

        button.composer-selector-button:active,
        button.composer-selector-button:checked,
        button#composer-selector-button:active,
        button#composer-selector-button:checked,
        button.composer-input-mic:active,
        button.composer-enzim-agent-button:active,
        button.composer-worktree-button:active {
          background-color: transparent;
        }

        button.composer-attach-trigger,
        button.send-button {
          background-color: transparent;
          background-image: none;
          border-color: transparent;
          box-shadow: none;
        }

        button.composer-attach-trigger:hover,
        button.send-button:hover {
          background-color: transparent;
        }

        button.composer-attach-trigger:active,
        button.composer-attach-trigger:checked,
        button.composer-attach-trigger:focus,
        button.composer-attach-trigger:focus-visible,
        button.send-button:active {
          background-color: transparent;
        }
        "#,
    );
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &local_css,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
    }

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&input_scroll));

    const DEFAULT_COMPOSER_PLACEHOLDER: &str = "Ask anything, @ to add files, / for commands";
    const STEER_QUEUE_PLACEHOLDER: &str = "Press Shift+Enter to steer queued message";

    let placeholder = gtk::Label::new(Some(DEFAULT_COMPOSER_PLACEHOLDER));
    placeholder.add_css_class("composer-placeholder");
    placeholder.set_halign(gtk::Align::Start);
    placeholder.set_valign(gtk::Align::Start);
    placeholder.set_margin_start(12);
    placeholder.set_margin_top(10);
    placeholder.set_can_target(false);
    overlay.add_overlay(&placeholder);

    let refresh_placeholder_text: Rc<dyn Fn()> = {
        let input_view = input_view.clone();
        let placeholder = placeholder.clone();
        let queued_entries = queued_entries.clone();
        let active_thread_id = active_thread_id.clone();
        let queue_steer_allowed_for_thread = queue_steer_allowed_for_thread.clone();
        Rc::new(move || {
            let buf = input_view.buffer();
            let start = buf.start_iter();
            let end = buf.end_iter();
            let text = buf.text(&start, &end, true);
            let is_empty = text.trim().is_empty();
            let queued_thread_id = queued_entries
                .borrow()
                .front()
                .and_then(|entry| entry.payload.borrow().expected_thread_id.clone())
                .or_else(|| active_thread_id.borrow().clone());
            let can_steer_queue = !queued_entries.borrow().is_empty()
                && queue_steer_allowed_for_thread(queued_thread_id.as_deref());
            placeholder.set_text(if is_empty && can_steer_queue {
                STEER_QUEUE_PLACEHOLDER
            } else {
                DEFAULT_COMPOSER_PLACEHOLDER
            });
        })
    };
    (refresh_placeholder_text)();

    let voice_activity_label = gtk::Label::new(None);
    voice_activity_label.add_css_class("composer-voice-activity");
    voice_activity_label.set_halign(gtk::Align::Start);
    voice_activity_label.set_valign(gtk::Align::Start);
    voice_activity_label.set_margin_start(12);
    voice_activity_label.set_margin_top(10);
    voice_activity_label.set_can_target(false);
    voice_activity_label.set_use_markup(true);
    voice_activity_label.set_visible(false);
    overlay.add_overlay(&voice_activity_label);

    let mic = gtk::Button::builder()
        .icon_name("audio-input-microphone-symbolic")
        .build();
    mic.set_has_frame(false);
    mic.add_css_class("app-flat-button");
    mic.add_css_class("composer-icon-button");
    mic.add_css_class("composer-input-mic");
    mic.set_halign(gtk::Align::End);
    mic.set_valign(gtk::Align::End);
    mic.set_margin_end(0);
    mic.set_margin_bottom(2);
    overlay.add_overlay(&mic);

    let layout = input_view.create_pango_layout(Some("Ag"));
    let (_, line_height) = layout.pixel_size();
    let min_height = line_height + 18;
    let max_height = line_height * 5 + 18;
    update_input_height(&input_scroll, &input_view, min_height, max_height);

    include!("build_body_voice_section.rs");

    let image_preview_strip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    image_preview_strip.add_css_class("composer-image-strip");
    image_preview_strip.set_visible(false);

    let image_preview_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(58)
        .child(&image_preview_strip)
        .build();
    image_preview_scroll.set_has_frame(false);
    image_preview_scroll.add_css_class("composer-image-strip-scroll");
    image_preview_scroll.set_visible(false);

    composer.append(&image_preview_scroll);
    composer.append(&overlay);
    composer.append(&thread_lock_note);

    let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    controls.add_css_class("composer-controls");
    let add_file = gtk::Button::new();
    add_file.set_has_frame(false);
    let add_file_label = gtk::Label::new(Some("+"));
    add_file_label.add_css_class("composer-plus-label");
    add_file.set_child(Some(&add_file_label));
    add_file.add_css_class("app-flat-button");
    add_file.add_css_class("composer-attach-trigger");
    controls.append(&add_file);

    let add_menu_popover = gtk::Popover::new();
    add_menu_popover.set_has_arrow(false);
    add_menu_popover.set_autohide(true);
    add_menu_popover.set_position(gtk::PositionType::Top);
    add_menu_popover.set_parent(&add_file);
    add_menu_popover.add_css_class("composer-attach-popover");

    let add_menu_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    add_menu_box.add_css_class("composer-attach-menu");
    let add_menu_file_button = build_attach_menu_item("text-x-generic-symbolic", "File");
    let add_menu_image_button = build_attach_menu_item("image-x-generic-symbolic", "Image");
    add_menu_box.append(&add_menu_file_button);
    add_menu_box.append(&add_menu_image_button);
    add_menu_popover.set_child(Some(&add_menu_box));

    let add_picker_popover = gtk::Popover::new();
    add_picker_popover.set_has_arrow(false);
    add_picker_popover.set_autohide(true);
    add_picker_popover.set_position(gtk::PositionType::Top);
    add_picker_popover.set_parent(&add_file);
    add_picker_popover.set_size_request(560, -1);
    add_picker_popover.add_css_class("composer-attach-picker-popover");

    let add_picker_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    add_picker_box.set_size_request(560, -1);
    add_picker_box.set_margin_start(8);
    add_picker_box.set_margin_end(8);
    add_picker_box.set_margin_top(8);
    add_picker_box.set_margin_bottom(8);
    add_picker_box.add_css_class("composer-attach-picker-box");

    let add_picker_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    add_picker_header.add_css_class("composer-attach-picker-header");
    let add_picker_back = gtk::Button::builder()
        .icon_name("pan-start-symbolic")
        .build();
    add_picker_back.set_has_frame(false);
    add_picker_back.add_css_class("app-flat-button");
    add_picker_back.add_css_class("composer-icon-button");
    add_picker_back.add_css_class("composer-attach-picker-back");
    let add_picker_path = gtk::Label::new(Some("./"));
    add_picker_path.set_xalign(0.0);
    add_picker_path.set_hexpand(true);
    add_picker_path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    add_picker_path.set_single_line_mode(true);
    add_picker_path.add_css_class("composer-attach-picker-path");
    add_picker_header.append(&add_picker_back);
    add_picker_header.append(&add_picker_path);
    add_picker_box.append(&add_picker_header);

    let add_picker_search = gtk::SearchEntry::new();
    add_picker_search.set_placeholder_text(Some("Search files"));
    add_picker_search.add_css_class("composer-attach-picker-search");
    add_picker_box.append(&add_picker_search);

    let add_picker_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    add_picker_list.add_css_class("composer-attach-picker-list");
    add_picker_list.set_hexpand(true);
    add_picker_list.set_margin_start(1);
    add_picker_list.set_margin_end(1);
    add_picker_list.set_margin_top(1);
    add_picker_list.set_margin_bottom(1);

    let add_picker_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_width(560)
        .max_content_width(560)
        .min_content_height(200)
        .max_content_height(320)
        .child(&add_picker_list)
        .build();
    add_picker_scroll.set_has_frame(false);
    add_picker_scroll.set_size_request(560, -1);
    add_picker_scroll.set_hexpand(true);
    add_picker_scroll.add_css_class("composer-attach-picker-scroll");
    add_picker_scroll.set_overflow(gtk::Overflow::Hidden);
    add_picker_box.append(&add_picker_scroll);

    let add_current_folder_button = gtk::Button::with_label("Add Current Folder");
    add_current_folder_button.set_has_frame(false);
    add_current_folder_button.set_halign(gtk::Align::End);
    add_current_folder_button.add_css_class("composer-attach-picker-action");
    add_picker_box.append(&add_current_folder_button);

    add_picker_popover.set_child(Some(&add_picker_box));

    let add_picker_entries: Rc<RefCell<Vec<BrowserEntry>>> = Rc::new(RefCell::new(Vec::new()));
    let add_picker_root: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let add_picker_current: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let add_picker_query: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let add_picker_cached_dir: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let add_picker_cached_entries: Rc<RefCell<Vec<BrowserEntry>>> =
        Rc::new(RefCell::new(Vec::new()));

    let model_setting_changed: Rc<dyn Fn(String)> = {
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        Rc::new(move |value: String| {
            save_default_composer_setting(&db, "model", &value);
            if let Some(thread_id) = active_thread_id.borrow().clone() {
                save_thread_setting(&db, &thread_id, "model", &value);
            }
        })
    };
    let mode_setting_changed: Rc<dyn Fn(String)> = {
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        Rc::new(move |value: String| {
            save_default_composer_setting(&db, "collaboration_mode", &value);
            if let Some(thread_id) = active_thread_id.borrow().clone() {
                save_thread_setting(&db, &thread_id, "collaboration_mode", &value);
            }
        })
    };
    let (mode_selector, selected_mode_id, set_mode_id) =
        super::runtime_controls::build_mode_selector(
            active_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| thread_setting_value(&db, thread_id, "collaboration_mode"))
                .or_else(|| default_composer_setting_value(&db, "collaboration_mode")),
            Some(mode_setting_changed),
        );
    controls.append(&mode_selector);

    let mode_model_separator = create_compact_separator();
    controls.append(&mode_model_separator);

    let active_model_client = active_thread_id
        .borrow()
        .as_deref()
        .and_then(|thread_id| manager.resolve_running_client_for_thread_id(thread_id))
        .or_else(|| {
            db.runtime_profile_id()
                .ok()
                .flatten()
                .and_then(|profile_id| manager.running_client_for_profile(profile_id))
        })
        .or(codex.clone());
    let cached_model_options: Rc<RefCell<Vec<crate::services::app::runtime::ModelInfo>>> =
        Rc::new(RefCell::new(super::runtime_controls::model_options(
            active_model_client.as_ref(),
        )));
    let cached_model_options_key: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let (model_selector, selected_model_id, initial_set_model_id) =
        super::runtime_controls::build_model_selector(
            active_model_client.as_ref(),
            active_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| thread_setting_value(&db, thread_id, "model"))
                .or_else(|| default_composer_setting_value(&db, "model")),
            Some(model_setting_changed.clone()),
        );
    let model_selector_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    model_selector_slot.append(&model_selector);
    controls.append(&model_selector_slot);
    let model_selector_signature: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let model_selector_setter: Rc<RefCell<Option<Rc<dyn Fn(&str)>>>> =
        Rc::new(RefCell::new(Some(initial_set_model_id.clone())));
    let set_model_id: Rc<dyn Fn(&str)> = {
        let selected_model_id = selected_model_id.clone();
        let model_selector_setter = model_selector_setter.clone();
        Rc::new(move |next_value: &str| {
            selected_model_id.replace(next_value.to_string());
            if let Some(setter) = model_selector_setter.borrow().as_ref() {
                setter(next_value);
            }
        })
    };

    let model_effort_separator = create_compact_separator();
    controls.append(&model_effort_separator);

    let effort_setting_changed: Rc<dyn Fn(String)> = {
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        Rc::new(move |value: String| {
            save_default_composer_setting(&db, "effort", &value);
            if let Some(thread_id) = active_thread_id.borrow().clone() {
                save_thread_setting(&db, &thread_id, "effort", &value);
            }
        })
    };
    let selected_effort: Rc<RefCell<String>> = Rc::new(RefCell::new(
        active_thread_id
            .borrow()
            .as_deref()
            .and_then(|thread_id| thread_setting_value(&db, thread_id, "effort"))
            .or_else(|| default_composer_setting_value(&db, "effort"))
            .unwrap_or_else(|| "medium".to_string()),
    ));
    let effort_selector = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    effort_selector.set_visible(false);
    controls.append(&effort_selector);
    let effort_selector_signature: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let effort_selector_setter: Rc<RefCell<Option<Rc<dyn Fn(&str)>>>> = Rc::new(RefCell::new(None));
    let set_effort: Rc<dyn Fn(&str)> = {
        let selected_effort = selected_effort.clone();
        let effort_selector_setter = effort_selector_setter.clone();
        Rc::new(move |next_value: &str| {
            selected_effort.replace(next_value.to_string());
            if let Some(setter) = effort_selector_setter.borrow().as_ref() {
                setter(next_value);
            }
        })
    };

    let variant_setting_changed: Rc<dyn Fn(String)> = {
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        Rc::new(move |value: String| {
            save_default_composer_setting(&db, "variant", &value);
            if let Some(thread_id) = active_thread_id.borrow().clone() {
                save_thread_setting(&db, &thread_id, "variant", &value);
            }
        })
    };
    let selected_variant: Rc<RefCell<String>> = Rc::new(RefCell::new(
        active_thread_id
            .borrow()
            .as_deref()
            .and_then(|thread_id| thread_setting_value(&db, thread_id, "variant"))
            .or_else(|| default_composer_setting_value(&db, "variant"))
            .unwrap_or_default(),
    ));
    let variant_selector_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    variant_selector_slot.set_visible(false);
    controls.append(&variant_selector_slot);
    let variant_selector_signature: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let variant_selector_setter: Rc<RefCell<Option<Rc<dyn Fn(&str)>>>> =
        Rc::new(RefCell::new(None));

    let opencode_command_setting_changed: Rc<dyn Fn(String)> = {
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        let resolve_client_for_thread = resolve_client_for_thread.clone();
        Rc::new(move |value: String| {
            save_default_composer_setting(&db, "opencode_command_mode", &value);
            if let Some(thread_id) = active_thread_id.borrow().clone() {
                save_thread_setting(&db, &thread_id, "opencode_command_mode", &value);
                if let Some(client) = resolve_client_for_thread(&thread_id) {
                    let thread_id = thread_id.clone();
                    let value = value.clone();
                    std::thread::spawn(move || {
                        let _ = client.thread_set_command_mode(&thread_id, &value);
                    });
                }
            }
        })
    };
    let (opencode_command_selector, selected_opencode_command_mode, set_opencode_command_mode) =
        super::runtime_controls::build_opencode_command_selector(
            active_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| thread_setting_value(&db, thread_id, "opencode_command_mode"))
                .or_else(|| default_composer_setting_value(&db, "opencode_command_mode")),
            Some(opencode_command_setting_changed),
        );
    opencode_command_selector.set_visible(false);
    controls.append(&opencode_command_selector);

    let effort_access_separator = create_compact_separator();
    controls.append(&effort_access_separator);

    let access_setting_changed: Rc<dyn Fn(String)> = {
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        Rc::new(move |value: String| {
            save_default_composer_setting(&db, "access_mode", &value);
            if let Some(thread_id) = active_thread_id.borrow().clone() {
                save_thread_setting(&db, &thread_id, "access_mode", &value);
            }
        })
    };
    let (access_selector, selected_access_mode, set_access_mode) =
        super::runtime_controls::build_access_selector(
            active_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| thread_setting_value(&db, thread_id, "access_mode"))
                .or_else(|| default_composer_setting_value(&db, "access_mode")),
            Some(access_setting_changed),
        );
    controls.append(&access_selector);
    let backend_controls_note = gtk::Label::new(None);
    backend_controls_note.add_css_class("composer-lock-note");
    backend_controls_note.set_xalign(0.0);
    backend_controls_note.set_wrap(true);
    backend_controls_note.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    backend_controls_note.set_visible(false);

    let draft_text_by_thread: Rc<RefCell<HashMap<String, String>>> =
        Rc::new(RefCell::new(HashMap::new()));

    {
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        let selected_mode_id = selected_mode_id.clone();
        let selected_model_id = selected_model_id.clone();
        let selected_effort = selected_effort.clone();
        let selected_variant = selected_variant.clone();
        let selected_opencode_command_mode = selected_opencode_command_mode.clone();
        let selected_access_mode = selected_access_mode.clone();
        let set_mode_id = set_mode_id.clone();
        let set_model_id = set_model_id.clone();
        let set_effort = set_effort.clone();
        let set_opencode_command_mode = set_opencode_command_mode.clone();
        let set_access_mode = set_access_mode.clone();
        let resolve_client_for_thread = resolve_client_for_thread.clone();
        let suggestion_row = suggestion_row.clone();
        let input_view = input_view.clone();
        let draft_text_by_thread_for_timer = draft_text_by_thread.clone();
        let last_seen_thread_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let last_seen_thread_id_for_timer = last_seen_thread_id.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(120), move || {
            if input_view.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let thread_id = active_thread_id.borrow().clone();
            let previous_thread_id = last_seen_thread_id_for_timer.borrow().clone();
            if thread_id == previous_thread_id {
                return gtk::glib::ControlFlow::Continue;
            }
            let per_thread_drafts_enabled = suggestion_row.parent().is_some();
            if per_thread_drafts_enabled {
                if let Some(previous_thread_id) = previous_thread_id {
                    let buf = input_view.buffer();
                    let start = buf.start_iter();
                    let end = buf.end_iter();
                    let text = buf.text(&start, &end, true).to_string();
                    draft_text_by_thread_for_timer
                        .borrow_mut()
                        .insert(previous_thread_id, text);
                }
            }

            last_seen_thread_id_for_timer.replace(thread_id.clone());
            if let Some(thread_id) = thread_id {
                let backend_kind = resolve_client_for_thread(&thread_id)
                    .map(|client| client.backend_kind().to_string())
                    .or_else(|| {
                        db.runtime_profile_id()
                            .ok()
                            .flatten()
                            .and_then(|profile_id| db.get_codex_profile(profile_id).ok().flatten())
                            .map(|profile| profile.backend_kind)
                    })
                    .unwrap_or_else(|| "codex".to_string());
                let is_opencode = backend_kind.eq_ignore_ascii_case("opencode");
                let saved_mode_id = thread_setting_value(&db, &thread_id, "collaboration_mode")
                    .or_else(|| default_composer_setting_value(&db, "collaboration_mode"));
                if let Some(saved_mode_id) = saved_mode_id {
                    set_mode_id(&saved_mode_id);
                }
                let saved_model_id = thread_setting_value(&db, &thread_id, "model")
                    .or_else(|| default_composer_setting_value(&db, "model"));
                if let Some(saved_model_id) = saved_model_id {
                    set_model_id(&saved_model_id);
                }
                if is_opencode {
                    selected_variant.replace(
                        thread_setting_value(&db, &thread_id, "variant")
                            .or_else(|| default_composer_setting_value(&db, "variant"))
                            .unwrap_or_default(),
                    );
                    let saved_command_mode =
                        thread_setting_value(&db, &thread_id, "opencode_command_mode")
                            .or_else(|| {
                                default_composer_setting_value(&db, "opencode_command_mode")
                            })
                            .unwrap_or_else(|| "allowAll".to_string());
                    set_opencode_command_mode(&saved_command_mode);
                    if let Some(client) = resolve_client_for_thread(&thread_id) {
                        let thread_id = thread_id.clone();
                        std::thread::spawn(move || {
                            let _ = client.thread_set_command_mode(&thread_id, &saved_command_mode);
                        });
                    }
                } else {
                    let saved_effort = thread_setting_value(&db, &thread_id, "effort")
                        .or_else(|| default_composer_setting_value(&db, "effort"))
                        .unwrap_or_else(|| "medium".to_string());
                    set_effort(&saved_effort);
                }
                let saved_access_mode = thread_setting_value(&db, &thread_id, "access_mode")
                    .or_else(|| default_composer_setting_value(&db, "access_mode"));
                if let Some(saved_access_mode) = saved_access_mode {
                    set_access_mode(&saved_access_mode);
                }

                save_thread_setting(
                    &db,
                    &thread_id,
                    "collaboration_mode",
                    &selected_mode_id.borrow(),
                );
                save_thread_setting(&db, &thread_id, "model", &selected_model_id.borrow());
                if is_opencode {
                    save_thread_setting(&db, &thread_id, "variant", &selected_variant.borrow());
                    save_thread_setting(
                        &db,
                        &thread_id,
                        "opencode_command_mode",
                        &selected_opencode_command_mode.borrow(),
                    );
                } else {
                    save_thread_setting(&db, &thread_id, "effort", &selected_effort.borrow());
                }
                save_thread_setting(
                    &db,
                    &thread_id,
                    "access_mode",
                    &selected_access_mode.borrow(),
                );

                if per_thread_drafts_enabled {
                    let next_draft = draft_text_by_thread_for_timer
                        .borrow()
                        .get(&thread_id)
                        .cloned()
                        .unwrap_or_default();
                    let buf = input_view.buffer();
                    let start = buf.start_iter();
                    let end = buf.end_iter();
                    let current_text = buf.text(&start, &end, true).to_string();
                    if current_text != next_draft {
                        buf.set_text(&next_draft);
                    }
                }
            } else if per_thread_drafts_enabled {
                let buf = input_view.buffer();
                let start = buf.start_iter();
                let end = buf.end_iter();
                if !buf.text(&start, &end, true).is_empty() {
                    buf.set_text("");
                }
            }

            gtk::glib::ControlFlow::Continue
        });
    }

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    controls.append(&spacer);

    let enzim_agent_button = gtk::Button::builder().icon_name("brain-symbolic").build();
    enzim_agent_button.set_has_frame(false);
    enzim_agent_button.add_css_class("app-flat-button");
    enzim_agent_button.add_css_class("composer-icon-button");
    enzim_agent_button.add_css_class("composer-enzim-agent-button");
    enzim_agent_button.set_tooltip_text(Some("Enzim Agent"));
    controls.append(&enzim_agent_button);

    let enzim_agent_popover = gtk::Popover::new();
    enzim_agent_popover.set_has_arrow(true);
    enzim_agent_popover.set_autohide(true);
    enzim_agent_popover.set_position(gtk::PositionType::Top);
    enzim_agent_popover.set_parent(&enzim_agent_button);
    enzim_agent_popover.add_css_class("composer-enzim-agent-popover");

    let enzim_agent_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    enzim_agent_box.add_css_class("composer-enzim-agent-box");
    enzim_agent_box.set_margin_start(10);
    enzim_agent_box.set_margin_end(10);
    enzim_agent_box.set_margin_top(10);
    enzim_agent_box.set_margin_bottom(10);
    enzim_agent_box.set_size_request(340, -1);

    let enzim_agent_title = gtk::Label::new(Some("Enzim Agent"));
    enzim_agent_title.set_xalign(0.0);
    enzim_agent_title.add_css_class("composer-enzim-agent-title");
    enzim_agent_box.append(&enzim_agent_title);

    let enzim_agent_loop_prompt_title = gtk::Label::new(Some("Prompt"));
    enzim_agent_loop_prompt_title.set_xalign(0.0);
    enzim_agent_loop_prompt_title.add_css_class("composer-enzim-agent-question-title");
    enzim_agent_box.append(&enzim_agent_loop_prompt_title);

    let enzim_agent_prompt_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(90)
        .max_content_height(140)
        .build();
    enzim_agent_prompt_scroll.set_has_frame(false);
    enzim_agent_prompt_scroll.add_css_class("composer-enzim-agent-answer-scroll");
    let enzim_agent_prompt_view = gtk::TextView::new();
    enzim_agent_prompt_view.set_wrap_mode(gtk::WrapMode::WordChar);
    enzim_agent_prompt_view.set_top_margin(8);
    enzim_agent_prompt_view.set_bottom_margin(8);
    enzim_agent_prompt_view.set_left_margin(10);
    enzim_agent_prompt_view.set_right_margin(10);
    enzim_agent_prompt_view.add_css_class("composer-enzim-agent-answer-view");
    enzim_agent_prompt_scroll.set_child(Some(&enzim_agent_prompt_view));
    enzim_agent_box.append(&enzim_agent_prompt_scroll);

    let enzim_agent_loop_instructions_title = gtk::Label::new(Some("Looping instructions"));
    enzim_agent_loop_instructions_title.set_xalign(0.0);
    enzim_agent_loop_instructions_title.add_css_class("composer-enzim-agent-question-title");
    enzim_agent_box.append(&enzim_agent_loop_instructions_title);

    let enzim_agent_instructions_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(90)
        .max_content_height(140)
        .build();
    enzim_agent_instructions_scroll.set_has_frame(false);
    enzim_agent_instructions_scroll.add_css_class("composer-enzim-agent-answer-scroll");
    let enzim_agent_instructions_view = gtk::TextView::new();
    enzim_agent_instructions_view.set_wrap_mode(gtk::WrapMode::WordChar);
    enzim_agent_instructions_view.set_top_margin(8);
    enzim_agent_instructions_view.set_bottom_margin(8);
    enzim_agent_instructions_view.set_left_margin(10);
    enzim_agent_instructions_view.set_right_margin(10);
    enzim_agent_instructions_view.add_css_class("composer-enzim-agent-answer-view");
    enzim_agent_instructions_scroll.set_child(Some(&enzim_agent_instructions_view));
    enzim_agent_box.append(&enzim_agent_instructions_scroll);

    let enzim_agent_loop_status = gtk::Label::new(Some("No active loop on this thread."));
    enzim_agent_loop_status.set_xalign(0.0);
    enzim_agent_loop_status.set_wrap(true);
    enzim_agent_loop_status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    enzim_agent_loop_status.add_css_class("composer-enzim-agent-subtitle");
    enzim_agent_box.append(&enzim_agent_loop_status);

    let enzim_agent_summary_label = gtk::Label::new(None);
    enzim_agent_summary_label.set_xalign(0.0);
    enzim_agent_summary_label.set_wrap(true);
    enzim_agent_summary_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    enzim_agent_summary_label.add_css_class("composer-enzim-agent-question");
    enzim_agent_summary_label.set_visible(false);
    enzim_agent_box.append(&enzim_agent_summary_label);

    let enzim_agent_idle_label =
        gtk::Label::new(Some("No Enzim Agent question is waiting on this thread."));
    enzim_agent_idle_label.set_xalign(0.0);
    enzim_agent_idle_label.set_wrap(true);
    enzim_agent_idle_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    enzim_agent_idle_label.add_css_class("composer-enzim-agent-subtitle");
    enzim_agent_box.append(&enzim_agent_idle_label);

    let enzim_agent_question_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    enzim_agent_question_box.set_visible(false);

    let enzim_agent_question_title = gtk::Label::new(Some("Question waiting for your answer"));
    enzim_agent_question_title.set_xalign(0.0);
    enzim_agent_question_title.add_css_class("composer-enzim-agent-question-title");
    enzim_agent_question_box.append(&enzim_agent_question_title);

    let enzim_agent_question_label = gtk::Label::new(None);
    enzim_agent_question_label.set_xalign(0.0);
    enzim_agent_question_label.set_wrap(true);
    enzim_agent_question_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    enzim_agent_question_label.add_css_class("composer-enzim-agent-question");
    enzim_agent_question_box.append(&enzim_agent_question_label);

    let enzim_agent_question_hint = gtk::Label::new(Some(
        "Answer here, or type a normal message in the composer. Your answer will be saved to the loop history and Enzim Agent will continue the loop.",
    ));
    enzim_agent_question_hint.set_xalign(0.0);
    enzim_agent_question_hint.set_wrap(true);
    enzim_agent_question_hint.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    enzim_agent_question_hint.add_css_class("composer-enzim-agent-subtitle");
    enzim_agent_question_box.append(&enzim_agent_question_hint);

    let enzim_agent_answer_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(90)
        .max_content_height(140)
        .build();
    enzim_agent_answer_scroll.set_has_frame(false);
    enzim_agent_answer_scroll.add_css_class("composer-enzim-agent-answer-scroll");
    let enzim_agent_answer_view = gtk::TextView::new();
    enzim_agent_answer_view.set_wrap_mode(gtk::WrapMode::WordChar);
    enzim_agent_answer_view.set_top_margin(8);
    enzim_agent_answer_view.set_bottom_margin(8);
    enzim_agent_answer_view.set_left_margin(10);
    enzim_agent_answer_view.set_right_margin(10);
    enzim_agent_answer_view.add_css_class("composer-enzim-agent-answer-view");
    enzim_agent_answer_scroll.set_child(Some(&enzim_agent_answer_view));
    enzim_agent_question_box.append(&enzim_agent_answer_scroll);

    let enzim_agent_status = gtk::Label::new(None);
    enzim_agent_status.set_xalign(0.0);
    enzim_agent_status.set_wrap(true);
    enzim_agent_status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    enzim_agent_status.add_css_class("composer-enzim-agent-status");
    enzim_agent_question_box.append(&enzim_agent_status);

    let enzim_agent_settings = gtk::Button::with_label("Settings");
    let enzim_agent_start = gtk::Button::with_label("Start Loop");
    enzim_agent_start.add_css_class("suggested-action");
    let enzim_agent_stop = gtk::Button::with_label("Stop Loop");
    let enzim_agent_cancel = gtk::Button::with_label("Close");
    enzim_agent_cancel.add_css_class("composer-enzim-agent-close");
    let enzim_agent_answer_submit = gtk::Button::with_label("Send Answer");
    enzim_agent_answer_submit.add_css_class("suggested-action");
    enzim_agent_answer_submit.add_css_class("composer-enzim-agent-submit");
    enzim_agent_question_box.append(&enzim_agent_answer_submit);

    let enzim_agent_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    enzim_agent_actions.set_halign(gtk::Align::End);
    enzim_agent_actions.append(&enzim_agent_settings);
    enzim_agent_actions.append(&enzim_agent_start);
    enzim_agent_actions.append(&enzim_agent_stop);
    enzim_agent_actions.append(&enzim_agent_cancel);

    enzim_agent_box.append(&enzim_agent_question_box);
    enzim_agent_box.append(&enzim_agent_actions);
    enzim_agent_popover.set_child(Some(&enzim_agent_box));

    let worktree_button = gtk::Button::builder().icon_name("git-symbolic").build();
    worktree_button.set_has_frame(false);
    worktree_button.add_css_class("app-flat-button");
    worktree_button.add_css_class("composer-icon-button");
    worktree_button.add_css_class("composer-worktree-button");
    worktree_button.set_tooltip_text(Some("Create worktree variants"));
    controls.append(&worktree_button);

    let send = create_send_button();
    controls.append(&send);
    let thread_locked = Rc::new(RefCell::new(false));
    let submit_loop_answer: Rc<
        RefCell<
            Option<Rc<dyn Fn(String, String, String) -> Result<(), String>>>,
        >,
    > = Rc::new(RefCell::new(None));

    include!("build_body_enzim_agent_section.rs");

    {
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        let manager = manager.clone();
        let resolve_client_for_thread = resolve_client_for_thread.clone();
        let mode_selector = mode_selector.clone();
        let mode_model_separator = mode_model_separator.clone();
        let model_selector_slot = model_selector_slot.clone();
        let model_selector_signature = model_selector_signature.clone();
        let model_selector_setter = model_selector_setter.clone();
        let effort_selector = effort_selector.clone();
        let effort_selector_signature = effort_selector_signature.clone();
        let effort_selector_setter = effort_selector_setter.clone();
        let selected_effort = selected_effort.clone();
        let selected_model_id = selected_model_id.clone();
        let set_model_id = set_model_id.clone();
        let model_setting_changed = model_setting_changed.clone();
        let selected_variant = selected_variant.clone();
        let opencode_command_selector = opencode_command_selector.clone();
        let model_effort_separator = model_effort_separator.clone();
        let variant_selector_slot = variant_selector_slot.clone();
        let variant_selector_signature = variant_selector_signature.clone();
        let variant_selector_setter = variant_selector_setter.clone();
        let variant_setting_changed = variant_setting_changed.clone();
        let cached_model_options = cached_model_options.clone();
        let cached_model_options_key = cached_model_options_key.clone();
        let access_selector = access_selector.clone();
        let effort_access_separator = effort_access_separator.clone();
        let backend_controls_note = backend_controls_note.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(180), move || {
            if mode_selector.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let backend_kind = active_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| resolve_client_for_thread(thread_id))
                .map(|client| client.backend_kind().to_string())
                .or_else(|| {
                    db.runtime_profile_id()
                        .ok()
                        .flatten()
                        .and_then(|profile_id| db.get_codex_profile(profile_id).ok().flatten())
                        .map(|profile| profile.backend_kind)
                })
                .unwrap_or_else(|| "codex".to_string());
            let is_opencode = backend_kind.eq_ignore_ascii_case("opencode");
            let model_cache_version = super::runtime_controls::model_options_cache_version();
            let active_client = active_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| resolve_client_for_thread(thread_id))
                .or_else(|| {
                    db.runtime_profile_id()
                        .ok()
                        .flatten()
                        .and_then(|profile_id| manager.running_client_for_profile(profile_id))
                });
            let model_options_key = active_thread_id
                .borrow()
                .clone()
                .map(|thread_id| format!("thread:{thread_id}:v:{model_cache_version}"))
                .or_else(|| {
                    db.runtime_profile_id()
                        .ok()
                        .flatten()
                        .map(|profile_id| format!("profile:{profile_id}:v:{model_cache_version}"))
                });
            if cached_model_options_key.borrow().as_ref() != model_options_key.as_ref() {
                cached_model_options.replace(super::runtime_controls::model_options(
                    active_client.as_ref(),
                ));
                cached_model_options_key.replace(model_options_key);
            }
            let model_signature = {
                let models = cached_model_options.borrow();
                format!(
                    "{}\u{1f}{}",
                    backend_kind,
                    models
                        .iter()
                        .map(|model| model.id.as_str())
                        .collect::<Vec<_>>()
                        .join("\u{1f}")
                )
            };
            if model_selector_signature.borrow().as_str() != model_signature {
                crate::ui::widget_tree::clear_box_children(&model_selector_slot);
                let current_selected_model_id = selected_model_id.borrow().clone();
                let (selector, set_model) =
                    super::runtime_controls::build_model_selector_with_state(
                        active_client.as_ref(),
                        selected_model_id.clone(),
                        Some(current_selected_model_id),
                        Some(model_setting_changed.clone()),
                    );
                model_selector_slot.append(&selector);
                model_selector_setter.replace(Some(set_model));
                model_selector_signature.replace(model_signature);
            }
            let current_model_id = selected_model_id.borrow().clone();
            let replacement_model_id = {
                let models = cached_model_options.borrow();
                if models.iter().any(|model| model.id == current_model_id) {
                    None
                } else if let Some(model) = models.first() {
                    Some(model.id.clone())
                } else if is_opencode {
                    Some(String::new())
                } else {
                    None
                }
            };
            if let Some(next_model_id) = replacement_model_id {
                if next_model_id != current_model_id {
                    selected_model_id.replace(next_model_id.clone());
                    set_model_id(&next_model_id);
                    model_setting_changed(next_model_id);
                }
            }
            let model_id = selected_model_id.borrow().clone();
            let (effort_options, default_effort) =
                super::runtime_controls::reasoning_effort_options_from_models(
                    &cached_model_options.borrow(),
                    &model_id,
                );
            let variant_options = if is_opencode {
                super::runtime_controls::opencode_variant_options_from_models(
                    &cached_model_options.borrow(),
                    &model_id,
                )
            } else {
                Vec::new()
            };
            mode_selector.set_visible(true);
            mode_model_separator.set_visible(true);
            if is_opencode {
                let has_variants = !variant_options.is_empty();
                effort_selector.set_visible(false);
                variant_selector_slot.set_visible(has_variants);
                model_effort_separator.set_visible(has_variants);
                if has_variants {
                    let signature = variant_options
                        .iter()
                        .map(|(_, value)| value.as_str())
                        .collect::<Vec<_>>()
                        .join("\u{1f}");
                    let selected_variant_value = selected_variant.borrow().clone();
                    let selected_is_valid = variant_options
                        .iter()
                        .any(|(_, value)| value == &selected_variant_value);
                    if !selected_is_valid {
                        selected_variant.replace(String::new());
                        variant_setting_changed(String::new());
                    }
                    if variant_selector_signature.borrow().as_str() != signature {
                        crate::ui::widget_tree::clear_box_children(&variant_selector_slot);
                        let selected_variant_state = selected_variant.clone();
                        let current_variant = selected_variant_state.borrow().clone();
                        let selected_variant_for_change = selected_variant_state.clone();
                        let variant_setting_changed = variant_setting_changed.clone();
                        let (selector, _selected, set_variant) =
                            super::runtime_controls::build_variant_selector(
                                &variant_options,
                                Some(current_variant),
                                Some(Rc::new(move |value: String| {
                                    selected_variant_for_change.replace(value.clone());
                                    variant_setting_changed(value);
                                })),
                            );
                        variant_selector_slot.append(&selector);
                        variant_selector_setter.replace(Some(set_variant));
                        variant_selector_signature.replace(signature);
                    }
                    if let Some(set_variant) = variant_selector_setter.borrow().as_ref() {
                        let variant_value = selected_variant.borrow().clone();
                        set_variant(&variant_value);
                    }
                } else {
                    if !selected_variant.borrow().is_empty() {
                        selected_variant.replace(String::new());
                        variant_setting_changed(String::new());
                    }
                    if !variant_selector_signature.borrow().is_empty() {
                        crate::ui::widget_tree::clear_box_children(&variant_selector_slot);
                        variant_selector_signature.borrow_mut().clear();
                        variant_selector_setter.replace(None);
                    }
                }
            } else {
                let has_effort_options = effort_options.len() > 1;
                effort_selector.set_visible(has_effort_options);
                variant_selector_slot.set_visible(false);
                model_effort_separator.set_visible(has_effort_options);
                if has_effort_options {
                    let signature = effort_options
                        .iter()
                        .map(|(_, value)| value.as_str())
                        .collect::<Vec<_>>()
                        .join("\u{1f}");
                    let default_effort = default_effort.unwrap_or_else(|| {
                        effort_options
                            .first()
                            .map(|(_, value)| value.clone())
                            .unwrap_or_else(|| "medium".to_string())
                    });
                    let selected_effort_value = selected_effort.borrow().clone();
                    let selected_is_valid = effort_options
                        .iter()
                        .any(|(_, value)| value == &selected_effort_value);
                    if !selected_is_valid {
                        selected_effort.replace(default_effort.clone());
                    }
                    if effort_selector_signature.borrow().as_str() != signature {
                        crate::ui::widget_tree::clear_box_children(&effort_selector);
                        let selected_effort_state = selected_effort.clone();
                        let current_effort = selected_effort_state.borrow().clone();
                        let selected_effort_for_change = selected_effort_state.clone();
                        let effort_setting_changed = effort_setting_changed.clone();
                        let (selector, _selected, set_effort_value) =
                            super::runtime_controls::build_effort_selector(
                                &effort_options,
                                Some(current_effort),
                                Some(Rc::new(move |value: String| {
                                    selected_effort_for_change.replace(value.clone());
                                    effort_setting_changed(value);
                                })),
                            );
                        effort_selector.append(&selector);
                        effort_selector_setter.replace(Some(set_effort_value));
                        effort_selector_signature.replace(signature);
                    }
                    if let Some(setter) = effort_selector_setter.borrow().as_ref() {
                        let effort_value = selected_effort.borrow().clone();
                        setter(&effort_value);
                    }
                } else if !effort_selector_signature.borrow().is_empty() {
                    crate::ui::widget_tree::clear_box_children(&effort_selector);
                    effort_selector_signature.borrow_mut().clear();
                    effort_selector_setter.replace(None);
                }
            }
            opencode_command_selector.set_visible(is_opencode);
            access_selector.set_visible(!is_opencode);
            effort_access_separator.set_visible(true);
            backend_controls_note.set_visible(false);
            gtk::glib::ControlFlow::Continue
        });
    }

    include!("build_body_worktree_section.rs");

    let buffer = input_view.buffer();

    include!("build_body_attachment_section.rs");

    include!("build_body_send_section.rs");

    composer.set_size_request(suggestion_row_natural_width, -1);
    live_turn_status_overlay.set_size_request(-1, -1);
    live_turn_status_overlay.set_hexpand(true);
    composer.append(&controls);
    composer.append(&backend_controls_note);
    composer_cluster.append(&composer);
    lower_content.append(&composer_cluster);
    if messages_box.first_child().is_some() {
        super::message_render::scroll_to_bottom(&messages_scroll);
    }

    ComposerSection {
        lower_content,
        suggestion_row,
        live_turn_status_revealer,
        live_turn_status_label,
        live_turn_timer_label,
    }
}
