use crate::codex_profiles::CodexProfileManager;
use crate::data::AppDb;
use crate::ui::components::thread_list;
use gtk::prelude::*;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ui::components::file_preview;

pub(super) struct CommandUi {
    pub(super) header_label: gtk::Label,
    pub(super) command_detail_label: gtk::Label,
    pub(super) status_label: gtk::Label,
    pub(super) headline_text: Rc<RefCell<String>>,
    pub(super) is_running: Rc<RefCell<bool>>,
    pub(super) running_wave_source: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    pub(super) running_wave_phase: Rc<RefCell<f64>>,
    pub(super) revealer: gtk::Revealer,
    pub(super) output_label: gtk::Label,
    pub(super) output_text: Rc<RefCell<String>>,
    pub(super) output_toggle: gtk::Box,
    pub(super) output_toggle_label: gtk::Label,
    pub(super) output_toggle_enabled: Rc<RefCell<bool>>,
}

pub(super) struct ToolCallUi {
    pub(super) tool_label: gtk::Label,
    pub(super) args_label: gtk::Label,
    pub(super) status_label: gtk::Label,
    pub(super) output_label: gtk::Label,
    pub(super) details_revealer: gtk::Revealer,
    pub(super) output_text: Rc<RefCell<String>>,
}

pub(super) struct GenericItemUi {
    pub(super) section_label: gtk::Label,
    pub(super) title_label: gtk::Label,
    pub(super) status_label: gtk::Label,
    pub(super) summary_label: gtk::Label,
    pub(super) output_label: gtk::Label,
    pub(super) output_scroll: gtk::ScrolledWindow,
    pub(super) details_revealer: gtk::Revealer,
    pub(super) details_enabled: Rc<RefCell<bool>>,
    pub(super) headline_text: Rc<RefCell<String>>,
    pub(super) is_running: Rc<RefCell<bool>>,
    pub(super) running_wave_source: Rc<RefCell<Option<gtk::glib::SourceId>>>,
    pub(super) running_wave_phase: Rc<RefCell<f64>>,
    pub(super) wave_enabled: bool,
    pub(super) details_supported: bool,
    pub(super) output_text: Rc<RefCell<String>>,
}

const COMMAND_PREVIEW_CHARS: usize = 120;
const DETAIL_TEXT_MAX_LINES: usize = 80;
const DETAIL_TEXT_MAX_CHARS: usize = 12_000;
const THINKING_OUTPUT_MAX_LINES: i32 = 20;
const COMMAND_OUTPUT_MIN_HEIGHT: i32 = 180;
const COMMAND_OUTPUT_MAX_HEIGHT: i32 = 360;
const STREAM_REVEAL_IDLE_MS: u64 = 120;
const STREAM_REVEAL_POLL_MS: u64 = 35;
const AUTO_SCROLL_FOLLOW_POLL_MS: u64 = 16;
const AUTO_SCROLL_FOLLOW_SETTLE_MS: i64 = 900;
const COMPLETION_TIMESTAMP_FADE_MS: u64 = 180;
const COMPLETION_TIMESTAMP_PLACEHOLDER: &str = "Sep 30 2026, 23:59";

#[derive(Clone, Default)]
struct ChatContextEntry {
    db: Option<Rc<AppDb>>,
    manager: Option<Rc<CodexProfileManager>>,
    thread_id: Option<String>,
}

#[derive(Default)]
struct RevealQueueState {
    queue: VecDeque<gtk::glib::WeakRef<gtk::Revealer>>,
    active: bool,
    last_enqueue_micros: i64,
    pump_source: Option<gtk::glib::SourceId>,
}

#[derive(Default)]
struct ActionSummaryWaveState {
    is_running: bool,
    running_wave_source: Option<gtk::glib::SourceId>,
    running_wave_phase: f64,
}

#[derive(Clone)]
struct ActionSectionUi {
    summary_label: gtk::Label,
    list: gtk::Box,
}

thread_local! {
    static CHAT_CONTEXT_REGISTRY: RefCell<HashMap<usize, ChatContextEntry>> = RefCell::new(HashMap::new());
    static STREAM_REVEAL_QUEUE: RefCell<RevealQueueState> = RefCell::new(RevealQueueState::default());
    static ACTION_SUMMARY_WAVE_REGISTRY: RefCell<HashMap<usize, ActionSummaryWaveState>> = RefCell::new(HashMap::new());
    static BOTTOM_SCROLL_FOLLOW_REGISTRY: RefCell<HashMap<usize, Rc<RefCell<i64>>>> = RefCell::new(HashMap::new());
    static AUTO_SCROLL_PAUSE_REGISTRY: RefCell<HashMap<usize, bool>> = RefCell::new(HashMap::new());
    static AUTO_SCROLL_TRACKING_REGISTRY: RefCell<HashSet<usize>> = RefCell::new(HashSet::new());
    static CHAT_REASONING_TOGGLE_REGISTRY: RefCell<HashMap<usize, gtk::glib::WeakRef<gtk::Box>>> = RefCell::new(HashMap::new());
}

fn process_stream_reveal_queue() -> gtk::glib::ControlFlow {
    STREAM_REVEAL_QUEUE.with(|state_cell| {
        let mut state = state_cell.borrow_mut();
        if state.active {
            return gtk::glib::ControlFlow::Continue;
        }

        while let Some(front) = state.queue.front() {
            if front.upgrade().is_none() {
                state.queue.pop_front();
            } else {
                break;
            }
        }

        if state.queue.is_empty() {
            state.pump_source.take();
            return gtk::glib::ControlFlow::Break;
        }

        let now = gtk::glib::monotonic_time();
        let wait_ms = STREAM_REVEAL_IDLE_MS;
        if now - state.last_enqueue_micros < (wait_ms as i64) * 1_000 {
            return gtk::glib::ControlFlow::Continue;
        }

        let Some(next) = state.queue.pop_front() else {
            return gtk::glib::ControlFlow::Continue;
        };
        if let Some(revealer) = next.upgrade() {
            state.active = true;
            revealer.set_reveal_child(true);
            let revealer_widget: gtk::Widget = revealer.clone().upcast();
            if let Some(messages_scroll) = find_ancestor_messages_scroll(&revealer_widget) {
                scroll_to_bottom(&messages_scroll);
            }
            let transition_ms =
                u64::from(revealer.transition_duration()).max(STREAM_REVEAL_IDLE_MS) + 30;
            gtk::glib::timeout_add_local_once(Duration::from_millis(transition_ms), move || {
                STREAM_REVEAL_QUEUE.with(|state_cell| {
                    state_cell.borrow_mut().active = false;
                });
            });
        }
        gtk::glib::ControlFlow::Continue
    })
}

fn messages_scroll_registry_key(messages_scroll: &gtk::ScrolledWindow) -> usize {
    messages_scroll.as_ptr() as usize
}

fn find_ancestor_messages_scroll(widget: &gtk::Widget) -> Option<gtk::ScrolledWindow> {
    let mut cursor = widget.parent();
    while let Some(node) = cursor {
        if let Ok(scroll) = node.clone().downcast::<gtk::ScrolledWindow>() {
            if scroll.widget_name() == "chat-messages-scroll" {
                return Some(scroll);
            }
        }
        cursor = node.parent();
    }
    None
}

fn find_ancestor_messages_box(widget: &gtk::Widget) -> Option<gtk::Box> {
    let mut cursor = widget.parent();
    while let Some(node) = cursor {
        if let Ok(messages_box) = node.clone().downcast::<gtk::Box>() {
            if messages_box.widget_name() == "chat-messages-box" {
                return Some(messages_box);
            }
        }
        cursor = node.parent();
    }
    None
}

fn find_ancestor_assistant_surface(widget: &gtk::Widget) -> Option<gtk::Box> {
    let mut cursor = widget.parent();
    while let Some(node) = cursor {
        if let Ok(surface) = node.clone().downcast::<gtk::Box>() {
            if surface.has_css_class("chat-assistant-surface") {
                return Some(surface);
            }
        }
        cursor = node.parent();
    }
    None
}

pub(super) fn messages_reasoning_visible(messages_box: &gtk::Box) -> bool {
    messages_box.has_css_class("chat-reasoning-visible")
}

fn is_scrolled_to_bottom(messages_scroll: &gtk::ScrolledWindow) -> bool {
    let adj = messages_scroll.vadjustment();
    let lower = adj.lower();
    let bottom = (adj.upper() - adj.page_size()).max(lower);
    adj.value() >= (bottom - 1.0)
}

fn is_auto_scroll_paused(messages_scroll: &gtk::ScrolledWindow) -> bool {
    let key = messages_scroll_registry_key(messages_scroll);
    AUTO_SCROLL_PAUSE_REGISTRY
        .with(|registry| registry.borrow().get(&key).copied().unwrap_or(false))
}

fn set_auto_scroll_paused(messages_scroll: &gtk::ScrolledWindow, paused: bool) {
    let key = messages_scroll_registry_key(messages_scroll);
    AUTO_SCROLL_PAUSE_REGISTRY.with(|registry| {
        registry.borrow_mut().insert(key, paused);
    });
    if paused {
        BOTTOM_SCROLL_FOLLOW_REGISTRY.with(|registry| {
            registry.borrow_mut().remove(&key);
        });
    }
}

pub(super) fn resume_auto_scroll(messages_scroll: &gtk::ScrolledWindow) {
    set_auto_scroll_paused(messages_scroll, false);
}

pub(super) fn should_follow_auto_scroll(messages_scroll: &gtk::ScrolledWindow) -> bool {
    !is_auto_scroll_paused(messages_scroll)
}

pub(super) fn register_auto_scroll_user_tracking(messages_scroll: &gtk::ScrolledWindow) {
    let key = messages_scroll_registry_key(messages_scroll);
    let should_attach = AUTO_SCROLL_TRACKING_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.contains(&key) {
            false
        } else {
            registry.insert(key);
            true
        }
    });
    if !should_attach {
        return;
    }

    AUTO_SCROLL_PAUSE_REGISTRY.with(|registry| {
        registry.borrow_mut().insert(key, false);
    });

    {
        let messages_scroll = messages_scroll.clone();
        let scroll_ctrl =
            gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let messages_scroll_for_cb = messages_scroll.clone();
        scroll_ctrl.connect_scroll(move |_, _, dy| {
            if dy < -0.01 {
                set_auto_scroll_paused(&messages_scroll_for_cb, true);
            }
            gtk::glib::Propagation::Proceed
        });
        messages_scroll.add_controller(scroll_ctrl);
    }

    {
        let messages_scroll = messages_scroll.clone();
        let click_ctrl = gtk::GestureClick::builder().button(1).build();
        let messages_scroll_for_press = messages_scroll.clone();
        let messages_scroll_for_release = messages_scroll.clone();
        click_ctrl.connect_pressed(move |_, _, _, _| {
            set_auto_scroll_paused(&messages_scroll_for_press, true);
        });
        click_ctrl.connect_released(move |_, _, _, _| {
            if is_scrolled_to_bottom(&messages_scroll_for_release) {
                set_auto_scroll_paused(&messages_scroll_for_release, false);
            }
        });
        messages_scroll.add_controller(click_ctrl);
    }

    {
        let messages_scroll = messages_scroll.clone();
        let adj = messages_scroll.vadjustment();
        adj.connect_value_changed(move |_| {
            if is_auto_scroll_paused(&messages_scroll) && is_scrolled_to_bottom(&messages_scroll) {
                set_auto_scroll_paused(&messages_scroll, false);
            }
        });
    }

    messages_scroll.connect_destroy(move |_| {
        AUTO_SCROLL_TRACKING_REGISTRY.with(|registry| {
            registry.borrow_mut().remove(&key);
        });
        AUTO_SCROLL_PAUSE_REGISTRY.with(|registry| {
            registry.borrow_mut().remove(&key);
        });
        BOTTOM_SCROLL_FOLLOW_REGISTRY.with(|registry| {
            registry.borrow_mut().remove(&key);
        });
    });
}

fn enqueue_stream_revealer(revealer: &gtk::Revealer) {
    STREAM_REVEAL_QUEUE.with(|state_cell| {
        let mut state = state_cell.borrow_mut();
        state.queue.push_back(revealer.downgrade());
        state.last_enqueue_micros = gtk::glib::monotonic_time();
        if state.pump_source.is_none() {
            let source = gtk::glib::timeout_add_local(
                Duration::from_millis(STREAM_REVEAL_POLL_MS),
                process_stream_reveal_queue,
            );
            state.pump_source.replace(source);
        }
    });
}

fn messages_box_registry_key(messages_box: &gtk::Box) -> usize {
    messages_box.as_ptr() as usize
}

fn register_chat_context(messages_box: &gtk::Box, entry: ChatContextEntry) {
    let key = messages_box_registry_key(messages_box);
    CHAT_CONTEXT_REGISTRY.with(|registry| {
        registry.borrow_mut().insert(key, entry);
    });
    messages_box.connect_destroy(move |_| {
        CHAT_CONTEXT_REGISTRY.with(|registry| {
            registry.borrow_mut().remove(&key);
        });
    });
}

fn update_chat_context(messages_box: &gtk::Box, f: impl FnOnce(&mut ChatContextEntry)) {
    let key = messages_box_registry_key(messages_box);
    CHAT_CONTEXT_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let entry = registry.entry(key).or_default();
        f(entry);
    });
}

pub(super) fn chat_handles_for_messages_box(
    messages_box: &gtk::Box,
) -> Option<(Rc<AppDb>, Rc<CodexProfileManager>)> {
    let key = messages_box_registry_key(messages_box);
    CHAT_CONTEXT_REGISTRY.with(|registry| {
        let registry = registry.borrow();
        let entry = registry.get(&key)?;
        let db = entry.db.as_ref()?.clone();
        let manager = entry.manager.as_ref()?.clone();
        Some((db, manager))
    })
}

pub(super) fn chat_thread_id_for_messages_box(messages_box: &gtk::Box) -> Option<String> {
    let key = messages_box_registry_key(messages_box);
    CHAT_CONTEXT_REGISTRY.with(|registry| {
        registry
            .borrow()
            .get(&key)
            .and_then(|entry| entry.thread_id.clone())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn truncate_to_lines(text: &str, max_lines: usize) -> (String, bool) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= max_lines {
        (text.to_string(), false)
    } else {
        let mut truncated: String = lines[..max_lines].join("\n");
        truncated.push_str(" …");
        (truncated, true)
    }
}

fn normalize_single_line(text: &str) -> String {
    text.lines()
        .flat_map(|line| line.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
}

fn truncate_to_chars(text: &str, max_chars: usize) -> (String, bool) {
    let mut count = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        if count >= max_chars {
            out.push('…');
            return (out, true);
        }
        out.push(ch);
        count += 1;
    }
    (out, false)
}

fn truncate_detail_text(text: &str) -> (String, bool) {
    let (lines_clipped, line_truncated) = truncate_to_lines(text, DETAIL_TEXT_MAX_LINES);
    let (chars_clipped, char_truncated) = truncate_to_chars(&lines_clipped, DETAIL_TEXT_MAX_CHARS);
    let truncated = line_truncated || char_truncated;
    if truncated {
        (
            format!("{}\n… output truncated", chars_clipped.trim_end()),
            true,
        )
    } else {
        (chars_clipped, false)
    }
}

fn wave_markup_for_text(text: &str, phase: f64) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return String::new();
    }

    let tail = 6.0f64;
    let cycle = chars.len() as f64 + tail;
    let center = phase.rem_euclid(cycle);

    let base = (149.0f64, 160.0f64, 173.0f64);
    let highlight = (238.0f64, 242.0f64, 247.0f64);
    let sigma = 1.2f64;

    let mut markup = String::with_capacity(text.len() * 24);
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

pub(super) fn set_plain_label_text(label: &gtk::Label, text: &str) {
    let display = if label.has_css_class("chat-command-output")
        && !label.has_css_class("chat-thinking-output")
    {
        truncate_detail_text(text).0
    } else {
        text.to_string()
    };
    let escaped = gtk::glib::markup_escape_text(&display);
    label.set_attributes(None);
    label.set_use_markup(true);
    label.set_markup(escaped.as_str());
}

pub(super) fn bind_chat_context(
    messages_box: &gtk::Box,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
) {
    let key = messages_box_registry_key(messages_box);
    let mut existing = CHAT_CONTEXT_REGISTRY.with(|registry| registry.borrow().get(&key).cloned());
    if existing.is_none() {
        existing = Some(ChatContextEntry::default());
    }
    let mut next = existing.unwrap_or_default();
    next.db = Some(db);
    next.manager = Some(manager);
    register_chat_context(messages_box, next);
}

pub(super) fn set_messages_box_thread_context(messages_box: &gtk::Box, thread_id: Option<&str>) {
    let value = thread_id.unwrap_or_default().trim().to_string();
    update_chat_context(messages_box, move |entry| {
        entry.thread_id = if value.is_empty() { None } else { Some(value) };
    });
}

impl CommandUi {
    pub(super) fn set_command_headline(&self, text: &str) {
        let normalized = normalize_single_line(text);
        let (display, _) = truncate_to_chars(&normalized, COMMAND_PREVIEW_CHARS);
        self.headline_text.replace(display.clone());
        if *self.is_running.borrow() {
            let phase = *self.running_wave_phase.borrow();
            self.header_label.set_use_markup(true);
            self.header_label
                .set_markup(&wave_markup_for_text(&display, phase));
        } else {
            set_plain_label_text(&self.header_label, &display);
        }
        set_plain_label_text(&self.command_detail_label, text);
    }

    pub(super) fn set_command_status_label(&self, status_text: &str) {
        self.status_label.set_text(status_text);
        self.set_running(status_text.to_ascii_lowercase().starts_with("running"));
    }

    pub(super) fn set_running(&self, running: bool) {
        if *self.is_running.borrow() == running {
            return;
        }
        self.is_running.replace(running);
        if let Some(source_id) = self.running_wave_source.borrow_mut().take() {
            source_id.remove();
        }
        if running {
            let header_label = self.header_label.clone();
            let headline_text = self.headline_text.clone();
            let wave_phase = self.running_wave_phase.clone();
            let source = gtk::glib::timeout_add_local(Duration::from_millis(33), move || {
                let next_phase = gtk::glib::monotonic_time() as f64 / 90_000.0;
                wave_phase.replace(next_phase);
                let text = headline_text.borrow().clone();
                header_label.set_use_markup(true);
                header_label.set_markup(&wave_markup_for_text(&text, next_phase));
                gtk::glib::ControlFlow::Continue
            });
            self.running_wave_source.replace(Some(source));
            let text = self.headline_text.borrow().clone();
            let phase = gtk::glib::monotonic_time() as f64 / 90_000.0;
            self.running_wave_phase.replace(phase);
            self.header_label.set_use_markup(true);
            self.header_label
                .set_markup(&wave_markup_for_text(&text, phase));
        } else {
            self.running_wave_phase.replace(0.0);
            let text = self.headline_text.borrow().clone();
            set_plain_label_text(&self.header_label, &text);
        }
    }

    pub(super) fn set_command_output(&mut self, output: &str) {
        self.output_text.replace(output.to_string());
        if self.revealer.reveals_child() {
            set_plain_label_text(&self.output_label, output);
        } else {
            set_plain_label_text(&self.output_label, "");
        }
        if output.trim().is_empty() {
            set_plain_label_text(&self.output_toggle_label, "No output");
            *self.output_toggle_enabled.borrow_mut() = false;
            self.output_toggle.add_css_class("disabled");
        } else {
            set_plain_label_text(&self.output_toggle_label, "Show output");
            *self.output_toggle_enabled.borrow_mut() = true;
            self.output_toggle.remove_css_class("disabled");
        }
    }
}

impl ToolCallUi {
    pub(super) fn set_output(&self, output: &str) {
        self.output_text.replace(output.to_string());
        self.apply_deferred_output();
    }

    pub(super) fn append_output_delta(&self, delta: &str) {
        self.output_text.borrow_mut().push_str(delta);
        self.apply_deferred_output();
    }

    fn apply_deferred_output(&self) {
        if self.details_revealer.reveals_child() {
            let output = self.output_text.borrow();
            set_plain_label_text(&self.output_label, output.as_str());
            self.output_label.set_visible(!output.trim().is_empty());
        } else {
            set_plain_label_text(&self.output_label, "");
            self.output_label.set_visible(false);
        }
    }
}

impl GenericItemUi {
    fn sync_output_scroll_layout(&self) {
        let output = self.output_text.borrow();
        sync_thinking_output_scroll_layout(&self.output_label, &self.output_scroll, output.trim());
    }

    fn schedule_output_scroll_layout_sync(&self) {
        if !self.output_label.has_css_class("chat-thinking-output") {
            return;
        }

        schedule_thinking_output_scroll_layout_sync(
            self.output_label.clone(),
            self.output_scroll.clone(),
            self.output_text.clone(),
            0,
        );
    }

    pub(super) fn set_title(&self, text: &str) {
        let normalized = normalize_single_line(text);
        let (display, _) = truncate_to_chars(&normalized, COMMAND_PREVIEW_CHARS);
        self.headline_text.replace(display.clone());
        if self.wave_enabled && *self.is_running.borrow() {
            let phase = *self.running_wave_phase.borrow();
            self.title_label.set_use_markup(true);
            self.title_label
                .set_markup(&wave_markup_for_text(&display, phase));
        } else {
            set_plain_label_text(&self.title_label, &display);
        }
    }

    pub(super) fn set_running(&self, running: bool) {
        if !self.wave_enabled {
            return;
        }
        if *self.is_running.borrow() == running {
            return;
        }
        self.is_running.replace(running);
        if let Some(source_id) = self.running_wave_source.borrow_mut().take() {
            source_id.remove();
        }
        if running {
            let title_label = self.title_label.clone();
            let headline_text = self.headline_text.clone();
            let wave_phase = self.running_wave_phase.clone();
            let source = gtk::glib::timeout_add_local(Duration::from_millis(33), move || {
                let next_phase = gtk::glib::monotonic_time() as f64 / 90_000.0;
                wave_phase.replace(next_phase);
                let text = headline_text.borrow().clone();
                title_label.set_use_markup(true);
                title_label.set_markup(&wave_markup_for_text(&text, next_phase));
                gtk::glib::ControlFlow::Continue
            });
            self.running_wave_source.replace(Some(source));
            let text = self.headline_text.borrow().clone();
            let phase = gtk::glib::monotonic_time() as f64 / 90_000.0;
            self.running_wave_phase.replace(phase);
            self.title_label.set_use_markup(true);
            self.title_label
                .set_markup(&wave_markup_for_text(&text, phase));
        } else {
            self.running_wave_phase.replace(0.0);
            let text = self.headline_text.borrow().clone();
            set_plain_label_text(&self.title_label, &text);
        }
    }

    pub(super) fn set_status(&self, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            self.status_label.set_visible(false);
            self.status_label.set_text("");
        } else {
            self.status_label.set_visible(true);
            self.status_label.set_text(text);
        }
    }

    pub(super) fn set_details(&self, summary: &str, output: &str) {
        if !self.details_supported {
            self.summary_label.set_visible(false);
            self.output_label.set_visible(false);
            self.output_scroll.set_visible(false);
            self.output_text.replace(String::new());
            self.details_enabled.replace(false);
            self.details_revealer.set_reveal_child(false);
            return;
        }
        set_plain_label_text(&self.summary_label, summary);
        self.output_text.replace(output.to_string());
        let show_summary = !summary.trim().is_empty();
        let show_output = !self.output_text.borrow().trim().is_empty();
        if self.output_label.has_css_class("chat-thinking-output") {
            let output = self.output_text.borrow();
            set_plain_label_text(&self.output_label, output.as_str());
            self.output_label.set_visible(show_output);
            self.output_scroll.set_visible(show_output);
            drop(output);
            self.sync_output_scroll_layout();
        } else if self.details_revealer.reveals_child() {
            let output = self.output_text.borrow();
            set_plain_label_text(&self.output_label, output.as_str());
            self.output_label.set_visible(!output.trim().is_empty());
            self.output_scroll.set_visible(!output.trim().is_empty());
        } else {
            set_plain_label_text(&self.output_label, "");
            self.output_label.set_visible(false);
            self.output_scroll.set_visible(false);
        }
        self.summary_label.set_visible(show_summary);
        let enabled = show_summary || show_output;
        self.details_enabled.replace(enabled);
        if !enabled {
            self.details_revealer.set_reveal_child(false);
        } else if self.output_label.has_css_class("chat-thinking-output")
            || self.details_revealer.reveals_child()
        {
            self.schedule_output_scroll_layout_sync();
        }
    }

    pub(super) fn set_expanded(&self, expanded: bool) {
        if !*self.details_enabled.borrow() {
            self.details_revealer.set_reveal_child(false);
            set_plain_label_text(&self.output_label, "");
            self.output_label.set_visible(false);
            self.output_scroll.set_visible(false);
            return;
        }
        if expanded {
            let output = self.output_text.borrow();
            self.sync_output_scroll_layout();
            set_plain_label_text(&self.output_label, output.as_str());
            self.output_label.set_visible(!output.trim().is_empty());
            self.output_scroll.set_visible(!output.trim().is_empty());
        } else if !self.output_label.has_css_class("chat-thinking-output") {
            set_plain_label_text(&self.output_label, "");
            self.output_label.set_visible(false);
            self.output_scroll.set_visible(false);
        }
        self.details_revealer.set_reveal_child(expanded);
        if expanded {
            self.schedule_output_scroll_layout_sync();
        }
    }

    pub(super) fn output_text(&self) -> String {
        self.output_text.borrow().clone()
    }
}

fn sync_thinking_output_scroll_layout(
    output_label: &gtk::Label,
    output_scroll: &gtk::ScrolledWindow,
    output: &str,
) -> bool {
    if !output_label.has_css_class("chat-thinking-output") {
        return true;
    }

    let output = output.trim();
    let line_height = output_label
        .create_pango_layout(Some("Ag"))
        .pixel_size()
        .1
        .max(1);
    let max_height = line_height * THINKING_OUTPUT_MAX_LINES;
    let available_width = output_scroll.width().max(output_label.width());

    if output.is_empty() {
        output_scroll.set_propagate_natural_height(false);
        output_scroll.set_min_content_height(0);
        output_scroll.set_max_content_height(0);
        return true;
    }

    if available_width <= 1 {
        output_scroll.set_propagate_natural_height(false);
        output_scroll.set_min_content_height(line_height * 3);
        output_scroll.set_max_content_height(max_height);
        return false;
    }

    let previous_text = output_label.text().to_string();
    set_plain_label_text(output_label, output);
    let (_, rendered_height, _, _) =
        output_label.measure(gtk::Orientation::Vertical, available_width.max(1));
    if previous_text != output {
        set_plain_label_text(output_label, &previous_text);
    }
    let visible_height = rendered_height.min(max_height).max(line_height);

    output_scroll.set_propagate_natural_height(false);
    output_scroll.set_min_content_height(visible_height);
    output_scroll.set_max_content_height(visible_height);
    true
}

fn schedule_thinking_output_scroll_layout_sync(
    output_label: gtk::Label,
    output_scroll: gtk::ScrolledWindow,
    output_text: Rc<RefCell<String>>,
    attempt: u8,
) {
    gtk::glib::idle_add_local_once(move || {
        let settled = {
            let output = output_text.borrow();
            sync_thinking_output_scroll_layout(&output_label, &output_scroll, output.trim())
        };
        if !settled && attempt < 6 {
            let output_label = output_label.clone();
            let output_scroll = output_scroll.clone();
            let output_text = output_text.clone();
            gtk::glib::timeout_add_local_once(Duration::from_millis(16), move || {
                schedule_thinking_output_scroll_layout_sync(
                    output_label,
                    output_scroll,
                    output_text,
                    attempt + 1,
                );
            });
        }
    });
}

const FIRST_MESSAGE_TOP_MARGIN: i32 = 18;

fn set_chat_label_selectable(label: &gtk::Label) {
    label.set_selectable(true);
    label.set_focusable(false);
}

pub(super) fn apply_first_message_top_spacing(messages_box: &gtk::Box, row: &gtk::Box) {
    let mut child = messages_box.first_child();
    while let Some(node) = child {
        if node.has_css_class("chat-message-row") {
            return;
        }
        child = node.next_sibling();
    }
    if row.has_css_class("chat-message-row") {
        row.set_margin_top(FIRST_MESSAGE_TOP_MARGIN);
    }
}

pub(super) fn append_message(
    messages_box: &gtk::Box,
    messages_scroll: Option<&gtk::ScrolledWindow>,
    conversation_stack: &gtk::Stack,
    text: &str,
    is_user: bool,
    timestamp: SystemTime,
) -> gtk::Label {
    let bubble = gtk::Label::new(Some(text));
    bubble.set_wrap(true);
    bubble.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    set_chat_label_selectable(&bubble);
    bubble.set_xalign(0.0);
    bubble.add_css_class("chat-message");
    if is_user {
        bubble.add_css_class("chat-message-user");
    } else {
        bubble.add_css_class("chat-message-assistant");
    }

    append_message_content_widget(
        messages_box,
        messages_scroll,
        conversation_stack,
        &bubble,
        is_user,
        timestamp,
    );

    bubble
}

pub(super) fn append_user_message_with_images(
    messages_box: &gtk::Box,
    messages_scroll: Option<&gtk::ScrolledWindow>,
    conversation_stack: &gtk::Stack,
    text: &str,
    image_paths: &[String],
    timestamp: SystemTime,
) -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.add_css_class("chat-user-message-content");

    if !text.trim().is_empty() {
        let bubble = gtk::Label::new(Some(text));
        bubble.set_wrap(true);
        bubble.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        set_chat_label_selectable(&bubble);
        bubble.set_xalign(0.0);
        bubble.add_css_class("chat-message");
        bubble.add_css_class("chat-message-user");
        content.append(&bubble);
    }

    if !image_paths.is_empty() {
        let image_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        image_row.add_css_class("chat-user-image-row");
        for path in image_paths {
            let path_buf = PathBuf::from(path);
            if !path_buf.exists() {
                continue;
            }
            let chip = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            chip.add_css_class("chat-user-image-chip");

            let preview = gtk::Image::from_file(path);
            preview.add_css_class("chat-user-image-thumb");
            preview.set_pixel_size(34);
            chip.append(&preview);

            let filename = path_buf
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("image");
            let name = gtk::Label::new(Some(filename));
            name.set_xalign(0.0);
            name.add_css_class("chat-user-image-name");
            chip.append(&name);

            let click = gtk::GestureClick::builder().button(1).build();
            let open_path = path_buf.clone();
            click.connect_released(move |_, _, _, _| {
                file_preview::open_file_preview(&open_path);
            });
            chip.add_controller(click);

            image_row.append(&chip);
        }
        if image_row.first_child().is_some() {
            content.append(&image_row);
        }
    }

    if content.first_child().is_none() {
        let fallback = gtk::Label::new(Some("[image]"));
        fallback.set_xalign(0.0);
        fallback.add_css_class("chat-message");
        fallback.add_css_class("chat-message-user");
        content.append(&fallback);
    }

    append_message_content_widget(
        messages_box,
        messages_scroll,
        conversation_stack,
        &content,
        true,
        timestamp,
    );

    content
}

fn append_message_content_widget<T: IsA<gtk::Widget> + Clone>(
    messages_box: &gtk::Box,
    messages_scroll: Option<&gtk::ScrolledWindow>,
    conversation_stack: &gtk::Stack,
    content: &T,
    is_user: bool,
    timestamp: SystemTime,
) {
    conversation_stack.set_visible_child_name("messages");
    ensure_shared_message_context_menu(messages_box);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.add_css_class("chat-message-row");
    row.set_halign(if is_user {
        gtk::Align::End
    } else {
        gtk::Align::Start
    });
    apply_first_message_top_spacing(messages_box, &row);

    append_hover_timestamp(messages_box, &row, content, is_user, timestamp);
    messages_box.append(&row);

    if let Some(scroll) = messages_scroll {
        scroll_to_bottom(scroll);
    }
}

pub(super) fn set_message_row_marker<T: IsA<gtk::Widget>>(content: &T, marker: &str) -> bool {
    let Some(shell) = content.parent() else {
        return false;
    };
    let Some(row) = shell.parent() else {
        return false;
    };
    row.set_widget_name(marker);
    true
}

pub(super) fn retag_message_row(messages_box: &gtk::Box, from: &str, to: &str) -> bool {
    let mut child = messages_box.first_child();
    while let Some(node) = child {
        if node.widget_name() == from {
            node.set_widget_name(to);
            return true;
        }
        child = node.next_sibling();
    }
    false
}

pub(super) fn append_steer_note_to_last_user_message(
    messages_box: &gtk::Box,
    messages_scroll: &gtk::ScrolledWindow,
    note_text: &str,
) -> bool {
    let mut row = messages_box.last_child();
    while let Some(node) = row {
        if let Ok(row_box) = node.clone().downcast::<gtk::Box>() {
            if row_box.halign() == gtk::Align::End {
                if let Some(shell_widget) = row_box.first_child() {
                    if let Ok(shell_box) = shell_widget.downcast::<gtk::Box>() {
                        let note = gtk::Label::new(Some(&format!("Steering applied: {note_text}")));
                        note.set_xalign(1.0);
                        note.set_wrap(true);
                        note.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                        note.add_css_class("chat-steer-note");

                        if let Some(first_child) = shell_box.first_child() {
                            shell_box.insert_child_after(&note, Some(&first_child));
                        } else {
                            shell_box.append(&note);
                        }

                        scroll_to_bottom(messages_scroll);
                        return true;
                    }
                }
            }
        }
        row = node.prev_sibling();
    }

    false
}

pub(super) fn append_hover_timestamp<T: IsA<gtk::Widget>>(
    messages_box: &gtk::Box,
    row: &gtk::Box,
    content: &T,
    is_user: bool,
    timestamp: SystemTime,
) {
    ensure_shared_message_context_menu(messages_box);
    let shell = gtk::Box::new(gtk::Orientation::Vertical, 2);
    shell.add_css_class("chat-message-shell");
    shell.set_halign(if is_user {
        gtk::Align::End
    } else {
        gtk::Align::Start
    });

    shell.append(content);

    let meta_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    meta_row.add_css_class("chat-message-meta-row");
    meta_row.set_halign(gtk::Align::Fill);

    let timestamp_label = gtk::Label::new(Some(&format_message_timestamp(timestamp)));
    timestamp_label.add_css_class("chat-message-timestamp");
    timestamp_label.set_xalign(if is_user { 1.0 } else { 0.0 });
    timestamp_label.set_halign(if is_user {
        gtk::Align::End
    } else {
        gtk::Align::Start
    });

    if is_user {
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        meta_row.append(&spacer);
        meta_row.append(&timestamp_label);
    } else {
        meta_row.append(&timestamp_label);
    }

    meta_row.set_visible(true);
    shell.append(&meta_row);

    row.append(&shell);
}

pub(super) fn make_assistant_row_full_width(row: &gtk::Box) {
    row.set_halign(gtk::Align::Fill);
    row.set_hexpand(true);
    if let Some(shell_widget) = row.first_child() {
        if let Ok(shell) = shell_widget.downcast::<gtk::Box>() {
            shell.set_halign(gtk::Align::Fill);
            shell.set_hexpand(true);
        }
    }
}

include!("message_render/context_menu.rs");
fn format_message_timestamp(timestamp: SystemTime) -> String {
    let Ok(duration) = timestamp.duration_since(UNIX_EPOCH) else {
        return String::new();
    };
    let unix = duration.as_secs() as i64;

    let Ok(value_dt) = gtk::glib::DateTime::from_unix_local(unix) else {
        return String::new();
    };
    let Ok(now_dt) = gtk::glib::DateTime::now_local() else {
        return String::new();
    };

    let is_today = value_dt.year() == now_dt.year()
        && value_dt.month() == now_dt.month()
        && value_dt.day_of_month() == now_dt.day_of_month();

    let format = if is_today {
        "%H:%M"
    } else if value_dt.year() == now_dt.year() {
        "%b %d, %H:%M"
    } else {
        "%b %d %Y, %H:%M"
    };
    value_dt
        .format(format)
        .map(|value| value.to_string())
        .unwrap_or_default()
}

pub(super) fn prepare_assistant_turn_completion_timestamp(
    row: &gtk::Box,
) -> Option<(gtk::Label, gtk::Revealer)> {
    let shell_widget = row.first_child()?;
    let shell = shell_widget.downcast::<gtk::Box>().ok()?;
    let mut meta: Option<gtk::Box> = None;
    let mut child = shell.first_child();
    while let Some(node) = child {
        if node.has_css_class("chat-message-meta-row") {
            if let Ok(meta_row) = node.clone().downcast::<gtk::Box>() {
                meta = Some(meta_row);
                break;
            }
        }
        child = node.next_sibling();
    }
    let meta = meta?;
    let mut timestamp_label: Option<gtk::Label> = None;
    let mut meta_child = meta.first_child();
    while let Some(node) = meta_child {
        if node.has_css_class("chat-message-timestamp") {
            if let Ok(label) = node.clone().downcast::<gtk::Label>() {
                timestamp_label = Some(label);
                break;
            }
        }
        meta_child = node.next_sibling();
    }
    let timestamp_label = timestamp_label?;
    timestamp_label.set_text(COMPLETION_TIMESTAMP_PLACEHOLDER);
    timestamp_label.set_opacity(0.0);
    timestamp_label.set_width_chars(COMPLETION_TIMESTAMP_PLACEHOLDER.chars().count() as i32);
    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
    revealer.set_transition_duration(220);
    revealer.set_reveal_child(true);
    revealer.set_opacity(0.0);
    meta.remove(&timestamp_label);
    revealer.set_child(Some(&timestamp_label));
    meta.append(&revealer);
    Some((timestamp_label, revealer))
}

pub(super) fn reveal_assistant_turn_completion_timestamp(
    timestamp_label: &gtk::Label,
    timestamp_revealer: &gtk::Revealer,
    completed_at: SystemTime,
) {
    timestamp_label.set_text(&format_message_timestamp(completed_at));
    timestamp_label.set_opacity(0.0);
    timestamp_revealer.set_opacity(1.0);
    timestamp_revealer.set_reveal_child(true);
    let timestamp_label = timestamp_label.clone();
    let started_at = gtk::glib::monotonic_time();
    gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
        if timestamp_label.root().is_none() {
            return gtk::glib::ControlFlow::Break;
        }
        let elapsed_ms = ((gtk::glib::monotonic_time() - started_at).max(0) as f64) / 1_000.0;
        let progress = (elapsed_ms / COMPLETION_TIMESTAMP_FADE_MS as f64).clamp(0.0, 1.0);
        timestamp_label.set_opacity(progress);
        if progress >= 1.0 {
            gtk::glib::ControlFlow::Break
        } else {
            gtk::glib::ControlFlow::Continue
        }
    });
}

pub(super) fn scroll_to_bottom(messages_scroll: &gtk::ScrolledWindow) {
    if is_auto_scroll_paused(messages_scroll) {
        return;
    }

    let key = messages_scroll_registry_key(messages_scroll);
    let settle_deadline = gtk::glib::monotonic_time() + (AUTO_SCROLL_FOLLOW_SETTLE_MS * 1_000);

    let existing_deadline =
        BOTTOM_SCROLL_FOLLOW_REGISTRY.with(|registry| registry.borrow().get(&key).cloned());
    if let Some(deadline) = existing_deadline {
        *deadline.borrow_mut() = settle_deadline;
        return;
    }

    let deadline = Rc::new(RefCell::new(settle_deadline));
    BOTTOM_SCROLL_FOLLOW_REGISTRY.with(|registry| {
        registry.borrow_mut().insert(key, deadline.clone());
    });

    let initial_scroll = messages_scroll.clone();
    crate::ui::scheduler::idle_once(move || {
        let still_registered =
            BOTTOM_SCROLL_FOLLOW_REGISTRY.with(|registry| registry.borrow().contains_key(&key));
        if !still_registered || is_auto_scroll_paused(&initial_scroll) {
            return;
        }
        let adj = initial_scroll.vadjustment();
        let lower = adj.lower();
        let target = (adj.upper() - adj.page_size()).max(lower);
        let current = adj.value();
        let delta = target - current;
        if delta.abs() > 0.5 {
            let next = current + (delta * 0.42);
            adj.set_value(next.clamp(lower, target));
        } else {
            adj.set_value(target);
        }
    });

    let messages_scroll_weak = messages_scroll.downgrade();
    let deadline_for_tick = deadline.clone();
    gtk::glib::timeout_add_local(
        Duration::from_millis(AUTO_SCROLL_FOLLOW_POLL_MS),
        move || {
            let still_registered =
                BOTTOM_SCROLL_FOLLOW_REGISTRY.with(|registry| registry.borrow().contains_key(&key));
            if !still_registered {
                return gtk::glib::ControlFlow::Break;
            }

            let Some(messages_scroll) = messages_scroll_weak.upgrade() else {
                BOTTOM_SCROLL_FOLLOW_REGISTRY.with(|registry| {
                    registry.borrow_mut().remove(&key);
                });
                return gtk::glib::ControlFlow::Break;
            };

            let adj = messages_scroll.vadjustment();
            let lower = adj.lower();
            let target = (adj.upper() - adj.page_size()).max(lower);
            let current = adj.value();
            let delta = target - current;

            if delta.abs() > 0.5 {
                let eased = current + (delta * 0.28);
                let next = if delta > 0.0 {
                    eased.max(current + 1.0).min(target)
                } else {
                    eased.min(current - 1.0).max(target)
                };
                adj.set_value(next.clamp(lower, target));
            } else {
                adj.set_value(target);
            }

            let now = gtk::glib::monotonic_time();
            let keep_following = now < *deadline_for_tick.borrow();
            let still_not_at_bottom = (target - adj.value()).abs() > 0.5;
            if keep_following || still_not_at_bottom {
                gtk::glib::ControlFlow::Continue
            } else {
                BOTTOM_SCROLL_FOLLOW_REGISTRY.with(|registry| {
                    registry.borrow_mut().remove(&key);
                });
                gtk::glib::ControlFlow::Break
            }
        },
    );
}

fn parse_link_target_with_optional_line_col(
    path: &std::path::Path,
) -> Option<(std::path::PathBuf, Option<u32>, Option<u32>)> {
    let raw = path.to_string_lossy();
    let candidate = raw.as_ref();
    let last_colon = candidate.rfind(':')?;
    let last = &candidate[(last_colon + 1)..];
    let last_value = last.parse::<u32>().ok()?;

    let maybe_with_column = &candidate[..last_colon];
    if let Some(second_colon) = maybe_with_column.rfind(':') {
        let second = &maybe_with_column[(second_colon + 1)..];
        if let Ok(line_value) = second.parse::<u32>() {
            let parsed = std::path::PathBuf::from(&maybe_with_column[..second_colon]);
            if parsed.exists() {
                return Some((parsed, Some(line_value), Some(last_value)));
            }
        }
    }

    let parsed = std::path::PathBuf::from(maybe_with_column);
    if parsed.exists() {
        return Some((parsed, Some(last_value), None));
    }
    None
}

fn parse_link_target_with_hash_anchor(
    path: &std::path::Path,
) -> Option<(std::path::PathBuf, Option<u32>, Option<u32>)> {
    let raw = path.to_string_lossy();
    let candidate = raw.as_ref();
    let hash_index = candidate.rfind('#')?;
    let base_path = std::path::PathBuf::from(&candidate[..hash_index]);
    if !base_path.exists() {
        return None;
    }

    let anchor = candidate[(hash_index + 1)..].split('-').next()?;
    if let Some(rest) = anchor.strip_prefix('L') {
        if let Some((line_raw, col_raw)) = rest.split_once('C') {
            let line = line_raw.parse::<u32>().ok()?;
            let column = col_raw.parse::<u32>().ok()?;
            return Some((base_path, Some(line), Some(column)));
        }
        let line = rest.parse::<u32>().ok()?;
        return Some((base_path, Some(line), None));
    }
    if let Some((line_raw, col_raw)) = anchor.split_once(':') {
        let line = line_raw.parse::<u32>().ok()?;
        let column = col_raw.parse::<u32>().ok()?;
        return Some((base_path, Some(line), Some(column)));
    }
    let line = anchor.parse::<u32>().ok()?;
    Some((base_path, Some(line), None))
}

fn parse_link_target_with_optional_location(
    path: &std::path::Path,
) -> Option<(std::path::PathBuf, Option<u32>, Option<u32>)> {
    parse_link_target_with_hash_anchor(path)
        .or_else(|| parse_link_target_with_optional_line_col(path))
}

fn open_link_in_preview(uri: &str) -> bool {
    let mut raw_path = if uri.starts_with("file://") {
        match gtk::glib::filename_from_uri(uri) {
            Ok((path, _)) => path,
            Err(err) => {
                let fallback = std::path::PathBuf::from(uri.trim_start_matches("file://"));
                eprintln!("[chat-link] failed to decode file URI ({err}), using fallback path");
                fallback
            }
        }
    } else {
        std::path::PathBuf::from(uri)
    };

    if !raw_path.is_absolute() {
        if let Ok(cwd) = std::env::current_dir() {
            let cwd_candidate = cwd.join(&raw_path);
            if cwd_candidate.exists()
                || parse_link_target_with_optional_location(&cwd_candidate).is_some()
            {
                raw_path = cwd_candidate;
            }
        }
        if !raw_path.is_absolute() {
            let workspace_candidate = crate::data::AppDb::open_default()
                .get_setting("last_active_workspace_path")
                .ok()
                .flatten()
                .map(std::path::PathBuf::from)
                .map(|workspace| workspace.join(&raw_path));
            if let Some(candidate) = workspace_candidate {
                if candidate.exists()
                    || parse_link_target_with_optional_location(&candidate).is_some()
                {
                    raw_path = candidate;
                }
            }
        }
    }
    let (path, line, column) = if raw_path.exists() {
        (raw_path, None, None)
    } else {
        let Some((resolved, line, column)) = parse_link_target_with_optional_location(&raw_path)
        else {
            return false;
        };
        (resolved, line, column)
    };
    file_preview::open_file_preview_at(&path, line, column);
    true
}

#[cfg(test)]
mod link_tests {
    use super::{
        parse_link_target_with_hash_anchor, parse_link_target_with_optional_line_col,
        parse_link_target_with_optional_location,
    };
    use std::fs;

    #[test]
    fn parses_line_suffix_when_target_exists() {
        let unique = format!(
            "enzimcoder-link-test-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos()
        );
        let file = std::env::temp_dir().join(unique);
        fs::write(&file, "ok").expect("write temp file");
        let with_line = std::path::PathBuf::from(format!("{}:231", file.display()));
        let (resolved, line, column) =
            parse_link_target_with_optional_line_col(&with_line).expect("resolve line suffix");
        assert_eq!(resolved, file);
        assert_eq!(line, Some(231));
        assert_eq!(column, None);
        let _ = fs::remove_file(resolved);
    }

    #[test]
    fn parses_line_and_column_suffix_when_target_exists() {
        let unique = format!(
            "enzimcoder-link-test-{}-{}.rs",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos()
        );
        let file = std::env::temp_dir().join(unique);
        fs::write(&file, "fn main() {}\n").expect("write temp file");
        let with_line_col = std::path::PathBuf::from(format!("{}:12:5", file.display()));
        let (resolved, line, column) = parse_link_target_with_optional_line_col(&with_line_col)
            .expect("resolve line/col suffix");
        assert_eq!(resolved, file);
        assert_eq!(line, Some(12));
        assert_eq!(column, Some(5));
        let _ = fs::remove_file(resolved);
    }

    #[test]
    fn parses_hash_line_anchor_when_target_exists() {
        let unique = format!(
            "enzimcoder-link-test-{}-{}.rs",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos()
        );
        let file = std::env::temp_dir().join(unique);
        fs::write(&file, "fn main() {}\n").expect("write temp file");
        let with_hash_line = std::path::PathBuf::from(format!("{}#L1066", file.display()));
        let (resolved, line, column) =
            parse_link_target_with_hash_anchor(&with_hash_line).expect("resolve hash line suffix");
        assert_eq!(resolved, file);
        assert_eq!(line, Some(1066));
        assert_eq!(column, None);
        let _ = fs::remove_file(resolved);
    }

    #[test]
    fn parses_hash_line_column_anchor_when_target_exists() {
        let unique = format!(
            "enzimcoder-link-test-{}-{}.rs",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock before epoch")
                .as_nanos()
        );
        let file = std::env::temp_dir().join(unique);
        fs::write(&file, "fn main() {}\n").expect("write temp file");
        let with_hash_line_col = std::path::PathBuf::from(format!("{}#L14C2", file.display()));
        let (resolved, line, column) =
            parse_link_target_with_optional_location(&with_hash_line_col)
                .expect("resolve hash line/col suffix");
        assert_eq!(resolved, file);
        assert_eq!(line, Some(14));
        assert_eq!(column, Some(2));
        let _ = fs::remove_file(resolved);
    }
}
pub(super) fn create_text_segment(body_box: &gtk::Box) -> gtk::Label {
    set_active_action_section_wave(body_box, false);
    let text_section = ensure_text_section(body_box);
    let label = gtk::Label::new(None);
    label.set_use_markup(false);
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    set_chat_label_selectable(&label);
    label.connect_activate_link(|_, uri| {
        if open_link_in_preview(uri) {
            gtk::glib::Propagation::Stop
        } else {
            eprintln!(
                "[chat-link] quick preview returned false, allowing default handler for uri={uri}"
            );
            gtk::glib::Propagation::Proceed
        }
    });
    label.add_css_class("chat-turn-text-segment");
    text_section.append(&label);
    label
}

pub(super) fn create_text_segment_revealed(body_box: &gtk::Box) -> gtk::Label {
    set_active_action_section_wave(body_box, false);
    let text_section = ensure_text_section(body_box);
    let label = gtk::Label::new(None);
    label.set_use_markup(false);
    label.set_xalign(0.0);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    set_chat_label_selectable(&label);
    label.connect_activate_link(|_, uri| {
        if open_link_in_preview(uri) {
            gtk::glib::Propagation::Stop
        } else {
            eprintln!(
                "[chat-link] quick preview returned false, allowing default handler for uri={uri}"
            );
            gtk::glib::Propagation::Proceed
        }
    });
    label.add_css_class("chat-turn-text-segment");
    append_widget_with_reveal(&text_section, &label);
    label
}

pub(super) fn append_widget_with_reveal<T: IsA<gtk::Widget>>(parent: &gtk::Box, child: &T) {
    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    revealer.set_transition_duration(270);
    revealer.set_reveal_child(false);
    revealer.set_child(Some(child));
    parent.append(&revealer);
    enqueue_stream_revealer(&revealer);
}

fn visit_widget_tree(root: &gtk::Widget, visitor: &mut dyn FnMut(&gtk::Widget)) {
    visitor(root);
    let mut child = root.first_child();
    while let Some(node) = child {
        visit_widget_tree(&node, visitor);
        child = node.next_sibling();
    }
}

fn refresh_registered_reasoning_toggle(messages_box: &gtk::Box) {
    let key = messages_box_registry_key(messages_box);
    let has_reasoning = {
        let root: gtk::Widget = messages_box.clone().upcast();
        let mut has_reasoning = false;
        visit_widget_tree(&root, &mut |widget| {
            if widget.has_css_class("chat-inline-reasoning-revealer") {
                has_reasoning = true;
            }
        });
        has_reasoning
    };
    let is_visible = messages_reasoning_visible(messages_box);
    CHAT_REASONING_TOGGLE_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let Some(toggle_weak) = registry.get(&key).cloned() else {
            return;
        };
        let Some(toggle) = toggle_weak.upgrade() else {
            registry.remove(&key);
            return;
        };
        toggle.set_visible(has_reasoning);
        toggle.remove_css_class("is-active");
        if is_visible {
            toggle.add_css_class("is-active");
        }
        toggle.set_tooltip_text(Some(if is_visible {
            "Hide thinking"
        } else {
            "Show thinking"
        }));
    });
}

pub(super) fn register_chat_reasoning_toggle(messages_box: &gtk::Box, toggle: &gtk::Box) {
    let key = messages_box_registry_key(messages_box);
    let weak = gtk::glib::WeakRef::new();
    weak.set(Some(toggle));
    CHAT_REASONING_TOGGLE_REGISTRY.with(|registry| {
        registry.borrow_mut().insert(key, weak);
    });
    {
        let key = key;
        messages_box.connect_destroy(move |_| {
            CHAT_REASONING_TOGGLE_REGISTRY.with(|registry| {
                registry.borrow_mut().remove(&key);
            });
        });
    }
    refresh_registered_reasoning_toggle(messages_box);
}

fn set_reasoning_entries_visible(messages_box: &gtk::Box, visible: bool) {
    let root: gtk::Widget = messages_box.clone().upcast();
    visit_widget_tree(&root, &mut |widget| {
        if let Ok(revealer) = widget.clone().downcast::<gtk::Revealer>() {
            if revealer.has_css_class("chat-inline-reasoning-revealer") {
                revealer.set_reveal_child(visible);
            }
        }
    });
    visit_widget_tree(&root, &mut |widget| {
        let Ok(body_box) = widget.clone().downcast::<gtk::Box>() else {
            return;
        };
        if !body_box.has_css_class("chat-command-list") {
            return;
        }

        let mut has_non_reasoning_content = false;
        let mut has_revealed_reasoning = false;
        let mut child = body_box.first_child();
        while let Some(node) = child {
            if let Ok(revealer) = node.clone().downcast::<gtk::Revealer>() {
                if revealer.has_css_class("chat-inline-reasoning-revealer") {
                    if revealer.reveals_child() || revealer.is_child_revealed() {
                        has_revealed_reasoning = true;
                    }
                    child = node.next_sibling();
                    continue;
                }
            }
            if node.is_visible() {
                has_non_reasoning_content = true;
                break;
            }
            child = node.next_sibling();
        }

        let should_show = has_non_reasoning_content || has_revealed_reasoning;
        body_box.set_visible(should_show);
        if let Some(surface) = find_ancestor_assistant_surface(&body_box.clone().upcast()) {
            if should_show {
                surface.remove_css_class("chat-turn-bubble-initial");
            } else {
                surface.add_css_class("chat-turn-bubble-initial");
            }
        }
    });
}

pub(super) fn toggle_chat_reasoning_visibility(messages_box: &gtk::Box) {
    set_chat_reasoning_visibility(messages_box, !messages_reasoning_visible(messages_box));
}

pub(super) fn set_chat_reasoning_visibility(messages_box: &gtk::Box, visible: bool) {
    if visible {
        messages_box.add_css_class("chat-reasoning-visible");
        set_reasoning_entries_visible(messages_box, true);
    } else {
        messages_box.remove_css_class("chat-reasoning-visible");
        set_reasoning_entries_visible(messages_box, false);
    }
    refresh_registered_reasoning_toggle(messages_box);
}

fn action_summary_registry_key(summary_label: &gtk::Label) -> usize {
    summary_label.as_ptr() as usize
}

fn ensure_action_summary_wave_state(summary_label: &gtk::Label) {
    let key = action_summary_registry_key(summary_label);
    let inserted = ACTION_SUMMARY_WAVE_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        if registry.contains_key(&key) {
            false
        } else {
            registry.insert(key, ActionSummaryWaveState::default());
            true
        }
    });
    if inserted {
        summary_label.connect_destroy(move |_| {
            ACTION_SUMMARY_WAVE_REGISTRY.with(|registry| {
                if let Some(mut state) = registry.borrow_mut().remove(&key) {
                    if let Some(source_id) = state.running_wave_source.take() {
                        source_id.remove();
                    }
                }
            });
        });
    }
}

fn set_action_summary_text(summary_label: &gtk::Label, text: &str) {
    ensure_action_summary_wave_state(summary_label);
    let key = action_summary_registry_key(summary_label);
    let (is_running, phase) = ACTION_SUMMARY_WAVE_REGISTRY.with(|registry| {
        let registry = registry.borrow();
        registry
            .get(&key)
            .map(|state| (state.is_running, state.running_wave_phase))
            .unwrap_or((false, 0.0))
    });
    if is_running {
        summary_label.set_use_markup(true);
        summary_label.set_markup(&wave_markup_for_text(text, phase));
    } else {
        set_plain_label_text(summary_label, text);
    }
}

fn set_action_summary_running(summary_label: &gtk::Label, running: bool) {
    ensure_action_summary_wave_state(summary_label);
    let key = action_summary_registry_key(summary_label);
    let previous_source = ACTION_SUMMARY_WAVE_REGISTRY.with(|registry| {
        let mut registry = registry.borrow_mut();
        let state = registry
            .get_mut(&key)
            .expect("action summary state should exist");
        if state.is_running == running {
            return None;
        }
        state.is_running = running;
        if !running {
            state.running_wave_phase = 0.0;
        }
        state.running_wave_source.take()
    });
    if let Some(source_id) = previous_source {
        source_id.remove();
    }

    if running {
        let phase = gtk::glib::monotonic_time() as f64 / 90_000.0;
        ACTION_SUMMARY_WAVE_REGISTRY.with(|registry| {
            if let Some(state) = registry.borrow_mut().get_mut(&key) {
                state.running_wave_phase = phase;
            }
        });
        let text = summary_label.text().to_string();
        summary_label.set_use_markup(true);
        summary_label.set_markup(&wave_markup_for_text(&text, phase));

        let summary_label = summary_label.clone();
        let source = gtk::glib::timeout_add_local(Duration::from_millis(33), move || {
            let key = action_summary_registry_key(&summary_label);
            let next_phase = gtk::glib::monotonic_time() as f64 / 90_000.0;
            let continue_running = ACTION_SUMMARY_WAVE_REGISTRY.with(|registry| {
                let mut registry = registry.borrow_mut();
                let Some(state) = registry.get_mut(&key) else {
                    return false;
                };
                if !state.is_running {
                    return false;
                }
                state.running_wave_phase = next_phase;
                true
            });
            if !continue_running {
                return gtk::glib::ControlFlow::Break;
            }
            let text = summary_label.text().to_string();
            summary_label.set_use_markup(true);
            summary_label.set_markup(&wave_markup_for_text(&text, next_phase));
            gtk::glib::ControlFlow::Continue
        });

        ACTION_SUMMARY_WAVE_REGISTRY.with(|registry| {
            if let Some(state) = registry.borrow_mut().get_mut(&key) {
                state.running_wave_source.replace(source);
            }
        });
        return;
    }

    let text = summary_label.text().to_string();
    set_plain_label_text(summary_label, &text);
}

fn action_section_parts(section: &gtk::Box) -> Option<ActionSectionUi> {
    let header_widget = section.first_child()?;
    let list_widget = header_widget.next_sibling()?;
    let header_row = header_widget.downcast::<gtk::Box>().ok()?;
    let summary_widget = header_row.first_child()?;
    let summary_label = summary_widget.downcast::<gtk::Label>().ok()?;
    let list = list_widget.downcast::<gtk::Box>().ok()?;
    Some(ActionSectionUi {
        summary_label,
        list,
    })
}

pub(super) fn set_active_action_section_wave(body_box: &gtk::Box, running: bool) {
    let active_key = body_box.last_child().and_then(|last| {
        let section = last.downcast::<gtk::Box>().ok()?;
        if !section.has_css_class("chat-action-section") {
            return None;
        }
        let action_ui = action_section_parts(&section)?;
        Some(action_summary_registry_key(&action_ui.summary_label))
    });

    let mut child = body_box.first_child();
    while let Some(node) = child {
        if let Ok(section) = node.clone().downcast::<gtk::Box>() {
            if section.has_css_class("chat-action-section") {
                if let Some(action_ui) = action_section_parts(&section) {
                    let should_run = running
                        && Some(action_summary_registry_key(&action_ui.summary_label))
                            == active_key;
                    set_action_summary_running(&action_ui.summary_label, should_run);
                }
            }
        }
        child = node.next_sibling();
    }
}

fn action_bucket_class(kind: &str) -> &'static str {
    let normalized = kind.to_ascii_lowercase();
    if normalized.contains("command") || normalized == "shell" {
        "chat-action-entry-command"
    } else if normalized.contains("search") {
        "chat-action-entry-search"
    } else if normalized.contains("filechange")
        || normalized.contains("file edit")
        || normalized.contains("edit")
    {
        "chat-action-entry-file"
    } else {
        "chat-action-entry-tool"
    }
}

fn format_action_count(label: &str, count: usize) -> String {
    if count == 1 {
        format!("1 {label}")
    } else {
        format!("{count} {label}s")
    }
}

fn update_action_section_summary(summary_label: &gtk::Label, list: &gtk::Box) {
    let mut commands = 0usize;
    let mut searches = 0usize;
    let mut file_edits = 0usize;
    let mut tools = 0usize;

    let mut child = list.first_child();
    while let Some(node) = child {
        let entry_widget = if let Ok(revealer) = node.clone().downcast::<gtk::Revealer>() {
            revealer.child().unwrap_or_else(|| node.clone())
        } else {
            node.clone()
        };
        if entry_widget.has_css_class("chat-action-entry-command") {
            commands += 1;
        } else if entry_widget.has_css_class("chat-action-entry-search") {
            searches += 1;
        } else if entry_widget.has_css_class("chat-action-entry-file") {
            file_edits += 1;
        } else if entry_widget.has_css_class("chat-action-entry-tool") {
            tools += 1;
        }
        child = node.next_sibling();
    }

    let mut parts = Vec::new();
    if commands > 0 {
        parts.push(format_action_count("command", commands));
    }
    if searches > 0 {
        parts.push(format_action_count("search", searches));
    }
    if file_edits > 0 {
        parts.push(format_action_count("file edit", file_edits));
    }
    if tools > 0 {
        parts.push(format_action_count("tool", tools));
    }

    let text = if parts.is_empty() {
        "Actions - 0".to_string()
    } else {
        format!("Actions - {}", parts.join(", "))
    };
    set_action_summary_text(summary_label, &text);
}

fn ensure_action_section(body_box: &gtk::Box) -> ActionSectionUi {
    if let Some(last) = body_box.last_child() {
        if let Ok(section) = last.clone().downcast::<gtk::Box>() {
            if section.has_css_class("chat-action-section") {
                if let Some(action_ui) = action_section_parts(&section) {
                    return action_ui;
                }
            }
        }
    }

    let section = gtk::Box::new(gtk::Orientation::Vertical, 2);
    section.add_css_class("chat-action-section");

    let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header_row.add_css_class("chat-action-header");
    header_row.set_hexpand(true);

    let summary_label = gtk::Label::new(Some("Actions - 0"));
    summary_label.set_xalign(0.0);
    summary_label.set_hexpand(true);
    summary_label.add_css_class("chat-action-summary");
    header_row.append(&summary_label);
    section.append(&header_row);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 1);
    list.set_spacing(0);
    list.add_css_class("chat-action-list");
    section.append(&list);

    body_box.append(&section);
    let action_ui = ActionSectionUi {
        summary_label: summary_label.clone(),
        list: list.clone(),
    };
    action_ui
}

fn current_action_section(body_box: &gtk::Box) -> Option<ActionSectionUi> {
    let last = body_box.last_child()?;
    let section = last.downcast::<gtk::Box>().ok()?;
    if !section.has_css_class("chat-action-section") {
        return None;
    }
    action_section_parts(&section)
}

fn ensure_text_section(body_box: &gtk::Box) -> gtk::Box {
    if let Some(last) = body_box.last_child() {
        if let Ok(section) = last.downcast::<gtk::Box>() {
            if section.has_css_class("chat-text-section") {
                return section;
            }
        }
    }

    let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    section.set_spacing(0);
    section.add_css_class("chat-text-section");
    body_box.append(&section);
    section
}

fn append_action_widget_internal<T: IsA<gtk::Widget>>(
    body_box: &gtk::Box,
    kind: &str,
    widget: &T,
    reveal: bool,
) {
    widget.add_css_class("chat-action-entry");
    widget.add_css_class(action_bucket_class(kind));

    let action_ui = ensure_action_section(body_box);
    if reveal {
        append_widget_with_reveal(&action_ui.list, widget);
    } else {
        action_ui.list.append(widget);
    }
    update_action_section_summary(&action_ui.summary_label, &action_ui.list);
    set_active_action_section_wave(body_box, reveal);
}

pub(super) fn append_action_widget_with_reveal<T: IsA<gtk::Widget>>(
    body_box: &gtk::Box,
    kind: &str,
    widget: &T,
) {
    append_action_widget_internal(body_box, kind, widget, true);
}

pub(super) fn append_action_widget<T: IsA<gtk::Widget>>(
    body_box: &gtk::Box,
    kind: &str,
    widget: &T,
) {
    append_action_widget_internal(body_box, kind, widget, false);
}

fn append_reasoning_widget_internal<T: IsA<gtk::Widget>>(
    body_box: &gtk::Box,
    widget: &T,
    reveal: bool,
) {
    widget.add_css_class("chat-inline-reasoning-card");
    let revealer = gtk::Revealer::new();
    revealer.add_css_class("chat-inline-reasoning-revealer");
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    revealer.set_transition_duration(240);
    revealer.set_reveal_child(false);
    revealer.set_child(Some(widget));
    if let Some(action_ui) = current_action_section(body_box) {
        action_ui.list.append(&revealer);
    } else {
        body_box.append(&revealer);
    }

    if let Some(messages_box) = find_ancestor_messages_box(&body_box.clone().upcast()) {
        if messages_reasoning_visible(&messages_box) {
            if reveal {
                enqueue_stream_revealer(&revealer);
            } else {
                revealer.set_reveal_child(true);
            }
        }
        refresh_registered_reasoning_toggle(&messages_box);
    }
}

pub(super) fn append_reasoning_widget_with_reveal<T: IsA<gtk::Widget>>(
    body_box: &gtk::Box,
    widget: &T,
) {
    append_reasoning_widget_internal(body_box, widget, true);
}

pub(super) fn append_reasoning_widget<T: IsA<gtk::Widget>>(body_box: &gtk::Box, widget: &T) {
    append_reasoning_widget_internal(body_box, widget, false);
}

pub(super) fn create_command_widget(command: &str) -> (gtk::Box, CommandUi) {
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
    wrapper.add_css_class("chat-command-card");
    wrapper.add_css_class("chat-activity-card");

    let section_header = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    section_header.add_css_class("chat-activity-row");
    section_header.add_css_class("chat-activity-toggle");
    section_header.set_halign(gtk::Align::Fill);
    section_header.set_hexpand(true);
    section_header.set_can_target(true);
    section_header.set_baseline_position(gtk::BaselinePosition::Center);
    let section_icon = gtk::Image::from_icon_name("terminal-symbolic");
    section_icon.set_pixel_size(12);
    section_icon.set_valign(gtk::Align::Center);
    section_icon.add_css_class("chat-command-section-icon");
    section_header.append(&section_icon);

    let section_title = gtk::Label::new(Some("Shell"));
    section_title.set_xalign(0.0);
    section_title.set_valign(gtk::Align::Baseline);
    section_title.add_css_class("chat-command-section-title");
    section_header.append(&section_title);

    let header_label = gtk::Label::new(None);
    header_label.set_xalign(0.0);
    header_label.set_valign(gtk::Align::Baseline);
    header_label.set_hexpand(true);
    header_label.set_wrap(false);
    header_label.set_single_line_mode(true);
    header_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    header_label.add_css_class("chat-command-header");
    section_header.append(&header_label);

    let status_label = gtk::Label::new(Some("Running"));
    status_label.set_xalign(1.0);
    status_label.set_valign(gtk::Align::Baseline);
    status_label.add_css_class("chat-card-status");

    wrapper.append(&section_header);

    let details_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    details_box.add_css_class("chat-activity-details");

    let command_detail_label = gtk::Label::new(None);
    set_chat_label_selectable(&command_detail_label);
    command_detail_label.set_xalign(0.0);
    command_detail_label.set_wrap(true);
    command_detail_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    command_detail_label.add_css_class("chat-command-detail");
    details_box.append(&command_detail_label);

    let output_toggle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    output_toggle.add_css_class("chat-command-output-toggle");
    output_toggle.set_halign(gtk::Align::End);
    output_toggle.set_can_target(true);
    output_toggle.add_css_class("disabled");
    let output_toggle_enabled = Rc::new(RefCell::new(false));

    let output_toggle_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let output_chevron = gtk::Image::from_icon_name("pan-end-symbolic");
    output_chevron.add_css_class("chat-command-chevron");
    output_chevron.set_pixel_size(10);
    output_toggle_row.append(&output_chevron);
    let output_toggle_label = gtk::Label::new(Some("No output"));
    output_toggle_label.add_css_class("chat-command-output-toggle-label");
    output_toggle_row.append(&output_toggle_label);
    output_toggle.append(&output_toggle_row);
    let output_text: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let output_label = gtk::Label::new(None);
    set_chat_label_selectable(&output_label);
    output_label.set_xalign(0.0);
    output_label.set_wrap(false);
    output_label.add_css_class("chat-command-output");

    let output_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(COMMAND_OUTPUT_MIN_HEIGHT)
        .max_content_height(COMMAND_OUTPUT_MAX_HEIGHT)
        .child(&output_label)
        .build();
    output_scroll.add_css_class("chat-command-output-scroll");
    output_scroll.set_has_frame(false);

    {
        let scroll_ctrl =
            gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let adj = output_scroll.vadjustment();
        scroll_ctrl.connect_scroll(move |_, _, dy| {
            let step = adj.step_increment().max(20.0);
            let new_val = adj.value() + dy * step;
            let max = (adj.upper() - adj.page_size()).max(adj.lower());
            adj.set_value(new_val.clamp(adj.lower(), max));
            gtk::glib::Propagation::Stop
        });
        output_scroll.add_controller(scroll_ctrl);
    }

    let output_revealer = gtk::Revealer::new();
    output_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    output_revealer.set_reveal_child(false);
    output_revealer.set_child(Some(&output_scroll));

    details_box.append(&output_toggle);
    details_box.append(&output_revealer);

    let details_revealer = gtk::Revealer::new();
    details_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    details_revealer.set_transition_duration(190);
    details_revealer.set_reveal_child(false);
    details_revealer.set_child(Some(&details_box));
    wrapper.append(&details_revealer);

    {
        let details_revealer_weak = details_revealer.downgrade();
        let output_revealer_weak = output_revealer.downgrade();
        let output_toggle_enabled = output_toggle_enabled.clone();
        let output_toggle_label_weak = output_toggle_label.downgrade();
        let output_chevron_weak = output_chevron.downgrade();
        let output_label_weak = output_label.downgrade();
        let output_text = output_text.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            let Some(details_revealer) = details_revealer_weak.upgrade() else {
                return;
            };
            let Some(output_revealer) = output_revealer_weak.upgrade() else {
                return;
            };
            let Some(output_toggle_label) = output_toggle_label_weak.upgrade() else {
                return;
            };
            let Some(output_chevron) = output_chevron_weak.upgrade() else {
                return;
            };
            let Some(output_label) = output_label_weak.upgrade() else {
                return;
            };
            let next = !details_revealer.reveals_child();
            if next && *output_toggle_enabled.borrow() {
                let output = output_text.borrow();
                set_plain_label_text(&output_label, output.as_str());
                output_revealer.set_reveal_child(true);
                output_chevron.set_icon_name(Some("pan-down-symbolic"));
                set_plain_label_text(&output_toggle_label, "Hide output");
            } else if !next {
                output_revealer.set_reveal_child(false);
                output_chevron.set_icon_name(Some("pan-end-symbolic"));
                set_plain_label_text(&output_label, "");
            }
            details_revealer.set_reveal_child(next);
        });
        section_header.add_controller(click);
    }

    {
        let revealer_weak = output_revealer.downgrade();
        let output_chevron_weak = output_chevron.downgrade();
        let output_toggle_label_weak = output_toggle_label.downgrade();
        let output_toggle_enabled = output_toggle_enabled.clone();
        let output_label_weak = output_label.downgrade();
        let output_text = output_text.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            let Some(revealer) = revealer_weak.upgrade() else {
                return;
            };
            let Some(output_chevron) = output_chevron_weak.upgrade() else {
                return;
            };
            let Some(output_toggle_label) = output_toggle_label_weak.upgrade() else {
                return;
            };
            let Some(output_label) = output_label_weak.upgrade() else {
                return;
            };
            if !*output_toggle_enabled.borrow() {
                return;
            }
            let next = !revealer.reveals_child();
            if next {
                let output = output_text.borrow();
                set_plain_label_text(&output_label, output.as_str());
            } else {
                set_plain_label_text(&output_label, "");
            }
            revealer.set_reveal_child(next);
            output_chevron.set_icon_name(if next {
                Some("pan-down-symbolic")
            } else {
                Some("pan-end-symbolic")
            });
            set_plain_label_text(
                &output_toggle_label,
                if next { "Hide output" } else { "Show output" },
            );
        });
        output_toggle.add_controller(click);
    }

    let headline_text: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let is_running: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let running_wave_source: Rc<RefCell<Option<gtk::glib::SourceId>>> = Rc::new(RefCell::new(None));
    let running_wave_phase: Rc<RefCell<f64>> = Rc::new(RefCell::new(0.0));

    {
        let running_wave_source = running_wave_source.clone();
        wrapper.connect_destroy(move |_| {
            if let Some(source_id) = running_wave_source.borrow_mut().take() {
                source_id.remove();
            }
        });
    }

    let command_ui = CommandUi {
        header_label,
        command_detail_label,
        status_label,
        headline_text,
        is_running,
        running_wave_source,
        running_wave_phase,
        revealer: output_revealer,
        output_label,
        output_text,
        output_toggle,
        output_toggle_label,
        output_toggle_enabled,
    };
    command_ui.set_command_headline(command);
    command_ui.set_running(true);

    (wrapper, command_ui)
}

pub(super) fn create_tool_call_widget(tool_name: &str, arguments: &str) -> (gtk::Box, ToolCallUi) {
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
    wrapper.add_css_class("chat-command-card");
    wrapper.add_css_class("chat-activity-card");

    let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    header_row.add_css_class("chat-activity-row");
    header_row.add_css_class("chat-activity-toggle");
    header_row.set_halign(gtk::Align::Fill);
    header_row.set_hexpand(true);
    header_row.set_can_target(true);
    header_row.set_baseline_position(gtk::BaselinePosition::Center);

    let icon = gtk::Image::from_icon_name("applications-system-symbolic");
    icon.set_pixel_size(12);
    icon.set_valign(gtk::Align::Center);
    icon.add_css_class("chat-command-section-icon");
    header_row.append(&icon);

    let section_label = gtk::Label::new(Some("Tool"));
    section_label.set_xalign(0.0);
    section_label.set_valign(gtk::Align::Baseline);
    section_label.add_css_class("chat-command-section-title");
    header_row.append(&section_label);

    let tool_label = gtk::Label::new(Some(tool_name));
    tool_label.set_xalign(0.0);
    tool_label.set_valign(gtk::Align::Baseline);
    tool_label.set_hexpand(true);
    tool_label.set_wrap(false);
    tool_label.set_single_line_mode(true);
    tool_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    tool_label.add_css_class("chat-command-header");
    header_row.append(&tool_label);

    let status_label = gtk::Label::new(Some("Running"));
    status_label.set_xalign(1.0);
    status_label.set_valign(gtk::Align::Baseline);
    status_label.add_css_class("chat-card-status");
    header_row.append(&status_label);

    wrapper.append(&header_row);

    let details = gtk::Box::new(gtk::Orientation::Vertical, 3);
    details.add_css_class("chat-activity-details");

    let args_title = gtk::Label::new(Some("Arguments"));
    args_title.set_xalign(0.0);
    args_title.add_css_class("chat-command-output-toggle-label");
    details.append(&args_title);

    let args_label = gtk::Label::new(Some(arguments));
    args_label.set_xalign(0.0);
    args_label.set_wrap(true);
    args_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    set_chat_label_selectable(&args_label);
    args_label.add_css_class("chat-command-output");
    details.append(&args_label);

    let output_label = gtk::Label::new(Some(""));
    output_label.set_xalign(0.0);
    output_label.set_wrap(true);
    output_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    set_chat_label_selectable(&output_label);
    output_label.add_css_class("chat-command-output");
    output_label.set_visible(false);
    details.append(&output_label);
    let output_text: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

    let details_revealer = gtk::Revealer::new();
    details_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    details_revealer.set_transition_duration(180);
    details_revealer.set_reveal_child(false);
    details_revealer.set_child(Some(&details));
    wrapper.append(&details_revealer);

    {
        let details_revealer_weak = details_revealer.downgrade();
        let output_label_weak = output_label.downgrade();
        let output_text = output_text.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            let Some(details_revealer) = details_revealer_weak.upgrade() else {
                return;
            };
            let Some(output_label) = output_label_weak.upgrade() else {
                return;
            };
            let next = !details_revealer.reveals_child();
            if next {
                let output = output_text.borrow();
                set_plain_label_text(&output_label, output.as_str());
                output_label.set_visible(!output.trim().is_empty());
            } else {
                set_plain_label_text(&output_label, "");
                output_label.set_visible(false);
            }
            details_revealer.set_reveal_child(next);
        });
        header_row.add_controller(click);
    }

    (
        wrapper,
        ToolCallUi {
            tool_label,
            args_label,
            status_label,
            output_label,
            details_revealer,
            output_text,
        },
    )
}

pub(super) fn create_generic_item_widget(
    section: &str,
    title: &str,
    summary: &str,
) -> (gtk::Box, GenericItemUi) {
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
    wrapper.add_css_class("chat-command-card");
    wrapper.add_css_class("chat-activity-card");

    let icon_name = if section == "Thinking" {
        "lightbulb-modern-symbolic"
    } else if section == "Web Search" || section == "Web Fetch" {
        "web-browser-symbolic"
    } else if section == "File Read" {
        "document-open-symbolic"
    } else if section == "File Search" || section == "Code Search" {
        "system-search-symbolic"
    } else if section == "Directory List" {
        "folder-symbolic"
    } else if section == "Todo" {
        "view-list-symbolic"
    } else if section == "Question" {
        "dialog-question-symbolic"
    } else {
        "applications-system-symbolic"
    };
    let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    header_row.add_css_class("chat-activity-row");
    header_row.add_css_class("chat-activity-toggle");
    header_row.set_halign(gtk::Align::Fill);
    header_row.set_hexpand(true);
    header_row.set_can_target(true);
    header_row.set_baseline_position(gtk::BaselinePosition::Center);

    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(12);
    icon.set_valign(gtk::Align::Center);
    icon.add_css_class("chat-command-section-icon");
    header_row.append(&icon);

    let section_label = gtk::Label::new(Some(section));
    section_label.set_xalign(0.0);
    section_label.set_valign(gtk::Align::Baseline);
    section_label.add_css_class("chat-command-section-title");
    header_row.append(&section_label);

    let title_label = gtk::Label::new(Some(title));
    title_label.set_xalign(0.0);
    title_label.set_valign(gtk::Align::Baseline);
    title_label.set_hexpand(true);
    title_label.set_wrap(false);
    title_label.set_single_line_mode(true);
    title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title_label.add_css_class("chat-command-header");
    header_row.append(&title_label);

    let status_label = gtk::Label::new(None);
    status_label.set_xalign(1.0);
    status_label.set_valign(gtk::Align::Baseline);
    status_label.add_css_class("chat-card-status");
    status_label.set_visible(false);
    header_row.append(&status_label);

    let chevron = gtk::Image::from_icon_name("pan-end-symbolic");
    chevron.add_css_class("chat-command-chevron");
    chevron.set_pixel_size(11);
    chevron.set_valign(gtk::Align::Center);

    wrapper.append(&header_row);

    let details = gtk::Box::new(gtk::Orientation::Vertical, 3);
    details.add_css_class("chat-activity-details");

    let summary_label = gtk::Label::new(Some(summary));
    summary_label.set_xalign(0.0);
    summary_label.set_wrap(true);
    summary_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    set_chat_label_selectable(&summary_label);
    summary_label.add_css_class("chat-command-output");
    details.append(&summary_label);

    let output_label = gtk::Label::new(Some(""));
    output_label.set_xalign(0.0);
    output_label.set_wrap(true);
    output_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    set_chat_label_selectable(&output_label);
    output_label.add_css_class("chat-command-output");
    let output_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .min_content_height(0)
        .max_content_height(180)
        .child(&output_label)
        .build();
    output_scroll.set_has_frame(false);
    output_scroll.set_visible(false);
    output_scroll.add_css_class("chat-command-output-scroll");
    {
        let scroll_ctrl =
            gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
        let adj = output_scroll.vadjustment();
        scroll_ctrl.connect_scroll(move |_, _, dy| {
            let step = adj.step_increment().max(20.0);
            let new_val = adj.value() + dy * step;
            let max = (adj.upper() - adj.page_size()).max(adj.lower());
            adj.set_value(new_val.clamp(adj.lower(), max));
            gtk::glib::Propagation::Stop
        });
        output_scroll.add_controller(scroll_ctrl);
    }
    details.append(&output_scroll);
    let output_text: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

    let details_revealer = gtk::Revealer::new();
    details_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    details_revealer.set_transition_duration(180);
    details_revealer.set_reveal_child(false);
    details_revealer.set_child(Some(&details));
    wrapper.append(&details_revealer);

    let details_enabled = Rc::new(RefCell::new(false));
    {
        let details_revealer_weak = details_revealer.downgrade();
        let details_enabled = details_enabled.clone();
        let output_label_weak = output_label.downgrade();
        let output_scroll_weak = output_scroll.downgrade();
        let output_text = output_text.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            let Some(details_revealer) = details_revealer_weak.upgrade() else {
                return;
            };
            let Some(output_label) = output_label_weak.upgrade() else {
                return;
            };
            let Some(output_scroll) = output_scroll_weak.upgrade() else {
                return;
            };
            if !*details_enabled.borrow() {
                return;
            }
            let next = !details_revealer.reveals_child();
            if next {
                let output = output_text.borrow();
                set_plain_label_text(&output_label, output.as_str());
                output_label.set_visible(!output.trim().is_empty());
                output_scroll.set_visible(!output.trim().is_empty());
            } else {
                set_plain_label_text(&output_label, "");
                output_label.set_visible(false);
                output_scroll.set_visible(false);
            }
            details_revealer.set_reveal_child(next);
        });
        header_row.add_controller(click);
    }

    let headline_text: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let is_running: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let running_wave_source: Rc<RefCell<Option<gtk::glib::SourceId>>> = Rc::new(RefCell::new(None));
    let running_wave_phase: Rc<RefCell<f64>> = Rc::new(RefCell::new(0.0));

    {
        let running_wave_source = running_wave_source.clone();
        wrapper.connect_destroy(move |_| {
            if let Some(source_id) = running_wave_source.borrow_mut().take() {
                source_id.remove();
            }
        });
    }

    let wave_enabled = section == "Web Search" || section == "Context Compaction";
    let details_supported = section != "Context Compaction";
    let generic_ui = GenericItemUi {
        section_label,
        title_label,
        status_label,
        summary_label,
        output_label,
        output_scroll,
        details_revealer,
        details_enabled,
        headline_text,
        is_running,
        running_wave_source,
        running_wave_phase,
        wave_enabled,
        details_supported,
        output_text,
    };
    generic_ui.set_title(title);
    generic_ui.set_details(summary, "");

    (wrapper, generic_ui)
}

pub(super) fn create_reasoning_widget() -> (gtk::Box, GenericItemUi) {
    let (widget, generic_ui) = create_generic_item_widget("Thinking", "Thinking...", "");
    widget.add_css_class("chat-thinking-card");
    generic_ui.section_label.set_visible(false);
    generic_ui.status_label.set_visible(false);
    generic_ui
        .output_scroll
        .set_widget_name("chat-thinking-output-scroll");
    generic_ui.output_scroll.set_overlay_scrolling(true);
    generic_ui
        .output_scroll
        .add_css_class("chat-thinking-output-scroll");
    generic_ui
        .summary_label
        .add_css_class("chat-thinking-summary");
    generic_ui
        .output_label
        .add_css_class("chat-thinking-output");
    {
        let output_label = generic_ui.output_label.clone();
        let output_scroll = generic_ui.output_scroll.clone();
        let output_text = generic_ui.output_text.clone();
        generic_ui.output_scroll.connect_map(move |_| {
            schedule_thinking_output_scroll_layout_sync(
                output_label.clone(),
                output_scroll.clone(),
                output_text.clone(),
                0,
            );
        });
    }
    {
        let output_label = generic_ui.output_label.clone();
        let output_scroll = generic_ui.output_scroll.clone();
        let output_text = generic_ui.output_text.clone();
        generic_ui
            .output_scroll
            .connect_notify_local(Some("width"), move |_, _| {
                schedule_thinking_output_scroll_layout_sync(
                    output_label.clone(),
                    output_scroll.clone(),
                    output_text.clone(),
                    0,
                );
            });
    }
    {
        let output_label = generic_ui.output_label.clone();
        let output_scroll = generic_ui.output_scroll.clone();
        let output_text = generic_ui.output_text.clone();
        generic_ui
            .output_label
            .connect_notify_local(Some("width"), move |_, _| {
                schedule_thinking_output_scroll_layout_sync(
                    output_label.clone(),
                    output_scroll.clone(),
                    output_text.clone(),
                    0,
                );
            });
    }
    (widget, generic_ui)
}

include!("message_render/file_changes.rs");
pub(super) fn create_error_widget(title: &str, message: &str) -> gtk::Box {
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 4);
    wrapper.add_css_class("chat-error-card");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
    icon.set_pixel_size(12);
    icon.add_css_class("chat-error-icon");
    header.append(&icon);

    let title_label = gtk::Label::new(Some(title));
    title_label.set_xalign(0.0);
    title_label.add_css_class("chat-error-title");
    header.append(&title_label);
    wrapper.append(&header);

    let message_label = gtk::Label::new(Some(message));
    message_label.set_xalign(0.0);
    message_label.set_wrap(true);
    message_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    set_chat_label_selectable(&message_label);
    message_label.add_css_class("chat-error-message");
    wrapper.append(&message_label);

    wrapper
}
