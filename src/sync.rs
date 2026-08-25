use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::herdr::{Herdr, Workspace};
use crate::state::{
    BindingStore, DEFAULT_SESSION_NAME, LeaseSet, OperationGuard, Paths, RouteState, RouteStore,
    RouteStrategy, SyncGuard, canonical_git_root,
};
use crate::zed::Zed;

pub struct Synchronizer {
    paths: Paths,
    herdr: Herdr,
    zed: Zed,
}

struct BindingSelection {
    session_name: String,
    socket: PathBuf,
    workspace_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ActionContext {
    workspace_id: String,
    workspace_cwd: Option<PathBuf>,
    worktree: Option<ActionWorktree>,
}

#[derive(Debug, Deserialize)]
struct ActionWorktree {
    checkout_path: Option<PathBuf>,
}

impl Synchronizer {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            paths: Paths::discover()?,
            herdr: Herdr::from_env(),
            zed: Zed::from_env(),
        })
    }

    pub fn event(&self) -> Result<()> {
        let event = std::env::var("HERDR_PLUGIN_EVENT")
            .map_err(|_| Error::User("missing HERDR_PLUGIN_EVENT".to_owned()))?;
        if event != "workspace.focused" {
            return Err(Error::User(format!(
                "unexpected Herdr plugin event {event:?}; expected workspace.focused"
            )));
        }
        validate_plugin_context()?;
        let socket = std::env::var_os("HERDR_SOCKET_PATH")
            .map(PathBuf::from)
            .ok_or_else(|| Error::User("missing HERDR_SOCKET_PATH".to_owned()))?;
        let leases = LeaseSet::new(self.paths.leases_dir.clone());
        match leases.inspect(&socket) {
            Ok(inspection) if !inspection.live => return Ok(()),
            Ok(_) => {}
            Err(error) => return self.notify_socket_error(&socket, error),
        }
        let route = match RouteStore::new(self.paths.routes_dir.clone()).load(&socket) {
            Ok(route) => route,
            Err(error) => return self.notify_socket_error(&socket, error),
        };
        match self.sync_session_socket(&route.session_name, &socket) {
            Ok(_) | Err(Error::NoLiveLease { .. }) => Ok(()),
            Err(error) => self.notify_and_return(&route.session_name, error),
        }
    }

    pub fn sync_manual(&self, explicit_session: Option<&str>) -> Result<()> {
        let selection = self.session_selection(explicit_session)?;
        self.sync_session_socket(&selection.session_name, &selection.socket)?;
        Ok(())
    }

    pub fn open_from_herdr(&self) -> Result<()> {
        let action_id = std::env::var("HERDR_PLUGIN_ACTION_ID")
            .map_err(|_| Error::User("missing HERDR_PLUGIN_ACTION_ID".to_owned()))?;
        if action_id != "open-zed" {
            return Err(Error::User(format!(
                "unexpected Herdr plugin action {action_id:?}; expected open-zed"
            )));
        }
        let socket = std::env::var_os("HERDR_SOCKET_PATH")
            .map(PathBuf::from)
            .ok_or_else(|| Error::User("missing HERDR_SOCKET_PATH".to_owned()))?;
        let session_name = self.herdr.session_name_for_socket(&socket)?;
        match self.open_from_herdr_session(&session_name, &socket) {
            Ok(()) => Ok(()),
            Err(error) => self.notify_action_error(&session_name, error),
        }
    }

    pub fn bind(&self, session_name: Option<&str>, candidate: Option<&Path>) -> Result<()> {
        let selection = self.session_selection(session_name)?;
        let _guard = self.acquire_binding_authority(&selection.session_name, &selection.socket)?;
        let workspace_id = self.binding_workspace_id(&selection)?;
        let cwd;
        let candidate = if let Some(candidate) = candidate {
            candidate
        } else {
            cwd = std::env::current_dir().map_err(|error| {
                Error::User(format!("failed to read current directory: {error}"))
            })?;
            &cwd
        };
        let root = BindingStore::new(self.paths.bindings_file.clone()).bind(
            &selection.session_name,
            &workspace_id,
            candidate,
        )?;
        if self
            .binding_route(&selection.session_name, &selection.socket)?
            .is_some()
        {
            self.apply_route(&selection.session_name, &selection.socket, &root)?;
        }
        Ok(())
    }

    pub fn unbind(&self, session_name: Option<&str>) -> Result<()> {
        let selection = self.session_selection(session_name)?;
        let _guard = self.acquire_binding_authority(&selection.session_name, &selection.socket)?;
        let workspace_id = self.binding_workspace_id(&selection)?;
        BindingStore::new(self.paths.bindings_file.clone())
            .unbind(&selection.session_name, &workspace_id)?;
        Ok(())
    }

    pub fn sync_session_socket(&self, session_name: &str, socket: &Path) -> Result<PathBuf> {
        if let Some(marker) = std::env::var_os("ZERDR_TEST_SYNC_WAIT_MARKER") {
            std::fs::write(&marker, b"waiting")
                .map_err(|error| Error::io(PathBuf::from(marker), error))?;
        }
        let _guard = SyncGuard::acquire(&self.paths.sync_locks_dir, socket)?;
        self.live_route(session_name, socket)?;
        let workspaces = self.herdr.workspaces_for(session_name)?;
        let focused = focused_workspace(&workspaces)?;
        let root = self.root_for_workspace(session_name, focused)?;
        let _zed_guard = self.acquire_zed_guard()?;
        let routes = RouteStore::new(self.paths.routes_dir.clone());
        let route = self.live_route(session_name, socket)?;
        let RouteStrategy::Internal { anchor_root } = &route.routing;
        self.zed.activate_existing(anchor_root)?;
        self.zed.add_to_current(&root)?;
        routes.promote_for(session_name, socket, &root)?;
        Ok(root)
    }

    pub fn root_for_workspace(&self, session_name: &str, workspace: &Workspace) -> Result<PathBuf> {
        let store = BindingStore::new(self.paths.bindings_file.clone());
        if let Some(root) = store.get(session_name, &workspace.id)? {
            if !root.exists() {
                return Err(Error::User(format!(
                    "binding for {} points to missing path {}; run `zerdr workspace bind PATH`",
                    workspace.id,
                    root.display()
                )));
            }
            let canonical = canonical_git_root(&root)?;
            if canonical != root {
                return Err(Error::User(format!(
                    "binding for {} is not the canonical Git checkout root: {}",
                    workspace.id,
                    root.display()
                )));
            }
            return Ok(root);
        }
        let discovered_cwd;
        let candidate = if let Some(checkout) = workspace.checkout_path.as_deref() {
            checkout
        } else {
            discovered_cwd = self.herdr.workspace_cwd(session_name, workspace)?;
            discovered_cwd.as_deref().ok_or_else(|| {
                Error::User(format!(
                    "workspace {} has no checkout path or working directory; run `zerdr workspace bind PATH`",
                    workspace.id
                ))
            })?
        };
        store.bind_if_absent(session_name, &workspace.id, candidate)
    }

    pub fn herdr(&self) -> &Herdr {
        &self.herdr
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    pub fn notification_session_name(&self, explicit_session: Option<&str>) -> String {
        if let Some(session_name) = explicit_session {
            return session_name.to_owned();
        }
        std::env::var_os("HERDR_SOCKET_PATH")
            .map(PathBuf::from)
            .and_then(|socket| self.herdr.session_name_for_socket(&socket).ok())
            .unwrap_or_else(|| DEFAULT_SESSION_NAME.to_owned())
    }

    fn acquire_zed_guard(&self) -> Result<OperationGuard> {
        if let Some(marker) = std::env::var_os("ZERDR_TEST_BEFORE_ZED_LOCK_MARKER") {
            std::fs::write(&marker, b"waiting")
                .map_err(|error| Error::io(PathBuf::from(marker), error))?;
        }
        if let Some(marker) = std::env::var_os("ZERDR_TEST_ZED_LOCK_BLOCKED_MARKER") {
            if let Some(guard) = OperationGuard::try_acquire(&self.paths.zed_lock_file)? {
                return Ok(guard);
            }
            std::fs::write(&marker, b"blocked")
                .map_err(|error| Error::io(PathBuf::from(marker), error))?;
        }
        OperationGuard::acquire(&self.paths.zed_lock_file)
    }

    fn live_route(&self, session_name: &str, socket: &Path) -> Result<RouteState> {
        let inspection =
            LeaseSet::new(self.paths.leases_dir.clone()).inspect_for(session_name, socket)?;
        let wrapper_pid = match inspection.live_wrapper_pids.as_slice() {
            [] => {
                return Err(Error::NoLiveLease {
                    session_name: session_name.to_owned(),
                });
            }
            [wrapper_pid] => *wrapper_pid,
            wrapper_pids => {
                return Err(Error::User(format!(
                    "the {session_name} session has {} live wrappers ({wrapper_pids:?}); keep only one wrapper for that session",
                    wrapper_pids.len()
                )));
            }
        };
        let route =
            RouteStore::new(self.paths.routes_dir.clone()).load_for(session_name, socket)?;
        if route.wrapper_pid != wrapper_pid {
            return Err(Error::User(format!(
                "route belongs to wrapper {}, but live wrapper is {wrapper_pid}; restart `zerdr --session {session_name}`",
                route.wrapper_pid
            )));
        }
        Ok(route)
    }

    fn open_from_herdr_session(&self, session_name: &str, socket: &Path) -> Result<()> {
        let context = action_context_from_env()?;
        let root = self.action_root(session_name, &context)?;
        if let Some(marker) = std::env::var_os("ZERDR_TEST_SYNC_WAIT_MARKER") {
            std::fs::write(&marker, b"waiting")
                .map_err(|error| Error::io(PathBuf::from(marker), error))?;
        }
        let _guard = SyncGuard::acquire(&self.paths.sync_locks_dir, socket)?;
        let _zed_guard = self.acquire_zed_guard()?;
        let inspection =
            LeaseSet::new(self.paths.leases_dir.clone()).inspect_for(session_name, socket)?;
        match inspection.live_wrapper_pids.as_slice() {
            [] => self.zed.activate_existing(&root),
            [wrapper_pid] => {
                let route = RouteStore::new(self.paths.routes_dir.clone())
                    .load_for(session_name, socket)?;
                if route.wrapper_pid != *wrapper_pid {
                    return Err(Error::User(format!(
                        "route belongs to wrapper {}, but live wrapper is {wrapper_pid}; restart `zerdr --session {session_name}`",
                        route.wrapper_pid
                    )));
                }
                self.apply_action_route(session_name, socket, &route, &root)
            }
            wrapper_pids => Err(Error::User(format!(
                "the {session_name} Herdr session has {} live wrappers ({wrapper_pids:?}); keep only one wrapper for that session",
                wrapper_pids.len()
            ))),
        }
    }

    fn apply_action_route(
        &self,
        session_name: &str,
        socket: &Path,
        route: &RouteState,
        root: &Path,
    ) -> Result<()> {
        let RouteStrategy::Internal { anchor_root } = &route.routing;
        self.zed.activate_existing(anchor_root)?;
        self.zed.add_to_current(root)?;
        RouteStore::new(self.paths.routes_dir.clone()).promote_for(session_name, socket, root)?;
        Ok(())
    }

    fn action_root(&self, session_name: &str, context: &ActionContext) -> Result<PathBuf> {
        let store = BindingStore::new(self.paths.bindings_file.clone());
        if let Some(root) = store.get(session_name, &context.workspace_id)? {
            if !root.exists() {
                return Err(Error::User(format!(
                    "binding for {} points to missing path {}; run `zerdr workspace bind PATH`",
                    context.workspace_id,
                    root.display()
                )));
            }
            let canonical = canonical_git_root(&root)?;
            if canonical != root {
                return Err(Error::User(format!(
                    "binding for {} is not the canonical Git checkout root: {}",
                    context.workspace_id,
                    root.display()
                )));
            }
            return Ok(root);
        }
        let candidate = context
            .worktree
            .as_ref()
            .and_then(|worktree| worktree.checkout_path.as_deref())
            .or(context.workspace_cwd.as_deref())
            .ok_or_else(|| {
                Error::User(format!(
                    "workspace {} has no checkout path or working directory; run `zerdr workspace bind PATH`",
                    context.workspace_id
                ))
            })?;
        canonical_git_root(candidate)
    }

    fn notify_action_error(&self, session_name: &str, error: Error) -> Result<()> {
        let message = error.to_string();
        match self.herdr.notify_error_for(session_name, &message) {
            Ok(_) => Err(error),
            Err(notification_error) => Err(Error::User(format!(
                "{message}; additionally failed to notify Herdr: {notification_error}"
            ))),
        }
    }

    fn session_selection(&self, explicit_session: Option<&str>) -> Result<BindingSelection> {
        if let Some(session_name) = explicit_session {
            return Ok(BindingSelection {
                session_name: session_name.to_owned(),
                socket: self.herdr.session_socket_for(session_name)?,
                workspace_id: None,
            });
        }

        let socket = std::env::var_os("HERDR_SOCKET_PATH");
        let workspace_id = std::env::var_os("HERDR_WORKSPACE_ID");
        match (socket, workspace_id) {
            (Some(socket), Some(workspace_id)) => {
                let socket = PathBuf::from(socket);
                let session_name = self.herdr.session_name_for_socket(&socket)?;
                let workspace_id = workspace_id.into_string().map_err(|_| {
                    Error::User("HERDR_WORKSPACE_ID must be valid UTF-8".to_owned())
                })?;
                Ok(BindingSelection {
                    session_name,
                    socket,
                    workspace_id: Some(workspace_id),
                })
            }
            (None, None) => Ok(BindingSelection {
                session_name: DEFAULT_SESSION_NAME.to_owned(),
                socket: self.herdr.session_socket()?,
                workspace_id: None,
            }),
            _ => Err(Error::User(
                "HERDR_SOCKET_PATH and HERDR_WORKSPACE_ID must be set together".to_owned(),
            )),
        }
    }

    fn binding_workspace_id(&self, selection: &BindingSelection) -> Result<String> {
        if let Some(workspace_id) = selection.workspace_id.as_ref() {
            return Ok(workspace_id.clone());
        }
        let workspaces = self.herdr.workspaces_for(&selection.session_name)?;
        Ok(focused_workspace(&workspaces)?.id.clone())
    }

    fn acquire_binding_authority(&self, session_name: &str, socket: &Path) -> Result<SyncGuard> {
        let guard = SyncGuard::acquire(&self.paths.sync_locks_dir, socket)?;
        self.binding_route(session_name, socket)?;
        Ok(guard)
    }

    fn binding_route(&self, session_name: &str, socket: &Path) -> Result<Option<RouteState>> {
        let inspection =
            LeaseSet::new(self.paths.leases_dir.clone()).inspect_for(session_name, socket)?;
        let wrapper_pid = match inspection.live_wrapper_pids.as_slice() {
            [] => return Ok(None),
            [wrapper_pid] => *wrapper_pid,
            wrapper_pids => {
                return Err(Error::User(format!(
                    "the {session_name} Herdr session has {} live wrappers ({wrapper_pids:?}); keep only one wrapper for that session",
                    wrapper_pids.len()
                )));
            }
        };
        let route =
            RouteStore::new(self.paths.routes_dir.clone()).load_for(session_name, socket)?;
        if route.wrapper_pid != wrapper_pid {
            return Err(Error::User(format!(
                "route belongs to wrapper {}, but live wrapper is {wrapper_pid}; restart `zerdr --session {session_name}`",
                route.wrapper_pid
            )));
        }
        Ok(Some(route))
    }

    fn apply_route(&self, session_name: &str, socket: &Path, root: &Path) -> Result<()> {
        let _zed_guard = self.acquire_zed_guard()?;
        let Some(route) = self.binding_route(session_name, socket)? else {
            return Ok(());
        };
        let RouteStrategy::Internal { anchor_root } = &route.routing;
        self.zed.activate_existing(anchor_root)?;
        self.zed.add_to_current(root)?;
        RouteStore::new(self.paths.routes_dir.clone()).promote_for(session_name, socket, root)?;
        Ok(())
    }

    fn notify_socket_error(&self, socket: &Path, error: Error) -> Result<()> {
        let session_name = RouteStore::new(self.paths.routes_dir.clone())
            .load(socket)
            .map(|route| route.session_name)
            .or_else(|_| self.herdr.session_name_for_socket(socket))
            .unwrap_or_else(|_| DEFAULT_SESSION_NAME.to_owned());
        self.notify_and_return(&session_name, error)
    }

    fn notify_and_return(&self, session_name: &str, error: Error) -> Result<()> {
        let message = error.to_string();
        match self.herdr.notify_error_for(session_name, &message) {
            Ok(_) => Err(error),
            Err(notification_error) => Err(Error::User(format!(
                "{message}; additionally failed to notify Herdr: {notification_error}"
            ))),
        }
    }
}

pub fn focused_workspace(workspaces: &[Workspace]) -> Result<&Workspace> {
    workspaces
        .iter()
        .find(|workspace| workspace.focused)
        .ok_or_else(|| Error::User("Herdr has no focused workspace".to_owned()))
}

fn action_context_from_env() -> Result<ActionContext> {
    let raw = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .map_err(|_| Error::User("missing HERDR_PLUGIN_CONTEXT_JSON".to_owned()))?;
    let value: Value = serde_json::from_str(&raw).map_err(|source| Error::Json {
        what: "HERDR_PLUGIN_CONTEXT_JSON".to_owned(),
        source,
    })?;
    if !value.is_object() {
        return Err(Error::User(
            "HERDR_PLUGIN_CONTEXT_JSON must be a JSON object".to_owned(),
        ));
    }
    serde_json::from_value(value).map_err(|source| Error::Json {
        what: "HERDR_PLUGIN_CONTEXT_JSON".to_owned(),
        source,
    })
}

fn validate_plugin_context() -> Result<()> {
    let raw = std::env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .map_err(|_| Error::User("missing HERDR_PLUGIN_CONTEXT_JSON".to_owned()))?;
    let value: Value = serde_json::from_str(&raw).map_err(|source| Error::Json {
        what: "HERDR_PLUGIN_CONTEXT_JSON".to_owned(),
        source,
    })?;
    if !value.is_object() {
        return Err(Error::User(
            "HERDR_PLUGIN_CONTEXT_JSON must be a JSON object".to_owned(),
        ));
    }
    if value.get("workspace_id").and_then(Value::as_str).is_none() {
        return Err(Error::User(
            "Herdr plugin context is missing workspace_id".to_owned(),
        ));
    }
    Ok(())
}
