#[derive(Clone, Debug)]
pub(super) struct GitFileEntry {
    pub(super) status: String,
    pub(super) path: String,
    pub(super) selected: bool,
}

#[derive(Clone, Debug)]
pub(super) struct LocalCommitEntry {
    pub(super) short_hash: String,
    pub(super) summary: String,
}

#[derive(Clone, Debug)]
pub(super) struct GitSnapshot {
    pub(super) workspace_root: String,
    pub(super) repo_name: String,
    pub(super) repository_url: String,
    pub(super) branch_label: String,
    pub(super) push_hint: String,
    pub(super) has_upstream: bool,
    pub(super) ahead_count: usize,
    pub(super) unpushed_commits: Vec<LocalCommitEntry>,
    pub(super) remotes: Vec<String>,
    pub(super) files: Vec<GitFileEntry>,
}

pub(super) enum WorkerEvent {
    Loaded(Result<GitSnapshot, String>),
    CommitDone(Result<String, String>),
    PushDone(PushOutcome),
    FetchDone(Result<String, String>),
    PullDone(Result<String, String>),
    BranchDone(Result<String, String>),
    InitDone(Result<String, String>),
    UpstreamDone(Result<String, String>),
}

pub(super) enum PushOutcome {
    Success(String),
    AuthRequired(String),
    Failure(String),
}

#[derive(Clone)]
pub(super) struct PushCredentials {
    pub(super) username: String,
    pub(super) password: String,
}

#[derive(Clone)]
pub(super) struct InitRepoOptions {
    pub(super) branch: String,
    pub(super) create_gitignore: bool,
    pub(super) create_initial_commit: bool,
    pub(super) commit_message: String,
}

#[derive(Clone)]
pub(super) struct UpstreamOptions {
    pub(super) remote_name: String,
    pub(super) remote_url: String,
    pub(super) branch_name: String,
}

#[derive(Clone)]
pub(super) enum BranchAction {
    Switch(String),
    Create(String),
}
