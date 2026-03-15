use super::*;
use rusqlite::params;
use std::collections::HashSet;

impl AppDb {
    fn opencode_hidden_models_setting_key(profile_id: i64) -> String {
        format!("opencode:profile:{profile_id}:hidden_models")
    }

    pub fn list_codex_profiles(&self) -> rusqlite::Result<Vec<CodexProfileRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, backend_kind, name, icon_name, home_dir, last_account_type, last_email, status, created_at, updated_at
             FROM codex_profiles
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CodexProfileRecord {
                id: row.get(0)?,
                backend_kind: row.get(1)?,
                name: row.get(2)?,
                icon_name: row.get(3)?,
                home_dir: row.get(4)?,
                last_account_type: row.get(5)?,
                last_email: row.get(6)?,
                status: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_codex_profile(
        &self,
        profile_id: i64,
    ) -> rusqlite::Result<Option<CodexProfileRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, backend_kind, name, icon_name, home_dir, last_account_type, last_email, status, created_at, updated_at
             FROM codex_profiles
             WHERE id = ?1
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![profile_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(CodexProfileRecord {
                id: row.get(0)?,
                backend_kind: row.get(1)?,
                name: row.get(2)?,
                icon_name: row.get(3)?,
                home_dir: row.get(4)?,
                last_account_type: row.get(5)?,
                last_email: row.get(6)?,
                status: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn create_codex_profile(
        &self,
        name: &str,
        backend_kind: &str,
        home_dir: &str,
    ) -> rusqlite::Result<CodexProfileRecord> {
        let icon_name = self.pick_icon_for_new_profile(name);
        let now = unix_now();
        let conn = self.conn.borrow_mut();
        conn.execute(
            "INSERT INTO codex_profiles(backend_kind, name, icon_name, home_dir, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'stopped', ?5, ?5)",
            params![backend_kind, name, icon_name, home_dir, now],
        )?;
        let id = conn.last_insert_rowid();
        Ok(CodexProfileRecord {
            id,
            backend_kind: backend_kind.to_string(),
            name: name.to_string(),
            icon_name,
            home_dir: home_dir.to_string(),
            last_account_type: None,
            last_email: None,
            status: "stopped".to_string(),
            created_at: now,
            updated_at: now,
        })
    }

    pub fn update_codex_profile_icon(
        &self,
        profile_id: i64,
        icon_name: &str,
    ) -> rusqlite::Result<()> {
        let icon_name = icon_name.trim().to_string();
        self.conn.borrow_mut().execute(
            "UPDATE codex_profiles
             SET icon_name = ?1, updated_at = ?2
             WHERE id = ?3",
            params![icon_name, unix_now(), profile_id],
        )?;
        Ok(())
    }

    fn pick_icon_for_new_profile(&self, name: &str) -> String {
        if name.trim().eq_ignore_ascii_case("system") {
            return "computer-symbolic".to_string();
        }

        let used_icons: HashSet<String> = self
            .list_codex_profiles()
            .unwrap_or_default()
            .into_iter()
            .map(|profile| profile.icon_name.trim().to_string())
            .filter(|icon_name| !icon_name.is_empty())
            .collect();

        let mut candidates: Vec<&str> = PROFILE_ICON_POOL
            .iter()
            .copied()
            .filter(|icon_name| !used_icons.contains(*icon_name))
            .collect();
        if candidates.is_empty() {
            candidates.extend(PROFILE_ICON_POOL);
        }
        if candidates.is_empty() {
            return "person-symbolic".to_string();
        }

        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as usize;
        candidates[seed % candidates.len()].to_string()
    }

    pub fn update_codex_profile_status(
        &self,
        profile_id: i64,
        status: &str,
    ) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "UPDATE codex_profiles
             SET status = ?1, updated_at = ?2
             WHERE id = ?3",
            params![status, unix_now(), profile_id],
        )?;
        Ok(())
    }

    pub fn update_codex_profile_account(
        &self,
        profile_id: i64,
        account_type: Option<&str>,
        email: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "UPDATE codex_profiles
             SET last_account_type = ?1, last_email = ?2, updated_at = ?3
             WHERE id = ?4",
            params![account_type, email, unix_now(), profile_id],
        )?;
        Ok(())
    }

    pub fn update_profile_account_identity(
        &self,
        profile_id: i64,
        account_type: Option<&str>,
        email: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.update_codex_profile_account(profile_id, account_type, email)
    }

    pub fn update_codex_profile_home_dir(
        &self,
        profile_id: i64,
        home_dir: &str,
    ) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "UPDATE codex_profiles
             SET home_dir = ?1, updated_at = ?2
             WHERE id = ?3",
            params![home_dir, unix_now(), profile_id],
        )?;
        Ok(())
    }

    pub fn remove_codex_profile(
        &self,
        profile_id: i64,
        fallback_profile_id: i64,
    ) -> rusqlite::Result<()> {
        let mut conn = self.conn.borrow_mut();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE threads
             SET profile_id = ?1
             WHERE profile_id = ?2",
            params![fallback_profile_id, profile_id],
        )?;
        tx.execute(
            "DELETE FROM codex_profiles
             WHERE id = ?1",
            params![profile_id],
        )?;
        tx.commit()?;

        if self.active_profile_id()? == Some(profile_id) {
            self.set_active_profile_id(fallback_profile_id)?;
        }
        if self.runtime_profile_id()? == Some(profile_id) {
            self.set_runtime_profile_id(fallback_profile_id)?;
        }
        Ok(())
    }

    pub fn ensure_default_codex_profile(&self, app_data_dir: &Path) -> rusqlite::Result<i64> {
        let configured_home = crate::data::configured_profile_home_dir(app_data_dir);
        let configured_home_str = configured_home.to_string_lossy().to_string();
        let mut profiles = self.list_codex_profiles()?;
        if profiles.is_empty() {
            let profile = self.create_codex_profile("System", "codex", &configured_home_str)?;
            self.set_active_profile_id(profile.id)?;
            return Ok(profile.id);
        }
        profiles.sort_by_key(|profile| profile.id);
        let mut default_id = profiles[0].id;
        if let Some(existing) = profiles
            .iter()
            .find(|profile| profile.home_dir == configured_home_str)
        {
            default_id = existing.id;
        }

        let target_id = self.active_profile_id()?.unwrap_or(default_id);
        if let Some(target_profile) = profiles.iter().find(|profile| profile.id == target_id) {
            if target_profile.home_dir != configured_home_str {
                if let Some(existing) = profiles
                    .iter()
                    .find(|profile| profile.home_dir == configured_home_str)
                {
                    default_id = existing.id;
                    if self.active_profile_id()?.is_none() {
                        self.set_active_profile_id(default_id)?;
                    }
                } else {
                    self.update_codex_profile_home_dir(target_id, &configured_home_str)?;
                    default_id = target_id;
                }
            } else {
                default_id = target_id;
            }
        }

        if self.active_profile_id()?.is_none() {
            self.set_active_profile_id(default_id)?;
        }
        let active_id = self.active_profile_id()?.unwrap_or(default_id);
        Ok(active_id)
    }

    pub fn current_codex_account(&self) -> rusqlite::Result<Option<(String, Option<String>)>> {
        let account_type = self.get_setting("codex_current_account_type")?;
        let Some(account_type) = account_type else {
            return Ok(None);
        };
        let account_type = account_type.trim().to_ascii_lowercase();
        if account_type.is_empty() {
            return Ok(None);
        }
        let account_email = self
            .get_setting("codex_current_account_email")?
            .map(|email| email.trim().to_ascii_lowercase())
            .filter(|email| !email.is_empty());
        Ok(Some((account_type, account_email)))
    }

    #[allow(dead_code)]
    pub fn current_profile_account_identity(
        &self,
    ) -> rusqlite::Result<Option<(String, Option<String>)>> {
        self.current_codex_account()
    }

    pub fn current_thread_account(&self) -> rusqlite::Result<Option<(String, Option<String>)>> {
        self.current_codex_account()
    }

    pub fn set_current_codex_account(
        &self,
        account_type: Option<&str>,
        account_email: Option<&str>,
    ) -> rusqlite::Result<()> {
        match account_type.map(|value| value.trim().to_ascii_lowercase()) {
            Some(value) if !value.is_empty() => {
                self.set_setting("codex_current_account_type", &value)?;
            }
            _ => {
                self.conn.borrow_mut().execute(
                    "DELETE FROM settings WHERE key = ?1",
                    params!["codex_current_account_type"],
                )?;
            }
        }

        match account_email.map(|value| value.trim().to_ascii_lowercase()) {
            Some(value) if !value.is_empty() => {
                self.set_setting("codex_current_account_email", &value)?;
            }
            _ => {
                self.conn.borrow_mut().execute(
                    "DELETE FROM settings WHERE key = ?1",
                    params!["codex_current_account_email"],
                )?;
            }
        }
        Ok(())
    }

    pub fn set_current_profile_account_identity(
        &self,
        account_type: Option<&str>,
        account_email: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.set_current_codex_account(account_type, account_email)
    }

    pub fn set_current_thread_account(
        &self,
        account_type: Option<&str>,
        account_email: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.set_current_codex_account(account_type, account_email)
    }

    pub fn is_local_thread_locked(&self, local_thread_id: i64) -> rusqlite::Result<bool> {
        let (thread_profile_id, thread_account_type, thread_account_email): (
            i64,
            Option<String>,
            Option<String>,
        ) = {
            let conn = self.conn.borrow();
            let mut stmt = conn.prepare(
                "SELECT profile_id, codex_account_type, codex_account_email
                 FROM threads
                 WHERE id = ?1
                 LIMIT 1",
            )?;
            let mut rows = stmt.query(params![local_thread_id])?;
            let Some(row) = rows.next()? else {
                return Ok(false);
            };
            (row.get(0)?, row.get(1)?, row.get(2)?)
        };
        let thread_has_identity = thread_account_type
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
            || thread_account_email
                .as_deref()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false);
        if !thread_has_identity {
            return Ok(false);
        }
        if self.any_profile_matches_thread_account(
            thread_account_type.as_deref(),
            thread_account_email.as_deref(),
        )? {
            return Ok(false);
        }
        let _ = thread_profile_id;
        Ok(true)
    }

    pub fn is_remote_thread_locked(&self, remote_thread_id: &str) -> rusqlite::Result<bool> {
        let (thread_profile_id, thread_account_type, thread_account_email): (
            i64,
            Option<String>,
            Option<String>,
        ) = {
            let conn = self.conn.borrow();
            let mut stmt = conn.prepare(
                "SELECT profile_id, codex_account_type, codex_account_email
                 FROM threads
                 WHERE codex_thread_id = ?1
                 LIMIT 1",
            )?;
            let mut rows = stmt.query(params![remote_thread_id])?;
            let Some(row) = rows.next()? else {
                return Ok(false);
            };
            (row.get(0)?, row.get(1)?, row.get(2)?)
        };
        let thread_has_identity = thread_account_type
            .as_deref()
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false)
            || thread_account_email
                .as_deref()
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false);
        if !thread_has_identity {
            return Ok(false);
        }
        if self.any_profile_matches_thread_account(
            thread_account_type.as_deref(),
            thread_account_email.as_deref(),
        )? {
            return Ok(false);
        }
        let _ = thread_profile_id;
        Ok(true)
    }

    #[allow(dead_code)]
    pub fn is_codex_thread_locked(&self, codex_thread_id: &str) -> rusqlite::Result<bool> {
        self.is_remote_thread_locked(codex_thread_id)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1 LIMIT 1")?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn opencode_hidden_models(&self, profile_id: i64) -> rusqlite::Result<HashSet<String>> {
        let Some(raw) = self.get_setting(&Self::opencode_hidden_models_setting_key(profile_id))?
        else {
            return Ok(HashSet::new());
        };
        let values = serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default();
        Ok(values
            .into_iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect())
    }

    pub fn set_opencode_hidden_models(
        &self,
        profile_id: i64,
        hidden_models: &HashSet<String>,
    ) -> rusqlite::Result<()> {
        let mut values = hidden_models
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        values.sort();
        values.dedup();
        self.set_setting(
            &Self::opencode_hidden_models_setting_key(profile_id),
            &serde_json::to_string(&values).unwrap_or_else(|_| "[]".to_string()),
        )
    }

    pub fn set_opencode_model_hidden(
        &self,
        profile_id: i64,
        model_id: &str,
        hidden: bool,
    ) -> rusqlite::Result<HashSet<String>> {
        let mut hidden_models = self.opencode_hidden_models(profile_id)?;
        let model_id = model_id.trim();
        if model_id.is_empty() {
            return Ok(hidden_models);
        }
        if hidden {
            hidden_models.insert(model_id.to_string());
        } else {
            hidden_models.remove(model_id);
        }
        self.set_opencode_hidden_models(profile_id, &hidden_models)?;
        Ok(hidden_models)
    }

    pub fn connection(&self) -> &RefCell<Connection> {
        &self.conn
    }

    pub fn replace_local_chat_turns_for_remote_thread(
        &self,
        remote_thread_id: &str,
        turns: &[LocalChatTurnInput],
    ) -> rusqlite::Result<()> {
        let Some(local_thread_id) = self.local_thread_id_for_remote_thread(remote_thread_id)?
        else {
            return Ok(());
        };

        let mut conn = self.conn.borrow_mut();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM chat_turns
             WHERE local_thread_id = ?1
               AND provider_id = 'codex'",
            params![local_thread_id],
        )?;

        for turn in turns {
            tx.execute(
                "INSERT INTO chat_turns(
                    local_thread_id,
                    provider_id,
                    external_thread_id,
                    external_turn_id,
                    user_text,
                    assistant_text,
                    raw_items_json,
                    status,
                    created_at,
                    completed_at,
                    updated_at
                ) VALUES (?1, 'codex', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    local_thread_id,
                    remote_thread_id,
                    turn.external_turn_id,
                    turn.user_text,
                    turn.assistant_text,
                    turn.raw_items_json,
                    turn.status,
                    turn.created_at,
                    turn.completed_at,
                    unix_now(),
                ],
            )?;
        }

        tx.commit()?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn replace_local_chat_turns_for_codex_thread(
        &self,
        codex_thread_id: &str,
        turns: &[LocalChatTurnInput],
    ) -> rusqlite::Result<()> {
        self.replace_local_chat_turns_for_remote_thread(codex_thread_id, turns)
    }

    pub fn list_local_chat_turns_for_remote_thread(
        &self,
        remote_thread_id: &str,
    ) -> rusqlite::Result<Vec<LocalChatTurnRecord>> {
        let Some(local_thread_id) = self.local_thread_id_for_remote_thread(remote_thread_id)?
        else {
            return Ok(Vec::new());
        };

        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT external_turn_id, user_text, assistant_text, raw_items_json, status, created_at, completed_at
             FROM chat_turns
             WHERE local_thread_id = ?1
               AND provider_id = 'codex'
             ORDER BY created_at ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![local_thread_id], |row| {
            Ok(LocalChatTurnRecord {
                external_turn_id: row.get(0)?,
                user_text: row.get(1)?,
                assistant_text: row.get(2)?,
                raw_items_json: row.get(3)?,
                status: row.get(4)?,
                created_at: row.get(5)?,
                completed_at: row.get(6)?,
            })
        })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    #[allow(dead_code)]
    pub fn list_local_chat_turns_for_codex_thread(
        &self,
        codex_thread_id: &str,
    ) -> rusqlite::Result<Vec<LocalChatTurnRecord>> {
        self.list_local_chat_turns_for_remote_thread(codex_thread_id)
    }

    pub fn local_thread_has_remote_chat_turns(
        &self,
        local_thread_id: i64,
    ) -> rusqlite::Result<bool> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT 1
             FROM chat_turns
             WHERE local_thread_id = ?1
               AND provider_id = 'codex'
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![local_thread_id])?;
        Ok(rows.next()?.is_some())
    }

    #[allow(dead_code)]
    pub fn local_thread_has_codex_chat_turns(
        &self,
        local_thread_id: i64,
    ) -> rusqlite::Result<bool> {
        self.local_thread_has_remote_chat_turns(local_thread_id)
    }

    pub fn workspace_path_for_remote_thread(
        &self,
        remote_thread_id: &str,
    ) -> rusqlite::Result<Option<String>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT CASE
                        WHEN t.worktree_active = 1
                             AND t.worktree_path IS NOT NULL
                             AND TRIM(t.worktree_path) <> ''
                        THEN t.worktree_path
                        ELSE w.path
                    END
             FROM threads t
             JOIN workspaces w ON w.id = t.workspace_id
             WHERE t.codex_thread_id = ?1
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
    pub fn workspace_path_for_codex_thread(
        &self,
        codex_thread_id: &str,
    ) -> rusqlite::Result<Option<String>> {
        self.workspace_path_for_remote_thread(codex_thread_id)
    }

    pub fn workspace_path_for_local_thread(
        &self,
        local_thread_id: i64,
    ) -> rusqlite::Result<Option<String>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT w.path
             FROM threads t
             JOIN workspaces w ON w.id = t.workspace_id
             WHERE t.id = ?1
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![local_thread_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }
}
