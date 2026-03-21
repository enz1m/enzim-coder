use adw::prelude::*;
use serde_json::Value;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::services::app::CodexProfileManager;
use crate::services::app::chat::AppDb;
use crate::services::app::runtime::RuntimeClient;
use crate::ui::settings::{
    SETTING_MULTIVIEW_ENABLED, SETTING_PANE_LAYOUT_V1, is_multiview_enabled,
};

use crate::ui::components::{
    bottom_bar::build_bottom_bar, chat, file_browser, git_tab,
    multi_chat::build_multi_chat_content, top_bar,
};

fn parse_thread_drop_payload(raw: &str) -> Option<(Option<String>, Option<String>)> {
    let parsed: Value = serde_json::from_str(raw).ok()?;
    let thread_id = parsed
        .get("threadId")
        .or_else(|| parsed.get("codexThreadId"))
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    let workspace_path = parsed
        .get("workspacePath")
        .and_then(Value::as_str)
        .map(|value| value.to_string());
    Some((thread_id, workspace_path))
}

fn build_multiview_content(
    db: Rc<AppDb>,
    profile_manager: Rc<CodexProfileManager>,
    codex: Option<Arc<RuntimeClient>>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_vexpand(true);
    let top = top_bar::build_top_bar(
        None,
        db.clone(),
        profile_manager.clone(),
        active_workspace_path.clone(),
    );
    root.append(&top);
    root.append(&build_multi_chat_content(
        db,
        profile_manager,
        codex,
        active_thread_id,
        active_workspace_path,
    ));
    root
}

fn build_classic_content(
    db: Rc<AppDb>,
    profile_manager: Rc<CodexProfileManager>,
    codex: Option<Arc<RuntimeClient>>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.set_vexpand(true);

    let stack = adw::ViewStack::new();
    stack.set_widget_name("main-content-view-stack");
    stack.set_vexpand(true);
    stack.add_named(
        &chat::build_chat_tab(
            db.clone(),
            profile_manager.clone(),
            codex.clone(),
            active_thread_id.clone(),
            active_workspace_path.clone(),
        ),
        Some("chat"),
    );
    stack.add_named(
        &git_tab::build_git_tab(db.clone(), active_workspace_path.clone()),
        Some("git"),
    );
    stack.add_named(
        &file_browser::build_files_tab(db.clone(), active_workspace_path.clone()),
        Some("files"),
    );
    stack.set_visible_child_name("chat");

    let top = top_bar::build_top_bar(
        Some(&stack),
        db.clone(),
        profile_manager.clone(),
        active_workspace_path.clone(),
    );
    root.append(&top);
    root.append(&stack);

    let handle_drop_payload: Rc<dyn Fn(String) -> bool> = Rc::new({
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        move |raw: String| {
            let Some((pane_two_thread, pane_two_workspace)) = parse_thread_drop_payload(&raw)
            else {
                return false;
            };
            let Some(pane_two_thread) = pane_two_thread.filter(|id| !id.trim().is_empty()) else {
                return false;
            };
            let pane_one_thread = active_thread_id.borrow().clone();
            let pane_one_workspace = active_workspace_path.borrow().clone();
            let pane_two_workspace =
                pane_two_workspace.or_else(|| active_workspace_path.borrow().clone());
            let layout = serde_json::json!({
                "version": 1,
                "focusedPaneId": 2,
                "panes": [
                    {
                        "id": 1,
                        "threadId": pane_one_thread,
                        "codexThreadId": pane_one_thread,
                        "workspacePath": pane_one_workspace,
                        "tab": "chat"
                    },
                    {
                        "id": 2,
                        "threadId": pane_two_thread,
                        "codexThreadId": pane_two_thread,
                        "workspacePath": pane_two_workspace,
                        "tab": "chat"
                    }
                ]
            })
            .to_string();
            let _ = db.set_setting(SETTING_MULTIVIEW_ENABLED, "1");
            let _ = db.set_setting(SETTING_PANE_LAYOUT_V1, &layout);
            true
        }
    });

    let drop_target_root = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::COPY);
    drop_target_root.connect_drop({
        let handle_drop_payload = handle_drop_payload.clone();
        move |_, value, _, _| {
            let Ok(raw) = value.get::<String>() else {
                return false;
            };
            handle_drop_payload(raw)
        }
    });
    root.add_controller(drop_target_root);

    let drop_target_stack = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::COPY);
    drop_target_stack.connect_drop({
        let handle_drop_payload = handle_drop_payload.clone();
        move |_, value, _, _| {
            let Ok(raw) = value.get::<String>() else {
                return false;
            };
            handle_drop_payload(raw)
        }
    });
    stack.add_controller(drop_target_stack);

    root
}

pub fn build_content(
    db: Rc<AppDb>,
    profile_manager: Rc<CodexProfileManager>,
    codex: Option<Arc<RuntimeClient>>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> adw::ToolbarView {
    let toolbar = adw::ToolbarView::new();
    toolbar.set_top_bar_style(adw::ToolbarStyle::Flat);
    toolbar.set_bottom_bar_style(adw::ToolbarStyle::Flat);
    toolbar.add_css_class("content-area");

    let content_shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_shell.add_css_class("content-shell");

    let mode_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    mode_host.set_vexpand(true);
    let mode_stack = gtk::Stack::new();
    mode_stack.set_vexpand(true);
    mode_stack.set_hexpand(true);
    mode_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    mode_stack.set_transition_duration(120);
    mode_host.append(&mode_stack);
    content_shell.append(&mode_host);

    let is_multi_mode = Rc::new(RefCell::new(is_multiview_enabled(db.as_ref())));
    let classic_view: Rc<RefCell<Option<gtk::Box>>> = Rc::new(RefCell::new(None));
    let multiview_view: Rc<RefCell<Option<gtk::Box>>> = Rc::new(RefCell::new(None));
    let ensure_mode_view: Rc<dyn Fn(bool)> = {
        let mode_stack = mode_stack.clone();
        let db = db.clone();
        let profile_manager = profile_manager.clone();
        let codex = codex.clone();
        let active_thread_id = active_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        let classic_view = classic_view.clone();
        let multiview_view = multiview_view.clone();
        Rc::new(move |multi_mode: bool| {
            if multi_mode {
                if multiview_view.borrow().is_none() {
                    let view = build_multiview_content(
                        db.clone(),
                        profile_manager.clone(),
                        codex.clone(),
                        active_thread_id.clone(),
                        active_workspace_path.clone(),
                    );
                    mode_stack.add_named(&view, Some("multi"));
                    multiview_view.replace(Some(view));
                }
            } else if classic_view.borrow().is_none() {
                let view = build_classic_content(
                    db.clone(),
                    profile_manager.clone(),
                    codex.clone(),
                    active_thread_id.clone(),
                    active_workspace_path.clone(),
                );
                mode_stack.add_named(&view, Some("classic"));
                classic_view.replace(Some(view));
            }
        })
    };
    let render_mode: Rc<dyn Fn(bool)> = {
        let ensure_mode_view = ensure_mode_view.clone();
        let mode_stack = mode_stack.clone();
        Rc::new(move |multi_mode: bool| {
            ensure_mode_view(multi_mode);
            mode_stack.set_visible_child_name(if multi_mode { "multi" } else { "classic" });
        })
    };
    render_mode(*is_multi_mode.borrow());
    {
        let db = db.clone();
        let is_multi_mode = is_multi_mode.clone();
        let render_mode = render_mode.clone();
        let mode_host = mode_host.clone();
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(220), move || {
            if mode_host.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let should_be_multi = is_multiview_enabled(db.as_ref());
            if should_be_multi != *is_multi_mode.borrow() {
                is_multi_mode.replace(should_be_multi);
                render_mode(should_be_multi);
            }
            gtk::glib::ControlFlow::Continue
        });
    }
    toolbar.set_content(Some(&content_shell));

    let bottom = build_bottom_bar(db.clone(), profile_manager);
    toolbar.add_bottom_bar(&bottom);
    toolbar
}
