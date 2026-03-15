use crate::codex_profiles::CodexProfileManager;
use crate::ui::widget_tree;
use adw::prelude::*;
use gtk::glib::value::ToValue;
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::data::{AppDb, ThreadRecord};

mod layout;

pub fn remove_thread_from_multiview_layout(db: &AppDb, remote_thread_id: &str) {
    layout::remove_thread_from_multiview_layout(db, remote_thread_id);
}

#[allow(dead_code)]
pub fn remove_codex_thread_from_multiview_layout(db: &AppDb, codex_thread_id: &str) {
    remove_thread_from_multiview_layout(db, codex_thread_id);
}

thread_local! {
    static THREAD_LIST_REGISTRY: RefCell<HashMap<usize, ThreadList>> = RefCell::new(HashMap::new());
}

pub fn refresh_all_profile_icon_visibility() {
    THREAD_LIST_REGISTRY.with(|registry| {
        for thread_list in registry.borrow().values() {
            thread_list.refresh_profile_icon_visibility();
        }
    });
}

fn listbox_registry_key(listbox: &gtk::ListBox) -> usize {
    listbox.as_ptr() as usize
}

fn register_thread_list(listbox: &gtk::ListBox, thread_list: &ThreadList) {
    let key = listbox_registry_key(listbox);
    THREAD_LIST_REGISTRY.with(|registry| {
        registry.borrow_mut().insert(key, thread_list.clone());
    });
    listbox.connect_destroy(move |_| {
        THREAD_LIST_REGISTRY.with(|registry| {
            registry.borrow_mut().remove(&key);
        });
    });
}

fn with_thread_list_for_listbox<T>(
    listbox: &gtk::ListBox,
    f: impl FnOnce(&ThreadList) -> T,
) -> Option<T> {
    let key = listbox_registry_key(listbox);
    THREAD_LIST_REGISTRY.with(|registry| {
        let registry = registry.borrow();
        registry.get(&key).map(f)
    })
}

#[derive(Clone)]
pub struct ThreadList {
    container: gtk::Box,
    listbox: gtk::ListBox,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
    workspace_path: String,
    show_profile_icons: Rc<Cell<bool>>,
}

pub fn update_thread_row_title(root: &gtk::Widget, thread_id: i64, title: &str) -> bool {
    let target_name = format!("thread-{}", thread_id);
    update_thread_row_title_inner(root, &target_name, title)
}

fn update_thread_row_title_inner(root: &gtk::Widget, target_name: &str, title: &str) -> bool {
    if let Some(row) = root.downcast_ref::<gtk::ListBoxRow>() {
        if row.widget_name() == target_name {
            if let Some(child) = row.child() {
                if let Some(label) = find_thread_title_label(&child) {
                    label.set_text(title);
                    return true;
                }
            }
            return false;
        }
    }

    let mut child = root.first_child();
    while let Some(node) = child {
        if update_thread_row_title_inner(&node, target_name, title) {
            return true;
        }
        child = node.next_sibling();
    }

    false
}

fn find_thread_title_label(root: &gtk::Widget) -> Option<gtk::Label> {
    if let Ok(label) = root.clone().downcast::<gtk::Label>() {
        if label.has_css_class("thread-title") {
            return Some(label);
        }
    }

    let mut child = root.first_child();
    while let Some(node) = child {
        if let Some(found) = find_thread_title_label(&node) {
            return Some(found);
        }
        child = node.next_sibling();
    }

    None
}

fn find_thread_worktree_icon(root: &gtk::Widget) -> Option<gtk::Image> {
    if let Ok(image) = root.clone().downcast::<gtk::Image>() {
        if image.has_css_class("thread-worktree-icon") {
            return Some(image);
        }
    }

    let mut child = root.first_child();
    while let Some(node) = child {
        if let Some(found) = find_thread_worktree_icon(&node) {
            return Some(found);
        }
        child = node.next_sibling();
    }

    None
}

fn find_thread_profile_icon(root: &gtk::Widget) -> Option<gtk::Image> {
    if let Ok(image) = root.clone().downcast::<gtk::Image>() {
        if image.has_css_class("thread-profile-icon") {
            return Some(image);
        }
    }

    let mut child = root.first_child();
    while let Some(node) = child {
        if let Some(found) = find_thread_profile_icon(&node) {
            return Some(found);
        }
        child = node.next_sibling();
    }

    None
}

fn local_thread_id_from_row(row: &gtk::ListBoxRow) -> Option<i64> {
    row.widget_name()
        .strip_prefix("thread-")
        .and_then(|value| value.parse::<i64>().ok())
}

fn profile_icon_name_for_profile(db: &AppDb, profile_id: i64) -> String {
    db.get_codex_profile(profile_id)
        .ok()
        .flatten()
        .map(|profile| profile.icon_name.trim().to_string())
        .filter(|icon_name| !icon_name.is_empty())
        .unwrap_or_else(|| "person-symbolic".to_string())
}

fn thread_has_linked_profile(thread: &ThreadRecord) -> bool {
    thread
        .remote_thread_id()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
        || thread
            .remote_account_type()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
        || thread
            .remote_account_email()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
}

fn profile_icon_name_for_thread(db: &AppDb, thread: &ThreadRecord) -> Option<String> {
    thread_has_linked_profile(thread).then(|| profile_icon_name_for_profile(db, thread.profile_id))
}

fn has_non_system_profile(db: &AppDb) -> bool {
    let system_home =
        crate::data::configured_profile_home_dir(&crate::data::default_app_data_dir());
    let system_home = system_home.to_string_lossy().to_string();
    db.list_codex_profiles()
        .ok()
        .map(|profiles| {
            profiles
                .iter()
                .any(|profile| profile.home_dir.trim() != system_home.trim())
        })
        .unwrap_or(true)
}

pub fn set_thread_row_worktree_icon_visible(
    root: &gtk::Widget,
    thread_id: i64,
    visible: bool,
) -> bool {
    let target_name = format!("thread-{}", thread_id);
    let Some(row) = widget_tree::find_listbox_row_by_widget_name(root, &target_name) else {
        return false;
    };
    let Some(child) = row.child() else {
        return false;
    };
    let Some(icon) = find_thread_worktree_icon(&child) else {
        return false;
    };
    icon.set_visible(visible);
    true
}

fn find_row_by_widget_name(listbox: &gtk::ListBox, target_name: &str) -> Option<gtk::ListBoxRow> {
    let mut index = 0;
    loop {
        let Some(row) = listbox.row_at_index(index) else {
            return None;
        };
        if row.widget_name() == target_name {
            return Some(row);
        }
        index += 1;
    }
}

fn set_listbox_selected_row(listbox: &gtk::ListBox, target_name: Option<&str>) {
    let mut index = 0;
    loop {
        let Some(row) = listbox.row_at_index(index) else {
            break;
        };
        let is_selected = target_name
            .map(|name| row.widget_name() == name)
            .unwrap_or(false);
        if is_selected {
            row.add_css_class("thread-row-selected");
        } else {
            row.remove_css_class("thread-row-selected");
        }
        index += 1;
    }
}

fn ordered_threads_for_display(threads: &[ThreadRecord]) -> Vec<ThreadRecord> {
    let mut children_by_parent: HashMap<i64, Vec<ThreadRecord>> = HashMap::new();
    for thread in threads {
        if let Some(parent_id) = thread.parent_thread_id {
            children_by_parent
                .entry(parent_id)
                .or_default()
                .push(thread.clone());
        }
    }

    let id_set: HashSet<i64> = threads.iter().map(|thread| thread.id).collect();
    let roots: Vec<ThreadRecord> = threads
        .iter()
        .filter(|thread| {
            thread
                .parent_thread_id
                .map(|parent_id| !id_set.contains(&parent_id))
                .unwrap_or(true)
        })
        .cloned()
        .collect();

    let mut out = Vec::with_capacity(threads.len());
    let mut visited = HashSet::new();

    fn append_branch(
        thread: &ThreadRecord,
        children_by_parent: &HashMap<i64, Vec<ThreadRecord>>,
        visited: &mut HashSet<i64>,
        out: &mut Vec<ThreadRecord>,
    ) {
        if !visited.insert(thread.id) {
            return;
        }
        out.push(thread.clone());
        if let Some(children) = children_by_parent.get(&thread.id) {
            for child in children {
                append_branch(child, children_by_parent, visited, out);
            }
        }
    }

    for root in roots {
        append_branch(&root, &children_by_parent, &mut visited, &mut out);
    }

    for thread in threads {
        if visited.insert(thread.id) {
            out.push(thread.clone());
        }
    }

    out
}

fn is_expected_pre_materialization_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("no rollout found for thread id")
        || (lower.contains("not materialized yet") && lower.contains("includeturns is unavailable"))
        || lower.contains("thread not loaded")
}

fn thread_runtime_workspace_path(thread: &ThreadRecord, workspace_path: &str) -> String {
    thread
        .worktree_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty() && thread.worktree_active)
        .map(|path| path.to_string())
        .unwrap_or_else(|| workspace_path.to_string())
}

impl ThreadList {
    pub fn new(
        db: Rc<AppDb>,
        manager: Rc<CodexProfileManager>,
        active_thread_id: Rc<RefCell<Option<String>>>,
        active_workspace_path: Rc<RefCell<Option<String>>>,
        workspace_path: String,
        threads: &[ThreadRecord],
        expanded: bool,
    ) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.add_css_class("threads-list");
        container.set_widget_name("workspace-thread-list");
        container.set_visible(expanded);

        let listbox = gtk::ListBox::new();
        listbox.add_css_class("thread-listbox");
        listbox.add_css_class("thread-listbox-custom");
        listbox.set_widget_name("workspace-thread-listbox");
        listbox.set_selection_mode(gtk::SelectionMode::None);
        let show_profile_icons = Rc::new(Cell::new(false));

        let ordered_threads = ordered_threads_for_display(threads);
        for thread in ordered_threads {
            listbox.append(&thread_row(
                db.clone(),
                manager.clone(),
                active_thread_id.clone(),
                active_workspace_path.clone(),
                workspace_path.clone(),
                show_profile_icons.clone(),
                thread,
            ));
        }

        container.append(&listbox);

        let thread_list = Self {
            container,
            listbox,
            db,
            manager,
            active_thread_id,
            active_workspace_path,
            workspace_path,
            show_profile_icons,
        };
        thread_list.refresh_profile_icon_visibility();
        register_thread_list(&thread_list.listbox, &thread_list);
        thread_list
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }

    pub fn is_expanded(&self) -> bool {
        self.container.is_visible()
    }

    pub fn set_expanded(&self, expanded: bool) {
        self.container.set_visible(expanded);
    }

    pub fn append_thread(&self, thread: ThreadRecord) -> gtk::ListBoxRow {
        let listbox_widget: gtk::Widget = self.listbox.clone().upcast();
        let scroll_state = widget_tree::capture_ancestor_vscroll(&listbox_widget);
        let runtime_workspace_path = thread_runtime_workspace_path(&thread, &self.workspace_path);
        self.active_workspace_path
            .replace(Some(runtime_workspace_path.clone()));
        self.active_thread_id
            .replace(thread.remote_thread_id_owned());

        let row = thread_row(
            self.db.clone(),
            self.manager.clone(),
            self.active_thread_id.clone(),
            self.active_workspace_path.clone(),
            self.workspace_path.clone(),
            self.show_profile_icons.clone(),
            thread.clone(),
        );
        if let Some(parent_id) = thread.parent_thread_id {
            if let Some(parent_row) =
                find_row_by_widget_name(&self.listbox, &format!("thread-{parent_id}"))
            {
                self.listbox.insert(&row, parent_row.index() + 1);
            } else {
                self.listbox.prepend(&row);
            }
        } else {
            self.listbox.prepend(&row);
        }
        if let Some(root) = self.listbox.root() {
            let root_widget: gtk::Widget = root.upcast();
            clear_thread_list_selections(&root_widget);
        }
        set_listbox_selected_row(&self.listbox, Some(&row.widget_name()));
        if let Some((scroll, value)) = scroll_state {
            widget_tree::restore_vscroll_position(&scroll, value);
        }
        self.refresh_profile_icon_visibility();
        row
    }

    pub fn append_thread_passive(&self, thread: ThreadRecord) -> gtk::ListBoxRow {
        let listbox_widget: gtk::Widget = self.listbox.clone().upcast();
        let scroll_state = widget_tree::capture_ancestor_vscroll(&listbox_widget);
        let row = thread_row(
            self.db.clone(),
            self.manager.clone(),
            self.active_thread_id.clone(),
            self.active_workspace_path.clone(),
            self.workspace_path.clone(),
            self.show_profile_icons.clone(),
            thread.clone(),
        );
        if let Some(parent_id) = thread.parent_thread_id {
            if let Some(parent_row) =
                find_row_by_widget_name(&self.listbox, &format!("thread-{parent_id}"))
            {
                self.listbox.insert(&row, parent_row.index() + 1);
            } else {
                self.listbox.prepend(&row);
            }
        } else {
            self.listbox.prepend(&row);
        }
        if let Some((scroll, value)) = scroll_state {
            widget_tree::restore_vscroll_position(&scroll, value);
        }
        self.refresh_profile_icon_visibility();
        row
    }

    fn refresh_profile_icon_visibility(&self) {
        let mut profile_ids = HashSet::new();
        let mut row_index = 0;
        loop {
            let Some(row) = self.listbox.row_at_index(row_index) else {
                break;
            };
            row_index += 1;
            let Some(local_thread_id) = local_thread_id_from_row(&row) else {
                continue;
            };
            if let Ok(Some(thread)) = self.db.get_thread_record(local_thread_id) {
                if thread_has_linked_profile(&thread) {
                    profile_ids.insert(thread.profile_id);
                }
            }
        }

        let show_icons = !profile_ids.is_empty() && has_non_system_profile(self.db.as_ref());
        self.show_profile_icons.set(show_icons);

        let mut row_index = 0;
        loop {
            let Some(row) = self.listbox.row_at_index(row_index) else {
                break;
            };
            row_index += 1;
            let Some(child) = row.child() else {
                continue;
            };
            if let Some(profile_icon) = find_thread_profile_icon(&child) {
                let Some(local_thread_id) = local_thread_id_from_row(&row) else {
                    profile_icon.set_visible(false);
                    continue;
                };
                let show_for_row = self
                    .db
                    .get_thread_record(local_thread_id)
                    .ok()
                    .flatten()
                    .and_then(|thread| profile_icon_name_for_thread(self.db.as_ref(), &thread))
                    .is_some();
                profile_icon.set_visible(show_icons && show_for_row);
            }
        }
    }
}

pub fn append_thread_under_parent_from_root(
    root: &gtk::Widget,
    parent_thread_id: i64,
    thread: ThreadRecord,
) -> bool {
    let Some(parent_row) =
        widget_tree::find_listbox_row_by_widget_name(root, &format!("thread-{parent_thread_id}"))
    else {
        return false;
    };
    let Some(listbox) = parent_row.parent().and_downcast::<gtk::ListBox>() else {
        return false;
    };
    let Some(row) =
        with_thread_list_for_listbox(&listbox, |thread_list| thread_list.append_thread(thread))
    else {
        return false;
    };
    set_listbox_selected_row(&listbox, Some(&row.widget_name()));
    true
}

pub fn append_thread_under_parent_from_root_passive(
    root: &gtk::Widget,
    parent_thread_id: i64,
    thread: ThreadRecord,
) -> bool {
    let Some(parent_row) =
        widget_tree::find_listbox_row_by_widget_name(root, &format!("thread-{parent_thread_id}"))
    else {
        return false;
    };
    let Some(listbox) = parent_row.parent().and_downcast::<gtk::ListBox>() else {
        return false;
    };
    with_thread_list_for_listbox(&listbox, |thread_list| {
        let _ = thread_list.append_thread_passive(thread);
    })
    .is_some()
}

fn clear_thread_list_selections(_root: &gtk::Widget) {
    THREAD_LIST_REGISTRY.with(|registry| {
        for thread_list in registry.borrow().values() {
            set_listbox_selected_row(&thread_list.listbox, None);
        }
    });
}

include!("thread_list/row_menu_impl.rs");
