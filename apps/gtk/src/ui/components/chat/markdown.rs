use gtk::prelude::*;

const INLINE_CODE_BACKGROUND: &str = "#262b33";
const INLINE_CODE_FOREGROUND: &str = "#dbe4ee";
const INLINE_CODE_BACKGROUND_ALPHA: &str = "55%";
const CODE_BLOCK_BACKGROUND: &str = "#1f242b";
const CODE_BLOCK_FOREGROUND: &str = "#dbe4ee";
const CODE_BLOCK_BACKGROUND_ALPHA: &str = "60%";
// Keep blank markdown lines selectable without collapsing them into a thin seam.
const COMPACT_BLANK_LINE_MARKUP: &str = "\u{00A0}";

pub(super) struct StreamingMarkdownBlocks {
    pub(super) finalized_blocks: Vec<String>,
    pub(super) tail_block: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StreamingBlockKind {
    Paragraph,
    List,
    Heading,
    CodeFence,
}

fn escape_markup(text: &str) -> String {
    gtk::glib::markup_escape_text(text).to_string()
}

fn href_is_safe(href: &str) -> bool {
    !href.is_empty()
        && !href.contains(['"', '\'', '<', '>', '\\', '\n', '\r', '\t'])
        && (href.starts_with('/')
            || href.starts_with("file://")
            || href.starts_with("http://")
            || href.starts_with("https://"))
}

fn looks_like_unfenced_diff(text: &str) -> bool {
    if text.contains("\ndiff --git ")
        || text.contains("\nindex ")
        || text.contains("\n--- ")
        || text.contains("\n+++ ")
        || text.starts_with("diff --git ")
        || text.starts_with("index ")
        || text.starts_with("--- ")
        || text.starts_with("+++ ")
        || text.contains("\n@@ ")
        || text.starts_with("@@ ")
    {
        return true;
    }
    let mut has_hunk_header = false;
    let mut change_line_count = 0usize;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("diff --git ")
            || trimmed.starts_with("index ")
            || trimmed.starts_with("--- ")
            || trimmed.starts_with("+++ ")
        {
            return true;
        }
        if trimmed.starts_with("@@ ") {
            has_hunk_header = true;
            continue;
        }
        if trimmed.starts_with('+') || trimmed.starts_with('-') {
            change_line_count += 1;
            if has_hunk_header && change_line_count >= 2 {
                return true;
            }
        }
    }
    false
}

fn parse_ordered_list_item(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut idx = 0usize;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx == 0 || idx + 1 >= bytes.len() {
        return None;
    }
    if bytes[idx] == b'.' && bytes[idx + 1] == b' ' {
        Some((&line[..idx], &line[(idx + 2)..]))
    } else {
        None
    }
}

fn is_list_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("- ")
        || trimmed.starts_with("* ")
        || trimmed.starts_with("• ")
        || parse_ordered_list_item(trimmed).is_some()
}

fn looks_like_list_block(text: &str) -> bool {
    text.lines()
        .filter(|line| is_list_line(line))
        .take(2)
        .count()
        >= 2
}

fn finish_streaming_block(current: &mut String, blocks: &mut Vec<String>) {
    let block = current.trim_end_matches('\n').trim().to_string();
    current.clear();
    if !block.is_empty() {
        blocks.push(block);
    }
}

pub(super) fn split_streaming_blocks(text: &str) -> StreamingMarkdownBlocks {
    let mut finalized_blocks = Vec::new();
    let mut current = String::new();
    let mut current_kind: Option<StreamingBlockKind> = None;
    let mut in_code_fence = false;

    for raw_line in text.lines() {
        let line = raw_line.trim_end();
        let stripped = line.trim_start();

        if in_code_fence {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
            if stripped.starts_with("```") {
                in_code_fence = false;
                current_kind = None;
                finish_streaming_block(&mut current, &mut finalized_blocks);
            }
            continue;
        }

        if stripped.starts_with("```") {
            finish_streaming_block(&mut current, &mut finalized_blocks);
            current.push_str(line);
            current_kind = Some(StreamingBlockKind::CodeFence);
            in_code_fence = true;
            continue;
        }

        if stripped.is_empty() {
            current_kind = None;
            finish_streaming_block(&mut current, &mut finalized_blocks);
            continue;
        }

        let next_kind = if stripped.starts_with('#') {
            StreamingBlockKind::Heading
        } else if is_list_line(stripped) {
            StreamingBlockKind::List
        } else {
            StreamingBlockKind::Paragraph
        };

        let should_break = match current_kind {
            None => false,
            Some(StreamingBlockKind::Heading) => true,
            Some(StreamingBlockKind::Paragraph) => next_kind != StreamingBlockKind::Paragraph,
            Some(StreamingBlockKind::List) => next_kind != StreamingBlockKind::List,
            Some(StreamingBlockKind::CodeFence) => false,
        };
        if should_break {
            finish_streaming_block(&mut current, &mut finalized_blocks);
        }

        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);

        if next_kind == StreamingBlockKind::Heading {
            current_kind = None;
            finish_streaming_block(&mut current, &mut finalized_blocks);
        } else {
            current_kind = Some(next_kind);
        }
    }

    let tail_block = current.trim_end_matches('\n').trim().to_string();
    StreamingMarkdownBlocks {
        finalized_blocks,
        tail_block,
    }
}

fn parse_link_line_col_suffix(href: &str) -> Option<(u32, Option<u32>)> {
    let mut parts = href.rsplitn(3, ':');
    let last = parts.next()?;
    let second = parts.next()?;
    if let (Ok(col), Ok(line)) = (last.parse::<u32>(), second.parse::<u32>()) {
        return Some((line, Some(col)));
    }
    if let Ok(line) = last.parse::<u32>() {
        return Some((line, None));
    }
    None
}

fn render_inline_markdown(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    while i < chars.len() {
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
                out.push_str("<span background=\"");
                out.push_str(INLINE_CODE_BACKGROUND);
                out.push_str("\" bgalpha=\"");
                out.push_str(INLINE_CODE_BACKGROUND_ALPHA);
                out.push_str("\" foreground=\"");
                out.push_str(INLINE_CODE_FOREGROUND);
                out.push_str("\"><tt> ");
                out.push_str(&escape_markup(inner.trim()));
                out.push_str(" </tt></span>");
                i = end + 1;
                continue;
            }
        }
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
                        let href_trimmed = href.trim();
                        if !href_is_safe(href_trimmed) {
                            let original: String = chars[i..=url_end].iter().collect();
                            out.push_str(&escape_markup(&original));
                            i = url_end + 1;
                            continue;
                        }
                        let normalized_href = if href_trimmed.starts_with('/') {
                            gtk::glib::filename_to_uri(std::path::Path::new(href_trimmed), None)
                                .map(|uri| uri.to_string())
                                .unwrap_or_else(|_| format!("file://{href_trimmed}"))
                        } else {
                            href_trimmed.to_string()
                        };
                        out.push_str("<a href=\"");
                        out.push_str(&escape_markup(&normalized_href));
                        out.push_str("\">");
                        out.push_str(&escape_markup(&label));
                        if let Some((line, maybe_col)) = parse_link_line_col_suffix(href_trimmed) {
                            let location = if let Some(col) = maybe_col {
                                format!(" L{line}:{col}")
                            } else {
                                format!(" L{line}")
                            };
                            out.push_str("<span foreground=\"#9AA4B1\" size=\"x-small\">");
                            out.push_str(&escape_markup(&location));
                            out.push_str("</span>");
                        }
                        out.push_str("</a>");
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
        out.push_str(&escape_markup(&chars[i].to_string()));
        i += 1;
    }
    out
}

fn markdown_to_pango(text: &str) -> String {
    let mut out = String::new();
    let mut in_code_block = false;
    let mut in_list = false;
    let mut emitted_blank_gap = false;
    for line in text.lines() {
        let trimmed = line.trim_end();
        let stripped = trimmed.trim_start();
        if trimmed.trim_start().starts_with("```") {
            if in_code_block {
                out.push_str("</tt></span>\n");
                in_code_block = false;
            } else {
                out.push_str("<span background=\"");
                out.push_str(CODE_BLOCK_BACKGROUND);
                out.push_str("\" bgalpha=\"");
                out.push_str(CODE_BLOCK_BACKGROUND_ALPHA);
                out.push_str("\" foreground=\"");
                out.push_str(CODE_BLOCK_FOREGROUND);
                out.push_str("\"><tt>");
                in_code_block = true;
            }
            emitted_blank_gap = false;
            continue;
        }
        if in_code_block {
            out.push_str(&escape_markup(trimmed));
            out.push('\n');
            emitted_blank_gap = false;
            continue;
        }
        if stripped.is_empty() {
            if !in_list && !emitted_blank_gap {
                out.push_str(COMPACT_BLANK_LINE_MARKUP);
                out.push('\n');
                emitted_blank_gap = true;
            }
            continue;
        }
        emitted_blank_gap = false;
        if let Some(rest) = stripped.strip_prefix("### ") {
            in_list = false;
            out.push_str("<span weight=\"700\">");
            out.push_str(&render_inline_markdown(rest));
            out.push_str("</span>\n");
            continue;
        }
        if let Some(rest) = stripped.strip_prefix("## ") {
            in_list = false;
            out.push_str("<span weight=\"700\" size=\"larger\">");
            out.push_str(&render_inline_markdown(rest));
            out.push_str("</span>\n");
            continue;
        }
        if let Some(rest) = stripped.strip_prefix("# ") {
            in_list = false;
            out.push_str("<span weight=\"700\" size=\"x-large\">");
            out.push_str(&render_inline_markdown(rest));
            out.push_str("</span>\n");
            continue;
        }
        if let Some(rest) = stripped
            .strip_prefix("- ")
            .or_else(|| stripped.strip_prefix("* "))
            .or_else(|| stripped.strip_prefix("• "))
        {
            in_list = true;
            out.push_str("   • ");
            out.push_str(&render_inline_markdown(rest));
            out.push('\n');
            continue;
        }
        if let Some((ordinal, rest)) = parse_ordered_list_item(stripped) {
            in_list = true;
            out.push_str("   ");
            out.push_str(&escape_markup(ordinal));
            out.push_str(". ");
            out.push_str(&render_inline_markdown(rest));
            out.push('\n');
            continue;
        }
        in_list = false;
        out.push_str(&render_inline_markdown(trimmed));
        out.push('\n');
    }
    if in_code_block {
        out.push_str("</tt></span>\n");
    }
    out.trim_end().to_string()
}

pub(super) fn set_markdown(label: &gtk::Label, text: &str) {
    if looks_like_list_block(text) {
        label.add_css_class("chat-turn-text-list-compact");
    } else {
        label.remove_css_class("chat-turn-text-list-compact");
    }
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
    label.set_attributes(None);
    label.set_use_markup(true);
    label.set_markup(&markup);
}

#[cfg(test)]
mod tests {
    use super::{
        CODE_BLOCK_BACKGROUND, CODE_BLOCK_BACKGROUND_ALPHA, COMPACT_BLANK_LINE_MARKUP,
        INLINE_CODE_BACKGROUND, INLINE_CODE_BACKGROUND_ALPHA, INLINE_CODE_FOREGROUND,
        looks_like_list_block, looks_like_unfenced_diff, markdown_to_pango,
    };

    #[test]
    fn renders_file_links_as_valid_pango_markup() {
        let text = "Implemented both requested fixes:\n\n- [assistant_turn.rs](/workspace/project/src/ui/components/chat/history/assistant_turn.rs:259)\n- [history.rs](/workspace/project/src/ui/components/chat/history.rs:183)\n\nValidation:\n- `cargo check` passes.";
        let markup = markdown_to_pango(text);
        assert!(markup.contains("<a href=\"file:///workspace/project/src/ui/components/chat/history/assistant_turn.rs:259\">"));
        assert!(markup.contains(INLINE_CODE_BACKGROUND));
        assert!(markup.contains(INLINE_CODE_BACKGROUND_ALPHA));
        assert!(markup.contains(INLINE_CODE_FOREGROUND));
    }

    #[test]
    fn treats_inline_code_as_literal_before_link_parsing() {
        let text = r#"`[assistant_turn.rs](/tmp/file.rs:1)`"#;
        let markup = markdown_to_pango(text);
        assert!(!markup.contains("<a href="));
        assert!(markup.contains("[assistant_turn.rs](/tmp/file.rs:1)"));
    }

    #[test]
    fn leaves_unsafe_link_targets_as_plain_text() {
        let text = r#"[bad](file:///tmp/file\"oops)"#;
        let markup = markdown_to_pango(text);
        assert!(!markup.contains("<a href="));
        assert!(markup.contains("[bad](file:///tmp/file\\&quot;oops)"));
    }

    #[test]
    fn styles_fenced_code_blocks_with_dedicated_palette() {
        let text = "```rust\nlet value = 42;\n```";
        let markup = markdown_to_pango(text);
        assert!(markup.contains(CODE_BLOCK_BACKGROUND));
        assert!(markup.contains(CODE_BLOCK_BACKGROUND_ALPHA));
    }

    #[test]
    fn keeps_diff_lines_with_ampersands_parseable() {
        let text = "@@ -10,3 +10,3 @@\n+            super::message_render::force_scroll_to_bottom(&messages_scroll);";
        let markup = markdown_to_pango(text);
        assert!(markup.contains("&amp;messages_scroll"));
        assert!(gtk::pango::parse_markup(&markup, '\0').is_ok());
    }

    #[test]
    fn detects_unfenced_diff_blocks() {
        let text = "@@ -8,2 +8,9 @@\n+fn supports_link_markup() -> bool {\n-fn old_name() {}\n";
        assert!(looks_like_unfenced_diff(text));
    }

    #[test]
    fn indents_bulleted_and_ordered_list_items() {
        let text = "1. First item\n- Child bullet";
        let markup = markdown_to_pango(text);
        assert!(markup.contains("   1. First item"));
        assert!(markup.contains("   • Child bullet"));
    }

    #[test]
    fn detects_multi_line_list_blocks() {
        let text = "1. First item\n2. Second item";
        assert!(looks_like_list_block(text));
    }

    #[test]
    fn compacts_blank_lines_between_paragraphs() {
        let text = "Top line\n\n\nBottom line";
        let markup = markdown_to_pango(text);
        assert!(markup.contains(COMPACT_BLANK_LINE_MARKUP));
        assert_eq!(markup.matches(COMPACT_BLANK_LINE_MARKUP).count(), 1);
    }

    #[test]
    fn renders_file_link_line_badge() {
        let text = "[m](/tmp/main.rs:42:7)";
        let markup = markdown_to_pango(text);
        assert!(markup.contains("L42:7"));
    }
}
