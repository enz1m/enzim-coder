use crate::services::app::chat::AppDb;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

fn apply_loop_editor_theme(
    scroll: &gtk::ScrolledWindow,
    view: &gtk::TextView,
    scroll_name: &str,
    view_name: &str,
) {
    scroll.set_widget_name(scroll_name);
    view.set_widget_name(view_name);

    let provider = gtk::CssProvider::new();
    let css = format!(
        r#"
#{scroll_name},
#{scroll_name} > viewport,
#{scroll_name} > viewport.view,
#{scroll_name} > viewport > textview,
#{scroll_name} > viewport > textview.view,
textview#{view_name},
textview#{view_name}.view,
textview#{view_name} text {{
  border-radius: 12px;
  border: 1px solid alpha(@window_fg_color, 0.09);
  background: alpha(@window_fg_color, 0.06);
  background-color: alpha(@window_fg_color, 0.06);
  background-image: none;
  color: @window_fg_color;
  box-shadow: none;
  outline: none;
}}

scrolledwindow#{scroll_name} > scrollbar.vertical,
scrolledwindow#{scroll_name} > scrollbar.vertical > range,
scrolledwindow#{scroll_name} > scrollbar.vertical > range > trough,
scrolledwindow#{scroll_name} > scrollbar.vertical > range > trough > slider {{
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
}}
"#
    );
    provider.load_from_string(&css);

    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
    }
}

fn loop_default_state_label(value: &str) -> &'static str {
    if value.trim().is_empty() {
        "Using built-in default"
    } else {
        "Custom default saved"
    }
}

fn open_system_prompt_dialog(parent: &gtk::Window, db: Rc<AppDb>, on_saved: Rc<dyn Fn()>) {
    let dialog = gtk::Window::builder()
        .title("Loop System Prompt")
        .default_width(720)
        .default_height(560)
        .modal(true)
        .transient_for(parent)
        .build();
    dialog.add_css_class("settings-window");

    let config = crate::services::enzim_agent::load_config(db.as_ref());
    let default_prompt = crate::services::enzim_agent::default_system_prompt().to_string();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(14);
    root.set_margin_end(14);
    root.set_margin_top(14);
    root.set_margin_bottom(14);

    let intro = gtk::Label::new(Some(
        "Override the default system prompt used to supervise coding-agent loops. Leave empty to use the built-in default.",
    ));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    intro.add_css_class("dim-label");
    root.append(&intro);

    let prompt_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();
    prompt_scroll.set_has_frame(false);
    prompt_scroll.add_css_class("composer-input");
    let prompt_view = gtk::TextView::new();
    prompt_view.set_wrap_mode(gtk::WrapMode::WordChar);
    prompt_view.set_top_margin(10);
    prompt_view.set_bottom_margin(10);
    prompt_view.set_left_margin(10);
    prompt_view.set_right_margin(10);
    prompt_view.add_css_class("composer-input-view");
    apply_loop_editor_theme(
        &prompt_scroll,
        &prompt_view,
        "enzim-agent-system-prompt-scroll",
        "enzim-agent-system-prompt-view",
    );
    let prompt_buf = prompt_view.buffer();
    prompt_buf.set_text(
        config
            .system_prompt_override
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(default_prompt.as_str()),
    );
    prompt_scroll.set_child(Some(&prompt_view));
    root.append(&prompt_scroll);

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.add_css_class("dim-label");
    root.append(&status);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let reset = gtk::Button::with_label("Reset");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    actions.append(&reset);
    actions.append(&save);
    root.append(&actions);

    {
        let prompt_buf = prompt_buf.clone();
        let status = status.clone();
        let default_prompt = default_prompt.clone();
        reset.connect_clicked(move |_| {
            prompt_buf.set_text(&default_prompt);
            status.set_text("Restored the built-in default prompt.");
        });
    }

    {
        let db = db.clone();
        let prompt_buf = prompt_buf.clone();
        let status = status.clone();
        let dialog = dialog.clone();
        let on_saved = on_saved.clone();
        let default_prompt = default_prompt.clone();
        save.connect_clicked(move |_| {
            let start = prompt_buf.start_iter();
            let end = prompt_buf.end_iter();
            let prompt = prompt_buf.text(&start, &end, true).to_string();
            let mut config = crate::services::enzim_agent::load_config(db.as_ref());
            config.system_prompt_override =
                if prompt.trim().is_empty() || prompt.trim() == default_prompt.trim() {
                    None
                } else {
                    Some(prompt)
                };
            match crate::services::enzim_agent::save_config(db.as_ref(), &config) {
                Ok(()) => {
                    status.set_text("Saved.");
                    on_saved();
                    dialog.close();
                }
                Err(err) => status.set_text(&err),
            }
        });
    }

    dialog.set_child(Some(&root));
    dialog.present();
}

fn open_loop_text_dialog(
    parent: &gtk::Window,
    title: &str,
    intro_text: &str,
    initial_text: &str,
    default_text: &str,
    on_save: Rc<dyn Fn(String) -> Result<(), String>>,
) {
    let dialog = gtk::Window::builder()
        .title(title)
        .default_width(720)
        .default_height(520)
        .modal(true)
        .transient_for(parent)
        .build();
    dialog.add_css_class("settings-window");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(14);
    root.set_margin_end(14);
    root.set_margin_top(14);
    root.set_margin_bottom(14);

    let intro = gtk::Label::new(Some(intro_text));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    intro.add_css_class("dim-label");
    root.append(&intro);

    let prompt_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();
    prompt_scroll.set_has_frame(false);
    prompt_scroll.add_css_class("composer-input");
    let prompt_view = gtk::TextView::new();
    prompt_view.set_wrap_mode(gtk::WrapMode::WordChar);
    prompt_view.set_top_margin(10);
    prompt_view.set_bottom_margin(10);
    prompt_view.set_left_margin(10);
    prompt_view.set_right_margin(10);
    prompt_view.add_css_class("composer-input-view");
    apply_loop_editor_theme(
        &prompt_scroll,
        &prompt_view,
        "enzim-agent-loop-text-scroll",
        "enzim-agent-loop-text-view",
    );
    let prompt_buf = prompt_view.buffer();
    prompt_buf.set_text(initial_text);
    prompt_scroll.set_child(Some(&prompt_view));
    root.append(&prompt_scroll);

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.add_css_class("dim-label");
    root.append(&status);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let reset = gtk::Button::with_label("Reset");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    actions.append(&reset);
    actions.append(&save);
    root.append(&actions);

    {
        let prompt_buf = prompt_buf.clone();
        let status = status.clone();
        let default_text = default_text.to_string();
        reset.connect_clicked(move |_| {
            prompt_buf.set_text(&default_text);
            status.set_text("Restored the built-in default.");
        });
    }

    {
        let prompt_buf = prompt_buf.clone();
        let status = status.clone();
        let dialog = dialog.clone();
        let on_save = on_save.clone();
        save.connect_clicked(move |_| {
            let start = prompt_buf.start_iter();
            let end = prompt_buf.end_iter();
            let text = prompt_buf.text(&start, &end, true).to_string();
            match on_save(text) {
                Ok(()) => {
                    status.set_text("Saved.");
                    dialog.close();
                }
                Err(err) => status.set_text(&err),
            }
        });
    }

    dialog.set_child(Some(&root));
    dialog.present();
}

pub(crate) fn build_settings_page(dialog: &gtk::Window, db: Rc<AppDb>) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let intro = gtk::Label::new(Some(
        "Configure the model used by Enzim Agent to supervise loops and send continue / finish / ask-user decisions.",
    ));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    intro.add_css_class("dim-label");
    root.append(&intro);

    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    section.add_css_class("profile-settings-section");

    let title = gtk::Label::new(Some("Enzim Agent"));
    title.set_xalign(0.0);
    title.add_css_class("profile-section-title");
    section.append(&title);

    let config_state = Rc::new(RefCell::new(crate::services::enzim_agent::load_config(
        db.as_ref(),
    )));

    let url_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let url_label = gtk::Label::new(Some("Base URL"));
    url_label.set_width_chars(14);
    url_label.set_xalign(0.0);
    let url_entry = gtk::Entry::new();
    url_entry.set_hexpand(true);
    url_entry.set_placeholder_text(Some("https://api.openai.com/v1"));
    url_entry.set_text(&config_state.borrow().base_url);
    url_row.append(&url_label);
    url_row.append(&url_entry);
    section.append(&url_row);

    let key_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let key_label = gtk::Label::new(Some("API key"));
    key_label.set_width_chars(14);
    key_label.set_xalign(0.0);
    let key_entry = gtk::PasswordEntry::new();
    key_entry.set_hexpand(true);
    key_entry.set_show_peek_icon(true);
    key_entry.set_text(config_state.borrow().api_key.as_deref().unwrap_or(""));
    key_row.append(&key_label);
    key_row.append(&key_entry);
    section.append(&key_row);

    let model_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let model_label = gtk::Label::new(Some("Model"));
    model_label.set_width_chars(14);
    model_label.set_xalign(0.0);
    let model_list = gtk::StringList::new(&[]);
    let model_dropdown = gtk::DropDown::new(Some(model_list.clone()), None::<&gtk::Expression>);
    model_dropdown.set_hexpand(true);
    model_row.append(&model_label);
    model_row.append(&model_dropdown);
    section.append(&model_row);

    let models_state: Rc<RefCell<Vec<crate::services::enzim_agent::EnzimAgentModelOption>>> =
        Rc::new(RefCell::new(Vec::new()));

    let reload_models_ui: Rc<dyn Fn()> = {
        let config_state = config_state.clone();
        let model_list = model_list.clone();
        let model_dropdown = model_dropdown.clone();
        let models_state = models_state.clone();
        Rc::new(move || {
            while model_list.n_items() > 0 {
                model_list.remove(0);
            }
            let config = config_state.borrow().clone();
            let mut selected_index = gtk::INVALID_LIST_POSITION;
            for (idx, model) in config.cached_models.iter().enumerate() {
                model_list.append(&model.display_name);
                if config.model_id.as_deref() == Some(model.id.as_str()) {
                    selected_index = idx as u32;
                }
            }
            if selected_index == gtk::INVALID_LIST_POSITION && !config.cached_models.is_empty() {
                selected_index = 0;
            }
            model_dropdown.set_selected(selected_index);
            models_state.replace(config.cached_models);
        })
    };
    (reload_models_ui)();

    let prompt_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let prompt_label = gtk::Label::new(Some("Loop system prompt"));
    prompt_label.set_width_chars(14);
    prompt_label.set_xalign(0.0);
    let prompt_state_label = gtk::Label::new(Some(loop_default_state_label(
        config_state
            .borrow()
            .system_prompt_override
            .as_deref()
            .unwrap_or(""),
    )));
    prompt_state_label.set_xalign(0.0);
    prompt_state_label.set_hexpand(true);
    prompt_state_label.add_css_class("dim-label");
    let prompt_edit = gtk::Button::with_label("Edit");
    let prompt_reset = gtk::Button::with_label("Reset");
    prompt_row.append(&prompt_label);
    prompt_row.append(&prompt_state_label);
    prompt_row.append(&prompt_edit);
    prompt_row.append(&prompt_reset);
    section.append(&prompt_row);

    let ask_prompt_state = Rc::new(RefCell::new(
        crate::services::enzim_agent::load_ask_system_prompt_override(db.as_ref())
            .unwrap_or_default(),
    ));

    let ask_prompt_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let ask_prompt_label = gtk::Label::new(Some("General chat prompt"));
    ask_prompt_label.set_width_chars(14);
    ask_prompt_label.set_xalign(0.0);
    let ask_prompt_state_label = gtk::Label::new(Some(loop_default_state_label(
        ask_prompt_state.borrow().as_str(),
    )));
    ask_prompt_state_label.set_xalign(0.0);
    ask_prompt_state_label.set_hexpand(true);
    ask_prompt_state_label.add_css_class("dim-label");
    let ask_prompt_edit = gtk::Button::with_label("Edit");
    let ask_prompt_reset = gtk::Button::with_label("Reset");
    ask_prompt_row.append(&ask_prompt_label);
    ask_prompt_row.append(&ask_prompt_state_label);
    ask_prompt_row.append(&ask_prompt_edit);
    ask_prompt_row.append(&ask_prompt_reset);
    section.append(&ask_prompt_row);

    let loop_defaults = Rc::new(RefCell::new(
        crate::services::enzim_agent::load_loop_draft_defaults(db.as_ref()),
    ));

    let default_prompt_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let default_prompt_label = gtk::Label::new(Some("Default prompt"));
    default_prompt_label.set_width_chars(14);
    default_prompt_label.set_xalign(0.0);
    let default_prompt_state = gtk::Label::new(Some(loop_default_state_label(
        &loop_defaults.borrow().prompt_text,
    )));
    default_prompt_state.set_xalign(0.0);
    default_prompt_state.set_hexpand(true);
    default_prompt_state.add_css_class("dim-label");
    let default_prompt_edit = gtk::Button::with_label("Edit");
    let default_prompt_reset = gtk::Button::with_label("Reset");
    default_prompt_row.append(&default_prompt_label);
    default_prompt_row.append(&default_prompt_state);
    default_prompt_row.append(&default_prompt_edit);
    default_prompt_row.append(&default_prompt_reset);
    section.append(&default_prompt_row);

    let default_instructions_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let default_instructions_label = gtk::Label::new(Some("Default instructions"));
    default_instructions_label.set_width_chars(14);
    default_instructions_label.set_xalign(0.0);
    let default_instructions_state = gtk::Label::new(Some(loop_default_state_label(
        &loop_defaults.borrow().instructions_text,
    )));
    default_instructions_state.set_xalign(0.0);
    default_instructions_state.set_hexpand(true);
    default_instructions_state.add_css_class("dim-label");
    let default_instructions_edit = gtk::Button::with_label("Edit");
    let default_instructions_reset = gtk::Button::with_label("Reset");
    default_instructions_row.append(&default_instructions_label);
    default_instructions_row.append(&default_instructions_state);
    default_instructions_row.append(&default_instructions_edit);
    default_instructions_row.append(&default_instructions_reset);
    section.append(&default_instructions_row);

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status.add_css_class("dim-label");
    section.append(&status);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let save = gtk::Button::with_label("Save");
    let refresh_models = gtk::Button::with_label("Refresh Models");
    save.add_css_class("suggested-action");
    actions.append(&refresh_models);
    actions.append(&save);
    section.append(&actions);

    root.append(&section);

    let reset_save_button: Rc<dyn Fn()> = {
        let save = save.clone();
        Rc::new(move || {
            save.set_label("Save");
            save.remove_css_class("save-confirmed");
        })
    };

    {
        let config_state = config_state.clone();
        let models_state = models_state.clone();
        let reset_save_button = reset_save_button.clone();
        model_dropdown.connect_selected_notify(move |dropdown| {
            let selected = dropdown.selected();
            if selected == gtk::INVALID_LIST_POSITION {
                config_state.borrow_mut().model_id = None;
                reset_save_button();
                return;
            }
            let models = models_state.borrow();
            config_state.borrow_mut().model_id =
                models.get(selected as usize).map(|model| model.id.clone());
            reset_save_button();
        });
    }

    {
        let db = db.clone();
        let config_state = config_state.clone();
        let reload_models_ui = reload_models_ui.clone();
        let prompt_state_label = prompt_state_label.clone();
        let dialog = dialog.clone();
        prompt_edit.connect_clicked(move |_| {
            let db = db.clone();
            let config_state = config_state.clone();
            let reload_models_ui = reload_models_ui.clone();
            let prompt_state_label = prompt_state_label.clone();
            open_system_prompt_dialog(
                &dialog,
                db.clone(),
                Rc::new(move || {
                    config_state.replace(crate::services::enzim_agent::load_config(db.as_ref()));
                    prompt_state_label.set_text(loop_default_state_label(
                        config_state
                            .borrow()
                            .system_prompt_override
                            .as_deref()
                            .unwrap_or(""),
                    ));
                    (reload_models_ui)();
                }),
            );
        });
    }

    {
        let db = db.clone();
        let dialog = dialog.clone();
        let ask_prompt_state = ask_prompt_state.clone();
        let ask_prompt_state_label = ask_prompt_state_label.clone();
        ask_prompt_edit.connect_clicked(move |_| {
            let dialog = dialog.clone();
            let db = db.clone();
            let ask_prompt_state = ask_prompt_state.clone();
            let ask_prompt_state_label = ask_prompt_state_label.clone();
            let stored_text = ask_prompt_state.borrow().clone();
            let initial_text = if stored_text.trim().is_empty() {
                crate::services::enzim_agent::ask_system_prompt().to_string()
            } else {
                stored_text
            };
            open_loop_text_dialog(
                &dialog,
                "General Chat System Prompt",
                "This system prompt is used by the Ask popup for general chat. Reset restores the built-in default.",
                &initial_text,
                crate::services::enzim_agent::ask_system_prompt(),
                Rc::new(move |text| {
                    let next_value = if text.trim().is_empty()
                        || text.trim() == crate::services::enzim_agent::ask_system_prompt().trim()
                    {
                        None
                    } else {
                        Some(text.as_str())
                    };
                    crate::services::enzim_agent::save_ask_system_prompt_override(
                        db.as_ref(),
                        next_value,
                    )?;
                    let stored = crate::services::enzim_agent::load_ask_system_prompt_override(
                        db.as_ref(),
                    )
                    .unwrap_or_default();
                    ask_prompt_state.replace(stored.clone());
                    ask_prompt_state_label.set_text(loop_default_state_label(&stored));
                    Ok(())
                }),
            );
        });
    }

    {
        let db = db.clone();
        let ask_prompt_state = ask_prompt_state.clone();
        let ask_prompt_state_label = ask_prompt_state_label.clone();
        ask_prompt_reset.connect_clicked(move |_| {
            match crate::services::enzim_agent::save_ask_system_prompt_override(db.as_ref(), None) {
                Ok(()) => {
                    ask_prompt_state.replace(String::new());
                    ask_prompt_state_label.set_text("Using built-in default");
                }
                Err(err) => eprintln!("failed to reset Enzim Agent general chat prompt: {err}"),
            }
        });
    }

    {
        let db = db.clone();
        let dialog = dialog.clone();
        let loop_defaults = loop_defaults.clone();
        let default_prompt_state = default_prompt_state.clone();
        default_prompt_edit.connect_clicked(move |_| {
            let dialog = dialog.clone();
            let db = db.clone();
            let loop_defaults = loop_defaults.clone();
            let default_prompt_state = default_prompt_state.clone();
            let stored_text = loop_defaults.borrow().prompt_text.clone();
            let initial_text = if stored_text.trim().is_empty() {
                crate::services::enzim_agent::default_loop_prompt().to_string()
            } else {
                stored_text
            };
            open_loop_text_dialog(
                &dialog,
                "Default Loop Prompt",
                "This fills the composer popup prompt field for new Enzim loops. Reset restores the built-in default.",
                &initial_text,
                crate::services::enzim_agent::default_loop_prompt(),
                Rc::new(move |text| {
                    let mut defaults =
                        crate::services::enzim_agent::load_loop_draft_defaults(db.as_ref());
                    defaults.prompt_text = if text.trim().is_empty()
                        || text.trim()
                            == crate::services::enzim_agent::default_loop_prompt().trim()
                    {
                        String::new()
                    } else {
                        text
                    };
                    crate::services::enzim_agent::save_loop_draft_defaults(db.as_ref(), &defaults)?;
                    loop_defaults.replace(defaults.clone());
                    default_prompt_state
                        .set_text(loop_default_state_label(&defaults.prompt_text));
                    Ok(())
                }),
            );
        });
    }

    {
        let db = db.clone();
        let loop_defaults = loop_defaults.clone();
        let default_prompt_state = default_prompt_state.clone();
        default_prompt_reset.connect_clicked(move |_| {
            let mut defaults = crate::services::enzim_agent::load_loop_draft_defaults(db.as_ref());
            defaults.prompt_text.clear();
            match crate::services::enzim_agent::save_loop_draft_defaults(db.as_ref(), &defaults) {
                Ok(()) => {
                    loop_defaults.replace(defaults);
                    default_prompt_state.set_text("Using built-in default");
                }
                Err(err) => eprintln!("failed to reset Enzim Agent default prompt: {err}"),
            }
        });
    }

    {
        let db = db.clone();
        let dialog = dialog.clone();
        let loop_defaults = loop_defaults.clone();
        let default_instructions_state = default_instructions_state.clone();
        default_instructions_edit.connect_clicked(move |_| {
            let dialog = dialog.clone();
            let db = db.clone();
            let loop_defaults = loop_defaults.clone();
            let default_instructions_state = default_instructions_state.clone();
            let stored_text = loop_defaults.borrow().instructions_text.clone();
            let initial_text = if stored_text.trim().is_empty() {
                crate::services::enzim_agent::default_loop_instructions().to_string()
            } else {
                stored_text
            };
            open_loop_text_dialog(
                &dialog,
                "Default Looping Instructions",
                "This fills the composer popup looping-instructions field for new Enzim loops. Reset restores the built-in default.",
                &initial_text,
                crate::services::enzim_agent::default_loop_instructions(),
                Rc::new(move |text| {
                    let mut defaults =
                        crate::services::enzim_agent::load_loop_draft_defaults(db.as_ref());
                    defaults.instructions_text = if text.trim().is_empty()
                        || text.trim()
                            == crate::services::enzim_agent::default_loop_instructions().trim()
                    {
                        String::new()
                    } else {
                        text
                    };
                    crate::services::enzim_agent::save_loop_draft_defaults(db.as_ref(), &defaults)?;
                    loop_defaults.replace(defaults.clone());
                    default_instructions_state
                        .set_text(loop_default_state_label(&defaults.instructions_text));
                    Ok(())
                }),
            );
        });
    }

    {
        let db = db.clone();
        let loop_defaults = loop_defaults.clone();
        let default_instructions_state = default_instructions_state.clone();
        default_instructions_reset.connect_clicked(move |_| {
            let mut defaults = crate::services::enzim_agent::load_loop_draft_defaults(db.as_ref());
            defaults.instructions_text.clear();
            match crate::services::enzim_agent::save_loop_draft_defaults(db.as_ref(), &defaults) {
                Ok(()) => {
                    loop_defaults.replace(defaults);
                    default_instructions_state.set_text("Using built-in default");
                }
                Err(err) => {
                    eprintln!("failed to reset Enzim Agent default instructions: {err}")
                }
            }
        });
    }

    {
        let db = db.clone();
        let config_state = config_state.clone();
        let prompt_state_label = prompt_state_label.clone();
        prompt_reset.connect_clicked(move |_| {
            let mut config = config_state.borrow().clone();
            config.system_prompt_override = None;
            match crate::services::enzim_agent::save_config(db.as_ref(), &config) {
                Ok(()) => {
                    config_state.replace(crate::services::enzim_agent::load_config(db.as_ref()));
                    prompt_state_label.set_text("Using built-in default");
                }
                Err(err) => eprintln!("failed to reset Enzim Agent prompt override: {err}"),
            }
        });
    }

    {
        let db = db.clone();
        let config_state = config_state.clone();
        let url_entry = url_entry.clone();
        let key_entry = key_entry.clone();
        let status = status.clone();
        let reset_save_button = reset_save_button.clone();
        let model_dropdown = model_dropdown.clone();
        let save = save.clone();
        let refresh_models = refresh_models.clone();
        let reload_models_ui = reload_models_ui.clone();
        let url_entry_for_refresh = url_entry.clone();
        refresh_models.clone().connect_clicked(move |_| {
            let mut config = config_state.borrow().clone();
            config.base_url = url_entry_for_refresh.text().to_string();
            config.api_key = Some(key_entry.text().to_string());
            status.set_text("Refreshing models...");
            save.set_sensitive(false);
            refresh_models.set_sensitive(false);
            if let Err(err) = crate::services::enzim_agent::save_config(db.as_ref(), &config) {
                status.set_text(&err);
                save.set_sensitive(true);
                refresh_models.set_sensitive(true);
                return;
            }

            let (tx, rx) = mpsc::channel::<
                Result<Vec<crate::services::enzim_agent::EnzimAgentModelOption>, String>,
            >();
            std::thread::spawn(move || {
                let result = AppDb::open_detached()
                    .map_err(|err| err.to_string())
                    .and_then(|db| crate::services::enzim_agent::refresh_models(&db));
                let _ = tx.send(result);
            });

            let config_state_for_timer = config_state.clone();
            let db_for_timer = db.clone();
            let reload_models_ui_for_timer = reload_models_ui.clone();
            let model_dropdown_for_timer = model_dropdown.clone();
            let status_for_timer = status.clone();
            let save_for_timer = save.clone();
            let refresh_models_for_timer = refresh_models.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(60), move || match rx.try_recv() {
                Ok(Ok(models)) => {
                    config_state_for_timer.replace(crate::services::enzim_agent::load_config(
                        db_for_timer.as_ref(),
                    ));
                    (reload_models_ui_for_timer)();
                    if !models.is_empty()
                        && model_dropdown_for_timer.selected() == gtk::INVALID_LIST_POSITION
                    {
                        model_dropdown_for_timer.set_selected(0);
                    }
                    status_for_timer.set_text("Models refreshed.");
                    save_for_timer.set_sensitive(true);
                    refresh_models_for_timer.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    config_state_for_timer.replace(crate::services::enzim_agent::load_config(
                        db_for_timer.as_ref(),
                    ));
                    (reload_models_ui_for_timer)();
                    status_for_timer.set_text(&err);
                    save_for_timer.set_sensitive(true);
                    refresh_models_for_timer.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    status_for_timer.set_text("Model refresh worker disconnected unexpectedly.");
                    save_for_timer.set_sensitive(true);
                    refresh_models_for_timer.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
            });
        });

        url_entry.connect_changed(move |_| {
            reset_save_button();
        });
    }

    {
        let reset_save_button = reset_save_button.clone();
        key_entry.connect_changed(move |_| {
            reset_save_button();
        });
    }

    {
        let db = db.clone();
        let config_state = config_state.clone();
        let url_entry = url_entry.clone();
        let key_entry = key_entry.clone();
        let status = status.clone();
        let save = save.clone();
        save.clone().connect_clicked(move |_| {
            let mut config = config_state.borrow().clone();
            config.base_url = url_entry.text().to_string();
            config.api_key = Some(key_entry.text().to_string());
            match crate::services::enzim_agent::save_config(db.as_ref(), &config) {
                Ok(()) => {
                    config_state.replace(crate::services::enzim_agent::load_config(db.as_ref()));
                    status.set_text("");
                    save.set_label("Saved");
                    save.add_css_class("save-confirmed");
                }
                Err(err) => status.set_text(&err),
            }
        });
    }

    root
}
