use super::*;
use rusqlite::params;

impl AppDb {
    pub fn remote_mode_enabled(&self) -> bool {
        crate::remote::bool_from_setting(
            self.get_setting(crate::remote::SETTING_REMOTE_MODE_ENABLED)
                .ok()
                .flatten(),
            false,
        )
    }

    pub fn set_remote_mode_enabled(&self, enabled: bool) -> rusqlite::Result<()> {
        self.set_setting(
            crate::remote::SETTING_REMOTE_MODE_ENABLED,
            if enabled { "1" } else { "0" },
        )
    }

    pub fn set_remote_telegram_active_account_id(
        &self,
        account_id: Option<i64>,
    ) -> rusqlite::Result<()> {
        if let Some(account_id) = account_id {
            self.set_setting(
                crate::remote::SETTING_REMOTE_TELEGRAM_ACTIVE_ACCOUNT_ID,
                &account_id.to_string(),
            )
        } else {
            self.conn.borrow_mut().execute(
                "DELETE FROM settings WHERE key = ?1",
                params![crate::remote::SETTING_REMOTE_TELEGRAM_ACTIVE_ACCOUNT_ID],
            )?;
            Ok(())
        }
    }

    pub fn remote_telegram_active_account(
        &self,
    ) -> rusqlite::Result<Option<RemoteTelegramAccountRecord>> {
        let active_id = self
            .get_setting(crate::remote::SETTING_REMOTE_TELEGRAM_ACTIVE_ACCOUNT_ID)?
            .and_then(|value| value.parse::<i64>().ok());
        if let Some(active_id) = active_id {
            if let Some(account) = self.remote_telegram_account_by_id(active_id)? {
                return Ok(Some(account));
            }
        }

        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, bot_token, telegram_user_id, telegram_chat_id, telegram_username, linked_at, updated_at
             FROM remote_telegram_accounts
             ORDER BY updated_at DESC, id DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            let account = RemoteTelegramAccountRecord {
                id: row.get(0)?,
                bot_token: row.get(1)?,
                telegram_user_id: row.get(2)?,
                telegram_chat_id: row.get(3)?,
                telegram_username: row.get(4)?,
                linked_at: row.get(5)?,
                updated_at: row.get(6)?,
            };
            drop(rows);
            drop(stmt);
            drop(conn);
            let _ = self.set_remote_telegram_active_account_id(Some(account.id));
            return Ok(Some(account));
        }
        Ok(None)
    }

    pub fn remote_telegram_account_by_id(
        &self,
        account_id: i64,
    ) -> rusqlite::Result<Option<RemoteTelegramAccountRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, bot_token, telegram_user_id, telegram_chat_id, telegram_username, linked_at, updated_at
             FROM remote_telegram_accounts
             WHERE id = ?1
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![account_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(RemoteTelegramAccountRecord {
                id: row.get(0)?,
                bot_token: row.get(1)?,
                telegram_user_id: row.get(2)?,
                telegram_chat_id: row.get(3)?,
                telegram_username: row.get(4)?,
                linked_at: row.get(5)?,
                updated_at: row.get(6)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn upsert_remote_telegram_account(
        &self,
        bot_token: &str,
        telegram_user_id: &str,
        telegram_chat_id: &str,
        telegram_username: Option<&str>,
    ) -> rusqlite::Result<RemoteTelegramAccountRecord> {
        let now = unix_now();
        self.conn.borrow_mut().execute(
            "INSERT INTO remote_telegram_accounts(
                bot_token, telegram_user_id, telegram_chat_id, telegram_username, linked_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
            ON CONFLICT(telegram_user_id, telegram_chat_id) DO UPDATE SET
                bot_token = excluded.bot_token,
                telegram_username = excluded.telegram_username,
                updated_at = excluded.updated_at",
            params![
                bot_token.trim(),
                telegram_user_id.trim(),
                telegram_chat_id.trim(),
                telegram_username.map(str::trim),
                now
            ],
        )?;

        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, bot_token, telegram_user_id, telegram_chat_id, telegram_username, linked_at, updated_at
             FROM remote_telegram_accounts
             WHERE telegram_user_id = ?1
               AND telegram_chat_id = ?2
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![telegram_user_id.trim(), telegram_chat_id.trim()])?;
        if let Some(row) = rows.next()? {
            let account = RemoteTelegramAccountRecord {
                id: row.get(0)?,
                bot_token: row.get(1)?,
                telegram_user_id: row.get(2)?,
                telegram_chat_id: row.get(3)?,
                telegram_username: row.get(4)?,
                linked_at: row.get(5)?,
                updated_at: row.get(6)?,
            };
            drop(rows);
            drop(stmt);
            drop(conn);

            self.set_remote_telegram_active_account_id(Some(account.id))?;
            self.set_setting(crate::remote::SETTING_REMOTE_TELEGRAM_POLLING_ENABLED, "1")?;
            return Ok(account);
        }

        Err(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn delete_remote_telegram_account(&self, account_id: i64) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "DELETE FROM remote_telegram_accounts WHERE id = ?1",
            params![account_id],
        )?;
        let active_id = self
            .get_setting(crate::remote::SETTING_REMOTE_TELEGRAM_ACTIVE_ACCOUNT_ID)?
            .and_then(|value| value.parse::<i64>().ok());
        if active_id == Some(account_id) {
            self.set_remote_telegram_active_account_id(None)?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn upsert_remote_telegram_message_map(
        &self,
        telegram_chat_id: &str,
        telegram_message_id: &str,
        local_thread_id: i64,
        codex_thread_id: Option<&str>,
        local_turn_id: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "INSERT INTO remote_telegram_message_map(
                telegram_chat_id,
                telegram_message_id,
                local_thread_id,
                codex_thread_id,
                local_turn_id,
                created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(telegram_chat_id, telegram_message_id) DO UPDATE SET
                local_thread_id = excluded.local_thread_id,
                codex_thread_id = excluded.codex_thread_id,
                local_turn_id = excluded.local_turn_id",
            params![
                telegram_chat_id.trim(),
                telegram_message_id.trim(),
                local_thread_id,
                codex_thread_id,
                local_turn_id,
                unix_now()
            ],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn local_thread_id_for_remote_telegram_reply(
        &self,
        telegram_chat_id: &str,
        telegram_message_id: &str,
    ) -> rusqlite::Result<Option<i64>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT local_thread_id
             FROM remote_telegram_message_map
             WHERE telegram_chat_id = ?1
               AND telegram_message_id = ?2
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![telegram_chat_id.trim(), telegram_message_id.trim()])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    pub fn enqueue_remote_pending_prompt(
        &self,
        local_thread_id: i64,
        text: &str,
        source: &str,
        telegram_chat_id: Option<&str>,
        telegram_message_id: Option<&str>,
        telegram_user_id: Option<&str>,
        telegram_username: Option<&str>,
    ) -> rusqlite::Result<i64> {
        let now = unix_now();
        let clean_text = text.trim();
        if clean_text.is_empty() {
            return Err(rusqlite::Error::InvalidQuery);
        }
        self.conn.borrow_mut().execute(
            "INSERT INTO remote_pending_prompts(
                local_thread_id,
                source,
                telegram_chat_id,
                telegram_message_id,
                telegram_user_id,
                telegram_username,
                text,
                created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                local_thread_id,
                source.trim(),
                telegram_chat_id.map(str::trim),
                telegram_message_id.map(str::trim),
                telegram_user_id.map(str::trim),
                telegram_username.map(str::trim),
                clean_text,
                now
            ],
        )?;
        Ok(self.conn.borrow().last_insert_rowid())
    }

    pub fn list_remote_pending_prompts_for_local_thread(
        &self,
        local_thread_id: i64,
        limit: usize,
    ) -> rusqlite::Result<Vec<RemotePendingPromptRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, local_thread_id, source, telegram_chat_id, telegram_message_id, telegram_user_id, telegram_username, text, created_at, consumed_at
             FROM remote_pending_prompts
             WHERE local_thread_id = ?1
               AND consumed_at IS NULL
             ORDER BY created_at ASC, id ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![local_thread_id, limit.max(1) as i64], |row| {
            Ok(RemotePendingPromptRecord {
                id: row.get(0)?,
                local_thread_id: row.get(1)?,
                source: row.get(2)?,
                telegram_chat_id: row.get(3)?,
                telegram_message_id: row.get(4)?,
                telegram_user_id: row.get(5)?,
                telegram_username: row.get(6)?,
                text: row.get(7)?,
                created_at: row.get(8)?,
                consumed_at: row.get(9)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn mark_remote_pending_prompt_consumed(&self, prompt_id: i64) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "UPDATE remote_pending_prompts
             SET consumed_at = ?1
             WHERE id = ?2",
            params![unix_now(), prompt_id],
        )?;
        Ok(())
    }
}
