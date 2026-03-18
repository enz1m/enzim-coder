use adw::prelude::*;

pub fn build_top_tabs(stack: &adw::ViewStack) -> gtk::Box {
    let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tabs.add_css_class("top-tabs");
    tabs.set_halign(gtk::Align::Center);
    tabs.set_valign(gtk::Align::Center);

    let chat = tab_button("chat-new-symbolic", "Chat");
    chat.add_css_class("top-tab-chat");
    let git = tab_button("git-symbolic", "Git");
    git.add_css_class("top-tab-git");
    let files = tab_button("folder-silhouette-symbolic", "Files");
    files.add_css_class("top-tab-files");
    let buttons = vec![chat.clone(), git.clone(), files.clone()];
    set_active_tab(&buttons, 0);

    {
        let stack = stack.clone();
        let buttons = buttons.clone();
        chat.connect_clicked(move |_| {
            stack.set_visible_child_name("chat");
            set_active_tab(&buttons, 0);
        });
    }

    {
        let stack = stack.clone();
        let buttons = buttons.clone();
        git.connect_clicked(move |_| {
            stack.set_visible_child_name("git");
            set_active_tab(&buttons, 1);
        });
    }

    {
        let stack = stack.clone();
        let buttons = buttons.clone();
        files.connect_clicked(move |_| {
            stack.set_visible_child_name("files");
            set_active_tab(&buttons, 2);
        });
    }

    tabs.append(&chat);
    tabs.append(&tab_separator());
    tabs.append(&git);
    tabs.append(&tab_separator());
    tabs.append(&files);
    tabs
}

fn set_active_tab(buttons: &[gtk::Button], active_idx: usize) {
    for (idx, button) in buttons.iter().enumerate() {
        if idx == active_idx {
            button.add_css_class("top-tab-active");
        } else {
            button.remove_css_class("top-tab-active");
        }
    }
}

fn tab_button(icon: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("app-flat-button");
    button.add_css_class("top-tab");
    button.set_valign(gtk::Align::Center);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    content.set_valign(gtk::Align::Center);
    let image = gtk::Image::from_icon_name(icon);
    image.set_pixel_size(12);
    image.set_valign(gtk::Align::Center);
    content.append(&image);

    let text = gtk::Label::new(Some(label));
    text.set_valign(gtk::Align::Center);
    content.append(&text);

    button.set_child(Some(&content));
    button
}

fn tab_separator() -> gtk::Label {
    let separator = gtk::Label::new(Some("|"));
    separator.add_css_class("tab-separator");
    separator.set_valign(gtk::Align::Center);
    separator
}
