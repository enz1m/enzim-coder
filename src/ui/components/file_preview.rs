use crate::data::AppDb;
use gtk::prelude::*;
use rusqlite::params;
use sourceview5::prelude::{BufferExt, ViewExt};
use sourceview5::{
    Buffer as SourceBuffer, LanguageManager, StyleSchemeManager, View as SourceView,
};
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::SystemTime;

const PREVIEW_STYLE_SCHEME_ID: &str = "enzimcoder-preview-dark";

struct FilePreviewWindow {
    window: gtk::Window,
    source_buffer: SourceBuffer,
    source_view: SourceView,
    image_view: gtk::Picture,
    preview_stack: gtk::Stack,
    title_label: gtk::Label,
    meta_label: gtk::Label,
    current_preview_file: Rc<RefCell<Option<PathBuf>>>,
    diff_toggle: gtk::ToggleButton,
    diff_source_stack: gtk::Stack,
    checkpoint_list: gtk::ListBox,
    git_list: gtk::ListBox,
    diff_buffer: SourceBuffer,
    diff_meta_label: gtk::Label,
    checkpoint_entries: Rc<RefCell<Vec<CheckpointDiffEntry>>>,
    git_entries: Rc<RefCell<Vec<GitCommitEntry>>>,
}

#[derive(Clone, Debug)]
struct CheckpointDiffEntry {
    checkpoint_id: i64,
    turn_id: String,
    created_at: i64,
    diff_text: String,
}

#[derive(Clone, Debug)]
struct GitCommitEntry {
    commit: String,
    date: String,
    subject: String,
    repo_root: PathBuf,
    relative_path: String,
}

#[derive(Clone, Debug)]
struct DiffListEntry {
    icon_name: String,
    title: String,
    subtitle: String,
}

thread_local! {
    static FILE_PREVIEW_WINDOW: RefCell<Option<FilePreviewWindow>> = const { RefCell::new(None) };
}

pub fn open_file_preview(path: &Path) {
    open_file_preview_at(path, None, None);
}

pub fn open_file_preview_at(path: &Path, line: Option<u32>, column: Option<u32>) {
    FILE_PREVIEW_WINDOW.with(|slot| {
        if slot.borrow().is_none() {
            slot.replace(Some(build_preview_window()));
        }

        if let Some(preview_window) = slot.borrow().as_ref() {
            attach_to_active_app_window(&preview_window.window);
            preview_window
                .current_preview_file
                .replace(Some(path.to_path_buf()));
            set_preview_file(
                path,
                &preview_window.source_buffer,
                &preview_window.source_view,
                &preview_window.image_view,
                &preview_window.preview_stack,
                &preview_window.title_label,
                &preview_window.meta_label,
                line,
                column,
            );
            if preview_window.diff_toggle.is_active() {
                refresh_diff_sources_for_file(
                    path,
                    &preview_window.checkpoint_list,
                    &preview_window.git_list,
                    &preview_window.checkpoint_entries,
                    &preview_window.git_entries,
                    &preview_window.diff_buffer,
                    &preview_window.diff_meta_label,
                );
            }
            preview_window.window.present();
        }
    });
}

pub fn open_git_diff_preview(repo_root: &Path, file_path: &Path, status: &str) {
    FILE_PREVIEW_WINDOW.with(|slot| {
        if slot.borrow().is_none() {
            slot.replace(Some(build_preview_window()));
        }

        if let Some(preview_window) = slot.borrow().as_ref() {
            attach_to_active_app_window(&preview_window.window);
            preview_window
                .current_preview_file
                .replace(Some(file_path.to_path_buf()));
            set_preview_git_diff(
                repo_root,
                file_path,
                status,
                &preview_window.source_buffer,
                &preview_window.source_view,
                &preview_window.preview_stack,
                &preview_window.title_label,
                &preview_window.meta_label,
            );
            if preview_window.diff_toggle.is_active() {
                refresh_diff_sources_for_file(
                    file_path,
                    &preview_window.checkpoint_list,
                    &preview_window.git_list,
                    &preview_window.checkpoint_entries,
                    &preview_window.git_entries,
                    &preview_window.diff_buffer,
                    &preview_window.diff_meta_label,
                );
            }
            preview_window.window.present();
        }
    });
}

pub fn open_checkpoint_diff_preview(file_path: &Path, checkpoint_id: i64) {
    FILE_PREVIEW_WINDOW.with(|slot| {
        if slot.borrow().is_none() {
            slot.replace(Some(build_preview_window()));
        }

        if let Some(preview_window) = slot.borrow().as_ref() {
            attach_to_active_app_window(&preview_window.window);
            preview_window
                .current_preview_file
                .replace(Some(file_path.to_path_buf()));
            set_preview_file(
                file_path,
                &preview_window.source_buffer,
                &preview_window.source_view,
                &preview_window.image_view,
                &preview_window.preview_stack,
                &preview_window.title_label,
                &preview_window.meta_label,
                None,
                None,
            );
            if !preview_window.diff_toggle.is_active() {
                preview_window.diff_toggle.set_active(true);
            }
            preview_window
                .diff_source_stack
                .set_visible_child_name("checkpoints");
            refresh_diff_sources_for_file(
                file_path,
                &preview_window.checkpoint_list,
                &preview_window.git_list,
                &preview_window.checkpoint_entries,
                &preview_window.git_entries,
                &preview_window.diff_buffer,
                &preview_window.diff_meta_label,
            );
            let target_row = preview_window
                .checkpoint_entries
                .borrow()
                .iter()
                .position(|entry| entry.checkpoint_id == checkpoint_id)
                .and_then(|idx| preview_window.checkpoint_list.row_at_index(idx as i32));
            if let Some(row) = target_row {
                preview_window.checkpoint_list.select_row(Some(&row));
            }
            preview_window.window.present();
        }
    });
}

fn build_preview_window() -> FilePreviewWindow {
    let window = gtk::Window::builder()
        .title("Quick Preview")
        .default_width(920)
        .default_height(640)
        .modal(true)
        .destroy_with_parent(true)
        .build();
    window.set_hide_on_close(true);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.add_css_class("chat-frame");
    content.add_css_class("file-preview-card");
    content.set_margin_start(8);
    content.set_margin_end(8);
    content.set_margin_top(8);
    content.set_margin_bottom(8);

    let preview_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    preview_header.add_css_class("file-preview-header");

    let preview_title = gtk::Label::new(Some("Quick Preview"));
    preview_title.add_css_class("file-preview-title");
    preview_title.set_xalign(0.0);
    preview_title.set_hexpand(true);
    preview_title.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let preview_meta = gtk::Label::new(Some(""));
    preview_meta.add_css_class("file-preview-meta");
    preview_meta.set_xalign(1.0);

    let diff_toggle = gtk::ToggleButton::new();
    diff_toggle.add_css_class("app-flat-button");
    diff_toggle.add_css_class("file-preview-diff-toggle");
    let diff_toggle_content = gtk::Box::new(gtk::Orientation::Horizontal, 5);
    let diff_toggle_icon = gtk::Image::from_icon_name("view-dual-symbolic");
    diff_toggle_icon.set_pixel_size(14);
    let diff_toggle_label = gtk::Label::new(Some("Diff"));
    diff_toggle_content.append(&diff_toggle_icon);
    diff_toggle_content.append(&diff_toggle_label);
    diff_toggle.set_child(Some(&diff_toggle_content));
    diff_toggle.set_tooltip_text(Some("Show file diff sources"));

    let open_external = gtk::Button::new();
    open_external.add_css_class("app-flat-button");
    open_external.add_css_class("circular");
    open_external.set_icon_name("folder-open-symbolic");
    open_external.set_tooltip_text(Some("Open with default application"));

    preview_header.append(&preview_title);
    preview_header.append(&preview_meta);
    preview_header.append(&diff_toggle);
    preview_header.append(&open_external);

    let source_buffer = SourceBuffer::new(None);
    source_buffer.set_highlight_syntax(true);
    source_buffer.set_highlight_matching_brackets(true);
    apply_preview_style_scheme(&source_buffer);

    let source_view = SourceView::with_buffer(&source_buffer);
    source_view.set_editable(false);
    source_view.set_cursor_visible(false);
    source_view.set_monospace(true);
    source_view.set_show_line_numbers(true);
    source_view.set_tab_width(4);
    source_view.add_css_class("file-preview-source");

    let preview_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .child(&source_view)
        .build();
    preview_scroll.add_css_class("file-preview-scroll");

    let image_view = gtk::Picture::new();
    image_view.set_can_shrink(true);
    image_view.set_content_fit(gtk::ContentFit::Contain);
    image_view.add_css_class("file-preview-image");

    let image_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .child(&image_view)
        .build();
    image_scroll.add_css_class("file-preview-scroll");

    let preview_stack = gtk::Stack::new();
    preview_stack.add_named(&preview_scroll, Some("text"));
    preview_stack.add_named(&image_scroll, Some("image"));
    preview_stack.set_visible_child_name("text");
    preview_stack.set_hexpand(true);
    preview_stack.set_vexpand(true);

    let checkpoint_list = gtk::ListBox::new();
    checkpoint_list.set_selection_mode(gtk::SelectionMode::Single);
    checkpoint_list.add_css_class("file-preview-diff-list");

    let git_list = gtk::ListBox::new();
    git_list.set_selection_mode(gtk::SelectionMode::Single);
    git_list.add_css_class("file-preview-diff-list");

    let checkpoint_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(150)
        .child(&checkpoint_list)
        .build();
    checkpoint_scroll.add_css_class("file-preview-diff-list-scroll");

    let git_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(150)
        .child(&git_list)
        .build();
    git_scroll.add_css_class("file-preview-diff-list-scroll");

    let diff_source_stack = gtk::Stack::new();
    diff_source_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    diff_source_stack.add_titled(&checkpoint_scroll, Some("checkpoints"), "Checkpoints");
    diff_source_stack.add_titled(&git_scroll, Some("git"), "Git");
    diff_source_stack.set_visible_child_name("checkpoints");

    let diff_switcher = gtk::StackSwitcher::new();
    diff_switcher.add_css_class("file-preview-diff-switcher");
    diff_switcher.set_stack(Some(&diff_source_stack));
    diff_switcher.set_halign(gtk::Align::Start);

    let diff_panel_title = gtk::Label::new(Some("Diff Explorer"));
    diff_panel_title.add_css_class("file-preview-diff-title");
    diff_panel_title.set_xalign(0.0);

    let diff_panel_subtitle = gtk::Label::new(Some(
        "Compare this file against in-app checkpoints or git commits.",
    ));
    diff_panel_subtitle.add_css_class("file-preview-diff-subtitle");
    diff_panel_subtitle.set_xalign(0.0);
    diff_panel_subtitle.set_wrap(true);
    diff_panel_subtitle.set_wrap_mode(gtk::pango::WrapMode::WordChar);

    let diff_header = gtk::Box::new(gtk::Orientation::Vertical, 2);
    diff_header.append(&diff_panel_title);
    diff_header.append(&diff_panel_subtitle);

    let diff_meta = gtk::Label::new(Some("Pick an item to preview a diff."));
    diff_meta.add_css_class("file-preview-diff-meta");
    diff_meta.set_xalign(0.0);
    diff_meta.set_wrap(true);
    diff_meta.set_wrap_mode(gtk::pango::WrapMode::WordChar);

    let diff_buffer = SourceBuffer::new(None);
    diff_buffer.set_highlight_syntax(true);
    diff_buffer.set_highlight_matching_brackets(true);
    apply_preview_style_scheme(&diff_buffer);
    let manager = LanguageManager::default();
    diff_buffer.set_language(manager.language("diff").as_ref());

    let diff_view = SourceView::with_buffer(&diff_buffer);
    diff_view.set_editable(false);
    diff_view.set_cursor_visible(false);
    diff_view.set_monospace(true);
    diff_view.set_show_line_numbers(true);
    diff_view.set_tab_width(4);
    diff_view.add_css_class("file-preview-source");

    let diff_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .child(&diff_view)
        .build();
    diff_scroll.add_css_class("file-preview-scroll");

    let diff_panel = gtk::Box::new(gtk::Orientation::Vertical, 8);
    diff_panel.add_css_class("file-preview-diff-panel");
    diff_panel.set_size_request(430, -1);
    diff_panel.append(&diff_header);
    diff_panel.append(&diff_switcher);
    diff_panel.append(&diff_source_stack);
    diff_panel.append(&diff_meta);
    diff_panel.append(&diff_scroll);

    let diff_revealer = gtk::Revealer::new();
    diff_revealer.set_transition_type(gtk::RevealerTransitionType::SlideLeft);
    diff_revealer.set_transition_duration(160);
    diff_revealer.set_reveal_child(false);
    diff_revealer.set_visible(false);
    diff_revealer.set_child(Some(&diff_panel));

    let body = gtk::Paned::new(gtk::Orientation::Horizontal);
    body.add_css_class("file-preview-split");
    body.set_wide_handle(true);
    body.set_resize_start_child(true);
    body.set_resize_end_child(true);
    body.set_shrink_start_child(true);
    body.set_shrink_end_child(false);
    body.set_position(860);
    body.set_start_child(Some(&preview_stack));
    body.set_end_child(Some(&diff_revealer));

    content.append(&preview_header);
    content.append(&body);
    window.set_child(Some(&content));

    let current_preview_file = Rc::new(RefCell::new(None::<PathBuf>));
    let checkpoint_entries: Rc<RefCell<Vec<CheckpointDiffEntry>>> =
        Rc::new(RefCell::new(Vec::new()));
    let git_entries: Rc<RefCell<Vec<GitCommitEntry>>> = Rc::new(RefCell::new(Vec::new()));

    {
        let current_preview_file = current_preview_file.clone();
        open_external.connect_clicked(move |_| {
            if let Some(path) = current_preview_file.borrow().clone() {
                let _ = gtk::gio::AppInfo::launch_default_for_uri(
                    &format!("file://{}", path.to_string_lossy()),
                    None::<&gtk::gio::AppLaunchContext>,
                );
            }
        });
    }

    {
        let checkpoint_list = checkpoint_list.clone();
        let git_list = git_list.clone();
        diff_source_stack.connect_visible_child_name_notify(move |stack| {
            let page = stack.visible_child_name().map(|name| name.to_string());
            match page.as_deref() {
                Some("git") => {
                    if git_list.selected_row().is_none() {
                        if let Some(first_row) = git_list.row_at_index(0) {
                            git_list.select_row(Some(&first_row));
                        }
                    }
                }
                _ => {
                    if checkpoint_list.selected_row().is_none() {
                        if let Some(first_row) = checkpoint_list.row_at_index(0) {
                            checkpoint_list.select_row(Some(&first_row));
                        }
                    }
                }
            }
        });
    }

    {
        let checkpoint_entries = checkpoint_entries.clone();
        let diff_buffer = diff_buffer.clone();
        let diff_meta = diff_meta.clone();
        checkpoint_list.connect_row_selected(move |_, row| {
            let Some(row) = row else {
                return;
            };
            let idx = row.index().max(0) as usize;
            let entries = checkpoint_entries.borrow();
            let Some(entry) = entries.get(idx) else {
                return;
            };
            let manager = LanguageManager::default();
            diff_buffer.set_language(manager.language("diff").as_ref());
            if entry.diff_text.trim().is_empty() {
                diff_buffer.set_text("No diff available for this checkpoint.");
            } else {
                diff_buffer.set_text(&entry.diff_text);
            }
            diff_meta.set_text(&format!(
                "Checkpoint #{} • {} • {}",
                entry.checkpoint_id,
                format_relative_age(entry.created_at),
                compact_turn_label(&entry.turn_id)
            ));
        });
    }

    {
        let git_entries = git_entries.clone();
        let diff_buffer = diff_buffer.clone();
        let diff_meta = diff_meta.clone();
        git_list.connect_row_selected(move |_, row| {
            let Some(row) = row else {
                return;
            };
            let idx = row.index().max(0) as usize;
            let entries = git_entries.borrow();
            let Some(entry) = entries.get(idx) else {
                return;
            };
            let manager = LanguageManager::default();
            diff_buffer.set_language(manager.language("diff").as_ref());
            let diff_text =
                load_git_commit_file_diff(&entry.repo_root, &entry.commit, &entry.relative_path);
            diff_buffer.set_text(&diff_text);
            diff_meta.set_text(&format!(
                "{} ({}) • {}",
                entry.commit, entry.date, entry.subject
            ));
        });
    }

    {
        let window = window.clone();
        let diff_revealer = diff_revealer.clone();
        let current_preview_file = current_preview_file.clone();
        let checkpoint_list = checkpoint_list.clone();
        let git_list = git_list.clone();
        let checkpoint_entries = checkpoint_entries.clone();
        let git_entries = git_entries.clone();
        let diff_buffer = diff_buffer.clone();
        let diff_meta = diff_meta.clone();
        diff_toggle.connect_toggled(move |toggle| {
            let visible = toggle.is_active();
            if visible {
                diff_revealer.set_visible(true);
            }
            diff_revealer.set_reveal_child(visible);
            if visible {
                window.set_default_size(1400, 760);
                if let Some(path) = current_preview_file.borrow().clone() {
                    refresh_diff_sources_for_file(
                        &path,
                        &checkpoint_list,
                        &git_list,
                        &checkpoint_entries,
                        &git_entries,
                        &diff_buffer,
                        &diff_meta,
                    );
                }
            } else {
                window.set_default_size(920, 640);
                let diff_revealer = diff_revealer.clone();
                gtk::glib::timeout_add_local_once(
                    std::time::Duration::from_millis(180),
                    move || {
                        if !diff_revealer.reveals_child() {
                            diff_revealer.set_visible(false);
                        }
                    },
                );
            }
        });
    }

    {
        let window = window.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                window.set_visible(false);
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        source_view.add_controller(key_controller);
    }

    FilePreviewWindow {
        window,
        source_buffer,
        source_view,
        image_view,
        preview_stack,
        title_label: preview_title,
        meta_label: preview_meta,
        current_preview_file,
        diff_toggle,
        diff_source_stack,
        checkpoint_list,
        git_list,
        diff_buffer,
        diff_meta_label: diff_meta,
        checkpoint_entries,
        git_entries,
    }
}

fn attach_to_active_app_window(window: &gtk::Window) {
    let app = gtk::Application::default();
    if let Some(parent) = app.active_window() {
        window.set_transient_for(Some(&parent));
    }
}

fn set_preview_file(
    file_path: &Path,
    source_buffer: &SourceBuffer,
    source_view: &SourceView,
    image_view: &gtk::Picture,
    preview_stack: &gtk::Stack,
    title_label: &gtk::Label,
    meta_label: &gtk::Label,
    target_line: Option<u32>,
    target_column: Option<u32>,
) {
    source_view.set_show_line_numbers(true);

    let name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.replace('\0', ""))
        .unwrap_or_else(|| "Unknown".to_string());
    title_label.set_text(&name);

    let modified = fs::metadata(file_path)
        .ok()
        .and_then(|meta| meta.modified().ok());

    let path_str = file_path.to_string_lossy().replace('\0', "");
    let line_hint = target_line.map(|line| {
        if let Some(col) = target_column {
            format!(" • L{line}:{col}")
        } else {
            format!(" • L{line}")
        }
    });
    meta_label.set_text(&format!(
        "{} • {}{}",
        path_str,
        format_modified(modified),
        line_hint.unwrap_or_default()
    ));

    let extension = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_lowercase());

    let is_image = matches!(
        extension.as_deref(),
        Some("png")
            | Some("jpg")
            | Some("jpeg")
            | Some("gif")
            | Some("bmp")
            | Some("webp")
            | Some("svg")
            | Some("ico")
    );

    if is_image {
        image_view.set_filename(Some(file_path));
        preview_stack.set_visible_child_name("image");
        return;
    }

    preview_stack.set_visible_child_name("text");

    match fs::metadata(file_path) {
        Ok(metadata) => {
            let file_size = metadata.len();
            if file_size > 1_048_576 {
                let size_mb = file_size as f64 / 1_048_576.0;
                source_buffer.set_language(None::<&sourceview5::Language>);
                source_buffer.set_text(&format!(
                    "File is too large for preview ({:.2} MB).\n\nClick the folder icon to open with your default application.",
                    size_mb
                ));
                return;
            }
        }
        Err(_) => {
            source_buffer.set_language(None::<&sourceview5::Language>);
            source_buffer.set_text(
                "Unable to read file.\n\nClick the folder icon to open with your default application.",
            );
            return;
        }
    }

    match fs::read(file_path) {
        Ok(bytes) => {
            let is_binary = bytes.iter().take(512).any(|byte| *byte == 0);
            if is_binary {
                source_buffer.set_language(None::<&sourceview5::Language>);
                source_buffer.set_text("Binary file preview is not supported.\n\nClick the folder icon to open with your default application.");
                return;
            }

            let text = String::from_utf8_lossy(&bytes);
            let clean_text = text.replace('\0', "");
            let limited_text = if clean_text.len() > 300_000 {
                format!(
                    "{}\n\n--- Preview truncated to first 300KB ---",
                    &clean_text[..300_000]
                )
            } else {
                clean_text
            };

            let manager = LanguageManager::default();
            let guessed_language = manager.guess_language(file_path.to_str(), None::<&str>);
            source_buffer.set_language(guessed_language.as_ref());
            source_buffer.set_text(&limited_text);
            reveal_source_location_with_settle(
                source_buffer,
                source_view,
                target_line,
                target_column,
            );
        }
        Err(_) => {
            source_buffer.set_language(None::<&sourceview5::Language>);
            source_buffer.set_text(
                "Unable to read file.\n\nClick the folder icon to open with your default application.",
            );
        }
    }
}

fn reveal_source_location(
    source_buffer: &SourceBuffer,
    source_view: &SourceView,
    target_line: Option<u32>,
    target_column: Option<u32>,
) {
    let Some(line_1_based) = target_line else {
        return;
    };
    let line_count = source_buffer.line_count().max(1);
    let line_zero_based = (line_1_based.saturating_sub(1) as i32).clamp(0, line_count - 1);
    let line_start = source_buffer
        .iter_at_line(line_zero_based)
        .unwrap_or_else(|| source_buffer.start_iter());
    let mut iter = line_start;
    if let Some(column_1_based) = target_column {
        let mut line_end = line_start;
        let _ = line_end.forward_to_line_end();
        let line_len_chars = (line_end.offset() - line_start.offset()).max(0);
        let col_zero_based = column_1_based.saturating_sub(1) as i32;
        let advance = col_zero_based.min(line_len_chars);
        let _ = iter.forward_chars(advance);
    }
    source_buffer.place_cursor(&iter);
    source_view.scroll_to_iter(&mut iter, 0.0, true, 0.12, 0.0);
}

fn reveal_source_location_with_settle(
    source_buffer: &SourceBuffer,
    source_view: &SourceView,
    target_line: Option<u32>,
    target_column: Option<u32>,
) {
    reveal_source_location(source_buffer, source_view, target_line, target_column);

    let source_buffer_idle = source_buffer.clone();
    let source_view_idle = source_view.clone();
    gtk::glib::idle_add_local_once(move || {
        reveal_source_location(
            &source_buffer_idle,
            &source_view_idle,
            target_line,
            target_column,
        );
    });

    let source_buffer_late = source_buffer.clone();
    let source_view_late = source_view.clone();
    gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(40), move || {
        reveal_source_location(
            &source_buffer_late,
            &source_view_late,
            target_line,
            target_column,
        );
    });
}

fn set_preview_git_diff(
    repo_root: &Path,
    file_path: &Path,
    status: &str,
    source_buffer: &SourceBuffer,
    source_view: &SourceView,
    preview_stack: &gtk::Stack,
    title_label: &gtk::Label,
    meta_label: &gtk::Label,
) {
    let relative_path = file_path
        .strip_prefix(repo_root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .replace('\0', "");
    let display_name = if relative_path.is_empty() {
        file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .replace('\0', "")
    } else {
        relative_path.clone()
    };

    title_label.set_text(&display_name);
    meta_label.set_text(&format!("Git Diff • {}", relative_path));
    preview_stack.set_visible_child_name("text");

    let status_code = status.trim();
    let diff_text = if status_code == "??" {
        build_untracked_diff_preview(file_path, &relative_path)
    } else {
        run_git_diff(repo_root, &relative_path)
    };

    let manager = LanguageManager::default();
    source_buffer.set_language(manager.language("diff").as_ref());
    source_view.set_show_line_numbers(false);
    source_buffer.set_text(&render_git_diff_with_actual_line_numbers(&diff_text));
}

fn refresh_diff_sources_for_file(
    file_path: &Path,
    checkpoint_list: &gtk::ListBox,
    git_list: &gtk::ListBox,
    checkpoint_entries: &Rc<RefCell<Vec<CheckpointDiffEntry>>>,
    git_entries: &Rc<RefCell<Vec<GitCommitEntry>>>,
    diff_buffer: &SourceBuffer,
    diff_meta_label: &gtk::Label,
) {
    let (checkpoint_items, checkpoint_empty_message) = collect_checkpoint_diff_entries(file_path);
    checkpoint_entries.replace(checkpoint_items.clone());
    let checkpoint_rows = checkpoint_items
        .iter()
        .map(|entry| DiffListEntry {
            icon_name: "revert-symbolic".to_string(),
            title: format!(
                "{} • #{}",
                compact_turn_label(&entry.turn_id),
                entry.checkpoint_id
            ),
            subtitle: format_relative_age(entry.created_at),
        })
        .collect::<Vec<DiffListEntry>>();
    repopulate_diff_listbox(checkpoint_list, &checkpoint_rows, &checkpoint_empty_message);

    let (git_items, git_empty_message) = collect_git_commit_entries(file_path);
    git_entries.replace(git_items.clone());
    let git_rows = git_items
        .iter()
        .map(|entry| DiffListEntry {
            icon_name: "commit-symbolic".to_string(),
            title: format!("{} {}", entry.commit, entry.subject),
            subtitle: entry.date.clone(),
        })
        .collect::<Vec<DiffListEntry>>();
    repopulate_diff_listbox(git_list, &git_rows, &git_empty_message);

    let manager = LanguageManager::default();
    diff_buffer.set_language(manager.language("diff").as_ref());
    if !checkpoint_items.is_empty() {
        if let Some(row) = checkpoint_list.row_at_index(0) {
            checkpoint_list.select_row(Some(&row));
        }
    } else if let Some(first_git) = git_items.first() {
        let diff_text = load_git_commit_file_diff(
            &first_git.repo_root,
            &first_git.commit,
            &first_git.relative_path,
        );
        diff_buffer.set_text(&diff_text);
        diff_meta_label.set_text(&format!(
            "{} ({}) • {}",
            first_git.commit, first_git.date, first_git.subject
        ));
    } else {
        diff_buffer.set_text("No diff sources available for this file.");
        diff_meta_label.set_text("No local checkpoint or git history diff entries found.");
    }
}

fn repopulate_diff_listbox(listbox: &gtk::ListBox, entries: &[DiffListEntry], empty_text: &str) {
    while let Some(child) = listbox.first_child() {
        listbox.remove(&child);
    }

    if entries.is_empty() {
        let placeholder = gtk::Label::new(Some(empty_text));
        placeholder.add_css_class("file-preview-diff-empty");
        placeholder.set_xalign(0.0);
        placeholder.set_wrap(true);
        placeholder.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        listbox.set_placeholder(Some(&placeholder));
        return;
    }

    listbox.set_placeholder(Option::<&gtk::Widget>::None);
    for entry in entries {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("file-preview-diff-row");
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.set_margin_start(8);
        line.set_margin_end(8);
        line.set_margin_top(6);
        line.set_margin_bottom(6);

        let icon = gtk::Image::from_icon_name(entry.icon_name.as_str());
        icon.set_pixel_size(13);
        icon.add_css_class("file-preview-diff-row-icon");
        icon.set_valign(gtk::Align::Center);

        let text_col = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text_col.set_hexpand(true);

        let title_label = gtk::Label::new(Some(entry.title.as_str()));
        title_label.set_xalign(0.0);
        title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title_label.add_css_class("file-preview-diff-row-title");

        let subtitle_label = gtk::Label::new(Some(entry.subtitle.as_str()));
        subtitle_label.set_xalign(0.0);
        subtitle_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        subtitle_label.add_css_class("file-preview-diff-row-subtitle");

        text_col.append(&title_label);
        text_col.append(&subtitle_label);
        line.append(&icon);
        line.append(&text_col);
        row.set_child(Some(&line));
        listbox.append(&row);
    }
}

fn collect_checkpoint_diff_entries(file_path: &Path) -> (Vec<CheckpointDiffEntry>, String) {
    let db = AppDb::open_default();
    let Some(local_thread_id) = db
        .get_setting("last_active_thread_id")
        .ok()
        .flatten()
        .and_then(|raw| raw.parse::<i64>().ok())
    else {
        return (
            Vec::new(),
            "No active thread for checkpoint diffs.".to_string(),
        );
    };
    let Some(thread) = db.get_thread_record(local_thread_id).ok().flatten() else {
        return (
            Vec::new(),
            "Active thread record is unavailable.".to_string(),
        );
    };
    let Some(remote_thread_id) = thread
        .remote_thread_id()
        .filter(|id| !id.trim().is_empty())
        .map(|id| id.to_string())
    else {
        return (
            Vec::new(),
            "Thread has no remote runtime id yet. Checkpoint diffs unavailable.".to_string(),
        );
    };
    let Some(workspace_path) = db
        .workspace_path_for_remote_thread(&remote_thread_id)
        .ok()
        .flatten()
    else {
        return (
            Vec::new(),
            "Workspace path not found for active thread.".to_string(),
        );
    };
    let workspace_root = PathBuf::from(workspace_path);
    let Ok(relative_path) = file_path.strip_prefix(&workspace_root) else {
        return (
            Vec::new(),
            "Current file is outside the active thread workspace.".to_string(),
        );
    };
    let relative_path = normalize_relative_path(relative_path);

    let mut items = Vec::new();
    let conn = db.connection();
    let conn = conn.borrow();
    let mut stmt = match conn.prepare(
        "SELECT c.id, c.turn_id, c.created_at, s.git_dir, s.before_tree, s.after_tree
         FROM restore_checkpoints c
         INNER JOIN restore_git_states s ON s.checkpoint_id = c.id
         WHERE c.codex_thread_id = ?1
           AND c.turn_id NOT LIKE 'restore-%'
         ORDER BY c.created_at DESC, c.id DESC",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return (Vec::new(), "Unable to load checkpoint history.".to_string()),
    };
    let rows = match stmt.query_map(params![remote_thread_id], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    }) {
        Ok(rows) => rows,
        Err(_) => {
            return (
                Vec::new(),
                "Unable to decode checkpoint history.".to_string(),
            );
        }
    };

    for row in rows.flatten() {
        let (checkpoint_id, turn_id, created_at, git_dir, before_tree, after_tree) = row;
        let diff_text = run_git_text_with_git_dir(
            &workspace_root,
            Path::new(&git_dir),
            &[
                "diff",
                "--no-color",
                before_tree.as_str(),
                after_tree.as_str(),
                "--",
                relative_path.as_str(),
            ],
        )
        .unwrap_or_default();
        if diff_text.trim().is_empty() {
            continue;
        }
        items.push(CheckpointDiffEntry {
            checkpoint_id,
            turn_id,
            created_at,
            diff_text,
        });
    }

    let empty = if items.is_empty() {
        "No checkpoint diffs found for this file.".to_string()
    } else {
        String::new()
    };
    (items, empty)
}

fn collect_git_commit_entries(file_path: &Path) -> (Vec<GitCommitEntry>, String) {
    let Some(file_dir) = file_path.parent() else {
        return (Vec::new(), "Unable to resolve file directory.".to_string());
    };
    let Some(repo_root_raw) = run_git_text_with_cwd(file_dir, &["rev-parse", "--show-toplevel"])
    else {
        return (
            Vec::new(),
            "Git repository not available for this file.".to_string(),
        );
    };
    let repo_root = PathBuf::from(repo_root_raw.trim());
    let Ok(relative_path) = file_path.strip_prefix(&repo_root) else {
        return (
            Vec::new(),
            "Current file is outside the detected git repository.".to_string(),
        );
    };
    let relative_path = normalize_relative_path(relative_path);
    let Some(log_text) = run_git_text_with_cwd(
        &repo_root,
        &[
            "log",
            "--date=short",
            "--pretty=format:%h\t%ad\t%s",
            "-n",
            "120",
            "--",
            relative_path.as_str(),
        ],
    ) else {
        return (
            Vec::new(),
            "Unable to read git history for this file.".to_string(),
        );
    };
    if log_text.trim().is_empty() {
        return (
            Vec::new(),
            "No git history entries for this file.".to_string(),
        );
    }

    let mut entries = Vec::new();
    for line in log_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(3, '\t');
        let Some(commit) = parts.next() else {
            continue;
        };
        let Some(date) = parts.next() else {
            continue;
        };
        let Some(subject) = parts.next() else {
            continue;
        };
        entries.push(GitCommitEntry {
            commit: commit.to_string(),
            date: date.to_string(),
            subject: subject.to_string(),
            repo_root: repo_root.clone(),
            relative_path: relative_path.clone(),
        });
    }
    let empty = if entries.is_empty() {
        "No git history entries for this file.".to_string()
    } else {
        String::new()
    };
    (entries, empty)
}

fn load_git_commit_file_diff(repo_root: &Path, commit: &str, relative_path: &str) -> String {
    let diff_text = run_git_text_with_cwd(
        repo_root,
        &[
            "show",
            "--no-color",
            "--pretty=format:",
            commit,
            "--",
            relative_path,
        ],
    )
    .unwrap_or_default();
    if diff_text.trim().is_empty() {
        "No diff available for this commit and file.".to_string()
    } else {
        diff_text
    }
}

fn run_git_text_with_cwd(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|text| text.replace('\0', ""))
}

fn run_git_text_with_git_dir(worktree: &Path, git_dir: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .arg("--git-dir")
        .arg(git_dir)
        .arg("--work-tree")
        .arg(worktree);
    command.args(args);
    let output = command.current_dir(worktree).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|text| text.replace('\0', ""))
}

fn normalize_relative_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().replace('\0', ""))
        .collect::<Vec<String>>()
        .join("/")
}

fn run_git_diff(repo_root: &Path, relative_path: &str) -> String {
    let mut diff_text = run_git_command_text(repo_root, &["diff", "--", relative_path]);
    if diff_text.trim().is_empty() {
        diff_text = run_git_command_text(repo_root, &["diff", "--cached", "--", relative_path]);
    }

    if diff_text.trim().is_empty() {
        "No diff available for this file.".to_string()
    } else {
        diff_text
    }
}

fn run_git_command_text(repo_root: &Path, args: &[&str]) -> String {
    crate::git_exec::run_git_text(repo_root, args)
        .unwrap_or_else(|_| "Unable to run git diff preview.".to_string())
}

fn build_untracked_diff_preview(file_path: &Path, relative_path: &str) -> String {
    match fs::read(file_path) {
        Ok(bytes) => {
            if bytes.iter().take(512).any(|byte| *byte == 0) {
                return format!(
                    "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n@@\n+Binary file preview is not supported.\n",
                    relative_path
                );
            }

            let text = String::from_utf8_lossy(&bytes).replace('\0', "");
            let mut diff = format!(
                "diff --git a/{0} b/{0}\nnew file mode 100644\n--- /dev/null\n+++ b/{0}\n",
                relative_path
            );

            if text.is_empty() {
                diff.push_str("@@ -0,0 +1,1 @@\n+\n");
            } else {
                diff.push_str(&format!("@@ -0,0 +1,{} @@\n", text.lines().count().max(1)));
                for line in text.lines() {
                    diff.push('+');
                    diff.push_str(line);
                    diff.push('\n');
                }
            }
            diff
        }
        Err(_) => "No diff available for this file.".to_string(),
    }
}

fn render_git_diff_with_actual_line_numbers(diff_text: &str) -> String {
    let mut rendered = String::new();
    let mut old_line = 0usize;
    let mut new_line = 0usize;
    let mut in_hunk = false;

    for line in diff_text.lines() {
        if let Some((old_start, new_start)) = parse_diff_hunk_starts(line) {
            old_line = old_start;
            new_line = new_start;
            in_hunk = true;
            rendered.push_str(&format!("{:>6} {:>6}  {}\n", "", "", line));
            continue;
        }

        if !in_hunk
            || line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("new file mode ")
            || line.starts_with("deleted file mode ")
            || line.starts_with("similarity index ")
            || line.starts_with("rename from ")
            || line.starts_with("rename to ")
        {
            rendered.push_str(&format!("{:>6} {:>6}  {}\n", "", "", line));
            continue;
        }

        let (old_display, new_display) = if line.starts_with('+') && !line.starts_with("+++") {
            let current = (None, Some(new_line));
            new_line = new_line.saturating_add(1);
            current
        } else if line.starts_with('-') && !line.starts_with("---") {
            let current = (Some(old_line), None);
            old_line = old_line.saturating_add(1);
            current
        } else {
            let current = (Some(old_line), Some(new_line));
            old_line = old_line.saturating_add(1);
            new_line = new_line.saturating_add(1);
            current
        };

        rendered.push_str(&format!(
            "{:>6} {:>6}  {}\n",
            format_diff_line_number(old_display),
            format_diff_line_number(new_display),
            line
        ));
    }

    if rendered.is_empty() {
        diff_text.to_string()
    } else {
        rendered
    }
}

fn format_diff_line_number(value: Option<usize>) -> String {
    value.map(|line| line.to_string()).unwrap_or_default()
}

fn parse_diff_hunk_starts(line: &str) -> Option<(usize, usize)> {
    let body = line.strip_prefix("@@ -")?;
    let (old_part, remainder) = body.split_once(" +")?;
    let (new_part, _) = remainder.split_once(" @@")?;
    Some((
        parse_diff_hunk_start_value(old_part)?,
        parse_diff_hunk_start_value(new_part)?,
    ))
}

fn parse_diff_hunk_start_value(part: &str) -> Option<usize> {
    let start = part.split_once(',').map(|(value, _)| value).unwrap_or(part);
    start.parse::<usize>().ok()
}

fn compact_turn_label(turn_id: &str) -> String {
    if turn_id.len() > 14 {
        format!("Turn {}", &turn_id[turn_id.len() - 14..])
    } else {
        format!("Turn {turn_id}")
    }
}

fn format_relative_age(unix_ts: i64) -> String {
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let diff = now.saturating_sub(unix_ts.max(0));
    if diff < 60 {
        "now".to_string()
    } else if diff < 3_600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86_400 {
        format!("{}h ago", diff / 3_600)
    } else if diff < 604_800 {
        format!("{}d ago", diff / 86_400)
    } else {
        format!("{}w ago", diff / 604_800)
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

fn apply_preview_style_scheme(source_buffer: &SourceBuffer) {
    let scheme_manager = StyleSchemeManager::default();
    register_preview_style_scheme(&scheme_manager);

    let preferred_schemes = [
        PREVIEW_STYLE_SCHEME_ID,
        "oblivion",
        "solarized-dark",
        "Adwaita-dark",
        "adwaita-dark",
        "classic-dark",
    ];

    let scheme = preferred_schemes
        .iter()
        .find_map(|scheme_id| scheme_manager.scheme(scheme_id))
        .or_else(|| {
            scheme_manager
                .scheme_ids()
                .into_iter()
                .find(|scheme_id| scheme_id.to_ascii_lowercase().contains("dark"))
                .and_then(|scheme_id| scheme_manager.scheme(scheme_id.as_str()))
        });

    if let Some(scheme) = scheme {
        source_buffer.set_style_scheme(Some(&scheme));
    }
}

fn register_preview_style_scheme(scheme_manager: &StyleSchemeManager) {
    static PREVIEW_SCHEME_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

    let preview_parent_scheme = [
        "oblivion",
        "solarized-dark",
        "Adwaita-dark",
        "adwaita-dark",
        "classic-dark",
    ]
    .iter()
    .find(|scheme_id| scheme_manager.scheme(scheme_id).is_some())
    .copied();

    let preview_scheme_dir = PREVIEW_SCHEME_DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join("enzimcoder").join("gtksourceview5");
        if fs::create_dir_all(&dir).is_err() {
            return None;
        }

        let parent_attr = preview_parent_scheme
            .map(|scheme| format!(" parent-scheme=\"{scheme}\""))
            .unwrap_or_default();

        let scheme_xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<style-scheme id=\"{id}\" name=\"Enzimcoder Preview Dark\" version=\"1.0\"{parent}>\n  <author>Enzimcoder</author>\n  <description>Deterministic dark background for quick preview.</description>\n  <color name=\"bg\" value=\"#17181c\"/>\n  <color name=\"fg\" value=\"#e6e6e6\"/>\n  <color name=\"line_bg\" value=\"#13141a\"/>\n  <color name=\"line_fg\" value=\"#7f8798\"/>\n  <color name=\"line_border\" value=\"#222633\"/>\n  <color name=\"line_current_fg\" value=\"#b9c1d1\"/>\n  <color name=\"selection\" value=\"#2d3342\"/>\n  <color name=\"selection_unfocused\" value=\"#2a2f3c\"/>\n  <color name=\"cursor\" value=\"#e6e6e6\"/>\n  <color name=\"current_line\" value=\"#1e212b\"/>\n  <style name=\"text\" foreground=\"fg\" background=\"bg\"/>\n  <style name=\"line-numbers\" foreground=\"line_fg\" background=\"line_bg\"/>\n  <style name=\"line-numbers-border\" background=\"line_border\"/>\n  <style name=\"current-line\" background=\"current_line\"/>\n  <style name=\"current-line-number\" foreground=\"line_current_fg\" background=\"line_bg\" bold=\"true\"/>\n  <style name=\"selection\" foreground=\"fg\" background=\"selection\"/>\n  <style name=\"selection-unfocused\" foreground=\"fg\" background=\"selection_unfocused\"/>\n  <style name=\"cursor\" foreground=\"cursor\"/>\n</style-scheme>\n",
            id = PREVIEW_STYLE_SCHEME_ID,
            parent = parent_attr
        );

        let scheme_file_path = dir.join(format!("{PREVIEW_STYLE_SCHEME_ID}.xml"));
        if fs::write(scheme_file_path, scheme_xml).is_err() {
            return None;
        }

        Some(dir)
    });

    if let Some(preview_scheme_dir) = preview_scheme_dir {
        let preview_scheme_dir = preview_scheme_dir.to_string_lossy().to_string();
        let existing_search_path = scheme_manager.search_path();
        let already_registered = existing_search_path
            .iter()
            .any(|path| path.as_str() == preview_scheme_dir);
        if !already_registered {
            scheme_manager.prepend_search_path(&preview_scheme_dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_diff_hunk_starts, render_git_diff_with_actual_line_numbers};

    #[test]
    fn parses_unified_diff_hunk_headers() {
        assert_eq!(parse_diff_hunk_starts("@@ -12,3 +40,5 @@"), Some((12, 40)));
        assert_eq!(parse_diff_hunk_starts("@@ -1 +9 @@"), Some((1, 9)));
        assert_eq!(parse_diff_hunk_starts("not a hunk"), None);
    }

    #[test]
    fn renders_actual_old_and_new_line_numbers_for_diff_lines() {
        let diff = "\
diff --git a/sample.rs b/sample.rs
@@ -10,3 +20,4 @@
 context
-old line
+new line
+newer line
";
        let rendered = render_git_diff_with_actual_line_numbers(diff);
        assert!(
            rendered.lines().any(|line| line.contains("10")
                && line.contains("20")
                && line.ends_with(" context"))
        );
        assert!(
            rendered
                .lines()
                .any(|line| line.contains("11") && line.ends_with("-old line"))
        );
        assert!(
            rendered
                .lines()
                .any(|line| line.contains("21") && line.ends_with("+new line"))
        );
        assert!(
            rendered
                .lines()
                .any(|line| line.contains("22") && line.ends_with("+newer line"))
        );
    }
}
