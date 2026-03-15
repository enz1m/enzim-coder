use std::rc::Rc;

use crate::codex_profiles::CodexProfileManager;
use crate::data::AppDb;

pub(crate) fn build_settings_page(
    dialog: &gtk::Window,
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
) -> (gtk::Box, gtk::Button) {
    super::shared::build_profile_settings_page(
        dialog,
        db,
        manager,
        Some("opencode"),
        "OpenCode",
        "OpenCode uses a single runtime settings page. This page shows its status and provider settings without exposing extra profile creation.",
        false,
        false,
        true,
    )
}
