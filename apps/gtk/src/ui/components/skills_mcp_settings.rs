use crate::services::app::runtime::RuntimeClient;
use crate::services::app::runtime::McpServerInfo;
use crate::services::app::CodexProfileManager;
use crate::services::app::chat::AppDb;
use crate::services::app::skills::{
    McpCatalogEntry, PolicyKind, ProfileAssignments, SkillCatalogEntry, SkillMcpCatalog,
    load_catalog, load_profile_assignments, remove_catalog_mcp, remove_catalog_skill,
    set_profile_assigned, supports_skill_assignment_for_backend, upsert_catalog_mcp,
    upsert_catalog_skill, write_skill_assignment_for_profile,
};
use crate::ui::components::skills_mcp_reload_guard::run_with_opencode_reload_guard;
use gtk::prelude::*;
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug)]
struct ProfileSettingsItem {
    id: i64,
    backend_kind: String,
    name: String,
    status: String,
}

impl ProfileSettingsItem {
    fn is_running(&self) -> bool {
        self.status.eq_ignore_ascii_case("running")
    }

    fn supports_skill_assignment(&self) -> bool {
        supports_skill_assignment_for_backend(&self.backend_kind)
    }
}

#[derive(Clone, Debug)]
struct SettingsData {
    runtime_profile_id: i64,
    profiles: Vec<ProfileSettingsItem>,
    catalog: SkillMcpCatalog,
    assignments: HashMap<i64, ProfileAssignments>,
    mcp_status: Vec<McpServerInfo>,
    warning: Option<String>,
}

fn load_settings_data(
    runtime_profile_id: i64,
    runtime_client: Option<Arc<RuntimeClient>>,
) -> SettingsData {
    let background_db = AppDb::open_default();
    let mut warning = None;

    let profiles = background_db
        .list_codex_profiles()
        .unwrap_or_default()
        .into_iter()
        .map(|profile| ProfileSettingsItem {
            id: profile.id,
            backend_kind: profile.backend_kind,
            name: profile.name,
            status: profile.status,
        })
        .collect::<Vec<_>>();

    let catalog = load_catalog(background_db.as_ref());

    let mut assignments = HashMap::<i64, ProfileAssignments>::new();
    for profile in &profiles {
        assignments.insert(
            profile.id,
            load_profile_assignments(background_db.as_ref(), profile.id),
        );
    }

    let mut mcp_status = Vec::<McpServerInfo>::new();
    if let Some(client) = runtime_client {
        match client.mcp_server_status_list(100) {
            Ok(items) => mcp_status = items,
            Err(err) => warning = Some(format!("mcpServerStatus/list failed: {err}")),
        }
    }

    SettingsData {
        runtime_profile_id,
        profiles,
        catalog,
        assignments,
        mcp_status,
        warning,
    }
}

fn normalize_mcp_server_name(input: &str) -> String {
    let mut out = String::new();
    for ch in input.trim().chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() || lower == '-' || lower == '_' {
            out.push(lower);
        } else if (lower.is_whitespace() || lower == '.') && !out.ends_with('_') {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "mcp_server".to_string()
    } else {
        trimmed
    }
}

fn parse_args_text(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(json_args) = serde_json::from_str::<Vec<String>>(trimmed) {
        return json_args
            .into_iter()
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect();
    }
    trimmed
        .split_whitespace()
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn parse_raw_mcp_text(raw: &str) -> Result<Value, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("Raw config text is empty.".to_string());
    }
    if let Ok(json_value) = serde_json::from_str::<Value>(trimmed) {
        return Ok(json_value);
    }
    if let Ok(toml_value) = toml::from_str::<toml::Value>(trimmed) {
        return serde_json::to_value(toml_value)
            .map_err(|err| format!("Failed to convert TOML to JSON value: {err}"));
    }
    Err("Raw MCP config must be valid JSON or TOML.".to_string())
}

fn normalized_server_config(value: &Value) -> Value {
    let Some(mut cfg) = value.as_object().cloned() else {
        return value.clone();
    };
    match cfg.get("type").and_then(Value::as_str) {
        Some("local") => {
            if let Some(command_parts) = cfg.get("command").and_then(Value::as_array) {
                let parts = command_parts
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>();
                if let Some((command, args)) = parts.split_first() {
                    cfg.insert("command".to_string(), Value::String(command.clone()));
                    cfg.insert(
                        "args".to_string(),
                        Value::Array(args.iter().cloned().map(Value::String).collect()),
                    );
                }
            }
            cfg.insert("transport".to_string(), Value::String("stdio".to_string()));
            cfg.remove("type");
        }
        Some("remote") => {
            cfg.insert(
                "transport".to_string(),
                Value::String("streamable_http".to_string()),
            );
            cfg.remove("type");
        }
        _ => {}
    }
    if !cfg.contains_key("transport") {
        if cfg.get("command").is_some() {
            if let Some(command_parts) = cfg.get("command").and_then(Value::as_array) {
                let parts = command_parts
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| value.to_string())
                    .collect::<Vec<_>>();
                if let Some((command, args)) = parts.split_first() {
                    cfg.insert("command".to_string(), Value::String(command.clone()));
                    cfg.insert(
                        "args".to_string(),
                        Value::Array(args.iter().cloned().map(Value::String).collect()),
                    );
                }
            }
            cfg.insert("transport".to_string(), Value::String("stdio".to_string()));
        } else if cfg.get("url").is_some() {
            cfg.insert(
                "transport".to_string(),
                Value::String("streamable_http".to_string()),
            );
        }
    }
    Value::Object(cfg)
}

fn build_mcp_entries_from_raw(
    name_hint: &str,
    raw: &Value,
) -> Result<Vec<(String, Value)>, String> {
    let Some(obj) = raw.as_object() else {
        return Err("Raw MCP config must be a JSON/TOML object.".to_string());
    };

    if let Some(mcp_servers) = obj.get("mcp_servers").and_then(Value::as_object) {
        let mut out = Vec::new();
        for (name, value) in mcp_servers {
            out.push((name.to_string(), normalized_server_config(value)));
        }
        if out.is_empty() {
            return Err("`mcp_servers` was present but empty.".to_string());
        }
        return Ok(out);
    }

    if let Some(mcp_servers) = obj.get("mcpServers").and_then(Value::as_object) {
        let mut out = Vec::new();
        for (name, value) in mcp_servers {
            out.push((name.to_string(), normalized_server_config(value)));
        }
        if out.is_empty() {
            return Err("`mcpServers` was present but empty.".to_string());
        }
        return Ok(out);
    }

    if let Some(mcp_servers) = obj.get("mcp").and_then(Value::as_object) {
        let mut out = Vec::new();
        for (name, value) in mcp_servers {
            out.push((name.to_string(), normalized_server_config(value)));
        }
        if out.is_empty() {
            return Err("`mcp` was present but empty.".to_string());
        }
        return Ok(out);
    }

    let fallback_name = name_hint.trim();
    if fallback_name.is_empty() {
        return Err("Server name is required in form or raw config.".to_string());
    }
    Ok(vec![(
        fallback_name.to_string(),
        normalized_server_config(raw),
    )])
}

fn yaml_quote_single_line(input: &str) -> String {
    let sanitized = input
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ")
        .replace('\r', " ")
        .trim()
        .to_string();
    format!("\"{}\"", sanitized)
}

fn strip_leading_yaml_frontmatter(raw: &str) -> String {
    let mut text = raw;
    if let Some(stripped) = text.strip_prefix('\u{feff}') {
        text = stripped;
    }

    let remainder = if let Some(stripped) = text.strip_prefix("---\n") {
        stripped
    } else if let Some(stripped) = text.strip_prefix("---\r\n") {
        stripped
    } else {
        return raw.to_string();
    };

    let end_marker = remainder
        .find("\n---\n")
        .map(|idx| (idx, 5))
        .or_else(|| remainder.find("\n---\r\n").map(|idx| (idx, 6)));
    let Some((end_idx, marker_len)) = end_marker else {
        return raw.to_string();
    };
    let mut body = remainder[end_idx + marker_len..].to_string();
    while body.starts_with('\n') || body.starts_with('\r') {
        body.remove(0);
    }
    body
}

fn extract_yaml_frontmatter(raw: &str) -> Option<(Option<String>, Option<String>, String)> {
    let mut text = raw;
    if let Some(stripped) = text.strip_prefix('\u{feff}') {
        text = stripped;
    }

    let remainder = if let Some(stripped) = text.strip_prefix("---\n") {
        stripped
    } else if let Some(stripped) = text.strip_prefix("---\r\n") {
        stripped
    } else {
        return None;
    };

    let end_marker = remainder
        .find("\n---\n")
        .map(|idx| (idx, 5))
        .or_else(|| remainder.find("\n---\r\n").map(|idx| (idx, 6)));
    let (end_idx, marker_len) = end_marker?;
    let yaml_text = &remainder[..end_idx];
    let mut body = remainder[end_idx + marker_len..].to_string();
    while body.starts_with('\n') || body.starts_with('\r') {
        body.remove(0);
    }

    let yaml = serde_yaml::from_str::<serde_yaml::Value>(yaml_text).ok()?;
    let name = yaml
        .get("name")
        .and_then(serde_yaml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let description = yaml
        .get("description")
        .and_then(serde_yaml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    Some((name, description, body))
}

fn build_pasted_skill_content(raw: &str, name: &str, description: &str) -> String {
    let body = strip_leading_yaml_frontmatter(raw);
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("name: {}\n", yaml_quote_single_line(name.trim())));
    let final_description = if description.trim().is_empty() {
        format!("Custom skill: {}", name.trim())
    } else {
        description.trim().to_string()
    };
    out.push_str(&format!(
        "description: {}\n",
        yaml_quote_single_line(&final_description)
    ));
    out.push_str("---\n\n");
    out.push_str(&body);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn apply_editor_theme(
    scroll: &gtk::ScrolledWindow,
    view: &gtk::TextView,
    scroll_name: &str,
    view_name: &str,
) {
    scroll.set_widget_name(scroll_name);
    view.set_widget_name(view_name);

    let provider = gtk::CssProvider::new();
    let css = format!(
        r#"
#{scroll_name},
#{scroll_name} > viewport,
#{scroll_name} > textview,
#{scroll_name} > border {{
  border-radius: 10px;
  border: 1px solid alpha(@window_fg_color, 0.14);
  background: alpha(@window_fg_color, 0.05);
  background-color: alpha(@window_fg_color, 0.05);
  background-image: none;
}}

textview#{view_name},
textview#{view_name}.view,
textview#{view_name} border,
textview#{view_name} text {{
  background: alpha(@window_fg_color, 0.05);
  background-color: alpha(@window_fg_color, 0.05);
  background-image: none;
  color: @window_fg_color;
  border-width: 0;
  border-style: none;
  border-color: transparent;
  box-shadow: unset;
}}
"#
    );
    provider.load_from_string(&css);

    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
    }
}

fn set_skill_for_profile(
    profile: &crate::services::app::chat::CodexProfileRecord,
    skill: &SkillCatalogEntry,
    enabled: bool,
    client: Arc<RuntimeClient>,
) -> Result<(), String> {
    write_skill_assignment_for_profile(profile, &skill.slug, &skill.content, enabled)?;

    let _ = client.skills_list(&[], true);

    Ok(())
}

fn set_mcp_for_profile(
    mcp: &McpCatalogEntry,
    enabled: bool,
    client: Arc<RuntimeClient>,
) -> Result<(), String> {
    let key_path = format!("mcp_servers.{}", normalize_mcp_server_name(&mcp.name));
    if enabled {
        client.config_batch_write(vec![(key_path, mcp.config.clone(), "upsert".to_string())])?;
    } else {
        client.config_value_write(&key_path, Value::Null, "replace")?;
    }
    client.config_mcp_server_reload()?;
    Ok(())
}

fn mcp_auth_map(items: &[McpServerInfo]) -> HashMap<String, (String, bool)> {
    items
        .iter()
        .map(|item| {
            (
                normalize_mcp_server_name(&item.name),
                (item.auth_label.clone(), item.authenticated),
            )
        })
        .collect::<HashMap<_, _>>()
}

fn open_add_skill_dialog(
    parent: &gtk::Window,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    status_label: gtk::Label,
    on_saved: Rc<dyn Fn()>,
) {
    let dialog = gtk::Window::builder()
        .title("Add Skill")
        .default_width(700)
        .default_height(520)
        .modal(true)
        .transient_for(parent)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let intro = gtk::Label::new(Some(
        "Add a skill from markdown or paste skill text. Skills are stored globally in Enzim Coder DB, then assigned per profile below.",
    ));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    root.append(&intro);

    let name_entry = gtk::Entry::new();
    name_entry.set_placeholder_text(Some("Skill name (used as $marker, e.g. frontend-design)"));
    root.append(&name_entry);

    let desc_entry = gtk::Entry::new();
    desc_entry.set_placeholder_text(Some("Description (optional)"));
    root.append(&desc_entry);

    let mode_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let mode_label = gtk::Label::new(Some("Source"));
    mode_label.set_xalign(0.0);
    mode_label.set_width_chars(10);
    let mode_dropdown = gtk::DropDown::from_strings(&["Browse .md file", "Paste text"]);
    mode_row.append(&mode_label);
    mode_row.append(&mode_dropdown);
    root.append(&mode_row);

    let auto_assign = gtk::CheckButton::with_label("Assign to active profile now (if running)");
    auto_assign.set_active(true);
    root.append(&auto_assign);

    let file_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let file_entry = gtk::Entry::new();
    file_entry.set_placeholder_text(Some("Path to SKILL.md or another .md file"));
    file_entry.set_hexpand(true);
    let file_browse = gtk::Button::with_label("Browse");
    file_browse.add_css_class("app-flat-button");
    file_row.append(&file_entry);
    file_row.append(&file_browse);
    root.append(&file_row);

    let text_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(220)
        .build();
    text_scroll.add_css_class("skills-mcp-editor-scroll");
    let text_view = gtk::TextView::new();
    text_view.add_css_class("skills-mcp-editor-view");
    text_view.set_wrap_mode(gtk::WrapMode::WordChar);
    apply_editor_theme(
        &text_scroll,
        &text_view,
        "skill-editor-scroll",
        "skill-editor-view",
    );
    text_view.buffer().set_text(
        "# Skill Name\n\nDescribe when this skill should be used and the exact workflow.",
    );
    text_scroll.set_child(Some(&text_view));
    root.append(&text_scroll);

    {
        let name_entry = name_entry.clone();
        let desc_entry = desc_entry.clone();
        let mode_dropdown = mode_dropdown.clone();
        let buffer = text_view.buffer();
        let guard = Rc::new(RefCell::new(false));
        let pending = Rc::new(RefCell::new(None::<gtk::glib::SourceId>));
        let pending_for_change = pending.clone();
        let guard_for_change = guard.clone();
        buffer.connect_changed(move |buf| {
            if mode_dropdown.selected() != 1 || *guard_for_change.borrow() {
                return;
            }
            if let Some(source_id) = pending_for_change.borrow_mut().take() {
                source_id.remove();
            }

            let name_entry = name_entry.clone();
            let desc_entry = desc_entry.clone();
            let buf = buf.clone();
            let guard = guard_for_change.clone();
            let pending = pending_for_change.clone();
            let source_id = gtk::glib::timeout_add_local(Duration::from_millis(250), move || {
                pending.borrow_mut().take();
                if *guard.borrow() {
                    return gtk::glib::ControlFlow::Break;
                }
                let text = buf
                    .text(&buf.start_iter(), &buf.end_iter(), true)
                    .to_string();
                let Some((name, description, body)) = extract_yaml_frontmatter(&text) else {
                    return gtk::glib::ControlFlow::Break;
                };
                *guard.borrow_mut() = true;
                if let Some(name) = name {
                    name_entry.set_text(&name);
                }
                if let Some(description) = description {
                    desc_entry.set_text(&description);
                }
                buf.set_text(&body);
                *guard.borrow_mut() = false;
                gtk::glib::ControlFlow::Break
            });
            pending_for_change.borrow_mut().replace(source_id);
        });
    }

    let local_status = gtk::Label::new(Some(""));
    local_status.set_xalign(0.0);
    local_status.set_wrap(true);
    local_status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    local_status.add_css_class("chat-profile-card-hint");
    root.append(&local_status);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save Skill");
    save.add_css_class("suggested-action");
    actions.append(&cancel);
    actions.append(&save);
    root.append(&actions);

    {
        let file_row = file_row.clone();
        let text_scroll = text_scroll.clone();
        let mode_dropdown = mode_dropdown.clone();
        let mode_dropdown_for_update = mode_dropdown.clone();
        let update_mode = move || {
            let is_browse = mode_dropdown_for_update.selected() == 0;
            file_row.set_visible(is_browse);
            text_scroll.set_visible(!is_browse);
        };
        update_mode();
        mode_dropdown.connect_selected_notify(move |_| update_mode());
    }

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }

    {
        let dialog = dialog.clone();
        let file_entry = file_entry.clone();
        file_browse.connect_clicked(move |_| {
            let chooser = gtk::FileDialog::builder()
                .title("Select Skill Markdown")
                .build();
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("Markdown"));
            filter.add_pattern("*.md");
            let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            chooser.set_filters(Some(&filters));
            chooser.set_default_filter(Some(&filter));
            let file_entry = file_entry.clone();
            chooser.open(
                Some(&dialog),
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    if let Ok(file) = result {
                        if let Some(path) = file.path() {
                            file_entry.set_text(&path.to_string_lossy());
                        }
                    }
                },
            );
        });
    }

    {
        let dialog = dialog.clone();
        let name_entry = name_entry.clone();
        let desc_entry = desc_entry.clone();
        let mode_dropdown = mode_dropdown.clone();
        let file_entry = file_entry.clone();
        let text_view = text_view.clone();
        let auto_assign = auto_assign.clone();
        let local_status = local_status.clone();
        let status_label = status_label.clone();
        let on_saved = on_saved.clone();
        let manager = manager.clone();
        save.connect_clicked(move |_| {
            let profile_id = db.runtime_profile_id().ok().flatten().unwrap_or(1);
            let assign_now = auto_assign.is_active();
            let mut name = name_entry.text().trim().to_string();
            let description = desc_entry.text().trim().to_string();
            let is_browse = mode_dropdown.selected() == 0;
            let source_path = file_entry.text().trim().to_string();
            let buffer = text_view.buffer();
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), true)
                .to_string();

            if name.is_empty() && is_browse {
                name = Path::new(&source_path)
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(|value| value.to_string())
                    .unwrap_or_default();
            }
            if name.trim().is_empty() {
                local_status.set_text("Skill name is required.");
                return;
            }
            if is_browse && source_path.trim().is_empty() {
                local_status.set_text("Choose a .md skill file first.");
                return;
            }
            if !is_browse && text.trim().is_empty() {
                local_status.set_text("Paste skill content first.");
                return;
            }

            let start_save: Rc<dyn Fn()> = {
                let manager = manager.clone();
                let local_status = local_status.clone();
                let status_label = status_label.clone();
                let on_saved = on_saved.clone();
                let dialog = dialog.clone();
                let source_path_for_thread = source_path.clone();
                let name_for_thread = name.clone();
                let description_for_thread = description.clone();
                let text_for_thread = text.clone();
                Rc::new(move || {
                    local_status.set_text("Saving skill...");
                    let client = if assign_now {
                        manager.running_client_for_profile(profile_id)
                    } else {
                        None
                    };
                    let (tx, rx) = mpsc::channel::<Result<(), String>>();
                    let source_path_for_thread = source_path_for_thread.clone();
                    let name_for_thread = name_for_thread.clone();
                    let description_for_thread = description_for_thread.clone();
                    let text_for_thread = text_for_thread.clone();
                    thread::spawn(move || {
                        let result = (|| -> Result<(), String> {
                            let content = if is_browse {
                                std::fs::read_to_string(&source_path_for_thread).map_err(|err| {
                                    format!(
                                        "Failed to read source skill file {}: {err}",
                                        source_path_for_thread
                                    )
                                })?
                            } else {
                                build_pasted_skill_content(
                                    &text_for_thread,
                                    &name_for_thread,
                                    &description_for_thread,
                                )
                            };

                            let background_db = AppDb::open_default();
                            let entry = upsert_catalog_skill(
                                background_db.as_ref(),
                                &name_for_thread,
                                &description_for_thread,
                                &content,
                            )?;

                            if assign_now {
                                set_profile_assigned(
                                    background_db.as_ref(),
                                    profile_id,
                                    PolicyKind::Skill,
                                    &entry.key,
                                    true,
                                )?;
                                let profile = background_db
                                    .get_codex_profile(profile_id)
                                    .map_err(|err| err.to_string())?
                                    .ok_or_else(|| format!("profile {} not found", profile_id))?;
                                if !supports_skill_assignment_for_backend(&profile.backend_kind) {
                                    return Err(
                                        "Skill assignment is not supported for this runtime profile."
                                            .to_string(),
                                    );
                                }
                                let Some(client) = client else {
                                    return Err(
                                        "Active profile is not running, cannot apply skill."
                                            .to_string(),
                                    );
                                };
                                set_skill_for_profile(&profile, &entry, true, client)?;
                            }
                            Ok(())
                        })();
                        let _ = tx.send(result);
                    });

                    let local_status = local_status.clone();
                    let status_label = status_label.clone();
                    let on_saved = on_saved.clone();
                    let dialog = dialog.clone();
                    gtk::glib::timeout_add_local(
                        Duration::from_millis(60),
                        move || match rx.try_recv() {
                            Ok(Ok(())) => {
                                status_label.set_text("Skill saved.");
                                on_saved();
                                dialog.close();
                                gtk::glib::ControlFlow::Break
                            }
                            Ok(Err(err)) => {
                                local_status.set_text(&format!("Failed to save skill: {err}"));
                                gtk::glib::ControlFlow::Break
                            }
                            Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                gtk::glib::ControlFlow::Break
                            }
                        },
                    );
                })
            };

            if assign_now {
                let local_status = local_status.clone();
                run_with_opencode_reload_guard(
                    Some(&dialog),
                    manager.clone(),
                    profile_id,
                    status_label.clone(),
                    start_save,
                    Rc::new(move || {
                        local_status.set_text("Skill save canceled.");
                    }),
                );
            } else {
                start_save();
            }
        });
    }

    dialog.set_child(Some(&root));
    dialog.present();
}

fn open_add_mcp_server_dialog(
    parent: &gtk::Window,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    status_label: gtk::Label,
    on_saved: Rc<dyn Fn()>,
) {
    let dialog = gtk::Window::builder()
        .title("Add MCP Server")
        .default_width(760)
        .default_height(560)
        .modal(true)
        .transient_for(parent)
        .build();

    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_start(12);
    root.set_margin_end(12);
    root.set_margin_top(12);
    root.set_margin_bottom(12);

    let intro = gtk::Label::new(Some(
        "Add MCP server config to the global catalog. Then assign it per profile below.",
    ));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    root.append(&intro);

    let name_entry = gtk::Entry::new();
    name_entry.set_placeholder_text(Some("Server name (e.g. github, linear)"));
    root.append(&name_entry);

    let desc_entry = gtk::Entry::new();
    desc_entry.set_placeholder_text(Some("Description (optional, local UI metadata)"));
    root.append(&desc_entry);

    let mode_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let mode_label = gtk::Label::new(Some("Input mode"));
    mode_label.set_width_chars(10);
    mode_label.set_xalign(0.0);
    let mode_dropdown = gtk::DropDown::from_strings(&["Form", "Raw config text"]);
    mode_row.append(&mode_label);
    mode_row.append(&mode_dropdown);
    root.append(&mode_row);

    let auto_assign = gtk::CheckButton::with_label("Assign to active profile now (if running)");
    auto_assign.set_active(true);
    root.append(&auto_assign);

    let form_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let transport_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let transport_label = gtk::Label::new(Some("Transport"));
    transport_label.set_width_chars(10);
    transport_label.set_xalign(0.0);
    let transport_dropdown = gtk::DropDown::from_strings(&["streamable_http", "stdio"]);
    transport_row.append(&transport_label);
    transport_row.append(&transport_dropdown);
    form_box.append(&transport_row);

    let url_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let url_label = gtk::Label::new(Some("URL"));
    url_label.set_width_chars(10);
    url_label.set_xalign(0.0);
    let url_entry = gtk::Entry::new();
    url_entry.set_placeholder_text(Some("https://example.com/mcp"));
    url_entry.set_hexpand(true);
    url_row.append(&url_label);
    url_row.append(&url_entry);
    form_box.append(&url_row);

    let command_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let command_label = gtk::Label::new(Some("Command"));
    command_label.set_width_chars(10);
    command_label.set_xalign(0.0);
    let command_entry = gtk::Entry::new();
    command_entry.set_placeholder_text(Some("npx"));
    command_entry.set_hexpand(true);
    command_row.append(&command_label);
    command_row.append(&command_entry);
    form_box.append(&command_row);

    let args_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let args_label = gtk::Label::new(Some("Args"));
    args_label.set_width_chars(10);
    args_label.set_xalign(0.0);
    let args_entry = gtk::Entry::new();
    args_entry.set_placeholder_text(Some(
        r#"space-separated or JSON array, e.g. ["-y", "@pkg/server"]"#,
    ));
    args_entry.set_hexpand(true);
    args_row.append(&args_label);
    args_row.append(&args_entry);
    form_box.append(&args_row);

    let required_toggle =
        gtk::CheckButton::with_label("Required server (thread start/resume fails if unavailable)");
    required_toggle.set_active(false);
    form_box.append(&required_toggle);

    root.append(&form_box);

    let raw_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .min_content_height(230)
        .build();
    raw_scroll.add_css_class("skills-mcp-editor-scroll");
    let raw_view = gtk::TextView::new();
    raw_view.add_css_class("skills-mcp-editor-view");
    raw_view.set_wrap_mode(gtk::WrapMode::WordChar);
    apply_editor_theme(
        &raw_scroll,
        &raw_view,
        "mcp-editor-scroll",
        "mcp-editor-view",
    );
    raw_view.buffer().set_text(
        "# Enzim / Codex JSON example\n# {\"mcp\": {\"github\": {\"transport\": \"streamable_http\", \"url\": \"https://example.com/mcp\"}}}\n\n# OpenCode JSON example\n# {\"mcp\": {\"github\": {\"type\": \"local\", \"command\": [\"npx\", \"-y\", \"@modelcontextprotocol/server-github\"]}}}\n\n# TOML example\n# [mcp.github]\n# transport = \"stdio\"\n# command = \"npx\"\n# args = [\"-y\", \"@modelcontextprotocol/server-github\"]\n",
    );
    raw_scroll.set_child(Some(&raw_view));
    root.append(&raw_scroll);

    let raw_hint = gtk::Label::new(Some(
        "Raw MCP config accepts both Enzim/Codex transport format and native OpenCode local/remote format. It will be normalized automatically.",
    ));
    raw_hint.set_xalign(0.0);
    raw_hint.set_wrap(true);
    raw_hint.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    raw_hint.add_css_class("chat-profile-card-hint");
    root.append(&raw_hint);

    let local_status = gtk::Label::new(Some(""));
    local_status.set_xalign(0.0);
    local_status.set_wrap(true);
    local_status.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    local_status.add_css_class("chat-profile-card-hint");
    root.append(&local_status);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save MCP Server");
    save.add_css_class("suggested-action");
    actions.append(&cancel);
    actions.append(&save);
    root.append(&actions);

    {
        let form_box = form_box.clone();
        let raw_scroll = raw_scroll.clone();
        let mode_dropdown = mode_dropdown.clone();
        let mode_dropdown_for_update = mode_dropdown.clone();
        let update_mode = move || {
            let is_form = mode_dropdown_for_update.selected() == 0;
            form_box.set_visible(is_form);
            raw_scroll.set_visible(!is_form);
        };
        update_mode();
        mode_dropdown.connect_selected_notify(move |_| update_mode());
    }

    {
        let url_row = url_row.clone();
        let command_row = command_row.clone();
        let args_row = args_row.clone();
        let transport_dropdown = transport_dropdown.clone();
        let transport_dropdown_for_update = transport_dropdown.clone();
        let update_transport = move || {
            let is_http = transport_dropdown_for_update.selected() == 0;
            url_row.set_visible(is_http);
            command_row.set_visible(!is_http);
            args_row.set_visible(!is_http);
        };
        update_transport();
        transport_dropdown.connect_selected_notify(move |_| update_transport());
    }

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }

    {
        let dialog = dialog.clone();
        let name_entry = name_entry.clone();
        let desc_entry = desc_entry.clone();
        let mode_dropdown = mode_dropdown.clone();
        let transport_dropdown = transport_dropdown.clone();
        let url_entry = url_entry.clone();
        let command_entry = command_entry.clone();
        let args_entry = args_entry.clone();
        let required_toggle = required_toggle.clone();
        let raw_view = raw_view.clone();
        let auto_assign = auto_assign.clone();
        let local_status = local_status.clone();
        let status_label = status_label.clone();
        let on_saved = on_saved.clone();
        let manager = manager.clone();
        save.connect_clicked(move |_| {
            let profile_id = db.runtime_profile_id().ok().flatten().unwrap_or(1);
            let assign_now = auto_assign.is_active();
            let name_input = name_entry.text().trim().to_string();
            let description = desc_entry.text().trim().to_string();
            let is_form = mode_dropdown.selected() == 0;

            let raw_buffer = raw_view.buffer();
            let raw_text = raw_buffer
                .text(&raw_buffer.start_iter(), &raw_buffer.end_iter(), true)
                .to_string();

            let mut edits = Vec::<(String, Value)>::new();
            if is_form {
                if name_input.trim().is_empty() {
                    local_status.set_text("Server name is required.");
                    return;
                }
                let is_http = transport_dropdown.selected() == 0;
                let mut cfg = serde_json::Map::new();
                if is_http {
                    let url = url_entry.text().trim().to_string();
                    if url.is_empty() {
                        local_status.set_text("URL is required for streamable_http transport.");
                        return;
                    }
                    cfg.insert(
                        "transport".to_string(),
                        Value::String("streamable_http".to_string()),
                    );
                    cfg.insert("url".to_string(), Value::String(url));
                } else {
                    let command = command_entry.text().trim().to_string();
                    if command.is_empty() {
                        local_status.set_text("Command is required for stdio transport.");
                        return;
                    }
                    cfg.insert("transport".to_string(), Value::String("stdio".to_string()));
                    cfg.insert("command".to_string(), Value::String(command));
                    cfg.insert(
                        "args".to_string(),
                        Value::Array(
                            parse_args_text(&args_entry.text())
                                .into_iter()
                                .map(Value::String)
                                .collect(),
                        ),
                    );
                }
                if required_toggle.is_active() {
                    cfg.insert("required".to_string(), Value::Bool(true));
                }
                edits.push((name_input.clone(), Value::Object(cfg)));
            } else {
                let parsed = match parse_raw_mcp_text(&raw_text) {
                    Ok(value) => value,
                    Err(err) => {
                        local_status.set_text(&err);
                        return;
                    }
                };
                edits = match build_mcp_entries_from_raw(&name_input, &parsed) {
                    Ok(items) => items,
                    Err(err) => {
                        local_status.set_text(&err);
                        return;
                    }
                };
            }

            let start_save: Rc<dyn Fn()> = {
                let manager = manager.clone();
                let local_status = local_status.clone();
                let status_label = status_label.clone();
                let on_saved = on_saved.clone();
                let dialog = dialog.clone();
                let description_for_thread = description.clone();
                let edits_for_thread = edits.clone();
                Rc::new(move || {
                    local_status.set_text("Saving MCP server config...");
                    let client = if assign_now {
                        manager.running_client_for_profile(profile_id)
                    } else {
                        None
                    };
                    let (tx, rx) = mpsc::channel::<Result<(), String>>();
                    let description_for_thread = description_for_thread.clone();
                    let edits_for_thread = edits_for_thread.clone();
                    thread::spawn(move || {
                        let result = (|| -> Result<(), String> {
                            let background_db = AppDb::open_default();
                            for (name, value) in &edits_for_thread {
                                let entry = upsert_catalog_mcp(
                                    background_db.as_ref(),
                                    name,
                                    &description_for_thread,
                                    value.clone(),
                                )?;
                                if assign_now {
                                    set_profile_assigned(
                                        background_db.as_ref(),
                                        profile_id,
                                        PolicyKind::Mcp,
                                        &entry.key,
                                        true,
                                    )?;
                                    let Some(client) = client.clone() else {
                                        return Err(
                                            "Active profile is not running, cannot apply MCP."
                                                .to_string(),
                                        );
                                    };
                                    set_mcp_for_profile(&entry, true, client)?;
                                }
                            }
                            Ok(())
                        })();
                        let _ = tx.send(result);
                    });

                    let local_status = local_status.clone();
                    let status_label = status_label.clone();
                    let on_saved = on_saved.clone();
                    let dialog = dialog.clone();
                    gtk::glib::timeout_add_local(Duration::from_millis(60), move || {
                        match rx.try_recv() {
                            Ok(Ok(())) => {
                                status_label.set_text("MCP server saved.");
                                on_saved();
                                dialog.close();
                                gtk::glib::ControlFlow::Break
                            }
                            Ok(Err(err)) => {
                                local_status.set_text(&format!("Failed to save MCP config: {err}"));
                                gtk::glib::ControlFlow::Break
                            }
                            Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                            Err(mpsc::TryRecvError::Disconnected) => gtk::glib::ControlFlow::Break,
                        }
                    });
                })
            };

            if assign_now {
                let local_status = local_status.clone();
                run_with_opencode_reload_guard(
                    Some(&dialog),
                    manager.clone(),
                    profile_id,
                    status_label.clone(),
                    start_save,
                    Rc::new(move || {
                        local_status.set_text("MCP save canceled.");
                    }),
                );
            } else {
                start_save();
            }
        });
    }

    dialog.set_child(Some(&root));
    dialog.present();
}

pub(crate) fn build_settings_page(
    parent: &gtk::Window,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
) -> gtk::Box {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 12);

    let info = gtk::Label::new(Some(
        "Global catalog: Skills and MCP servers are saved in Enzim Coder SQLite and can be assigned per profile.\n\nStopped profiles are read-only here. Start a profile first to change its assignments. Skill assignment is materialized into the selected runtime profile; MCP assignment is applied through the active runtime backend.",
    ));
    info.set_xalign(0.0);
    info.set_wrap(true);
    info.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    info.add_css_class("chat-profile-card-hint");
    root.append(&info);

    let status_label = gtk::Label::new(Some(""));
    status_label.set_xalign(0.0);
    status_label.set_wrap(true);
    status_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    status_label.add_css_class("chat-profile-card-hint");
    root.append(&status_label);

    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let add_skill_button = gtk::Button::with_label("Add Skill");
    add_skill_button.add_css_class("app-flat-button");
    let add_mcp_button = gtk::Button::with_label("Add MCP Server");
    add_mcp_button.add_css_class("app-flat-button");
    toolbar.append(&add_skill_button);
    toolbar.append(&add_mcp_button);
    root.append(&toolbar);

    let skills_title = gtk::Label::new(Some("Skills"));
    skills_title.set_xalign(0.0);
    skills_title.add_css_class("actions-section-heading");
    root.append(&skills_title);

    let skills_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.append(&skills_box);

    let mcp_title = gtk::Label::new(Some("MCP Servers"));
    mcp_title.set_xalign(0.0);
    mcp_title.add_css_class("actions-section-heading");
    root.append(&mcp_title);

    let mcps_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    root.append(&mcps_box);

    let refresh_handle: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let refresh_fn: Rc<dyn Fn()> = {
        let db = db.clone();
        let manager = manager.clone();
        let parent = parent.clone();
        let status_label = status_label.clone();
        let skills_box = skills_box.clone();
        let mcps_box = mcps_box.clone();
        let refresh_handle = refresh_handle.clone();
        Rc::new(move || {
            while let Some(child) = skills_box.first_child() {
                skills_box.remove(&child);
            }
            while let Some(child) = mcps_box.first_child() {
                mcps_box.remove(&child);
            }

            let runtime_profile_id = db.runtime_profile_id().ok().flatten().unwrap_or(1);
            let runtime_client = manager.running_client_for_profile(runtime_profile_id);
            status_label.set_text("Loading Skills/MCP catalog...");

            let (tx, rx) = mpsc::channel::<SettingsData>();
            thread::spawn(move || {
                let data = load_settings_data(runtime_profile_id, runtime_client);
                let _ = tx.send(data);
            });

            let status_label = status_label.clone();
            let skills_box = skills_box.clone();
            let mcps_box = mcps_box.clone();
            let manager_for_rows = manager.clone();
            let refresh_handle_for_rows = refresh_handle.clone();
            let parent_for_rows = parent.clone();
            gtk::glib::timeout_add_local(Duration::from_millis(60), move || match rx.try_recv() {
                Ok(data) => {
                    while let Some(child) = skills_box.first_child() {
                        skills_box.remove(&child);
                    }
                    while let Some(child) = mcps_box.first_child() {
                        mcps_box.remove(&child);
                    }

                    if data.catalog.skills.is_empty() {
                        let empty = gtk::Label::new(Some("No skills in catalog yet."));
                        empty.set_xalign(0.0);
                        empty.add_css_class("dim-label");
                        skills_box.append(&empty);
                    }

                    for skill in data.catalog.skills.clone() {
                        let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
                        row.add_css_class("actions-command-card");

                        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                        let name = gtk::Label::new(Some(&skill.name));
                        name.set_xalign(0.0);
                        name.set_hexpand(true);
                        name.add_css_class("actions-command-title");
                        top.append(&name);

                        let kind = gtk::Label::new(Some("Skill"));
                        kind.add_css_class("actions-run-status");
                        top.append(&kind);

                        let remove = gtk::Button::with_label("Remove");
                        remove.add_css_class("app-flat-button");
                        remove.add_css_class("actions-run-button");
                        top.append(&remove);
                        row.append(&top);

                        if !skill.description.trim().is_empty() {
                            let desc = gtk::Label::new(Some(&skill.description));
                            desc.set_xalign(0.0);
                            desc.set_wrap(true);
                            desc.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                            desc.add_css_class("actions-command-text");
                            row.append(&desc);
                        }

                        let chips = gtk::FlowBox::builder()
                            .selection_mode(gtk::SelectionMode::None)
                            .max_children_per_line(8)
                            .row_spacing(6)
                            .column_spacing(6)
                            .build();

                        for profile in &data.profiles {
                            let profile_assignments = data
                                .assignments
                                .get(&profile.id)
                                .cloned()
                                .unwrap_or_default();
                            let enabled = profile_assignments.skills.contains(&skill.key);
                            let toggle = gtk::CheckButton::with_label(&profile.name);
                            toggle.set_active(enabled);
                            let running = profile.is_running();
                            let can_toggle = running && profile.supports_skill_assignment();
                            toggle.set_sensitive(can_toggle);
                            if !running {
                                toggle.set_tooltip_text(Some(
                                    "Profile is stopped. Start it to change assignments.",
                                ));
                            } else if !profile.supports_skill_assignment() {
                                toggle.set_tooltip_text(Some(
                                    "Skill assignment is not supported for this runtime profile.",
                                ));
                            }

                            let profile_id = profile.id;
                            let profile_backend_kind = profile.backend_kind.clone();
                            let skill_key = skill.key.clone();
                            let parent_for_toggle = parent_for_rows.clone();
                            let manager_for_toggle = manager_for_rows.clone();
                            let status_label_for_toggle = status_label.clone();
                            let refresh_handle_for_toggle = refresh_handle_for_rows.clone();
                            toggle.connect_toggled(move |btn| {
                                let enabled = btn.is_active();
                                let start_toggle: Rc<dyn Fn()> = {
                                    let manager_for_toggle = manager_for_toggle.clone();
                                    let status_label_for_toggle = status_label_for_toggle.clone();
                                    let refresh_handle_for_toggle =
                                        refresh_handle_for_toggle.clone();
                                    let skill_key_for_thread = skill_key.clone();
                                    let profile_backend_kind_for_thread =
                                        profile_backend_kind.clone();
                                    let btn = btn.clone();
                                    Rc::new(move || {
                                        status_label_for_toggle
                                            .set_text("Updating skill assignment for profile...");
                                        let client =
                                            manager_for_toggle.running_client_for_profile(profile_id);
                                        let (tx, rx) = mpsc::channel::<Result<(), String>>();
                                        let skill_key_for_thread =
                                            skill_key_for_thread.clone();
                                        let profile_backend_kind_for_thread =
                                            profile_backend_kind_for_thread.clone();
                                        thread::spawn(move || {
                                            let result = (|| -> Result<(), String> {
                                                let background_db = AppDb::open_default();
                                                set_profile_assigned(
                                                    background_db.as_ref(),
                                                    profile_id,
                                                    PolicyKind::Skill,
                                                    &skill_key_for_thread,
                                                    enabled,
                                                )?;
                                                let profile = background_db
                                                    .get_codex_profile(profile_id)
                                                    .map_err(|err| err.to_string())?
                                                    .ok_or_else(|| {
                                                        format!(
                                                            "profile {} not found",
                                                            profile_id
                                                        )
                                                    })?;
                                                if !supports_skill_assignment_for_backend(
                                                    &profile_backend_kind_for_thread,
                                                ) {
                                                    return Err(
                                                        "Skill assignment is not supported for this runtime profile."
                                                            .to_string(),
                                                    );
                                                }
                                                let catalog = load_catalog(background_db.as_ref());
                                                let skill = catalog
                                                    .skills
                                                    .into_iter()
                                                    .find(|item| item.key == skill_key_for_thread)
                                                    .ok_or_else(|| {
                                                        format!(
                                                            "skill not found in catalog: {}",
                                                            skill_key_for_thread
                                                        )
                                                    })?;
                                                let Some(client) = client else {
                                                    return Err(
                                                        "Runtime profile is not running for skill update."
                                                            .to_string(),
                                                    );
                                                };
                                                set_skill_for_profile(
                                                    &profile,
                                                    &skill,
                                                    enabled,
                                                    client,
                                                )
                                            })();
                                            let _ = tx.send(result);
                                        });
                                        let status_label_for_toggle =
                                            status_label_for_toggle.clone();
                                        let refresh_handle_for_toggle =
                                            refresh_handle_for_toggle.clone();
                                        let btn = btn.clone();
                                        gtk::glib::timeout_add_local(
                                            Duration::from_millis(60),
                                            move || match rx.try_recv() {
                                                Ok(Ok(())) => {
                                                    status_label_for_toggle
                                                        .set_text("Skill assignment updated.");
                                                    if let Some(refresh) =
                                                        refresh_handle_for_toggle.borrow().as_ref()
                                                    {
                                                        refresh();
                                                    }
                                                    gtk::glib::ControlFlow::Break
                                                }
                                                Ok(Err(err)) => {
                                                    status_label_for_toggle.set_text(&format!(
                                                        "Skill update failed: {err}"
                                                    ));
                                                    btn.set_active(!enabled);
                                                    gtk::glib::ControlFlow::Break
                                                }
                                                Err(mpsc::TryRecvError::Empty) => {
                                                    gtk::glib::ControlFlow::Continue
                                                }
                                                Err(mpsc::TryRecvError::Disconnected) => {
                                                    gtk::glib::ControlFlow::Break
                                                }
                                            },
                                        );
                                    })
                                };
                                let status_label_for_cancel = status_label_for_toggle.clone();
                                let btn_for_cancel = btn.clone();
                                run_with_opencode_reload_guard(
                                    Some(&parent_for_toggle),
                                    manager_for_toggle.clone(),
                                    profile_id,
                                    status_label_for_toggle.clone(),
                                    start_toggle,
                                    Rc::new(move || {
                                        status_label_for_cancel
                                            .set_text("Skill update canceled.");
                                        btn_for_cancel.set_active(!enabled);
                                    }),
                                );
                            });

                            chips.insert(&toggle, -1);
                        }

                        row.append(&chips);

                        let skill_key = skill.key.clone();
                        let skill_snapshot = skill.clone();
                        let profiles_snapshot = data.profiles.clone();
                        let assignments_snapshot = data.assignments.clone();
                        let manager_for_remove = manager_for_rows.clone();
                        let status_label_for_remove = status_label.clone();
                        let refresh_handle_for_remove = refresh_handle_for_rows.clone();
                        remove.connect_clicked(move |_| {
                            status_label_for_remove.set_text("Removing skill from catalog...");
                            let skill_key_for_thread = skill_key.clone();
                            let skill_snapshot_for_thread = skill_snapshot.clone();
                            let profiles_snapshot_for_thread = profiles_snapshot.clone();
                            let assignments_snapshot_for_thread = assignments_snapshot.clone();
                            let clients = profiles_snapshot_for_thread
                                .iter()
                                .filter_map(|profile| {
                                    manager_for_remove
                                        .running_client_for_profile(profile.id)
                                        .map(|client| (profile.id, client))
                                })
                                .collect::<HashMap<_, _>>();
                            let (tx, rx) = mpsc::channel::<Result<(), String>>();
                            thread::spawn(move || {
                                let result = (|| -> Result<(), String> {
                                    let background_db = AppDb::open_default();
                                    for profile in profiles_snapshot_for_thread {
                                        let is_assigned = assignments_snapshot_for_thread
                                            .get(&profile.id)
                                            .map(|items| {
                                                items.skills.contains(&skill_key_for_thread)
                                            })
                                            .unwrap_or(false);
                                        if !is_assigned {
                                            continue;
                                        }
                                        set_profile_assigned(
                                            background_db.as_ref(),
                                            profile.id,
                                            PolicyKind::Skill,
                                            &skill_key_for_thread,
                                            false,
                                        )?;
                                        if let Some(client) = clients.get(&profile.id).cloned() {
                                            let profile_rec = background_db
                                                .get_codex_profile(profile.id)
                                                .map_err(|err| err.to_string())?
                                                .ok_or_else(|| {
                                                    format!("profile {} not found", profile.id)
                                                })?;
                                            if supports_skill_assignment_for_backend(
                                                &profile_rec.backend_kind,
                                            ) {
                                                set_skill_for_profile(
                                                    &profile_rec,
                                                    &skill_snapshot_for_thread,
                                                    false,
                                                    client,
                                                )?;
                                            }
                                        }
                                    }
                                    remove_catalog_skill(
                                        background_db.as_ref(),
                                        &skill_key_for_thread,
                                    )
                                })();
                                let _ = tx.send(result);
                            });
                            let status_label_for_remove = status_label_for_remove.clone();
                            let refresh_handle_for_remove = refresh_handle_for_remove.clone();
                            gtk::glib::timeout_add_local(
                                Duration::from_millis(60),
                                move || match rx.try_recv() {
                                    Ok(Ok(())) => {
                                        status_label_for_remove.set_text("Skill removed.");
                                        if let Some(refresh) =
                                            refresh_handle_for_remove.borrow().as_ref()
                                        {
                                            refresh();
                                        }
                                        gtk::glib::ControlFlow::Break
                                    }
                                    Ok(Err(err)) => {
                                        status_label_for_remove
                                            .set_text(&format!("Skill removal failed: {err}"));
                                        gtk::glib::ControlFlow::Break
                                    }
                                    Err(mpsc::TryRecvError::Empty) => {
                                        gtk::glib::ControlFlow::Continue
                                    }
                                    Err(mpsc::TryRecvError::Disconnected) => {
                                        gtk::glib::ControlFlow::Break
                                    }
                                },
                            );
                        });

                        skills_box.append(&row);
                    }

                    let auth_map = mcp_auth_map(&data.mcp_status);

                    if data.catalog.mcps.is_empty() {
                        let empty = gtk::Label::new(Some("No MCP servers in catalog yet."));
                        empty.set_xalign(0.0);
                        empty.add_css_class("dim-label");
                        mcps_box.append(&empty);
                    }

                    for mcp in data.catalog.mcps.clone() {
                        let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
                        row.add_css_class("actions-command-card");

                        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                        let name = gtk::Label::new(Some(&mcp.name));
                        name.set_xalign(0.0);
                        name.set_hexpand(true);
                        name.add_css_class("actions-command-title");
                        top.append(&name);

                        let kind = gtk::Label::new(Some("MCP"));
                        kind.add_css_class("actions-run-status");
                        top.append(&kind);

                        if data.runtime_profile_id > 0 {
                            if let Some((auth_label, _)) =
                                auth_map.get(&normalize_mcp_server_name(&mcp.name)).cloned()
                            {
                                let auth = gtk::Label::new(Some(&auth_label));
                                auth.add_css_class("actions-run-status");
                                top.append(&auth);
                            }
                        }

                        let remove = gtk::Button::with_label("Remove");
                        remove.add_css_class("app-flat-button");
                        remove.add_css_class("actions-run-button");
                        top.append(&remove);
                        row.append(&top);

                        if !mcp.description.trim().is_empty() {
                            let desc = gtk::Label::new(Some(&mcp.description));
                            desc.set_xalign(0.0);
                            desc.set_wrap(true);
                            desc.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                            desc.add_css_class("actions-command-text");
                            row.append(&desc);
                        }

                        let chips = gtk::FlowBox::builder()
                            .selection_mode(gtk::SelectionMode::None)
                            .max_children_per_line(8)
                            .row_spacing(6)
                            .column_spacing(6)
                            .build();

                        for profile in &data.profiles {
                            let profile_assignments = data
                                .assignments
                                .get(&profile.id)
                                .cloned()
                                .unwrap_or_default();
                            let enabled = profile_assignments.mcps.contains(&mcp.key);
                            let toggle = gtk::CheckButton::with_label(&profile.name);
                            toggle.set_active(enabled);
                            let running = profile.is_running();
                            toggle.set_sensitive(running);
                            if !running {
                                toggle.set_tooltip_text(Some(
                                    "Profile is stopped. Start it to change assignments.",
                                ));
                            }

                            let profile_id = profile.id;
                            let mcp_key = mcp.key.clone();
                            let parent_for_toggle = parent_for_rows.clone();
                            let manager_for_toggle = manager_for_rows.clone();
                            let status_label_for_toggle = status_label.clone();
                            let refresh_handle_for_toggle = refresh_handle_for_rows.clone();
                            toggle.connect_toggled(move |btn| {
                                let enabled = btn.is_active();
                                let start_toggle: Rc<dyn Fn()> = {
                                    let manager_for_toggle = manager_for_toggle.clone();
                                    let status_label_for_toggle = status_label_for_toggle.clone();
                                    let refresh_handle_for_toggle =
                                        refresh_handle_for_toggle.clone();
                                    let mcp_key_for_thread = mcp_key.clone();
                                    let btn = btn.clone();
                                    Rc::new(move || {
                                        status_label_for_toggle
                                            .set_text("Updating MCP assignment for profile...");
                                        let client =
                                            manager_for_toggle.running_client_for_profile(profile_id);
                                        let (tx, rx) = mpsc::channel::<Result<(), String>>();
                                        let mcp_key_for_thread = mcp_key_for_thread.clone();
                                        thread::spawn(move || {
                                            let result = (|| -> Result<(), String> {
                                                let background_db = AppDb::open_default();
                                                set_profile_assigned(
                                                    background_db.as_ref(),
                                                    profile_id,
                                                    PolicyKind::Mcp,
                                                    &mcp_key_for_thread,
                                                    enabled,
                                                )?;
                                                let mcp = load_catalog(background_db.as_ref())
                                                    .mcps
                                                    .into_iter()
                                                    .find(|item| item.key == mcp_key_for_thread)
                                                    .ok_or_else(|| {
                                                        format!(
                                                            "mcp not found in catalog: {}",
                                                            mcp_key_for_thread
                                                        )
                                                    })?;
                                                let Some(client) = client else {
                                                    return Err(
                                                        "Runtime profile is not running for MCP update."
                                                            .to_string(),
                                                    );
                                                };
                                                set_mcp_for_profile(&mcp, enabled, client)
                                            })();
                                            let _ = tx.send(result);
                                        });
                                        let status_label_for_toggle =
                                            status_label_for_toggle.clone();
                                        let refresh_handle_for_toggle =
                                            refresh_handle_for_toggle.clone();
                                        let btn = btn.clone();
                                        gtk::glib::timeout_add_local(
                                            Duration::from_millis(60),
                                            move || match rx.try_recv() {
                                                Ok(Ok(())) => {
                                                    status_label_for_toggle
                                                        .set_text("MCP assignment updated.");
                                                    if let Some(refresh) =
                                                        refresh_handle_for_toggle.borrow().as_ref()
                                                    {
                                                        refresh();
                                                    }
                                                    gtk::glib::ControlFlow::Break
                                                }
                                                Ok(Err(err)) => {
                                                    status_label_for_toggle.set_text(&format!(
                                                        "MCP update failed: {err}"
                                                    ));
                                                    btn.set_active(!enabled);
                                                    gtk::glib::ControlFlow::Break
                                                }
                                                Err(mpsc::TryRecvError::Empty) => {
                                                    gtk::glib::ControlFlow::Continue
                                                }
                                                Err(mpsc::TryRecvError::Disconnected) => {
                                                    gtk::glib::ControlFlow::Break
                                                }
                                            },
                                        );
                                    })
                                };
                                let status_label_for_cancel = status_label_for_toggle.clone();
                                let btn_for_cancel = btn.clone();
                                run_with_opencode_reload_guard(
                                    Some(&parent_for_toggle),
                                    manager_for_toggle.clone(),
                                    profile_id,
                                    status_label_for_toggle.clone(),
                                    start_toggle,
                                    Rc::new(move || {
                                        status_label_for_cancel
                                            .set_text("MCP update canceled.");
                                        btn_for_cancel.set_active(!enabled);
                                    }),
                                );
                            });

                            chips.insert(&toggle, -1);
                        }

                        row.append(&chips);

                        let mcp_key = mcp.key.clone();
                        let mcp_snapshot = mcp.clone();
                        let profiles_snapshot = data.profiles.clone();
                        let assignments_snapshot = data.assignments.clone();
                        let manager_for_remove = manager_for_rows.clone();
                        let status_label_for_remove = status_label.clone();
                        let refresh_handle_for_remove = refresh_handle_for_rows.clone();
                        remove.connect_clicked(move |_| {
                            status_label_for_remove.set_text("Removing MCP from catalog...");
                            let mcp_key_for_thread = mcp_key.clone();
                            let mcp_snapshot_for_thread = mcp_snapshot.clone();
                            let profiles_snapshot_for_thread = profiles_snapshot.clone();
                            let assignments_snapshot_for_thread = assignments_snapshot.clone();
                            let clients = profiles_snapshot_for_thread
                                .iter()
                                .filter_map(|profile| {
                                    manager_for_remove
                                        .running_client_for_profile(profile.id)
                                        .map(|client| (profile.id, client))
                                })
                                .collect::<HashMap<_, _>>();
                            let (tx, rx) = mpsc::channel::<Result<(), String>>();
                            thread::spawn(move || {
                                let result = (|| -> Result<(), String> {
                                    let background_db = AppDb::open_default();
                                    for profile in profiles_snapshot_for_thread {
                                        let is_assigned = assignments_snapshot_for_thread
                                            .get(&profile.id)
                                            .map(|items| items.mcps.contains(&mcp_key_for_thread))
                                            .unwrap_or(false);
                                        if !is_assigned {
                                            continue;
                                        }
                                        set_profile_assigned(
                                            background_db.as_ref(),
                                            profile.id,
                                            PolicyKind::Mcp,
                                            &mcp_key_for_thread,
                                            false,
                                        )?;
                                        if let Some(client) = clients.get(&profile.id).cloned() {
                                            set_mcp_for_profile(
                                                &mcp_snapshot_for_thread,
                                                false,
                                                client,
                                            )?;
                                        }
                                    }
                                    remove_catalog_mcp(background_db.as_ref(), &mcp_key_for_thread)
                                })();
                                let _ = tx.send(result);
                            });
                            let status_label_for_remove = status_label_for_remove.clone();
                            let refresh_handle_for_remove = refresh_handle_for_remove.clone();
                            gtk::glib::timeout_add_local(
                                Duration::from_millis(60),
                                move || match rx.try_recv() {
                                    Ok(Ok(())) => {
                                        status_label_for_remove.set_text("MCP removed.");
                                        if let Some(refresh) =
                                            refresh_handle_for_remove.borrow().as_ref()
                                        {
                                            refresh();
                                        }
                                        gtk::glib::ControlFlow::Break
                                    }
                                    Ok(Err(err)) => {
                                        status_label_for_remove
                                            .set_text(&format!("MCP removal failed: {err}"));
                                        gtk::glib::ControlFlow::Break
                                    }
                                    Err(mpsc::TryRecvError::Empty) => {
                                        gtk::glib::ControlFlow::Continue
                                    }
                                    Err(mpsc::TryRecvError::Disconnected) => {
                                        gtk::glib::ControlFlow::Break
                                    }
                                },
                            );
                        });

                        mcps_box.append(&row);
                    }

                    if let Some(warning) = data.warning {
                        status_label.set_text(&warning);
                    } else {
                        status_label.set_text("");
                    }
                    gtk::glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    status_label.set_text("Failed to load Skills/MCP catalog.");
                    gtk::glib::ControlFlow::Break
                }
            });
        })
    };
    refresh_handle.replace(Some(refresh_fn.clone()));

    {
        let parent = parent.clone();
        let db = db.clone();
        let manager = manager.clone();
        let status_label = status_label.clone();
        let refresh_handle = refresh_handle.clone();
        add_skill_button.connect_clicked(move |_| {
            let on_saved: Rc<dyn Fn()> = {
                let refresh_handle = refresh_handle.clone();
                Rc::new(move || {
                    if let Some(refresh) = refresh_handle.borrow().as_ref() {
                        refresh();
                    }
                })
            };
            open_add_skill_dialog(
                &parent,
                db.clone(),
                manager.clone(),
                status_label.clone(),
                on_saved,
            );
        });
    }

    {
        let parent = parent.clone();
        let db = db.clone();
        let manager = manager.clone();
        let status_label = status_label.clone();
        let refresh_handle = refresh_handle.clone();
        add_mcp_button.connect_clicked(move |_| {
            let on_saved: Rc<dyn Fn()> = {
                let refresh_handle = refresh_handle.clone();
                Rc::new(move || {
                    if let Some(refresh) = refresh_handle.borrow().as_ref() {
                        refresh();
                    }
                })
            };
            open_add_mcp_server_dialog(
                &parent,
                db.clone(),
                manager.clone(),
                status_label.clone(),
                on_saved,
            );
        });
    }

    refresh_fn();
    root
}
