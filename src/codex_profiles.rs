use crate::codex_appserver::{AccountInfo, CodexAppServer};
use crate::data::{AppDb, CodexProfileRecord};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone)]
pub struct CodexProfileManager {
    db: Rc<AppDb>,
    clients: Rc<RefCell<HashMap<i64, Arc<CodexAppServer>>>>,
}

impl CodexProfileManager {
    pub fn new(db: Rc<AppDb>) -> Self {
        Self {
            db,
            clients: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    pub fn active_profile_id(&self) -> Option<i64> {
        self.db.active_profile_id().ok().flatten()
    }

    #[allow(dead_code)]
    pub fn set_active_profile(&self, profile_id: i64) -> Result<(), String> {
        self.db
            .set_active_profile_id(profile_id)
            .map_err(|err| err.to_string())?;
        let client = self.ensure_started(profile_id)?;
        self.refresh_profile_account(profile_id, &client);
        Ok(())
    }

    pub fn client_for_profile(&self, profile_id: i64) -> Option<Arc<CodexAppServer>> {
        if let Some(client) = self.clients.borrow().get(&profile_id) {
            return Some(client.clone());
        }
        self.ensure_started(profile_id).ok()
    }

    pub fn running_client_for_profile(&self, profile_id: i64) -> Option<Arc<CodexAppServer>> {
        self.clients.borrow().get(&profile_id).cloned()
    }

    pub fn resolve_running_client_for_thread_id(
        &self,
        codex_thread_id: &str,
    ) -> Option<Arc<CodexAppServer>> {
        let thread = self
            .db
            .get_thread_record_by_codex_thread_id(codex_thread_id)
            .ok()
            .flatten()?;
        let running_ids: Vec<i64> = self
            .running_clients()
            .into_iter()
            .map(|(profile_id, _)| profile_id)
            .collect();
        if running_ids.contains(&thread.profile_id) {
            return self.running_client_for_profile(thread.profile_id);
        }
        if let Some(profile_id) =
            self.match_profile_by_thread_email(&thread.codex_account_email, &running_ids)
        {
            return self.running_client_for_profile(profile_id);
        }
        None
    }

    pub fn resolve_client_for_thread_id(
        &self,
        codex_thread_id: &str,
    ) -> Option<Arc<CodexAppServer>> {
        if let Some(client) = self.resolve_running_client_for_thread_id(codex_thread_id) {
            return Some(client);
        }
        let thread = self
            .db
            .get_thread_record_by_codex_thread_id(codex_thread_id)
            .ok()
            .flatten()?;
        self.client_for_profile(thread.profile_id)
    }

    pub fn switch_runtime_to_thread(&self, codex_thread_id: &str) {
        let thread = self
            .db
            .get_thread_record_by_codex_thread_id(codex_thread_id)
            .ok()
            .flatten();
        let Some(thread) = thread else {
            return;
        };

        let running_ids: Vec<i64> = self
            .running_clients()
            .into_iter()
            .map(|(profile_id, _)| profile_id)
            .collect();
        let runtime_profile_id = if running_ids.contains(&thread.profile_id) {
            thread.profile_id
        } else {
            self.match_profile_by_thread_email(&thread.codex_account_email, &running_ids)
                .unwrap_or(thread.profile_id)
        };
        let _ = self.db.set_runtime_profile_id(runtime_profile_id);
    }

    pub fn ensure_started(&self, profile_id: i64) -> Result<Arc<CodexAppServer>, String> {
        if let Some(existing) = self.clients.borrow().get(&profile_id) {
            return Ok(existing.clone());
        }
        let profile = self
            .db
            .get_codex_profile(profile_id)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| format!("profile {} not found", profile_id))?;
        let home = PathBuf::from(&profile.home_dir);
        let _ = std::fs::create_dir_all(&home);
        let label = format!("{}#{}", profile.name, profile_id);
        let client = CodexAppServer::connect_with_home_and_label(Some(&home), &label)?;
        self.clients.borrow_mut().insert(profile_id, client.clone());
        let _ = self.db.update_codex_profile_status(profile_id, "running");
        self.refresh_profile_account(profile_id, &client);
        Ok(client)
    }

    pub fn stop_profile(&self, profile_id: i64) {
        if let Some(client) = self.clients.borrow_mut().remove(&profile_id) {
            let _ = client.shutdown();
        }
        let _ = self.db.update_codex_profile_status(profile_id, "stopped");
    }

    pub fn shutdown_all(&self) {
        let profile_ids: Vec<i64> = self.clients.borrow().keys().copied().collect();
        for profile_id in profile_ids {
            self.stop_profile(profile_id);
        }
    }

    pub fn restart_profile(&self, profile_id: i64) -> Result<Arc<CodexAppServer>, String> {
        self.stop_profile(profile_id);
        self.ensure_started(profile_id)
    }

    pub fn create_profile(&self, name: &str) -> Result<CodexProfileRecord, String> {
        let base = crate::data::default_app_data_dir().join("codex_profiles");
        let _ = std::fs::create_dir_all(&base);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let dir_name = format!("profile_{now}");
        let home = base.join(dir_name);
        let _ = std::fs::create_dir_all(&home);
        self.db
            .create_codex_profile(name, &home.to_string_lossy())
            .map_err(|err| err.to_string())
    }

    pub fn remove_profile(&self, profile_id: i64) -> Result<(), String> {
        let profile = self
            .db
            .get_codex_profile(profile_id)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| format!("profile {} not found", profile_id))?;
        let system_home =
            crate::data::configured_profile_home_dir(&crate::data::default_app_data_dir());
        let system_home = system_home.to_string_lossy().to_string();
        if system_home.trim().eq(profile.home_dir.trim()) {
            return Err("system profile cannot be removed".to_string());
        }

        let fallback_profile_id = self
            .db
            .list_codex_profiles()
            .map_err(|err| err.to_string())?
            .into_iter()
            .find(|candidate| candidate.id != profile_id)
            .map(|candidate| candidate.id)
            .ok_or_else(|| "at least one profile must remain".to_string())?;

        self.stop_profile(profile_id);
        self.db
            .remove_codex_profile(profile_id, fallback_profile_id)
            .map_err(|err| err.to_string())?;

        let base = crate::data::default_app_data_dir().join("codex_profiles");
        let profile_home = PathBuf::from(&profile.home_dir);
        let base_canon = std::fs::canonicalize(&base).unwrap_or(base);
        let home_canon = std::fs::canonicalize(&profile_home).unwrap_or(profile_home.clone());
        if home_canon.starts_with(&base_canon) {
            let _ = std::fs::remove_dir_all(&home_canon);
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn poll_accounts(&self) {
        let running: Vec<(i64, Arc<CodexAppServer>)> = self
            .clients
            .borrow()
            .iter()
            .map(|(id, client)| (*id, client.clone()))
            .collect();
        for (profile_id, client) in running {
            self.refresh_profile_account(profile_id, &client);
        }
    }

    pub fn running_clients(&self) -> Vec<(i64, Arc<CodexAppServer>)> {
        self.clients
            .borrow()
            .iter()
            .map(|(id, client)| (*id, client.clone()))
            .collect()
    }

    fn refresh_profile_account(&self, profile_id: i64, client: &Arc<CodexAppServer>) {
        let account = client.account_read(false).ok().flatten();
        let (account_type, email): (Option<String>, Option<String>) = match account {
            Some(AccountInfo {
                account_type,
                email,
            }) => (Some(account_type), email),
            None => (None, None),
        };
        let _ = self.db.update_codex_profile_account(
            profile_id,
            account_type.as_deref(),
            email.as_deref(),
        );
        if self.active_profile_id() == Some(profile_id) {
            let _ = self
                .db
                .set_current_codex_account(account_type.as_deref(), email.as_deref());
        }
    }

    fn match_profile_by_thread_email(
        &self,
        thread_email: &Option<String>,
        candidate_profile_ids: &[i64],
    ) -> Option<i64> {
        let target = thread_email
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .filter(|value| !value.is_empty())?;
        for profile_id in candidate_profile_ids {
            let profile = self.db.get_codex_profile(*profile_id).ok().flatten()?;
            let profile_email = profile
                .last_email
                .as_deref()
                .map(str::trim)
                .map(str::to_ascii_lowercase)
                .filter(|value| !value.is_empty());
            if profile_email.as_deref() == Some(target.as_str()) {
                return Some(*profile_id);
            }
        }
        None
    }
}
