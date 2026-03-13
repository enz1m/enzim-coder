use crate::codex_profiles::CodexProfileManager;
use crate::data::AppDb;
use adw::prelude::*;
use std::path::PathBuf;
use std::rc::Rc;

use super::profile_settings_dialog;
use super::settings_dialog;
use super::style_picker;

fn active_branch_label(db: &AppDb) -> Option<String> {
    let last_thread_id = db
        .get_setting("last_active_thread_id")
        .ok()
        .flatten()
        .and_then(|raw| raw.trim().parse::<i64>().ok())?;
    db.get_thread_record(last_thread_id).ok().flatten()?;

    let workspace_path = db
        .workspace_path_for_local_thread(last_thread_id)
        .ok()
        .flatten()?;
    let workspace_root = crate::git_exec::run_git_text(
        &PathBuf::from(workspace_path),
        &["rev-parse", "--show-toplevel"],
    )
    .ok()?;
    let repo_root = PathBuf::from(workspace_root.trim());

    crate::git_exec::run_git_text(&repo_root, &["symbolic-ref", "--short", "HEAD"])
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            crate::git_exec::run_git_text(&repo_root, &["rev-parse", "--short", "HEAD"])
                .ok()
                .map(|value| format!("detached@{}", value.trim()))
        })
}

pub fn build_bottom_bar(db: Rc<AppDb>, manager: Rc<CodexProfileManager>) -> gtk::CenterBox {
    let bottom_bar = gtk::CenterBox::new();
    bottom_bar.add_css_class("bottom-section");
    bottom_bar.set_margin_start(10);
    bottom_bar.set_margin_end(10);
    bottom_bar.set_margin_top(4);
    bottom_bar.set_margin_bottom(4);
    bottom_bar.set_valign(gtk::Align::Center);

    let location = gtk::Label::new(Some("local"));
    location.add_css_class("bottom-location");
    location.set_valign(gtk::Align::Center);
    bottom_bar.set_start_widget(Some(&location));
    {
        let db = db.clone();
        let location = location.clone();
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(900), move || {
            if location.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let branch = active_branch_label(&db);
            if let Some(name) = branch {
                location.set_text(&name);
                location.remove_css_class("bottom-location");
                location.add_css_class("bottom-branch");
            } else {
                location.set_text("local");
                location.remove_css_class("bottom-branch");
                location.add_css_class("bottom-location");
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    let right_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    right_box.set_valign(gtk::Align::Center);

    let remote_button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    remote_button.add_css_class("bottom-icon-button");
    remote_button.add_css_class("bottom-remote-button");
    remote_button.set_width_request(18);
    remote_button.set_height_request(18);
    remote_button.set_halign(gtk::Align::Center);
    remote_button.set_valign(gtk::Align::Center);
    remote_button.set_can_focus(false);
    let remote_icon = gtk::Image::from_icon_name("waves-and-screen-symbolic");
    remote_icon.set_pixel_size(15);
    remote_icon.add_css_class("bottom-icon-image");
    remote_button.append(&remote_icon);
    remote_button.set_tooltip_text(Some("Remote mode"));
    {
        let db = db.clone();
        let click_target = remote_button.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_pressed(move |_, _, _, _| {
            click_target.add_css_class("is-active");
        });

        let db = db.clone();
        let manager = manager.clone();
        let click_target = remote_button.clone();
        click.connect_released(move |_, _, _, _| {
            click_target.remove_css_class("is-active");
            let linked_account = db.remote_telegram_active_account().ok().flatten();
            if linked_account.is_none() {
                let parent = click_target
                    .root()
                    .and_then(|root| root.downcast::<gtk::Window>().ok());
                settings_dialog::show(
                    parent.as_ref(),
                    db.clone(),
                    manager.clone(),
                    settings_dialog::SettingsPage::Remote,
                );
                return;
            }
            let next = !db.remote_mode_enabled();
            let _ = db.set_remote_mode_enabled(next);
            if next {
                crate::remote::runtime::start_background_worker();
            }
        });
        remote_button.add_controller(click);
    }
    {
        let motion_target = remote_button.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |_, _, _| {
            motion_target.add_css_class("is-hover");
        });

        let motion_target = remote_button.clone();
        motion.connect_leave(move |_| {
            motion_target.remove_css_class("is-hover");
        });
        remote_button.add_controller(motion);
    }
    {
        let db = db.clone();
        let remote_button = remote_button.clone();
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
            if db.remote_mode_enabled() {
                remote_button.add_css_class("is-on");
            } else {
                remote_button.remove_css_class("is-on");
            }
            gtk::glib::ControlFlow::Continue
        });
    }
    right_box.append(&remote_button);

    let settings_button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    settings_button.add_css_class("bottom-icon-button");
    settings_button.add_css_class("bottom-settings-button");
    settings_button.set_width_request(18);
    settings_button.set_height_request(18);
    settings_button.set_halign(gtk::Align::Center);
    settings_button.set_valign(gtk::Align::Center);
    settings_button.set_can_focus(false);
    let settings_icon = gtk::Image::from_icon_name("preferences-system-symbolic");
    settings_icon.set_pixel_size(15);
    settings_icon.add_css_class("bottom-icon-image");
    settings_button.append(&settings_icon);
    settings_button.set_tooltip_text(Some("Settings"));
    {
        let db = db.clone();
        let manager = manager.clone();
        let click_target = settings_button.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_pressed(move |_, _, _, _| {
            click_target.add_css_class("is-active");
        });

        let click_target = settings_button.clone();
        click.connect_released(move |_, _, _, _| {
            click_target.remove_css_class("is-active");
            let parent = click_target
                .root()
                .and_then(|root| root.downcast::<gtk::Window>().ok());
            profile_settings_dialog::show(parent.as_ref(), db.clone(), manager.clone());
        });
        settings_button.add_controller(click);
    }
    {
        let motion_target = settings_button.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |_, _, _| {
            motion_target.add_css_class("is-hover");
        });

        let motion_target = settings_button.clone();
        motion.connect_leave(move |_| {
            motion_target.remove_css_class("is-hover");
        });
        settings_button.add_controller(motion);
    }
    right_box.append(&settings_button);

    let style_button = style_picker::create_style_picker_button(db);
    style_button.add_css_class("bottom-icon-button");
    style_button.add_css_class("bottom-style-button");
    right_box.append(&style_button);

    bottom_bar.set_end_widget(Some(&right_box));
    bottom_bar
}
