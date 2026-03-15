pub(crate) mod codex;
pub(crate) mod opencode;
mod shared;

use std::rc::Rc;

use crate::codex_profiles::CodexProfileManager;
use crate::data::AppDb;

pub(crate) fn show(parent: Option<&gtk::Window>, db: Rc<AppDb>, manager: Rc<CodexProfileManager>) {
    shared::show(parent, db, manager);
}
