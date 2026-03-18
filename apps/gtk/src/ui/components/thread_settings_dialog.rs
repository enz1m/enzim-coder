use gtk::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};

fn escape_markup(text: &str) -> String {
    gtk::glib::markup_escape_text(text).to_string()
}

fn supports_link_markup() -> bool {
    static SUPPORTS_LINKS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTS_LINKS.get_or_init(|| {
        gtk::pango::parse_markup(r#"<a href="https://example.com">x</a>"#, '\0').is_ok()
    })
}

fn looks_like_unfenced_diff(text: &str) -> bool {
    text.contains("\n@@ ")
        || text.starts_with("@@ ")
        || text.contains("\ndiff --git ")
        || text.starts_with("diff --git ")
}

fn render_inline_markdown(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '[' {
            let mut j = i + 1;
            let mut label_end = None;
            while j < chars.len() {
                if chars[j] == ']' {
                    label_end = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(label_end) = label_end {
                let open_paren = label_end + 1;
                if open_paren < chars.len() && chars[open_paren] == '(' {
                    let mut k = open_paren + 1;
                    let mut url_end = None;
                    while k < chars.len() {
                        if chars[k] == ')' {
                            url_end = Some(k);
                            break;
                        }
                        k += 1;
                    }
                    if let Some(url_end) = url_end {
                        let label: String = chars[(i + 1)..label_end].iter().collect();
                        let href: String = chars[(open_paren + 1)..url_end].iter().collect();
                        let href = href.trim();
                        if supports_link_markup() {
                            out.push_str("<a href=\"");
                            out.push_str(&escape_markup(href));
                            out.push_str("\">");
                            out.push_str(&escape_markup(&label));
                            out.push_str("</a>");
                        } else {
                            out.push_str(&escape_markup(&format!("[{label}]({href})")));
                        }
                        i = url_end + 1;
                        continue;
                    }
                }
            }
        }
        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            let mut j = i + 2;
            let mut found = None;
            while j + 1 < chars.len() {
                if chars[j] == '*' && chars[j + 1] == '*' {
                    found = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(end) = found {
                let inner: String = chars[(i + 2)..end].iter().collect();
                out.push_str("<b>");
                out.push_str(&escape_markup(&inner));
                out.push_str("</b>");
                i = end + 2;
                continue;
            }
        }
        if chars[i] == '`' {
            let mut j = i + 1;
            let mut found = None;
            while j < chars.len() {
                if chars[j] == '`' {
                    found = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(end) = found {
                let inner: String = chars[(i + 1)..end].iter().collect();
                out.push_str("<tt>");
                out.push_str(&escape_markup(&inner));
                out.push_str("</tt>");
                i = end + 1;
                continue;
            }
        }
        out.push_str(&escape_markup(&chars[i].to_string()));
        i += 1;
    }
    out
}

fn markdown_to_pango(text: &str) -> String {
    let mut out = String::new();
    let mut in_code_block = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim_start().starts_with("```") {
            if in_code_block {
                out.push_str("</tt></span>\n");
                in_code_block = false;
            } else {
                out.push_str("<span alpha=\"85%\"><tt>");
                in_code_block = true;
            }
            continue;
        }
        if in_code_block {
            out.push_str(&escape_markup(trimmed));
            out.push('\n');
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            out.push_str("<span weight=\"700\">");
            out.push_str(&render_inline_markdown(rest));
            out.push_str("</span>\n");
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("## ") {
            out.push_str("<span weight=\"700\" size=\"larger\">");
            out.push_str(&render_inline_markdown(rest));
            out.push_str("</span>\n");
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# ") {
            out.push_str("<span weight=\"700\" size=\"x-large\">");
            out.push_str(&render_inline_markdown(rest));
            out.push_str("</span>\n");
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("- ") {
            out.push_str("• ");
            out.push_str(&render_inline_markdown(rest));
            out.push('\n');
            continue;
        }
        out.push_str(&render_inline_markdown(trimmed));
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn set_markdown(label: &gtk::Label, text: &str) {
    if looks_like_unfenced_diff(text) {
        label.set_attributes(None);
        label.set_use_markup(false);
        label.set_text(text);
        return;
    }
    let markup = markdown_to_pango(text);
    if markup.is_empty() {
        label.set_attributes(None);
        label.set_use_markup(false);
        label.set_text("");
        return;
    }
    match gtk::pango::parse_markup(&markup, '\0') {
        Ok((attrs, rendered, _)) => {
            label.set_attributes(Some(&attrs));
            label.set_use_markup(false);
            label.set_text(&rendered);
        }
        Err(err) => {
            eprintln!("thread settings markdown parse failed, falling back to plain text: {err}");
            label.set_attributes(None);
            label.set_use_markup(false);
            label.set_text(text);
        }
    }
}

fn resolve_agents_file(workspace_path: &str) -> Option<PathBuf> {
    let workspace = Path::new(workspace_path);
    let upper = workspace.join("AGENTS.md");
    if upper.exists() {
        return Some(upper);
    }
    let lower = workspace.join("agents.md");
    if lower.exists() {
        return Some(lower);
    }
    None
}

pub fn show(parent: Option<&gtk::Window>, workspace_name: &str, workspace_path: &str) {
    let dialog = gtk::Window::builder()
        .title("Thread Settings")
        .default_width(720)
        .default_height(620)
        .modal(true)
        .build();
    if let Some(parent) = parent {
        dialog.set_transient_for(Some(parent));
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(14);
    root.set_margin_end(14);
    root.set_margin_top(14);
    root.set_margin_bottom(14);

    let top = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    top.set_hexpand(true);
    let title_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    title_box.set_hexpand(true);
    let title = gtk::Label::new(Some("AGENTS.md"));
    title.set_xalign(0.0);
    title.add_css_class("thread-settings-title");
    let subtitle = gtk::Label::new(Some(&format!("{workspace_name}  •  {}", workspace_path)));
    subtitle.set_xalign(0.0);
    subtitle.set_wrap(true);
    subtitle.add_css_class("thread-settings-subtitle");
    title_box.append(&title);
    title_box.append(&subtitle);

    let open_button = gtk::Button::new();
    open_button.add_css_class("app-flat-button");
    open_button.add_css_class("circular");
    open_button.add_css_class("thread-settings-open-button");
    open_button.set_icon_name("document-edit-symbolic");
    open_button.set_tooltip_text(Some("Open AGENTS.md in default app"));
    top.append(&title_box);
    top.append(&open_button);
    root.append(&top);

    let content_frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_frame.add_css_class("thread-settings-content");
    let content_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();
    content_scroll.set_has_frame(false);

    let markdown_label = gtk::Label::new(None);
    markdown_label.set_xalign(0.0);
    markdown_label.set_yalign(0.0);
    markdown_label.set_wrap(true);
    markdown_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    markdown_label.set_selectable(true);
    markdown_label.set_margin_start(14);
    markdown_label.set_margin_end(14);
    markdown_label.set_margin_top(14);
    markdown_label.set_margin_bottom(14);
    content_scroll.set_child(Some(&markdown_label));
    content_frame.append(&content_scroll);
    root.append(&content_frame);

    let agents_file = resolve_agents_file(workspace_path);
    if let Some(path) = agents_file.as_ref() {
        let text = fs::read_to_string(path).unwrap_or_else(|_| "".to_string());
        if text.trim().is_empty() {
            markdown_label.set_text("AGENTS.md is empty.");
        } else {
            set_markdown(&markdown_label, &text);
        }
    } else {
        markdown_label.set_text(
            "No AGENTS.md found in this workspace.\n\nCreate `AGENTS.md` in the workspace root to define project-specific instructions.",
        );
    }

    open_button.set_sensitive(agents_file.is_some());
    if let Some(path) = agents_file {
        open_button.connect_clicked(move |_| {
            let uri = gtk::gio::File::for_path(&path).uri();
            let _ = gtk::gio::AppInfo::launch_default_for_uri(
                &uri,
                None::<&gtk::gio::AppLaunchContext>,
            );
        });
    }

    let close_button = gtk::Button::with_label("Close");
    close_button.set_halign(gtk::Align::End);
    {
        let dialog = dialog.clone();
        close_button.connect_clicked(move |_| dialog.close());
    }
    root.append(&close_button);

    dialog.set_child(Some(&root));
    dialog.present();
}
