use crate::services::app::chat::{AppDb, ThreadAutocloseConfig};
use gtk::prelude::*;
use std::rc::Rc;

fn persist_thread_autoclose_config(db: &AppDb, enabled: bool, days: i64) {
    let config = ThreadAutocloseConfig {
        enabled,
        days: days.max(1),
    };
    if let Err(err) = db.upsert_thread_autoclose_config(&config) {
        eprintln!("failed to save thread autoclose config: {err}");
    }
}

pub(crate) fn build_settings_page(_dialog: &gtk::Window, db: Rc<AppDb>) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let intro = gtk::Label::new(Some("General app behavior and housekeeping settings."));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    intro.add_css_class("dim-label");
    root.append(&intro);

    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    section.add_css_class("profile-settings-section");

    let section_title = gtk::Label::new(Some("Threads"));
    section_title.set_xalign(0.0);
    section_title.add_css_class("profile-section-title");
    section.append(&section_title);

    let toggle_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let toggle_text = gtk::Box::new(gtk::Orientation::Vertical, 4);
    toggle_text.set_hexpand(true);
    let toggle_title = gtk::Label::new(Some("Autoclose threads"));
    toggle_title.set_xalign(0.0);
    toggle_title.add_css_class("settings-subtitle");
    let toggle_subtitle = gtk::Label::new(Some(
        "Close threads automatically when their last message is older than the configured age.",
    ));
    toggle_subtitle.set_xalign(0.0);
    toggle_subtitle.set_wrap(true);
    toggle_subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    toggle_subtitle.add_css_class("dim-label");
    toggle_text.append(&toggle_title);
    toggle_text.append(&toggle_subtitle);
    let toggle = gtk::Switch::new();
    toggle.set_valign(gtk::Align::Center);
    toggle_row.append(&toggle_text);
    toggle_row.append(&toggle);
    section.append(&toggle_row);

    let days_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let days_label = gtk::Label::new(Some("Close after"));
    days_label.set_xalign(0.0);
    days_label.set_width_chars(12);
    let days_adjustment = gtk::Adjustment::new(30.0, 1.0, 3650.0, 1.0, 7.0, 0.0);
    let days_spin = gtk::SpinButton::new(Some(&days_adjustment), 1.0, 0);
    days_spin.set_numeric(true);
    days_spin.set_hexpand(false);
    let days_suffix = gtk::Label::new(Some("days since the last message"));
    days_suffix.set_xalign(0.0);
    days_suffix.add_css_class("dim-label");
    days_suffix.set_hexpand(true);
    days_row.append(&days_label);
    days_row.append(&days_spin);
    days_row.append(&days_suffix);
    section.append(&days_row);

    root.append(&section);

    let initial = db.thread_autoclose_config().unwrap_or_default();
    toggle.set_active(initial.enabled);
    days_spin.set_value(initial.days as f64);
    days_spin.set_sensitive(initial.enabled);
    persist_thread_autoclose_config(db.as_ref(), initial.enabled, initial.days);

    {
        let db = db.clone();
        let days_spin = days_spin.clone();
        toggle.connect_active_notify(move |toggle| {
            let enabled = toggle.is_active();
            days_spin.set_sensitive(enabled);
            persist_thread_autoclose_config(db.as_ref(), enabled, days_spin.value_as_int() as i64);
        });
    }

    {
        let db = db.clone();
        let toggle = toggle.clone();
        days_spin.connect_value_changed(move |spin| {
            persist_thread_autoclose_config(
                db.as_ref(),
                toggle.is_active(),
                spin.value_as_int() as i64,
            );
        });
    }

    root
}
