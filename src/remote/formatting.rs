#![allow(dead_code)]

use serde_json::Value;

const TELEGRAM_HARD_LIMIT: usize = 4096;

pub fn summarize_assistant_turn(raw_items_json: Option<&str>, assistant_text: &str) -> String {
    let mut command_count = 0usize;
    let mut file_edit_count = 0usize;
    let mut other_action_count = 0usize;

    if let Some(items) = parse_items(raw_items_json) {
        for item in items {
            let kind = item
                .get("kind")
                .and_then(Value::as_str)
                .or_else(|| item.get("type").and_then(Value::as_str))
                .unwrap_or_default()
                .to_ascii_lowercase();
            if kind.contains("command") {
                command_count += 1;
            } else if kind.contains("file") || kind.contains("patch") || kind.contains("edit") {
                file_edit_count += 1;
            } else if !kind.is_empty() {
                other_action_count += 1;
            }
        }
    }

    let mut lines = Vec::<String>::new();
    if !assistant_text.trim().is_empty() {
        lines.push(assistant_text.trim().to_string());
    }

    let total_actions = command_count + file_edit_count + other_action_count;
    if total_actions > 0 {
        let mut parts = Vec::<String>::new();
        if command_count > 0 {
            parts.push(format!("{command_count} commands"));
        }
        if file_edit_count > 0 {
            parts.push(format!("{file_edit_count} file edits"));
        }
        if other_action_count > 0 {
            parts.push(format!("{other_action_count} other actions"));
        }
        lines.push(format!(
            "{total_actions} actions run - {}",
            parts.join(", ")
        ));
    }

    lines.join("\n")
}

pub fn markdown_to_telegram_html(markdown: &str) -> String {
    if markdown.trim().is_empty() {
        return String::new();
    }
    let escaped = escape_html(markdown);
    let mut out = String::new();
    let mut in_code_block = false;
    for line in escaped.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            if in_code_block {
                out.push_str("</code></pre>\n");
            } else {
                out.push_str("<pre><code>");
            }
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        out.push_str(&line.replace("`", ""));
        out.push('\n');
    }
    if in_code_block {
        out.push_str("</code></pre>\n");
    }
    out.trim().to_string()
}

pub fn chunk_telegram_html(html: &str) -> Vec<String> {
    if html.len() <= TELEGRAM_HARD_LIMIT {
        return vec![html.to_string()];
    }

    let target = 3000usize;
    let mut chunks = Vec::<String>::new();
    let mut remaining = html.trim();
    while remaining.len() > TELEGRAM_HARD_LIMIT {
        let split_at = choose_split_index(remaining, target);
        let (head, tail) = remaining.split_at(split_at);
        chunks.push(head.trim().to_string());
        remaining = tail.trim_start();
    }
    if !remaining.is_empty() {
        chunks.push(remaining.to_string());
    }
    chunks
}

fn choose_split_index(text: &str, target: usize) -> usize {
    if text.len() <= target {
        return text.len();
    }
    let candidate = text
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|i| *i <= target);
    let mut last = 0usize;
    for i in candidate {
        last = i;
    }
    let hard_limit = last.max(1);

    for pattern in ["\n\n", "\n", ". ", " "] {
        if let Some(index) = text[..hard_limit].rfind(pattern) {
            return (index + pattern.len()).max(1);
        }
    }
    hard_limit
}

fn parse_items(raw_items_json: Option<&str>) -> Option<Vec<Value>> {
    let raw = raw_items_json?.trim();
    if raw.is_empty() {
        return None;
    }
    let value: Value = serde_json::from_str(raw).ok()?;
    value.as_array().cloned()
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
