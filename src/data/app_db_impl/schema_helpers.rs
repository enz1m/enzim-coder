use super::*;
use rusqlite::params;

impl AppDb {
    pub fn open_file_connection() -> rusqlite::Result<Connection> {
        let db_path = default_db_path();
        if let Some(parent) = db_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        Connection::open(db_path)
    }

    pub(super) fn init_schema(&self) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute_batch(
            r#"
            PRAGMA foreign_keys = ON;

            CREATE TABLE IF NOT EXISTS workspaces (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                name        TEXT NOT NULL,
                path        TEXT NOT NULL UNIQUE,
                created_at  INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS codex_profiles (
                id                INTEGER PRIMARY KEY AUTOINCREMENT,
                backend_kind      TEXT NOT NULL DEFAULT 'codex',
                name              TEXT NOT NULL,
                icon_name         TEXT NOT NULL DEFAULT '',
                home_dir          TEXT NOT NULL UNIQUE,
                last_account_type TEXT,
                last_email        TEXT,
                status            TEXT NOT NULL DEFAULT 'stopped',
                created_at        INTEGER NOT NULL,
                updated_at        INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS threads (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                workspace_id INTEGER NOT NULL,
                profile_id   INTEGER NOT NULL DEFAULT 1,
                parent_thread_id INTEGER,
                worktree_path TEXT,
                worktree_branch TEXT,
                worktree_active INTEGER NOT NULL DEFAULT 0,
                title        TEXT NOT NULL,
                codex_thread_id TEXT,
                codex_account_type TEXT,
                codex_account_email TEXT,
                created_at   INTEGER NOT NULL,
                updated_at   INTEGER NOT NULL,
                is_closed    INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY(workspace_id) REFERENCES workspaces(id) ON DELETE CASCADE,
                FOREIGN KEY(profile_id) REFERENCES codex_profiles(id),
                FOREIGN KEY(parent_thread_id) REFERENCES threads(id) ON DELETE SET NULL
            );

            CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chat_turns (
                id                 INTEGER PRIMARY KEY AUTOINCREMENT,
                local_thread_id    INTEGER NOT NULL,
                provider_id        TEXT NOT NULL,
                external_thread_id TEXT NOT NULL,
                external_turn_id   TEXT NOT NULL,
                user_text          TEXT NOT NULL DEFAULT '',
                assistant_text     TEXT NOT NULL DEFAULT '',
                raw_items_json     TEXT,
                status             TEXT NOT NULL DEFAULT 'completed',
                created_at         INTEGER NOT NULL,
                completed_at       INTEGER,
                updated_at         INTEGER NOT NULL,
                FOREIGN KEY(local_thread_id) REFERENCES threads(id) ON DELETE CASCADE,
                UNIQUE(local_thread_id, provider_id, external_turn_id)
            );

            CREATE INDEX IF NOT EXISTS idx_chat_turns_local_thread
                ON chat_turns(local_thread_id, created_at, id);

            CREATE INDEX IF NOT EXISTS idx_chat_turns_external
                ON chat_turns(provider_id, external_thread_id, created_at, id);

            CREATE TABLE IF NOT EXISTS voice_to_text_config (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                provider TEXT NOT NULL DEFAULT 'local',
                local_whisper_command TEXT NOT NULL DEFAULT 'whisper',
                local_model_path TEXT,
                cloud_provider TEXT NOT NULL DEFAULT 'openai',
                cloud_url TEXT,
                cloud_api_key TEXT,
                cloud_model TEXT,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS remote_telegram_accounts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                bot_token TEXT NOT NULL,
                telegram_user_id TEXT NOT NULL,
                telegram_chat_id TEXT NOT NULL,
                telegram_username TEXT,
                linked_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                UNIQUE(telegram_user_id, telegram_chat_id)
            );

            CREATE TABLE IF NOT EXISTS remote_telegram_message_map (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                telegram_chat_id TEXT NOT NULL,
                telegram_message_id TEXT NOT NULL,
                local_thread_id INTEGER NOT NULL,
                codex_thread_id TEXT,
                local_turn_id TEXT,
                created_at INTEGER NOT NULL,
                UNIQUE(telegram_chat_id, telegram_message_id)
            );

            CREATE TABLE IF NOT EXISTS remote_pending_prompts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                local_thread_id INTEGER NOT NULL,
                source TEXT NOT NULL DEFAULT 'telegram',
                telegram_chat_id TEXT,
                telegram_message_id TEXT,
                telegram_user_id TEXT,
                telegram_username TEXT,
                text TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                consumed_at INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_remote_pending_prompts_thread
                ON remote_pending_prompts(local_thread_id, consumed_at, created_at, id);

            CREATE TABLE IF NOT EXISTS enzim_agent_config (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                base_url TEXT NOT NULL DEFAULT '',
                api_key TEXT,
                model_id TEXT,
                system_prompt_override TEXT,
                cached_models_json TEXT,
                cached_models_refreshed_at INTEGER,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS enzim_agent_loops (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                local_thread_id INTEGER NOT NULL,
                status TEXT NOT NULL,
                prompt_text TEXT NOT NULL,
                instructions_text TEXT NOT NULL,
                backend_kind TEXT NOT NULL,
                remote_thread_id_snapshot TEXT,
                config_base_url_snapshot TEXT NOT NULL,
                config_model_id_snapshot TEXT NOT NULL,
                system_prompt_snapshot TEXT NOT NULL,
                iteration_count INTEGER NOT NULL DEFAULT 0,
                error_count INTEGER NOT NULL DEFAULT 0,
                last_seen_external_turn_id TEXT,
                final_summary_text TEXT,
                last_error_text TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                finished_at INTEGER,
                FOREIGN KEY(local_thread_id) REFERENCES threads(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_enzim_agent_loops_thread
                ON enzim_agent_loops(local_thread_id, status, updated_at, id);

            CREATE UNIQUE INDEX IF NOT EXISTS idx_enzim_agent_loops_active_thread
                ON enzim_agent_loops(local_thread_id)
                WHERE status IN ('active', 'waiting_runtime', 'evaluating', 'waiting_user');

            CREATE TABLE IF NOT EXISTS enzim_agent_loop_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                loop_id INTEGER NOT NULL,
                sequence_no INTEGER NOT NULL,
                event_kind TEXT NOT NULL,
                author_kind TEXT NOT NULL,
                external_turn_id TEXT,
                full_text TEXT,
                compact_text TEXT,
                decision_json TEXT,
                created_at INTEGER NOT NULL,
                FOREIGN KEY(loop_id) REFERENCES enzim_agent_loops(id) ON DELETE CASCADE,
                UNIQUE(loop_id, sequence_no)
            );

            CREATE INDEX IF NOT EXISTS idx_enzim_agent_loop_events_loop
                ON enzim_agent_loop_events(loop_id, sequence_no, id);
            "#,
        )?;
        self.ensure_threads_codex_column()?;
        self.ensure_threads_account_columns()?;
        self.ensure_threads_profile_column()?;
        self.ensure_threads_parent_column()?;
        self.ensure_threads_worktree_columns()?;
        self.ensure_profiles_backend_kind_column()?;
        self.ensure_profiles_icon_column()?;
        self.ensure_chat_turns_raw_items_column()?;
        Ok(())
    }

    pub(super) fn local_thread_id_for_remote_thread(
        &self,
        remote_thread_id: &str,
    ) -> rusqlite::Result<Option<i64>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id
             FROM threads
             WHERE codex_thread_id = ?1
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![remote_thread_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    #[allow(dead_code)]
    pub(super) fn local_thread_id_for_codex_thread(
        &self,
        codex_thread_id: &str,
    ) -> rusqlite::Result<Option<i64>> {
        self.local_thread_id_for_remote_thread(codex_thread_id)
    }

    pub(super) fn list_workspaces(&self) -> rusqlite::Result<Vec<WorkspaceRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, name, path, created_at
             FROM workspaces
             ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(WorkspaceRecord {
                id: row.get(0)?,
                name: row.get(1)?,
                path: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub(super) fn list_threads_for_workspace(
        &self,
        workspace_id: i64,
    ) -> rusqlite::Result<Vec<ThreadRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, workspace_id, title, codex_thread_id, codex_account_type, codex_account_email, created_at, updated_at, parent_thread_id
             , profile_id, worktree_path, worktree_branch, worktree_active
             FROM threads
             WHERE workspace_id = ?1 AND is_closed = 0
             ORDER BY updated_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![workspace_id], |row| {
            Ok(ThreadRecord {
                id: row.get(0)?,
                workspace_id: row.get(1)?,
                title: row.get(2)?,
                codex_thread_id: row.get(3)?,
                codex_account_type: row.get(4)?,
                codex_account_email: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
                parent_thread_id: row.get(8)?,
                profile_id: row.get(9)?,
                worktree_path: row.get(10)?,
                worktree_branch: row.get(11)?,
                worktree_active: row.get::<_, i64>(12)? != 0,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub(super) fn get_workspace_by_path(&self, path: &str) -> rusqlite::Result<WorkspaceRecord> {
        let conn = self.conn.borrow();
        conn.query_row(
            "SELECT id, name, path, created_at FROM workspaces WHERE path = ?1 LIMIT 1",
            params![path],
            |row| {
                Ok(WorkspaceRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    created_at: row.get(3)?,
                })
            },
        )
    }

    fn ensure_threads_codex_column(&self) -> rusqlite::Result<()> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("PRAGMA table_info(threads)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut has_codex = false;
        for row in rows {
            if row? == "codex_thread_id" {
                has_codex = true;
                break;
            }
        }
        drop(stmt);
        drop(conn);

        if !has_codex {
            self.conn
                .borrow_mut()
                .execute("ALTER TABLE threads ADD COLUMN codex_thread_id TEXT", [])?;
        }
        Ok(())
    }

    fn ensure_profiles_backend_kind_column(&self) -> rusqlite::Result<()> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("PRAGMA table_info(codex_profiles)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        for row in rows {
            if row? == "backend_kind" {
                return Ok(());
            }
        }
        drop(stmt);
        drop(conn);
        self.conn.borrow_mut().execute(
            "ALTER TABLE codex_profiles ADD COLUMN backend_kind TEXT NOT NULL DEFAULT 'codex'",
            [],
        )?;
        Ok(())
    }

    fn ensure_chat_turns_raw_items_column(&self) -> rusqlite::Result<()> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("PRAGMA table_info(chat_turns)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut has_raw_items = false;
        for row in rows {
            if row? == "raw_items_json" {
                has_raw_items = true;
                break;
            }
        }
        drop(stmt);
        drop(conn);

        if !has_raw_items {
            self.conn
                .borrow_mut()
                .execute("ALTER TABLE chat_turns ADD COLUMN raw_items_json TEXT", [])?;
        }
        Ok(())
    }

    fn ensure_threads_account_columns(&self) -> rusqlite::Result<()> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("PRAGMA table_info(threads)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut has_account_type = false;
        let mut has_account_email = false;
        for row in rows {
            let column = row?;
            if column == "codex_account_type" {
                has_account_type = true;
            } else if column == "codex_account_email" {
                has_account_email = true;
            }
        }
        drop(stmt);
        drop(conn);

        if !has_account_type {
            self.conn
                .borrow_mut()
                .execute("ALTER TABLE threads ADD COLUMN codex_account_type TEXT", [])?;
        }
        if !has_account_email {
            self.conn.borrow_mut().execute(
                "ALTER TABLE threads ADD COLUMN codex_account_email TEXT",
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_threads_profile_column(&self) -> rusqlite::Result<()> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("PRAGMA table_info(threads)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut has_profile_id = false;
        for row in rows {
            if row? == "profile_id" {
                has_profile_id = true;
                break;
            }
        }
        drop(stmt);
        drop(conn);

        if !has_profile_id {
            self.conn.borrow_mut().execute(
                "ALTER TABLE threads ADD COLUMN profile_id INTEGER NOT NULL DEFAULT 1",
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_threads_parent_column(&self) -> rusqlite::Result<()> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("PRAGMA table_info(threads)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut has_parent_thread_id = false;
        for row in rows {
            if row? == "parent_thread_id" {
                has_parent_thread_id = true;
                break;
            }
        }
        drop(stmt);
        drop(conn);

        if !has_parent_thread_id {
            self.conn.borrow_mut().execute(
                "ALTER TABLE threads ADD COLUMN parent_thread_id INTEGER",
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_threads_worktree_columns(&self) -> rusqlite::Result<()> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("PRAGMA table_info(threads)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut has_worktree_path = false;
        let mut has_worktree_branch = false;
        let mut has_worktree_active = false;
        for row in rows {
            let column = row?;
            if column == "worktree_path" {
                has_worktree_path = true;
            } else if column == "worktree_branch" {
                has_worktree_branch = true;
            } else if column == "worktree_active" {
                has_worktree_active = true;
            }
        }
        drop(stmt);
        drop(conn);

        if !has_worktree_path {
            self.conn
                .borrow_mut()
                .execute("ALTER TABLE threads ADD COLUMN worktree_path TEXT", [])?;
        }
        if !has_worktree_branch {
            self.conn
                .borrow_mut()
                .execute("ALTER TABLE threads ADD COLUMN worktree_branch TEXT", [])?;
        }
        if !has_worktree_active {
            self.conn.borrow_mut().execute(
                "ALTER TABLE threads ADD COLUMN worktree_active INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        Ok(())
    }

    fn ensure_profiles_icon_column(&self) -> rusqlite::Result<()> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("PRAGMA table_info(codex_profiles)")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        let mut has_icon_name = false;
        for row in rows {
            if row? == "icon_name" {
                has_icon_name = true;
                break;
            }
        }
        drop(stmt);
        drop(conn);

        if !has_icon_name {
            self.conn.borrow_mut().execute(
                "ALTER TABLE codex_profiles
                 ADD COLUMN icon_name TEXT NOT NULL DEFAULT ''",
                [],
            )?;
        }
        Ok(())
    }

    pub(super) fn any_profile_matches_thread_account(
        &self,
        thread_account_type: Option<&str>,
        thread_account_email: Option<&str>,
    ) -> rusqlite::Result<bool> {
        let target_type = thread_account_type
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        let target_email = thread_account_email
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty());
        let Some(target_type) = target_type else {
            return Ok(false);
        };
        let profiles = self.list_codex_profiles()?;
        for profile in profiles {
            let profile_type = profile
                .last_account_type
                .as_deref()
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty());
            if profile_type.as_deref() != Some(target_type.as_str()) {
                continue;
            }
            let profile_email = profile
                .last_email
                .as_deref()
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty());
            if profile_email == target_email {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
