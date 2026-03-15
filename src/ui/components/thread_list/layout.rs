use crate::data::AppDb;
use crate::ui::settings::SETTING_PANE_LAYOUT_V1;
use serde_json::{Value, json};

fn parse_layout(raw: &str) -> Value {
    serde_json::from_str::<Value>(raw).unwrap_or_else(|_| {
        json!({
            "version": 1,
            "focusedPaneId": 1,
            "panes": []
        })
    })
}

pub(super) fn remove_thread_from_multiview_layout(db: &AppDb, codex_thread_id: &str) {
    if codex_thread_id.trim().is_empty() {
        return;
    }
    let Some(raw) = db.get_setting(SETTING_PANE_LAYOUT_V1).ok().flatten() else {
        return;
    };
    let mut layout = parse_layout(&raw);
    let mut panes: Vec<Value> = layout
        .get("panes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    panes.retain(|pane| {
        pane.get("threadId")
            .or_else(|| pane.get("codexThreadId"))
            .and_then(Value::as_str)
            .map(|id| id != codex_thread_id)
            .unwrap_or(true)
    });
    let focused = layout
        .get("focusedPaneId")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let focused_next = if panes.is_empty() {
        1
    } else if panes
        .iter()
        .any(|pane| pane.get("id").and_then(Value::as_u64) == Some(focused))
    {
        focused
    } else {
        panes[0].get("id").and_then(Value::as_u64).unwrap_or(1)
    };
    layout["focusedPaneId"] = Value::from(focused_next);
    layout["panes"] = Value::Array(panes);
    let _ = db.set_setting(SETTING_PANE_LAYOUT_V1, &layout.to_string());
}
