use adw::prelude::*;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use crate::data::AppDb;
use crate::ui::components::file_preview;

pub fn build_files_tab(
    db: Rc<AppDb>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> gtk::Box {
    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content_box.set_margin_start(0);
    content_box.set_margin_end(14);
    content_box.set_margin_top(0);
    content_box.set_margin_bottom(0);
    content_box.set_vexpand(true);

    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("chat-frame");
    frame.set_vexpand(true);

    let overlay = gtk::Overlay::new();
    overlay.set_vexpand(true);
    frame.append(&overlay);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.add_css_class("file-browser-root");
    root.set_margin_start(10);
    root.set_margin_end(10);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_vexpand(true);
    overlay.set_child(Some(&root));

    let initial_root = resolve_workspace_root(&db, &active_workspace_path);
    let workspace_root = Rc::new(RefCell::new(initial_root.clone()));
    let current_dir = Rc::new(RefCell::new(initial_root));

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("file-browser-header");

    let up_button = gtk::Button::new();
    up_button.add_css_class("app-flat-button");
    up_button.set_icon_name("go-up-symbolic");
    up_button.set_tooltip_text(Some("Go to parent directory"));

    let path_label = gtk::Label::new(None);
    path_label.add_css_class("file-browser-path");
    path_label.set_xalign(0.0);
    path_label.set_hexpand(true);
    path_label.set_ellipsize(gtk::pango::EllipsizeMode::Start);

    header.append(&up_button);
    header.append(&path_label);

    let listbox = gtk::ListBox::new();
    listbox.add_css_class("file-browser-list");
    listbox.set_selection_mode(gtk::SelectionMode::None);
    listbox.set_margin_start(1);
    listbox.set_margin_end(1);
    listbox.set_margin_top(1);
    listbox.set_margin_bottom(1);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(&listbox)
        .build();
    scroll.add_css_class("file-browser-scroll");
    scroll.set_overflow(gtk::Overflow::Hidden);

    root.append(&header);
    root.append(&scroll);

    let last_selected_file = Rc::new(RefCell::new(None::<PathBuf>));

    let open_preview_fn: Rc<dyn Fn(PathBuf)> = {
        Rc::new(move |path: PathBuf| {
            file_preview::open_file_preview(&path);
        })
    };

    {
        let current_dir = current_dir.clone();
        let workspace_root = workspace_root.clone();
        let listbox = listbox.clone();
        let path_label = path_label.clone();
        let up_button_ref = up_button.clone();
        let open_preview_fn = open_preview_fn.clone();
        let last_selected_file = last_selected_file.clone();
        up_button.clone().connect_clicked(move |_| {
            let parent = current_dir.borrow().parent().map(|path| path.to_path_buf());
            if let Some(parent_path) = parent {
                let root = workspace_root.borrow().clone();
                let parent_canonical = canonicalize_or_self(parent_path);
                if is_within_root(&parent_canonical, &root) {
                    current_dir.replace(parent_canonical);
                    populate_file_list(
                        &listbox,
                        &path_label,
                        &up_button_ref,
                        &workspace_root,
                        &current_dir,
                        &open_preview_fn,
                        &last_selected_file,
                    );
                }
            }
        });
    }

    {
        let listbox = listbox.clone();
        let open_preview_fn = open_preview_fn.clone();
        let last_selected_file = last_selected_file.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::space {
                if let Some(path) = last_selected_file.borrow().clone() {
                    (open_preview_fn)(path);
                    gtk::glib::Propagation::Stop
                } else {
                    gtk::glib::Propagation::Proceed
                }
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        listbox.add_controller(key_controller);
    }

    populate_file_list(
        &listbox,
        &path_label,
        &up_button,
        &workspace_root,
        &current_dir,
        &open_preview_fn,
        &last_selected_file,
    );

    {
        let db = db.clone();
        let active_workspace_path = active_workspace_path.clone();
        let workspace_root = workspace_root.clone();
        let current_dir = current_dir.clone();
        let listbox = listbox.clone();
        let path_label = path_label.clone();
        let up_button = up_button.clone();
        let open_preview_fn = open_preview_fn.clone();
        let last_selected_file = last_selected_file.clone();
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
            let desired_root = resolve_workspace_root(&db, &active_workspace_path);
            if *workspace_root.borrow() != desired_root {
                workspace_root.replace(desired_root.clone());
                current_dir.replace(desired_root);
                last_selected_file.replace(None);
                populate_file_list(
                    &listbox,
                    &path_label,
                    &up_button,
                    &workspace_root,
                    &current_dir,
                    &open_preview_fn,
                    &last_selected_file,
                );
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    content_box.append(&frame);
    content_box
}

fn resolve_workspace_root(
    db: &AppDb,
    active_workspace_path: &Rc<RefCell<Option<String>>>,
) -> PathBuf {
    if let Some(path) = active_workspace_path.borrow().clone() {
        let path = PathBuf::from(path);
        if path.exists() {
            return canonicalize_or_self(path);
        }
    }

    if let Ok(Some(saved_path)) = db.get_setting("last_workspace_path") {
        let path = PathBuf::from(saved_path);
        if path.exists() {
            return canonicalize_or_self(path);
        }
    }

    canonicalize_or_self(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn populate_file_list(
    listbox: &gtk::ListBox,
    path_label: &gtk::Label,
    up_button: &gtk::Button,
    workspace_root: &Rc<RefCell<PathBuf>>,
    current_dir: &Rc<RefCell<PathBuf>>,
    open_preview_fn: &Rc<dyn Fn(PathBuf)>,
    last_selected_file: &Rc<RefCell<Option<PathBuf>>>,
) {
    while let Some(child) = listbox.first_child() {
        listbox.remove(&child);
    }

    let path = current_dir.borrow().clone();
    let root = workspace_root.borrow().clone();

    path_label.set_text(&path.to_string_lossy());
    up_button.set_sensitive(path != root);

    let mut entries: Vec<(PathBuf, bool, String, Option<SystemTime>)> = Vec::new();
    if let Ok(read_dir) = fs::read_dir(&path) {
        for item in read_dir.flatten() {
            let item_path = item.path();
            let is_dir = item_path.is_dir();
            let name = item.file_name().to_string_lossy().to_string();
            let modified = item.metadata().ok().and_then(|meta| meta.modified().ok());
            entries.push((item_path, is_dir, name, modified));
        }
    }

    entries.sort_by(|a, b| {
        let dir_cmp = b.1.cmp(&a.1);
        if dir_cmp == std::cmp::Ordering::Equal {
            a.2.to_lowercase().cmp(&b.2.to_lowercase())
        } else {
            dir_cmp
        }
    });

    if entries.is_empty() {
        let empty = gtk::Label::new(Some("No files found"));
        empty.add_css_class("dim-label");
        empty.set_xalign(0.0);
        empty.set_margin_start(8);
        empty.set_margin_end(8);
        empty.set_margin_top(8);
        empty.set_margin_bottom(8);
        listbox.append(&empty);
        return;
    }

    for (item_path, is_dir, name, modified) in entries {
        let button = gtk::Button::new();
        button.set_has_frame(false);
        button.add_css_class("file-browser-row");
        button.set_hexpand(true);
        button.set_halign(gtk::Align::Fill);

        let row_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row_content.set_margin_start(8);
        row_content.set_margin_end(8);
        row_content.set_margin_top(0);
        row_content.set_margin_bottom(0);

        let icon_name = if is_dir {
            "folder-symbolic"
        } else {
            "file-code-symbolic"
        };
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(14);
        icon.add_css_class("file-browser-icon");

        let label = gtk::Label::new(Some(&name));
        label.add_css_class("file-browser-name");
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let modified_label = gtk::Label::new(Some(&format_modified(modified)));
        modified_label.add_css_class("file-browser-modified");
        modified_label.set_xalign(1.0);

        row_content.append(&icon);
        row_content.append(&label);
        row_content.append(&modified_label);
        button.set_child(Some(&row_content));

        if is_dir {
            let listbox = listbox.clone();
            let path_label = path_label.clone();
            let up_button = up_button.clone();
            let current_dir = current_dir.clone();
            let workspace_root = workspace_root.clone();
            let open_preview_fn = open_preview_fn.clone();
            let last_selected_file = last_selected_file.clone();
            button.connect_clicked(move |_| {
                let next_path = canonicalize_or_self(item_path.clone());
                let root = workspace_root.borrow().clone();
                if is_within_root(&next_path, &root) {
                    current_dir.replace(next_path);
                    last_selected_file.replace(None);
                    populate_file_list(
                        &listbox,
                        &path_label,
                        &up_button,
                        &workspace_root,
                        &current_dir,
                        &open_preview_fn,
                        &last_selected_file,
                    );
                }
            });
        } else {
            let open_preview_fn = open_preview_fn.clone();
            let last_selected_file_click = last_selected_file.clone();
            let file_path = item_path.clone();
            button.connect_clicked(move |_| {
                last_selected_file_click.replace(Some(file_path.clone()));
                (open_preview_fn)(file_path.clone());
            });

            let file_path = item_path.clone();
            let last_selected_file_focus = last_selected_file.clone();
            button.connect_has_focus_notify(move |btn| {
                if btn.has_focus() {
                    last_selected_file_focus.replace(Some(file_path.clone()));
                }
            });
        }

        let row = gtk::ListBoxRow::new();
        row.add_css_class("file-browser-item");
        row.set_margin_top(0);
        row.set_margin_bottom(0);
        row.set_selectable(false);
        row.set_activatable(false);
        row.set_child(Some(&button));
        listbox.append(&row);
    }
}

fn format_modified(modified: Option<SystemTime>) -> String {
    let Some(modified_time) = modified else {
        return "—".to_string();
    };

    let now = SystemTime::now();
    let elapsed = match now.duration_since(modified_time) {
        Ok(value) => value,
        Err(_) => return "now".to_string(),
    };
    let seconds = elapsed.as_secs();

    if seconds < 60 {
        "now".to_string()
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else if seconds < 604_800 {
        format!("{}d", seconds / 86_400)
    } else {
        format!("{}w", seconds / 604_800)
    }
}

fn canonicalize_or_self(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

fn is_within_root(candidate: &Path, root: &Path) -> bool {
    candidate == root || candidate.starts_with(root)
}
