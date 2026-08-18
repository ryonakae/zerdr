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

pub const SESSION_NAME: &str = "zerdr";
const SCHEMA_VERSION: u32 = 1;
const ROUTE_SCHEMA_VERSION: u32 = 2;
static UNIQUE_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub bindings_file: PathBuf,
    pub leases_dir: PathBuf,
    pub routes_dir: PathBuf,
    pub sync_locks_dir: PathBuf,
    pub lifecycle_lock_file: PathBuf,
    pub install_state_file: PathBuf,
    pub plugin_dir: PathBuf,
    pub zed_tasks_file: PathBuf,
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
            .unwrap_or_else(default_zed_tasks_file);
        Ok(Self::from_roots(
            project.config_dir(),
            project.data_dir(),
            state,
            zed_tasks_file,
        ))
    }

    pub fn for_test(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        Self::from_roots(
            root.join("config"),
            root.join("data"),
            root.join("state"),
            root.join("zed/tasks.json"),
        )
    }

    fn from_roots(
        config_dir: impl AsRef<Path>,
        data_dir: impl AsRef<Path>,
        state_dir: impl AsRef<Path>,
        zed_tasks_file: PathBuf,
    ) -> Self {
        let config_dir = config_dir.as_ref().to_path_buf();
        let data_dir = data_dir.as_ref().to_path_buf();
        let state_dir = state_dir.as_ref().to_path_buf();
        let lifecycle_lock_file = state_dir
            .parent()
            .unwrap_or(&state_dir)
            .join(".dev.ryonakae.zerdr.lifecycle.lock");
        Self {
            bindings_file: state_dir.join("bindings.json"),
            leases_dir: state_dir.join("leases"),
            routes_dir: state_dir.join("routes"),
            sync_locks_dir: state_dir.join("sync-locks"),
            lifecycle_lock_file,
            install_state_file: state_dir.join("install.json"),
            plugin_dir: data_dir.join("plugin-v1"),
            zed_tasks_file,
            config_dir,
            data_dir,
            state_dir,
        }
    }
}

fn default_zed_tasks_file() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join("zed/tasks.json");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    home.join(".config/zed/tasks.json")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BindingState {
    pub schema_version: u32,
    pub session_name: String,
    pub bindings: BTreeMap<String, PathBuf>,
}

impl Default for BindingState {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            session_name: SESSION_NAME.to_owned(),
            bindings: BTreeMap::new(),
        }
    }
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
        let state: BindingState = serde_json::from_slice(&bytes).map_err(|source| Error::Json {
            what: self.path.display().to_string(),
            source,
        })?;
        validate_binding_state(&state)?;
        Ok(state)
    }

    pub fn save(&self, state: &BindingState) -> Result<()> {
        let _lock = self.acquire_write_lock()?;
        self.save_unlocked(state)
    }

    pub fn bind(&self, workspace_id: &str, candidate: &Path) -> Result<PathBuf> {
        let root = canonical_git_root(candidate)?;
        let _lock = self.acquire_write_lock()?;
        let mut state = self.load()?;
        state.bindings.insert(workspace_id.to_owned(), root.clone());
        self.save_unlocked(&state)?;
        Ok(root)
    }

    pub fn bind_if_absent(&self, workspace_id: &str, candidate: &Path) -> Result<PathBuf> {
        {
            let _lock = self.acquire_write_lock()?;
            if let Some(existing) = self.load()?.bindings.get(workspace_id) {
                return Ok(existing.clone());
            }
        }

        let root = match canonical_git_root(candidate) {
            Ok(root) => root,
            Err(resolution_error) => {
                let _lock = self.acquire_write_lock()?;
                if let Some(existing) = self.load()?.bindings.get(workspace_id) {
                    return Ok(existing.clone());
                }
                return Err(resolution_error);
            }
        };

        let _lock = self.acquire_write_lock()?;
        let mut state = self.load()?;
        if let Some(existing) = state.bindings.get(workspace_id) {
            return Ok(existing.clone());
        }
        state.bindings.insert(workspace_id.to_owned(), root.clone());
        self.save_unlocked(&state)?;
        Ok(root)
    }

    pub fn set_canonical(&self, workspace_id: &str, root: &Path) -> Result<()> {
        let root = root
            .canonicalize()
            .map_err(|error| Error::io(root, error))?;
        let _lock = self.acquire_write_lock()?;
        let mut state = self.load()?;
        state.bindings.insert(workspace_id.to_owned(), root);
        self.save_unlocked(&state)
    }

    pub fn unbind(&self, workspace_id: &str) -> Result<bool> {
        let _lock = self.acquire_write_lock()?;
        let mut state = self.load()?;
        let removed = state.bindings.remove(workspace_id).is_some();
        if removed {
            self.save_unlocked(&state)?;
        }
        Ok(removed)
    }

    pub fn get(&self, workspace_id: &str) -> Result<Option<PathBuf>> {
        Ok(self.load()?.bindings.get(workspace_id).cloned())
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
    if state.schema_version != SCHEMA_VERSION {
        return Err(Error::User(format!(
            "unsupported binding schema version {}; expected {SCHEMA_VERSION}",
            state.schema_version
        )));
    }
    if state.session_name != SESSION_NAME {
        return Err(Error::User(format!(
            "binding state belongs to session {:?}, expected {SESSION_NAME:?}",
            state.session_name
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RouteFocus {
    Terminal,
    Zed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
pub enum RouteStrategy {
    Internal { anchor_root: PathBuf },
    External { focus: RouteFocus },
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
            RouteStrategy::External { .. } => None,
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
        self.initialize_strategy(
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
        let socket_path = canonical_socket(socket_path)?;
        let routing = match routing {
            RouteStrategy::Internal { anchor_root } => RouteStrategy::Internal {
                anchor_root: canonical_git_root(&anchor_root)?,
            },
            RouteStrategy::External { focus } => RouteStrategy::External { focus },
        };
        let state = RouteState {
            schema_version: ROUTE_SCHEMA_VERSION,
            session_name: SESSION_NAME.to_owned(),
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
        validate_route(&state, &socket_path)?;
        Ok(state)
    }

    pub fn promote(&self, socket_path: &Path, anchor: &Path) -> Result<()> {
        let socket_path = canonical_socket(socket_path)?;
        let mut state = self.load(&socket_path)?;
        let RouteStrategy::Internal { anchor_root } = &mut state.routing else {
            return Err(Error::User(
                "external routes do not have a promotable anchor".to_owned(),
            ));
        };
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

fn validate_route(state: &RouteState, expected_socket: &Path) -> Result<()> {
    if !matches!(state.schema_version, SCHEMA_VERSION | ROUTE_SCHEMA_VERSION)
        || state.session_name != SESSION_NAME
    {
        return Err(Error::User(format!(
            "route has an incompatible schema or session for {}",
            expected_socket.display()
        )));
    }
    if state.schema_version == SCHEMA_VERSION
        && !matches!(state.routing, RouteStrategy::Internal { .. })
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
    if let RouteStrategy::Internal { anchor_root } = &state.routing {
        let anchor = canonical_git_root(anchor_root)?;
        if anchor != *anchor_root {
            return Err(Error::User(format!(
                "route anchor is not a canonical Git checkout root: {}",
                anchor_root.display()
            )));
        }
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
            session_name: SESSION_NAME.to_owned(),
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
                    validate_lease(&record, &socket_path)?;
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
                stale_removed: 0,
            });
        }
        let mut sweep = LeaseSweep {
            live_count: 0,
            live_scope_hashes: Vec::new(),
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
                let file = match OpenOptions::new().read(true).write(true).open(&path) {
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
                        sweep.live_count += 1;
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

fn validate_lease(record: &LeaseRecord, expected_socket: &Path) -> Result<()> {
    if record.schema_version != SCHEMA_VERSION || record.session_name != SESSION_NAME {
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
