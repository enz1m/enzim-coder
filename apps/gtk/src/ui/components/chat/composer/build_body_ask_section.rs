use crate::services::app::chat::AppDb;
use crate::services::enzim_agent::{EnzimAskChat, EnzimAskMessage, EnzimAskState};
use enzimcoder::data;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn to_system_time(ts: i64) -> SystemTime {
    if ts > 1_000_000_000_000 {
        UNIX_EPOCH + Duration::from_millis(ts as u64)
    } else {
        UNIX_EPOCH + Duration::from_secs(ts.max(0) as u64)
    }
}

fn next_chat_id() -> String {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    format!("ask-{micros}")
}

fn selected_chat(state: &EnzimAskState) -> Option<EnzimAskChat> {
    let current_chat_id = state.current_chat_id.as_deref()?;
    state
        .chats
        .iter()
        .find(|chat| chat.id == current_chat_id)
        .cloned()
}

fn normalize_current_chat_id(state: &mut EnzimAskState) {
    if state
        .current_chat_id
        .as_deref()
        .is_some_and(|chat_id| state.chats.iter().all(|chat| chat.id != chat_id))
    {
        state.current_chat_id = None;
    }
}

pub(super) fn build_ask_button(db: Rc<AppDb>) -> gtk::Box {
    let ask_button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    ask_button.set_width_request(18);
    ask_button.set_height_request(18);
    ask_button.set_hexpand(false);
    ask_button.set_vexpand(false);
    ask_button.set_halign(gtk::Align::Center);
    ask_button.set_valign(gtk::Align::Center);
    ask_button.set_can_focus(false);
    let ask_icon = gtk::Image::from_icon_name("chat-bubble-symbolic");
    ask_icon.set_pixel_size(15);
    ask_icon.add_css_class("bottom-icon-image");
    ask_button.append(&ask_icon);
    ask_button.set_tooltip_text(Some("Ask Enzim Agent"));

    let ask_popover = gtk::Popover::new();
    ask_popover.set_has_arrow(true);
    ask_popover.set_autohide(true);
    ask_popover.set_position(gtk::PositionType::Top);
    ask_popover.set_parent(&ask_button);
    ask_popover.add_css_class("composer-worktree-popover");
    ask_popover.add_css_class("composer-ask-popover");

    let ask_state = Rc::new(RefCell::new(crate::services::enzim_agent::load_ask_state(
        db.as_ref(),
    )));
    {
        let mut state = ask_state.borrow_mut();
        normalize_current_chat_id(&mut state);
    }
    let selected_model_id = Rc::new(RefCell::new(
        crate::services::enzim_agent::effective_ask_model_id(db.as_ref()).unwrap_or_default(),
    ));
    let is_request_running = Rc::new(RefCell::new(false));

    let ask_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    ask_box.add_css_class("composer-worktree-popover-box");
    ask_box.add_css_class("composer-ask-box");
    ask_box.set_margin_start(10);
    ask_box.set_margin_end(10);
    ask_box.set_margin_top(10);
    ask_box.set_margin_bottom(10);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let history_button = gtk::Label::new(Some("History"));
    history_button.set_xalign(0.0);
    history_button.add_css_class("composer-ask-header-action");
    let header_title = gtk::Label::new(Some("Enzim Agent"));
    header_title.set_xalign(0.5);
    header_title.set_hexpand(true);
    header_title.add_css_class("composer-enzim-agent-title");
    let new_chat_button = gtk::Label::new(Some("New chat"));
    new_chat_button.set_xalign(1.0);
    new_chat_button.add_css_class("composer-ask-header-action");
    header.append(&history_button);
    header.append(&header_title);
    header.append(&new_chat_button);
    ask_box.append(&header);

    let history_popover = gtk::Popover::new();
    history_popover.set_has_arrow(true);
    history_popover.set_autohide(true);
    history_popover.set_position(gtk::PositionType::Bottom);
    history_popover.set_parent(&history_button);
    history_popover.add_css_class("composer-worktree-popover");
    history_popover.add_css_class("composer-ask-history-popover");

    let history_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_width(300)
        .max_content_height(280)
        .propagate_natural_height(true)
        .build();
    history_scroll.set_has_frame(false);
    let history_list = gtk::Box::new(gtk::Orientation::Vertical, 4);
    history_list.add_css_class("composer-ask-history-list");
    history_scroll.set_child(Some(&history_list));
    history_popover.set_child(Some(&history_scroll));

    let conversation_stack = gtk::Stack::new();
    conversation_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    conversation_stack.set_transition_duration(120);

    let empty_state = gtk::Label::new(Some(
        "Ask Enzim Agent about anything outside the current project.",
    ));
    empty_state.set_xalign(0.0);
    empty_state.set_wrap(true);
    empty_state.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    empty_state.add_css_class("composer-ask-empty");
    empty_state.set_margin_top(10);
    empty_state.set_margin_bottom(10);
    conversation_stack.add_named(&empty_state, Some("empty"));

    let messages_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(260)
        .max_content_height(380)
        .vexpand(true)
        .build();
    messages_scroll.set_has_frame(false);
    messages_scroll.set_widget_name("chat-messages-scroll");
    messages_scroll.add_css_class("composer-ask-messages-scroll");

    let messages_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    messages_box.set_widget_name("chat-messages-box");
    messages_box.add_css_class("chat-messages-box");
    messages_box.set_margin_start(2);
    messages_box.set_margin_end(2);
    messages_box.set_margin_top(2);
    messages_box.set_margin_bottom(2);
    messages_scroll.set_child(Some(&messages_box));
    super::super::message_render::register_auto_scroll_user_tracking(&messages_scroll);
    conversation_stack.add_named(&messages_scroll, Some("messages"));
    conversation_stack.set_visible_child_name("empty");
    ask_box.append(&conversation_stack);

    let status_label = gtk::Label::new(None);
    status_label.set_xalign(0.0);
    status_label.set_wrap(true);
    status_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status_label.add_css_class("composer-ask-status");
    status_label.set_visible(false);
    ask_box.append(&status_label);

    let composer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    composer.add_css_class("composer");
    composer.add_css_class("composer-ask-composer");

    let input_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(90)
        .max_content_height(140)
        .build();
    input_scroll.set_has_frame(false);
    input_scroll.add_css_class("composer-input");
    input_scroll.add_css_class("composer-enzim-agent-input-scroll");
    input_scroll.set_widget_name("ask-popup-answer-scroll");
    input_scroll.set_focusable(false);
    let input_view = gtk::TextView::new();
    input_view.set_widget_name("ask-popup-answer-view");
    input_view.set_wrap_mode(gtk::WrapMode::WordChar);
    input_view.set_accepts_tab(false);
    input_view.set_top_margin(8);
    input_view.set_bottom_margin(8);
    input_view.set_left_margin(10);
    input_view.set_right_margin(10);
    input_view.add_css_class("composer-input-view");
    input_view.add_css_class("composer-enzim-agent-input-view");

    input_scroll.set_child(Some(&input_view));

    let input_overlay = gtk::Overlay::new();
    input_overlay.set_child(Some(&input_scroll));
    let input_placeholder = gtk::Label::new(Some("Ask anything..."));
    input_placeholder.add_css_class("composer-placeholder");
    input_placeholder.set_halign(gtk::Align::Start);
    input_placeholder.set_valign(gtk::Align::Start);
    input_placeholder.set_margin_start(10);
    input_placeholder.set_margin_top(10);
    input_placeholder.set_can_target(false);
    input_overlay.add_overlay(&input_placeholder);

    composer.append(&input_overlay);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("composer-ask-footer");
    let model_selector_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    footer.append(&model_selector_slot);
    let footer_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    footer_spacer.set_hexpand(true);
    footer.append(&footer_spacer);
    let send_button = super::create_send_button();
    footer.append(&send_button);
    composer.append(&footer);
    ask_box.append(&composer);
    ask_popover.set_child(Some(&ask_box));

    let persist_state: Rc<dyn Fn()> = {
        let db = db.clone();
        let ask_state = ask_state.clone();
        let status_label = status_label.clone();
        Rc::new(move || {
            let state = ask_state.borrow().clone();
            if let Err(err) = crate::services::enzim_agent::save_ask_state(db.as_ref(), &state) {
                status_label.set_text(&err);
                status_label.set_visible(true);
            }
        })
    };

    let update_input_ui: Rc<dyn Fn()> = {
        let input_view = input_view.clone();
        let send_button = send_button.clone();
        let is_request_running = is_request_running.clone();
        let input_placeholder = input_placeholder.clone();
        Rc::new(move || {
            let buffer = input_view.buffer();
            let start = buffer.start_iter();
            let end = buffer.end_iter();
            let text = buffer.text(&start, &end, true);
            let has_text = !text.trim().is_empty();
            input_placeholder.set_visible(!has_text);
            if has_text && !*is_request_running.borrow() {
                send_button.add_css_class("send-button-active");
            } else {
                send_button.remove_css_class("send-button-active");
            }
        })
    };

    let set_running_ui: Rc<dyn Fn(bool)> = {
        let is_request_running = is_request_running.clone();
        let input_view = input_view.clone();
        let send_button = send_button.clone();
        let history_button = history_button.clone();
        let new_chat_button = new_chat_button.clone();
        let model_selector_slot = model_selector_slot.clone();
        let update_input_ui = update_input_ui.clone();
        Rc::new(move |running: bool| {
            is_request_running.replace(running);
            input_view.set_editable(!running);
            input_view.set_cursor_visible(!running);
            send_button.set_sensitive(!running);
            history_button.set_sensitive(!running);
            new_chat_button.set_sensitive(!running);
            model_selector_slot.set_sensitive(!running);
            update_input_ui();
        })
    };

    let render_current_chat_handle: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let load_chat_handle: Rc<RefCell<Option<Rc<dyn Fn(String)>>>> = Rc::new(RefCell::new(None));
    let render_history_handle: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));

    let render_current_chat: Rc<dyn Fn()> = {
        let ask_state = ask_state.clone();
        let messages_box = messages_box.clone();
        let messages_scroll = messages_scroll.clone();
        let conversation_stack = conversation_stack.clone();
        Rc::new(move || {
            super::super::clear_messages(&messages_box);
            let chat = {
                let state = ask_state.borrow();
                selected_chat(&state)
            };
            if let Some(chat) = chat {
                for message in &chat.messages {
                    if message.role == "assistant" {
                        super::super::message_render::append_assistant_markdown_message(
                            &messages_box,
                            Some(&messages_scroll),
                            &conversation_stack,
                            &message.content,
                            to_system_time(message.created_at),
                        );
                    } else {
                        super::super::message_render::append_message(
                            &messages_box,
                            Some(&messages_scroll),
                            &conversation_stack,
                            &message.content,
                            true,
                            to_system_time(message.created_at),
                        );
                    }
                }
                if chat.messages.is_empty() {
                    conversation_stack.set_visible_child_name("empty");
                } else {
                    conversation_stack.set_visible_child_name("messages");
                    super::super::message_render::scroll_to_bottom(&messages_scroll);
                }
            } else {
                conversation_stack.set_visible_child_name("empty");
            }
        })
    };
    render_current_chat_handle.replace(Some(render_current_chat.clone()));

    let render_history: Rc<dyn Fn()> = {
        let ask_state = ask_state.clone();
        let history_list = history_list.clone();
        let history_popover = history_popover.clone();
        let load_chat_handle = load_chat_handle.clone();
        Rc::new(move || {
            while let Some(child) = history_list.first_child() {
                history_list.remove(&child);
            }

            let state = ask_state.borrow().clone();
            if state.chats.is_empty() {
                let empty = gtk::Label::new(Some("No previous chats yet."));
                empty.set_xalign(0.0);
                empty.add_css_class("dim-label");
                empty.add_css_class("composer-ask-history-empty");
                history_list.append(&empty);
                return;
            }

            let current_chat_id = state.current_chat_id.clone();
            for chat in &state.chats {
                let row = gtk::Button::new();
                row.set_has_frame(false);
                row.add_css_class("app-flat-button");
                row.add_css_class("composer-ask-history-row");
                if current_chat_id.as_deref() == Some(chat.id.as_str()) {
                    row.add_css_class("composer-ask-history-row-active");
                }

                let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
                content.set_halign(gtk::Align::Fill);
                content.set_hexpand(true);

                let title = gtk::Label::new(Some(&chat.title));
                title.set_xalign(0.0);
                title.set_hexpand(true);
                title.set_halign(gtk::Align::Fill);
                title.set_width_chars(1);
                title.set_max_width_chars(30);
                title.set_ellipsize(gtk::pango::EllipsizeMode::End);
                title.add_css_class("composer-ask-history-title");
                content.append(&title);

                let meta = gtk::Label::new(Some(&data::format_relative_age(chat.updated_at)));
                meta.set_xalign(0.0);
                meta.add_css_class("composer-ask-history-meta");
                content.append(&meta);

                row.set_child(Some(&content));

                let chat_id = chat.id.clone();
                let history_popover = history_popover.clone();
                let load_chat_handle = load_chat_handle.clone();
                row.connect_clicked(move |_| {
                    if let Some(load_chat) = load_chat_handle.borrow().as_ref() {
                        load_chat(chat_id.clone());
                    }
                    history_popover.popdown();
                });

                history_list.append(&row);
            }
        })
    };
    render_history_handle.replace(Some(render_history.clone()));

    let load_chat: Rc<dyn Fn(String)> = {
        let ask_state = ask_state.clone();
        let persist_state = persist_state.clone();
        let render_current_chat = render_current_chat.clone();
        let render_history = render_history.clone();
        let status_label = status_label.clone();
        Rc::new(move |chat_id: String| {
            {
                let mut state = ask_state.borrow_mut();
                state.current_chat_id = Some(chat_id);
                normalize_current_chat_id(&mut state);
            }
            persist_state();
            status_label.set_visible(false);
            render_current_chat();
            render_history();
        })
    };
    load_chat_handle.replace(Some(load_chat.clone()));

    let start_new_chat: Rc<dyn Fn()> = {
        let ask_state = ask_state.clone();
        let persist_state = persist_state.clone();
        let render_current_chat = render_current_chat.clone();
        let render_history = render_history.clone();
        let input_view = input_view.clone();
        let status_label = status_label.clone();
        let update_input_ui = update_input_ui.clone();
        Rc::new(move || {
            ask_state.borrow_mut().current_chat_id = None;
            persist_state();
            status_label.set_visible(false);
            input_view.buffer().set_text("");
            update_input_ui();
            render_current_chat();
            render_history();
        })
    };

    let model_selector_signature = Rc::new(RefCell::new(String::new()));
    let model_selector_setter: Rc<RefCell<Option<Rc<dyn Fn(&str)>>>> = Rc::new(RefCell::new(None));
    let refresh_model_selector: Rc<dyn Fn()> = {
        let db = db.clone();
        let selected_model_id = selected_model_id.clone();
        let model_selector_slot = model_selector_slot.clone();
        let model_selector_signature = model_selector_signature.clone();
        let model_selector_setter = model_selector_setter.clone();
        let status_label = status_label.clone();
        Rc::new(move || {
            let config = crate::services::enzim_agent::load_config(db.as_ref());
            let models = crate::services::enzim_agent::ask_model_options(&config);
            let signature = models
                .iter()
                .map(|model| format!("{}:{}", model.display_name, model.id))
                .collect::<Vec<_>>()
                .join("\u{1f}");

            let mut next_model_id = selected_model_id.borrow().clone();
            if !models.iter().any(|model| model.id == next_model_id) {
                next_model_id = crate::services::enzim_agent::effective_ask_model_id(db.as_ref())
                    .unwrap_or_else(|| {
                        models
                            .first()
                            .map(|model| model.id.clone())
                            .unwrap_or_default()
                    });
                selected_model_id.replace(next_model_id.clone());
                let _ = crate::services::enzim_agent::save_ask_model_choice(
                    db.as_ref(),
                    &next_model_id,
                );
            }

            if model_selector_signature.borrow().as_str() != signature {
                crate::ui::widget_tree::clear_box_children(&model_selector_slot);
                if models.is_empty() {
                    let empty = gtk::Label::new(Some("No model"));
                    empty.set_xalign(0.0);
                    empty.add_css_class("composer-ask-model-empty");
                    model_selector_slot.append(&empty);
                    model_selector_setter.replace(None);
                } else {
                    let options = models
                        .iter()
                        .map(|model| (model.display_name.clone(), model.id.clone()))
                        .collect::<Vec<_>>();
                    let current_label = models
                        .iter()
                        .find(|model| model.id == next_model_id)
                        .map(|model| model.display_name.clone())
                        .unwrap_or_else(|| "Select model".to_string());
                    let db = db.clone();
                    let selected_model_id_for_change = selected_model_id.clone();
                    let status_label = status_label.clone();
                    let (selector, set_model) = super::super::create_selector_menu(
                        &current_label,
                        &options,
                        selected_model_id.clone(),
                        None,
                        Some(Rc::new(move |value: String| {
                            selected_model_id_for_change.replace(value.clone());
                            if let Err(err) = crate::services::enzim_agent::save_ask_model_choice(
                                db.as_ref(),
                                &value,
                            ) {
                                status_label.set_text(&err);
                                status_label.set_visible(true);
                            }
                        })),
                        gtk::PositionType::Top,
                    );
                    model_selector_slot.append(&selector);
                    model_selector_setter.replace(Some(set_model));
                }
                model_selector_signature.replace(signature);
            }

            if let Some(setter) = model_selector_setter.borrow().as_ref() {
                setter(&next_model_id);
            }

            if models.is_empty() {
                let hint = if config.base_url.trim().is_empty() {
                    "Set an Enzim Agent base URL in Settings first."
                } else {
                    "Refresh Enzim Agent models in Settings to pick a model here."
                };
                status_label.set_text(hint);
                status_label.set_visible(true);
            }
        })
    };

    let submit_message_handle: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let submit_message: Rc<dyn Fn()> = {
        let ask_state = ask_state.clone();
        let selected_model_id = selected_model_id.clone();
        let input_view = input_view.clone();
        let status_label = status_label.clone();
        let persist_state = persist_state.clone();
        let render_current_chat = render_current_chat.clone();
        let render_history = render_history.clone();
        let set_running_ui = set_running_ui.clone();
        let update_input_ui = update_input_ui.clone();
        Rc::new(move || {
            if *is_request_running.borrow() {
                return;
            }

            let buffer = input_view.buffer();
            let start = buffer.start_iter();
            let end = buffer.end_iter();
            let text = buffer.text(&start, &end, true).trim().to_string();
            if text.is_empty() {
                return;
            }

            let model_id = selected_model_id.borrow().trim().to_string();
            if model_id.is_empty() {
                status_label.set_text("Select an Ask model in Enzim Agent settings first.");
                status_label.set_visible(true);
                return;
            }

            status_label.set_visible(false);
            let now = unix_now();
            let (chat_id, request_messages) = {
                let mut state = ask_state.borrow_mut();
                normalize_current_chat_id(&mut state);

                let chat_id = state.current_chat_id.clone().unwrap_or_else(next_chat_id);
                let chat_idx = state
                    .chats
                    .iter()
                    .position(|chat| chat.id == chat_id)
                    .unwrap_or_else(|| {
                        let title = super::title_from_first_prompt(&text)
                            .unwrap_or_else(|| "New chat".to_string());
                        state.chats.insert(
                            0,
                            EnzimAskChat {
                                id: chat_id.clone(),
                                title,
                                messages: Vec::new(),
                                created_at: now,
                                updated_at: now,
                            },
                        );
                        0
                    });

                let request_messages = {
                    let chat = &mut state.chats[chat_idx];
                    if chat.messages.is_empty() {
                        chat.title = super::title_from_first_prompt(&text)
                            .unwrap_or_else(|| "New chat".to_string());
                    }
                    chat.messages.push(EnzimAskMessage {
                        role: "user".to_string(),
                        content: text.clone(),
                        created_at: now,
                    });
                    chat.updated_at = now;
                    chat.messages.clone()
                };
                state.current_chat_id = Some(chat_id.clone());
                (chat_id, request_messages)
            };

            persist_state();
            render_current_chat();
            render_history();
            buffer.set_text("");
            update_input_ui();
            status_label.set_text("Enzim Agent is replying...");
            status_label.set_visible(true);
            set_running_ui(true);

            let (tx, rx) = mpsc::channel::<Result<String, String>>();
            let model_id_for_thread = model_id.clone();
            thread::spawn(move || {
                let result = AppDb::open_detached()
                    .map_err(|err| err.to_string())
                    .and_then(|db| {
                        let config = crate::services::enzim_agent::load_config(&db);
                        let system_prompt =
                            crate::services::enzim_agent::effective_ask_system_prompt(&db);
                        crate::services::enzim_agent::ask_chat_completion(
                            &config,
                            &system_prompt,
                            &model_id_for_thread,
                            &request_messages,
                        )
                    });
                let _ = tx.send(result);
            });

            let ask_state = ask_state.clone();
            let persist_state = persist_state.clone();
            let render_current_chat = render_current_chat.clone();
            let render_history = render_history.clone();
            let set_running_ui = set_running_ui.clone();
            let status_label = status_label.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(60), move || match rx.try_recv() {
                Ok(Ok(reply)) => {
                    let now = unix_now();
                    {
                        let mut state = ask_state.borrow_mut();
                        if let Some(chat) = state.chats.iter_mut().find(|chat| chat.id == chat_id) {
                            chat.messages.push(EnzimAskMessage {
                                role: "assistant".to_string(),
                                content: reply,
                                created_at: now,
                            });
                            chat.updated_at = now;
                        }
                    }
                    persist_state();
                    render_current_chat();
                    render_history();
                    status_label.set_visible(false);
                    set_running_ui(false);
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    status_label.set_text(&err);
                    status_label.set_visible(true);
                    set_running_ui(false);
                    gtk::glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    status_label.set_text("Ask response worker disconnected unexpectedly.");
                    status_label.set_visible(true);
                    set_running_ui(false);
                    gtk::glib::ControlFlow::Break
                }
            });
        })
    };
    submit_message_handle.replace(Some(submit_message.clone()));

    {
        let history_popover = history_popover.clone();
        let render_history = render_history.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_released(move |_, _, _, _| {
            render_history();
            if history_popover.is_visible() {
                history_popover.popdown();
            } else {
                history_popover.popup();
            }
        });
        history_button.add_controller(click);
    }

    {
        let start_new_chat = start_new_chat.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_released(move |_, _, _, _| {
            start_new_chat();
        });
        new_chat_button.add_controller(click);
    }

    for label in [&history_button, &new_chat_button] {
        let hover_target = label.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |_, _, _| {
            hover_target.add_css_class("is-hover");
        });

        let hover_target = label.clone();
        motion.connect_leave(move |_| {
            hover_target.remove_css_class("is-hover");
        });
        label.add_controller(motion);

        let active_target = label.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_pressed(move |_, _, _, _| {
            active_target.add_css_class("is-active");
        });

        let active_target = label.clone();
        click.connect_released(move |_, _, _, _| {
            active_target.remove_css_class("is-active");
        });
        label.add_controller(click);
    }

    send_button.connect_clicked({
        let submit_message = submit_message.clone();
        move |_| {
            submit_message();
        }
    });

    input_view.buffer().connect_changed({
        let update_input_ui = update_input_ui.clone();
        move |_| {
            update_input_ui();
        }
    });

    {
        let submit_message = submit_message.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _, state| {
            let is_enter = key == gtk::gdk::Key::Return || key == gtk::gdk::Key::KP_Enter;
            if is_enter && !state.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                submit_message();
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        input_view.add_controller(key_controller);
    }

    let ask_toggle = gtk::GestureClick::builder().button(1).build();
    ask_toggle.connect_pressed({
        let ask_button = ask_button.clone();
        move |_, _, _, _| {
            ask_button.add_css_class("is-active");
        }
    });
    ask_toggle.connect_released({
        let ask_popover = ask_popover.clone();
        let refresh_model_selector = refresh_model_selector.clone();
        let render_current_chat = render_current_chat.clone();
        let render_history = render_history.clone();
        let update_input_ui = update_input_ui.clone();
        let ask_button = ask_button.clone();
        move |_, _, _, _| {
            ask_button.remove_css_class("is-active");
            refresh_model_selector();
            render_current_chat();
            render_history();
            update_input_ui();
            if ask_popover.is_visible() {
                ask_popover.popdown();
            } else {
                ask_popover.popup();
            }
        }
    });
    ask_button.add_controller(ask_toggle);

    {
        let input_view = input_view.clone();
        ask_popover.connect_visible_notify(move |p| {
            if p.is_visible() {
                let focus_view = input_view.clone();
                gtk::glib::idle_add_local_once(move || {
                    focus_view.grab_focus();
                });
                let focus_view = input_view.clone();
                gtk::glib::timeout_add_local_once(
                    std::time::Duration::from_millis(50),
                    move || {
                        focus_view.grab_focus();
                    },
                );
                let focus_view = input_view.clone();
                gtk::glib::timeout_add_local_once(
                    std::time::Duration::from_millis(150),
                    move || {
                        if let Some(root) = focus_view.root() {
                            root.set_focus(Some(&focus_view));
                        }
                    },
                );
            }
        });
    }

    refresh_model_selector();
    render_current_chat();
    render_history();
    update_input_ui();

    ask_button
}
