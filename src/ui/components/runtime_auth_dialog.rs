use crate::codex_appserver::AccountInfo;
use crate::codex_profiles::CodexProfileManager;
use crate::data::AppDb;
use adw::prelude::*;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub(crate) fn reload_opencode_runtime_after_auth(manager: &CodexProfileManager, profile_id: i64) {
    if let Err(err) = manager.restart_profile(profile_id) {
        eprintln!(
            "[opencode-auth] failed to restart runtime for profile {} after auth update: {}",
            profile_id, err
        );
    }
    crate::ui::components::chat::runtime_controls::invalidate_model_options_cache_for_backend(
        "opencode",
    );
    if let Ok(client) = manager.ensure_started(profile_id) {
        crate::ui::components::chat::runtime_controls::refresh_model_options_cache_async(Some(
            client,
        ));
    }
}

pub(crate) fn sync_runtime_account_to_db(
    db: &AppDb,
    profile_id: i64,
    account: Option<AccountInfo>,
) -> Result<(), String> {
    let account_type = account.as_ref().map(|value| value.account_type.as_str());
    let email = account.as_ref().and_then(|value| value.email.as_deref());
    sync_runtime_account_fields_to_db(db, profile_id, account_type, email)
}

pub(crate) fn sync_runtime_account_fields_to_db(
    db: &AppDb,
    profile_id: i64,
    account_type: Option<&str>,
    email: Option<&str>,
) -> Result<(), String> {
    db.update_profile_account_identity(profile_id, account_type, email)
        .map_err(|err| err.to_string())?;
    if db.active_profile_id().ok().flatten() == Some(profile_id) {
        db.set_current_profile_account_identity(account_type, email)
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

pub(crate) fn clear_runtime_account_for_profile(db: &AppDb, profile_id: i64) -> Result<(), String> {
    sync_runtime_account_fields_to_db(db, profile_id, None, None)
}

pub(crate) fn start_opencode_api_key_flow(
    parent: Option<&gtk::Window>,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    profile_id: i64,
    trigger_button: gtk::Button,
    status_label: gtk::Label,
    status_class: &'static str,
    on_success: Rc<dyn Fn(Option<AccountInfo>, String)>,
) {
    let client = match manager.ensure_started(profile_id) {
        Ok(client) => client,
        Err(err) => {
            status_label.set_visible(true);
            status_label.set_text(&format!("Unable to start the selected runtime: {err}"));
            trigger_button.set_sensitive(true);
            return;
        }
    };
    if client.backend_kind() != "opencode" {
        status_label.set_visible(true);
        status_label.set_text("API-key login is currently only exposed for OpenCode profiles.");
        trigger_button.set_sensitive(true);
        return;
    }

    trigger_button.set_sensitive(false);
    status_label.set_visible(true);
    status_label.set_text("Loading provider list...");
    let (tx, rx) = mpsc::channel::<Result<Vec<(String, String)>, String>>();
    thread::spawn(move || {
        let _ = tx.send(client.account_api_key_provider_options());
    });

    let parent = parent.cloned();
    gtk::glib::timeout_add_local(Duration::from_millis(60), move || match rx.try_recv() {
        Ok(Ok(options)) => {
            trigger_button.set_sensitive(true);
            if options.is_empty() {
                status_label.set_text(
                    "No API-key capable OpenCode providers were reported by the runtime.",
                );
                return gtk::glib::ControlFlow::Break;
            }

            let mut prompt_builder = gtk::Window::builder()
                .title("Set OpenCode API Key")
                .default_width(420)
                .modal(true);
            if let Some(parent) = parent.as_ref() {
                prompt_builder = prompt_builder.transient_for(parent);
            }
            let prompt = prompt_builder.build();
            let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
            root.set_margin_start(12);
            root.set_margin_end(12);
            root.set_margin_top(12);
            root.set_margin_bottom(12);
            let provider_label = gtk::Label::new(Some("Provider"));
            provider_label.set_xalign(0.0);
            root.append(&provider_label);
            let provider_model = gtk::StringList::new(
                &options
                    .iter()
                    .map(|(_, label)| label.as_str())
                    .collect::<Vec<_>>(),
            );
            let provider_dropdown =
                gtk::DropDown::new(Some(provider_model), None::<&gtk::Expression>);
            provider_dropdown.set_selected(0);
            root.append(&provider_dropdown);
            let key_label = gtk::Label::new(Some("API Key"));
            key_label.set_xalign(0.0);
            root.append(&key_label);
            let key_entry = gtk::PasswordEntry::new();
            key_entry.set_show_peek_icon(true);
            key_entry.set_placeholder_text(Some("Paste provider API key"));
            root.append(&key_entry);
            let dialog_status = gtk::Label::new(None);
            dialog_status.set_xalign(0.0);
            dialog_status.set_wrap(true);
            dialog_status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            dialog_status.add_css_class(status_class);
            dialog_status.set_visible(false);
            root.append(&dialog_status);
            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            actions.set_halign(gtk::Align::End);
            let cancel = gtk::Button::with_label("Cancel");
            let save = gtk::Button::with_label("Save");
            save.add_css_class("suggested-action");
            actions.append(&cancel);
            actions.append(&save);
            root.append(&actions);
            prompt.set_child(Some(&root));

            {
                let prompt = prompt.clone();
                cancel.connect_clicked(move |_| prompt.close());
            }

            {
                let options = options.clone();
                let db = db.clone();
                let manager = manager.clone();
                let status_label = status_label.clone();
                let dialog_status = dialog_status.clone();
                let key_entry = key_entry.clone();
                let provider_dropdown = provider_dropdown.clone();
                let prompt = prompt.clone();
                let save_button = save.clone();
                let on_success = on_success.clone();
                save.connect_clicked(move |_| {
                    let selected = provider_dropdown.selected() as usize;
                    let Some((provider_id, provider_name)) = options.get(selected).cloned() else {
                        dialog_status.set_visible(true);
                        dialog_status.set_text("Select a provider.");
                        return;
                    };
                    let api_key = key_entry.text().trim().to_string();
                    if api_key.is_empty() {
                        dialog_status.set_visible(true);
                        dialog_status.set_text("API key is required.");
                        return;
                    }

                    dialog_status.set_visible(true);
                    dialog_status.set_text("Saving API key...");
                    save_button.set_sensitive(false);

                    let client = match manager.ensure_started(profile_id) {
                        Ok(client) => client,
                        Err(err) => {
                            dialog_status
                                .set_text(&format!("Unable to start the selected runtime: {err}"));
                            save_button.set_sensitive(true);
                            return;
                        }
                    };
                    let (tx, rx) = mpsc::channel::<Result<Option<AccountInfo>, String>>();
                    thread::spawn(move || {
                        let result = client
                            .account_login_start_api_key_for_provider(&provider_id, &api_key)
                            .and_then(|_| client.account_read(true));
                        let _ = tx.send(result);
                    });

                    let db = db.clone();
                    let status_label = status_label.clone();
                    let dialog_status = dialog_status.clone();
                    let prompt = prompt.clone();
                    let save_button = save_button.clone();
                    let manager = manager.clone();
                    let on_success = on_success.clone();
                    let provider_name_for_done = provider_name.clone();
                    gtk::glib::timeout_add_local(Duration::from_millis(60), move || {
                        match rx.try_recv() {
                            Ok(Ok(account)) => {
                                if let Err(err) =
                                    sync_runtime_account_to_db(&db, profile_id, account.clone())
                                {
                                    dialog_status
                                        .set_text(&format!("Failed to store account info: {err}"));
                                    save_button.set_sensitive(true);
                                    return gtk::glib::ControlFlow::Break;
                                }
                                status_label.set_visible(true);
                                status_label.set_text(&format!(
                                    "Saved API key for {provider_name_for_done}."
                                ));
                                reload_opencode_runtime_after_auth(&manager, profile_id);
                                on_success(account, provider_name_for_done.clone());
                                prompt.close();
                                gtk::glib::ControlFlow::Break
                            }
                            Ok(Err(err)) => {
                                dialog_status.set_text(&format!("Failed to save API key: {err}"));
                                save_button.set_sensitive(true);
                                gtk::glib::ControlFlow::Break
                            }
                            Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                dialog_status
                                    .set_text("API-key login request stopped unexpectedly.");
                                save_button.set_sensitive(true);
                                gtk::glib::ControlFlow::Break
                            }
                        }
                    });
                });
            }

            prompt.present();
            gtk::glib::ControlFlow::Break
        }
        Ok(Err(err)) => {
            trigger_button.set_sensitive(true);
            status_label.set_text(&format!("Unable to load provider list: {err}"));
            gtk::glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => {
            trigger_button.set_sensitive(true);
            status_label.set_text("Provider list request stopped unexpectedly.");
            gtk::glib::ControlFlow::Break
        }
    });
}

pub(crate) fn start_opencode_api_key_flow_for_provider(
    parent: Option<&gtk::Window>,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    profile_id: i64,
    provider_id: String,
    provider_name: String,
    trigger_button: gtk::Button,
    status_label: gtk::Label,
    status_class: &'static str,
    on_success: Rc<dyn Fn(Option<AccountInfo>, String)>,
) {
    let client = match manager.ensure_started(profile_id) {
        Ok(client) => client,
        Err(err) => {
            status_label.set_visible(true);
            status_label.set_text(&format!("Unable to start the selected runtime: {err}"));
            trigger_button.set_sensitive(true);
            return;
        }
    };
    if client.backend_kind() != "opencode" {
        status_label.set_visible(true);
        status_label.set_text("API-key login is currently only exposed for OpenCode profiles.");
        trigger_button.set_sensitive(true);
        return;
    }

    trigger_button.set_sensitive(false);

    let mut prompt_builder = gtk::Window::builder()
        .title(&format!("Set {provider_name} API Key"))
        .default_width(420)
        .modal(true);
    if let Some(parent) = parent {
        prompt_builder = prompt_builder.transient_for(parent);
    }
    let prompt = prompt_builder.build();
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);
    let provider_label = gtk::Label::new(Some(&format!("Provider: {provider_name}")));
    provider_label.set_xalign(0.0);
    root.append(&provider_label);
    let key_label = gtk::Label::new(Some("API Key"));
    key_label.set_xalign(0.0);
    root.append(&key_label);
    let key_entry = gtk::PasswordEntry::new();
    key_entry.set_show_peek_icon(true);
    key_entry.set_placeholder_text(Some("Paste provider API key"));
    root.append(&key_entry);
    let dialog_status = gtk::Label::new(None);
    dialog_status.set_xalign(0.0);
    dialog_status.set_wrap(true);
    dialog_status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    dialog_status.add_css_class(status_class);
    dialog_status.set_visible(false);
    root.append(&dialog_status);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    actions.append(&cancel);
    actions.append(&save);
    root.append(&actions);
    prompt.set_child(Some(&root));

    {
        let prompt = prompt.clone();
        let trigger_button = trigger_button.clone();
        cancel.connect_clicked(move |_| {
            trigger_button.set_sensitive(true);
            prompt.close();
        });
    }

    {
        let db = db.clone();
        let manager = manager.clone();
        let status_label = status_label.clone();
        let dialog_status = dialog_status.clone();
        let key_entry = key_entry.clone();
        let prompt = prompt.clone();
        let save_button = save.clone();
        let on_success = on_success.clone();
        let provider_id = provider_id.clone();
        let provider_name = provider_name.clone();
        let trigger_button = trigger_button.clone();
        save.connect_clicked(move |_| {
            let api_key = key_entry.text().trim().to_string();
            if api_key.is_empty() {
                dialog_status.set_visible(true);
                dialog_status.set_text("API key is required.");
                return;
            }

            dialog_status.set_visible(true);
            dialog_status.set_text("Saving API key...");
            save_button.set_sensitive(false);

            let client = match manager.ensure_started(profile_id) {
                Ok(client) => client,
                Err(err) => {
                    dialog_status.set_text(&format!("Unable to start the selected runtime: {err}"));
                    save_button.set_sensitive(true);
                    return;
                }
            };
            let (tx, rx) = mpsc::channel::<Result<Option<AccountInfo>, String>>();
            let provider_id_for_thread = provider_id.clone();
            let api_key_for_thread = api_key.clone();
            thread::spawn(move || {
                let result = client
                    .account_login_start_api_key_for_provider(
                        &provider_id_for_thread,
                        &api_key_for_thread,
                    )
                    .and_then(|_| client.account_read(true));
                let _ = tx.send(result);
            });

            let db = db.clone();
            let status_label = status_label.clone();
            let dialog_status = dialog_status.clone();
            let prompt = prompt.clone();
            let save_button = save_button.clone();
            let manager = manager.clone();
            let on_success = on_success.clone();
            let provider_name_for_done = provider_name.clone();
            let trigger_button = trigger_button.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(60), move || match rx.try_recv() {
                Ok(Ok(account)) => {
                    if let Err(err) = sync_runtime_account_to_db(&db, profile_id, account.clone()) {
                        dialog_status.set_text(&format!("Failed to store account info: {err}"));
                        save_button.set_sensitive(true);
                        trigger_button.set_sensitive(true);
                        return gtk::glib::ControlFlow::Break;
                    }
                    status_label.set_visible(true);
                    status_label.set_text(&format!("Saved API key for {provider_name_for_done}."));
                    reload_opencode_runtime_after_auth(&manager, profile_id);
                    trigger_button.set_sensitive(true);
                    on_success(account, provider_name_for_done.clone());
                    prompt.close();
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    dialog_status.set_text(&format!("Failed to save API key: {err}"));
                    save_button.set_sensitive(true);
                    trigger_button.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    dialog_status.set_text("API-key login request stopped unexpectedly.");
                    save_button.set_sensitive(true);
                    trigger_button.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
            });
        });
    }

    {
        let trigger_button = trigger_button.clone();
        prompt.connect_close_request(move |_| {
            trigger_button.set_sensitive(true);
            gtk::glib::Propagation::Proceed
        });
    }

    prompt.present();
}
