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
        Some("codex"),
        "Codex Profiles",
        "Manage isolated Codex runtime profiles. Create additional profiles for separate accounts and backend sessions.",
        true,
        false,
        false,
    )
}
