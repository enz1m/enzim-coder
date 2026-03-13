use super::*;
use rusqlite::params;

impl AppDb {
    pub fn open_default() -> Rc<Self> {
        let conn = match Self::open_file_connection() {
            Ok(conn) => conn,
            Err(err) => {
                eprintln!("failed to open DB file, using in-memory DB: {err}");
                Connection::open_in_memory().expect("failed to create in-memory sqlite DB")
            }
        };

        let db = Self {
            conn: RefCell::new(conn),
        };

        if let Err(err) = db.init_schema() {
            eprintln!("failed to initialize DB schema: {err}");
        }

        Rc::new(db)
    }

    pub fn list_workspaces_with_threads(&self) -> rusqlite::Result<Vec<WorkspaceWithThreads>> {
        let workspaces = self.list_workspaces()?;
        let mut out = Vec::with_capacity(workspaces.len());

        for workspace in workspaces {
            let threads = self.list_threads_for_workspace(workspace.id)?;
            out.push(WorkspaceWithThreads { workspace, threads });
        }

        Ok(out)
    }

    pub fn add_workspace_from_path(
        &self,
        folder_path: &Path,
    ) -> rusqlite::Result<Option<WorkspaceRecord>> {
        let canonical = fs::canonicalize(folder_path).unwrap_or_else(|_| folder_path.to_path_buf());
        let path_str = canonical.to_string_lossy().to_string();
        let name = canonical
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| path_str.clone());

        let conn = self.conn.borrow_mut();
        let inserted = conn.execute(
            "INSERT INTO workspaces (name, path, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO NOTHING",
            params![name, path_str, unix_now()],
        )?;
        drop(conn);

        if inserted == 0 {
            return Ok(None);
        }

        self.get_workspace_by_path(&path_str).map(Some)
    }

    pub fn create_thread(
        &self,
        workspace_id: i64,
        profile_id: i64,
        parent_thread_id: Option<i64>,
        title: &str,
        codex_thread_id: Option<&str>,
        codex_account_type: Option<&str>,
        codex_account_email: Option<&str>,
    ) -> rusqlite::Result<ThreadRecord> {
        let now = unix_now();
        let conn = self.conn.borrow_mut();
        conn.execute(
            "INSERT INTO threads (
                workspace_id,
                profile_id,
                parent_thread_id,
                title,
                codex_thread_id,
                codex_account_type,
                codex_account_email,
                created_at,
                updated_at,
                is_closed
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, 0)",
            params![
                workspace_id,
                profile_id,
                parent_thread_id,
                title,
                codex_thread_id,
                codex_account_type,
                codex_account_email,
                now
            ],
        )?;
        let id = conn.last_insert_rowid();
        Ok(ThreadRecord {
            id,
            workspace_id,
            profile_id,
            parent_thread_id,
            worktree_path: None,
            worktree_branch: None,
            worktree_active: false,
            title: title.to_string(),
            codex_thread_id: codex_thread_id.map(|s| s.to_string()),
            codex_account_type: codex_account_type.map(|s| s.to_string()),
            codex_account_email: codex_account_email.map(|s| s.to_string()),
            created_at: now,
            updated_at: now,
        })
    }

    pub fn rename_thread(&self, thread_id: i64, title: &str) -> rusqlite::Result<()> {
        let now = unix_now();
        self.conn.borrow_mut().execute(
            "UPDATE threads SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title, now, thread_id],
        )?;
        Ok(())
    }

    pub fn rename_thread_if_new_by_codex_id(
        &self,
        codex_thread_id: &str,
        title: &str,
    ) -> rusqlite::Result<Option<i64>> {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }

        let conn = self.conn.borrow_mut();
        let mut stmt = conn.prepare(
            "SELECT id, title
             FROM threads
             WHERE codex_thread_id = ?1
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![codex_thread_id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };

        let thread_id: i64 = row.get(0)?;
        let current_title: String = row.get(1)?;
        if current_title.trim() != "New thread" {
            return Ok(None);
        }

        let now = unix_now();
        conn.execute(
            "UPDATE threads SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![trimmed, now, thread_id],
        )?;
        Ok(Some(thread_id))
    }

    pub fn close_thread(&self, thread_id: i64) -> rusqlite::Result<()> {
        let now = unix_now();
        self.conn.borrow_mut().execute(
            "UPDATE threads SET is_closed = 1, updated_at = ?1 WHERE id = ?2",
            params![now, thread_id],
        )?;
        Ok(())
    }

    pub fn delete_open_threads_without_turns(&self) -> rusqlite::Result<usize> {
        self.conn.borrow_mut().execute(
            "DELETE FROM threads
             WHERE is_closed = 0
               AND NOT EXISTS (
                   SELECT 1
                   FROM chat_turns
                   WHERE chat_turns.local_thread_id = threads.id
               )",
            [],
        )
    }

    pub fn list_threads_for_workspace_all(
        &self,
        workspace_id: i64,
    ) -> rusqlite::Result<Vec<ThreadRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, profile_id, parent_thread_id, worktree_path, worktree_branch, worktree_active, title, codex_thread_id, codex_account_type, codex_account_email, created_at, updated_at
             FROM threads
             WHERE workspace_id = ?1
             ORDER BY updated_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![workspace_id], |row| {
            Ok(ThreadRecord {
                id: row.get(0)?,
                workspace_id: row.get(1)?,
                profile_id: row.get(2)?,
                parent_thread_id: row.get(3)?,
                worktree_path: row.get(4)?,
                worktree_branch: row.get(5)?,
                worktree_active: row.get::<_, i64>(6)? != 0,
                title: row.get(7)?,
                codex_thread_id: row.get(8)?,
                codex_account_type: row.get(9)?,
                codex_account_email: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_workspace(&self, workspace_id: i64) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "DELETE FROM workspaces WHERE id = ?1",
            params![workspace_id],
        )?;
        Ok(())
    }

    pub fn voice_to_text_config(&self) -> rusqlite::Result<Option<VoiceToTextConfig>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT provider, local_whisper_command, local_model_path, cloud_provider, cloud_url, cloud_api_key, cloud_model, updated_at
             FROM voice_to_text_config
             WHERE id = 1
             LIMIT 1",
        )?;
        let mut rows = stmt.query([])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        Ok(Some(VoiceToTextConfig {
            provider: row.get(0)?,
            local_whisper_command: row.get(1)?,
            local_model_path: row.get(2)?,
            cloud_provider: row.get(3)?,
            cloud_url: row.get(4)?,
            cloud_api_key: row.get(5)?,
            cloud_model: row.get(6)?,
            updated_at: row.get(7)?,
        }))
    }

    pub fn upsert_voice_to_text_config(&self, config: &VoiceToTextConfig) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "INSERT INTO voice_to_text_config(
                id,
                provider,
                local_whisper_command,
                local_model_path,
                cloud_provider,
                cloud_url,
                cloud_api_key,
                cloud_model,
                updated_at
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                provider = excluded.provider,
                local_whisper_command = excluded.local_whisper_command,
                local_model_path = excluded.local_model_path,
                cloud_provider = excluded.cloud_provider,
                cloud_url = excluded.cloud_url,
                cloud_api_key = excluded.cloud_api_key,
                cloud_model = excluded.cloud_model,
                updated_at = excluded.updated_at",
            params![
                config.provider,
                config.local_whisper_command,
                config.local_model_path,
                config.cloud_provider,
                config.cloud_url,
                config.cloud_api_key,
                config.cloud_model,
                config.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn set_thread_codex_id_with_account(
        &self,
        thread_id: i64,
        codex_thread_id: &str,
        codex_account_type: Option<&str>,
        codex_account_email: Option<&str>,
    ) -> rusqlite::Result<()> {
        let now = unix_now();
        self.conn.borrow_mut().execute(
            "UPDATE threads
             SET codex_thread_id = ?1,
                 codex_account_type = ?2,
                 codex_account_email = ?3,
                 updated_at = ?4
             WHERE id = ?5",
            params![
                codex_thread_id,
                codex_account_type,
                codex_account_email,
                now,
                thread_id
            ],
        )?;
        Ok(())
    }

    pub fn set_thread_account_identity(
        &self,
        thread_id: i64,
        codex_account_type: Option<&str>,
        codex_account_email: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "UPDATE threads
             SET codex_account_type = ?1,
                 codex_account_email = ?2
             WHERE id = ?3",
            params![codex_account_type, codex_account_email, thread_id],
        )?;
        Ok(())
    }

    pub fn get_thread_profile_id_by_codex_thread_id(
        &self,
        codex_thread_id: &str,
    ) -> rusqlite::Result<Option<i64>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT profile_id
             FROM threads
             WHERE codex_thread_id = ?1
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![codex_thread_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn get_thread_record_by_codex_thread_id(
        &self,
        codex_thread_id: &str,
    ) -> rusqlite::Result<Option<ThreadRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, profile_id, parent_thread_id, worktree_path, worktree_branch, worktree_active, title, codex_thread_id, codex_account_type, codex_account_email, created_at, updated_at
             FROM threads
             WHERE codex_thread_id = ?1
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![codex_thread_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(ThreadRecord {
                id: row.get(0)?,
                workspace_id: row.get(1)?,
                profile_id: row.get(2)?,
                parent_thread_id: row.get(3)?,
                worktree_path: row.get(4)?,
                worktree_branch: row.get(5)?,
                worktree_active: row.get::<_, i64>(6)? != 0,
                title: row.get(7)?,
                codex_thread_id: row.get(8)?,
                codex_account_type: row.get(9)?,
                codex_account_email: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn has_open_thread_for_codex_thread_id(
        &self,
        codex_thread_id: &str,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT 1
             FROM threads
             WHERE codex_thread_id = ?1
               AND is_closed = 0
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![codex_thread_id])?;
        Ok(rows.next()?.is_some())
    }

    pub fn get_thread_record(&self, thread_id: i64) -> rusqlite::Result<Option<ThreadRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, profile_id, parent_thread_id, worktree_path, worktree_branch, worktree_active, title, codex_thread_id, codex_account_type, codex_account_email, created_at, updated_at
             FROM threads
             WHERE id = ?1
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![thread_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(ThreadRecord {
                id: row.get(0)?,
                workspace_id: row.get(1)?,
                profile_id: row.get(2)?,
                parent_thread_id: row.get(3)?,
                worktree_path: row.get(4)?,
                worktree_branch: row.get(5)?,
                worktree_active: row.get::<_, i64>(6)? != 0,
                title: row.get(7)?,
                codex_thread_id: row.get(8)?,
                codex_account_type: row.get(9)?,
                codex_account_email: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn thread_display_timestamp(&self, thread_id: i64) -> rusqlite::Result<Option<i64>> {
        let conn = self.conn.borrow();
        let mut turn_stmt = conn.prepare(
            "SELECT MAX(created_at)
             FROM chat_turns
             WHERE local_thread_id = ?1",
        )?;
        let turn_ts: Option<i64> = turn_stmt.query_row(params![thread_id], |row| row.get(0))?;
        if turn_ts.is_some() {
            return Ok(turn_ts);
        }

        let mut thread_stmt = conn.prepare(
            "SELECT created_at
             FROM threads
             WHERE id = ?1
             LIMIT 1",
        )?;
        let mut rows = thread_stmt.query(params![thread_id])?;
        if let Some(row) = rows.next()? {
            let created_at: i64 = row.get(0)?;
            Ok(Some(created_at))
        } else {
            Ok(None)
        }
    }

    pub fn thread_relative_time_by_id(&self, thread_id: i64, fallback_created_at: i64) -> String {
        let ts = self
            .thread_display_timestamp(thread_id)
            .ok()
            .flatten()
            .unwrap_or(fallback_created_at);
        format_relative_age(ts)
    }

    pub fn assign_thread_profile_and_codex(
        &self,
        thread_id: i64,
        profile_id: i64,
        codex_thread_id: &str,
        codex_account_type: Option<&str>,
        codex_account_email: Option<&str>,
    ) -> rusqlite::Result<()> {
        let now = unix_now();
        self.conn.borrow_mut().execute(
            "UPDATE threads
             SET profile_id = ?1,
                 codex_thread_id = ?2,
                 codex_account_type = ?3,
                 codex_account_email = ?4,
                 updated_at = ?5
             WHERE id = ?6",
            params![
                profile_id,
                codex_thread_id,
                codex_account_type,
                codex_account_email,
                now,
                thread_id
            ],
        )?;
        Ok(())
    }

    pub fn set_thread_worktree_info(
        &self,
        thread_id: i64,
        worktree_path: Option<&str>,
        worktree_branch: Option<&str>,
        worktree_active: bool,
    ) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "UPDATE threads
             SET worktree_path = ?1,
                 worktree_branch = ?2,
                 worktree_active = ?3,
                 updated_at = ?4
             WHERE id = ?5",
            params![
                worktree_path,
                worktree_branch,
                if worktree_active { 1 } else { 0 },
                unix_now(),
                thread_id
            ],
        )?;
        Ok(())
    }

    pub fn active_profile_id(&self) -> rusqlite::Result<Option<i64>> {
        let Some(value) = self.get_setting("codex_active_profile_id")? else {
            return Ok(None);
        };
        Ok(value.parse::<i64>().ok())
    }

    pub fn set_active_profile_id(&self, profile_id: i64) -> rusqlite::Result<()> {
        self.set_setting("codex_active_profile_id", &profile_id.to_string())
    }

    pub fn runtime_profile_id(&self) -> rusqlite::Result<Option<i64>> {
        let Some(value) = self.get_setting("codex_runtime_profile_id")? else {
            return Ok(None);
        };
        Ok(value.parse::<i64>().ok())
    }

    pub fn set_runtime_profile_id(&self, profile_id: i64) -> rusqlite::Result<()> {
        self.set_setting("codex_runtime_profile_id", &profile_id.to_string())
    }
}
