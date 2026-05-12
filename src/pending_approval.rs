use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::approval::Approval;
use crate::error::AppError;

const STORE_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const STORE_LOCK_RETRY_DELAY: Duration = Duration::from_millis(10);

#[derive(Debug, Default, Serialize, Deserialize)]
struct PendingApprovalStore {
    approvals: Vec<Approval>,
}

pub fn persist_pending_approval(approval: &Approval) -> Result<(), AppError> {
    persist_pending_approval_at_path(&store_path()?, approval)
}

pub fn list_pending_approvals() -> Result<Vec<Approval>, AppError> {
    list_pending_approvals_at_path(&store_path()?)
}

pub fn load_pending_approval(reference: &str) -> Result<Option<Approval>, AppError> {
    load_pending_approval_at_path(&store_path()?, reference)
}

pub fn save_pending_approval(approval: &Approval) -> Result<(), AppError> {
    persist_pending_approval(approval)
}

pub fn remove_pending_approval(reference: &str) -> Result<(), AppError> {
    remove_pending_approval_at_path(&store_path()?, reference)
}

fn persist_pending_approval_at_path(path: &Path, approval: &Approval) -> Result<(), AppError> {
    let _lock = acquire_store_lock(path)?;
    let mut store = load_store(path)?;
    upsert_approval(&mut store.approvals, approval.clone());
    save_store(path, &store)
}

fn list_pending_approvals_at_path(path: &Path) -> Result<Vec<Approval>, AppError> {
    let _lock = acquire_store_lock(path)?;
    let mut approvals = load_store(path)?.approvals;
    approvals.sort_by(|left, right| {
        right
            .requested_at
            .cmp(&left.requested_at)
            .then_with(|| left.approval_id.cmp(&right.approval_id))
    });
    Ok(approvals)
}

fn load_pending_approval_at_path(
    path: &Path,
    reference: &str,
) -> Result<Option<Approval>, AppError> {
    let _lock = acquire_store_lock(path)?;
    let store = load_store(path)?;
    Ok(find_approval(&store.approvals, reference)?.cloned())
}

fn remove_pending_approval_at_path(path: &Path, reference: &str) -> Result<(), AppError> {
    let _lock = acquire_store_lock(path)?;
    let mut store = load_store(path)?;
    let Some(target_resume_token) =
        find_approval(&store.approvals, reference)?.map(|approval| approval.resume_token.clone())
    else {
        if !path.exists() {
            return Ok(());
        }
        save_store(path, &store)?;
        return Ok(());
    };
    store
        .approvals
        .retain(|approval| approval.resume_token != target_resume_token);
    save_store(path, &store)
}

fn upsert_approval(approvals: &mut Vec<Approval>, approval: Approval) {
    approvals.retain(|existing| existing.resume_token != approval.resume_token);
    approvals.push(approval);
}

fn find_approval<'a>(
    approvals: &'a [Approval],
    reference: &str,
) -> Result<Option<&'a Approval>, AppError> {
    if let Some(approval) = approvals
        .iter()
        .find(|approval| approval.resume_token == reference)
    {
        return Ok(Some(approval));
    }

    let matches = approvals
        .iter()
        .filter(|approval| approval.approval_id == reference)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [approval] => Ok(Some(*approval)),
        approvals => {
            let resume_tokens = approvals
                .iter()
                .map(|approval| approval.resume_token.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            Err(AppError::protocol(
                "approval",
                format!(
                    "multiple pending approvals share id {reference}; use one of the resume tokens instead: {resume_tokens}"
                ),
            ))
        }
    }
}

fn store_path() -> Result<PathBuf, AppError> {
    dirs::config_dir()
        .map(|dir| dir.join("codex-app-server-client-cli/pending-approvals.json"))
        .ok_or_else(|| {
            AppError::protocol(
                "approval",
                "local config directory unavailable for pending approval storage",
            )
        })
}

fn load_store(path: &Path) -> Result<PendingApprovalStore, AppError> {
    if !path.exists() {
        return Ok(PendingApprovalStore::default());
    }

    let content = fs::read_to_string(path)
        .map_err(|source| AppError::config_io(path.to_path_buf(), source))?;
    serde_json::from_str(&content).map_err(|source| {
        AppError::protocol(
            "approval",
            format!(
                "failed to parse pending approval store at {}: {source}",
                path.display()
            ),
        )
    })
}

fn save_store(path: &Path, store: &PendingApprovalStore) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| AppError::config_io(parent.to_path_buf(), source))?;
    }
    let content = serde_json::to_string_pretty(store).map_err(AppError::json)?;
    let temp_path = temporary_store_path(path);
    fs::write(&temp_path, content)
        .map_err(|source| AppError::config_io(temp_path.clone(), source))?;
    fs::rename(&temp_path, path).map_err(|source| AppError::config_io(path.to_path_buf(), source))
}

fn temporary_store_path(path: &Path) -> PathBuf {
    let mut os = OsString::from(path.as_os_str());
    os.push(format!(".{}.tmp", std::process::id()));
    PathBuf::from(os)
}

fn lock_path(path: &Path) -> PathBuf {
    let mut os = OsString::from(path.as_os_str());
    os.push(".lock");
    PathBuf::from(os)
}

fn acquire_store_lock(path: &Path) -> Result<StoreLock, AppError> {
    let lock_path = lock_path(path);
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|source| AppError::config_io(parent.to_path_buf(), source))?;
    }

    let deadline = Instant::now() + STORE_LOCK_TIMEOUT;
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(file) => {
                return Ok(StoreLock {
                    path: lock_path,
                    _file: file,
                });
            }
            Err(source)
                if source.kind() == ErrorKind::AlreadyExists && Instant::now() < deadline =>
            {
                thread::sleep(STORE_LOCK_RETRY_DELAY);
            }
            Err(source) if source.kind() == ErrorKind::AlreadyExists => {
                return Err(AppError::protocol(
                    "approval",
                    format!(
                        "timed out waiting for pending approval store lock at {}",
                        lock_path.display()
                    ),
                ));
            }
            Err(source) => return Err(AppError::config_io(lock_path.clone(), source)),
        }
    }
}

struct StoreLock {
    path: PathBuf,
    _file: File,
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;
    use crate::approval::{ApprovalScope, ApprovalStatus};
    use crate::protocol::messages::RequestId;

    fn temp_store_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should move forward")
            .as_nanos();
        std::env::temp_dir().join(format!("codex_cli_pending_approval_{label}_{nonce}.json"))
    }

    fn approval(session_id: &str, approval_id: &str, resume_token: &str) -> Approval {
        Approval {
            approval_id: approval_id.to_owned(),
            session_id: Some(session_id.to_owned()),
            scope: ApprovalScope::CommandExecution,
            risk_traits: vec!["shell_exec".to_owned()],
            summary: format!("approval for {session_id}"),
            requested_action: "run command".to_owned(),
            requested_at: "2026-05-12T00:00:00Z".to_owned(),
            expires_at: None,
            resume_token: resume_token.to_owned(),
            status: ApprovalStatus::Pending,
            raw_method: "item/commandExecution/requestApproval".to_owned(),
            request_id: RequestId::String(approval_id.to_owned()),
            item_id: Some(format!("item-{session_id}")),
            data: json!({}),
        }
    }

    #[test]
    fn approvals_with_shared_ids_require_resume_token_disambiguation() {
        let path = temp_store_path("shared_ids");
        let first = approval("sess-a", "approval-1", "sess-a:approval-1");
        let second = approval("sess-b", "approval-1", "sess-b:approval-1");

        persist_pending_approval_at_path(&path, &first).expect("persist first approval");
        persist_pending_approval_at_path(&path, &second).expect("persist second approval");

        let loaded_first = load_pending_approval_at_path(&path, "sess-a:approval-1")
            .expect("load first by resume token")
            .expect("first approval present");
        assert_eq!(loaded_first.session_id.as_deref(), Some("sess-a"));

        let err = load_pending_approval_at_path(&path, "approval-1")
            .expect_err("shared approval ids should require resume token");
        assert!(
            err.to_string()
                .contains("multiple pending approvals share id approval-1")
        );

        remove_pending_approval_at_path(&path, "sess-a:approval-1")
            .expect("remove first approval by token");
        let remaining = list_pending_approvals_at_path(&path).expect("list remaining approvals");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].session_id.as_deref(), Some("sess-b"));
    }

    #[test]
    fn concurrent_persist_operations_do_not_corrupt_store() {
        let path = temp_store_path("concurrent_writes");
        let first = approval("sess-a", "approval-1", "sess-a:approval-1");
        let second = approval("sess-b", "approval-1", "sess-b:approval-1");

        let first_path = path.clone();
        let first_thread = thread::spawn(move || {
            for _ in 0..50 {
                persist_pending_approval_at_path(&first_path, &first)
                    .expect("persist first approval");
            }
        });
        let second_path = path.clone();
        let second_thread = thread::spawn(move || {
            for _ in 0..50 {
                persist_pending_approval_at_path(&second_path, &second)
                    .expect("persist second approval");
            }
        });

        first_thread.join().expect("first thread should complete");
        second_thread.join().expect("second thread should complete");

        let approvals =
            list_pending_approvals_at_path(&path).expect("list approvals after concurrent writes");
        assert_eq!(approvals.len(), 2);
        assert!(
            approvals
                .iter()
                .any(|approval| approval.resume_token == "sess-a:approval-1")
        );
        assert!(
            approvals
                .iter()
                .any(|approval| approval.resume_token == "sess-b:approval-1")
        );
    }
}
