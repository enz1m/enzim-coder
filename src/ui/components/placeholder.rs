use adw::prelude::*;

#[allow(dead_code)]
pub fn build_placeholder_tab(title: &str) -> gtk::Box {
    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content_box.set_margin_start(0);
    content_box.set_margin_end(14);
    content_box.set_margin_top(0);
    content_box.set_margin_bottom(0);
    content_box.set_vexpand(true);

    let frame = gtk::Box::new(gtk::Orientation::Vertical, 8);
    frame.add_css_class("chat-frame");
    frame.set_vexpand(true);
    frame.set_valign(gtk::Align::Fill);
    frame.set_halign(gtk::Align::Fill);

    let status = adw::StatusPage::builder()
        .icon_name("system-run-symbolic")
        .title(title)
        .description("Mockup section")
        .vexpand(true)
        .build();
    frame.append(&status);

    content_box.append(&frame);
    content_box
}
