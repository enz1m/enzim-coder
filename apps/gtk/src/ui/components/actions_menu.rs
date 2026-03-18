use crate::actions::{
    ActionRunSnapshot, SavedWorkspaceAction, action_runner, canonical_workspace_path,
    load_workspace_actions, remove_workspace_action, save_workspace_action,
};
use crate::services::app::chat::AppDb;
use crate::ui::widget_tree;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

fn workspace_display_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(|name| name.to_string())
        .unwrap_or_else(|| path.to_string())
}

fn current_workspace_path(active_workspace_path: &Rc<RefCell<Option<String>>>) -> Option<String> {
    active_workspace_path
        .borrow()
        .clone()
        .and_then(|path| canonical_workspace_path(&path))
}

fn compute_signature(
    workspace_path: Option<&str>,
    saved_actions: &[SavedWorkspaceAction],
    running: &[ActionRunSnapshot],
) -> String {
    let saved_sig = saved_actions
        .iter()
        .map(|item| {
            format!(
                "{}:{}:{}",
                item.id,
                item.title.as_deref().unwrap_or(""),
                item.command
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    let running_sig = running
        .iter()
        .map(|item| {
            format!(
                "{}:{}:{}:{}",
                item.id,
                item.is_running,
                item.status_text,
                item.output.len()
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "{};{};{}",
        workspace_path.unwrap_or(""),
        saved_sig,
        running_sig
    )
}

pub fn build_actions_button(
    db: Rc<AppDb>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
    compact: bool,
) -> gtk::Box {
    let button = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    button.add_css_class("actions-toggle-button");
    button.set_halign(gtk::Align::Center);
    button.set_valign(gtk::Align::Center);
    if compact {
        button.add_css_class("multi-chat-pane-actions-button");
    } else {
        button.add_css_class("topbar-actions-button");
    }
    let icon = gtk::Image::from_icon_name("media-playback-start-symbolic");
    icon.set_pixel_size(14);
    icon.set_hexpand(true);
    icon.set_halign(gtk::Align::Center);
    button.append(&icon);
    button.set_tooltip_text(Some("Actions"));

    let popover = gtk::Popover::new();
    popover.set_has_arrow(true);
    popover.set_autohide(true);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_parent(&button);
    popover.add_css_class("actions-popover");

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_start(10);
    root.set_margin_end(10);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_size_request(420, -1);
    root.add_css_class("actions-popover-root");

    let title_label = gtk::Label::new(Some("Actions"));
    title_label.add_css_class("actions-popover-title");
    title_label.set_xalign(0.0);
    root.append(&title_label);

    let workspace_label = gtk::Label::new(Some("No workspace selected"));
    workspace_label.set_xalign(0.0);
    workspace_label.add_css_class("actions-popover-workspace");
    root.append(&workspace_label);

    let status_label = gtk::Label::new(None);
    status_label.set_xalign(0.0);
    status_label.set_wrap(true);
    status_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status_label.add_css_class("actions-popover-status");
    root.append(&status_label);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    stack.set_transition_duration(140);
    stack.set_hhomogeneous(true);
    stack.set_vhomogeneous(false);
    stack.set_hexpand(true);
    stack.set_vexpand(true);

    let list_page = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let list_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(250)
        .build();
    list_scroll.set_has_frame(false);

    let list_content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let saved_heading = gtk::Label::new(Some("Saved Commands"));
    saved_heading.add_css_class("actions-section-heading");
    saved_heading.set_xalign(0.0);
    list_content.append(&saved_heading);

    let saved_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    list_content.append(&saved_box);

    list_scroll.set_child(Some(&list_content));
    list_page.append(&list_scroll);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.set_halign(gtk::Align::Fill);
    let add_button = gtk::Button::with_label("Add command");
    add_button.add_css_class("app-flat-button");
    add_button.add_css_class("actions-add-button");
    add_button.set_halign(gtk::Align::Start);
    footer.append(&add_button);
    list_page.append(&footer);
    stack.add_named(&list_page, Some("list"));

    let add_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let add_heading = gtk::Label::new(Some("Save Command"));
    add_heading.add_css_class("actions-popover-title");
    add_heading.set_xalign(0.0);
    add_page.append(&add_heading);

    let add_workspace = gtk::Label::new(None);
    add_workspace.set_xalign(0.0);
    add_workspace.add_css_class("actions-popover-workspace");
    add_page.append(&add_workspace);

    let title_entry = gtk::Entry::new();
    title_entry.set_placeholder_text(Some("Optional title"));
    title_entry.add_css_class("actions-input-entry");
    add_page.append(&title_entry);

    let command_entry = gtk::Entry::new();
    command_entry.set_placeholder_text(Some("Command (e.g. npm run dev)"));
    command_entry.add_css_class("actions-input-entry");
    add_page.append(&command_entry);

    let add_status = gtk::Label::new(None);
    add_status.set_xalign(0.0);
    add_status.set_wrap(true);
    add_status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    add_status.add_css_class("actions-popover-status");
    add_page.append(&add_status);

    let add_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    add_spacer.set_vexpand(true);
    add_page.append(&add_spacer);

    let add_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    add_actions.set_valign(gtk::Align::End);
    let cancel_button = gtk::Button::with_label("Cancel");
    cancel_button.add_css_class("app-flat-button");
    cancel_button.add_css_class("actions-add-button");
    let save_button = gtk::Button::with_label("Save");
    save_button.add_css_class("app-flat-button");
    save_button.add_css_class("actions-add-button");
    add_actions.append(&cancel_button);
    add_actions.append(&save_button);
    add_page.append(&add_actions);

    stack.add_named(&add_page, Some("add"));
    stack.set_visible_child_name("list");
    root.append(&stack);

    popover.set_child(Some(&root));

    let last_signature = Rc::new(RefCell::new(String::new()));
    let expanded_output_by_action: Rc<RefCell<HashMap<String, bool>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let refresh_handle: Rc<RefCell<Option<Rc<dyn Fn(bool)>>>> = Rc::new(RefCell::new(None));
    let refresh_fn: Rc<dyn Fn(bool)> = {
        let db = db.clone();
        let active_workspace_path = active_workspace_path.clone();
        let workspace_label = workspace_label.clone();
        let add_workspace = add_workspace.clone();
        let status_label = status_label.clone();
        let saved_box = saved_box.clone();
        let last_signature = last_signature.clone();
        let expanded_output_by_action = expanded_output_by_action.clone();
        let refresh_handle_for_refresh = refresh_handle.clone();
        Rc::new(move |force: bool| {
            let workspace = current_workspace_path(&active_workspace_path);
            if let Some(path) = workspace.as_deref() {
                workspace_label.set_text(&format!("Workspace: {}", workspace_display_name(path)));
                add_workspace.set_text(&format!("Workspace: {}", workspace_display_name(path)));
            } else {
                workspace_label.set_text("No workspace selected");
                add_workspace.set_text("No workspace selected");
            }

            let saved_actions = workspace
                .as_deref()
                .map(|path| load_workspace_actions(db.as_ref(), path))
                .unwrap_or_default();
            let runs = workspace
                .as_deref()
                .map(|path| action_runner().list_for_workspace(path))
                .unwrap_or_default();

            let signature = compute_signature(workspace.as_deref(), &saved_actions, &runs);
            if !force && *last_signature.borrow() == signature {
                return;
            }
            last_signature.replace(signature);

            widget_tree::clear_box_children(&saved_box);

            if workspace.is_none() {
                status_label.set_text("Select a thread/workspace first.");
            } else {
                status_label.set_text("");
            }

            let mut latest_runs_by_command: HashMap<String, ActionRunSnapshot> = HashMap::new();
            for run in runs.iter().cloned() {
                latest_runs_by_command
                    .entry(run.command.clone())
                    .or_insert(run);
            }

            if saved_actions.is_empty() {
                let empty = gtk::Label::new(Some("No saved commands yet."));
                empty.set_xalign(0.0);
                empty.add_css_class("dim-label");
                saved_box.append(&empty);
            } else {
                for action in saved_actions {
                    let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
                    card.add_css_class("actions-command-card");
                    card.set_margin_start(2);
                    card.set_margin_end(2);
                    card.set_margin_top(2);
                    card.set_margin_bottom(2);

                    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                    let title = gtk::Label::new(Some(
                        action
                            .title
                            .as_deref()
                            .filter(|t| !t.trim().is_empty())
                            .unwrap_or("Command"),
                    ));
                    title.set_xalign(0.0);
                    title.set_hexpand(true);
                    title.add_css_class("actions-command-title");
                    row.append(&title);

                    if let Some(run) = latest_runs_by_command.get(&action.command) {
                        let status = gtk::Label::new(Some(&run.status_text));
                        status.add_css_class("actions-run-status");
                        row.append(&status);
                    }

                    let delete_button = gtk::Button::new();
                    delete_button.set_has_frame(false);
                    delete_button.set_icon_name("user-trash-symbolic");
                    delete_button.set_tooltip_text(Some("Remove action"));
                    delete_button.add_css_class("app-flat-button");
                    delete_button.add_css_class("actions-run-button");
                    delete_button.add_css_class("actions-delete-button");
                    row.append(&delete_button);

                    let run_button = gtk::Button::with_label("Run");
                    run_button.set_has_frame(false);
                    run_button.add_css_class("app-flat-button");
                    run_button.add_css_class("actions-run-button");
                    row.append(&run_button);
                    card.append(&row);

                    let cmd = gtk::Label::new(Some(&action.command));
                    cmd.set_xalign(0.0);
                    cmd.set_wrap(true);
                    cmd.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    cmd.add_css_class("actions-command-text");
                    card.append(&cmd);

                    {
                        let action = action.clone();
                        let db = db.clone();
                        let active_workspace_path = active_workspace_path.clone();
                        let status_label = status_label.clone();
                        let refresh_handle = refresh_handle_for_refresh.clone();
                        delete_button.connect_clicked(move |_| {
                            let Some(workspace_path) =
                                current_workspace_path(&active_workspace_path)
                            else {
                                status_label.set_text("No active workspace.");
                                return;
                            };
                            match remove_workspace_action(db.as_ref(), &workspace_path, &action.id)
                            {
                                Ok(()) => {
                                    status_label.set_text("Action removed.");
                                    if let Some(refresh) = refresh_handle.borrow().as_ref() {
                                        refresh(true);
                                    }
                                }
                                Err(err) => status_label.set_text(&err),
                            }
                        });
                    }

                    {
                        let action = action.clone();
                        let active_workspace_path = active_workspace_path.clone();
                        let status_label = status_label.clone();
                        let expanded_output_by_action = expanded_output_by_action.clone();
                        let refresh_handle = refresh_handle_for_refresh.clone();
                        run_button.connect_clicked(move |_| {
                            let Some(workspace_path) =
                                current_workspace_path(&active_workspace_path)
                            else {
                                status_label.set_text("No active workspace.");
                                return;
                            };
                            match action_runner().start(
                                &workspace_path,
                                action.title.as_deref(),
                                &action.command,
                            ) {
                                Ok(_) => {
                                    status_label.set_text("Command started.");
                                    expanded_output_by_action
                                        .borrow_mut()
                                        .insert(action.id.clone(), true);
                                    if let Some(refresh) = refresh_handle.borrow().as_ref() {
                                        refresh(true);
                                    }
                                }
                                Err(err) => status_label.set_text(&err),
                            }
                        });
                    }

                    if let Some(run) = latest_runs_by_command.get(&action.command).cloned() {
                        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 6);

                        let output_toggle = gtk::Button::new();
                        output_toggle.add_css_class("app-flat-button");
                        output_toggle.add_css_class("actions-output-toggle");

                        let action_key = action.id.clone();
                        let is_expanded = expanded_output_by_action
                            .borrow()
                            .get(&action_key)
                            .copied()
                            .unwrap_or(run.is_running);
                        output_toggle.set_label(if is_expanded {
                            "Hide output"
                        } else {
                            "Show output"
                        });
                        controls.append(&output_toggle);

                        if run.is_running {
                            let kill = gtk::Button::with_label("Kill");
                            kill.add_css_class("app-flat-button");
                            kill.add_css_class("actions-kill-button");
                            {
                                let status_label = status_label.clone();
                                let run_id = run.id;
                                kill.connect_clicked(move |_| match action_runner().kill(run_id) {
                                    Ok(_) => status_label.set_text("Stopping command..."),
                                    Err(err) => status_label.set_text(&err),
                                });
                            }
                            controls.append(&kill);
                        }
                        card.append(&controls);

                        let revealer = gtk::Revealer::new();
                        revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
                        revealer.set_transition_duration(110);
                        revealer.set_reveal_child(is_expanded);

                        let output_scroll = gtk::ScrolledWindow::builder()
                            .hscrollbar_policy(gtk::PolicyType::Automatic)
                            .vscrollbar_policy(gtk::PolicyType::Automatic)
                            .min_content_height(72)
                            .build();
                        output_scroll.set_has_frame(false);
                        output_scroll.add_css_class("actions-output-scroll");
                        let output_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
                        output_box.add_css_class("actions-output-content");
                        output_box.set_margin_start(7);
                        output_box.set_margin_end(7);
                        output_box.set_margin_top(6);
                        output_box.set_margin_bottom(6);

                        let output_label = gtk::Label::new(Some(if run.output.trim().is_empty() {
                            "(no output yet)"
                        } else {
                            &run.output
                        }));
                        output_label.set_xalign(0.0);
                        output_label.set_yalign(0.0);
                        output_label.set_selectable(true);
                        output_label.set_wrap(false);
                        output_label.add_css_class("actions-output-label");
                        output_box.append(&output_label);
                        output_scroll.set_child(Some(&output_box));
                        revealer.set_child(Some(&output_scroll));
                        card.append(&revealer);

                        {
                            let action_key = action_key.clone();
                            let expanded_output_by_action = expanded_output_by_action.clone();
                            let revealer = revealer.clone();
                            let output_toggle_btn = output_toggle.clone();
                            output_toggle.connect_clicked(move |_| {
                                let next = !revealer.reveals_child();
                                revealer.set_reveal_child(next);
                                output_toggle_btn.set_label(if next {
                                    "Hide output"
                                } else {
                                    "Show output"
                                });
                                expanded_output_by_action
                                    .borrow_mut()
                                    .insert(action_key.clone(), next);
                            });
                        }
                    }

                    saved_box.append(&card);
                }
            }
        })
    };
    refresh_handle.replace(Some(refresh_fn.clone()));

    {
        let stack = stack.clone();
        let add_status = add_status.clone();
        let command_entry = command_entry.clone();
        let title_entry = title_entry.clone();
        let refresh_handle = refresh_handle.clone();
        add_button.connect_clicked(move |_| {
            title_entry.set_text("");
            command_entry.set_text("");
            add_status.set_text("");
            stack.set_visible_child_name("add");
            let focus_entry = title_entry.clone();
            gtk::glib::idle_add_local_once(move || {
                focus_entry.grab_focus();
                focus_entry.set_position(-1);
            });
            let focus_entry = title_entry.clone();
            gtk::glib::timeout_add_local_once(Duration::from_millis(45), move || {
                focus_entry.grab_focus();
                focus_entry.set_position(-1);
            });
            if let Some(refresh) = refresh_handle.borrow().as_ref() {
                refresh(true);
            }
        });
    }

    {
        let stack = stack.clone();
        let command_entry = command_entry.clone();
        title_entry.connect_activate(move |_| {
            stack.set_visible_child_name("add");
            command_entry.grab_focus();
            command_entry.set_position(-1);
        });
    }

    {
        let save_button = save_button.clone();
        command_entry.connect_activate(move |_| {
            save_button.emit_clicked();
        });
    }

    {
        let stack = stack.clone();
        let title_entry = title_entry.clone();
        let command_entry = command_entry.clone();
        let add_status = add_status.clone();
        cancel_button.connect_clicked(move |_| {
            stack.set_visible_child_name("list");
            title_entry.set_text("");
            command_entry.set_text("");
            add_status.set_text("");
        });
    }

    {
        let db = db.clone();
        let active_workspace_path = active_workspace_path.clone();
        let stack = stack.clone();
        let title_entry = title_entry.clone();
        let command_entry = command_entry.clone();
        let add_status = add_status.clone();
        let refresh_handle = refresh_handle.clone();
        save_button.connect_clicked(move |_| {
            let Some(workspace) = current_workspace_path(&active_workspace_path) else {
                add_status.set_text("No active workspace.");
                return;
            };
            let title = title_entry.text().to_string();
            let command = command_entry.text().to_string();
            match save_workspace_action(
                db.as_ref(),
                &workspace,
                if title.trim().is_empty() {
                    None
                } else {
                    Some(title.trim())
                },
                command.trim(),
            ) {
                Ok(_) => {
                    title_entry.set_text("");
                    command_entry.set_text("");
                    add_status.set_text("");
                    stack.set_visible_child_name("list");
                    if let Some(refresh) = refresh_handle.borrow().as_ref() {
                        refresh(true);
                    }
                }
                Err(err) => add_status.set_text(&err),
            }
        });
    }

    {
        let stack = stack.clone();
        let add_status = add_status.clone();
        let title_entry = title_entry.clone();
        let command_entry = command_entry.clone();
        let refresh_handle = refresh_handle.clone();
        popover.connect_visible_notify(move |p| {
            if p.is_visible() {
                stack.set_visible_child_name("list");
                add_status.set_text("");
                title_entry.set_text("");
                command_entry.set_text("");
                if let Some(refresh) = refresh_handle.borrow().as_ref() {
                    refresh(true);
                }
            }
        });
    }

    {
        let popover = popover.clone();
        let refresh_handle = refresh_handle.clone();
        let click = gtk::GestureClick::builder().button(1).build();
        click.connect_released(move |_, _, _, _| {
            if popover.is_visible() {
                popover.popdown();
            } else {
                if let Some(refresh) = refresh_handle.borrow().as_ref() {
                    refresh(true);
                }
                popover.popup();
            }
        });
        button.add_controller(click);
    }

    {
        let button_for_controller = button.clone();
        let button_for_enter = button.clone();
        let button_for_leave = button.clone();
        let popover_for_leave = popover.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_enter(move |_, _, _| {
            button_for_enter.add_css_class("is-active");
        });
        motion.connect_leave(move |_| {
            if !popover_for_leave.is_visible() {
                button_for_leave.remove_css_class("is-active");
            }
        });
        button_for_controller.add_controller(motion);
    }

    {
        let button = button.clone();
        popover.connect_closed(move |_| {
            button.remove_css_class("is-active");
        });
    }

    {
        let popover = popover.clone();
        let stack = stack.clone();
        let refresh_handle = refresh_handle.clone();
        let title_entry = title_entry.clone();
        let command_entry = command_entry.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(260), move || {
            if popover.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            if popover.is_visible() {
                let on_add_page = stack
                    .visible_child_name()
                    .as_deref()
                    .map(|name| name == "add")
                    .unwrap_or(false);
                if on_add_page {
                    return gtk::glib::ControlFlow::Continue;
                }
                if title_entry.has_focus() || command_entry.has_focus() {
                    return gtk::glib::ControlFlow::Continue;
                }
                if let Some(refresh) = refresh_handle.borrow().as_ref() {
                    refresh(false);
                }
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    {
        let button = button.clone();
        popover.connect_visible_notify(move |p| {
            if p.is_visible() {
                button.add_css_class("is-active");
            } else {
                button.remove_css_class("is-active");
            }
        });
    }

    button
}
