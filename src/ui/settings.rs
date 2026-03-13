use crate::data::AppDb;

pub(crate) const SETTING_MULTIVIEW_ENABLED: &str = "multiview_enabled";
pub(crate) const SETTING_PANE_LAYOUT_V1: &str = "pane_layout_v1";

pub(crate) fn bool_setting(db: &AppDb, key: &str, default: bool) -> bool {
    db.get_setting(key)
        .ok()
        .flatten()
        .map(|raw| raw == "1" || raw.eq_ignore_ascii_case("true"))
        .unwrap_or(default)
}

pub(crate) fn is_multiview_enabled(db: &AppDb) -> bool {
    bool_setting(db, SETTING_MULTIVIEW_ENABLED, false)
}
