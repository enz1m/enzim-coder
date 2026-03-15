use adw::prelude::*;
use gtk::gio;

mod actions;
mod backend;
mod codex_appserver;
mod codex_profiles;
mod data;
mod git_exec;
mod remote;
mod restore;
mod skill_mcp;
mod ui;
mod worktree;

const APP_ID: &str = "dev.enzim.EnzimCoder";
const APP_NAME: &str = "Enzim Coder";

fn main() {
    gtk::glib::set_program_name(Some(APP_ID));
    gtk::glib::set_application_name(APP_NAME);
    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_startup(|_| {
        sourceview5::init();

        let resources_bytes = include_bytes!("../resources.gresource");
        let resource_data = gtk::glib::Bytes::from_static(resources_bytes);
        let resource = gio::Resource::from_data(&resource_data).expect("Failed to load resources");
        gio::resources_register(&resource);

        if let Some(display) = gtk::gdk::Display::default() {
            let icon_theme = gtk::IconTheme::for_display(&display);
            icon_theme.add_resource_path("/com/enzim/coder/icons");
        }
        gtk::Window::set_default_icon_name(APP_ID);

        ui::install_css();
    });

    app.connect_activate(ui::build_ui);
    app.connect_shutdown(|_| {
        actions::shutdown_all_running_actions();
    });

    app.run();
}
