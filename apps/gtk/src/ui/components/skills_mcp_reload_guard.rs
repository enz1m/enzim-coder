use crate::services::app::CodexProfileManager;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

fn active_opencode_turn_count(manager: &Rc<CodexProfileManager>, profile_id: i64) -> usize {
    manager
        .running_client_for_profile(profile_id)
        .map(|client| client.active_opencode_turn_count())
        .unwrap_or(0)
}

fn waiting_message(active_count: usize) -> String {
    let thread_label = if active_count == 1 {
        "active OpenCode thread"
    } else {
        "active OpenCode threads"
    };
    format!(
        "OpenCode needs to reload to apply this Skills/MCP change.\n\n{active_count} {thread_label}, waiting to finish turn..."
    )
}

pub(crate) fn run_with_opencode_reload_guard(
    parent: Option<&gtk::Window>,
    manager: Rc<CodexProfileManager>,
    profile_id: i64,
    status_label: gtk::Label,
    on_ready: Rc<dyn Fn()>,
    on_cancel: Rc<dyn Fn()>,
) {
    let active_count = active_opencode_turn_count(&manager, profile_id);
    if active_count == 0 {
        on_ready();
        return;
    }

    let dialog = gtk::Window::builder()
        .title("OpenCode Turn Active")
        .default_width(460)
        .modal(true)
        .build();
    if let Some(parent) = parent {
        dialog.set_transient_for(Some(parent));
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let label = gtk::Label::new(Some(&waiting_message(active_count)));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    root.append(&label);

    let canceled = Rc::new(Cell::new(false));
    let completed = Rc::new(Cell::new(false));

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);

    let cancel = gtk::Button::with_label("Cancel Action");
    {
        let dialog = dialog.clone();
        let canceled = canceled.clone();
        let completed = completed.clone();
        let on_cancel = on_cancel.clone();
        cancel.connect_clicked(move |_| {
            canceled.set(true);
            completed.set(true);
            on_cancel();
            dialog.close();
        });
    }
    actions.append(&cancel);

    let defer = gtk::Button::with_label("Hide and reload after turn");
    defer.add_css_class("suggested-action");
    {
        let dialog = dialog.clone();
        let manager = manager.clone();
        let status_label = status_label.clone();
        let completed = completed.clone();
        defer.connect_clicked(move |_| {
            if completed.get() {
                return;
            }
            status_label.set_text(&waiting_message(active_opencode_turn_count(
                &manager, profile_id,
            )));
            dialog.close();
        });
    }
    actions.append(&defer);

    root.append(&actions);
    dialog.set_child(Some(&root));
    dialog.present();

    {
        let dialog = dialog.clone();
        let label = label.clone();
        let manager = manager.clone();
        let status_label = status_label.clone();
        let canceled = canceled.clone();
        let completed = completed.clone();
        let on_ready = on_ready.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(400), move || {
            if completed.get() {
                return gtk::glib::ControlFlow::Break;
            }

            let active_count = active_opencode_turn_count(&manager, profile_id);
            let message = waiting_message(active_count);
            label.set_text(&message);
            status_label.set_text(&message);

            if active_count > 0 {
                return gtk::glib::ControlFlow::Continue;
            }

            completed.set(true);
            dialog.close();

            if canceled.get() {
                return gtk::glib::ControlFlow::Break;
            }

            if manager.running_client_for_profile(profile_id).is_none() {
                status_label.set_text(
                    "OpenCode runtime stopped before the deferred Skills/MCP reload could run.",
                );
                return gtk::glib::ControlFlow::Break;
            }

            on_ready();
            gtk::glib::ControlFlow::Break
        });
    }
}
