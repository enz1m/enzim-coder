use crate::services::app::runtime::RuntimeClient;
use crate::services::app::CodexProfileManager;
use crate::services::app::chat::AppDb;
use gtk::prelude::*;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(super) struct ComposerSection {
    pub(super) lower_content: gtk::Box,
    pub(super) suggestion_row: gtk::Box,
    pub(super) live_turn_status_revealer: gtk::Revealer,
    pub(super) live_turn_status_label: gtk::Label,
    pub(super) live_turn_timer_label: gtk::Label,
}

#[derive(Clone)]
struct MentionAttachment {
    display: String,
    path: String,
}

#[derive(Clone)]
struct BrowserEntry {
    display: String,
    path: PathBuf,
    is_dir: bool,
}

#[derive(Clone, Debug)]
struct WorktreeBatchEntry {
    forked_thread_id: String,
    worktree_path: String,
    worktree_branch: String,
}

#[derive(Clone, Debug)]
struct WorktreeBatchResult {
    entries: Vec<WorktreeBatchEntry>,
    errors: Vec<String>,
}

#[derive(Clone, Debug)]
struct ImageAttachment {
    path: String,
}

#[derive(Clone)]
struct QueuedPayload {
    remote_prompt_id: Option<i64>,
    text: String,
    summary: String,
    mentions: Vec<(String, String)>,
    images: Vec<String>,
    expected_thread_id: Option<String>,
    model_id: String,
    effort: String,
    sandbox_policy: Option<Value>,
    collaboration_mode: Option<Value>,
}

#[derive(Clone)]
struct QueuedUiEntry {
    id: u64,
    row: gtk::Box,
    preview_label: gtk::Label,
    steer_button: gtk::Button,
    payload: Rc<RefCell<QueuedPayload>>,
}

const MENTION_SCAN_RESULT_LIMIT: usize = 1200;
const MENTION_SCAN_MAX_DIRS: usize = 400;
const MENTION_SCAN_MAX_ENTRIES_PER_DIR: usize = 512;

fn is_supported_image_path(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tiff" | "tif" | "svg"
            )
        })
        .unwrap_or(false)
}

fn normalize_image_path(path: &Path) -> Option<String> {
    if !is_supported_image_path(path) {
        return None;
    }
    path.canonicalize()
        .ok()
        .or_else(|| Some(path.to_path_buf()))
        .and_then(|resolved| resolved.to_str().map(|value| value.to_string()))
}

fn add_image_attachments(
    selected_images: &Rc<RefCell<Vec<ImageAttachment>>>,
    image_paths: &[PathBuf],
) -> usize {
    let mut added = 0usize;
    let mut images = selected_images.borrow_mut();
    for path in image_paths {
        let Some(normalized) = normalize_image_path(path) else {
            continue;
        };
        if images.iter().any(|entry| entry.path == normalized) {
            continue;
        }
        images.push(ImageAttachment { path: normalized });
        added += 1;
    }
    added
}

fn send_payload_summary_from_paths(text: &str, image_paths: &[String]) -> String {
    let mut parts = Vec::new();
    if !text.trim().is_empty() {
        parts.push(text.to_string());
    }
    for path in image_paths {
        let name = Path::new(path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("image");
        parts.push(format!("[image] {name}"));
    }
    parts.join("\n")
}

fn update_send_button_active_state(
    send: &gtk::Button,
    input_view: &gtk::TextView,
    selected_images: &Rc<RefCell<Vec<ImageAttachment>>>,
    is_locked: bool,
) {
    let buffer = input_view.buffer();
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    let text = buffer.text(&start, &end, true);
    let has_payload = !text.trim().is_empty() || !selected_images.borrow().is_empty();
    if has_payload && !is_locked {
        send.add_css_class("send-button-active");
    } else {
        send.remove_css_class("send-button-active");
    }
}

fn refresh_image_preview_strip(
    scroll: &gtk::ScrolledWindow,
    strip: &gtk::Box,
    selected_images: &Rc<RefCell<Vec<ImageAttachment>>>,
    send: &gtk::Button,
    input_view: &gtk::TextView,
    thread_locked: &Rc<RefCell<bool>>,
) {
    while let Some(child) = strip.first_child() {
        strip.remove(&child);
    }

    let images = selected_images.borrow().clone();
    strip.set_visible(!images.is_empty());
    scroll.set_visible(!images.is_empty());
    for (idx, image) in images.iter().enumerate() {
        let chip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        chip.add_css_class("composer-image-chip");
        chip.set_valign(gtk::Align::Center);

        let preview = gtk::Image::from_file(&image.path);
        preview.add_css_class("composer-image-preview");
        preview.set_pixel_size(34);
        chip.append(&preview);

        let name = Path::new(&image.path)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("image");
        let label = gtk::Label::new(Some(name));
        label.set_xalign(0.0);
        label.add_css_class("composer-image-name");
        chip.append(&label);

        let remove = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .build();
        remove.set_has_frame(false);
        remove.add_css_class("app-flat-button");
        remove.add_css_class("composer-image-remove");
        {
            let selected_images = selected_images.clone();
            let scroll = scroll.clone();
            let strip = strip.clone();
            let send = send.clone();
            let input_view = input_view.clone();
            let thread_locked = thread_locked.clone();
            remove.connect_clicked(move |_| {
                let mut images = selected_images.borrow_mut();
                if idx < images.len() {
                    images.remove(idx);
                }
                drop(images);
                refresh_image_preview_strip(
                    &scroll,
                    &strip,
                    &selected_images,
                    &send,
                    &input_view,
                    &thread_locked,
                );
            });
        }
        chip.append(&remove);
        strip.append(&chip);
    }

    update_send_button_active_state(send, input_view, selected_images, *thread_locked.borrow());
}

fn parse_uri_list_paths(raw: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in raw.lines() {
        let value = line.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        if let Ok((path, _)) = gtk::glib::filename_from_uri(value) {
            out.push(path);
            continue;
        }
        if value.starts_with('/') {
            out.push(PathBuf::from(value));
        }
    }
    out
}

fn ensure_composer_image_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("enzimcoder-composer-images");
    fs::create_dir_all(&dir).map_err(|err| format!("failed to create temp image dir: {err}"))?;
    Ok(dir)
}

fn worktree_merge_action_label(action: &crate::services::app::worktree::WorktreeMergeAction) -> &'static str {
    match action {
        crate::services::app::worktree::WorktreeMergeAction::Write => "Update",
        crate::services::app::worktree::WorktreeMergeAction::Delete => "Delete",
        crate::services::app::worktree::WorktreeMergeAction::Rename => "Rename",
    }
}

pub(super) fn open_worktree_merge_popup(
    parent: Option<gtk::Window>,
    db: Rc<AppDb>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    conversation_stack: &gtk::Stack,
    local_thread_id: i64,
    worktree_path: &str,
    live_workspace_path: &str,
) {
    let dialog = gtk::Window::builder()
        .title("Merge Worktree")
        .default_width(560)
        .default_height(420)
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

    let heading = gtk::Label::new(Some("Merge worktree changes into live workspace?"));
    heading.set_xalign(0.0);
    heading.add_css_class("chat-restore-popup-heading");
    root.append(&heading);

    let summary = gtk::Label::new(Some(""));
    summary.set_xalign(0.0);
    summary.add_css_class("dim-label");
    summary.set_wrap(true);
    summary.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    root.append(&summary);

    let listbox = gtk::ListBox::new();
    listbox.add_css_class("navigation-sidebar");
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .child(&listbox)
        .min_content_height(220)
        .build();
    scroll.set_has_frame(false);
    root.append(&scroll);

    let status = gtk::Label::new(Some(""));
    status.set_xalign(0.0);
    status.add_css_class("dim-label");
    status.set_wrap(true);
    status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    root.append(&status);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let merge = gtk::Button::with_label("Merge & Stop");
    merge.add_css_class("suggested-action");
    actions.append(&cancel);
    actions.append(&merge);
    root.append(&actions);

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }

    let preview = match crate::services::app::worktree::preview_worktree_merge(worktree_path) {
        Ok(preview) => preview,
        Err(err) => {
            summary.set_text("Unable to read worktree changes.");
            status.set_text(&err);
            merge.set_sensitive(false);
            super::message_render::append_message(
                messages_box,
                Some(messages_scroll),
                conversation_stack,
                &format!("Failed to preview worktree merge: {err}"),
                false,
                std::time::SystemTime::now(),
            );
            dialog.set_child(Some(&root));
            dialog.present();
            return;
        }
    };

    for item in &preview.items {
        let row = gtk::ListBoxRow::new();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.set_margin_start(8);
        line.set_margin_end(8);
        line.set_margin_top(6);
        line.set_margin_bottom(6);

        let mut label_text = item.path.clone();
        if let Some(from) = item.from_path.as_deref() {
            label_text = format!("{from} -> {}", item.path);
        }
        let path = gtk::Label::new(Some(&label_text));
        path.set_xalign(0.0);
        path.set_hexpand(true);
        path.add_css_class("restore-preview-path");
        line.append(&path);

        let action = gtk::Label::new(Some(worktree_merge_action_label(&item.action)));
        action.add_css_class("restore-preview-kind");
        line.append(&action);

        row.set_child(Some(&line));
        listbox.append(&row);
    }

    if preview.items.is_empty() {
        summary.set_text(&format!(
            "No pending file changes. Stop this worktree and switch this thread back to live workspace?\n\nLive workspace: {}",
            live_workspace_path
        ));
        merge.set_label("Stop Worktree");
    } else {
        summary.set_text(&format!(
            "This will merge {} file change(s) into:\n{}\n\nWorktree: {}",
            preview.items.len(),
            live_workspace_path,
            worktree_path
        ));
    }

    {
        let db_for_apply = db.clone();
        let active_workspace_path_for_apply = active_workspace_path.clone();
        let messages_box_for_apply = messages_box.clone();
        let messages_scroll_for_apply = messages_scroll.clone();
        let conversation_stack_for_apply = conversation_stack.clone();
        let dialog_for_apply = dialog.clone();
        let merge_for_apply = merge.clone();
        let cancel_for_apply = cancel.clone();
        let status_for_apply = status.clone();
        let worktree_path_for_apply = worktree_path.to_string();
        let live_workspace_path_for_apply = live_workspace_path.to_string();
        merge.connect_clicked(move |_| {
            merge_for_apply.set_sensitive(false);
            cancel_for_apply.set_sensitive(false);
            status_for_apply.set_text("Merging and stopping worktree...");

            let (tx, rx) = mpsc::channel::<Result<crate::services::app::worktree::WorktreeMergeResult, String>>();
            let worktree_path_bg = worktree_path_for_apply.clone();
            let live_workspace_path_bg = live_workspace_path_for_apply.clone();
            thread::spawn(move || {
                let result = crate::services::app::worktree::apply_worktree_merge(
                    &worktree_path_bg,
                    &live_workspace_path_bg,
                )
                .and_then(|merge_result| {
                    crate::services::app::worktree::stop_worktree_checkout(&worktree_path_bg)?;
                    Ok(merge_result)
                });
                let _ = tx.send(result);
            });

            let db_for_result = db_for_apply.clone();
            let active_workspace_path_for_result = active_workspace_path_for_apply.clone();
            let messages_box_for_result = messages_box_for_apply.clone();
            let messages_scroll_for_result = messages_scroll_for_apply.clone();
            let conversation_stack_for_result = conversation_stack_for_apply.clone();
            let dialog_for_result = dialog_for_apply.clone();
            let merge_for_result = merge_for_apply.clone();
            let cancel_for_result = cancel_for_apply.clone();
            let status_for_result = status_for_apply.clone();
            let live_workspace_path_for_result = live_workspace_path_for_apply.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(40), move || {
                match rx.try_recv() {
                    Ok(Ok(result)) => {
                        let _ = db_for_result.set_thread_worktree_info(local_thread_id, None, None, false);
                        let _ = db_for_result
                            .set_setting("last_active_workspace_path", &live_workspace_path_for_result);
                        active_workspace_path_for_result
                            .replace(Some(live_workspace_path_for_result.clone()));

                        if let Some(root) = messages_box_for_result.root() {
                            let root_widget: gtk::Widget = root.upcast();
                            let _ = crate::ui::components::thread_list::set_thread_row_worktree_icon_visible(
                                &root_widget,
                                local_thread_id,
                                false,
                            );
                        }

                        let summary = format!(
                            "Worktree merged and stopped for this thread.\nUpdated: {} • Deleted: {} • Renamed: {}",
                            result.merged_count, result.deleted_count, result.renamed_count
                        );
                        super::message_render::append_message(
                            &messages_box_for_result,
                            Some(&messages_scroll_for_result),
                            &conversation_stack_for_result,
                            &summary,
                            false,
                            std::time::SystemTime::now(),
                        );
                        dialog_for_result.close();
                        gtk::glib::ControlFlow::Break
                    }
                    Ok(Err(err)) => {
                        status_for_result.set_text(&format!("Merge failed: {err}"));
                        merge_for_result.set_sensitive(true);
                        cancel_for_result.set_sensitive(true);
                        gtk::glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        status_for_result.set_text("Merge failed: worker disconnected.");
                        merge_for_result.set_sensitive(true);
                        cancel_for_result.set_sensitive(true);
                        gtk::glib::ControlFlow::Break
                    }
                }
            });
        });
    }

    dialog.set_child(Some(&root));
    dialog.present();
}

fn create_compact_separator() -> gtk::Label {
    let separator = gtk::Label::new(Some("|"));
    separator.add_css_class("compact-separator");
    separator.set_valign(gtk::Align::Center);
    separator
}

fn create_suggestion_chip(label: &str) -> gtk::Box {
    let chip_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    chip_box.add_css_class("suggestion-chip");

    let label_widget = gtk::Label::new(Some(label));
    chip_box.append(&label_widget);

    let gesture = gtk::GestureClick::new();
    gesture.connect_released(|_, _, _, _| {});
    chip_box.add_controller(gesture);

    chip_box
}

fn create_send_button() -> gtk::Button {
    let button = gtk::Button::new();
    button.set_has_frame(false);
    button.set_icon_name("satnav-symbolic");
    button.add_css_class("send-button");
    button.set_size_request(30, 30);
    button
}

fn collaboration_mode_payload(mode: &str, model_id: &str, effort: &str) -> Option<Value> {
    match mode {
        "plan" => Some(json!({
            "mode": "plan",
            "model": model_id,
            "settings": {
                "model": model_id,
                "reasoning_effort": effort,
                "developer_instructions": Value::Null
            }
        })),
        "default" | "agent" => Some(json!({
            "mode": "default",
            "model": model_id,
            "settings": {
                "model": model_id,
                "reasoning_effort": effort,
                "developer_instructions": Value::Null
            }
        })),
        _ => None,
    }
}

fn is_expected_pre_materialization_error(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("no rollout found for thread id")
        || (lower.contains("not materialized yet") && lower.contains("includeturns is unavailable"))
        || lower.contains("thread not loaded")
}

fn next_pending_user_row_marker() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    format!("pending-user-row:{now}")
}

fn mention_query_range(input_view: &gtk::TextView) -> Option<(usize, usize, String)> {
    let buffer = input_view.buffer();
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    let text = buffer.text(&start, &end, true).to_string();
    let chars: Vec<char> = text.chars().collect();
    let cursor = (buffer.cursor_position() as usize).min(chars.len());

    let mut at_index: Option<usize> = None;
    let mut index = cursor;
    while index > 0 {
        index -= 1;
        let ch = chars[index];
        if ch == '@' {
            if index == 0 || chars[index - 1].is_whitespace() {
                at_index = Some(index);
            }
            break;
        }
        if ch.is_whitespace() {
            break;
        }
    }

    let at_index = at_index?;
    let query: String = chars[(at_index + 1)..cursor].iter().collect();
    if query.contains(char::is_whitespace) {
        return None;
    }
    Some((at_index, cursor, query))
}

fn insert_selected_mention(input_view: &gtk::TextView, mention_display: &str) -> bool {
    let Some((start, end, _)) = mention_query_range(input_view) else {
        return false;
    };
    let buffer = input_view.buffer();
    let mut start_iter = buffer.iter_at_offset(start as i32);
    let mut end_iter = buffer.iter_at_offset(end as i32);
    buffer.delete(&mut start_iter, &mut end_iter);
    buffer.insert(&mut start_iter, &format!("@{} ", mention_display));
    true
}

fn append_direct_mention(input_view: &gtk::TextView, mention_display: &str) {
    let buffer = input_view.buffer();
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    let text = buffer.text(&start, &end, true);
    let needs_space = text
        .chars()
        .last()
        .map(|ch| !ch.is_whitespace())
        .unwrap_or(false);

    let mut insert_at = buffer.end_iter();
    if needs_space {
        buffer.insert(&mut insert_at, " ");
    }
    buffer.insert(&mut insert_at, &format!("@{} ", mention_display));
}

fn resolve_workspace_root(active_workspace_path: &Rc<RefCell<Option<String>>>) -> Option<PathBuf> {
    active_workspace_path
        .borrow()
        .clone()
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .or_else(|| std::env::current_dir().ok())
}

fn ensure_mention_files_loaded(
    active_workspace_path: &Rc<RefCell<Option<String>>>,
    mention_files_root: &Rc<RefCell<Option<PathBuf>>>,
    mention_files: &Rc<RefCell<Vec<(String, String)>>>,
) {
    let Some(root) = resolve_workspace_root(active_workspace_path) else {
        return;
    };

    let needs_reload = mention_files_root
        .borrow()
        .as_ref()
        .map(|current| current != &root)
        .unwrap_or(true);

    if needs_reload || mention_files.borrow().is_empty() {
        mention_files_root.replace(Some(root.clone()));
        mention_files.replace(collect_workspace_files(&root, MENTION_SCAN_RESULT_LIMIT));
    }
}

fn is_ignored_dir_name(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "build" | "target" | "node_modules")
}

fn collect_directory_entries(dir: &Path) -> Vec<BrowserEntry> {
    let mut entries = Vec::new();

    let Ok(read_dir) = fs::read_dir(dir) else {
        return entries;
    };

    for entry in read_dir.flatten().take(MENTION_SCAN_MAX_ENTRIES_PER_DIR) {
        let path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();

        if path.is_dir() {
            if is_ignored_dir_name(&file_name) {
                continue;
            }
            entries.push(BrowserEntry {
                display: format!("{}/", file_name),
                path,
                is_dir: true,
            });
            continue;
        }

        if path.is_file() {
            entries.push(BrowserEntry {
                display: file_name,
                path,
                is_dir: false,
            });
        }
    }

    entries.sort_by(|a, b| {
        a.is_dir
            .cmp(&b.is_dir)
            .reverse()
            .then_with(|| a.display.to_lowercase().cmp(&b.display.to_lowercase()))
    });

    entries
}

fn mention_display_for_path(root: &Path, path: &Path, is_dir: bool) -> String {
    let mut display = path
        .strip_prefix(root)
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.to_string_lossy().to_string())
        });

    if is_dir && !display.ends_with('/') {
        display.push('/');
    }

    display
}

fn refresh_add_picker_browser(
    list: &gtk::Box,
    path_label: &gtk::Label,
    root: &Path,
    current_dir: &Path,
    filter_query: &str,
    cached_dir: &Rc<RefCell<Option<PathBuf>>>,
    cached_entries: &Rc<RefCell<Vec<BrowserEntry>>>,
    workspace_search_root: &Rc<RefCell<Option<PathBuf>>>,
    workspace_search_entries: &Rc<RefCell<Vec<(String, String)>>>,
    entries_state: &Rc<RefCell<Vec<BrowserEntry>>>,
    on_entry_activated: &Rc<dyn Fn(usize)>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let query = filter_query.trim().to_ascii_lowercase();
    let entries = if query.is_empty() {
        let relative = current_dir
            .strip_prefix(root)
            .ok()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        if relative.is_empty() {
            path_label.set_label("./");
        } else {
            path_label.set_label(&format!("./{}/", relative));
        }

        if cached_dir.borrow().as_deref() != Some(current_dir) {
            cached_dir.replace(Some(current_dir.to_path_buf()));
            cached_entries.replace(collect_directory_entries(current_dir));
        }
        cached_entries.borrow().clone()
    } else {
        path_label.set_label("Workspace search");
        if workspace_search_root.borrow().as_deref() != Some(root)
            || workspace_search_entries.borrow().is_empty()
        {
            workspace_search_root.replace(Some(root.to_path_buf()));
            workspace_search_entries
                .replace(collect_workspace_files(root, MENTION_SCAN_RESULT_LIMIT));
        }
        workspace_search_entries
            .borrow()
            .iter()
            .filter(|(display, _)| display.to_ascii_lowercase().contains(&query))
            .take(250)
            .map(|(display, path)| BrowserEntry {
                display: display.clone(),
                path: PathBuf::from(path),
                is_dir: display.ends_with('/'),
            })
            .collect::<Vec<_>>()
    };

    entries_state.replace(entries.clone());
    for (index, entry) in entries.iter().enumerate() {
        let button = gtk::Button::new();
        button.set_has_frame(false);
        button.add_css_class("composer-attach-picker-row-button");
        button.set_halign(gtk::Align::Fill);
        button.set_hexpand(true);
        let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row_box.add_css_class("composer-attach-picker-row");

        let icon = gtk::Image::from_icon_name(if entry.is_dir {
            "folder-symbolic"
        } else {
            "file-code-symbolic"
        });
        icon.set_pixel_size(14);
        icon.add_css_class("composer-attach-picker-row-icon");
        icon.add_css_class("file-browser-icon");
        row_box.append(&icon);

        let label = gtk::Label::new(Some(&entry.display));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_wrap(false);
        label.set_single_line_mode(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.add_css_class("composer-attach-picker-row-label");
        row_box.append(&label);

        button.set_child(Some(&row_box));
        let on_entry_activated = on_entry_activated.clone();
        button.connect_clicked(move |_| on_entry_activated(index));
        list.append(&button);
    }

    if entries.is_empty() {
        let empty = gtk::Label::new(Some("No matching files"));
        empty.set_xalign(0.0);
        empty.add_css_class("composer-attach-picker-empty");
        list.append(&empty);
    }
}

fn collect_workspace_files(root: &Path, limit: usize) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut scanned_dirs = 0usize;
    let hard_limit = limit.min(MENTION_SCAN_RESULT_LIMIT);

    while let Some(dir) = stack.pop() {
        if out.len() >= hard_limit || scanned_dirs >= MENTION_SCAN_MAX_DIRS {
            break;
        }
        scanned_dirs += 1;

        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten().take(MENTION_SCAN_MAX_ENTRIES_PER_DIR) {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().to_string();

            if path.is_dir() {
                if is_ignored_dir_name(&file_name) {
                    continue;
                }

                let rel = path
                    .strip_prefix(root)
                    .ok()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| file_name.clone());
                if !rel.is_empty() {
                    out.push((format!("{}/", rel), path.to_string_lossy().to_string()));
                    if out.len() >= hard_limit {
                        break;
                    }
                }

                stack.push(path);
                continue;
            }

            if !path.is_file() {
                continue;
            }

            let rel = path
                .strip_prefix(root)
                .ok()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| file_name.clone());
            out.push((rel, path.to_string_lossy().to_string()));

            if out.len() >= hard_limit {
                break;
            }
        }
    }

    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out
}

fn refresh_mention_popup(
    mention_popover: &gtk::Popover,
    mention_listbox: &gtk::ListBox,
    filtered_mentions: &Rc<RefCell<Vec<(String, String)>>>,
    mention_files: &Rc<RefCell<Vec<(String, String)>>>,
    input_view: &gtk::TextView,
) {
    while let Some(child) = mention_listbox.first_child() {
        mention_listbox.remove(&child);
    }

    let Some((_, _, query)) = mention_query_range(input_view) else {
        mention_popover.popdown();
        filtered_mentions.replace(Vec::new());
        return;
    };

    let query = query.to_lowercase();
    let matches: Vec<(String, String)> = mention_files
        .borrow()
        .iter()
        .filter(|(display, _)| {
            if query.is_empty() {
                true
            } else {
                display.to_lowercase().contains(&query)
            }
        })
        .take(30)
        .cloned()
        .collect();

    if matches.is_empty() {
        mention_popover.popdown();
        filtered_mentions.replace(Vec::new());
        return;
    }

    for (display, _) in &matches {
        let row = gtk::ListBoxRow::new();
        row.set_selectable(true);
        row.set_activatable(true);

        let label = gtk::Label::new(Some(display));
        label.set_xalign(0.0);
        label.set_margin_start(8);
        label.set_margin_end(8);
        label.set_margin_top(4);
        label.set_margin_bottom(4);
        row.set_child(Some(&label));
        mention_listbox.append(&row);
    }

    filtered_mentions.replace(matches);
    if let Some(first_row) = mention_listbox.row_at_index(0) {
        mention_listbox.select_row(Some(&first_row));
    }
    mention_popover.popup();
}

fn move_mention_selection(listbox: &gtk::ListBox, scroll: &gtk::ScrolledWindow, delta: i32) {
    let current_index = listbox.selected_row().map(|row| row.index()).unwrap_or(-1);
    let mut count = 0;
    let mut probe = 0;
    while listbox.row_at_index(probe).is_some() {
        count += 1;
        probe += 1;
    }
    if count == 0 {
        return;
    }
    let base = if current_index < 0 { 0 } else { current_index };
    let next = (base + delta).clamp(0, count - 1);
    if let Some(row) = listbox.row_at_index(next) {
        listbox.select_row(Some(&row));

        let Some(bounds) = row.compute_bounds(listbox) else {
            return;
        };
        let row_top = bounds.y() as f64;
        let row_bottom = (bounds.y() + bounds.height()) as f64;

        let adjustment = scroll.vadjustment();
        let current_top = adjustment.value();
        let current_bottom = current_top + adjustment.page_size();

        let mut target = current_top;
        if row_top < current_top {
            target = row_top;
        } else if row_bottom > current_bottom {
            target = row_bottom - adjustment.page_size();
        }

        let min = adjustment.lower();
        let max = (adjustment.upper() - adjustment.page_size()).max(min);
        adjustment.set_value(target.clamp(min, max));
    }
}

fn apply_mention_key_input(
    input_view: &gtk::TextView,
    key: gtk::gdk::Key,
    state: gtk::gdk::ModifierType,
) -> bool {
    if state.intersects(
        gtk::gdk::ModifierType::CONTROL_MASK
            | gtk::gdk::ModifierType::ALT_MASK
            | gtk::gdk::ModifierType::META_MASK,
    ) {
        return false;
    }

    let buffer = input_view.buffer();

    if key == gtk::gdk::Key::BackSpace {
        if buffer.delete_selection(true, true) {
            return true;
        }

        let cursor = buffer.cursor_position();
        if cursor > 0 {
            let mut end = buffer.iter_at_offset(cursor);
            let mut start = end;
            if start.backward_char() {
                buffer.delete(&mut start, &mut end);
                return true;
            }
        }
        return false;
    }

    if key == gtk::gdk::Key::Delete {
        if buffer.delete_selection(true, true) {
            return true;
        }

        let cursor = buffer.cursor_position();
        let mut start = buffer.iter_at_offset(cursor);
        let mut end = start;
        if end.forward_char() {
            buffer.delete(&mut start, &mut end);
            return true;
        }
        return false;
    }

    if let Some(ch) = key.to_unicode() {
        if ch.is_control() {
            return false;
        }

        let _ = buffer.delete_selection(true, true);
        let mut insert_at = buffer.iter_at_offset(buffer.cursor_position());
        let mut encoded = [0u8; 4];
        let text = ch.encode_utf8(&mut encoded);
        buffer.insert(&mut insert_at, text);
        return true;
    }

    false
}

fn update_input_height(
    input_scroll: &gtk::ScrolledWindow,
    input_view: &gtk::TextView,
    min_height: i32,
    max_height: i32,
) {
    let buffer = input_view.buffer();
    let end = buffer.end_iter();
    let rect = input_view.iter_location(&end);

    let desired =
        (rect.y() + rect.height() + input_view.top_margin() + input_view.bottom_margin() + 8)
            .clamp(min_height, max_height);
    input_scroll.set_min_content_height(desired);
    input_scroll.set_max_content_height(max_height);
}

fn composer_setting_key(thread_id: &str, suffix: &str) -> String {
    format!("thread:{thread_id}:composer:{suffix}")
}

pub(crate) fn default_composer_setting_key(suffix: &str) -> String {
    format!("composer:default:{suffix}")
}

pub(crate) fn default_composer_setting_value(db: &AppDb, suffix: &str) -> Option<String> {
    db.get_setting(&default_composer_setting_key(suffix))
        .ok()
        .flatten()
}

pub(crate) fn save_default_composer_setting_value(db: &AppDb, suffix: &str, value: &str) {
    let _ = db.set_setting(&default_composer_setting_key(suffix), value);
}

fn thread_setting_value(db: &AppDb, thread_id: &str, suffix: &str) -> Option<String> {
    db.get_setting(&composer_setting_key(thread_id, suffix))
        .ok()
        .flatten()
}

fn save_thread_setting(db: &AppDb, thread_id: &str, suffix: &str, value: &str) {
    let _ = db.set_setting(&composer_setting_key(thread_id, suffix), value);
}

fn save_default_composer_setting(db: &AppDb, suffix: &str, value: &str) {
    save_default_composer_setting_value(db, suffix, value);
}

const FIRST_PROMPT_TITLE_PREVIEW_CHARS: usize = 30;

fn title_from_first_prompt(prompt: &str) -> Option<String> {
    let mut preview = String::with_capacity(FIRST_PROMPT_TITLE_PREVIEW_CHARS);
    let mut preview_len = 0usize;
    let mut pending_space = false;

    for ch in prompt.chars() {
        if ch.is_whitespace() {
            pending_space = preview_len > 0;
            continue;
        }

        if pending_space && preview_len < FIRST_PROMPT_TITLE_PREVIEW_CHARS {
            preview.push(' ');
            preview_len += 1;
        }
        pending_space = false;

        if preview_len >= FIRST_PROMPT_TITLE_PREVIEW_CHARS {
            break;
        }

        preview.push(ch);
        preview_len += 1;
        if preview_len >= FIRST_PROMPT_TITLE_PREVIEW_CHARS {
            break;
        }
    }

    if preview.is_empty() {
        None
    } else {
        Some(preview)
    }
}

#[cfg(test)]
mod tests {
    use super::title_from_first_prompt;

    #[test]
    fn first_prompt_title_collapses_whitespace_without_scanning_full_prompt() {
        assert_eq!(
            title_from_first_prompt("   hello    from\t\tcomposer\npreview   "),
            Some("hello from composer preview".to_string())
        );
    }

    #[test]
    fn first_prompt_title_is_bounded_to_thirty_chars() {
        let prompt = format!("{} trailing text that should not matter", "a".repeat(64));
        assert_eq!(title_from_first_prompt(&prompt), Some("a".repeat(30)));
    }
}
pub(crate) mod voice;
include!("composer/build_impl.rs");
