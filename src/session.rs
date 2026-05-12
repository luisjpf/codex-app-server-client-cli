use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Session identity helpers for the v1 CLI.
///
/// Protocol assumption: the underlying app-server still speaks in terms of
/// thread ids (`thread/start`, `thread/resume`, `thread/list`), while the CLI
/// exposes a higher-level session concept. In v1 we therefore treat a CLI
/// session id as the server's opaque thread id, add optional human aliases on
/// top, and derive workspace identity locally from the filesystem until the
/// server offers a first-class workspace identifier.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionDescriptor {
    pub id: String,
    pub alias: Option<String>,
    pub workspace_root: PathBuf,
    pub repo_root: Option<PathBuf>,
    pub ephemeral: bool,
    pub yolo: bool,
    pub last_active_at: Option<String>,
}

impl SessionDescriptor {
    pub fn matches_workspace(&self, binding: &WorkspaceBinding) -> bool {
        self.workspace_root == binding.workspace_root
    }

    pub fn alias_key(&self) -> Option<&str> {
        self.alias.as_deref()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceBindingKind {
    RepoRoot,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceBinding {
    pub requested_cwd: PathBuf,
    pub resolved_cwd: PathBuf,
    pub workspace_root: PathBuf,
    pub repo_root: Option<PathBuf>,
    pub binding_kind: WorkspaceBindingKind,
}

impl WorkspaceBinding {
    pub fn discover(path: impl AsRef<Path>) -> Result<Self, SessionError> {
        let requested_cwd = path.as_ref().to_path_buf();
        let resolved_cwd =
            fs::canonicalize(&requested_cwd).map_err(|source| SessionError::PathIo {
                path: requested_cwd.clone(),
                source,
            })?;

        let repo_root = find_repo_root(&resolved_cwd)?;
        let workspace_root = repo_root.clone().unwrap_or_else(|| resolved_cwd.clone());
        let binding_kind = if repo_root.is_some() {
            WorkspaceBindingKind::RepoRoot
        } else {
            WorkspaceBindingKind::Directory
        };

        Ok(Self {
            requested_cwd,
            resolved_cwd,
            workspace_root,
            repo_root,
            binding_kind,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionReferenceKind {
    Id,
    Alias,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionReference {
    pub raw: String,
    pub kind: SessionReferenceKind,
}

impl SessionReference {
    pub fn parse(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let kind = if looks_like_session_id(&raw) {
            SessionReferenceKind::Id
        } else {
            SessionReferenceKind::Alias
        };
        Self { raw, kind }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionDraft {
    pub workspace_root: PathBuf,
    pub repo_root: Option<PathBuf>,
    pub ephemeral: bool,
    pub history_mode: SessionHistoryMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionHistoryMode {
    ResumePrior,
    CleanWorkspaceIdentity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SessionSelection {
    Reuse {
        session: SessionDescriptor,
        reason: SessionSelectionReason,
    },
    Create {
        draft: SessionDraft,
        reason: SessionSelectionReason,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionSelectionReason {
    ExplicitEphemeral,
    WorkspaceScopedDefault,
    NoWorkspaceMatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionIndex<'a> {
    by_id: HashMap<&'a str, &'a SessionDescriptor>,
    by_alias: HashMap<&'a str, Vec<&'a SessionDescriptor>>,
}

impl<'a> SessionIndex<'a> {
    pub fn new(sessions: &'a [SessionDescriptor]) -> Self {
        let mut by_id = HashMap::new();
        let mut by_alias: HashMap<&'a str, Vec<&'a SessionDescriptor>> = HashMap::new();

        for session in sessions {
            by_id.insert(session.id.as_str(), session);
            if let Some(alias) = session.alias_key() {
                by_alias.entry(alias).or_default().push(session);
            }
        }

        Self { by_id, by_alias }
    }

    pub fn resolve(
        &'a self,
        reference: &SessionReference,
        binding: Option<&WorkspaceBinding>,
    ) -> Result<&'a SessionDescriptor, SessionResolveError> {
        match reference.kind {
            SessionReferenceKind::Id => {
                self.by_id
                    .get(reference.raw.as_str())
                    .copied()
                    .ok_or_else(|| SessionResolveError::NotFound {
                        reference: reference.raw.clone(),
                    })
            }
            SessionReferenceKind::Alias => {
                let matches = self.by_alias.get(reference.raw.as_str()).ok_or_else(|| {
                    SessionResolveError::NotFound {
                        reference: reference.raw.clone(),
                    }
                })?;

                if matches.len() == 1 {
                    return Ok(matches[0]);
                }

                if let Some(binding) = binding {
                    let workspace_matches: Vec<_> = matches
                        .iter()
                        .copied()
                        .filter(|session| session.matches_workspace(binding))
                        .collect();
                    if workspace_matches.len() == 1 {
                        return Ok(workspace_matches[0]);
                    }
                    if workspace_matches.len() > 1 {
                        return Err(SessionResolveError::Ambiguous {
                            reference: reference.raw.clone(),
                            candidate_ids: workspace_matches
                                .iter()
                                .map(|session| session.id.clone())
                                .collect(),
                        });
                    }
                }

                Err(SessionResolveError::Ambiguous {
                    reference: reference.raw.clone(),
                    candidate_ids: matches.iter().map(|session| session.id.clone()).collect(),
                })
            }
        }
    }
}

pub fn select_default_session(
    binding: &WorkspaceBinding,
    sessions: &[SessionDescriptor],
    ephemeral: bool,
) -> SessionSelection {
    if ephemeral {
        return SessionSelection::Create {
            draft: SessionDraft {
                workspace_root: binding.workspace_root.clone(),
                repo_root: binding.repo_root.clone(),
                ephemeral: true,
                history_mode: SessionHistoryMode::CleanWorkspaceIdentity,
            },
            reason: SessionSelectionReason::ExplicitEphemeral,
        };
    }

    let preferred = sessions
        .iter()
        .filter(|session| !session.ephemeral && session.matches_workspace(binding))
        .max_by(|left, right| compare_activity(left, right));

    match preferred {
        Some(session) => SessionSelection::Reuse {
            session: session.clone(),
            reason: SessionSelectionReason::WorkspaceScopedDefault,
        },
        None => SessionSelection::Create {
            draft: SessionDraft {
                workspace_root: binding.workspace_root.clone(),
                repo_root: binding.repo_root.clone(),
                ephemeral: false,
                history_mode: SessionHistoryMode::ResumePrior,
            },
            reason: SessionSelectionReason::NoWorkspaceMatch,
        },
    }
}

fn compare_activity(left: &SessionDescriptor, right: &SessionDescriptor) -> std::cmp::Ordering {
    left.last_active_at
        .as_deref()
        .cmp(&right.last_active_at.as_deref())
        .then_with(|| left.id.cmp(&right.id))
}

fn looks_like_session_id(value: &str) -> bool {
    value.starts_with("sess_") || value.starts_with("thread_") || value.starts_with("thr_")
}

fn find_repo_root(start: &Path) -> Result<Option<PathBuf>, SessionError> {
    let mut current = Some(start);
    while let Some(path) = current {
        let git_dir = path.join(".git");
        match fs::metadata(&git_dir) {
            Ok(_) => return Ok(Some(path.to_path_buf())),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                current = path.parent();
            }
            Err(source) => {
                return Err(SessionError::PathIo {
                    path: git_dir,
                    source,
                });
            }
        }
    }
    Ok(None)
}

#[derive(Debug)]
pub enum SessionResolveError {
    NotFound {
        reference: String,
    },
    Ambiguous {
        reference: String,
        candidate_ids: Vec<String>,
    },
}

impl fmt::Display for SessionResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { reference } => write!(f, "session reference not found: {reference}"),
            Self::Ambiguous {
                reference,
                candidate_ids,
            } => write!(
                f,
                "session reference is ambiguous: {reference} (candidates: {})",
                candidate_ids.join(", ")
            ),
        }
    }
}

impl std::error::Error for SessionResolveError {}

#[derive(Debug)]
pub enum SessionError {
    PathIo { path: PathBuf, source: io::Error },
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PathIo { path, source } => {
                write!(
                    f,
                    "filesystem lookup failed for {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::PathIo { source, .. } => Some(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn workspace_binding_uses_repo_root_when_git_dir_exists() {
        let root = temp_path("repo_binding_root");
        let nested = root.join("src/bin");
        fs::create_dir_all(root.join(".git")).expect("git dir should exist");
        fs::create_dir_all(&nested).expect("nested dir should exist");
        let canonical_root = fs::canonicalize(&root).expect("root should canonicalize");

        let binding = WorkspaceBinding::discover(&nested).expect("binding should resolve");

        assert_eq!(binding.workspace_root, canonical_root);
        assert_eq!(binding.repo_root, Some(binding.workspace_root.clone()));
        assert_eq!(binding.binding_kind, WorkspaceBindingKind::RepoRoot);
    }

    #[test]
    fn workspace_binding_falls_back_to_directory_without_repo_root() {
        let root = temp_path("dir_binding_root");
        fs::create_dir_all(&root).expect("directory should exist");

        let binding = WorkspaceBinding::discover(&root).expect("binding should resolve");

        assert_eq!(binding.workspace_root, binding.resolved_cwd);
        assert_eq!(binding.repo_root, None);
        assert_eq!(binding.binding_kind, WorkspaceBindingKind::Directory);
    }

    #[test]
    fn alias_resolution_prefers_workspace_match_when_alias_repeats() {
        let workspace_root = temp_path("workspace_a");
        let other_root = temp_path("workspace_b");
        fs::create_dir_all(workspace_root.join(".git")).expect("workspace a git dir should exist");
        fs::create_dir_all(other_root.join(".git")).expect("workspace b git dir should exist");
        fs::create_dir_all(workspace_root.join("src")).expect("workspace a src should exist");
        let binding =
            WorkspaceBinding::discover(workspace_root.join("src")).expect("binding should resolve");

        let sessions = vec![
            session(
                "sess_other",
                Some("feature-auth"),
                other_root,
                false,
                Some("2026-05-11T10:00:00Z"),
            ),
            session(
                "sess_here",
                Some("feature-auth"),
                binding.workspace_root.clone(),
                false,
                Some("2026-05-11T11:00:00Z"),
            ),
        ];
        let index = SessionIndex::new(&sessions);

        let resolved = index
            .resolve(&SessionReference::parse("feature-auth"), Some(&binding))
            .expect("workspace-scoped alias should resolve");

        assert_eq!(resolved.id, "sess_here");
    }

    #[test]
    fn default_selection_prefers_reusable_workspace_session() {
        let root = temp_path("select_default_root");
        fs::create_dir_all(root.join(".git")).expect("git dir should exist");
        fs::create_dir_all(root.join("pkg")).expect("pkg dir should exist");
        let binding = WorkspaceBinding::discover(root.join("pkg")).expect("binding should resolve");
        let sessions = vec![
            session(
                "sess_old",
                Some("alpha"),
                binding.workspace_root.clone(),
                false,
                Some("2026-05-11T09:00:00Z"),
            ),
            session(
                "sess_ephemeral",
                Some("beta"),
                binding.workspace_root.clone(),
                true,
                Some("2026-05-11T12:00:00Z"),
            ),
            session(
                "sess_new",
                Some("gamma"),
                binding.workspace_root.clone(),
                false,
                Some("2026-05-11T13:00:00Z"),
            ),
        ];

        let selection = select_default_session(&binding, &sessions, false);

        match selection {
            SessionSelection::Reuse { session, reason } => {
                assert_eq!(session.id, "sess_new");
                assert_eq!(reason, SessionSelectionReason::WorkspaceScopedDefault);
            }
            other => panic!("expected reuse selection, got {other:?}"),
        }
    }

    #[test]
    fn ephemeral_selection_creates_history_clean_draft() {
        let root = temp_path("ephemeral_selection_root");
        fs::create_dir_all(root.join(".git")).expect("git dir should exist");
        let binding = WorkspaceBinding::discover(&root).expect("binding should resolve");

        let selection = select_default_session(&binding, &[], true);

        match selection {
            SessionSelection::Create { draft, reason } => {
                assert!(draft.ephemeral);
                assert_eq!(draft.workspace_root, binding.workspace_root);
                assert_eq!(
                    draft.history_mode,
                    SessionHistoryMode::CleanWorkspaceIdentity
                );
                assert_eq!(reason, SessionSelectionReason::ExplicitEphemeral);
            }
            other => panic!("expected create selection, got {other:?}"),
        }
    }

    #[test]
    fn id_like_reference_resolves_by_id() {
        let workspace_root = temp_path("id_lookup_root");
        fs::create_dir_all(&workspace_root).expect("workspace should exist");
        let sessions = vec![session(
            "sess_123",
            Some("sess_123"),
            workspace_root,
            false,
            Some("2026-05-11T12:00:00Z"),
        )];
        let index = SessionIndex::new(&sessions);

        let resolved = index
            .resolve(&SessionReference::parse("sess_123"), None)
            .expect("id-like reference should resolve as id");

        assert_eq!(resolved.id, "sess_123");
    }

    fn session(
        id: &str,
        alias: Option<&str>,
        workspace_root: PathBuf,
        ephemeral: bool,
        last_active_at: Option<&str>,
    ) -> SessionDescriptor {
        SessionDescriptor {
            id: id.to_owned(),
            alias: alias.map(str::to_owned),
            workspace_root: fs::canonicalize(workspace_root)
                .expect("workspace root should canonicalize"),
            repo_root: None,
            ephemeral,
            yolo: false,
            last_active_at: last_active_at.map(str::to_owned),
        }
    }

    fn temp_path(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "codex_session_tests_{label}_{}_{}",
            std::process::id(),
            unique
        ))
    }
}
