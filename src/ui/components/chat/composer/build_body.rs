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
    codex: Option<Arc<CodexAppServer>>,
    active_codex_thread_id: Rc<RefCell<Option<String>>>,
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
    let resolve_client_for_thread = {
        let db = db.clone();
        let manager = manager.clone();
        move |thread_id: &str| {
            manager.resolve_client_for_thread_id(thread_id).or_else(|| {
                db.runtime_profile_id()
                    .ok()
                    .flatten()
                    .and_then(|profile_id| manager.client_for_profile(profile_id))
            })
        }
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
        button.composer-input-mic,
        button.composer-worktree-button {
          background-color: transparent;
          background-image: none;
          border-color: transparent;
          box-shadow: none;
        }

        button.composer-selector-button:hover,
        button.composer-input-mic:hover,
        button.composer-worktree-button:hover {
          background-color: transparent;
        }

        button.composer-selector-button:active,
        button.composer-selector-button:checked,
        button.composer-input-mic:active,
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

    let placeholder = gtk::Label::new(Some("Ask Codex anything, @ to add files, / for commands"));
    placeholder.add_css_class("composer-placeholder");
    placeholder.set_halign(gtk::Align::Start);
    placeholder.set_valign(gtk::Align::Start);
    placeholder.set_margin_start(12);
    placeholder.set_margin_top(10);
    placeholder.set_can_target(false);
    overlay.add_overlay(&placeholder);

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
    let add_picker_back = gtk::Button::builder().icon_name("pan-start-symbolic").build();
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
    let add_picker_cached_entries: Rc<RefCell<Vec<BrowserEntry>>> = Rc::new(RefCell::new(Vec::new()));

    let model_setting_changed: Rc<dyn Fn(String)> = {
        let db = db.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        Rc::new(move |value: String| {
            if let Some(thread_id) = active_codex_thread_id.borrow().clone() {
                save_thread_setting(&db, &thread_id, "model", &value);
            }
        })
    };
    let mode_setting_changed: Rc<dyn Fn(String)> = {
        let db = db.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        Rc::new(move |value: String| {
            if let Some(thread_id) = active_codex_thread_id.borrow().clone() {
                save_thread_setting(&db, &thread_id, "collaboration_mode", &value);
            }
        })
    };
    let (mode_selector, selected_mode_id, set_mode_id) = super::codex_controls::build_mode_selector(
        active_codex_thread_id
            .borrow()
            .as_deref()
            .and_then(|thread_id| thread_setting_value(&db, thread_id, "collaboration_mode")),
        Some(mode_setting_changed),
    );
    controls.append(&mode_selector);

    let separator = create_compact_separator();
    controls.append(&separator);

    let active_model_client = active_codex_thread_id
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
    let (model_selector, selected_model_id, set_model_id) =
        super::codex_controls::build_model_selector(
            active_model_client.as_ref(),
            active_codex_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| thread_setting_value(&db, thread_id, "model")),
            Some(model_setting_changed),
        );
    controls.append(&model_selector);

    let separator = create_compact_separator();
    controls.append(&separator);

    let effort_setting_changed: Rc<dyn Fn(String)> = {
        let db = db.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        Rc::new(move |value: String| {
            if let Some(thread_id) = active_codex_thread_id.borrow().clone() {
                save_thread_setting(&db, &thread_id, "effort", &value);
            }
        })
    };
    let (effort_selector, selected_effort, set_effort) =
        super::codex_controls::build_effort_selector(
            active_codex_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| thread_setting_value(&db, thread_id, "effort")),
            Some(effort_setting_changed),
        );
    controls.append(&effort_selector);

    let separator = create_compact_separator();
    controls.append(&separator);

    let access_setting_changed: Rc<dyn Fn(String)> = {
        let db = db.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        Rc::new(move |value: String| {
            if let Some(thread_id) = active_codex_thread_id.borrow().clone() {
                save_thread_setting(&db, &thread_id, "access_mode", &value);
            }
        })
    };
    let (access_selector, selected_access_mode, set_access_mode) =
        super::codex_controls::build_access_selector(
            active_codex_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| thread_setting_value(&db, thread_id, "access_mode")),
            Some(access_setting_changed),
        );
    controls.append(&access_selector);
    let draft_text_by_thread: Rc<RefCell<HashMap<String, String>>> =
        Rc::new(RefCell::new(HashMap::new()));

    {
        let db = db.clone();
        let active_codex_thread_id = active_codex_thread_id.clone();
        let selected_mode_id = selected_mode_id.clone();
        let selected_model_id = selected_model_id.clone();
        let selected_effort = selected_effort.clone();
        let selected_access_mode = selected_access_mode.clone();
        let set_mode_id = set_mode_id.clone();
        let set_model_id = set_model_id.clone();
        let set_effort = set_effort.clone();
        let set_access_mode = set_access_mode.clone();
        let suggestion_row = suggestion_row.clone();
        let input_view = input_view.clone();
        let draft_text_by_thread_for_timer = draft_text_by_thread.clone();
        let last_seen_thread_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let last_seen_thread_id_for_timer = last_seen_thread_id.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(120), move || {
            if input_view.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let thread_id = active_codex_thread_id.borrow().clone();
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
                if let Some(saved_mode_id) =
                    thread_setting_value(&db, &thread_id, "collaboration_mode")
                {
                    set_mode_id(&saved_mode_id);
                }
                if let Some(saved_model_id) = thread_setting_value(&db, &thread_id, "model") {
                    set_model_id(&saved_model_id);
                }
                if let Some(saved_effort) = thread_setting_value(&db, &thread_id, "effort") {
                    set_effort(&saved_effort);
                }
                if let Some(saved_access_mode) =
                    thread_setting_value(&db, &thread_id, "access_mode")
                {
                    set_access_mode(&saved_access_mode);
                }

                save_thread_setting(
                    &db,
                    &thread_id,
                    "collaboration_mode",
                    &selected_mode_id.borrow(),
                );
                save_thread_setting(&db, &thread_id, "model", &selected_model_id.borrow());
                save_thread_setting(&db, &thread_id, "effort", &selected_effort.borrow());
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

    include!("build_body_worktree_section.rs");

    let buffer = input_view.buffer();

    include!("build_body_attachment_section.rs");

    include!("build_body_send_section.rs");

    composer.set_size_request(suggestion_row_natural_width, -1);
    live_turn_status_overlay.set_size_request(-1, -1);
    live_turn_status_overlay.set_hexpand(true);
    composer.append(&controls);
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
