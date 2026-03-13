use std::collections::HashSet;

pub(super) fn is_probably_file_write_command(command: &str) -> bool {
    let cmd = command.to_ascii_lowercase();
    let direct_write_markers = [
        "apply_patch",
        "sed -i",
        "perl -pi",
        "truncate ",
        "touch ",
        "mkdir ",
        "install ",
        "cp ",
        "mv ",
        "rm ",
        "git apply",
        "git restore ",
        "git checkout --",
        "git clean -f",
    ];
    if direct_write_markers
        .iter()
        .any(|marker| cmd.contains(marker))
    {
        return true;
    }

    let redirection_markers = [" >> ", " > ", ">|", "1>>", "1>"];
    let has_file_redirection = redirection_markers
        .iter()
        .any(|marker| cmd.contains(marker));
    let here_doc_write = cmd.contains("cat >") || cmd.contains("cat>>");
    let tee_write = cmd.contains("| tee") || cmd.contains("|tee") || cmd.contains(" tee ");

    has_file_redirection || here_doc_write || tee_write
}

fn shell_like_tokens(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;

    for ch in input.chars() {
        match quote {
            Some(q) if ch == q => {
                quote = None;
            }
            Some(_) => current.push(ch),
            None => match ch {
                '\'' | '"' => quote = Some(ch),
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        out.push(current.clone());
                        current.clear();
                    }
                }
                _ => current.push(ch),
            },
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn normalize_command_path_token(raw: &str) -> Option<String> {
    let mut token = raw
        .trim()
        .trim_end_matches(';')
        .trim_end_matches(',')
        .to_string();
    while token.starts_with('(') || token.starts_with('[') || token.starts_with('{') {
        token.remove(0);
    }
    while token.ends_with(')') || token.ends_with(']') || token.ends_with('}') {
        token.pop();
    }
    let token = token
        .trim()
        .trim_matches('\'')
        .trim_matches('"')
        .to_string();
    if token.is_empty()
        || token == "-"
        || token == "/dev/null"
        || token.starts_with('-')
        || token.contains('*')
        || token.contains('?')
        || token.contains("$(")
        || token.contains("${")
    {
        return None;
    }
    Some(token)
}

pub(super) fn extract_write_paths_from_command(command: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let push_path = |candidate: &str, seen: &mut HashSet<String>, out: &mut Vec<String>| {
        let Some(path) = normalize_command_path_token(candidate) else {
            return;
        };
        if seen.insert(path.clone()) {
            out.push(path);
        }
    };

    for line in command.lines() {
        let trimmed = line.trim();
        if let Some(path) = trimmed.strip_prefix("*** Add File: ") {
            push_path(path, &mut seen, &mut out);
        }
        if let Some(path) = trimmed.strip_prefix("*** Update File: ") {
            push_path(path, &mut seen, &mut out);
        }
        if let Some(path) = trimmed.strip_prefix("*** Delete File: ") {
            push_path(path, &mut seen, &mut out);
        }
        if let Some(path) = trimmed.strip_prefix("*** Move to: ") {
            push_path(path, &mut seen, &mut out);
        }

        let outer_tokens = shell_like_tokens(trimmed);
        let mut snippets = vec![trimmed.to_string()];
        for (idx, token) in outer_tokens.iter().enumerate() {
            if (token == "-c" || token == "-lc" || token == "-ic")
                && outer_tokens.get(idx + 1).is_some()
            {
                if let Some(payload) = outer_tokens.get(idx + 1) {
                    snippets.push(payload.clone());
                }
            }
        }

        for snippet in snippets {
            let tokens = shell_like_tokens(&snippet);
            if tokens.is_empty() {
                continue;
            }

            for (idx, token) in tokens.iter().enumerate() {
                if token == ">" || token == ">>" || token == "1>" || token == "1>>" {
                    if let Some(next) = tokens.get(idx + 1) {
                        push_path(next, &mut seen, &mut out);
                    }
                    continue;
                }
                if let Some(rest) = token.strip_prefix(">>").or_else(|| token.strip_prefix('>')) {
                    if !rest.is_empty() {
                        push_path(rest, &mut seen, &mut out);
                    }
                }
                if token == "tee" || token.ends_with("/tee") {
                    for next in tokens.iter().skip(idx + 1) {
                        if next.starts_with('-') || next == "|" {
                            continue;
                        }
                        push_path(next, &mut seen, &mut out);
                        break;
                    }
                }
                if token == "cp" || token == "mv" || token == "install" {
                    if let Some(last) = tokens.last() {
                        push_path(last, &mut seen, &mut out);
                    }
                }
                if token == "touch" || token == "truncate" || token == "rm" || token == "mkdir" {
                    for next in tokens.iter().skip(idx + 1) {
                        if next.starts_with('-') {
                            continue;
                        }
                        push_path(next, &mut seen, &mut out);
                    }
                }
                if token == "sed" {
                    let mut saw_inplace = false;
                    for next in tokens.iter().skip(idx + 1) {
                        if !saw_inplace {
                            if next == "-i" || next.starts_with("-i") {
                                saw_inplace = true;
                            }
                            continue;
                        }
                        if next.starts_with('-') {
                            continue;
                        }
                        push_path(next, &mut seen, &mut out);
                    }
                }
                if token == "git" && tokens.get(idx + 1).is_some() {
                    let sub = tokens[idx + 1].as_str();
                    if sub == "restore" || sub == "checkout" {
                        for next in tokens.iter().skip(idx + 2) {
                            if next == "--" || next.starts_with('-') {
                                continue;
                            }
                            push_path(next, &mut seen, &mut out);
                        }
                    }
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{extract_write_paths_from_command, is_probably_file_write_command};

    #[test]
    fn detects_write_like_commands() {
        assert!(is_probably_file_write_command("echo hi > README.md"));
        assert!(is_probably_file_write_command(
            "cat >> notes.txt <<'EOF'\nline\nEOF"
        ));
        assert!(is_probably_file_write_command("git apply patch.diff"));
        assert!(!is_probably_file_write_command("rg --files | head -20"));
    }

    #[test]
    fn extracts_write_paths_from_common_shell_forms() {
        let cmd = "/usr/bin/bash -lc \"echo hi > README.md && tee logs/output.txt < input\"";
        let paths = extract_write_paths_from_command(cmd);
        assert!(paths.contains(&"README.md".to_string()));
        assert!(paths.contains(&"logs/output.txt".to_string()));
    }

    #[test]
    fn extracts_paths_from_apply_patch_headers() {
        let cmd =
            "*** Add File: src/new.rs\n*** Update File: src/main.rs\n*** Move to: src/moved.rs";
        let paths = extract_write_paths_from_command(cmd);
        assert!(paths.contains(&"src/new.rs".to_string()));
        assert!(paths.contains(&"src/main.rs".to_string()));
        assert!(paths.contains(&"src/moved.rs".to_string()));
    }
}
