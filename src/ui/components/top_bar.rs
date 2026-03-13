use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use super::{actions_menu, appimage_update, skills_mcp_menu, top_tabs};
use crate::codex_profiles::CodexProfileManager;
use crate::data::AppDb;
use crate::ui::settings::{SETTING_MULTIVIEW_ENABLED, is_multiview_enabled};

pub fn build_top_bar(
    stack: Option<&adw::ViewStack>,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> adw::HeaderBar {
    let is_classic_mode = stack.is_some();
    let header = adw::HeaderBar::new();
    header.add_css_class("top-tabs-bar");
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);
    header.set_centering_policy(adw::CenteringPolicy::Strict);

    if let Some(stack) = stack {
        let tabs = top_tabs::build_top_tabs(stack);
        tabs.add_css_class("top-tabs-container");
        tabs.set_valign(gtk::Align::Center);
        header.set_title_widget(Some(&tabs));
    } else {
        let empty_center = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.set_title_widget(Some(&empty_center));
    }

    let multi_toggle = gtk::ToggleButton::new();
    multi_toggle.set_icon_name("app-grid-symbolic");
    multi_toggle.add_css_class("multiview-toggle-button");
    multi_toggle.set_active(is_multiview_enabled(db.as_ref()));
    multi_toggle.set_tooltip_text(Some("Toggle multiview"));
    {
        let db = db.clone();
        multi_toggle.connect_toggled(move |btn| {
            let _ = db.set_setting(
                SETTING_MULTIVIEW_ENABLED,
                if btn.is_active() { "1" } else { "0" },
            );
        });
    }
    {
        let db = db.clone();
        let multi_toggle = multi_toggle.clone();
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
            if multi_toggle.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let target_state = is_multiview_enabled(db.as_ref());
            if multi_toggle.is_active() != target_state {
                multi_toggle.set_active(target_state);
            }
            gtk::glib::ControlFlow::Continue
        });
    }
    let close_button = gtk::Button::new();
    close_button.add_css_class("top-window-close-button");
    close_button.set_widget_name("top-window-close-button");
    close_button.set_has_frame(false);
    close_button.set_focus_on_click(false);
    close_button.set_valign(gtk::Align::Center);
    close_button.set_tooltip_text(Some("Close Window"));
    let close_icon = gtk::Image::from_icon_name("x-symbolic");
    close_icon.add_css_class("top-window-close-icon");
    close_icon.set_widget_name("top-window-close-icon");
    close_icon.set_pixel_size(14);
    close_button.set_child(Some(&close_icon));
    close_button.connect_clicked(move |button| {
        if let Some(root) = button.root() {
            if let Ok(window) = root.downcast::<gtk::Window>() {
                window.close();
            }
        }
    });

    let end_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    let update_button = appimage_update::build_update_button();
    update_button.set_valign(gtk::Align::Center);
    end_box.append(&update_button);

    if is_classic_mode {
        let skills_mcp_button = skills_mcp_menu::build_skills_mcp_button(
            db.clone(),
            manager.clone(),
            active_workspace_path.clone(),
            false,
        );
        skills_mcp_button.set_valign(gtk::Align::Center);
        end_box.append(&skills_mcp_button);

        let actions_button =
            actions_menu::build_actions_button(db.clone(), active_workspace_path, false);
        actions_button.set_valign(gtk::Align::Center);
        end_box.append(&actions_button);
    }
    end_box.append(&multi_toggle);
    end_box.append(&close_button);
    header.pack_end(&end_box);
    header
}
