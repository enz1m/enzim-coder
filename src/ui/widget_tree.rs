use gtk::glib::object::Cast;
use gtk::prelude::*;

pub(crate) fn clear_box_children(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

pub(crate) fn find_widget_by_name(root: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if root.widget_name() == name {
        return Some(root.clone());
    }

    let mut child = root.first_child();
    while let Some(node) = child {
        if let Some(found) = find_widget_by_name(&node, name) {
            return Some(found);
        }
        child = node.next_sibling();
    }

    None
}

pub(crate) fn find_widget_by_css_class(root: &gtk::Widget, css_class: &str) -> Option<gtk::Widget> {
    if root.has_css_class(css_class) {
        return Some(root.clone());
    }

    let mut child = root.first_child();
    while let Some(node) = child {
        if let Some(found) = find_widget_by_css_class(&node, css_class) {
            return Some(found);
        }
        child = node.next_sibling();
    }

    None
}

pub(crate) fn find_listbox_row_by_widget_name(
    root: &gtk::Widget,
    widget_name: &str,
) -> Option<gtk::ListBoxRow> {
    if let Some(row) = root.downcast_ref::<gtk::ListBoxRow>() {
        if row.widget_name() == widget_name {
            return Some(row.clone());
        }
    }

    let mut child = root.first_child();
    while let Some(node) = child {
        if let Some(found) = find_listbox_row_by_widget_name(&node, widget_name) {
            return Some(found);
        }
        child = node.next_sibling();
    }

    None
}

pub(crate) fn capture_ancestor_vscroll(widget: &gtk::Widget) -> Option<(gtk::ScrolledWindow, f64)> {
    let mut current = widget.parent();
    while let Some(node) = current {
        if let Ok(scroll) = node.clone().downcast::<gtk::ScrolledWindow>() {
            let value = scroll.vadjustment().value();
            return Some((scroll, value));
        }
        current = node.parent();
    }
    None
}

pub(crate) fn restore_vscroll_position(scroll: &gtk::ScrolledWindow, value: f64) {
    let apply_value = move |scroll: &gtk::ScrolledWindow| {
        let adjustment = scroll.vadjustment();
        let lower = adjustment.lower();
        let upper = adjustment.upper();
        let page_size = adjustment.page_size();
        let max_value = (upper - page_size).max(lower);
        adjustment.set_value(value.clamp(lower, max_value));
    };

    apply_value(scroll);

    let scroll = scroll.clone();
    gtk::glib::idle_add_local_once(move || {
        apply_value(&scroll);
    });
}

fn clear_thread_listbox_selections(root: &gtk::Widget) {
    if let Some(listbox) = root.downcast_ref::<gtk::ListBox>() {
        if listbox.has_css_class("thread-listbox") {
            if let Some(selected_row) = listbox.selected_row() {
                listbox.unselect_row(&selected_row);
            }
            let mut index = 0;
            while let Some(row) = listbox.row_at_index(index) {
                row.remove_css_class("thread-row-selected");
                index += 1;
            }
        }
    }

    let mut child = root.first_child();
    while let Some(node) = child {
        clear_thread_listbox_selections(&node);
        child = node.next_sibling();
    }
}

pub(crate) fn select_thread_row(root: &gtk::Widget, thread_id: i64) -> bool {
    let widget_name = format!("thread-{thread_id}");
    let Some(row) = find_listbox_row_by_widget_name(root, &widget_name) else {
        return false;
    };
    if let Some(listbox) = row.parent().and_then(|p| p.downcast::<gtk::ListBox>().ok()) {
        clear_thread_listbox_selections(root);
        listbox.select_row(Some(&row));
        row.add_css_class("thread-row-selected");
        return true;
    }
    false
}
