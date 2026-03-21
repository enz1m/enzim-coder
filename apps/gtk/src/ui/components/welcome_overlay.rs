use crate::services::app::CodexProfileManager;
use crate::services::app::chat::{AppDb, CodexProfileRecord};
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

static WELCOME_OVERLAY_VISIBLE: AtomicBool = AtomicBool::new(false);

pub(crate) fn is_visible() -> bool {
    WELCOME_OVERLAY_VISIBLE.load(Ordering::Relaxed)
}

#[derive(Clone, Default, PartialEq, Eq)]
struct WelcomeState {
    codex_installed: bool,
    opencode_installed: bool,
    profile_id: Option<i64>,
    backend_kind: Option<String>,
    account_type: Option<String>,
    email: Option<String>,
}

impl WelcomeState {
    fn any_installed(&self) -> bool {
        self.codex_installed || self.opencode_installed
    }

    fn is_logged_in(&self) -> bool {
        self.email
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
            || self
                .account_type
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty())
    }

    fn account_display(&self) -> Option<String> {
        if let Some(email) = self
            .email
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return Some(email.to_string());
        }
        self.account_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    }
}

enum PollResult {
    State(WelcomeState),
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty())
}

fn copy_to_clipboard(text: &str) {
    if text.trim().is_empty() {
        return;
    }
    if let Some(display) = gtk::gdk::Display::default() {
        display.clipboard().set_text(text.trim());
    }
}

fn set_provider_badge_state(label: &gtk::Label, installed: bool) {
    label.remove_css_class("welcome-status-installed");
    label.remove_css_class("welcome-status-not-installed");
    if installed {
        label.set_text("Installed");
        label.add_css_class("welcome-status-installed");
    } else {
        label.set_text("Not installed");
        label.add_css_class("welcome-status-not-installed");
    }
}

fn profile_has_account_identity(profile: &CodexProfileRecord) -> bool {
    profile
        .last_email
        .as_deref()
        .map(str::trim)
        .is_some_and(|value| !value.is_empty())
        || profile
            .last_account_type
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
}

fn select_welcome_profile<'a>(
    profiles: &'a [CodexProfileRecord],
    active_profile_id: Option<i64>,
    runtime_profile_id: Option<i64>,
) -> Option<&'a CodexProfileRecord> {
    profiles
        .iter()
        .filter(|profile| {
            crate::services::app::runtime::runtime_cli_available_for_backend(&profile.backend_kind)
        })
        .max_by_key(|profile| {
            (
                profile_has_account_identity(profile),
                Some(profile.id) == active_profile_id,
                Some(profile.id) == runtime_profile_id,
            )
        })
}

fn start_brand_reveal_animation(title: &gtk::Label, details_revealer: &gtk::Revealer) {
    let full_text = "Enzim Coder".chars().collect::<Vec<char>>();
    let cursor = Rc::new(RefCell::new(0usize));
    let title = title.clone();
    let details_revealer = details_revealer.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(44), move || {
        let mut index = cursor.borrow_mut();
        if *index > full_text.len() {
            details_revealer.set_reveal_child(true);
            return gtk::glib::ControlFlow::Break;
        }
        let current = full_text.iter().take(*index).collect::<String>();
        title.set_text(&current);
        *index += 1;
        gtk::glib::ControlFlow::Continue
    });
}

fn walk_widgets(root: &gtk::Widget, f: &mut dyn FnMut(&gtk::Widget)) {
    f(root);
    let mut child = root.first_child();
    while let Some(node) = child {
        walk_widgets(&node, f);
        child = node.next_sibling();
    }
}

fn collect_visible_widgets_by_css_classes(
    root: &gtk::Widget,
    classes: &[String],
) -> Vec<gtk::Widget> {
    let mut out = Vec::<gtk::Widget>::new();
    if classes.is_empty() {
        return out;
    }
    walk_widgets(root, &mut |widget: &gtk::Widget| {
        if !widget.is_visible() {
            return;
        }
        if classes.iter().any(|css| widget.has_css_class(css)) {
            out.push(widget.clone());
        }
    });
    out
}

fn collect_spotlight_groups(root: &gtk::Widget, groups: &[Vec<String>]) -> Vec<Vec<gtk::Widget>> {
    let mut out = Vec::<Vec<gtk::Widget>>::new();
    for group in groups {
        let mut widgets = Vec::<gtk::Widget>::new();
        for selector in group {
            let (first_only, class_name) = if let Some(rest) = selector.strip_prefix("first:") {
                (true, rest.to_string())
            } else {
                (false, selector.clone())
            };
            let mut matches = collect_visible_widgets_by_css_classes(root, &[class_name]);
            if first_only && !matches.is_empty() {
                matches.truncate(1);
            }
            widgets.extend(matches);
        }
        if !widgets.is_empty() {
            out.push(widgets);
        }
    }
    out
}

fn clear_onboarding_hover_pulse(root: &gtk::Widget) {
    walk_widgets(root, &mut |widget: &gtk::Widget| {
        widget.remove_css_class("onboarding-hover-pulse");
        widget.remove_css_class("onboarding-hover-pulse-soft");
    });
}

fn apply_onboarding_hover_pulse(root: &gtk::Widget, selectors: &[String], strong: bool) {
    clear_onboarding_hover_pulse(root);
    if selectors.is_empty() {
        return;
    }
    let groups = collect_spotlight_groups(root, &[selectors.to_vec()]);
    let css_class = if strong {
        "onboarding-hover-pulse"
    } else {
        "onboarding-hover-pulse-soft"
    };
    for widget in groups.into_iter().flatten() {
        widget.add_css_class(css_class);
    }
}

pub fn attach(
    root_overlay: &gtk::Overlay,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    runtime_profile_id: i64,
) {
    WELCOME_OVERLAY_VISIBLE.store(true, Ordering::Relaxed);
    crate::ui::sidebar::set_onboarding_guide_step(0);
    let welcome_overlay = gtk::Overlay::new();
    welcome_overlay.add_css_class("welcome-overlay");
    welcome_overlay.set_hexpand(true);
    welcome_overlay.set_vexpand(true);
    welcome_overlay.set_halign(gtk::Align::Fill);
    welcome_overlay.set_valign(gtk::Align::Fill);
    let welcome_backdrop = gtk::Box::new(gtk::Orientation::Vertical, 0);
    welcome_backdrop.set_hexpand(true);
    welcome_backdrop.set_vexpand(true);
    welcome_overlay.set_child(Some(&welcome_backdrop));

    let guide_spotlight = gtk::DrawingArea::new();
    guide_spotlight.set_hexpand(true);
    guide_spotlight.set_vexpand(true);
    guide_spotlight.set_halign(gtk::Align::Fill);
    guide_spotlight.set_valign(gtk::Align::Fill);
    guide_spotlight.set_can_target(false);
    guide_spotlight.set_visible(false);
    welcome_overlay.add_overlay(&guide_spotlight);

    let welcome_center = gtk::Box::new(gtk::Orientation::Vertical, 0);
    welcome_center.add_css_class("welcome-overlay-center");
    welcome_center.set_hexpand(true);
    welcome_center.set_vexpand(true);
    welcome_center.set_halign(gtk::Align::Center);
    welcome_center.set_valign(gtk::Align::Center);

    let welcome_dialog = gtk::Box::new(gtk::Orientation::Vertical, 14);
    welcome_dialog.add_css_class("welcome-dialog");
    welcome_dialog.set_width_request(540);
    welcome_dialog.set_halign(gtk::Align::Center);
    welcome_dialog.set_valign(gtk::Align::Center);
    welcome_dialog.set_margin_start(24);
    welcome_dialog.set_margin_end(24);
    welcome_dialog.set_margin_top(16);
    welcome_dialog.set_margin_bottom(16);

    let brand_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    brand_row.add_css_class("app-brand");
    brand_row.add_css_class("welcome-brand");
    brand_row.set_halign(gtk::Align::Center);

    let brand_title = gtk::Label::new(Some(""));
    brand_title.add_css_class("app-brand-title");
    brand_title.add_css_class("welcome-brand-title");
    brand_title.set_halign(gtk::Align::Center);

    let brand_badge = gtk::Label::new(Some("PREVIEW"));
    brand_badge.add_css_class("app-brand-badge");
    brand_badge.add_css_class("preview-badge");
    brand_badge.add_css_class("welcome-brand-badge");
    brand_badge.set_halign(gtk::Align::Center);

    brand_row.append(&brand_title);
    brand_row.append(&brand_badge);
    welcome_dialog.append(&brand_row);

    let flow_stack = gtk::Stack::new();
    flow_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    flow_stack.set_transition_duration(220);
    flow_stack.set_hhomogeneous(false);
    flow_stack.set_vhomogeneous(false);
    flow_stack.set_hexpand(true);
    flow_stack.set_vexpand(false);

    let details_revealer = gtk::Revealer::new();
    details_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    details_revealer.set_transition_duration(320);
    details_revealer.set_reveal_child(false);

    let details_box = gtk::Box::new(gtk::Orientation::Vertical, 12);

    let provider_section = gtk::Box::new(gtk::Orientation::Vertical, 8);
    provider_section.add_css_class("welcome-section");

    let provider_title = gtk::Label::new(Some("Providers"));
    provider_title.add_css_class("welcome-section-title");
    provider_title.set_xalign(0.0);
    provider_section.append(&provider_title);

    let provider_rows = gtk::Box::new(gtk::Orientation::Vertical, 6);
    provider_rows.add_css_class("welcome-provider-rows");

    let codex_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    codex_row.add_css_class("welcome-provider-row");
    let codex_icon = gtk::Image::from_icon_name("provider-codex");
    codex_icon.add_css_class("welcome-provider-icon");
    codex_icon.add_css_class("welcome-provider-icon-codex");
    codex_icon.set_pixel_size(16);
    let codex_name = gtk::Label::new(Some("Codex"));
    codex_name.add_css_class("welcome-provider-name");
    codex_name.set_xalign(0.0);
    codex_name.set_hexpand(true);
    let codex_badge = gtk::Label::new(Some("Not installed"));
    codex_badge.add_css_class("welcome-pill-badge");
    codex_badge.add_css_class("welcome-status-not-installed");
    codex_badge.set_halign(gtk::Align::End);
    codex_row.append(&codex_icon);
    codex_row.append(&codex_name);
    codex_row.append(&codex_badge);
    provider_rows.append(&codex_row);

    let opencode_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    opencode_row.add_css_class("welcome-provider-row");
    let opencode_icon = gtk::Image::from_icon_name("provider-opencode");
    opencode_icon.add_css_class("welcome-provider-icon");
    opencode_icon.set_pixel_size(16);
    let opencode_name = gtk::Label::new(Some("OpenCode"));
    opencode_name.add_css_class("welcome-provider-name");
    opencode_name.set_xalign(0.0);
    opencode_name.set_hexpand(true);
    let opencode_badge = gtk::Label::new(Some("Not installed"));
    opencode_badge.add_css_class("welcome-pill-badge");
    opencode_badge.add_css_class("welcome-status-not-installed");
    opencode_badge.set_halign(gtk::Align::End);
    opencode_row.append(&opencode_icon);
    opencode_row.append(&opencode_name);
    opencode_row.append(&opencode_badge);
    provider_rows.append(&opencode_row);

    for (name, icon_name, badge_text, badge_class) in [
        (
            "Claude Code",
            "provider-claude",
            "Soon",
            "welcome-status-soon",
        ),
        (
            "Gemini CLI",
            "provider-gemini",
            "Soon",
            "welcome-status-soon",
        ),
    ] {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.add_css_class("welcome-provider-row");
        let provider_icon = gtk::Image::from_icon_name(icon_name);
        provider_icon.add_css_class("welcome-provider-icon");
        provider_icon.set_pixel_size(16);
        let provider_name = gtk::Label::new(Some(name));
        provider_name.add_css_class("welcome-provider-name");
        provider_name.set_xalign(0.0);
        provider_name.set_hexpand(true);
        let soon_badge = gtk::Label::new(Some(badge_text));
        soon_badge.add_css_class("welcome-pill-badge");
        soon_badge.add_css_class(badge_class);
        soon_badge.set_halign(gtk::Align::End);
        row.append(&provider_icon);
        row.append(&provider_name);
        row.append(&soon_badge);
        provider_rows.append(&row);
    }

    provider_section.append(&provider_rows);
    details_box.append(&provider_section);

    let logged_revealer = gtk::Revealer::new();
    logged_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    logged_revealer.set_transition_duration(220);
    logged_revealer.set_reveal_child(false);
    let logged_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    logged_box.add_css_class("welcome-section");
    let logged_label = gtk::Label::new(Some("Runtime profile detected with account: -"));
    logged_label.set_xalign(0.0);
    logged_label.set_wrap(true);
    logged_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    logged_label.add_css_class("welcome-auth-detected");
    logged_box.append(&logged_label);
    logged_revealer.set_child(Some(&logged_box));
    details_box.append(&logged_revealer);

    let login_revealer = gtk::Revealer::new();
    login_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    login_revealer.set_transition_duration(220);
    login_revealer.set_reveal_child(false);

    let login_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    login_box.add_css_class("welcome-section");

    let login_hint = gtk::Label::new(Some(
        "You need to log in to a supported runtime before using Enzim Coder.",
    ));
    login_hint.set_xalign(0.5);
    login_hint.set_halign(gtk::Align::Center);
    login_hint.set_justify(gtk::Justification::Center);
    login_hint.set_wrap(true);
    login_hint.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    login_hint.add_css_class("welcome-muted");

    let login_button = gtk::Button::with_label("Start Login");
    login_button.add_css_class("sidebar-action-button");
    login_button.add_css_class("welcome-login-button");
    login_button.set_halign(gtk::Align::Center);

    let api_key_button = gtk::Button::with_label("Use API Key");
    api_key_button.add_css_class("app-flat-button");
    api_key_button.set_halign(gtk::Align::Center);
    api_key_button.set_visible(false);

    let login_status = gtk::Label::new(None);
    login_status.set_xalign(0.0);
    login_status.set_wrap(true);
    login_status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    login_status.add_css_class("welcome-muted");
    login_status.set_visible(false);

    let login_url_revealer = gtk::Revealer::new();
    login_url_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    login_url_revealer.set_transition_duration(220);
    login_url_revealer.set_reveal_child(false);

    let login_url_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    login_url_row.set_hexpand(true);
    let login_url_entry = gtk::Entry::new();
    login_url_entry.set_hexpand(true);
    login_url_entry.set_editable(false);
    login_url_entry.add_css_class("welcome-login-url");
    login_url_entry.set_placeholder_text(Some("Login URL will appear here"));

    let copy_login_url_button = gtk::Button::new();
    copy_login_url_button.add_css_class("app-flat-button");
    copy_login_url_button.add_css_class("welcome-icon-copy");
    copy_login_url_button.set_sensitive(false);
    copy_login_url_button.set_tooltip_text(Some("Copy login URL"));
    let copy_login_icon = gtk::Image::from_icon_name("edit-copy-symbolic");
    copy_login_icon.set_pixel_size(14);
    copy_login_url_button.set_child(Some(&copy_login_icon));

    login_url_row.append(&login_url_entry);
    login_url_row.append(&copy_login_url_button);
    login_url_revealer.set_child(Some(&login_url_row));

    let login_terminal_hint = gtk::Label::new(Some(
        "You can also use your runtime CLI in the terminal to complete login.",
    ));
    login_terminal_hint.set_xalign(0.5);
    login_terminal_hint.set_halign(gtk::Align::Center);
    login_terminal_hint.set_justify(gtk::Justification::Center);
    login_terminal_hint.set_wrap(true);
    login_terminal_hint.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    login_terminal_hint.add_css_class("welcome-subtle");

    login_box.append(&login_hint);
    login_box.append(&login_button);
    login_box.append(&api_key_button);
    login_box.append(&login_status);
    login_box.append(&login_url_revealer);
    login_box.append(&login_terminal_hint);
    login_revealer.set_child(Some(&login_box));
    details_box.append(&login_revealer);

    let install_revealer = gtk::Revealer::new();
    install_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    install_revealer.set_transition_duration(220);
    install_revealer.set_reveal_child(true);

    let install_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    install_box.add_css_class("welcome-section");

    let install_hint = gtk::Label::new(Some("Install at least one supported runtime CLI first:"));
    install_hint.set_xalign(0.0);
    install_hint.add_css_class("welcome-muted");

    let install_command_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    install_command_row.add_css_class("welcome-code-block");

    let install_command = gtk::Label::new(Some(
        "npm i -g @openai/codex  # or: npm install -g opencode-ai",
    ));
    install_command.add_css_class("welcome-code-text");
    install_command.set_xalign(0.0);
    install_command.set_hexpand(true);
    install_command.set_selectable(true);

    let copy_install_button = gtk::Button::new();
    copy_install_button.add_css_class("app-flat-button");
    copy_install_button.add_css_class("welcome-icon-copy");
    copy_install_button.set_tooltip_text(Some("Copy install command"));
    let copy_install_icon = gtk::Image::from_icon_name("edit-copy-symbolic");
    copy_install_icon.set_pixel_size(14);
    copy_install_button.set_child(Some(&copy_install_icon));

    install_command_row.append(&install_command);
    install_command_row.append(&copy_install_button);

    install_box.append(&install_hint);
    install_box.append(&install_command_row);
    install_revealer.set_child(Some(&install_box));
    details_box.append(&install_revealer);

    let next_revealer = gtk::Revealer::new();
    next_revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);
    next_revealer.set_transition_duration(240);
    next_revealer.set_reveal_child(false);
    let next_button = gtk::Button::with_label("Next");
    next_button.add_css_class("suggested-action");
    next_button.add_css_class("welcome-next-button");
    next_button.set_halign(gtk::Align::Center);
    next_revealer.set_child(Some(&next_button));
    details_box.append(&next_revealer);

    let skip_button = gtk::Button::with_label("Skip");
    skip_button.add_css_class("app-flat-button");
    skip_button.add_css_class("welcome-skip-button");
    skip_button.set_halign(gtk::Align::Center);
    details_box.append(&skip_button);

    details_revealer.set_child(Some(&details_box));

    let guide_page = gtk::Box::new(gtk::Orientation::Vertical, 10);
    guide_page.add_css_class("welcome-guide-page");
    guide_page.set_hexpand(true);
    guide_page.set_vexpand(false);

    let guide_title = gtk::Label::new(Some("Workspaces and Threads"));
    guide_title.add_css_class("welcome-guide-title");
    guide_title.set_xalign(0.0);
    guide_page.append(&guide_title);

    let guide_sections = gtk::Box::new(gtk::Orientation::Vertical, 8);
    guide_sections.add_css_class("welcome-guide-sections");
    guide_page.append(&guide_sections);

    let guide_section_one = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    guide_section_one.add_css_class("welcome-guide-section");
    let guide_section_one_icon_wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    guide_section_one_icon_wrap.add_css_class("welcome-guide-section-icon-wrap");
    guide_section_one_icon_wrap.set_halign(gtk::Align::Center);
    guide_section_one_icon_wrap.set_valign(gtk::Align::Center);
    let guide_section_one_icon = gtk::Image::from_icon_name("folder-silhouette-symbolic");
    guide_section_one_icon.set_pixel_size(14);
    guide_section_one_icon.set_halign(gtk::Align::Center);
    guide_section_one_icon.set_valign(gtk::Align::Center);
    guide_section_one_icon_wrap.append(&guide_section_one_icon);
    let guide_section_one_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let guide_section_one_title = gtk::Label::new(None);
    guide_section_one_title.add_css_class("welcome-guide-section-title");
    guide_section_one_title.set_xalign(0.0);
    let guide_section_one_body = gtk::Label::new(None);
    guide_section_one_body.add_css_class("welcome-guide-section-body");
    guide_section_one_body.set_wrap(true);
    guide_section_one_body.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    guide_section_one_body.set_xalign(0.0);
    guide_section_one_text.append(&guide_section_one_title);
    guide_section_one_text.append(&guide_section_one_body);
    guide_section_one.append(&guide_section_one_icon_wrap);
    guide_section_one.append(&guide_section_one_text);
    guide_sections.append(&guide_section_one);

    let guide_section_two = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    guide_section_two.add_css_class("welcome-guide-section");
    let guide_section_two_icon_wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    guide_section_two_icon_wrap.add_css_class("welcome-guide-section-icon-wrap");
    guide_section_two_icon_wrap.set_halign(gtk::Align::Center);
    guide_section_two_icon_wrap.set_valign(gtk::Align::Center);
    let guide_section_two_icon = gtk::Image::from_icon_name("chat-new-symbolic");
    guide_section_two_icon.set_pixel_size(14);
    guide_section_two_icon.set_halign(gtk::Align::Center);
    guide_section_two_icon.set_valign(gtk::Align::Center);
    guide_section_two_icon_wrap.append(&guide_section_two_icon);
    let guide_section_two_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let guide_section_two_title = gtk::Label::new(None);
    guide_section_two_title.add_css_class("welcome-guide-section-title");
    guide_section_two_title.set_xalign(0.0);
    let guide_section_two_body = gtk::Label::new(None);
    guide_section_two_body.add_css_class("welcome-guide-section-body");
    guide_section_two_body.set_wrap(true);
    guide_section_two_body.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    guide_section_two_body.set_xalign(0.0);
    guide_section_two_text.append(&guide_section_two_title);
    guide_section_two_text.append(&guide_section_two_body);
    guide_section_two.append(&guide_section_two_icon_wrap);
    guide_section_two.append(&guide_section_two_text);
    guide_sections.append(&guide_section_two);

    let guide_section_three = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    guide_section_three.add_css_class("welcome-guide-section");
    let guide_section_three_icon_wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    guide_section_three_icon_wrap.add_css_class("welcome-guide-section-icon-wrap");
    guide_section_three_icon_wrap.set_halign(gtk::Align::Center);
    guide_section_three_icon_wrap.set_valign(gtk::Align::Center);
    let guide_section_three_icon = gtk::Image::from_icon_name("window-close-symbolic");
    guide_section_three_icon.set_pixel_size(14);
    guide_section_three_icon.set_halign(gtk::Align::Center);
    guide_section_three_icon.set_valign(gtk::Align::Center);
    guide_section_three_icon_wrap.append(&guide_section_three_icon);
    let guide_section_three_text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let guide_section_three_title = gtk::Label::new(None);
    guide_section_three_title.add_css_class("welcome-guide-section-title");
    guide_section_three_title.set_xalign(0.0);
    let guide_section_three_body = gtk::Label::new(None);
    guide_section_three_body.add_css_class("welcome-guide-section-body");
    guide_section_three_body.set_wrap(true);
    guide_section_three_body.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    guide_section_three_body.set_xalign(0.0);
    guide_section_three_text.append(&guide_section_three_title);
    guide_section_three_text.append(&guide_section_three_body);
    guide_section_three.append(&guide_section_three_icon_wrap);
    guide_section_three.append(&guide_section_three_text);
    guide_sections.append(&guide_section_three);

    let guide_note = gtk::Label::new(None);
    guide_note.add_css_class("welcome-guide-note");
    guide_note.set_wrap(true);
    guide_note.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    guide_note.set_xalign(0.5);
    guide_note.set_halign(gtk::Align::Center);
    guide_note.set_justify(gtk::Justification::Center);
    guide_note.set_visible(false);
    guide_page.append(&guide_note);

    let guide_spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    guide_spacer.set_vexpand(true);
    guide_page.append(&guide_spacer);

    let guide_next_button = gtk::Button::with_label("Next");
    guide_next_button.add_css_class("suggested-action");
    guide_next_button.add_css_class("welcome-next-button");
    guide_next_button.set_halign(gtk::Align::Center);
    guide_next_button.set_margin_top(4);
    guide_next_button.set_margin_bottom(2);
    guide_page.append(&guide_next_button);

    flow_stack.add_named(&details_revealer, Some("setup"));
    flow_stack.add_named(&guide_page, Some("guide"));
    flow_stack.set_visible_child_name("setup");
    welcome_dialog.append(&flow_stack);

    welcome_center.append(&welcome_dialog);
    welcome_overlay.add_overlay(&welcome_center);
    root_overlay.add_overlay(&welcome_overlay);

    {
        copy_install_button.connect_clicked(move |_| {
            copy_to_clipboard("npm i -g @openai/codex  # or: npm install -g opencode-ai");
        });
    }

    {
        let login_url_entry = login_url_entry.clone();
        copy_login_url_button.connect_clicked(move |_| {
            let text = login_url_entry.text().to_string();
            copy_to_clipboard(&text);
        });
    }

    let current_state = Rc::new(RefCell::new(WelcomeState::default()));
    let welcome_profile_id = Rc::new(RefCell::new(None::<i64>));
    let apply_state: Rc<dyn Fn(WelcomeState)> = {
        let current_state = current_state.clone();
        let welcome_profile_id = welcome_profile_id.clone();
        let codex_badge = codex_badge.clone();
        let opencode_badge = opencode_badge.clone();
        let install_revealer = install_revealer.clone();
        let login_revealer = login_revealer.clone();
        let logged_revealer = logged_revealer.clone();
        let logged_label = logged_label.clone();
        let next_revealer = next_revealer.clone();
        let login_url_revealer = login_url_revealer.clone();
        let login_status = login_status.clone();
        let login_button = login_button.clone();
        let api_key_button = api_key_button.clone();
        let login_hint = login_hint.clone();
        Rc::new(move |next: WelcomeState| {
            current_state.replace(next.clone());
            welcome_profile_id.replace(next.profile_id);
            set_provider_badge_state(&codex_badge, next.codex_installed);
            set_provider_badge_state(&opencode_badge, next.opencode_installed);

            let logged_in = next.is_logged_in();
            let installed = next.any_installed();
            let backend_display = next
                .backend_kind
                .as_deref()
                .map(crate::services::app::runtime::backend_display_name)
                .unwrap_or("runtime");
            let has_profile_target = next.profile_id.is_some();

            install_revealer.set_reveal_child(!installed);
            login_revealer.set_reveal_child(installed && !logged_in);
            logged_revealer.set_reveal_child(installed && logged_in);
            next_revealer.set_reveal_child(installed && logged_in);

            if logged_in {
                login_url_revealer.set_reveal_child(false);
                login_status.set_visible(false);
                login_status.set_text("");
                login_button.set_sensitive(true);
                login_button.set_visible(true);
                login_button.set_label(&format!("Start {backend_display} Login"));
                api_key_button.set_visible(false);
                let value = next
                    .account_display()
                    .unwrap_or_else(|| "unknown".to_string());
                logged_label.set_text(&format!(
                    "{backend_display} runtime ready with account: {value}"
                ));
                login_hint.set_text("A supported runtime is authenticated and ready.");
            } else {
                login_url_revealer.set_reveal_child(false);
                if installed && has_profile_target {
                    login_hint.set_text(&format!(
                        "Authenticate {backend_display} before using Enzim Coder."
                    ));
                    login_button.set_label(&format!("Start {backend_display} Login"));
                    login_button.set_visible(true);
                    login_button.set_sensitive(true);
                } else if installed {
                    login_hint.set_text(
                        "A supported runtime is installed, but no matching profile is ready yet. Open Settings to add or select one.",
                    );
                    login_button.set_visible(false);
                } else {
                    login_hint.set_text(
                        "You need to install a supported runtime before using Enzim Coder.",
                    );
                    login_button.set_label("Start Login");
                    login_button.set_visible(true);
                    login_button.set_sensitive(false);
                }
                let show_api_key = installed
                    && has_profile_target
                    && next
                        .backend_kind
                        .as_deref()
                        .is_some_and(|value| value.eq_ignore_ascii_case("opencode"));
                api_key_button.set_visible(show_api_key);
            }
        })
    };

    (apply_state)(WelcomeState::default());

    {
        let db = db.clone();
        let manager = manager.clone();
        let welcome_profile_id = welcome_profile_id.clone();
        let login_button_for_signal = login_button.clone();
        let login_button = login_button.clone();
        let login_status = login_status.clone();
        let login_url_revealer = login_url_revealer.clone();
        let login_url_entry = login_url_entry.clone();
        let copy_login_url_button = copy_login_url_button.clone();
        login_button_for_signal.connect_clicked(move |_| {
            let Some(profile_id) = (*welcome_profile_id.borrow())
                .or_else(|| db.active_profile_id().ok().flatten())
                .or_else(|| db.runtime_profile_id().ok().flatten())
                .or(Some(runtime_profile_id))
            else {
                login_status.set_visible(true);
                login_status.set_text(
                    "No supported profile is ready yet. Open Settings and add or select a runtime profile first.",
                );
                return;
            };
            let _ = db.set_active_profile_id(profile_id);
            let _ = db.set_runtime_profile_id(profile_id);
            let client = match manager.ensure_started(profile_id) {
                Ok(client) => client,
                Err(err) => {
                    login_status.set_visible(true);
                    login_status.set_text(&format!("Unable to start the selected runtime: {err}"));
                    login_button.set_sensitive(true);
                    return;
                }
            };
            login_button.set_sensitive(false);
            login_status.set_visible(true);
            login_status.set_text("Generating login URL...");
            login_url_revealer.set_reveal_child(false);
            login_url_entry.set_text("");
            copy_login_url_button.set_sensitive(false);

            let (tx, rx) = mpsc::channel::<Result<(String, String), String>>();
            thread::spawn(move || {
                let _ = tx.send(client.account_login_start_chatgpt());
            });

            let login_button_done = login_button.clone();
            let login_status_done = login_status.clone();
            let login_url_revealer_done = login_url_revealer.clone();
            let login_url_entry_done = login_url_entry.clone();
            let copy_login_url_button_done = copy_login_url_button.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(60), move || match rx.try_recv() {
                Ok(Ok((_login_id, url))) => {
                    login_url_entry_done.set_text(&url);
                    copy_login_url_button_done.set_sensitive(true);
                    login_url_revealer_done.set_reveal_child(true);
                    login_status_done.set_text(
                        "Open the URL, complete login, then return here. This screen will update automatically.",
                    );
                    login_button_done.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
                Ok(Err(err)) => {
                    login_status_done.set_text(&format!("Login URL request failed: {err}"));
                    login_button_done.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    login_status_done.set_text("Login URL request stopped unexpectedly.");
                    login_button_done.set_sensitive(true);
                    gtk::glib::ControlFlow::Break
                }
            });
        });
    }

    {
        let db = db.clone();
        let manager = manager.clone();
        let apply_state = apply_state.clone();
        let welcome_profile_id = welcome_profile_id.clone();
        let api_key_button_for_signal = api_key_button.clone();
        let api_key_button = api_key_button.clone();
        let login_status = login_status.clone();
        api_key_button_for_signal.connect_clicked(move |_| {
            let Some(profile_id) = (*welcome_profile_id.borrow())
                .or_else(|| db.active_profile_id().ok().flatten())
                .or_else(|| db.runtime_profile_id().ok().flatten())
                .or(Some(runtime_profile_id))
            else {
                login_status.set_visible(true);
                login_status.set_text(
                    "No OpenCode profile is ready yet. Open Settings and select an OpenCode profile first.",
                );
                return;
            };
            let _ = db.set_active_profile_id(profile_id);
            let _ = db.set_runtime_profile_id(profile_id);
            let db_for_success = db.clone();
            let apply_state_for_success = apply_state.clone();
            let login_status_for_success = login_status.clone();
            crate::ui::components::runtime_auth_dialog::start_opencode_api_key_flow(
                None,
                db.clone(),
                manager.clone(),
                profile_id,
                api_key_button.clone(),
                login_status.clone(),
                "welcome-muted",
                Rc::new(move |account, provider_name| {
                    let account_type = account.as_ref().map(|a| a.account_type.clone());
                    let email = account.as_ref().and_then(|a| a.email.clone());
                    let _ = db_for_success.update_codex_profile_status(profile_id, "running");
                    (apply_state_for_success)(WelcomeState {
                        codex_installed: crate::services::app::runtime::runtime_cli_available_for_backend("codex"),
                        opencode_installed: crate::services::app::runtime::runtime_cli_available_for_backend(
                            "opencode",
                        ),
                        profile_id: Some(profile_id),
                        backend_kind: Some("opencode".to_string()),
                        account_type: normalize_optional(account_type),
                        email: normalize_optional(email),
                    });
                    login_status_for_success.set_visible(true);
                    login_status_for_success
                        .set_text(&format!("Saved API key for {provider_name}."));
                }),
            );
        });
    }

    let welcome_closed = Rc::new(RefCell::new(false));
    let root_widget: gtk::Widget = root_overlay.clone().upcast();
    let spotlight_groups = Rc::new(RefCell::new(Vec::<Vec<String>>::new()));
    let spotlight_enabled = Rc::new(RefCell::new(false));
    let section_hover_targets = Rc::new(RefCell::new(vec![Vec::new(), Vec::new(), Vec::new()]));
    let hovered_section_idx = Rc::new(RefCell::new(None::<usize>));
    let hover_pulse_phase = Rc::new(RefCell::new(false));
    {
        let root_widget = root_widget.clone();
        let spotlight_groups = spotlight_groups.clone();
        let spotlight_enabled = spotlight_enabled.clone();
        guide_spotlight.set_draw_func(move |_, cr, width, height| {
            if !*spotlight_enabled.borrow() {
                return;
            }

            let groups = spotlight_groups.borrow().clone();
            let grouped_widgets = collect_spotlight_groups(&root_widget, &groups);
            if grouped_widgets.is_empty() {
                return;
            }

            cr.set_source_rgba(0.08, 0.1, 0.14, 0.3);
            cr.rectangle(0.0, 0.0, width as f64, height as f64);
            let _ = cr.fill();

            cr.set_operator(gtk::cairo::Operator::Clear);
            for group in &grouped_widgets {
                let mut min_x = f32::MAX;
                let mut min_y = f32::MAX;
                let mut max_x = f32::MIN;
                let mut max_y = f32::MIN;
                let mut has_any = false;
                for widget in group {
                    let Some(bounds) = widget.compute_bounds(&root_widget) else {
                        continue;
                    };
                    has_any = true;
                    min_x = min_x.min(bounds.x());
                    min_y = min_y.min(bounds.y());
                    max_x = max_x.max(bounds.x() + bounds.width());
                    max_y = max_y.max(bounds.y() + bounds.height());
                }
                if !has_any {
                    continue;
                }
                let cx = min_x + ((max_x - min_x) / 2.0);
                let cy = min_y + ((max_y - min_y) / 2.0);
                let radius = (((max_x - min_x).max(max_y - min_y)) * 0.68 + 30.0).max(32.0);
                cr.arc(
                    cx as f64,
                    cy as f64,
                    radius as f64,
                    0.0,
                    std::f64::consts::TAU,
                );
                let _ = cr.fill();
            }

            cr.set_operator(gtk::cairo::Operator::Over);
            cr.set_line_width(2.0);
            cr.set_source_rgba(0.83, 0.88, 0.96, 0.55);
            for group in &grouped_widgets {
                let mut min_x = f32::MAX;
                let mut min_y = f32::MAX;
                let mut max_x = f32::MIN;
                let mut max_y = f32::MIN;
                let mut has_any = false;
                for widget in group {
                    let Some(bounds) = widget.compute_bounds(&root_widget) else {
                        continue;
                    };
                    has_any = true;
                    min_x = min_x.min(bounds.x());
                    min_y = min_y.min(bounds.y());
                    max_x = max_x.max(bounds.x() + bounds.width());
                    max_y = max_y.max(bounds.y() + bounds.height());
                }
                if !has_any {
                    continue;
                }
                let cx = min_x + ((max_x - min_x) / 2.0);
                let cy = min_y + ((max_y - min_y) / 2.0);
                let radius = (((max_x - min_x).max(max_y - min_y)) * 0.68 + 30.0).max(32.0);
                cr.arc(
                    cx as f64,
                    cy as f64,
                    radius as f64,
                    0.0,
                    std::f64::consts::TAU,
                );
                let _ = cr.stroke();
            }
        });
    }
    {
        let guide_spotlight = guide_spotlight.clone();
        let welcome_closed = welcome_closed.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(33), move || {
            if *welcome_closed.borrow() || guide_spotlight.root().is_none() {
                return gtk::glib::ControlFlow::Break;
            }
            if guide_spotlight.is_visible() {
                guide_spotlight.queue_draw();
            }
            gtk::glib::ControlFlow::Continue
        });
    }
    {
        let motion = gtk::EventControllerMotion::new();
        let enter_hovered_section_idx = hovered_section_idx.clone();
        let enter_root_widget = root_widget.clone();
        motion.connect_enter(move |_, _, _| {
            enter_hovered_section_idx.replace(Some(0));
            clear_onboarding_hover_pulse(&enter_root_widget);
        });
        let leave_hovered_section_idx = hovered_section_idx.clone();
        let leave_root_widget = root_widget.clone();
        motion.connect_leave(move |_| {
            if *leave_hovered_section_idx.borrow() == Some(0) {
                leave_hovered_section_idx.replace(None);
            }
            clear_onboarding_hover_pulse(&leave_root_widget);
        });
        guide_section_one.add_controller(motion);
    }
    {
        let motion = gtk::EventControllerMotion::new();
        let enter_hovered_section_idx = hovered_section_idx.clone();
        let enter_root_widget = root_widget.clone();
        motion.connect_enter(move |_, _, _| {
            enter_hovered_section_idx.replace(Some(1));
            clear_onboarding_hover_pulse(&enter_root_widget);
        });
        let leave_hovered_section_idx = hovered_section_idx.clone();
        let leave_root_widget = root_widget.clone();
        motion.connect_leave(move |_| {
            if *leave_hovered_section_idx.borrow() == Some(1) {
                leave_hovered_section_idx.replace(None);
            }
            clear_onboarding_hover_pulse(&leave_root_widget);
        });
        guide_section_two.add_controller(motion);
    }
    {
        let motion = gtk::EventControllerMotion::new();
        let enter_hovered_section_idx = hovered_section_idx.clone();
        let enter_root_widget = root_widget.clone();
        motion.connect_enter(move |_, _, _| {
            enter_hovered_section_idx.replace(Some(2));
            clear_onboarding_hover_pulse(&enter_root_widget);
        });
        let leave_hovered_section_idx = hovered_section_idx.clone();
        let leave_root_widget = root_widget.clone();
        motion.connect_leave(move |_| {
            if *leave_hovered_section_idx.borrow() == Some(2) {
                leave_hovered_section_idx.replace(None);
            }
            clear_onboarding_hover_pulse(&leave_root_widget);
        });
        guide_section_three.add_controller(motion);
    }
    {
        let welcome_closed = welcome_closed.clone();
        let root_widget = root_widget.clone();
        let hovered_section_idx = hovered_section_idx.clone();
        let section_hover_targets = section_hover_targets.clone();
        let hover_pulse_phase = hover_pulse_phase.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(240), move || {
            if *welcome_closed.borrow() || root_widget.root().is_none() {
                clear_onboarding_hover_pulse(&root_widget);
                return gtk::glib::ControlFlow::Break;
            }
            let Some(idx) = *hovered_section_idx.borrow() else {
                clear_onboarding_hover_pulse(&root_widget);
                return gtk::glib::ControlFlow::Continue;
            };
            let selectors = section_hover_targets
                .borrow()
                .get(idx)
                .cloned()
                .unwrap_or_default();
            if selectors.is_empty() {
                clear_onboarding_hover_pulse(&root_widget);
                return gtk::glib::ControlFlow::Continue;
            }
            let strong = {
                let mut phase = hover_pulse_phase.borrow_mut();
                *phase = !*phase;
                *phase
            };
            apply_onboarding_hover_pulse(&root_widget, &selectors, strong);
            gtk::glib::ControlFlow::Continue
        });
    }
    let guide_step = Rc::new(RefCell::new(1u8));
    let apply_guide_step: Rc<dyn Fn(u8)> = {
        let guide_title = guide_title.clone();
        let guide_section_one = guide_section_one.clone();
        let guide_section_one_icon = guide_section_one_icon.clone();
        let guide_section_one_title = guide_section_one_title.clone();
        let guide_section_one_body = guide_section_one_body.clone();
        let guide_section_two = guide_section_two.clone();
        let guide_section_two_icon = guide_section_two_icon.clone();
        let guide_section_two_title = guide_section_two_title.clone();
        let guide_section_two_body = guide_section_two_body.clone();
        let guide_section_three = guide_section_three.clone();
        let guide_section_three_icon = guide_section_three_icon.clone();
        let guide_section_three_title = guide_section_three_title.clone();
        let guide_section_three_body = guide_section_three_body.clone();
        let guide_note = guide_note.clone();
        let guide_next_button = guide_next_button.clone();
        let spotlight_groups = spotlight_groups.clone();
        let spotlight_enabled = spotlight_enabled.clone();
        let guide_spotlight = guide_spotlight.clone();
        let section_hover_targets = section_hover_targets.clone();
        let hovered_section_idx = hovered_section_idx.clone();
        let root_widget = root_widget.clone();
        Rc::new(move |step: u8| {
            crate::ui::sidebar::set_onboarding_guide_step(step);
            spotlight_enabled.replace(true);
            guide_spotlight.set_visible(true);
            hovered_section_idx.replace(None);
            clear_onboarding_hover_pulse(&root_widget);
            match step {
                1 => {
                    guide_title.set_text("Workspaces and Threads");
                    guide_section_one.set_visible(true);
                    guide_section_one_icon.set_icon_name(Some("folder-silhouette-symbolic"));
                    guide_section_one_title.set_text("Workspace");
                    guide_section_one_body.set_text(
                        "A workspace is a folder on your computer. Pick one to scope chats and tools.",
                    );
                    guide_section_two.set_visible(true);
                    guide_section_two_icon.set_icon_name(Some("chat-new-symbolic"));
                    guide_section_two_title.set_text("Thread");
                    guide_section_two_body.set_text(
                        "Threads are separate chats with empty context. Create new ones for different tasks.",
                    );
                    guide_section_three.set_visible(false);
                    guide_note.set_visible(true);
                    guide_note.set_text(
                        "Rename or close a thread with right click. Close a workspace with right click.",
                    );
                    spotlight_groups.replace(vec![
                        vec!["sidebar-add-workspace-button".to_string()],
                        vec!["first:onboarding-mock-add-thread".to_string()],
                    ]);
                    section_hover_targets.replace(vec![
                        vec!["sidebar-add-workspace-button".to_string()],
                        vec!["first:onboarding-mock-add-thread".to_string()],
                        Vec::new(),
                    ]);
                    guide_next_button.set_label("Next");
                }
                2 => {
                    guide_title.set_text("Chat, Git and Files");
                    guide_section_one.set_visible(true);
                    guide_section_one_icon.set_icon_name(Some("chat-new-symbolic"));
                    guide_section_one_title.set_text("Chat");
                    guide_section_one_body.set_text(
                        "Chats are separated by thread. Switch threads to keep context organized.",
                    );
                    guide_section_two.set_visible(true);
                    guide_section_two_icon.set_icon_name(Some("git-symbolic"));
                    guide_section_two_title.set_text("Git");
                    guide_section_two_body.set_text(
                        "Review changes, commit, push, and fetch. You can check repo status quickly here.",
                    );
                    guide_section_three.set_visible(true);
                    guide_section_three_icon.set_icon_name(Some("folder-silhouette-symbolic"));
                    guide_section_three_title.set_text("Files");
                    guide_section_three_body.set_text(
                        "Browse files in the active workspace. Open anything without leaving the app.",
                    );
                    guide_note.set_visible(false);
                    spotlight_groups.replace(vec![vec!["top-tab".to_string()]]);
                    section_hover_targets.replace(vec![
                        vec!["top-tab-chat".to_string()],
                        vec!["top-tab-git".to_string()],
                        vec!["top-tab-files".to_string()],
                    ]);
                    guide_next_button.set_label("Next");
                }
                3 => {
                    guide_title.set_text("Skills, MCP, Run and Multichat");
                    guide_section_one.set_visible(true);
                    guide_section_one_icon.set_icon_name(Some("3d-box-symbolic"));
                    guide_section_one_title.set_text("Skills & MCP");
                    guide_section_one_body.set_text(
                        "Extend tools and profile capabilities. Enable only what each profile needs.",
                    );
                    guide_section_two.set_visible(true);
                    guide_section_two_icon.set_icon_name(Some("media-playback-start-symbolic"));
                    guide_section_two_title.set_text("Run");
                    guide_section_two_body.set_text(
                        "Run saved workspace commands from the top bar. This is useful for repeat tasks.",
                    );
                    guide_section_three.set_visible(true);
                    guide_section_three_icon.set_icon_name(Some("view-grid-symbolic"));
                    guide_section_three_title.set_text("Multichat View");
                    guide_section_three_body
                        .set_text("Open multichat view to work with multiple chats side by side.");
                    guide_note.set_visible(false);
                    spotlight_groups.replace(vec![vec![
                        "topbar-skills-mcp-button".to_string(),
                        "topbar-actions-button".to_string(),
                        "multiview-toggle-button".to_string(),
                    ]]);
                    section_hover_targets.replace(vec![
                        vec!["topbar-skills-mcp-button".to_string()],
                        vec!["topbar-actions-button".to_string()],
                        vec!["multiview-toggle-button".to_string()],
                    ]);
                    guide_next_button.set_label("Next");
                }
                _ => {
                    guide_title.set_text("Bottom Bar");
                    guide_section_one.set_visible(true);
                    guide_section_one_icon.set_icon_name(Some("waves-and-screen-symbolic"));
                    guide_section_one_title.set_text("Remote");
                    guide_section_one_body.set_text(
                        "Toggle Telegram forwarding mode. Use it when you want remote interaction.",
                    );
                    guide_section_two.set_visible(true);
                    guide_section_two_icon.set_icon_name(Some("preferences-system-symbolic"));
                    guide_section_two_title.set_text("Settings");
                    guide_section_two_body.set_text(
                        "Manage profiles and application options. Update providers and runtime behavior here.",
                    );
                    guide_section_three.set_visible(true);
                    guide_section_three_icon.set_icon_name(Some("color-symbolic"));
                    guide_section_three_title.set_text("Styles");
                    guide_section_three_body.set_text(
                        "Change theme and visual styling. Pick the look you prefer for daily use.",
                    );
                    guide_note.set_visible(false);
                    spotlight_groups.replace(vec![vec![
                        "bottom-remote-button".to_string(),
                        "bottom-settings-button".to_string(),
                        "bottom-style-button".to_string(),
                    ]]);
                    section_hover_targets.replace(vec![
                        vec!["bottom-remote-button".to_string()],
                        vec!["bottom-settings-button".to_string()],
                        vec!["bottom-style-button".to_string()],
                    ]);
                    guide_next_button.set_label("Finish");
                }
            }
            guide_spotlight.queue_draw();
        })
    };

    let finish_onboarding: Rc<dyn Fn()> = {
        let welcome_closed = welcome_closed.clone();
        let welcome_overlay = welcome_overlay.clone();
        let root_overlay = root_overlay.clone();
        let spotlight_groups = spotlight_groups.clone();
        let spotlight_enabled = spotlight_enabled.clone();
        let guide_spotlight = guide_spotlight.clone();
        let hovered_section_idx = hovered_section_idx.clone();
        let root_widget = root_widget.clone();
        Rc::new(move || {
            crate::ui::sidebar::set_onboarding_guide_step(0);
            spotlight_groups.replace(Vec::new());
            spotlight_enabled.replace(false);
            guide_spotlight.set_visible(false);
            hovered_section_idx.replace(None);
            clear_onboarding_hover_pulse(&root_widget);
            welcome_closed.replace(true);
            welcome_overlay.set_can_target(false);
            welcome_overlay.add_css_class("is-dismissing");
            let root_overlay = root_overlay.clone();
            let welcome_overlay = welcome_overlay.clone();
            gtk::glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                root_overlay.remove_overlay(&welcome_overlay);
                WELCOME_OVERLAY_VISIBLE.store(false, Ordering::Relaxed);
            });
        })
    };

    let start_guide: Rc<dyn Fn()> = {
        let flow_stack = flow_stack.clone();
        let apply_guide_step = apply_guide_step.clone();
        let guide_step = guide_step.clone();
        let welcome_overlay = welcome_overlay.clone();
        Rc::new(move || {
            guide_step.replace(1);
            welcome_overlay.add_css_class("guide-mode");
            flow_stack.set_visible_child_name("guide");
            (apply_guide_step)(1);
        })
    };

    {
        let start_guide = start_guide.clone();
        next_button.connect_clicked(move |_| {
            (start_guide)();
        });
    }

    {
        let start_guide = start_guide.clone();
        skip_button.connect_clicked(move |_| {
            (start_guide)();
        });
    }

    {
        let guide_step = guide_step.clone();
        let apply_guide_step = apply_guide_step.clone();
        let finish_onboarding = finish_onboarding.clone();
        guide_next_button.connect_clicked(move |_| {
            let current = *guide_step.borrow();
            if current >= 4 {
                (finish_onboarding)();
                return;
            }
            let next = current.saturating_add(1);
            guide_step.replace(next);
            (apply_guide_step)(next);
        });
    }

    let (poll_tx, poll_rx) = mpsc::channel::<PollResult>();
    let poll_in_flight = Rc::new(RefCell::new(false));

    {
        let db = db.clone();
        let manager = manager.clone();
        let poll_tx = poll_tx.clone();
        let poll_in_flight = poll_in_flight.clone();
        let welcome_closed = welcome_closed.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(1200), move || {
            if *welcome_closed.borrow() {
                return gtk::glib::ControlFlow::Break;
            }
            if *poll_in_flight.borrow() {
                return gtk::glib::ControlFlow::Continue;
            }
            poll_in_flight.replace(true);

            let codex_installed =
                crate::services::app::runtime::runtime_cli_available_for_backend("codex");
            let opencode_installed =
                crate::services::app::runtime::runtime_cli_available_for_backend("opencode");
            let active_profile_id = db.active_profile_id().ok().flatten();
            let current_runtime_profile_id = db
                .runtime_profile_id()
                .ok()
                .flatten()
                .or(Some(runtime_profile_id));
            let profiles = db.list_codex_profiles().unwrap_or_default();
            let Some(profile) =
                select_welcome_profile(&profiles, active_profile_id, current_runtime_profile_id)
            else {
                let _ = poll_tx.send(PollResult::State(WelcomeState {
                    codex_installed,
                    opencode_installed,
                    ..WelcomeState::default()
                }));
                return gtk::glib::ControlFlow::Continue;
            };

            let base_state = WelcomeState {
                codex_installed,
                opencode_installed,
                profile_id: Some(profile.id),
                backend_kind: Some(profile.backend_kind.clone()),
                account_type: normalize_optional(profile.last_account_type.clone()),
                email: normalize_optional(profile.last_email.clone()),
            };

            match manager.ensure_started(profile.id) {
                Ok(client) => {
                    let poll_tx = poll_tx.clone();
                    let mut next_state = base_state.clone();
                    thread::spawn(move || {
                        let account = client.account_read(true).ok().flatten();
                        next_state.backend_kind = Some(client.backend_kind().to_string());
                        next_state.account_type = normalize_optional(
                            account.as_ref().map(|info| info.account_type.clone()),
                        );
                        next_state.email = normalize_optional(account.and_then(|info| info.email));
                        let _ = poll_tx.send(PollResult::State(next_state));
                    });
                }
                Err(_) => {
                    let _ = poll_tx.send(PollResult::State(base_state));
                }
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    {
        let db = db.clone();
        let apply_state = apply_state.clone();
        let poll_in_flight = poll_in_flight.clone();
        let welcome_closed = welcome_closed.clone();
        gtk::glib::timeout_add_local(Duration::from_millis(80), move || {
            if *welcome_closed.borrow() {
                return gtk::glib::ControlFlow::Break;
            }
            while let Ok(result) = poll_rx.try_recv() {
                poll_in_flight.replace(false);
                match result {
                    PollResult::State(state) => {
                        if let Some(profile_id) = state.profile_id {
                            let profile_running = state.any_installed()
                                && state
                                    .backend_kind
                                    .as_deref()
                                    .is_some_and(
                                        crate::services::app::runtime::runtime_cli_available_for_backend,
                                    );
                            let _ = db.update_codex_profile_status(
                                profile_id,
                                if profile_running {
                                    "running"
                                } else {
                                    "stopped"
                                },
                            );
                            let _ = crate::ui::components::runtime_auth_dialog::sync_runtime_account_fields_to_db(
                                &db,
                                profile_id,
                                state.account_type.as_deref(),
                                state.email.as_deref(),
                            );
                        }
                        (apply_state)(state);
                    }
                }
            }
            gtk::glib::ControlFlow::Continue
        });
    }

    start_brand_reveal_animation(&brand_title, &details_revealer);
}
