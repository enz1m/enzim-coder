include!("build_helpers.rs");
include!("build_content_body.rs");

pub fn build_multi_chat_content(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    codex: Option<Arc<RuntimeClient>>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
) -> gtk::Box {
    build_multi_chat_content_inner(
        db,
        manager,
        codex,
        active_thread_id,
        active_workspace_path,
    )
}
