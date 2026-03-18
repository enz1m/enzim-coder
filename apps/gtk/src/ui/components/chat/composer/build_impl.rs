include!("build_helpers.rs");
include!("build_body.rs");

pub(super) fn build(
    db: Rc<AppDb>,
    manager: Rc<CodexProfileManager>,
    codex: Option<Arc<RuntimeClient>>,
    active_thread_id: Rc<RefCell<Option<String>>>,
    active_workspace_path: Rc<RefCell<Option<String>>>,
    messages_box: gtk::Box,
    messages_scroll: gtk::ScrolledWindow,
    conversation_stack: gtk::Stack,
    active_turn: Rc<RefCell<Option<String>>>,
    active_turn_thread: Rc<RefCell<Option<String>>>,
) -> ComposerSection {
    build_inner(
        db,
        manager,
        codex,
        active_thread_id,
        active_workspace_path,
        messages_box,
        messages_scroll,
        conversation_stack,
        active_turn,
        active_turn_thread,
    )
}
