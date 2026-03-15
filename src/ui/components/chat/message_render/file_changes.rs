fn diff_line_counts(diff: &str) -> (usize, usize) {
    let mut added = 0usize;
    let mut removed = 0usize;
    for line in diff.lines() {
        if line.starts_with("+++")
            || line.starts_with("---")
            || line.starts_with("@@")
            || line.starts_with("diff --git")
            || line.starts_with("index ")
        {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

const DIFF_PREVIEW_MAX_LINES: usize = 120;

fn truncate_diff_text(diff: &str) -> (String, bool) {
    let lines: Vec<&str> = diff.lines().collect();
    if lines.len() <= DIFF_PREVIEW_MAX_LINES {
        (diff.to_string(), false)
    } else {
        (
            format!(
                "{}\n… diff truncated",
                lines[..DIFF_PREVIEW_MAX_LINES].join("\n")
            ),
            true,
        )
    }
}

#[allow(dead_code)]
fn diff_to_pango_markup(diff: &str) -> String {
    let mut out = String::from("<tt>");
    for line in diff.lines() {
        let escaped = gtk::glib::markup_escape_text(line).to_string();
        if line.starts_with('+') && !line.starts_with("+++") {
            out.push_str("<span foreground=\"#9ad6a6\" background=\"#233b2a\">");
            out.push_str(&escaped);
            out.push_str("</span>\n");
        } else if line.starts_with('-') && !line.starts_with("---") {
            out.push_str("<span foreground=\"#e6a3ad\" background=\"#3d2428\">");
            out.push_str(&escaped);
            out.push_str("</span>\n");
        } else if line.starts_with("@@")
            || line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("+++")
            || line.starts_with("---")
        {
            out.push_str("<span foreground=\"#9aa4b2\">");
            out.push_str(&escaped);
            out.push_str("</span>\n");
        } else {
            out.push_str(&escaped);
            out.push('\n');
        }
    }
    out.push_str("</tt>");
    out
}

fn kind_display(kind: &str) -> &'static str {
    match kind {
        "add" | "added" | "create" | "created" => "Added",
        "delete" | "deleted" | "remove" | "removed" => "Deleted",
        _ => "Updated",
    }
}

fn short_file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(|value| value.to_string())
        .unwrap_or_else(|| path.to_string())
}

fn is_created_kind(kind: &str) -> bool {
    matches!(kind, "add" | "added" | "create" | "created")
}

fn single_change_is_new_file(changes: &[Value], operation: &str) -> bool {
    if changes.len() != 1 {
        return false;
    }

    if operation == "create" {
        return true;
    }

    changes
        .first()
        .and_then(|change| change.get("kind").and_then(Value::as_str))
        .is_some_and(is_created_kind)
}

fn create_file_change_row(path: &str, kind: &str, diff: Option<&str>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
    row.add_css_class("chat-filechange-row");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);

    let path_label = gtk::Label::new(Some(path));
    path_label.set_xalign(0.0);
    path_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    path_label.set_hexpand(true);
    path_label.add_css_class("chat-filechange-path");
    header.append(&path_label);

    let meta_label = gtk::Label::new(None);
    meta_label.set_xalign(1.0);
    meta_label.add_css_class("chat-filechange-meta");
    header.append(&meta_label);

    let diff_text = diff.unwrap_or_default().to_string();
    if diff_text.trim().is_empty() {
        meta_label.set_use_markup(false);
        meta_label.set_text(kind_display(kind));
        row.append(&header);
        return row;
    }

    let (added, removed) = diff_line_counts(&diff_text);
    let was_truncated = diff_text.lines().count() > DIFF_PREVIEW_MAX_LINES;
    meta_label.set_use_markup(true);
    meta_label.set_markup(&format!(
        "{}  <span foreground=\"#8CCF8C\">+{}</span> <span foreground=\"#E28E8E\">-{}</span>{}",
        gtk::glib::markup_escape_text(kind_display(kind)),
        added,
        removed,
        if was_truncated {
            "  <span foreground=\"#98A1AC\">(truncated)</span>"
        } else {
            ""
        }
    ));

    let header_toggle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    header_toggle.add_css_class("chat-filechange-toggle");
    header_toggle.set_halign(gtk::Align::Fill);
    header_toggle.set_hexpand(true);
    header_toggle.set_can_target(true);
    header_toggle.append(&header);
    row.append(&header_toggle);

    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    revealer.set_reveal_child(false);
    row.append(&revealer);

    let diff_text = Rc::new(diff_text);
    let body_built = Rc::new(RefCell::new(false));
    let is_expanded = Rc::new(RefCell::new(true));
    let ensure_body: Rc<dyn Fn()> = {
        let revealer_weak = revealer.downgrade();
        let diff_text = diff_text.clone();
        let body_built = body_built.clone();
        Rc::new(move || {
            if *body_built.borrow() {
                return;
            }
            let Some(revealer) = revealer_weak.upgrade() else {
                return;
            };
            let (diff_preview, _) = truncate_diff_text(diff_text.as_str());
            let diff_label = gtk::Label::new(Some(&diff_preview));
            diff_label.set_selectable(true);
            diff_label.set_xalign(0.0);
            diff_label.set_wrap(false);
            diff_label.add_css_class("chat-filechange-diff");
            let diff_markup = diff_to_pango_markup(&diff_preview);
            match gtk::pango::parse_markup(&diff_markup, '\0') {
                Ok((attrs, rendered, _)) => {
                    diff_label.set_attributes(Some(&attrs));
                    diff_label.set_use_markup(false);
                    diff_label.set_text(&rendered);
                }
                Err(err) => {
                    eprintln!(
                        "file change diff markup parse failed, falling back to plain text: {err}"
                    );
                    diff_label.set_attributes(None);
                    diff_label.set_use_markup(false);
                    diff_label.set_text(&diff_preview);
                }
            }

            let diff_scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Automatic)
                .vscrollbar_policy(gtk::PolicyType::Automatic)
                .min_content_height(72)
                .max_content_height(180)
                .child(&diff_label)
                .build();
            diff_scroll.set_has_frame(false);
            diff_scroll.add_css_class("chat-filechange-diff-scroll");

            {
                let scroll_ctrl =
                    gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
                scroll_ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
                let adj = diff_scroll.vadjustment();
                scroll_ctrl.connect_scroll(move |_, _, dy| {
                    let step = adj.step_increment().max(20.0);
                    let new_val = adj.value() + dy * step;
                    let max = (adj.upper() - adj.page_size()).max(adj.lower());
                    adj.set_value(new_val.clamp(adj.lower(), max));
                    gtk::glib::Propagation::Stop
                });
                diff_scroll.add_controller(scroll_ctrl);
            }

            revealer.set_child(Some(&diff_scroll));
            *body_built.borrow_mut() = true;
        })
    };

    {
        let ensure_body = ensure_body.clone();
        let is_expanded = is_expanded.clone();
        let revealer_weak = revealer.downgrade();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            let next = {
                let mut expanded = is_expanded.borrow_mut();
                *expanded = !*expanded;
                *expanded
            };
            if next {
                ensure_body();
            }
            let Some(revealer) = revealer_weak.upgrade() else {
                return;
            };
            revealer.set_reveal_child(next);
        });
        header_toggle.add_controller(click);
    }

    {
        let ensure_body = ensure_body.clone();
        let is_expanded = is_expanded.clone();
        let revealer_weak = revealer.downgrade();
        row.connect_map(move |_| {
            if !*is_expanded.borrow() {
                return;
            }
            ensure_body();
            if let Some(revealer) = revealer_weak.upgrade() {
                revealer.set_reveal_child(true);
            }
        });
    }

    row
}

pub(super) fn create_file_change_widget(item: &Value) -> gtk::Box {
    let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 0);
    wrapper.add_css_class("chat-filechange-card");
    wrapper.add_css_class("chat-activity-card");

    let changes = item
        .get("changes")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let operation = item
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("edit");
    let is_new_file = single_change_is_new_file(changes, operation);
    let section = if is_new_file {
        "New File".to_string()
    } else if changes.len() == 1 && operation == "write" {
        "File Write".to_string()
    } else if changes.len() == 1 {
        "File Edit".to_string()
    } else {
        format!("File Edits ({})", changes.len())
    };

    let mut total_added = 0usize;
    let mut total_removed = 0usize;
    for change in changes {
        let diff = change
            .get("diff")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (added, removed) = diff_line_counts(diff);
        total_added += added;
        total_removed += removed;
    }
    let lead_file_name = changes
        .first()
        .and_then(|change| change.get("path").and_then(Value::as_str))
        .map(short_file_name)
        .unwrap_or_else(|| "No file".to_string());
    let files_summary = if changes.is_empty() {
        "No file".to_string()
    } else if changes.len() == 1 && total_added == 0 && total_removed == 0 {
        gtk::glib::markup_escape_text(&lead_file_name).to_string()
    } else if changes.len() == 1 {
        format!(
            "{}  <span foreground=\"#8CCF8C\">+{}</span> <span foreground=\"#E28E8E\">-{}</span>",
            gtk::glib::markup_escape_text(&lead_file_name),
            total_added,
            total_removed
        )
    } else {
        format!(
            "{} (+{} files)  <span foreground=\"#8CCF8C\">+{}</span> <span foreground=\"#E28E8E\">-{}</span>",
            gtk::glib::markup_escape_text(&lead_file_name),
            changes.len().saturating_sub(1),
            total_added,
            total_removed
        )
    };

    let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    header_row.add_css_class("chat-activity-row");
    header_row.add_css_class("chat-activity-toggle");
    header_row.set_halign(gtk::Align::Fill);
    header_row.set_hexpand(true);
    header_row.set_can_target(true);
    header_row.set_baseline_position(gtk::BaselinePosition::Center);

    let icon = gtk::Image::from_icon_name("pencil-symbolic");
    icon.set_pixel_size(12);
    icon.set_valign(gtk::Align::Center);
    icon.add_css_class("chat-command-section-icon");
    header_row.append(&icon);

    let title = gtk::Label::new(Some(&section));
    title.set_xalign(0.0);
    title.set_valign(gtk::Align::Baseline);
    title.add_css_class("chat-command-section-title");
    header_row.append(&title);

    let summary = gtk::Label::new(None);
    summary.set_xalign(0.0);
    summary.set_valign(gtk::Align::Baseline);
    summary.set_hexpand(true);
    summary.set_wrap(false);
    summary.set_single_line_mode(true);
    summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
    summary.add_css_class("chat-command-header");
    summary.set_use_markup(true);
    summary.set_markup(&files_summary);
    header_row.append(&summary);

    wrapper.append(&header_row);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    list.add_css_class("chat-activity-details");
    list.add_css_class("chat-filechange-list");

    if changes.is_empty() {
        let label = gtk::Label::new(Some("No file details available."));
        label.set_xalign(0.0);
        label.add_css_class("chat-filechange-empty");
        list.append(&label);
    } else {
        for change in changes {
            let path = change
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("(unknown path)");
            let kind = change
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("updated");
            let diff = change.get("diff").and_then(Value::as_str);
            list.append(&create_file_change_row(path, kind, diff));
        }
    }

    let details_revealer = gtk::Revealer::new();
    details_revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    details_revealer.set_transition_duration(190);
    details_revealer.set_reveal_child(false);
    details_revealer.set_child(Some(&list));
    wrapper.append(&details_revealer);

    {
        let details_revealer_weak = details_revealer.downgrade();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| {
            let Some(details_revealer) = details_revealer_weak.upgrade() else {
                return;
            };
            let next = !details_revealer.reveals_child();
            details_revealer.set_reveal_child(next);
        });
        header_row.add_controller(click);
    }

    wrapper
}

#[cfg(test)]
mod tests {
    use super::diff_to_pango_markup;

    #[test]
    fn escapes_ampersands_in_diff_lines() {
        let diff = "+ let x = &messages_scroll;";
        let markup = diff_to_pango_markup(diff);
        assert!(markup.contains("&amp;messages_scroll"));
        assert!(gtk::pango::parse_markup(&markup, '\0').is_ok());
    }

    #[test]
    fn parses_real_world_patch_chunks() {
        let diff = r####"@@ -8,2 +8,9 @@

+fn supports_link_markup() -> bool {
+    static SUPPORTS_LINKS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
+    *SUPPORTS_LINKS.get_or_init(|| {
+        gtk::pango::parse_markup(r#"<a href="https://example.com">x</a>"#, '\0').is_ok()
+    })
+}
+
 fn render_inline_markdown(text: &str) -> String {"####;
        let markup = diff_to_pango_markup(diff);
        assert!(gtk::pango::parse_markup(&markup, '\0').is_ok());
    }
}
