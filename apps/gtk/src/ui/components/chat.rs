use crate::services::app::runtime::RuntimeClient;
use crate::services::app::CodexProfileManager;
use crate::services::app::chat::AppDb;
use crate::ui::widget_tree;
use adw::prelude::*;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
use std::time::SystemTime;

mod codex_events;
mod codex_runtime;
pub(crate) mod composer;
mod history;
mod markdown;
mod message_render;
mod profile_selector;
pub(crate) mod runtime_controls;

struct TurnUi {
    bubble: gtk::Box,
    status_row: gtk::Box,
    status_label: gtk::Label,
    runtime_status_text: Option<String>,
    timestamp_label: gtk::Label,
    timestamp_revealer: gtk::Revealer,
    in_progress: bool,
    body_box: gtk::Box,
    text_widgets: HashMap<String, gtk::Label>,
    streaming_text_widgets: HashMap<String, message_render::StreamingMarkdownUi>,
    text_buffers: HashMap<String, String>,
    text_pending_deltas: HashMap<String, String>,
    agent_message_item_ids: HashSet<String>,
    reasoning_item_ids: HashSet<String>,
    status_buffers: HashMap<String, String>,
    status_last_text: String,
    status_last_updated_micros: i64,
    reasoning_started_micros: HashMap<String, i64>,
    pending_items: HashMap<String, String>,
    command_widgets: HashMap<String, message_render::CommandUi>,
    file_change_widgets: HashMap<String, gtk::Box>,
    tool_call_widgets: HashMap<String, message_render::ToolCallUi>,
    generic_item_widgets: HashMap<String, message_render::GenericItemUi>,
}

pub(super) fn clear_messages(messages_box: &gtk::Box) {
    widget_tree::clear_box_children(messages_box);
}

fn create_selector_menu(
    current_label: &str,
    options: &[(String, String)],
    selected_value: Rc<RefCell<String>>,
    selected_label: Option<Rc<dyn Fn(&str, &str) -> String>>,
    on_change: Option<Rc<dyn Fn(String)>>,
    position: gtk::PositionType,
) -> (gtk::Button, Rc<dyn Fn(&str)>) {
    let button = gtk::Button::new();
    button.set_widget_name("composer-selector-button");
    button.set_has_frame(false);
    button.add_css_class("composer-selector-button");

    let selector = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    selector.add_css_class("compact-selector");
    selector.set_valign(gtk::Align::Center);

    let label_widget = gtk::Label::new(Some(current_label));
    label_widget.set_widget_name("compact-selector-label");
    label_widget.add_css_class("compact-selector-label");
    selector.append(&label_widget);

    let arrow = gtk::Image::from_icon_name("pan-down-symbolic");
    arrow.set_widget_name("compact-selector-arrow");
    arrow.set_pixel_size(10);
    arrow.add_css_class("compact-selector-arrow");
    selector.append(&arrow);

    button.set_child(Some(&selector));

    let popover = gtk::Popover::new();
    popover.set_widget_name("compact-selector-popover");
    popover.set_has_arrow(true);
    popover.set_autohide(true);
    popover.set_position(position);
    popover.add_css_class("compact-selector-popover");
    popover.set_parent(&button);
    {
        let popover = popover.clone();
        button.connect_destroy(move |_| {
            popover.popdown();
            popover.unparent();
        });
    }

    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.add_css_class("compact-selector-menu");
    list.set_margin_start(4);
    list.set_margin_end(4);
    list.set_margin_top(4);
    list.set_margin_bottom(4);

    for (display_name, value) in options {
        let item_button = gtk::Button::new();
        item_button.set_widget_name("compact-selector-item");
        item_button.set_has_frame(false);
        item_button.add_css_class("compact-selector-item");
        item_button.set_halign(gtk::Align::Fill);
        item_button.set_hexpand(true);

        let item_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        item_row.set_hexpand(true);
        let item_label = gtk::Label::new(Some(display_name));
        item_label.set_widget_name("compact-selector-item-label");
        item_label.set_xalign(0.0);
        item_label.set_hexpand(true);
        item_row.append(&item_label);
        item_button.set_child(Some(&item_row));

        let selected_value = selected_value.clone();
        let popover = popover.clone();
        let label_widget = label_widget.clone();
        let display_name = display_name.clone();
        let value = value.clone();
        let selected_label = selected_label.clone();
        let on_change = on_change.clone();
        item_button.connect_clicked(move |_| {
            selected_value.replace(value.clone());
            let next_label = selected_label
                .as_ref()
                .map(|formatter| formatter(&display_name, &value))
                .unwrap_or_else(|| display_name.clone());
            label_widget.set_text(&next_label);
            if let Some(on_change) = on_change.as_ref() {
                on_change(value.clone());
            }
            popover.popdown();
        });
        list.append(&item_button);
    }

    let list_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_width(260)
        .max_content_height(320)
        .propagate_natural_height(true)
        .child(&list)
        .build();
    list_scroll.set_widget_name("compact-selector-scroll");
    list_scroll.set_has_frame(false);
    list_scroll.add_css_class("compact-selector-scroll");

    popover.set_child(Some(&list_scroll));
    {
        let popover = popover.clone();
        button.connect_clicked(move |_| {
            if popover.is_visible() {
                popover.popdown();
            } else {
                popover.popup();
            }
        });
    }

    let options_for_setter = options.to_vec();
    let selected_value_for_setter = selected_value.clone();
    let label_widget_for_setter = label_widget.clone();
    let setter: Rc<dyn Fn(&str)> = Rc::new(move |next_value: &str| {
        if let Some((display_name, value)) = options_for_setter
            .iter()
            .find(|(_, value)| value == next_value)
        {
            selected_value_for_setter.replace(value.clone());
            let next_label = selected_label
                .as_ref()
                .map(|formatter| formatter(display_name, value))
                .unwrap_or_else(|| display_name.clone());
            label_widget_for_setter.set_text(&next_label);
        }
    });

    (button, setter)
}

pub(super) fn create_grouped_selector_menu(
    current_label: &str,
    options: &[(String, String)],
    selected_value: Rc<RefCell<String>>,
    selected_label: Option<Rc<dyn Fn(&str, &str) -> String>>,
    on_change: Option<Rc<dyn Fn(String)>>,
    position: gtk::PositionType,
) -> (gtk::Button, Rc<dyn Fn(&str)>) {
    let button = gtk::Button::new();
    button.set_widget_name("composer-selector-button");
    button.set_has_frame(false);
    button.add_css_class("composer-selector-button");

    let selector = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    selector.add_css_class("compact-selector");
    selector.set_valign(gtk::Align::Center);

    let label_widget = gtk::Label::new(Some(current_label));
    label_widget.set_widget_name("compact-selector-label");
    label_widget.add_css_class("compact-selector-label");
    selector.append(&label_widget);

    let arrow = gtk::Image::from_icon_name("pan-down-symbolic");
    arrow.set_widget_name("compact-selector-arrow");
    arrow.set_pixel_size(10);
    arrow.add_css_class("compact-selector-arrow");
    selector.append(&arrow);
    button.set_child(Some(&selector));

    let popover = gtk::Popover::new();
    popover.set_widget_name("compact-selector-popover");
    popover.set_has_arrow(true);
    popover.set_autohide(true);
    popover.set_position(position);
    popover.add_css_class("compact-selector-popover");
    popover.set_parent(&button);
    {
        let popover = popover.clone();
        button.connect_destroy(move |_| {
            popover.popdown();
            popover.unparent();
        });
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.set_margin_start(4);
    root.set_margin_end(4);
    root.set_margin_top(4);
    root.set_margin_bottom(4);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let back_button = gtk::Button::with_label("Back");
    back_button.set_has_frame(false);
    back_button.add_css_class("compact-selector-item");
    back_button.set_visible(false);
    let header_label = gtk::Label::new(Some("Providers"));
    header_label.add_css_class("compact-selector-label");
    header_label.set_xalign(0.0);
    header_label.set_hexpand(true);
    header.append(&back_button);
    header.append(&header_label);
    root.append(&header);

    let pages = gtk::Stack::new();
    pages.set_hhomogeneous(false);
    pages.set_vhomogeneous(false);
    pages.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    pages.set_transition_duration(160);

    let provider_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    provider_list.add_css_class("compact-selector-menu");
    let model_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    model_list.add_css_class("compact-selector-menu");
    pages.add_named(&provider_list, Some("providers"));
    pages.add_named(&model_list, Some("models"));
    pages.set_visible_child_name("providers");

    let pages_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_width(260)
        .max_content_height(320)
        .propagate_natural_height(true)
        .child(&pages)
        .build();
    pages_scroll.set_widget_name("compact-selector-scroll");
    pages_scroll.set_has_frame(false);
    pages_scroll.add_css_class("compact-selector-scroll");
    root.append(&pages_scroll);
    popover.set_child(Some(&root));

    let mut grouped = BTreeMap::<String, Vec<(String, String)>>::new();
    for (display_name, value) in options {
        let (provider_name, model_name) = display_name
            .split_once(" / ")
            .map(|(provider, model)| (provider.to_string(), model.to_string()))
            .unwrap_or_else(|| ("Other".to_string(), display_name.clone()));
        grouped
            .entry(provider_name)
            .or_default()
            .push((model_name, value.clone()));
    }
    for model_entries in grouped.values_mut() {
        model_entries.sort_by(|left, right| left.0.cmp(&right.0));
    }

    let render_model_list: Rc<dyn Fn(&str)> = {
        let grouped = grouped.clone();
        let model_list = model_list.clone();
        let pages = pages.clone();
        let header_label = header_label.clone();
        let back_button = back_button.clone();
        let pages_scroll = pages_scroll.clone();
        let selected_value = selected_value.clone();
        let label_widget = label_widget.clone();
        let selected_label = selected_label.clone();
        let on_change = on_change.clone();
        let popover = popover.clone();
        Rc::new(move |provider_name: &str| {
            while let Some(child) = model_list.first_child() {
                model_list.remove(&child);
            }
            header_label.set_text(provider_name);
            back_button.set_visible(true);
            pages.set_visible_child_name("models");
            pages_scroll.vadjustment().set_value(0.0);
            for (model_name, value) in grouped.get(provider_name).cloned().unwrap_or_default() {
                let item_button = gtk::Button::new();
                item_button.set_widget_name("compact-selector-item");
                item_button.set_has_frame(false);
                item_button.add_css_class("compact-selector-item");
                item_button.set_halign(gtk::Align::Fill);
                item_button.set_hexpand(true);

                let item_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                item_row.set_hexpand(true);
                let item_label = gtk::Label::new(Some(&model_name));
                item_label.set_widget_name("compact-selector-item-label");
                item_label.set_xalign(0.0);
                item_label.set_hexpand(true);
                item_row.append(&item_label);
                item_button.set_child(Some(&item_row));

                let selected_value = selected_value.clone();
                let label_widget = label_widget.clone();
                let selected_label = selected_label.clone();
                let on_change = on_change.clone();
                let popover = popover.clone();
                let full_display_name = format!("{provider_name} / {model_name}");
                item_button.connect_clicked(move |_| {
                    selected_value.replace(value.clone());
                    let next_label = selected_label
                        .as_ref()
                        .map(|formatter| formatter(&full_display_name, &value))
                        .unwrap_or_else(|| full_display_name.clone());
                    label_widget.set_text(&next_label);
                    if let Some(on_change) = on_change.as_ref() {
                        on_change(value.clone());
                    }
                    popover.popdown();
                });
                model_list.append(&item_button);
            }
        })
    };

    for (provider_name, model_entries) in &grouped {
        let item_button = gtk::Button::new();
        item_button.set_widget_name("compact-selector-item");
        item_button.set_has_frame(false);
        item_button.add_css_class("compact-selector-item");
        item_button.set_halign(gtk::Align::Fill);
        item_button.set_hexpand(true);

        let item_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        item_row.set_hexpand(true);
        let item_label = gtk::Label::new(Some(provider_name));
        item_label.set_widget_name("compact-selector-item-label");
        item_label.set_xalign(0.0);
        item_label.set_hexpand(true);
        let item_count = gtk::Label::new(Some(&format!("{}", model_entries.len())));
        item_count.add_css_class("dim-label");
        item_row.append(&item_label);
        item_row.append(&item_count);
        item_button.set_child(Some(&item_row));

        let render_model_list = render_model_list.clone();
        let provider_name = provider_name.clone();
        item_button.connect_clicked(move |_| {
            render_model_list(&provider_name);
        });
        provider_list.append(&item_button);
    }

    {
        let pages = pages.clone();
        let header_label = header_label.clone();
        let back_button = back_button.clone();
        let back_button_for_click = back_button.clone();
        let pages_scroll = pages_scroll.clone();
        back_button.connect_clicked(move |_| {
            pages.set_visible_child_name("providers");
            header_label.set_text("Providers");
            back_button_for_click.set_visible(false);
            pages_scroll.vadjustment().set_value(0.0);
        });
    }

    {
        let popover = popover.clone();
        let pages = pages.clone();
        let header_label = header_label.clone();
        let back_button = back_button.clone();
        let pages_scroll = pages_scroll.clone();
        button.connect_clicked(move |_| {
            if popover.is_visible() {
                popover.popdown();
            } else {
                pages.set_visible_child_name("providers");
                header_label.set_text("Providers");
                back_button.set_visible(false);
                pages_scroll.vadjustment().set_value(0.0);
                popover.popup();
            }
        });
    }

    let options_for_setter = options.to_vec();
    let selected_value_for_setter = selected_value.clone();
    let label_widget_for_setter = label_widget.clone();
    let setter: Rc<dyn Fn(&str)> = Rc::new(move |next_value: &str| {
        if let Some((display_name, value)) = options_for_setter
            .iter()
            .find(|(_, value)| value == next_value)
        {
            selected_value_for_setter.replace(value.clone());
            let next_label = selected_label
                .as_ref()
                .map(|formatter| formatter(display_name, value))
                .unwrap_or_else(|| display_name.clone());
            label_widget_for_setter.set_text(&next_label);
        }
    });

    (button, setter)
}

fn refresh_turn_status(turn_ui: &mut TurnUi) {
    let reasoning_visible = turn_ui
        .body_box
        .root()
        .and_then(|root| {
            let root_widget: gtk::Widget = root.upcast();
            crate::ui::widget_tree::find_widget_by_name(&root_widget, "chat-messages-box")
        })
        .and_then(|widget| widget.downcast::<gtk::Box>().ok())
        .is_some_and(|messages_box| message_render::messages_reasoning_visible(&messages_box));
    let has_visible_generic_items = turn_ui
        .generic_item_widgets
        .keys()
        .any(|item_id| !turn_ui.reasoning_item_ids.contains(item_id) || reasoning_visible);
    let has_content = turn_ui
        .text_buffers
        .values()
        .any(|content| !content.trim().is_empty())
        || !turn_ui.command_widgets.is_empty()
        || !turn_ui.file_change_widgets.is_empty()
        || !turn_ui.tool_call_widgets.is_empty()
        || has_visible_generic_items;
    turn_ui.body_box.set_visible(has_content);

    if !turn_ui.in_progress {
        turn_ui.runtime_status_text = None;
        turn_ui.status_row.set_visible(false);
        turn_ui.bubble.remove_css_class("chat-turn-bubble-initial");
        return;
    }

    if has_content {
        turn_ui.bubble.remove_css_class("chat-turn-bubble-initial");
    } else {
        turn_ui.bubble.add_css_class("chat-turn-bubble-initial");
    }

    if let Some(status_text) = turn_ui.runtime_status_text.as_deref() {
        let status_text = status_text.trim();
        if !status_text.is_empty() {
            turn_ui.status_row.set_visible(true);
            turn_ui.status_label.set_text(status_text);
            return;
        }
    }

    if turn_ui
        .pending_items
        .values()
        .any(|kind| kind == "reasoning" || kind == "plan")
    {
        turn_ui.status_row.set_visible(true);
        turn_ui.status_label.set_text("Thinking...");
        return;
    }
    if turn_ui
        .pending_items
        .values()
        .any(|kind| kind == "commandExecution")
    {
        turn_ui.status_row.set_visible(true);
        turn_ui.status_label.set_text("Running command...");
        return;
    }
    if turn_ui
        .pending_items
        .values()
        .any(|kind| kind == "fileChange")
    {
        turn_ui.status_row.set_visible(true);
        turn_ui.status_label.set_text("Applying file changes...");
        return;
    }
    if turn_ui.pending_items.values().any(|kind| {
        kind == "dynamicToolCall"
            || kind == "webSearch"
            || kind == "mcpToolCall"
            || kind == "collabToolCall"
            || kind == "imageView"
            || kind == "contextCompaction"
    }) {
        turn_ui.status_row.set_visible(true);
        turn_ui.status_label.set_text("Using tools...");
        return;
    }
    turn_ui.status_row.set_visible(true);
    turn_ui.status_label.set_text("Working...");
}

const CHAT_REASONING_VISIBLE_SETTING: &str = "chat_reasoning_visible";

fn format_turn_elapsed(total_secs: u64) -> String {
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn truncate_live_status_text(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    text.chars().take(max_chars).collect()
}

fn wave_status_markup(text: &str, phase: f64) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return String::new();
    }

    let tail = 6.0f64;
    let cycle = chars.len() as f64 + tail;
    let center = phase.rem_euclid(cycle);

    let base = (152.0f64, 161.0f64, 172.0f64);
    let highlight = (241.0f64, 243.0f64, 246.0f64);
    let sigma = 1.25f64;

    let mut markup = String::with_capacity(text.len() * 28);
    for (idx, ch) in chars.iter().enumerate() {
        let dist = idx as f64 - center;
        let weight = (-(dist * dist) / (2.0 * sigma * sigma))
            .exp()
            .clamp(0.0, 1.0);
        let r = (base.0 + (highlight.0 - base.0) * weight).round() as u8;
        let g = (base.1 + (highlight.1 - base.1) * weight).round() as u8;
        let b = (base.2 + (highlight.2 - base.2) * weight).round() as u8;
        let color = format!("#{r:02X}{g:02X}{b:02X}");
        let escaped = gtk::glib::markup_escape_text(&ch.to_string());
        markup.push_str("<span foreground=\"");
        markup.push_str(&color);
        markup.push_str("\">");
        markup.push_str(escaped.as_str());
        markup.push_str("</span>");
    }
    markup
}

pub(crate) fn sidebar_wave_status_markup(text: &str, phase: f64) -> String {
    wave_status_markup(text, phase)
}

pub(crate) fn thread_has_active_turn(thread_id: &str) -> bool {
    codex_runtime::active_turn_for_thread(thread_id).is_some()
}

pub(crate) fn has_any_active_turn() -> bool {
    codex_runtime::has_any_active_turn()
}

pub(crate) fn mark_thread_completed_unseen(thread_id: &str) {
    codex_runtime::mark_thread_completed_unseen(thread_id);
}

pub(crate) fn clear_thread_completed_unseen(thread_id: &str) {
    codex_runtime::clear_thread_completed_unseen(thread_id);
}

pub(crate) fn thread_has_completed_unseen(thread_id: &str) -> bool {
    codex_runtime::thread_has_completed_unseen(thread_id)
}

fn create_turn_ui(
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    conversation_stack: &gtk::Stack,
) -> TurnUi {
    conversation_stack.set_visible_child_name("messages");

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("chat-message-row");
    row.set_halign(gtk::Align::Fill);
    row.set_hexpand(true);
    message_render::apply_first_message_top_spacing(messages_box, &row);

    let bubble = gtk::Box::new(gtk::Orientation::Vertical, 8);
    bubble.add_css_class("chat-assistant-surface");

    let body_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    body_box.add_css_class("chat-command-list");
    bubble.append(&body_box);

    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    status_row.set_halign(gtk::Align::Start);
    status_row.set_visible(false);
    let status_label = gtk::Label::new(Some("Working..."));
    status_label.set_xalign(0.0);
    status_label.add_css_class("chat-status-label");
    status_label.set_wrap(true);
    status_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status_row.append(&status_label);

    message_render::append_hover_timestamp(messages_box, &row, &bubble, false, SystemTime::now());
    message_render::make_assistant_row_full_width(&row);
    let (timestamp_label, timestamp_revealer) =
        message_render::prepare_assistant_turn_completion_timestamp(&row).unwrap_or_else(|| {
            let label = gtk::Label::new(None);
            let revealer = gtk::Revealer::new();
            revealer.set_reveal_child(false);
            (label, revealer)
        });
    messages_box.append(&row);
    message_render::scroll_to_bottom(messages_scroll);

    TurnUi {
        bubble,
        status_row,
        status_label,
        runtime_status_text: None,
        timestamp_label,
        timestamp_revealer,
        in_progress: false,
        body_box,
        text_widgets: HashMap::new(),
        streaming_text_widgets: HashMap::new(),
        text_buffers: HashMap::new(),
        text_pending_deltas: HashMap::new(),
        agent_message_item_ids: HashSet::new(),
        reasoning_item_ids: HashSet::new(),
        status_buffers: HashMap::new(),
        status_last_text: String::new(),
        status_last_updated_micros: 0,
        reasoning_started_micros: HashMap::new(),
        pending_items: HashMap::new(),
        command_widgets: HashMap::new(),
        file_change_widgets: HashMap::new(),
        tool_call_widgets: HashMap::new(),
        generic_item_widgets: HashMap::new(),
    }
}

#[derive(Clone)]
pub struct ChatPaneWidgets {
    pub root: gtk::Box,
    pub messages_box: gtk::Box,
    pub messages_scroll: gtk::ScrolledWindow,
    pub conversation_stack: gtk::Stack,
}

pub fn build_chat_pane_without_composer(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    codex: Option<Arc<RuntimeClient>>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> Option<ChatPaneWidgets> {
    let root = build_chat_tab_single(
        db,
        manager,
        codex,
        active_thread_id.clone(),
        active_thread_id,
        false,
        active_workspace_path,
    );
    root.set_spacing(0);
    root.set_margin_end(0);
    let root_widget: gtk::Widget = root.clone().upcast();
    if let Some(composer_shell_widget) =
        widget_tree::find_widget_by_css_class(&root_widget, "composer-floating-shell")
    {
        composer_shell_widget.set_visible(false);
    }

    if let Some(live_status_widget) =
        widget_tree::find_widget_by_name(&root_widget, "chat-live-status-revealer")
    {
        if let Ok(live_status_revealer) = live_status_widget.downcast::<gtk::Revealer>() {
            if let Some(parent) = live_status_revealer.parent() {
                if let Ok(parent_box) = parent.downcast::<gtk::Box>() {
                    parent_box.remove(&live_status_revealer);
                }
            }

            live_status_revealer.set_hexpand(true);
            live_status_revealer.set_halign(gtk::Align::Fill);
            live_status_revealer.set_valign(gtk::Align::Center);

            let bottom_extension = gtk::Box::new(gtk::Orientation::Vertical, 0);
            bottom_extension.add_css_class("multi-chat-pane-bottom-extension");
            bottom_extension.set_halign(gtk::Align::Fill);
            bottom_extension.set_hexpand(true);
            bottom_extension.set_margin_start(12);
            bottom_extension.set_margin_end(12);
            bottom_extension.set_margin_top(0);
            bottom_extension.set_margin_bottom(6);
            bottom_extension.append(&live_status_revealer);

            let extension_revealer = gtk::Revealer::new();
            extension_revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);
            extension_revealer.set_transition_duration(180);
            extension_revealer.set_reveal_child(false);
            extension_revealer.set_visible(false);
            extension_revealer.set_child(Some(&bottom_extension));

            let sync_extension_visibility = {
                let live_status_revealer = live_status_revealer.clone();
                let extension_revealer = extension_revealer.clone();
                move || {
                    let should_show = live_status_revealer.reveals_child()
                        || live_status_revealer.is_child_revealed();
                    if should_show {
                        if !extension_revealer.is_visible() {
                            extension_revealer.set_visible(true);
                        }
                        extension_revealer.set_reveal_child(true);
                        return;
                    }
                    extension_revealer.set_reveal_child(false);
                    if !extension_revealer.is_child_revealed() {
                        extension_revealer.set_visible(false);
                    }
                }
            };
            sync_extension_visibility();

            {
                let sync_extension_visibility = sync_extension_visibility.clone();
                live_status_revealer.connect_reveal_child_notify(move |_| {
                    sync_extension_visibility();
                });
            }
            {
                let sync_extension_visibility = sync_extension_visibility.clone();
                live_status_revealer.connect_child_revealed_notify(move |_| {
                    sync_extension_visibility();
                });
            }
            {
                let sync_extension_visibility = sync_extension_visibility.clone();
                live_status_revealer.connect_visible_notify(move |_| {
                    sync_extension_visibility();
                });
            }
            {
                extension_revealer.connect_child_revealed_notify(move |revealer| {
                    if !revealer.reveals_child() && !revealer.is_child_revealed() {
                        revealer.set_visible(false);
                    }
                });
            }

            root.append(&extension_revealer);
        }
    }
    let messages_box = widget_tree::find_widget_by_name(&root_widget, "chat-messages-box")
        .and_then(|widget| widget.downcast::<gtk::Box>().ok())?;
    messages_box.set_margin_bottom(0);
    let messages_scroll = widget_tree::find_widget_by_name(&root_widget, "chat-messages-scroll")
        .and_then(|widget| widget.downcast::<gtk::ScrolledWindow>().ok())?;
    let conversation_stack =
        widget_tree::find_widget_by_name(&root_widget, "chat-conversation-stack")
            .and_then(|widget| widget.downcast::<gtk::Stack>().ok())?;
    Some(ChatPaneWidgets {
        root,
        messages_box,
        messages_scroll,
        conversation_stack,
    })
}

pub fn build_shared_composer_for_chat_target(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    codex: Option<Arc<RuntimeClient>>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
    messages_box: gtk::Box,
    messages_scroll: gtk::ScrolledWindow,
    conversation_stack: gtk::Stack,
) -> gtk::Box {
    let active_turn: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let active_turn_thread: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let composer_section = composer::build(
        db,
        manager,
        codex,
        active_thread_id,
        active_workspace_path,
        messages_box,
        messages_scroll,
        conversation_stack,
        active_turn,
        active_turn_thread,
    );
    let suggestion_row = composer_section.suggestion_row;
    let lower_content = composer_section.lower_content;
    lower_content.remove(&suggestion_row);
    lower_content.set_halign(gtk::Align::Center);
    lower_content.set_valign(gtk::Align::End);
    lower_content.set_margin_start(12);
    lower_content.set_margin_end(12);
    lower_content.set_margin_bottom(10);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(1200);
    clamp.set_tightening_threshold(1200);
    clamp.set_child(Some(&lower_content));
    clamp.set_halign(gtk::Align::Center);
    clamp.set_valign(gtk::Align::End);

    let shell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    shell.add_css_class("shared-composer-shell");
    shell.append(&clamp);
    shell
}

pub(crate) fn refresh_visible_history_for_thread(
    db: &AppDb,
    parent_window: &gtk::Window,
    thread_id: &str,
    thread: &Value,
) -> bool {
    let thread_turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .map(|turns| turns.len())
        .unwrap_or(0);
    eprintln!(
        "[chat-refresh] start thread_id={} thread_turns={}",
        thread_id, thread_turns
    );

    let root_widget: gtk::Widget = parent_window.clone().upcast();
    let pane_root = widget_tree::find_widget_by_name(&root_widget, "chat-thread-pane-stack")
        .and_then(|stack_widget| stack_widget.downcast::<gtk::Stack>().ok())
        .and_then(|stack| {
            stack
                .child_by_name(&format!("thread:{thread_id}"))
                .or_else(|| stack.visible_child())
        });
    let search_root = pane_root.as_ref().unwrap_or(&root_widget);

    let Some(messages_widget) = widget_tree::find_widget_by_name(search_root, "chat-messages-box")
    else {
        eprintln!("[chat-refresh] failed: chat-messages-box not found");
        return false;
    };
    let Some(scroll_widget) = widget_tree::find_widget_by_name(search_root, "chat-messages-scroll")
    else {
        eprintln!("[chat-refresh] failed: chat-messages-scroll not found");
        return false;
    };
    let Some(stack_widget) =
        widget_tree::find_widget_by_name(search_root, "chat-conversation-stack")
    else {
        eprintln!("[chat-refresh] failed: chat-conversation-stack not found");
        return false;
    };
    let Some(suggestion_widget) =
        widget_tree::find_widget_by_name(search_root, "chat-suggestion-row")
    else {
        eprintln!("[chat-refresh] failed: chat-suggestion-row not found");
        return false;
    };

    let Ok(messages_box) = messages_widget.downcast::<gtk::Box>() else {
        eprintln!("[chat-refresh] failed: chat-messages-box downcast");
        return false;
    };
    let Ok(messages_scroll) = scroll_widget.downcast::<gtk::ScrolledWindow>() else {
        eprintln!("[chat-refresh] failed: chat-messages-scroll downcast");
        return false;
    };
    let Ok(conversation_stack) = stack_widget.downcast::<gtk::Stack>() else {
        eprintln!("[chat-refresh] failed: chat-conversation-stack downcast");
        return false;
    };
    let Ok(suggestion_row) = suggestion_widget.downcast::<gtk::Box>() else {
        eprintln!("[chat-refresh] failed: chat-suggestion-row downcast");
        return false;
    };

    let mut before_rows = 0usize;
    let mut before_child = messages_box.first_child();
    while let Some(node) = before_child {
        before_rows += 1;
        before_child = node.next_sibling();
    }
    eprintln!(
        "[chat-refresh] before render thread_id={} visible_rows={}",
        thread_id, before_rows
    );

    if let Err(err) = history::sync_completed_turns_from_thread(db, thread_id, thread) {
        eprintln!(
            "[chat-refresh] failed to sync local completed turns thread_id={}: {}",
            thread_id, err
        );
        return false;
    }
    history::prune_cached_state_for_thread(db, thread_id, thread);

    let _ = history::render_local_thread_history_from_db(
        db,
        None,
        &messages_box,
        &messages_scroll,
        &conversation_stack,
        &suggestion_row,
        thread_id,
        None,
    );

    let mut after_rows = 0usize;
    let mut after_child = messages_box.first_child();
    while let Some(node) = after_child {
        after_rows += 1;
        after_child = node.next_sibling();
    }
    eprintln!(
        "[chat-refresh] after render thread_id={} visible_rows={} suggestion_visible={}",
        thread_id,
        after_rows,
        suggestion_row.is_visible()
    );
    true
}

pub(crate) fn sync_local_history_for_thread(db: &AppDb, thread_id: &str, thread: &Value) -> bool {
    match history::sync_completed_turns_from_thread(db, thread_id, thread) {
        Ok(_) => {
            history::prune_cached_state_for_thread(db, thread_id, thread);
            true
        }
        Err(err) => {
            eprintln!(
                "[chat-refresh] failed to sync local history thread_id={}: {}",
                thread_id, err
            );
            false
        }
    }
}

pub(crate) fn request_runtime_history_reload(thread_id: &str) {
    codex_runtime::request_history_reload_for_thread(thread_id);
}

fn build_chat_tab_single(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    codex: Option<Arc<RuntimeClient>>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    selected_thread_id: Rc<RefCell<Option<String>>>,
    track_background_completion: bool,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> gtk::Box {
    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content_box.set_margin_start(0);
    content_box.set_margin_end(14);
    content_box.set_margin_top(0);
    content_box.set_margin_bottom(0);
    content_box.set_vexpand(true);

    let chat_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    chat_frame.add_css_class("chat-frame");
    chat_frame.set_vexpand(true);

    let conversation_stack = gtk::Stack::new();
    conversation_stack.set_widget_name("chat-conversation-stack");
    conversation_stack.set_vexpand(true);
    conversation_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    conversation_stack.set_transition_duration(160);

    let empty_state = gtk::Box::new(gtk::Orientation::Vertical, 4);
    empty_state.set_vexpand(true);
    empty_state.set_valign(gtk::Align::Center);
    empty_state.set_halign(gtk::Align::Center);

    let heading = gtk::Label::new(Some("Select a Thread"));
    heading.add_css_class("compact-heading");
    empty_state.append(&heading);

    let install_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    install_box.add_css_class("welcome-section");
    install_box.set_halign(gtk::Align::Center);
    install_box.set_visible(false);

    let install_hint = gtk::Label::new(Some("Install a supported runtime CLI first:"));
    install_hint.set_xalign(0.0);
    install_hint.add_css_class("welcome-muted");
    install_box.append(&install_hint);

    let install_command_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    install_command_row.add_css_class("welcome-code-block");

    let install_command = gtk::Label::new(Some(
        "npm i -g @openai/codex  # or: npm install -g opencode-ai",
    ));
    install_command.add_css_class("welcome-code-text");
    install_command.set_xalign(0.0);
    install_command.set_hexpand(true);
    install_command.set_selectable(true);
    install_command.set_focusable(false);
    install_command_row.append(&install_command);

    let copy_install_button = gtk::Button::new();
    copy_install_button.add_css_class("app-flat-button");
    copy_install_button.add_css_class("welcome-icon-copy");
    copy_install_button.set_tooltip_text(Some("Copy install command"));
    let copy_install_icon = gtk::Image::from_icon_name("edit-copy-symbolic");
    copy_install_icon.set_pixel_size(14);
    copy_install_button.set_child(Some(&copy_install_icon));
    copy_install_button.connect_clicked(move |_| {
        if let Some(display) = gtk::gdk::Display::default() {
            display
                .clipboard()
                .set_text("npm i -g @openai/codex  # or: npm install -g opencode-ai");
        }
    });
    install_command_row.append(&copy_install_button);
    install_box.append(&install_command_row);
    empty_state.append(&install_box);

    let loading_state = gtk::Box::new(gtk::Orientation::Vertical, 8);
    loading_state.set_vexpand(true);
    loading_state.set_valign(gtk::Align::Center);
    loading_state.set_halign(gtk::Align::Center);
    let loading_label = gtk::Label::new(Some("Loading thread..."));
    loading_label.add_css_class("compact-heading");
    loading_state.append(&loading_label);

    let messages_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    messages_box.set_widget_name("chat-messages-box");
    messages_box.set_margin_start(12);
    messages_box.set_margin_end(12);
    messages_box.set_margin_top(0);
    messages_box.set_margin_bottom(138);
    message_render::bind_chat_context(&messages_box, db.clone(), manager.clone());

    let messages_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .vexpand(true)
        .child(&messages_box)
        .build();
    messages_scroll.set_has_frame(false);
    messages_scroll.set_widget_name("chat-messages-scroll");
    message_render::register_auto_scroll_user_tracking(&messages_scroll);
    {
        let messages_scroll = messages_scroll.clone();
        messages_scroll.clone().connect_map(move |_| {
            if !message_render::should_follow_auto_scroll(&messages_scroll) {
                return;
            }
            let messages_scroll = messages_scroll.clone();
            crate::ui::scheduler::idle_once(move || {
                if messages_scroll.root().is_none() {
                    return;
                }
                message_render::scroll_to_bottom(&messages_scroll);
            });
        });
    }

    conversation_stack.add_named(&empty_state, Some("empty"));
    conversation_stack.add_named(&loading_state, Some("loading"));
    conversation_stack.add_named(&messages_scroll, Some("messages"));
    conversation_stack.set_visible_child_name("loading");

    let conversation_overlay = gtk::Overlay::new();
    conversation_overlay.set_vexpand(true);
    conversation_overlay.set_child(Some(&conversation_stack));

    let bottom_fade = gtk::Box::new(gtk::Orientation::Vertical, 0);
    bottom_fade.add_css_class("chat-scroll-bottom-fade");
    bottom_fade.set_valign(gtk::Align::End);
    bottom_fade.set_halign(gtk::Align::Fill);
    bottom_fade.set_margin_start(12);
    bottom_fade.set_margin_end(12);
    bottom_fade.set_height_request(18);
    bottom_fade.set_can_target(false);
    bottom_fade.set_visible(false);
    conversation_overlay.add_overlay(&bottom_fade);

    let scroll_down_button = gtk::Button::builder()
        .icon_name("disclose-arrow-down-symbolic")
        .build();
    scroll_down_button.add_css_class("scroll-down-button");
    scroll_down_button.add_css_class("circular");
    scroll_down_button.set_has_frame(false);
    scroll_down_button.set_halign(gtk::Align::End);
    scroll_down_button.set_valign(gtk::Align::End);
    scroll_down_button.set_margin_end(14);
    scroll_down_button.set_margin_bottom(12);
    scroll_down_button.set_size_request(32, 32);
    scroll_down_button.set_visible(false);
    {
        let messages_scroll = messages_scroll.clone();
        scroll_down_button.connect_clicked(move |_| {
            message_render::resume_auto_scroll(&messages_scroll);
            message_render::scroll_to_bottom(&messages_scroll);
        });
    }

    let reasoning_toggle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    reasoning_toggle.add_css_class("chat-action-hidden-thinking-button");
    reasoning_toggle.add_css_class("chat-floating-thinking-toggle");
    reasoning_toggle.set_can_target(true);
    reasoning_toggle.set_focusable(false);
    reasoning_toggle.set_valign(gtk::Align::End);
    reasoning_toggle.set_visible(false);
    let reasoning_toggle_overlay = gtk::Overlay::new();
    let reasoning_toggle_icon = gtk::Image::from_icon_name("lightbulb-modern-symbolic");
    reasoning_toggle_icon.set_pixel_size(12);
    reasoning_toggle_overlay.set_child(Some(&reasoning_toggle_icon));
    let reasoning_toggle_slash = gtk::Label::new(Some("/"));
    reasoning_toggle_slash.add_css_class("chat-action-hidden-thinking-slash");
    reasoning_toggle_slash.set_halign(gtk::Align::Center);
    reasoning_toggle_slash.set_valign(gtk::Align::Center);
    reasoning_toggle_overlay.add_overlay(&reasoning_toggle_slash);
    reasoning_toggle.append(&reasoning_toggle_overlay);

    let floating_chat_controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    floating_chat_controls.set_halign(gtk::Align::End);
    floating_chat_controls.set_valign(gtk::Align::End);
    floating_chat_controls.set_margin_end(14);
    floating_chat_controls.set_margin_bottom(12);
    floating_chat_controls.append(&scroll_down_button);
    floating_chat_controls.append(&reasoning_toggle);
    conversation_overlay.add_overlay(&floating_chat_controls);
    message_render::register_chat_reasoning_toggle(&messages_box, &reasoning_toggle);
    let restore_reasoning_visible = db
        .get_setting(CHAT_REASONING_VISIBLE_SETTING)
        .ok()
        .flatten()
        .is_some_and(|value| value == "1");
    if restore_reasoning_visible {
        message_render::set_chat_reasoning_visibility(&messages_box, true);
    }
    {
        let db = db.clone();
        let messages_box = messages_box.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            message_render::toggle_chat_reasoning_visibility(&messages_box);
            let value = if message_render::messages_reasoning_visible(&messages_box) {
                "1"
            } else {
                "0"
            };
            let _ = db.set_setting(CHAT_REASONING_VISIBLE_SETTING, value);
        });
        reasoning_toggle.add_controller(click);
    }

    #[derive(Clone)]
    struct WorktreeOverlayState {
        local_thread_id: i64,
        worktree_path: String,
        live_workspace_path: String,
    }

    let worktree_overlay = gtk::Box::new(gtk::Orientation::Vertical, 8);
    worktree_overlay.add_css_class("chat-worktree-overlay");
    worktree_overlay.set_halign(gtk::Align::End);
    worktree_overlay.set_valign(gtk::Align::Start);
    worktree_overlay.set_margin_top(10);
    worktree_overlay.set_margin_end(14);
    worktree_overlay.set_visible(false);

    let worktree_title_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let worktree_icon = gtk::Image::from_icon_name("git-symbolic");
    worktree_icon.set_pixel_size(14);
    worktree_icon.add_css_class("chat-worktree-overlay-icon");
    worktree_title_row.append(&worktree_icon);
    let worktree_title = gtk::Label::new(Some("Worktree Active"));
    worktree_title.add_css_class("chat-worktree-overlay-title");
    worktree_title.set_xalign(0.0);
    worktree_title.set_hexpand(true);
    worktree_title_row.append(&worktree_title);
    worktree_overlay.append(&worktree_title_row);

    let worktree_fork_label = gtk::Label::new(Some("Fork of: —"));
    worktree_fork_label.add_css_class("chat-worktree-overlay-subtitle");
    worktree_fork_label.set_xalign(0.0);
    worktree_fork_label.set_wrap(true);
    worktree_fork_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    worktree_overlay.append(&worktree_fork_label);

    let worktree_actions_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    worktree_actions_row.set_halign(gtk::Align::Start);
    let worktree_copy_button = gtk::Button::new();
    worktree_copy_button.add_css_class("app-flat-button");
    worktree_copy_button.add_css_class("chat-worktree-overlay-action");
    worktree_copy_button.set_has_frame(false);
    let copy_content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let copy_icon = gtk::Image::from_icon_name("edit-copy-symbolic");
    copy_icon.set_pixel_size(12);
    copy_icon.add_css_class("chat-worktree-overlay-action-icon");
    let copy_label = gtk::Label::new(Some("Copy Worktree Path"));
    copy_content.append(&copy_icon);
    copy_content.append(&copy_label);
    worktree_copy_button.set_child(Some(&copy_content));

    let worktree_merge_button = gtk::Button::new();
    worktree_merge_button.add_css_class("app-flat-button");
    worktree_merge_button.add_css_class("chat-worktree-overlay-action");
    worktree_merge_button.set_has_frame(false);
    let merge_content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let merge_icon = gtk::Image::from_icon_name("merge-symbolic");
    merge_icon.set_pixel_size(12);
    merge_icon.add_css_class("chat-worktree-overlay-action-icon");
    let merge_label = gtk::Label::new(Some("Stop and Merge"));
    merge_content.append(&merge_icon);
    merge_content.append(&merge_label);
    worktree_merge_button.set_child(Some(&merge_content));
    worktree_actions_row.append(&worktree_copy_button);
    worktree_actions_row.append(&worktree_merge_button);
    worktree_overlay.append(&worktree_actions_row);
    conversation_overlay.add_overlay(&worktree_overlay);

    let worktree_overlay_state: Rc<RefCell<Option<WorktreeOverlayState>>> =
        Rc::new(RefCell::new(None));
    {
        let worktree_overlay_state = worktree_overlay_state.clone();
        worktree_copy_button.connect_clicked(move |_| {
            let Some(state) = worktree_overlay_state.borrow().clone() else {
                return;
            };
            if let Some(display) = gtk::gdk::Display::default() {
                let clipboard = display.clipboard();
                clipboard.set_text(&state.worktree_path);
            }
        });
    }
    {
        let db = db.clone();
        let active_workspace_path = active_workspace_path.clone();
        let worktree_overlay_state = worktree_overlay_state.clone();
        let messages_box = messages_box.clone();
        let messages_scroll = messages_scroll.clone();
        let conversation_stack = conversation_stack.clone();
        let worktree_merge_button_for_parent = worktree_merge_button.clone();
        worktree_merge_button.connect_clicked(move |_| {
            let Some(state) = worktree_overlay_state.borrow().clone() else {
                return;
            };
            let parent = worktree_merge_button_for_parent
                .root()
                .and_then(|root| root.downcast::<gtk::Window>().ok());
            composer::open_worktree_merge_popup(
                parent,
                db.clone(),
                active_workspace_path.clone(),
                &messages_box,
                &messages_scroll,
                &conversation_stack,
                state.local_thread_id,
                &state.worktree_path,
                &state.live_workspace_path,
            );
        });
    }
    {
        let db = db.clone();
        let active_thread_id = active_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        let worktree_overlay = worktree_overlay.clone();
        let worktree_overlay_state = worktree_overlay_state.clone();
        let worktree_fork_label = worktree_fork_label.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(220), move || {
            if worktree_overlay.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            let state = active_thread_id
                .borrow()
                .as_deref()
                .and_then(|thread_id| db.get_thread_record_by_remote_thread_id(thread_id).ok())
                .flatten()
                .and_then(|thread| {
                    if !thread.worktree_active {
                        return None;
                    }
                    let worktree_path = thread
                        .worktree_path
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())?
                        .to_string();
                    let live_workspace_path = db
                        .workspace_path_for_local_thread(thread.id)
                        .ok()
                        .flatten()
                        .or_else(|| active_workspace_path.borrow().clone())
                        .map(|value| value.trim().to_string())
                        .filter(|value| !value.is_empty())?;
                    let fork_title = thread
                        .parent_thread_id
                        .and_then(|parent_id| db.get_thread_record(parent_id).ok().flatten())
                        .map(|record| record.title)
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or_else(|| "Unknown".to_string());
                    worktree_fork_label.set_text(&format!("Fork of: {fork_title}"));
                    Some(WorktreeOverlayState {
                        local_thread_id: thread.id,
                        worktree_path,
                        live_workspace_path,
                    })
                });
            worktree_overlay_state.replace(state.clone());
            worktree_overlay.set_visible(state.is_some());
            gtk::glib::ControlFlow::Continue
        });
    }

    let update_scroll_overlays: Rc<dyn Fn()> = {
        let bottom_fade = bottom_fade.clone();
        let scroll_down_button = scroll_down_button.clone();
        let messages_scroll = messages_scroll.clone();
        Rc::new(move || {
            let adj = messages_scroll.vadjustment();
            let lower = adj.lower();
            let upper = adj.upper();
            let page = adj.page_size();
            let value = adj.value();
            let can_scroll = (upper - lower) > page + 1.0;
            let bottom = (upper - page).max(lower);
            let distance_from_bottom = (bottom - value).max(0.0);
            let show_fade = can_scroll && distance_from_bottom > 1.0;
            let show_button = can_scroll && distance_from_bottom > 400.0;
            bottom_fade.set_visible(show_fade);
            scroll_down_button.set_visible(show_button);
        })
    };
    (update_scroll_overlays)();
    {
        let update_scroll_overlays = update_scroll_overlays.clone();
        let adj = messages_scroll.vadjustment();
        adj.connect_value_changed(move |_| {
            (update_scroll_overlays)();
        });
    }
    {
        let update_scroll_overlays = update_scroll_overlays.clone();
        let adj = messages_scroll.vadjustment();
        adj.connect_changed(move |_| {
            (update_scroll_overlays)();
        });
    }

    let active_turn: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let active_turn_thread: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

    let composer_section = composer::build(
        db.clone(),
        manager.clone(),
        codex.clone(),
        active_thread_id.clone(),
        active_workspace_path.clone(),
        messages_box.clone(),
        messages_scroll.clone(),
        conversation_stack.clone(),
        active_turn.clone(),
        active_turn_thread.clone(),
    );
    let lower_content = composer_section.lower_content;
    let suggestion_row = composer_section.suggestion_row;
    let live_turn_status_revealer = composer_section.live_turn_status_revealer;
    let live_turn_status_label = composer_section.live_turn_status_label;
    let live_turn_timer_label = composer_section.live_turn_timer_label;
    live_turn_status_revealer.set_widget_name("chat-live-status-revealer");
    suggestion_row.set_widget_name("chat-suggestion-row");
    lower_content.set_halign(gtk::Align::Center);
    lower_content.set_valign(gtk::Align::End);
    lower_content.set_margin_start(12);
    lower_content.set_margin_end(12);
    lower_content.set_margin_bottom(10);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(1200);
    clamp.set_tightening_threshold(1200);
    clamp.set_child(Some(&lower_content));
    clamp.set_halign(gtk::Align::Center);
    clamp.set_valign(gtk::Align::End);
    let composer_revealer = gtk::Revealer::new();
    composer_revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
    composer_revealer.set_transition_duration(220);
    composer_revealer.set_reveal_child(true);
    composer_revealer.set_visible(true);
    composer_revealer.set_halign(gtk::Align::Center);
    composer_revealer.set_valign(gtk::Align::End);
    composer_revealer.set_child(Some(&clamp));
    conversation_overlay.add_overlay(&composer_revealer);

    profile_selector::attach(profile_selector::AttachArgs {
        db: db.clone(),
        manager: manager.clone(),
        active_thread_id: active_thread_id.clone(),
        selected_thread_id: selected_thread_id.clone(),
        active_workspace_path: active_workspace_path.clone(),
        composer_revealer: composer_revealer.clone(),
        live_turn_status_revealer: live_turn_status_revealer.clone(),
        heading: heading.clone(),
        install_box: install_box.clone(),
        empty_state: empty_state.clone(),
        messages_box: messages_box.clone(),
        conversation_stack: conversation_stack.clone(),
    });
    chat_frame.append(&conversation_overlay);

    let turn_uis: Rc<RefCell<HashMap<String, TurnUi>>> = Rc::new(RefCell::new(HashMap::new()));
    let item_turns: Rc<RefCell<HashMap<String, String>>> = Rc::new(RefCell::new(HashMap::new()));
    let item_kinds: Rc<RefCell<HashMap<String, String>>> = Rc::new(RefCell::new(HashMap::new()));
    let item_threads: Rc<RefCell<HashMap<String, String>>> = Rc::new(RefCell::new(HashMap::new()));
    let turn_threads: Rc<RefCell<HashMap<String, String>>> = Rc::new(RefCell::new(HashMap::new()));
    let cached_commands_for_thread: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
    let cached_file_changes_for_thread: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
    let cached_tool_items_for_thread: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
    let cached_pending_requests_for_thread: Rc<RefCell<Vec<Value>>> =
        Rc::new(RefCell::new(Vec::new()));
    let cached_turn_errors_for_thread: Rc<RefCell<Vec<Value>>> = Rc::new(RefCell::new(Vec::new()));
    let loaded_history_thread_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let loading_history_thread_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    {
        codex_runtime::attach(
            db.clone(),
            manager,
            messages_box.clone(),
            messages_scroll.clone(),
            conversation_stack.clone(),
            suggestion_row.clone(),
            track_background_completion,
            active_thread_id.clone(),
            turn_uis.clone(),
            item_turns.clone(),
            item_kinds.clone(),
            item_threads.clone(),
            turn_threads.clone(),
            active_turn.clone(),
            active_turn_thread.clone(),
            cached_commands_for_thread.clone(),
            cached_file_changes_for_thread.clone(),
            cached_tool_items_for_thread.clone(),
            cached_pending_requests_for_thread.clone(),
            cached_turn_errors_for_thread.clone(),
            loaded_history_thread_id.clone(),
            loading_history_thread_id.clone(),
        );
    }
    {
        let turn_uis = turn_uis.clone();
        let turn_threads = turn_threads.clone();
        let active_turn = active_turn.clone();
        let active_thread_id = active_thread_id.clone();
        let composer_revealer = composer_revealer.clone();
        let live_turn_status_revealer = live_turn_status_revealer.clone();
        let live_turn_status_label = live_turn_status_label.clone();
        let live_turn_timer_label = live_turn_timer_label.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(33), move || {
            if live_turn_status_revealer.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            if !composer_revealer.is_visible() {
                live_turn_status_revealer.set_reveal_child(false);
                live_turn_status_revealer.set_visible(false);
                return gtk::glib::ControlFlow::Continue;
            }

            let active_thread_id = active_thread_id.borrow().clone();
            let now_micros = gtk::glib::monotonic_time();

            let selected_turn_id = {
                let turns = turn_uis.borrow();
                let turn_threads = turn_threads.borrow();
                active_turn
                    .borrow()
                    .clone()
                    .filter(|turn_id| {
                        let in_progress = turns
                            .get(turn_id)
                            .map(|turn_ui| turn_ui.in_progress)
                            .unwrap_or(false);
                        let belongs_to_active = active_thread_id
                            .as_deref()
                            .and_then(|active| {
                                turn_threads
                                    .get(turn_id)
                                    .map(|owner| owner.as_str() == active)
                            })
                            .unwrap_or(false);
                        in_progress && belongs_to_active
                    })
                    .or_else(|| {
                        turns.iter().find_map(|(turn_id, turn_ui)| {
                            let belongs_to_active = active_thread_id
                                .as_deref()
                                .and_then(|active| {
                                    turn_threads
                                        .get(turn_id)
                                        .map(|owner| owner.as_str() == active)
                                })
                                .unwrap_or(false);
                            if turn_ui.in_progress && belongs_to_active {
                                Some(turn_id.clone())
                            } else {
                                None
                            }
                        })
                    })
            };

            if let Some(turn_id) = selected_turn_id {
                let status_text = {
                    let turns = turn_uis.borrow();
                    turns
                        .get(&turn_id)
                        .map(|turn_ui| turn_ui.status_label.text().to_string())
                        .unwrap_or_default()
                };
                let started_micros =
                    codex_runtime::active_turn_started_micros(&turn_id).unwrap_or(now_micros);
                let elapsed_secs = ((now_micros - started_micros).max(0) / 1_000_000) as u64;
                live_turn_timer_label.set_text(&format_turn_elapsed(elapsed_secs));
                let status_text = status_text.trim();
                let status_text = if status_text.is_empty() {
                    "Working..."
                } else {
                    status_text
                };
                let status_text = truncate_live_status_text(status_text, 20);
                let wave_phase = now_micros as f64 / 90_000.0;
                live_turn_status_label.set_use_markup(true);
                live_turn_status_label.set_markup(&wave_status_markup(&status_text, wave_phase));
                live_turn_status_revealer.set_visible(true);
                live_turn_status_revealer.set_reveal_child(true);
            } else {
                live_turn_status_revealer.set_reveal_child(false);
                live_turn_timer_label.set_text("00:00");
                live_turn_status_label.set_use_markup(false);
                live_turn_status_label.set_text("Working...");
            }

            gtk::glib::ControlFlow::Continue
        });
    }

    content_box.append(&chat_frame);
    content_box
}

fn build_thread_stack_state(label_text: &str, _show_spinner: bool) -> (gtk::Box, gtk::Label) {
    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content_box.set_margin_start(0);
    content_box.set_margin_end(14);
    content_box.set_margin_top(0);
    content_box.set_margin_bottom(0);
    content_box.set_vexpand(true);

    let chat_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    chat_frame.add_css_class("chat-frame");
    chat_frame.set_vexpand(true);

    let center = gtk::Box::new(gtk::Orientation::Vertical, 8);
    center.set_vexpand(true);
    center.set_valign(gtk::Align::Center);
    center.set_halign(gtk::Align::Center);

    let heading = gtk::Label::new(Some(label_text));
    heading.add_css_class("compact-heading");
    center.append(&heading);

    chat_frame.append(&center);
    content_box.append(&chat_frame);
    (content_box, heading)
}

pub fn build_chat_tab(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    codex: Option<Arc<RuntimeClient>>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> gtk::Box {
    let host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    host.set_vexpand(true);

    let pane_stack = gtk::Stack::new();
    pane_stack.set_vexpand(true);
    pane_stack.set_hexpand(true);
    pane_stack.set_widget_name("chat-thread-pane-stack");
    pane_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    pane_stack.set_transition_duration(150);

    let has_workspaces = db
        .list_workspaces_with_threads()
        .map(|items| !items.is_empty())
        .unwrap_or(false);
    let initial_empty_heading = if has_workspaces {
        "Select a Thread"
    } else {
        "Add a Workspace"
    };
    let (empty_state, empty_heading) = build_thread_stack_state(initial_empty_heading, false);
    let empty_install_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    empty_install_box.add_css_class("welcome-section");
    empty_install_box.set_halign(gtk::Align::Center);
    empty_install_box.set_visible(false);
    if let Some(chat_frame) = empty_state.first_child().and_downcast::<gtk::Box>() {
        if let Some(center) = chat_frame.first_child().and_downcast::<gtk::Box>() {
            let install_hint = gtk::Label::new(Some("Install a supported runtime CLI first:"));
            install_hint.set_xalign(0.0);
            install_hint.add_css_class("welcome-muted");
            empty_install_box.append(&install_hint);

            let install_command_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            install_command_row.add_css_class("welcome-code-block");
            let install_command = gtk::Label::new(Some(
                "npm i -g @openai/codex  # or: npm install -g opencode-ai",
            ));
            install_command.add_css_class("welcome-code-text");
            install_command.set_xalign(0.0);
            install_command.set_hexpand(true);
            install_command.set_selectable(true);
            install_command.set_focusable(false);
            install_command_row.append(&install_command);

            let copy_install_button = gtk::Button::new();
            copy_install_button.add_css_class("app-flat-button");
            copy_install_button.add_css_class("welcome-icon-copy");
            copy_install_button.set_tooltip_text(Some("Copy install command"));
            let copy_install_icon = gtk::Image::from_icon_name("edit-copy-symbolic");
            copy_install_icon.set_pixel_size(14);
            copy_install_button.set_child(Some(&copy_install_icon));
            copy_install_button.connect_clicked(move |_| {
                if let Some(display) = gtk::gdk::Display::default() {
                    display
                        .clipboard()
                        .set_text("npm i -g @openai/codex  # or: npm install -g opencode-ai");
                }
            });
            install_command_row.append(&copy_install_button);
            empty_install_box.append(&install_command_row);
            center.append(&empty_install_box);
        }
    }
    let (loading_state, _) = build_thread_stack_state("Loading thread...", true);

    pane_stack.add_named(&empty_state, Some("empty"));
    pane_stack.add_named(&loading_state, Some("loading"));
    let restoring_last_thread = db
        .get_setting("last_active_thread_id")
        .ok()
        .flatten()
        .and_then(|value| value.parse::<i64>().ok())
        .and_then(|thread_id| db.get_thread_record(thread_id).ok().flatten())
        .and_then(|thread| thread.remote_thread_id_owned())
        .is_some();
    pane_stack.set_visible_child_name(if restoring_last_thread {
        "loading"
    } else {
        "empty"
    });

    host.append(&pane_stack);

    let panes_by_thread: Rc<RefCell<HashMap<String, gtk::Box>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let visible_thread_id: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let startup_loading_deadline_micros: Rc<RefCell<Option<i64>>> = Rc::new(RefCell::new(
        restoring_last_thread.then(|| gtk::glib::monotonic_time() + 1_500_000),
    ));

    {
        let db = db.clone();
        let manager = manager.clone();
        let codex = codex.clone();
        let active_thread_id = active_thread_id.clone();
        let active_workspace_path = active_workspace_path.clone();
        let pane_stack = pane_stack.clone();
        let empty_heading = empty_heading.clone();
        let panes_by_thread = panes_by_thread.clone();
        let visible_thread_id = visible_thread_id.clone();
        let startup_loading_deadline_micros = startup_loading_deadline_micros.clone();
        let codex_install_state: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
        let codex_install_check_in_flight = Rc::new(RefCell::new(false));
        let codex_install_last_check_micros = Rc::new(RefCell::new(0i64));
        let (codex_install_tx, codex_install_rx) = mpsc::channel::<bool>();
        let empty_install_box = empty_install_box.clone();
        crate::ui::scheduler::every(Duration::from_millis(16), move || {
            if pane_stack.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            while let Ok(installed) = codex_install_rx.try_recv() {
                codex_install_check_in_flight.replace(false);
                codex_install_state.replace(Some(installed));
            }
            let has_workspaces = db
                .list_workspaces_with_threads()
                .map(|items| !items.is_empty())
                .unwrap_or(false);
            let should_probe_codex = has_workspaces && active_thread_id.borrow().is_none();
            if should_probe_codex && *codex_install_state.borrow() != Some(true) {
                let now = gtk::glib::monotonic_time();
                let last_check = *codex_install_last_check_micros.borrow();
                let retry_interval = if codex_install_state.borrow().is_some() {
                    3_000_000
                } else {
                    0
                };
                if !*codex_install_check_in_flight.borrow() && now - last_check >= retry_interval {
                    codex_install_check_in_flight.replace(true);
                    codex_install_last_check_micros.replace(now);
                    let tx = codex_install_tx.clone();
                    thread::spawn(move || {
                        let _ = tx.send(crate::services::app::runtime::any_runtime_cli_available());
                    });
                }
            }
            let codex_missing =
                should_probe_codex && matches!(*codex_install_state.borrow(), Some(false));
            empty_install_box.set_visible(codex_missing);
            empty_heading.set_text(if codex_missing {
                "Install Runtime CLI"
            } else if has_workspaces {
                "Select a Thread"
            } else {
                "Add a Workspace"
            });
            let active_thread = active_thread_id.borrow().clone();
            let pending_view_key = if active_thread.is_none() {
                db.get_setting("pending_profile_thread_id")
                    .ok()
                    .flatten()
                    .and_then(|value| value.parse::<i64>().ok())
                    .and_then(|thread_id| db.get_thread_record(thread_id).ok().flatten())
                    .and_then(|thread| {
                        let unresolved = thread
                            .remote_thread_id()
                            .map(|value| value.trim().is_empty())
                            .unwrap_or(true);
                        unresolved.then(|| format!("pending-local:{}", thread.id))
                    })
            } else {
                None
            };
            let desired_view = active_thread.clone().or(pending_view_key.clone());
            if *visible_thread_id.borrow() == desired_view {
                return gtk::glib::ControlFlow::Continue;
            }

            let Some(thread_id) = desired_view else {
                if let Some(deadline) = *startup_loading_deadline_micros.borrow() {
                    if gtk::glib::monotonic_time() < deadline {
                        pane_stack.set_visible_child_name("loading");
                        return gtk::glib::ControlFlow::Continue;
                    }
                    startup_loading_deadline_micros.replace(None);
                }
                pane_stack.set_visible_child_name("empty");
                visible_thread_id.replace(None);
                return gtk::glib::ControlFlow::Continue;
            };
            startup_loading_deadline_micros.replace(None);

            if let Some(local_id_str) = thread_id.strip_prefix("pending-local:") {
                let local_id = local_id_str.parse::<i64>().ok();
                let child_name = format!("thread:{thread_id}");
                if !panes_by_thread.borrow().contains_key(&thread_id) {
                    let pane_active_thread_id = Rc::new(RefCell::new(None));
                    let pane_workspace = local_id
                        .and_then(|id| db.workspace_path_for_local_thread(id).ok().flatten())
                        .or_else(|| active_workspace_path.borrow().clone());
                    let pane_active_workspace_path = Rc::new(RefCell::new(pane_workspace));
                    let pane = build_chat_tab_single(
                        db.clone(),
                        manager.clone(),
                        codex.clone(),
                        pane_active_thread_id,
                        active_thread_id.clone(),
                        true,
                        pane_active_workspace_path,
                    );
                    pane_stack.add_named(&pane, Some(&child_name));
                    panes_by_thread.borrow_mut().insert(thread_id.clone(), pane);
                }
                pane_stack.set_visible_child_name(&child_name);
                visible_thread_id.replace(Some(thread_id));
                return gtk::glib::ControlFlow::Continue;
            }

            let child_name = format!("thread:{thread_id}");
            if !panes_by_thread.borrow().contains_key(&thread_id) {
                pane_stack.set_visible_child_name("loading");
                visible_thread_id.replace(Some(thread_id.clone()));
                let db = db.clone();
                let manager = manager.clone();
                let codex = codex.clone();
                let active_thread_id = active_thread_id.clone();
                let active_workspace_path = active_workspace_path.clone();
                let pane_stack = pane_stack.clone();
                let panes_by_thread = panes_by_thread.clone();
                let visible_thread_id = visible_thread_id.clone();
                let thread_id_for_build = thread_id.clone();
                crate::ui::scheduler::once(Duration::from_millis(120), move || {
                    if active_thread_id.borrow().as_deref() != Some(thread_id_for_build.as_str()) {
                        return;
                    }
                    if panes_by_thread.borrow().contains_key(&thread_id_for_build) {
                        return;
                    }

                    let child_name = format!("thread:{thread_id_for_build}");
                    let pane_active_thread_id =
                        Rc::new(RefCell::new(Some(thread_id_for_build.clone())));
                    let pane_workspace = db
                        .workspace_path_for_remote_thread(&thread_id_for_build)
                        .ok()
                        .flatten()
                        .or_else(|| active_workspace_path.borrow().clone());
                    let pane_active_workspace_path = Rc::new(RefCell::new(pane_workspace));
                    let pane = build_chat_tab_single(
                        db.clone(),
                        manager.clone(),
                        codex.clone(),
                        pane_active_thread_id,
                        active_thread_id.clone(),
                        true,
                        pane_active_workspace_path,
                    );
                    pane_stack.add_named(&pane, Some(&child_name));
                    panes_by_thread
                        .borrow_mut()
                        .insert(thread_id_for_build.clone(), pane);
                    pane_stack.set_visible_child_name(&child_name);
                    visible_thread_id.replace(Some(thread_id_for_build));
                });
                return gtk::glib::ControlFlow::Continue;
            }

            pane_stack.set_visible_child_name(&child_name);
            visible_thread_id.replace(Some(thread_id));
            gtk::glib::ControlFlow::Continue
        });
    }

    host
}
