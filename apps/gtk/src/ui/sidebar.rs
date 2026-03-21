use crate::services::app::CodexProfileManager;
use crate::services::app::chat::{AppDb, WorkspaceWithThreads};
use crate::ui::components::thread_list::ThreadList;
use crate::ui::widget_tree;
use adw::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

static ONBOARDING_GUIDE_STEP: AtomicU8 = AtomicU8::new(0);
pub(crate) const SIDEBAR_WIDTH: i32 = 180;

pub(crate) fn set_onboarding_guide_step(step: u8) {
    ONBOARDING_GUIDE_STEP.store(step, Ordering::Relaxed);
}

fn onboarding_guide_step() -> u8 {
    ONBOARDING_GUIDE_STEP.load(Ordering::Relaxed)
}

fn truncate_label_text(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return ".".repeat(max_chars);
    }
    let prefix: String = value.chars().take(max_chars - 3).collect();
    format!("{prefix}...")
}

fn build_onboarding_mock_workspaces() -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
    root.add_css_class("onboarding-mock-workspaces");

    let rows = vec![
        (
            "website-redesign",
            vec!["Hero copy refresh", "Fix settings submit"],
        ),
        (
            "mobile-app",
            vec!["New login flow", "Polish profile screen"],
        ),
    ];

    for (workspace_name, threads) in rows {
        let workspace_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        workspace_box.add_css_class("workspace-container");
        workspace_box.add_css_class("onboarding-mock-workspace");

        let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        header_row.set_margin_start(2);
        header_row.set_margin_end(2);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.add_css_class("workspace-header");
        header.set_hexpand(true);
        header.set_margin_start(4);
        header.set_margin_end(4);
        header.set_margin_top(1);
        header.set_margin_bottom(1);

        let chevron = gtk::Image::from_icon_name("pan-down-symbolic");
        chevron.set_pixel_size(10);
        header.append(&chevron);

        let title = gtk::Label::new(Some(&truncate_label_text(workspace_name, 30)));
        title.add_css_class("workspace-name");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_max_width_chars(30);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        header.append(&title);

        let add_thread_button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        add_thread_button.add_css_class("workspace-add-thread-button");
        add_thread_button.add_css_class("onboarding-mock-add-thread");
        add_thread_button.set_opacity(1.0);
        add_thread_button.set_width_request(18);
        add_thread_button.set_height_request(18);
        let add_thread_icon = gtk::Image::from_icon_name("chat-new-symbolic");
        add_thread_icon.set_pixel_size(14);
        add_thread_button.append(&add_thread_icon);

        header_row.append(&header);
        header_row.append(&add_thread_button);
        workspace_box.append(&header_row);

        let thread_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        thread_box.add_css_class("onboarding-mock-thread-list");
        thread_box.set_margin_start(16);
        thread_box.set_margin_end(4);
        thread_box.set_margin_bottom(4);
        for thread_name in threads {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row.add_css_class("onboarding-mock-thread-row");
            let label = gtk::Label::new(Some(thread_name));
            label.add_css_class("thread-title");
            label.set_xalign(0.0);
            row.append(&label);
            thread_box.append(&row);
        }
        workspace_box.append(&thread_box);
        root.append(&workspace_box);
    }

    root
}

pub fn build_sidebar(
    window: &adw::ApplicationWindow,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> adw::ToolbarView {
    let toolbar = adw::ToolbarView::new();
    toolbar.set_top_bar_style(adw::ToolbarStyle::Flat);
    toolbar.add_css_class("sidebar-content");
    toolbar.set_hexpand(false);
    toolbar.set_halign(gtk::Align::Start);
    toolbar.set_width_request(SIDEBAR_WIDTH);
    toolbar.set_size_request(SIDEBAR_WIDTH, -1);

    let header = adw::HeaderBar::new();
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);

    let title_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    title_box.add_css_class("app-brand");
    title_box.set_halign(gtk::Align::Start);
    title_box.set_valign(gtk::Align::Center);

    let title = gtk::Label::new(Some("Enzim Coder"));
    title.add_css_class("app-brand-title");
    title.set_xalign(0.0);
    title.set_valign(gtk::Align::Center);

    let badge = gtk::Label::new(Some("PREVIEW"));
    badge.add_css_class("app-brand-badge");
    badge.add_css_class("preview-badge");
    badge.set_valign(gtk::Align::Center);

    title_box.append(&title);
    title_box.append(&badge);
    header.set_title_widget(Some(&title_box));
    toolbar.add_top_bar(&header);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.add_css_class("sidebar-body");
    root.set_hexpand(false);
    root.set_halign(gtk::Align::Fill);

    let workspaces_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    workspaces_header.set_margin_start(4);
    workspaces_header.set_margin_end(2);
    workspaces_header.set_margin_top(2);
    workspaces_header.set_margin_bottom(0);

    let workspaces_label = gtk::Label::new(Some("Workspaces"));
    workspaces_label.add_css_class("section-title");
    workspaces_label.set_xalign(0.0);
    workspaces_label.set_hexpand(true);
    workspaces_header.append(&workspaces_label);
    root.append(&workspaces_header);

    let list_container = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list_container.add_css_class("sidebar-frame");
    list_container.set_hexpand(false);
    list_container.set_halign(gtk::Align::Fill);
    let onboarding_mock_box = build_onboarding_mock_workspaces();

    let workspace_rows = db.list_workspaces_with_threads().unwrap_or_default();
    for workspace in workspace_rows {
        list_container.append(&build_workspace(
            db.clone(),
            manager.clone(),
            active_thread_id.clone(),
            active_workspace_path.clone(),
            workspace,
            true,
        ));
    }

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .vexpand(true)
        .child(&list_container)
        .build();
    scroll.add_css_class("sidebar-scroll");
    scroll.set_propagate_natural_width(false);

    let scroll_overlay = gtk::Overlay::new();
    scroll_overlay.add_css_class("sidebar-scroll-overlay");
    scroll_overlay.set_vexpand(true);
    scroll_overlay.set_child(Some(&scroll));

    let top_fade = gtk::Box::new(gtk::Orientation::Vertical, 0);
    top_fade.add_css_class("sidebar-scroll-fade");
    top_fade.add_css_class("sidebar-scroll-fade-top");
    top_fade.set_hexpand(true);
    top_fade.set_halign(gtk::Align::Fill);
    top_fade.set_valign(gtk::Align::Start);
    top_fade.set_height_request(18);
    top_fade.set_can_target(false);
    top_fade.set_visible(false);
    scroll_overlay.add_overlay(&top_fade);

    let bottom_fade = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bottom_fade.add_css_class("sidebar-scroll-fade");
    bottom_fade.add_css_class("sidebar-scroll-fade-bottom");
    bottom_fade.set_hexpand(true);
    bottom_fade.set_halign(gtk::Align::Fill);
    bottom_fade.set_valign(gtk::Align::End);
    bottom_fade.set_height_request(18);
    bottom_fade.set_can_target(false);
    bottom_fade.set_visible(false);
    scroll_overlay.add_overlay(&bottom_fade);

    let update_scroll_fades: Rc<dyn Fn()> = {
        let scroll = scroll.clone();
        let top_fade = top_fade.clone();
        let bottom_fade = bottom_fade.clone();
        Rc::new(move || {
            let adj = scroll.vadjustment();
            let max_value = (adj.upper() - adj.page_size()).max(0.0);
            let has_overflow = max_value > 0.5;
            let value = adj.value();
            top_fade.set_visible(has_overflow && value > 0.5);
            bottom_fade.set_visible(has_overflow && value < (max_value - 0.5));
        })
    };
    {
        let update_scroll_fades = update_scroll_fades.clone();
        scroll
            .vadjustment()
            .connect_value_changed(move |_| update_scroll_fades());
    }
    {
        let update_scroll_fades = update_scroll_fades.clone();
        scroll
            .vadjustment()
            .connect_changed(move |_| update_scroll_fades());
    }
    {
        let update_scroll_fades = update_scroll_fades.clone();
        scroll_overlay.connect_map(move |_| update_scroll_fades());
    }
    root.append(&scroll_overlay);

    let add_project = gtk::Button::new();
    add_project.add_css_class("sidebar-action-button");
    add_project.add_css_class("sidebar-add-workspace-button");
    let add_project_label = gtk::Label::new(Some("Add Workspace"));
    add_project.set_child(Some(&add_project_label));
    let no_workspaces = db
        .list_workspaces_with_threads()
        .map(|items| items.is_empty())
        .unwrap_or(false);
    if no_workspaces {
        add_project.add_css_class("workspace-attention");
    }
    let no_workspaces_state = Rc::new(RefCell::new(no_workspaces));
    add_project.set_margin_top(4);
    add_project.set_margin_bottom(4);
    add_project.set_margin_start(4);
    add_project.set_margin_end(4);

    {
        let db = db.clone();
        let manager = manager.clone();
        let active_thread_id = active_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        let window = window.clone();
        let list_container = list_container.clone();
        add_project.connect_clicked(move |_| {
            open_workspace_picker(
                &window,
                db.clone(),
                manager.clone(),
                active_thread_id.clone(),
                active_workspace_path.clone(),
                list_container.clone(),
            );
        });
    }
    root.append(&add_project);
    {
        let db = db.clone();
        let add_project = add_project.clone();
        let no_workspaces_state = no_workspaces_state.clone();
        let list_container = list_container.clone();
        let onboarding_mock_box = onboarding_mock_box.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(280), move || {
            if add_project.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let onboarding_step = onboarding_guide_step();
            let show_mock = onboarding_step == 1;
            if show_mock && onboarding_mock_box.parent().is_none() {
                list_container.append(&onboarding_mock_box);
            } else if !show_mock {
                if let Some(parent) = onboarding_mock_box
                    .parent()
                    .and_then(|node| node.downcast::<gtk::Box>().ok())
                {
                    parent.remove(&onboarding_mock_box);
                }
            }
            let no_workspaces = db
                .list_workspaces_with_threads()
                .map(|items| items.is_empty())
                .unwrap_or(false);
            no_workspaces_state.replace(no_workspaces);
            if no_workspaces {
                add_project.add_css_class("workspace-attention");
            } else {
                add_project.remove_css_class("workspace-attention");
            }
            gtk::glib::ControlFlow::Continue
        });
    }
    {
        let no_workspaces_state = no_workspaces_state.clone();
        let add_project_label = add_project_label.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(33), move || {
            if add_project_label.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let should_wave = *no_workspaces_state.borrow()
                && !crate::ui::components::welcome_overlay::is_visible();
            if should_wave {
                let phase = gtk::glib::monotonic_time() as f64 / 90_000.0;
                add_project_label.set_use_markup(true);
                add_project_label.set_markup(
                    crate::ui::components::chat::sidebar_wave_status_markup("Add Workspace", phase)
                        .as_str(),
                );
            } else {
                add_project_label.set_use_markup(false);
                add_project_label.set_text("Add Workspace");
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    toolbar.set_content(Some(&root));
    toolbar
}

fn build_workspace(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
    workspace: WorkspaceWithThreads,
    expanded: bool,
) -> gtk::Box {
    let workspace_id = workspace.workspace.id;
    let workspace_name = workspace.workspace.name.clone();
    let workspace_path = workspace.workspace.path.clone();
    let workspace_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    workspace_box.add_css_class("workspace-container");
    workspace_box.set_hexpand(false);
    workspace_box.set_halign(gtk::Align::Fill);

    let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    header_row.set_margin_start(2);
    header_row.set_margin_end(2);
    header_row.set_hexpand(true);

    let header_click_area = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header_click_area.add_css_class("workspace-header");
    header_click_area.set_hexpand(true);
    header_click_area.set_halign(gtk::Align::Fill);

    let header_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header_content.set_hexpand(true);
    header_content.set_halign(gtk::Align::Fill);
    header_content.set_margin_start(4);
    header_content.set_margin_end(4);
    header_content.set_margin_top(1);
    header_content.set_margin_bottom(1);

    let chevron = gtk::Image::from_icon_name(if expanded {
        "pan-down-symbolic"
    } else {
        "pan-end-symbolic"
    });
    chevron.set_pixel_size(10);
    header_content.append(&chevron);

    let workspace_label = gtk::Label::new(Some(&truncate_label_text(&workspace_name, 30)));
    workspace_label.set_xalign(0.0);
    workspace_label.set_hexpand(true);
    workspace_label.set_halign(gtk::Align::Fill);
    workspace_label.set_width_chars(1);
    workspace_label.set_max_width_chars(30);
    workspace_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    workspace_label.add_css_class("workspace-name");
    workspace_label.set_tooltip_text(Some(&workspace_path));
    header_content.append(&workspace_label);

    header_click_area.append(&header_content);

    let add_thread_button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    add_thread_button.add_css_class("workspace-add-thread-button");
    let add_thread_icon = gtk::Image::from_icon_name("chat-new-symbolic");
    add_thread_icon.set_pixel_size(14);
    add_thread_button.append(&add_thread_icon);

    add_thread_button.set_width_request(18);
    add_thread_button.set_height_request(18);
    add_thread_button.set_halign(gtk::Align::Center);
    add_thread_button.set_valign(gtk::Align::Center);
    add_thread_button.set_can_focus(false);
    add_thread_button.set_sensitive(true);
    add_thread_button.set_tooltip_text(Some("Start new thread"));
    add_thread_button.set_opacity(0.0);

    let thread_settings_button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    thread_settings_button.add_css_class("workspace-thread-settings-button");
    let thread_settings_icon = gtk::Image::from_icon_name("cogged-wheel-big-symbolic");
    thread_settings_icon.set_pixel_size(13);
    thread_settings_button.append(&thread_settings_icon);
    thread_settings_button.set_width_request(18);
    thread_settings_button.set_height_request(18);
    thread_settings_button.set_halign(gtk::Align::Center);
    thread_settings_button.set_valign(gtk::Align::Center);
    thread_settings_button.set_can_focus(false);
    thread_settings_button.set_tooltip_text(Some("Thread settings"));
    thread_settings_button.set_opacity(0.0);

    header_row.append(&header_click_area);
    header_row.append(&thread_settings_button);
    header_row.append(&add_thread_button);
    workspace_box.append(&header_row);

    {
        let add_thread_button = add_thread_button.clone();
        let thread_settings_button = thread_settings_button.clone();
        let motion_controller = gtk::EventControllerMotion::new();
        motion_controller.connect_enter(move |_, _, _| {
            add_thread_button.set_opacity(1.0);
            thread_settings_button.set_opacity(1.0);
        });
        header_row.add_controller(motion_controller);
    }

    {
        let add_thread_button = add_thread_button.clone();
        let thread_settings_button = thread_settings_button.clone();
        let motion_controller = gtk::EventControllerMotion::new();
        motion_controller.connect_leave(move |_| {
            add_thread_button.set_opacity(0.0);
            thread_settings_button.set_opacity(0.0);
        });
        header_row.add_controller(motion_controller);
    }

    let thread_list = ThreadList::new(
        db.clone(),
        manager.clone(),
        active_thread_id.clone(),
        active_workspace_path.clone(),
        workspace_path.clone(),
        &workspace.threads,
        expanded,
    );
    workspace_box.append(thread_list.widget());

    let open_settings_action: Rc<dyn Fn()> = {
        let workspace_name = workspace_name.clone();
        let workspace_path = workspace_path.clone();
        let header_row = header_row.clone();
        Rc::new(move || {
            let parent = header_row
                .root()
                .and_then(|root| root.downcast::<gtk::Window>().ok());
            crate::ui::components::thread_settings_dialog::show(
                parent.as_ref(),
                &workspace_name,
                &workspace_path,
            );
        })
    };

    {
        let open_settings_action = open_settings_action.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_released(move |_, _, _, _| {
            (open_settings_action)();
        });
        thread_settings_button.add_controller(click);
    }

    {
        let thread_list = thread_list.clone();
        let chevron = chevron.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        let active_workspace_path = active_workspace_path.clone();
        let workspace_path = workspace_path.clone();
        click.connect_released(move |_, _, _, _| {
            active_workspace_path.replace(Some(workspace_path.clone()));
            let is_visible = thread_list.is_expanded();
            thread_list.set_expanded(!is_visible);
            chevron.set_icon_name(Some(if is_visible {
                "pan-end-symbolic"
            } else {
                "pan-down-symbolic"
            }));
        });
        header_click_area.add_controller(click);
    }

    let start_new_thread_action: Rc<dyn Fn()> = {
        let thread_list = thread_list.clone();
        let chevron = chevron.clone();
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        let workspace_path = workspace_path.clone();
        Rc::new(move || {
            active_workspace_path.replace(Some(workspace_path.clone()));
            if !thread_list.is_expanded() {
                thread_list.set_expanded(true);
                chevron.set_icon_name(Some("pan-down-symbolic"));
            }
            let profiles = db.list_codex_profiles().unwrap_or_default();
            let profile_id = profiles
                .first()
                .map(|profile| profile.id)
                .or_else(|| db.runtime_profile_id().ok().flatten())
                .or_else(|| db.active_profile_id().ok().flatten())
                .unwrap_or(1);

            match db.create_thread_with_remote_identity(
                workspace_id,
                profile_id,
                None,
                "New thread",
                None,
                None,
                None,
            ) {
                Ok(thread) => {
                    let _ = db.set_setting("last_active_thread_id", &thread.id.to_string());
                    let _ = db.set_setting("last_active_workspace_path", &workspace_path);
                    let _ = db.set_setting("pending_profile_thread_id", &thread.id.to_string());
                    active_thread_id.replace(None);
                    let _ = thread_list.append_thread(thread);
                }
                Err(err) => eprintln!("failed to create pending thread: {err}"),
            }
        })
    };

    {
        let start_new_thread_action = start_new_thread_action.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_released(move |_, _, _, _| {
            (start_new_thread_action)();
        });
        add_thread_button.add_controller(click);
    }

    let close_workspace_action: Rc<dyn Fn()> = {
        let db = db.clone();
        let manager = manager.clone();
        let active_thread_id = active_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        let workspace_box = workspace_box.clone();
        let workspace_path = workspace_path.clone();
        Rc::new(move || {
            let threads = db
                .list_threads_for_workspace_all(workspace_id)
                .unwrap_or_default();
            let mut removed_local_thread_ids = HashSet::new();
            let mut removed_remote_thread_ids = Vec::new();
            for thread in &threads {
                removed_local_thread_ids.insert(thread.id);
                if let Some(remote_thread_id) = thread.remote_thread_id() {
                    removed_remote_thread_ids.push(remote_thread_id.to_string());
                }
            }

            workspace_box.set_sensitive(false);

            let (cleanup_tx, cleanup_rx) = mpsc::channel::<Result<(), String>>();
            let workspace_path_for_worker = workspace_path.clone();
            let threads_for_worker = threads.clone();
            thread::spawn(move || {
                let db = AppDb::open_default();
                let mut errors: Vec<String> = Vec::new();

                for thread in threads_for_worker {
                    if let Some(path) = thread
                        .worktree_path
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                    {
                        if let Err(err) =
                            crate::services::app::worktree::stop_worktree_checkout(path)
                        {
                            errors.push(format!(
                                "worktree cleanup failed for thread {}: {err}",
                                thread.id
                            ));
                        }
                    }
                    if let Err(err) = crate::services::app::restore::clear_thread_restore_data(
                        db.as_ref(),
                        thread.id,
                    ) {
                        errors.push(format!(
                            "checkpoint cleanup failed for thread {}: {err}",
                            thread.id
                        ));
                    }
                }

                if let Err(err) = db.delete_workspace(workspace_id) {
                    errors.push(format!(
                        "failed to remove workspace {} from sqlite: {err}",
                        workspace_path_for_worker
                    ));
                }

                if errors.is_empty() {
                    let _ = cleanup_tx.send(Ok(()));
                } else {
                    let _ = cleanup_tx.send(Err(errors.join(" | ")));
                }
            });

            let workspace_box_for_result = workspace_box.clone();
            let workspace_path_for_result = workspace_path.clone();
            let db_for_result = db.clone();
            let manager_for_result = manager.clone();
            let active_thread_id_for_result = active_thread_id.clone();
            let active_workspace_path_for_result = active_workspace_path.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(40), move || {
                match cleanup_rx.try_recv() {
                    Ok(Ok(())) => {
                        for remote_thread_id in &removed_remote_thread_ids {
                            if let Some(client) =
                                manager_for_result.resolve_client_for_thread_id(remote_thread_id)
                            {
                                let client = client.clone();
                                let remote_thread_id_bg = remote_thread_id.clone();
                                thread::spawn(move || {
                                    let _ = client.thread_archive(&remote_thread_id_bg);
                                });
                            }
                            crate::ui::components::thread_list::remove_thread_from_multiview_layout(
                                db_for_result.as_ref(),
                                remote_thread_id,
                            );
                        }

                        if let Some(parent) =
                            workspace_box_for_result.parent().and_downcast::<gtk::Box>()
                        {
                            parent.remove(&workspace_box_for_result);
                        }

                        if active_workspace_path_for_result.borrow().as_deref()
                            == Some(workspace_path_for_result.as_str())
                        {
                            active_workspace_path_for_result.replace(None);
                        }

                        if let Some(active_thread) = active_thread_id_for_result.borrow().clone() {
                            if removed_remote_thread_ids
                                .iter()
                                .any(|id| id == &active_thread)
                            {
                                active_thread_id_for_result.replace(None);
                            }
                        }

                        let selected_local_thread = db_for_result
                            .get_setting("last_active_thread_id")
                            .ok()
                            .flatten()
                            .and_then(|value| value.parse::<i64>().ok());
                        if selected_local_thread
                            .map(|thread_id| removed_local_thread_ids.contains(&thread_id))
                            .unwrap_or(false)
                        {
                            let _ = db_for_result.set_setting("last_active_thread_id", "");
                            let _ = db_for_result.set_setting("pending_profile_thread_id", "");
                        }

                        if db_for_result
                            .get_setting("last_active_workspace_path")
                            .ok()
                            .flatten()
                            .as_deref()
                            == Some(workspace_path_for_result.as_str())
                        {
                            let _ = db_for_result.set_setting("last_active_workspace_path", "");
                        }

                        gtk::glib::ControlFlow::Break
                    }
                    Ok(Err(err)) => {
                        workspace_box_for_result.set_sensitive(true);
                        eprintln!(
                            "failed to close workspace {}: {err}",
                            workspace_path_for_result
                        );
                        gtk::glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        workspace_box_for_result.set_sensitive(true);
                        eprintln!(
                            "failed to close workspace {}: cleanup worker disconnected",
                            workspace_path_for_result
                        );
                        gtk::glib::ControlFlow::Break
                    }
                }
            });
        })
    };

    let workspace_menu = gtk::Popover::new();
    workspace_menu.set_has_arrow(true);
    workspace_menu.set_autohide(true);
    workspace_menu.set_offset(0, 0);
    workspace_menu.set_parent(&header_row);
    workspace_menu.add_css_class("actions-popover");

    let menu_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    menu_box.add_css_class("chat-message-context-menu");
    menu_box.set_margin_start(6);
    menu_box.set_margin_end(6);
    menu_box.set_margin_top(6);
    menu_box.set_margin_bottom(6);

    let new_thread_button = build_workspace_context_item("chat-new-symbolic", "New Thread");
    {
        let workspace_menu = workspace_menu.clone();
        let start_new_thread_action = start_new_thread_action.clone();
        new_thread_button.connect_clicked(move |_| {
            (start_new_thread_action)();
            workspace_menu.popdown();
        });
    }
    menu_box.append(&new_thread_button);

    let settings_button = build_workspace_context_item("preferences-system-symbolic", "Settings");
    {
        let workspace_menu = workspace_menu.clone();
        let open_settings_action = open_settings_action.clone();
        settings_button.connect_clicked(move |_| {
            (open_settings_action)();
            workspace_menu.popdown();
        });
    }
    menu_box.append(&settings_button);

    let close_button = build_workspace_context_item("window-close-symbolic", "Close Workspace");
    {
        let workspace_menu = workspace_menu.clone();
        let close_workspace_action = close_workspace_action.clone();
        close_button.connect_clicked(move |_| {
            (close_workspace_action)();
            workspace_menu.popdown();
        });
    }
    menu_box.append(&close_button);
    workspace_menu.set_child(Some(&menu_box));

    {
        let workspace_menu = workspace_menu.clone();
        let right_click = gtk::GestureClick::builder().button(3).build();
        right_click.connect_pressed(move |_, _, x, y| {
            let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            workspace_menu.set_pointing_to(Some(&rect));
            workspace_menu.popup();
        });
        header_row.add_controller(right_click);
    }

    workspace_box
}

fn open_workspace_picker(
    window: &adw::ApplicationWindow,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
    list_container: gtk::Box,
) {
    let dialog = gtk::FileDialog::builder()
        .title("Choose Workspace Folder")
        .modal(true)
        .build();

    dialog.select_folder(
        Some(window),
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let list_container_widget: gtk::Widget = list_container.clone().upcast();
            let scroll_state = widget_tree::capture_ancestor_vscroll(&list_container_widget);
            let folder = match result {
                Ok(folder) => folder,
                Err(err) => {
                    if err.matches(gtk::gio::IOErrorEnum::Cancelled) {
                        return;
                    }
                    eprintln!("folder selection failed: {err}");
                    return;
                }
            };

            let Some(path) = folder.path() else {
                return;
            };

            match db.add_workspace_from_path(&path) {
                Ok(Some(workspace)) => {
                    let _ = db.set_setting("last_workspace_path", &display_path(&path));
                    let workspace_path = workspace.path.clone();
                    let profiles = db.list_codex_profiles().unwrap_or_default();
                    let default_profile_id = db
                        .runtime_profile_id()
                        .ok()
                        .flatten()
                        .or_else(|| profiles.first().map(|profile| profile.id))
                        .or_else(|| db.active_profile_id().ok().flatten())
                        .unwrap_or(1);
                    let first_thread = match db.create_thread_with_remote_identity(
                        workspace.id,
                        default_profile_id,
                        None,
                        "New thread",
                        None,
                        None,
                        None,
                    ) {
                        Ok(thread) => thread,
                        Err(err) => {
                            eprintln!(
                                "failed to create initial thread for workspace {}: {err}",
                                workspace_path
                            );
                            return;
                        }
                    };

                    let _ = db.set_setting("last_active_thread_id", &first_thread.id.to_string());
                    let _ = db.set_setting("last_active_workspace_path", &workspace_path);
                    active_workspace_path.replace(Some(workspace_path.clone()));
                    let _ =
                        db.set_setting("pending_profile_thread_id", &first_thread.id.to_string());
                    active_thread_id.replace(None);

                    let first_thread_id = first_thread.id;
                    let workspace = WorkspaceWithThreads {
                        workspace,
                        threads: vec![first_thread],
                    };
                    list_container.prepend(&build_workspace(
                        db.clone(),
                        manager.clone(),
                        active_thread_id.clone(),
                        active_workspace_path.clone(),
                        workspace,
                        true,
                    ));
                    let list_container_for_select = list_container.clone();
                    gtk::glib::idle_add_local_once(move || {
                        let widget: gtk::Widget = list_container_for_select.upcast();
                        let _ = widget_tree::select_thread_row(&widget, first_thread_id);
                        if let Some((scroll, value)) = scroll_state {
                            widget_tree::restore_vscroll_position(&scroll, value);
                        }
                    });
                }
                Ok(None) => {}
                Err(err) => eprintln!("failed to add workspace {}: {err}", display_path(&path)),
            }
        },
    );
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn build_workspace_context_item(icon_name: &str, label: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.set_has_frame(false);
    button.add_css_class("app-flat-button");
    button.add_css_class("chat-message-context-item");
    button.set_halign(gtk::Align::Fill);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(13);
    icon.add_css_class("chat-message-context-icon");

    let text = gtk::Label::new(Some(label));
    text.set_xalign(0.0);
    text.add_css_class("chat-message-context-label");

    row.append(&icon);
    row.append(&text);
    button.set_child(Some(&row));
    button
}
