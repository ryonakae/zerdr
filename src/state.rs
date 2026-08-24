use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

pub const DEFAULT_SESSION_NAME: &str = "default";
const LEGACY_SESSION_NAME: &str = "zerdr";
const SCHEMA_VERSION: u32 = 1;
const BINDING_SCHEMA_VERSION: u32 = 2;
const ROUTE_SCHEMA_VERSION: u32 = 2;
static UNIQUE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub bindings_file: PathBuf,
    pub leases_dir: PathBuf,
    pub thread_leases_dir: PathBuf,
    pub thread_memory_dir: PathBuf,
    pub routes_dir: PathBuf,
    pub sync_locks_dir: PathBuf,
    pub lifecycle_lock_file: PathBuf,
    pub zed_lock_file: PathBuf,
    pub install_state_file: PathBuf,
    pub thread_auto_flag_file: PathBuf,
    pub plugin_dir: PathBuf,
    pub zed_tasks_file: PathBuf,
    pub zed_settings_file: PathBuf,
}

impl Paths {
    pub fn discover() -> Result<Self> {
        if let Some(root) = std::env::var_os("ZERDR_TEST_ROOT") {
            return Ok(Self::for_test(root));
        }
        let project = ProjectDirs::from("dev", "ryonakae", "zerdr")
            .ok_or_else(|| Error::User("could not resolve platform directories".to_owned()))?;
        let state = project.state_dir().unwrap_or(project.data_local_dir());
        let zed_tasks_file = std::env::var_os("ZERDR_ZED_TASKS_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| default_zed_config_file("tasks.json"));
        let zed_settings_file = std::env::var_os("ZERDR_ZED_SETTINGS_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| default_zed_config_file("settings.json"));
        Ok(Self::from_roots(
            project.config_dir(),
            project.data_dir(),
            state,
            zed_tasks_file,
            zed_settings_file,
        ))
    }

    pub fn for_test(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self::from_roots(
            root.join("config"),
            root.join("data"),
            root.join("state"),
            root.join("zed/tasks.json"),
            root.join("zed/settings.json"),
        )
    }

    fn from_roots(
        config_dir: impl AsRef<Path>,
        data_dir: impl AsRef<Path>,
        state_dir: impl AsRef<Path>,
        zed_tasks_file: PathBuf,
        zed_settings_file: PathBuf,
    ) -> Self {
        let config_dir = config_dir.as_ref().to_path_buf();
        let data_dir = data_dir.as_ref().to_path_buf();
        let state_dir = state_dir.as_ref().to_path_buf();
        let lock_root = state_dir.parent().unwrap_or(&state_dir);
        let lifecycle_lock_file = lock_root.join(".dev.ryonakae.zerdr.lifecycle.lock");
        let zed_lock_file = lock_root.join(".dev.ryonakae.zerdr.zed.lock");
        Self {
            bindings_file: state_dir.join("bindings.json"),
            leases_dir: state_dir.join("leases"),
            thread_leases_dir: state_dir.join("thread-leases"),
            thread_memory_dir: state_dir.join("thread-panes"),
            routes_dir: state_dir.join("routes"),
            sync_locks_dir: state_dir.join("sync-locks"),
            lifecycle_lock_file,
            zed_lock_file,
            install_state_file: state_dir.join("install.json"),
            thread_auto_flag_file: state_dir.join("thread-auto"),
            plugin_dir: data_dir.join("plugin-v1"),
            zed_tasks_file,
            zed_settings_file,
            config_dir,
            data_dir,
            state_dir,
        }
    }
}

fn default_zed_config_file(name: &str) -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join("zed").join(name);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config/zed").join(name)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BindingState {
    pub schema_version: u32,
    pub sessions: BTreeMap<String, BTreeMap<String, PathBuf>>,
}

impl Default for BindingState {
    fn default() -> Self {
        Self {
            schema_version: BINDING_SCHEMA_VERSION,
            sessions: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingStateV1 {
    schema_version: u32,
    session_name: String,
    bindings: BTreeMap<String, PathBuf>,
}

#[derive(Debug, Clone)]
pub struct BindingStore {
    path: PathBuf,
}

impl BindingStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<BindingState> {
        if !self.path.exists() {
            return Ok(BindingState::default());
        }
        let bytes = fs::read(&self.path).map_err(|error| Error::io(&self.path, error))?;
        self.parse(&bytes)
    }

    pub fn bind(
        &self,
        session_name: &str,
        workspace_id: &str,
        candidate: &Path,
    ) -> Result<PathBuf> {
        let root = canonical_git_root(candidate)?;
        let _lock = self.acquire_write_lock()?;
        let mut state = self.load()?;
        state
            .sessions
            .entry(session_name.to_owned())
            .or_default()
            .insert(workspace_id.to_owned(), root.clone());
        self.save_unlocked(&state)?;
        Ok(root)
    }

    pub fn bind_if_absent(
        &self,
        session_name: &str,
        workspace_id: &str,
        candidate: &Path,
    ) -> Result<PathBuf> {
        {
            let _lock = self.acquire_write_lock()?;
            if let Some(existing) = self.get_from_state(&self.load()?, session_name, workspace_id) {
                return Ok(existing.clone());
            }
        }

        let root = match canonical_git_root(candidate) {
            Ok(root) => root,
            Err(resolution_error) => {
                let _lock = self.acquire_write_lock()?;
                if let Some(existing) =
                    self.get_from_state(&self.load()?, session_name, workspace_id)
                {
                    return Ok(existing.clone());
                }
                return Err(resolution_error);
            }
        };

        let _lock = self.acquire_write_lock()?;
        let mut state = self.load()?;
        if let Some(existing) = self.get_from_state(&state, session_name, workspace_id) {
            return Ok(existing.clone());
        }
        state
            .sessions
            .entry(session_name.to_owned())
            .or_default()
            .insert(workspace_id.to_owned(), root.clone());
        self.save_unlocked(&state)?;
        Ok(root)
    }

    pub fn set_canonical(&self, session_name: &str, workspace_id: &str, root: &Path) -> Result<()> {
        let root = canonical_git_root(root)?;
        let _lock = self.acquire_write_lock()?;
        let mut state = self.load()?;
        state
            .sessions
            .entry(session_name.to_owned())
            .or_default()
            .insert(workspace_id.to_owned(), root);
        self.save_unlocked(&state)
    }

    pub fn unbind(&self, session_name: &str, workspace_id: &str) -> Result<bool> {
        let _lock = self.acquire_write_lock()?;
        let mut state = self.load()?;
        let removed = state
            .sessions
            .get_mut(session_name)
            .is_some_and(|bindings| bindings.remove(workspace_id).is_some());
        self.save_unlocked(&state)?;
        Ok(removed)
    }

    pub fn get(&self, session_name: &str, workspace_id: &str) -> Result<Option<PathBuf>> {
        Ok(self
            .get_from_state(&self.load()?, session_name, workspace_id)
            .cloned())
    }

    fn parse(&self, bytes: &[u8]) -> Result<BindingState> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|source| Error::Json {
                what: self.path.display().to_string(),
                source,
            })?;
        let schema_version = value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                Error::User(format!(
                    "binding state has no valid schema version: {}",
                    self.path.display()
                ))
            })?;
        match schema_version {
            1 => {
                let legacy: BindingStateV1 =
                    serde_json::from_value(value).map_err(|source| Error::Json {
                        what: self.path.display().to_string(),
                        source,
                    })?;
                if legacy.schema_version != SCHEMA_VERSION
                    || legacy.session_name != LEGACY_SESSION_NAME
                {
                    return Err(Error::User(format!(
                        "legacy binding state belongs to session {:?}, expected {LEGACY_SESSION_NAME:?}",
                        legacy.session_name
                    )));
                }
                Ok(BindingState {
                    schema_version: BINDING_SCHEMA_VERSION,
                    sessions: BTreeMap::from([(LEGACY_SESSION_NAME.to_owned(), legacy.bindings)]),
                })
            }
            version if version == u64::from(BINDING_SCHEMA_VERSION) => {
                let state: BindingState =
                    serde_json::from_value(value).map_err(|source| Error::Json {
                        what: self.path.display().to_string(),
                        source,
                    })?;
                validate_binding_state(&state)?;
                Ok(state)
            }
            version => Err(Error::User(format!(
                "unsupported binding schema version {version}; expected 1 or {BINDING_SCHEMA_VERSION}"
            ))),
        }
    }

    fn get_from_state<'a>(
        &self,
        state: &'a BindingState,
        session_name: &str,
        workspace_id: &str,
    ) -> Option<&'a PathBuf> {
        state
            .sessions
            .get(session_name)
            .and_then(|bindings| bindings.get(workspace_id))
    }

    fn save_unlocked(&self, state: &BindingState) -> Result<()> {
        validate_binding_state(state)?;
        atomic_write_json(&self.path, state)
    }

    fn acquire_write_lock(&self) -> Result<File> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| Error::User(format!("path has no parent: {}", self.path.display())))?;
        fs::create_dir_all(parent).map_err(|error| Error::io(parent, error))?;
        let lock_path = parent.join("bindings.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|error| Error::io(&lock_path, error))?;
        FileExt::lock_exclusive(&file).map_err(|error| Error::io(&lock_path, error))?;
        Ok(file)
    }
}

fn validate_binding_state(state: &BindingState) -> Result<()> {
    if state.schema_version != BINDING_SCHEMA_VERSION {
        return Err(Error::User(format!(
            "unsupported binding schema version {}; expected {BINDING_SCHEMA_VERSION}",
            state.schema_version
        )));
    }
    Ok(())
}

pub fn canonical_git_root(candidate: &Path) -> Result<PathBuf> {
    if !candidate.exists() {
        return Err(Error::User(format!(
            "Git checkout path does not exist: {}",
            candidate.display()
        )));
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(candidate)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|error| Error::User(format!("failed to run git: {error}")))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(Error::User(format!(
            "{} is not inside a Git checkout{}",
            candidate.display(),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        )));
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    PathBuf::from(root)
        .canonicalize()
        .map_err(|error| Error::io(candidate, error))
}

/// Whether `root` is a linked Git worktree rather than a primary checkout, detected
/// tool-agnostically: the two directories differ exactly for linked worktrees.
pub fn is_linked_worktree(root: &Path) -> Result<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--git-dir", "--git-common-dir"])
        .output()
        .map_err(|error| Error::User(format!("failed to run git: {error}")))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(Error::User(format!(
            "could not inspect the Git checkout at {}: {detail}",
            root.display()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let (Some(git_dir), Some(common_dir)) = (lines.next(), lines.next()) else {
        return Err(Error::User(format!(
            "git rev-parse returned no directories for {}",
            root.display()
        )));
    };
    // Either line may be relative (plain `.git` in a primary checkout) and unresolved,
    // so both are anchored to the root and canonicalized before comparing.
    let resolve = |dir: &str| {
        let path = Path::new(dir);
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        absolute
            .canonicalize()
            .map_err(|error| Error::io(dir, error))
    };
    Ok(resolve(git_dir)? != resolve(common_dir)?)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
pub enum RouteStrategy {
    Internal { anchor_root: PathBuf },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RouteState {
    pub schema_version: u32,
    pub session_name: String,
    pub socket_path: PathBuf,
    pub wrapper_pid: u32,
    pub routing: RouteStrategy,
}

impl RouteState {
    pub fn internal_anchor(&self) -> Option<&Path> {
        match &self.routing {
            RouteStrategy::Internal { anchor_root } => Some(anchor_root),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteStateV2 {
    schema_version: u32,
    session_name: String,
    socket_path: PathBuf,
    wrapper_pid: u32,
    routing: RouteStrategy,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteStateV1 {
    schema_version: u32,
    session_name: String,
    socket_path: PathBuf,
    anchor_root: PathBuf,
    wrapper_pid: u32,
}

impl RouteStateV2 {
    fn into_route(self) -> RouteState {
        RouteState {
            schema_version: self.schema_version,
            session_name: self.session_name,
            socket_path: self.socket_path,
            wrapper_pid: self.wrapper_pid,
            routing: self.routing,
        }
    }
}

impl RouteStateV1 {
    fn into_route(self) -> RouteState {
        RouteState {
            schema_version: self.schema_version,
            session_name: self.session_name,
            socket_path: self.socket_path,
            wrapper_pid: self.wrapper_pid,
            routing: RouteStrategy::Internal {
                anchor_root: self.anchor_root,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct RouteStore {
    root: PathBuf,
}

impl RouteStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn initialize(&self, socket_path: &Path, anchor: &Path, wrapper_pid: u32) -> Result<()> {
        self.initialize_for(DEFAULT_SESSION_NAME, socket_path, anchor, wrapper_pid)
    }

    pub fn initialize_for(
        &self,
        session_name: &str,
        socket_path: &Path,
        anchor: &Path,
        wrapper_pid: u32,
    ) -> Result<()> {
        self.initialize_strategy_for(
            session_name,
            socket_path,
            RouteStrategy::Internal {
                anchor_root: anchor.to_path_buf(),
            },
            wrapper_pid,
        )
    }

    pub fn initialize_strategy(
        &self,
        socket_path: &Path,
        routing: RouteStrategy,
        wrapper_pid: u32,
    ) -> Result<()> {
        self.initialize_strategy_for(DEFAULT_SESSION_NAME, socket_path, routing, wrapper_pid)
    }

    pub fn initialize_strategy_for(
        &self,
        session_name: &str,
        socket_path: &Path,
        routing: RouteStrategy,
        wrapper_pid: u32,
    ) -> Result<()> {
        if session_name.is_empty() {
            return Err(Error::User("Herdr session name cannot be empty".to_owned()));
        }
        let socket_path = canonical_socket(socket_path)?;
        let routing = match routing {
            RouteStrategy::Internal { anchor_root } => RouteStrategy::Internal {
                anchor_root: canonical_git_root(&anchor_root)?,
            },
        };
        let state = RouteState {
            schema_version: ROUTE_SCHEMA_VERSION,
            session_name: session_name.to_owned(),
            socket_path: socket_path.clone(),
            wrapper_pid,
            routing,
        };
        if std::env::var("ZERDR_TEST_FAIL_ROUTE_INITIALIZE").is_ok_and(|value| value == "1") {
            return Err(Error::User(
                "injected route initialization failure".to_owned(),
            ));
        }
        atomic_write_json(&self.path_for_canonical_socket(&socket_path), &state)
    }

    pub fn load(&self, socket_path: &Path) -> Result<RouteState> {
        self.load_with_session(socket_path, None)
    }

    pub fn load_for(&self, session_name: &str, socket_path: &Path) -> Result<RouteState> {
        self.load_with_session(socket_path, Some(session_name))
    }

    fn load_with_session(
        &self,
        socket_path: &Path,
        expected_session: Option<&str>,
    ) -> Result<RouteState> {
        let socket_path = canonical_socket(socket_path)?;
        let path = self.path_for_canonical_socket(&socket_path);
        let bytes = fs::read(&path).map_err(|error| Error::io(&path, error))?;
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|source| Error::Json {
                what: path.display().to_string(),
                source,
            })?;
        let schema_version = value
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| {
                Error::User(format!(
                    "route has no valid schema version: {}",
                    path.display()
                ))
            })?;
        let state = match schema_version {
            1 => serde_json::from_value::<RouteStateV1>(value)
                .map_err(|source| Error::Json {
                    what: path.display().to_string(),
                    source,
                })?
                .into_route(),
            2 => serde_json::from_value::<RouteStateV2>(value)
                .map_err(|source| Error::Json {
                    what: path.display().to_string(),
                    source,
                })?
                .into_route(),
            version => {
                return Err(Error::User(format!(
                    "unsupported route schema version {version}; expected 1 or {ROUTE_SCHEMA_VERSION}"
                )));
            }
        };
        validate_route(&state, expected_session, &socket_path)?;
        Ok(state)
    }

    pub fn promote(&self, socket_path: &Path, anchor: &Path) -> Result<()> {
        self.promote_with_session(socket_path, None, anchor)
    }

    pub fn promote_for(&self, session_name: &str, socket_path: &Path, anchor: &Path) -> Result<()> {
        self.promote_with_session(socket_path, Some(session_name), anchor)
    }

    fn promote_with_session(
        &self,
        socket_path: &Path,
        expected_session: Option<&str>,
        anchor: &Path,
    ) -> Result<()> {
        let socket_path = canonical_socket(socket_path)?;
        let mut state = self.load_with_session(&socket_path, expected_session)?;
        let RouteStrategy::Internal { anchor_root } = &mut state.routing;
        *anchor_root = canonical_git_root(anchor)?;
        state.schema_version = ROUTE_SCHEMA_VERSION;
        if std::env::var("ZERDR_TEST_FAIL_ROUTE_WRITE").is_ok_and(|value| value == "1") {
            return Err(Error::User("injected route write failure".to_owned()));
        }
        atomic_write_json(&self.path_for_canonical_socket(&socket_path), &state)
    }

    pub fn path(&self, socket_path: &Path) -> Result<PathBuf> {
        let socket_path = canonical_socket(socket_path)?;
        Ok(self.path_for_canonical_socket(&socket_path))
    }

    pub fn remove_stale_except(&self, live_scope_hashes: &[String]) -> Result<usize> {
        if !self.root.exists() {
            return Ok(0);
        }
        let mut removed = 0;
        for entry in fs::read_dir(&self.root).map_err(|error| Error::io(&self.root, error))? {
            let path = entry.map_err(|error| Error::io(&self.root, error))?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            if path
                .file_stem()
                .and_then(|value| value.to_str())
                .is_some_and(|hash| live_scope_hashes.iter().any(|live| live == hash))
            {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(Error::io(&path, error)),
            }
        }
        Ok(removed)
    }

    fn path_for_canonical_socket(&self, socket_path: &Path) -> PathBuf {
        self.root.join(format!("{}.json", path_hash(socket_path)))
    }
}

fn validate_route(
    state: &RouteState,
    expected_session: Option<&str>,
    expected_socket: &Path,
) -> Result<()> {
    if !matches!(state.schema_version, SCHEMA_VERSION | ROUTE_SCHEMA_VERSION)
        || state.session_name.is_empty()
        || expected_session.is_some_and(|expected| state.session_name != expected)
    {
        return Err(Error::User(format!(
            "route has an incompatible schema or session for {}",
            expected_socket.display()
        )));
    }
    let socket = canonical_socket(&state.socket_path)?;
    if socket != expected_socket {
        return Err(Error::User(format!(
            "route socket {} does not match {}",
            socket.display(),
            expected_socket.display()
        )));
    }
    let RouteStrategy::Internal { anchor_root } = &state.routing;
    let anchor = canonical_git_root(anchor_root)?;
    if anchor != *anchor_root {
        return Err(Error::User(format!(
            "route anchor is not a canonical Git checkout root: {}",
            anchor_root.display()
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub schema_version: u32,
    pub session_name: String,
    pub socket_path: PathBuf,
    pub wrapper_pid: u32,
    pub client_pid: u32,
    pub created_unix_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseInspection {
    pub live: bool,
    pub live_wrapper_pids: Vec<u32>,
    pub stale_removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseSweep {
    pub live_count: usize,
    pub live_scope_hashes: Vec<String>,
    pub live_session_names: Vec<String>,
    pub stale_removed: usize,
}

#[derive(Debug, Clone)]
pub struct LeaseSet {
    root: PathBuf,
}

impl LeaseSet {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn acquire(&self, socket_path: &Path, client_pid: u32) -> Result<LeaseGuard> {
        self.acquire_for(DEFAULT_SESSION_NAME, socket_path, client_pid)
    }

    pub fn acquire_for(
        &self,
        session_name: &str,
        socket_path: &Path,
        client_pid: u32,
    ) -> Result<LeaseGuard> {
        if session_name.is_empty() {
            return Err(Error::User("Herdr session name cannot be empty".to_owned()));
        }
        let socket_path = canonical_socket(socket_path)?;
        let directory = self.socket_directory(&socket_path);
        fs::create_dir_all(&directory).map_err(|error| Error::io(&directory, error))?;
        let id = unique_id();
        let final_path = directory.join(format!("{}-{id}.json", std::process::id()));
        let temporary = directory.join(format!(".{}-{id}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| Error::io(&temporary, error))?;
        FileExt::lock_exclusive(&file).map_err(|error| Error::io(&temporary, error))?;
        let record = LeaseRecord {
            schema_version: SCHEMA_VERSION,
            session_name: session_name.to_owned(),
            socket_path,
            wrapper_pid: std::process::id(),
            client_pid,
            created_unix_ms: now_millis(),
        };
        serde_json::to_writer_pretty(&mut file, &record).map_err(|source| Error::Json {
            what: temporary.display().to_string(),
            source,
        })?;
        file.write_all(b"\n")
            .map_err(|error| Error::io(&temporary, error))?;
        file.sync_all()
            .map_err(|error| Error::io(&temporary, error))?;
        fs::rename(&temporary, &final_path).map_err(|error| Error::io(&final_path, error))?;
        Ok(LeaseGuard {
            file: Some(file),
            path: final_path,
        })
    }

    pub fn has_live(&self, socket_path: &Path) -> Result<bool> {
        Ok(self.inspect(socket_path)?.live)
    }

    pub fn inspect(&self, socket_path: &Path) -> Result<LeaseInspection> {
        self.inspect_with_session(socket_path, None)
    }

    pub fn inspect_for(&self, session_name: &str, socket_path: &Path) -> Result<LeaseInspection> {
        self.inspect_with_session(socket_path, Some(session_name))
    }

    fn inspect_with_session(
        &self,
        socket_path: &Path,
        expected_session: Option<&str>,
    ) -> Result<LeaseInspection> {
        let socket_path = canonical_socket(socket_path)?;
        let directory = self.socket_directory(&socket_path);
        if !directory.exists() {
            return Ok(LeaseInspection {
                live: false,
                live_wrapper_pids: Vec::new(),
                stale_removed: 0,
            });
        }
        let mut live_wrapper_pids = Vec::new();
        let mut stale_removed = 0;
        for entry in fs::read_dir(&directory).map_err(|error| Error::io(&directory, error))? {
            let path = entry.map_err(|error| Error::io(&directory, error))?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let mut file = match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(Error::io(&path, error)),
            };
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => {
                    let _ = FileExt::unlock(&file);
                    if fs::remove_file(&path).is_ok() {
                        stale_removed += 1;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    file.seek(SeekFrom::Start(0))
                        .map_err(|error| Error::io(&path, error))?;
                    let mut bytes = Vec::new();
                    file.read_to_end(&mut bytes)
                        .map_err(|error| Error::io(&path, error))?;
                    let record: LeaseRecord =
                        serde_json::from_slice(&bytes).map_err(|source| Error::Json {
                            what: format!("locked lease {}", path.display()),
                            source,
                        })?;
                    validate_lease(&record, expected_session, &socket_path)?;
                    live_wrapper_pids.push(record.wrapper_pid);
                }
                Err(error) => return Err(Error::io(&path, error)),
            }
        }
        Ok(LeaseInspection {
            live: !live_wrapper_pids.is_empty(),
            live_wrapper_pids,
            stale_removed,
        })
    }

    pub fn any_live(&self) -> Result<bool> {
        Ok(self.sweep_all()?.live_count > 0)
    }

    pub fn sweep_all(&self) -> Result<LeaseSweep> {
        if !self.root.exists() {
            return Ok(LeaseSweep {
                live_count: 0,
                live_scope_hashes: Vec::new(),
                live_session_names: Vec::new(),
                stale_removed: 0,
            });
        }
        let mut sweep = LeaseSweep {
            live_count: 0,
            live_scope_hashes: Vec::new(),
            live_session_names: Vec::new(),
            stale_removed: 0,
        };
        for entry in fs::read_dir(&self.root).map_err(|error| Error::io(&self.root, error))? {
            let directory = entry.map_err(|error| Error::io(&self.root, error))?.path();
            if !directory.is_dir() {
                continue;
            }
            let mut scope_live = false;
            for lease in fs::read_dir(&directory).map_err(|error| Error::io(&directory, error))? {
                let path = lease.map_err(|error| Error::io(&directory, error))?.path();
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let mut file = match OpenOptions::new().read(true).write(true).open(&path) {
                    Ok(file) => file,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(Error::io(&path, error)),
                };
                match FileExt::try_lock_exclusive(&file) {
                    Ok(()) => {
                        let _ = FileExt::unlock(&file);
                        if fs::remove_file(&path).is_ok() {
                            sweep.stale_removed += 1;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        file.seek(SeekFrom::Start(0))
                            .map_err(|error| Error::io(&path, error))?;
                        let mut bytes = Vec::new();
                        file.read_to_end(&mut bytes)
                            .map_err(|error| Error::io(&path, error))?;
                        let record: LeaseRecord =
                            serde_json::from_slice(&bytes).map_err(|source| Error::Json {
                                what: format!("locked lease {}", path.display()),
                                source,
                            })?;
                        if record.schema_version != SCHEMA_VERSION || record.session_name.is_empty()
                        {
                            return Err(Error::User(format!(
                                "locked lease has an incompatible schema or session: {}",
                                path.display()
                            )));
                        }
                        sweep.live_count += 1;
                        if !sweep.live_session_names.contains(&record.session_name) {
                            sweep.live_session_names.push(record.session_name);
                        }
                        scope_live = true;
                    }
                    Err(error) => return Err(Error::io(&path, error)),
                }
            }
            if scope_live && let Some(hash) = directory.file_name().and_then(|value| value.to_str())
            {
                sweep.live_scope_hashes.push(hash.to_owned());
            }
        }
        Ok(sweep)
    }

    fn socket_directory(&self, socket_path: &Path) -> PathBuf {
        self.root.join(path_hash(socket_path))
    }
}

fn validate_lease(
    record: &LeaseRecord,
    expected_session: Option<&str>,
    expected_socket: &Path,
) -> Result<()> {
    if record.schema_version != SCHEMA_VERSION
        || record.session_name.is_empty()
        || expected_session.is_some_and(|expected| record.session_name != expected)
    {
        return Err(Error::User(format!(
            "locked lease has an incompatible schema or session for {}",
            expected_socket.display()
        )));
    }
    let record_socket = canonical_socket(&record.socket_path)?;
    if record_socket != expected_socket {
        return Err(Error::User(format!(
            "locked lease socket {} does not match {}",
            record_socket.display(),
            expected_socket.display()
        )));
    }
    Ok(())
}

pub struct LeaseGuard {
    file: Option<File>,
    path: PathBuf,
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = fs::remove_file(&self.path);
            let _ = FileExt::unlock(&file);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadLeaseRecord {
    pub schema_version: u32,
    pub session_name: String,
    pub socket_path: PathBuf,
    pub pane_id: String,
    pub thread_pid: u32,
    pub created_unix_ms: u128,
}

/// Per-pane leases that stop two bare `zerdr connect` invocations from attaching to the
/// same Herdr agent. Unlike [`LeaseSet`] a scope holds many live leases at once, so the
/// lease identity includes the pane and each pane maps to exactly one file.
#[derive(Debug, Clone)]
pub struct ThreadLeaseSet {
    root: PathBuf,
}

impl ThreadLeaseSet {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn acquire(
        &self,
        session_name: &str,
        socket_path: &Path,
        pane_id: &str,
    ) -> Result<ThreadLeaseGuard> {
        if session_name.is_empty() {
            return Err(Error::User("Herdr session name cannot be empty".to_owned()));
        }
        if pane_id.is_empty() {
            return Err(Error::User("Herdr pane id cannot be empty".to_owned()));
        }
        let socket_path = canonical_socket(socket_path)?;
        let directory = self.scope_directory(session_name, &socket_path);
        fs::create_dir_all(&directory).map_err(|error| Error::io(&directory, error))?;
        let path = directory.join(format!("{}.json", path_hash(Path::new(pane_id))));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| Error::io(&path, error))?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(Error::User(format!(
                    "Herdr pane {pane_id} already has a live zerdr connection"
                )));
            }
            Err(error) => return Err(Error::io(&path, error)),
        }
        let record = ThreadLeaseRecord {
            schema_version: SCHEMA_VERSION,
            session_name: session_name.to_owned(),
            socket_path,
            pane_id: pane_id.to_owned(),
            thread_pid: std::process::id(),
            created_unix_ms: now_millis(),
        };
        let mut bytes = serde_json::to_vec_pretty(&record).map_err(|source| Error::Json {
            what: path.display().to_string(),
            source,
        })?;
        bytes.push(b'\n');
        file.set_len(0).map_err(|error| Error::io(&path, error))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|error| Error::io(&path, error))?;
        file.write_all(&bytes)
            .map_err(|error| Error::io(&path, error))?;
        file.sync_all().map_err(|error| Error::io(&path, error))?;
        Ok(ThreadLeaseGuard {
            file: Some(file),
            path,
        })
    }

    /// Pane ids whose lease is currently held. A record whose lock is free belongs to a
    /// process that is gone, so it is removed rather than reported.
    pub fn leased_panes(
        &self,
        session_name: &str,
        socket_path: &Path,
    ) -> Result<std::collections::BTreeSet<String>> {
        let socket_path = canonical_socket(socket_path)?;
        let directory = self.scope_directory(session_name, &socket_path);
        let mut leased = std::collections::BTreeSet::new();
        if !directory.exists() {
            return Ok(leased);
        }
        for entry in fs::read_dir(&directory).map_err(|error| Error::io(&directory, error))? {
            let path = entry.map_err(|error| Error::io(&directory, error))?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let mut file = match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(Error::io(&path, error)),
            };
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => {
                    let _ = FileExt::unlock(&file);
                    let _ = fs::remove_file(&path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    file.seek(SeekFrom::Start(0))
                        .map_err(|error| Error::io(&path, error))?;
                    let mut bytes = Vec::new();
                    file.read_to_end(&mut bytes)
                        .map_err(|error| Error::io(&path, error))?;
                    let record: ThreadLeaseRecord =
                        serde_json::from_slice(&bytes).map_err(|source| Error::Json {
                            what: format!("locked thread lease {}", path.display()),
                            source,
                        })?;
                    if record.schema_version != SCHEMA_VERSION
                        || record.session_name != session_name
                        || record.pane_id.is_empty()
                    {
                        return Err(Error::User(format!(
                            "locked thread lease has an incompatible schema or session: {}",
                            path.display()
                        )));
                    }
                    leased.insert(record.pane_id);
                }
                Err(error) => return Err(Error::io(&path, error)),
            }
        }
        Ok(leased)
    }

    /// Lock guarding resolve-then-acquire for one session, so concurrent threads on
    /// unrelated Herdr sessions never wait on each other.
    pub fn resolve_lock_path(&self, session_name: &str, socket_path: &Path) -> Result<PathBuf> {
        let socket_path = canonical_socket(socket_path)?;
        Ok(self
            .scope_directory(session_name, &socket_path)
            .join("resolve.lock"))
    }

    fn scope_directory(&self, session_name: &str, socket_path: &Path) -> PathBuf {
        let mut scope = socket_path.as_os_str().to_os_string();
        scope.push("\u{0}");
        scope.push(session_name);
        self.root.join(path_hash(Path::new(&scope)))
    }

    /// Lock serializing the headless start of one named session. Keyed by
    /// session name alone: the socket this session will answer on does not
    /// exist yet.
    pub fn session_start_lock_path(&self, session_name: &str) -> PathBuf {
        self.root.join(format!(
            "session-start-{}.lock",
            path_hash(Path::new(session_name))
        ))
    }
}

const THREAD_PANE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadPaneRecord {
    pub workspace_id: String,
    pub pane_id: String,
    pub last_attached_unix_ms: u128,
}

#[derive(Debug, Serialize, Deserialize)]
struct ThreadPaneState {
    schema_version: u32,
    panes: Vec<ThreadPaneRecord>,
}

/// Remembers which Herdr panes zerdr threads were attached to, so a thread reopened
/// after a Zed restart reattaches instead of creating yet another tab. The store is
/// advisory: unreadable or foreign content loads as empty, and callers treat write
/// failures as non-fatal.
pub struct ThreadPaneMemory {
    root: PathBuf,
}

impl ThreadPaneMemory {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Records for one session scope, most recently attached first.
    pub fn load(&self, session_name: &str, socket_path: &Path) -> Vec<ThreadPaneRecord> {
        let Ok(path) = self.scope_path(session_name, socket_path) else {
            return Vec::new();
        };
        let Ok(bytes) = fs::read(&path) else {
            return Vec::new();
        };
        let Ok(state) = serde_json::from_slice::<ThreadPaneState>(&bytes) else {
            return Vec::new();
        };
        if state.schema_version != THREAD_PANE_SCHEMA_VERSION {
            return Vec::new();
        }
        let mut panes = state.panes;
        panes.sort_by(|first, second| {
            second
                .last_attached_unix_ms
                .cmp(&first.last_attached_unix_ms)
        });
        panes
    }

    /// Adds or refreshes one pane, deduplicated by pane id.
    pub fn record(
        &self,
        session_name: &str,
        socket_path: &Path,
        workspace_id: &str,
        pane_id: &str,
    ) -> Result<()> {
        let path = self.scope_path(session_name, socket_path)?;
        let mut panes = self.load(session_name, socket_path);
        panes.retain(|record| record.pane_id != pane_id);
        panes.push(ThreadPaneRecord {
            workspace_id: workspace_id.to_owned(),
            pane_id: pane_id.to_owned(),
            last_attached_unix_ms: now_millis(),
        });
        self.write(&path, panes)
    }

    /// Drops panes that no longer exist.
    pub fn prune(&self, session_name: &str, socket_path: &Path, pane_ids: &[String]) -> Result<()> {
        let path = self.scope_path(session_name, socket_path)?;
        let mut panes = self.load(session_name, socket_path);
        panes.retain(|record| !pane_ids.contains(&record.pane_id));
        self.write(&path, panes)
    }

    fn write(&self, path: &Path, panes: Vec<ThreadPaneRecord>) -> Result<()> {
        fs::create_dir_all(&self.root).map_err(|error| Error::io(&self.root, error))?;
        atomic_write_json(
            path,
            &ThreadPaneState {
                schema_version: THREAD_PANE_SCHEMA_VERSION,
                panes,
            },
        )
    }

    fn scope_path(&self, session_name: &str, socket_path: &Path) -> Result<PathBuf> {
        let socket_path = canonical_socket(socket_path)?;
        let mut scope = socket_path.as_os_str().to_os_string();
        scope.push("\u{0}");
        scope.push(session_name);
        Ok(self
            .root
            .join(format!("{}.json", path_hash(Path::new(&scope)))))
    }
}

pub struct ThreadLeaseGuard {
    file: Option<File>,
    path: PathBuf,
}

impl ThreadLeaseGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ThreadLeaseGuard {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = fs::remove_file(&self.path);
            let _ = FileExt::unlock(&file);
        }
    }
}

pub struct LifecycleGuard {
    file: File,
}

impl LifecycleGuard {
    pub fn acquire(path: &Path) -> Result<Self> {
        let parent = path
            .parent()
            .ok_or_else(|| Error::User(format!("path has no parent: {}", path.display())))?;
        fs::create_dir_all(parent).map_err(|error| Error::io(parent, error))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| Error::io(path, error))?;
        FileExt::lock_exclusive(&file).map_err(|error| Error::io(path, error))?;
        Ok(Self { file })
    }
}

impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub struct OperationGuard {
    file: File,
}

impl OperationGuard {
    pub fn acquire(path: &Path) -> Result<Self> {
        let file = open_lock_file(path)?;
        FileExt::lock_exclusive(&file).map_err(|error| Error::io(path, error))?;
        Ok(Self { file })
    }

    pub(crate) fn try_acquire(path: &Path) -> Result<Option<Self>> {
        let file = open_lock_file(path)?;
        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Some(Self { file })),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(Error::io(path, error)),
        }
    }
}

impl Drop for OperationGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn open_lock_file(path: &Path) -> Result<File> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::User(format!("path has no parent: {}", path.display())))?;
    fs::create_dir_all(parent).map_err(|error| Error::io(parent, error))?;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| Error::io(path, error))
}

pub struct SyncGuard {
    file: File,
}

impl SyncGuard {
    pub fn acquire(root: &Path, socket_path: &Path) -> Result<Self> {
        let socket_path = canonical_socket(socket_path)?;
        fs::create_dir_all(root).map_err(|error| Error::io(root, error))?;
        let path = root.join(format!("{}.lock", path_hash(&socket_path)));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| Error::io(&path, error))?;
        FileExt::lock_exclusive(&file).map_err(|error| Error::io(&path, error))?;
        Ok(Self { file })
    }
}

impl Drop for SyncGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn canonical_socket(path: &Path) -> Result<PathBuf> {
    path.canonicalize().map_err(|error| Error::io(path, error))
}

fn path_hash(path: &Path) -> String {
    hex::encode(Sha256::digest(path.as_os_str().as_encoded_bytes()))
}

fn unique_id() -> u64 {
    let sequence = UNIQUE_ID.fetch_add(1, Ordering::Relaxed);
    (now_millis() as u64)
        .wrapping_mul(1000)
        .wrapping_add(sequence)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn atomic_write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::User(format!("path has no parent: {}", path.display())))?;
    fs::create_dir_all(parent).map_err(|error| Error::io(parent, error))?;
    let temporary = parent.join(format!(".zerdr-{}.tmp", unique_id()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| Error::io(&temporary, error))?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|source| Error::Json {
        what: temporary.display().to_string(),
        source,
    })?;
    file.write_all(b"\n")
        .map_err(|error| Error::io(&temporary, error))?;
    file.sync_all()
        .map_err(|error| Error::io(&temporary, error))?;
    fs::rename(&temporary, path).map_err(|error| Error::io(path, error))?;
    Ok(())
}
