use super::*;
use rusqlite::params;

impl AppDb {
    pub fn enzim_agent_config(&self) -> rusqlite::Result<Option<EnzimAgentConfigRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT base_url, api_key, model_id, system_prompt_override, cached_models_json,
                    cached_models_refreshed_at, updated_at
             FROM enzim_agent_config
             WHERE id = 1
             LIMIT 1",
        )?;
        let mut rows = stmt.query([])?;
        if let Some(row) = rows.next()? {
            Ok(Some(EnzimAgentConfigRecord {
                base_url: row.get(0)?,
                api_key: row.get(1)?,
                model_id: row.get(2)?,
                system_prompt_override: row.get(3)?,
                cached_models_json: row.get(4)?,
                cached_models_refreshed_at: row.get(5)?,
                updated_at: row.get(6)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn upsert_enzim_agent_config(
        &self,
        config: &EnzimAgentConfigRecord,
    ) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "INSERT INTO enzim_agent_config(
                id, base_url, api_key, model_id, system_prompt_override,
                cached_models_json, cached_models_refreshed_at, updated_at
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                base_url = excluded.base_url,
                api_key = excluded.api_key,
                model_id = excluded.model_id,
                system_prompt_override = excluded.system_prompt_override,
                cached_models_json = excluded.cached_models_json,
                cached_models_refreshed_at = excluded.cached_models_refreshed_at,
                updated_at = excluded.updated_at",
            params![
                config.base_url,
                config.api_key,
                config.model_id,
                config.system_prompt_override,
                config.cached_models_json,
                config.cached_models_refreshed_at,
                config.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn create_enzim_agent_loop(
        &self,
        local_thread_id: i64,
        status: &str,
        prompt_text: &str,
        instructions_text: &str,
        backend_kind: &str,
        remote_thread_id_snapshot: Option<&str>,
        config_base_url_snapshot: &str,
        config_model_id_snapshot: &str,
        system_prompt_snapshot: &str,
    ) -> rusqlite::Result<EnzimAgentLoopRecord> {
        let now = unix_now();
        self.conn.borrow_mut().execute(
            "INSERT INTO enzim_agent_loops(
                local_thread_id, status, prompt_text, instructions_text, backend_kind,
                remote_thread_id_snapshot, config_base_url_snapshot, config_model_id_snapshot,
                system_prompt_snapshot, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            params![
                local_thread_id,
                status,
                prompt_text,
                instructions_text,
                backend_kind,
                remote_thread_id_snapshot,
                config_base_url_snapshot,
                config_model_id_snapshot,
                system_prompt_snapshot,
                now,
            ],
        )?;
        let id = self.conn.borrow().last_insert_rowid();
        self.get_enzim_agent_loop(id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    pub fn get_enzim_agent_loop(
        &self,
        loop_id: i64,
    ) -> rusqlite::Result<Option<EnzimAgentLoopRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, local_thread_id, status, prompt_text, instructions_text, backend_kind,
                    remote_thread_id_snapshot, config_base_url_snapshot, config_model_id_snapshot,
                    system_prompt_snapshot, iteration_count, error_count, last_seen_external_turn_id,
                    final_summary_text, last_error_text, created_at, updated_at, finished_at
             FROM enzim_agent_loops
             WHERE id = ?1
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![loop_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(EnzimAgentLoopRecord {
                id: row.get(0)?,
                local_thread_id: row.get(1)?,
                status: row.get(2)?,
                prompt_text: row.get(3)?,
                instructions_text: row.get(4)?,
                backend_kind: row.get(5)?,
                remote_thread_id_snapshot: row.get(6)?,
                config_base_url_snapshot: row.get(7)?,
                config_model_id_snapshot: row.get(8)?,
                system_prompt_snapshot: row.get(9)?,
                iteration_count: row.get(10)?,
                error_count: row.get(11)?,
                last_seen_external_turn_id: row.get(12)?,
                final_summary_text: row.get(13)?,
                last_error_text: row.get(14)?,
                created_at: row.get(15)?,
                updated_at: row.get(16)?,
                finished_at: row.get(17)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn active_enzim_agent_loop_for_local_thread(
        &self,
        local_thread_id: i64,
    ) -> rusqlite::Result<Option<EnzimAgentLoopRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, local_thread_id, status, prompt_text, instructions_text, backend_kind,
                    remote_thread_id_snapshot, config_base_url_snapshot, config_model_id_snapshot,
                    system_prompt_snapshot, iteration_count, error_count, last_seen_external_turn_id,
                    final_summary_text, last_error_text, created_at, updated_at, finished_at
             FROM enzim_agent_loops
             WHERE local_thread_id = ?1
               AND status IN ('active', 'waiting_runtime', 'evaluating', 'waiting_user')
             ORDER BY id DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![local_thread_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(EnzimAgentLoopRecord {
                id: row.get(0)?,
                local_thread_id: row.get(1)?,
                status: row.get(2)?,
                prompt_text: row.get(3)?,
                instructions_text: row.get(4)?,
                backend_kind: row.get(5)?,
                remote_thread_id_snapshot: row.get(6)?,
                config_base_url_snapshot: row.get(7)?,
                config_model_id_snapshot: row.get(8)?,
                system_prompt_snapshot: row.get(9)?,
                iteration_count: row.get(10)?,
                error_count: row.get(11)?,
                last_seen_external_turn_id: row.get(12)?,
                final_summary_text: row.get(13)?,
                last_error_text: row.get(14)?,
                created_at: row.get(15)?,
                updated_at: row.get(16)?,
                finished_at: row.get(17)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn latest_enzim_agent_loop_for_local_thread(
        &self,
        local_thread_id: i64,
    ) -> rusqlite::Result<Option<EnzimAgentLoopRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, local_thread_id, status, prompt_text, instructions_text, backend_kind,
                    remote_thread_id_snapshot, config_base_url_snapshot, config_model_id_snapshot,
                    system_prompt_snapshot, iteration_count, error_count, last_seen_external_turn_id,
                    final_summary_text, last_error_text, created_at, updated_at, finished_at
             FROM enzim_agent_loops
             WHERE local_thread_id = ?1
             ORDER BY id DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query(params![local_thread_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(EnzimAgentLoopRecord {
                id: row.get(0)?,
                local_thread_id: row.get(1)?,
                status: row.get(2)?,
                prompt_text: row.get(3)?,
                instructions_text: row.get(4)?,
                backend_kind: row.get(5)?,
                remote_thread_id_snapshot: row.get(6)?,
                config_base_url_snapshot: row.get(7)?,
                config_model_id_snapshot: row.get(8)?,
                system_prompt_snapshot: row.get(9)?,
                iteration_count: row.get(10)?,
                error_count: row.get(11)?,
                last_seen_external_turn_id: row.get(12)?,
                final_summary_text: row.get(13)?,
                last_error_text: row.get(14)?,
                created_at: row.get(15)?,
                updated_at: row.get(16)?,
                finished_at: row.get(17)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn list_active_enzim_agent_loops(&self) -> rusqlite::Result<Vec<EnzimAgentLoopRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, local_thread_id, status, prompt_text, instructions_text, backend_kind,
                    remote_thread_id_snapshot, config_base_url_snapshot, config_model_id_snapshot,
                    system_prompt_snapshot, iteration_count, error_count, last_seen_external_turn_id,
                    final_summary_text, last_error_text, created_at, updated_at, finished_at
             FROM enzim_agent_loops
             WHERE status IN ('active', 'waiting_runtime', 'evaluating', 'waiting_user')
             ORDER BY updated_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(EnzimAgentLoopRecord {
                id: row.get(0)?,
                local_thread_id: row.get(1)?,
                status: row.get(2)?,
                prompt_text: row.get(3)?,
                instructions_text: row.get(4)?,
                backend_kind: row.get(5)?,
                remote_thread_id_snapshot: row.get(6)?,
                config_base_url_snapshot: row.get(7)?,
                config_model_id_snapshot: row.get(8)?,
                system_prompt_snapshot: row.get(9)?,
                iteration_count: row.get(10)?,
                error_count: row.get(11)?,
                last_seen_external_turn_id: row.get(12)?,
                final_summary_text: row.get(13)?,
                last_error_text: row.get(14)?,
                created_at: row.get(15)?,
                updated_at: row.get(16)?,
                finished_at: row.get(17)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn update_enzim_agent_loop_status(
        &self,
        loop_id: i64,
        status: &str,
        last_error_text: Option<&str>,
        final_summary_text: Option<&str>,
        finished_at: Option<i64>,
    ) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "UPDATE enzim_agent_loops
             SET status = ?1,
                 last_error_text = COALESCE(?2, last_error_text),
                 final_summary_text = COALESCE(?3, final_summary_text),
                 finished_at = COALESCE(?4, finished_at),
                 updated_at = ?5
             WHERE id = ?6",
            params![
                status,
                last_error_text,
                final_summary_text,
                finished_at,
                unix_now(),
                loop_id
            ],
        )?;
        Ok(())
    }

    pub fn update_enzim_agent_loop_progress(
        &self,
        loop_id: i64,
        status: &str,
        last_seen_external_turn_id: Option<&str>,
        iteration_delta: i64,
        error_delta: i64,
        last_error_text: Option<&str>,
        final_summary_text: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "UPDATE enzim_agent_loops
             SET status = ?1,
                 last_seen_external_turn_id = COALESCE(?2, last_seen_external_turn_id),
                 iteration_count = iteration_count + ?3,
                 error_count = error_count + ?4,
                 last_error_text = ?5,
                 final_summary_text = COALESCE(?6, final_summary_text),
                 updated_at = ?7
             WHERE id = ?8",
            params![
                status,
                last_seen_external_turn_id,
                iteration_delta,
                error_delta,
                last_error_text,
                final_summary_text,
                unix_now(),
                loop_id
            ],
        )?;
        Ok(())
    }

    pub fn append_enzim_agent_loop_event(
        &self,
        loop_id: i64,
        event_kind: &str,
        author_kind: &str,
        external_turn_id: Option<&str>,
        full_text: Option<&str>,
        compact_text: Option<&str>,
        decision_json: Option<&str>,
    ) -> rusqlite::Result<EnzimAgentLoopEventRecord> {
        let sequence_no = self.next_enzim_agent_loop_event_sequence(loop_id)?;
        let now = unix_now();
        self.conn.borrow_mut().execute(
            "INSERT INTO enzim_agent_loop_events(
                loop_id, sequence_no, event_kind, author_kind, external_turn_id,
                full_text, compact_text, decision_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                loop_id,
                sequence_no,
                event_kind,
                author_kind,
                external_turn_id,
                full_text,
                compact_text,
                decision_json,
                now
            ],
        )?;
        let id = self.conn.borrow().last_insert_rowid();
        Ok(EnzimAgentLoopEventRecord {
            id,
            loop_id,
            sequence_no,
            event_kind: event_kind.to_string(),
            author_kind: author_kind.to_string(),
            external_turn_id: external_turn_id.map(ToOwned::to_owned),
            full_text: full_text.map(ToOwned::to_owned),
            compact_text: compact_text.map(ToOwned::to_owned),
            decision_json: decision_json.map(ToOwned::to_owned),
            created_at: now,
        })
    }

    fn next_enzim_agent_loop_event_sequence(&self, loop_id: i64) -> rusqlite::Result<i64> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(MAX(sequence_no), 0) + 1
             FROM enzim_agent_loop_events
             WHERE loop_id = ?1",
        )?;
        stmt.query_row(params![loop_id], |row| row.get(0))
    }

    pub fn list_enzim_agent_loop_events(
        &self,
        loop_id: i64,
    ) -> rusqlite::Result<Vec<EnzimAgentLoopEventRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT id, loop_id, sequence_no, event_kind, author_kind, external_turn_id,
                    full_text, compact_text, decision_json, created_at
             FROM enzim_agent_loop_events
             WHERE loop_id = ?1
             ORDER BY sequence_no ASC, id ASC",
        )?;
        let rows = stmt.query_map(params![loop_id], |row| {
            Ok(EnzimAgentLoopEventRecord {
                id: row.get(0)?,
                loop_id: row.get(1)?,
                sequence_no: row.get(2)?,
                event_kind: row.get(3)?,
                author_kind: row.get(4)?,
                external_turn_id: row.get(5)?,
                full_text: row.get(6)?,
                compact_text: row.get(7)?,
                decision_json: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn update_enzim_agent_loop_event(
        &self,
        event_id: i64,
        external_turn_id: Option<&str>,
        compact_text: Option<&str>,
        decision_json: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.borrow_mut().execute(
            "UPDATE enzim_agent_loop_events
             SET external_turn_id = COALESCE(?1, external_turn_id),
                 compact_text = COALESCE(?2, compact_text),
                 decision_json = COALESCE(?3, decision_json)
             WHERE id = ?4",
            params![external_turn_id, compact_text, decision_json, event_id],
        )?;
        Ok(())
    }

    pub fn mark_enzim_agent_turn_origin(
        &self,
        thread_id: &str,
        turn_id: &str,
        origin: &str,
    ) -> rusqlite::Result<()> {
        self.set_setting(
            &format!("enzim_agent:thread:{thread_id}:turn_origin:{turn_id}"),
            origin,
        )
    }

    pub fn enzim_agent_turn_origin(
        &self,
        thread_id: &str,
        turn_id: &str,
    ) -> rusqlite::Result<Option<String>> {
        self.get_setting(&format!("enzim_agent:thread:{thread_id}:turn_origin:{turn_id}"))
    }

    pub fn list_local_chat_turns_for_local_thread(
        &self,
        local_thread_id: i64,
    ) -> rusqlite::Result<Vec<LocalChatTurnRecord>> {
        let conn = self.conn.borrow();
        let mut stmt = conn.prepare(
            "SELECT external_turn_id, user_text, assistant_text, raw_items_json, status, created_at, completed_at
             FROM chat_turns
             WHERE local_thread_id = ?1
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
}
