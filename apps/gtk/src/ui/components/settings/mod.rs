pub(crate) mod codex;
pub(crate) mod enzim_agent;
pub(crate) mod general;
pub(crate) mod opencode;
mod shared;

use std::rc::Rc;

use crate::services::app::CodexProfileManager;
use crate::services::app::chat::AppDb;

pub(crate) fn show(parent: Option<&gtk::Window>, db: Rc<AppDb>, manager: Rc<CodexProfileManager>) {
    shared::show(parent, db, manager);
}
