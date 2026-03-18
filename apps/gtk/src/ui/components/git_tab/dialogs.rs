use adw::prelude::*;
use std::rc::Rc;

use super::model::{BranchAction, InitRepoOptions, PushCredentials, UpstreamOptions};

pub(super) fn open_upstream_dialog(
    parent: Option<gtk::Window>,
    workspace_root: &str,
    default_remote: &str,
    default_remote_url: &str,
    default_branch: &str,
    on_submit: Rc<dyn Fn(UpstreamOptions)>,
) {
    let dialog = gtk::Window::builder()
        .title("Configure Upstream")
        .default_width(520)
        .modal(true)
        .build();
    if let Some(parent) = parent.as_ref() {
        dialog.set_transient_for(Some(parent));
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let intro = gtk::Label::new(Some(
        "Set the tracking remote for this branch. Add a remote URL if this remote does not exist yet.",
    ));
    intro.set_wrap(true);
    intro.set_xalign(0.0);

    let workspace = gtk::Label::new(Some(&format!("Workspace: {}", workspace_root)));
    workspace.add_css_class("dim-label");
    workspace.set_wrap(true);
    workspace.set_xalign(0.0);

    let remote_name_entry = gtk::Entry::new();
    remote_name_entry.set_placeholder_text(Some("Remote name"));
    remote_name_entry.set_text(default_remote);

    let remote_url_entry = gtk::Entry::new();
    remote_url_entry.set_placeholder_text(Some("Remote URL (optional if remote already exists)"));
    remote_url_entry.set_text(default_remote_url);

    let branch_entry = gtk::Entry::new();
    branch_entry.set_placeholder_text(Some("Branch"));
    branch_entry.set_text(default_branch);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);

    let cancel = gtk::Button::with_label("Cancel");
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }

    let submit = gtk::Button::with_label("Save Upstream");
    submit.add_css_class("suggested-action");
    {
        let dialog = dialog.clone();
        let remote_name_entry = remote_name_entry.clone();
        let remote_url_entry = remote_url_entry.clone();
        let branch_entry = branch_entry.clone();
        let on_submit = on_submit.clone();
        submit.connect_clicked(move |_| {
            let remote_name = remote_name_entry.text().trim().to_string();
            let remote_url = remote_url_entry.text().trim().to_string();
            let branch_name = branch_entry.text().trim().to_string();

            if remote_name.is_empty() || branch_name.is_empty() {
                return;
            }

            on_submit(UpstreamOptions {
                remote_name,
                remote_url,
                branch_name,
            });
            dialog.close();
        });
    }

    actions.append(&cancel);
    actions.append(&submit);

    root.append(&intro);
    root.append(&workspace);
    root.append(&remote_name_entry);
    root.append(&remote_url_entry);
    root.append(&branch_entry);
    root.append(&actions);

    dialog.set_child(Some(&root));
    dialog.present();
}

pub(super) fn open_init_repository_dialog(
    parent: Option<gtk::Window>,
    workspace_root: &str,
    on_submit: Rc<dyn Fn(InitRepoOptions)>,
) {
    let dialog = gtk::Window::builder()
        .title("Initialize Git Repository")
        .default_width(520)
        .modal(true)
        .build();
    if let Some(parent) = parent.as_ref() {
        dialog.set_transient_for(Some(parent));
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let intro = gtk::Label::new(Some(
        "No Git repository was found for this workspace. Initialize one with recommended defaults.",
    ));
    intro.set_wrap(true);
    intro.set_xalign(0.0);

    let workspace = gtk::Label::new(Some(&format!("Workspace: {}", workspace_root)));
    workspace.add_css_class("dim-label");
    workspace.set_wrap(true);
    workspace.set_xalign(0.0);

    let branch_entry = gtk::Entry::new();
    branch_entry.set_placeholder_text(Some("Default branch"));
    branch_entry.set_text("main");

    let gitignore_toggle = gtk::CheckButton::with_label("Create .gitignore if missing");
    gitignore_toggle.set_active(true);

    let initial_commit_toggle = gtk::CheckButton::with_label("Create initial commit");
    initial_commit_toggle.set_active(true);

    let commit_message_entry = gtk::Entry::new();
    commit_message_entry.set_placeholder_text(Some("Initial commit message"));
    commit_message_entry.set_text("chore: initial commit");

    {
        let commit_message_entry = commit_message_entry.clone();
        initial_commit_toggle.connect_toggled(move |toggle| {
            commit_message_entry.set_sensitive(toggle.is_active());
        });
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);

    let cancel = gtk::Button::with_label("Cancel");
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }

    let submit = gtk::Button::with_label("Initialize");
    submit.add_css_class("suggested-action");
    {
        let dialog = dialog.clone();
        let branch_entry = branch_entry.clone();
        let gitignore_toggle = gitignore_toggle.clone();
        let initial_commit_toggle = initial_commit_toggle.clone();
        let commit_message_entry = commit_message_entry.clone();
        let on_submit = on_submit.clone();
        submit.connect_clicked(move |_| {
            let branch = branch_entry.text().trim().to_string();
            if branch.is_empty() {
                return;
            }

            let create_initial_commit = initial_commit_toggle.is_active();
            let commit_message = commit_message_entry.text().trim().to_string();
            if create_initial_commit && commit_message.is_empty() {
                return;
            }

            on_submit(InitRepoOptions {
                branch,
                create_gitignore: gitignore_toggle.is_active(),
                create_initial_commit,
                commit_message,
            });
            dialog.close();
        });
    }

    actions.append(&cancel);
    actions.append(&submit);

    root.append(&intro);
    root.append(&workspace);
    root.append(&branch_entry);
    root.append(&gitignore_toggle);
    root.append(&initial_commit_toggle);
    root.append(&commit_message_entry);
    root.append(&actions);

    dialog.set_child(Some(&root));
    dialog.present();
}

pub(super) fn open_branch_manager_popover(
    anchor: &gtk::Button,
    current_branch: &str,
    branches: &[String],
    on_submit: Rc<dyn Fn(BranchAction)>,
) {
    let popover = gtk::Popover::new();
    popover.set_has_arrow(true);
    popover.set_autohide(true);
    popover.set_position(gtk::PositionType::Bottom);
    popover.set_parent(anchor);
    popover.connect_closed(|p| {
        if p.parent().is_some() {
            p.unparent();
        }
    });

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let current = gtk::Label::new(Some(&format!("Current branch: {}", current_branch)));
    current.add_css_class("dim-label");
    current.set_xalign(0.0);

    let mut branch_options = vec!["New branch…".to_string()];
    branch_options.extend(
        branches
            .iter()
            .map(|branch| branch.trim().to_string())
            .filter(|branch| !branch.is_empty()),
    );
    branch_options.sort_by(|a, b| {
        if a == "New branch…" {
            std::cmp::Ordering::Less
        } else if b == "New branch…" {
            std::cmp::Ordering::Greater
        } else {
            a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase())
        }
    });
    branch_options.dedup();

    let branch_model = gtk::StringList::new(&[]);
    for option in &branch_options {
        branch_model.append(option);
    }

    let branch_dropdown = gtk::DropDown::new(Some(branch_model.clone()), None::<&gtk::Expression>);
    branch_dropdown.set_hexpand(true);
    if let Some(index) = branch_options
        .iter()
        .position(|branch| branch == current_branch)
    {
        branch_dropdown.set_selected(index as u32);
    }

    let create_entry = gtk::Entry::new();
    create_entry.set_placeholder_text(Some("New branch name"));
    create_entry.set_visible(branch_dropdown.selected() == 0);

    let branch_options = Rc::new(branch_options);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);

    let cancel = gtk::Button::with_label("Cancel");
    {
        let popover = popover.clone();
        cancel.connect_clicked(move |_| popover.popdown());
    }

    let submit_button = gtk::Button::with_label(if branch_dropdown.selected() == 0 {
        "Create & Switch"
    } else {
        "Switch"
    });
    submit_button.add_css_class("suggested-action");

    {
        let popover = popover.clone();
        let branch_dropdown = branch_dropdown.clone();
        let branch_options = branch_options.clone();
        let create_entry = create_entry.clone();
        let on_submit = on_submit.clone();
        submit_button.connect_clicked(move |_| {
            let selected_index = branch_dropdown.selected() as usize;
            if selected_index == 0 {
                let branch = create_entry.text().trim().to_string();
                if branch.is_empty() {
                    return;
                }
                on_submit(BranchAction::Create(branch));
            } else if let Some(branch) = branch_options.get(selected_index).cloned() {
                if branch.is_empty() {
                    return;
                }
                on_submit(BranchAction::Switch(branch));
            }
            popover.popdown();
        });
    }

    {
        let branch_dropdown = branch_dropdown.clone();
        let create_entry = create_entry.clone();
        let submit_button = submit_button.clone();
        branch_dropdown.connect_selected_notify(move |dropdown| {
            let is_new = dropdown.selected() == 0;
            create_entry.set_visible(is_new);
            submit_button.set_label(if is_new { "Create & Switch" } else { "Switch" });
        });
    }

    actions.append(&cancel);
    actions.append(&submit_button);

    root.append(&current);
    root.append(&branch_dropdown);
    root.append(&create_entry);
    root.append(&actions);

    popover.set_child(Some(&root));
    popover.popup();
}

pub(super) fn friendly_auth_message(error_text: &str) -> String {
    let lower = error_text.to_ascii_lowercase();
    if lower.contains("authentication failed") || lower.contains("access denied") {
        return "Authentication failed. Check your username and personal access token, then try again."
            .to_string();
    }
    if lower.contains("could not read username") || lower.contains("terminal prompts disabled") {
        return "This push requires authentication. Enter your Git username and personal access token."
            .to_string();
    }
    if lower.contains("http") && lower.contains("401") {
        return "Remote rejected credentials (401 Unauthorized). Verify your token scope and retry."
            .to_string();
    }
    "Authentication is required to push. Enter your Git username and personal access token."
        .to_string()
}

pub(super) fn open_push_credentials_dialog(
    parent: Option<gtk::Window>,
    initial_error: &str,
    on_submit: Rc<dyn Fn(PushCredentials)>,
) {
    let dialog = gtk::Window::builder()
        .title("Authentication Required")
        .default_width(420)
        .modal(true)
        .build();
    if let Some(parent) = parent.as_ref() {
        dialog.set_transient_for(Some(parent));
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let info = gtk::Label::new(Some(&friendly_auth_message(initial_error)));
    info.set_wrap(true);
    info.set_xalign(0.0);

    let username = gtk::Entry::new();
    username.set_placeholder_text(Some("Git username"));

    let password = gtk::PasswordEntry::new();
    password.set_placeholder_text(Some("Personal access token (or password)"));

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);

    let cancel = gtk::Button::with_label("Cancel");
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }

    let submit = gtk::Button::with_label("Push Again");
    submit.add_css_class("suggested-action");
    submit.set_receives_default(true);

    let submit_action: Rc<dyn Fn()> = {
        let dialog = dialog.clone();
        let username = username.clone();
        let password = password.clone();
        let on_submit = on_submit.clone();
        Rc::new(move || {
            let user = username.text().trim().to_string();
            let pass = password.text().to_string();
            if user.is_empty() || pass.is_empty() {
                return;
            }

            on_submit(PushCredentials {
                username: user,
                password: pass,
            });
            dialog.close();
        })
    };

    {
        let submit_action = submit_action.clone();
        submit.connect_clicked(move |_| {
            submit_action();
        });
    }

    {
        let submit_action = submit_action.clone();
        username.connect_activate(move |_| {
            submit_action();
        });
    }

    {
        let submit_action = submit_action.clone();
        password.connect_activate(move |_| {
            submit_action();
        });
    }

    dialog.set_default_widget(Some(&submit));

    actions.append(&cancel);
    actions.append(&submit);

    root.append(&info);
    root.append(&username);
    root.append(&password);
    root.append(&actions);

    dialog.set_child(Some(&root));
    dialog.present();
}

pub(super) fn open_git_feedback_dialog(parent: Option<gtk::Window>, title: &str, message: &str) {
    let dialog = gtk::Window::builder()
        .title(title)
        .default_width(460)
        .modal(true)
        .build();
    if let Some(parent) = parent.as_ref() {
        dialog.set_transient_for(Some(parent));
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let label = gtk::Label::new(Some(message));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);

    let close = gtk::Button::with_label("OK");
    close.add_css_class("suggested-action");
    {
        let dialog = dialog.clone();
        close.connect_clicked(move |_| dialog.close());
    }

    actions.append(&close);
    root.append(&label);
    root.append(&actions);

    dialog.set_child(Some(&root));
    dialog.present();
}
