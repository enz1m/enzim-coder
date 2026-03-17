use crate::backend::RuntimeClient;
use crate::data::AppDb;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone)]
pub(super) struct ThreadSyncOutcome {
    pub(super) thread: Value,
    pub(super) request_runtime_history_reload: bool,
}

#[derive(Clone)]
pub(super) struct ApplyRestoreWorkerOutcome {
    pub(super) result: crate::restore::RestoreApplyResult,
    pub(super) rollback_status: String,
    pub(super) thread_sync: Option<ThreadSyncOutcome>,
}

#[derive(Clone)]
pub(super) struct ChatRestoreWorkerOutcome {
    pub(super) status_text: String,
    pub(super) thread_sync: Option<ThreadSyncOutcome>,
}

#[derive(Clone)]
pub(super) struct UndoRestoreWorkerOutcome {
    pub(super) result: crate::restore::RestoreApplyResult,
    pub(super) chat_restore_status: String,
    pub(super) thread_sync: Option<ThreadSyncOutcome>,
}

fn read_thread_for_rollback(
    client: &Arc<RuntimeClient>,
    thread_id: &str,
    workspace_path: Option<&str>,
) -> Result<Value, String> {
    match client.thread_read(thread_id, true) {
        Ok(thread) => Ok(thread),
        Err(err) if err.contains("thread not loaded") || err.contains("no rollout found") => {
            if let Some(workspace_path) = workspace_path {
                match client.thread_resume(thread_id, Some(workspace_path), Some("gpt-5.3-codex")) {
                    Ok(_) => client.thread_read(thread_id, true),
                    Err(resume_err) => Err(format!(
                        "failed to materialize thread for rollback: {resume_err}"
                    )),
                }
            } else {
                Err(err)
            }
        }
        Err(err) => Err(err),
    }
}

fn build_trimmed_thread_view(
    thread: &Value,
    target_turn_id: &str,
) -> Result<(Value, usize, usize, usize), String> {
    let Some(turns) = thread.get("turns").and_then(Value::as_array) else {
        return Err("thread has no turns payload.".to_string());
    };

    let Some(target_index) = turns
        .iter()
        .position(|turn| turn.get("id").and_then(Value::as_str) == Some(target_turn_id))
    else {
        return Err("selected turn was not found in the remote thread.".to_string());
    };

    let mut indexed: Vec<(usize, i64)> = turns
        .iter()
        .enumerate()
        .map(|(idx, turn)| {
            let ts = super::parse_turn_timestamp_opt(turn).unwrap_or(idx as i64);
            (idx, ts)
        })
        .collect();
    indexed.sort_by_key(|(_, ts)| *ts);

    let chrono_target_pos = indexed
        .iter()
        .position(|(idx, _)| *idx == target_index)
        .unwrap_or(target_index);
    let rollback_count = indexed.len().saturating_sub(chrono_target_pos);

    let keep_ids: HashSet<&str> = indexed
        .iter()
        .take(chrono_target_pos)
        .filter_map(|(idx, _)| turns[*idx].get("id").and_then(Value::as_str))
        .collect();

    let mut trimmed = thread.clone();
    if let Some(obj) = trimmed.as_object_mut() {
        let filtered = turns
            .iter()
            .filter(|turn| {
                turn.get("id")
                    .and_then(Value::as_str)
                    .map(|id| keep_ids.contains(id))
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();
        obj.insert("turns".to_string(), Value::Array(filtered));
    }

    Ok((trimmed, rollback_count, target_index, chrono_target_pos))
}

fn rollback_thread_to_target_worker(
    client: &Arc<RuntimeClient>,
    workspace_path: Option<&str>,
    thread_id: &str,
    target_turn_id: &str,
) -> Result<(String, Option<ThreadSyncOutcome>), String> {
    if !client.capabilities().supports_rollback {
        return Ok((
            " • Chat trim skipped (runtime does not support rollback)".to_string(),
            None,
        ));
    }

    if client.backend_kind().eq_ignore_ascii_case("opencode") {
        if let Some(workspace_path) = workspace_path {
            if let Err(err) =
                client.thread_resume(thread_id, Some(workspace_path), Some("gpt-5.3-codex"))
            {
                eprintln!(
                    "[restore] warning: thread/resume before rollback failed thread_id={}: {}",
                    thread_id, err
                );
            }
        }
    }

    let thread = read_thread_for_rollback(client, thread_id, workspace_path)
        .map_err(|err| format!("chat trim failed: {err}"))?;
    let (trimmed_thread, rollback_count, target_index, chrono_target_pos) =
        build_trimmed_thread_view(&thread, target_turn_id)
            .map_err(|err| format!("chat trim failed: {err}"))?;

    let total_turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .map(|turns| turns.len())
        .unwrap_or(0);
    eprintln!(
        "[restore] rollback request thread_id={} target_turn_id={} total_turns={} target_index={} chrono_target_pos={} rollback_count={}",
        thread_id,
        target_turn_id,
        total_turns,
        target_index,
        chrono_target_pos,
        rollback_count
    );

    if rollback_count == 0 {
        return Ok((
            " • Chat already at restored point".to_string(),
            Some(ThreadSyncOutcome {
                thread: trimmed_thread,
                request_runtime_history_reload: false,
            }),
        ));
    }

    let rollback_result = match client.thread_rollback(thread_id, rollback_count) {
        Ok(thread) => Ok(thread),
        Err(err)
            if err.contains("thread not found")
                || err.contains("thread not loaded")
                || err.contains("no rollout found") =>
        {
            if let Some(workspace_path) = workspace_path {
                match client.thread_resume(thread_id, Some(workspace_path), Some("gpt-5.3-codex")) {
                    Ok(_) => client.thread_rollback(thread_id, rollback_count),
                    Err(resume_err) => Err(format!(
                        "{err}; resume before rollback failed: {resume_err}"
                    )),
                }
            } else {
                Err(err)
            }
        }
        Err(err) => Err(err),
    };

    if let Err(err) = rollback_result {
        return Err(format!("chat trim failed: {err}"));
    }

    if let Some(workspace_path) = workspace_path {
        if let Err(err) =
            client.thread_resume(thread_id, Some(workspace_path), Some("gpt-5.3-codex"))
        {
            eprintln!(
                "[restore] warning: thread/resume after rollback failed thread_id={}: {}",
                thread_id, err
            );
        }
    }

    let is_opencode = client.backend_kind().eq_ignore_ascii_case("opencode");
    let sync_thread = if is_opencode {
        trimmed_thread.clone()
    } else {
        client
            .thread_read(thread_id, true)
            .unwrap_or_else(|_| trimmed_thread.clone())
    };
    let remaining_turns = sync_thread
        .get("turns")
        .and_then(Value::as_array)
        .map(|turns| turns.len())
        .unwrap_or(0);
    eprintln!(
        "[restore] rollback applied thread_id={} removed_turns={} remaining_turns={} synthetic_sync={}",
        thread_id, rollback_count, remaining_turns, is_opencode
    );

    Ok((
        format!(" • Chat trimmed: {} turn(s)", rollback_count),
        Some(ThreadSyncOutcome {
            thread: sync_thread,
            request_runtime_history_reload: !is_opencode,
        }),
    ))
}

pub(super) fn apply_opencode_restore_worker(
    codex: Option<Arc<RuntimeClient>>,
    workspace_path: Option<&str>,
    codex_thread_id: &str,
    target_turn_id: Option<&str>,
) -> Result<ChatRestoreWorkerOutcome, String> {
    let Some(target_turn_id) = target_turn_id else {
        return Err("OpenCode restore requires a checkpoint turn target.".to_string());
    };
    let Some(client) = codex.as_ref() else {
        return Err("OpenCode restore requires a running runtime client.".to_string());
    };
    if !client.backend_kind().eq_ignore_ascii_case("opencode") {
        return Err("OpenCode restore is only available for OpenCode threads.".to_string());
    }

    let (status_text, thread_sync) =
        rollback_thread_to_target_worker(client, workspace_path, codex_thread_id, target_turn_id)?;
    Ok(ChatRestoreWorkerOutcome {
        status_text,
        thread_sync,
    })
}

pub(super) fn undo_opencode_restore_worker(
    codex: Option<Arc<RuntimeClient>>,
    workspace_path: Option<&str>,
    codex_thread_id: &str,
) -> Result<ChatRestoreWorkerOutcome, String> {
    let Some(client) = codex.as_ref() else {
        return Err("OpenCode restore undo requires a running runtime client.".to_string());
    };
    if !client.backend_kind().eq_ignore_ascii_case("opencode") {
        return Err("OpenCode restore undo is only available for OpenCode threads.".to_string());
    }

    if let Some(workspace_path) = workspace_path {
        if let Err(err) =
            client.thread_resume(codex_thread_id, Some(workspace_path), Some("gpt-5.3-codex"))
        {
            eprintln!(
                "[restore] warning: thread/resume before OpenCode undo failed thread_id={}: {}",
                codex_thread_id, err
            );
        }
    }

    let thread = client
        .thread_unrollback(codex_thread_id)
        .map_err(|err| format!("OpenCode undo failed: {err}"))?;

    if let Some(workspace_path) = workspace_path {
        if let Err(err) =
            client.thread_resume(codex_thread_id, Some(workspace_path), Some("gpt-5.3-codex"))
        {
            eprintln!(
                "[restore] warning: thread/resume after OpenCode undo failed thread_id={}: {}",
                codex_thread_id, err
            );
        }
    }

    Ok(ChatRestoreWorkerOutcome {
        status_text: " • OpenCode restore undone".to_string(),
        thread_sync: Some(ThreadSyncOutcome {
            thread,
            request_runtime_history_reload: true,
        }),
    })
}

pub(super) fn apply_restore_worker(
    db: &AppDb,
    codex: Option<Arc<RuntimeClient>>,
    workspace_path: Option<&str>,
    codex_thread_id: &str,
    checkpoint_id: i64,
    target_turn_id: Option<&str>,
    selected_paths: &[String],
    forced_paths: &[String],
) -> Result<ApplyRestoreWorkerOutcome, String> {
    let mut rollback_status = " • Chat trim skipped (no remote turn context)".to_string();
    let mut thread_sync = None;

    let opencode_pretrim = codex
        .as_ref()
        .is_some_and(|client| client.backend_kind().eq_ignore_ascii_case("opencode"));

    if opencode_pretrim {
        let Some(target_turn_id) = target_turn_id else {
            return Err(
                "Restore applied, but chat trim failed: no remote turn context.".to_string(),
            );
        };
        let Some(client) = codex.as_ref() else {
            return Err(
                "Restore applied, but chat trim failed: runtime client unavailable.".to_string(),
            );
        };
        let (next_status, next_thread_sync) =
            rollback_thread_to_target_worker(client, workspace_path, codex_thread_id, target_turn_id)
                .map_err(|err| format!("Restore apply aborted because {err}"))?;
        rollback_status = next_status;
        thread_sync = next_thread_sync;
    }

    let apply = crate::restore::apply_restore_to_checkpoint_by_remote_id(
        db,
        codex_thread_id,
        checkpoint_id,
        selected_paths,
        forced_paths,
    )?;
    let Some(result) = apply else {
        return Err("Restore apply unavailable for this checkpoint.".to_string());
    };

    if !opencode_pretrim {
        rollback_status = if let Some(target_turn_id) = target_turn_id {
            let Some(client) = codex.as_ref() else {
                return Err(
                    "Restore applied, but chat trim failed: runtime client unavailable."
                        .to_string(),
                );
            };

            let (next_status, next_thread_sync) =
                rollback_thread_to_target_worker(client, workspace_path, codex_thread_id, target_turn_id)
                    .map_err(|err| format!("Restore applied, but {err}"))?;
            thread_sync = next_thread_sync;
            next_status
        } else {
            " • Chat trim skipped (no remote turn context)".to_string()
        };
    }

    Ok(ApplyRestoreWorkerOutcome {
        result,
        rollback_status,
        thread_sync,
    })
}

pub(super) fn undo_restore_worker(
    db: &AppDb,
    codex: Option<Arc<RuntimeClient>>,
    workspace_path: Option<&str>,
    codex_thread_id: &str,
    backup_checkpoint_id: i64,
    undo_forced_paths: &[String],
) -> Result<Option<UndoRestoreWorkerOutcome>, String> {
    let mut chat_restore_status = String::new();
    let mut thread_sync = None;
    if codex.as_ref().is_some_and(|client| {
        client.backend_kind().eq_ignore_ascii_case("opencode")
    }) {
        match undo_opencode_restore_worker(codex.clone(), workspace_path, codex_thread_id) {
            Ok(outcome) => {
                chat_restore_status = outcome.status_text;
                thread_sync = outcome.thread_sync;
            }
            Err(err) => {
                eprintln!(
                    "[restore] warning: opencode chat undo failed thread_id={}: {}",
                    codex_thread_id, err
                );
                chat_restore_status = format!(" • Chat restore skipped ({err})");
            }
        }
    }

    let result = crate::restore::apply_restore_to_checkpoint_by_remote_id(
        db,
        codex_thread_id,
        backup_checkpoint_id,
        &[],
        undo_forced_paths,
    )?;
    Ok(result.map(|result| UndoRestoreWorkerOutcome {
        result,
        chat_restore_status,
        thread_sync,
    }))
}
