use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{Error, Result};
use crate::herdr::{Herdr, Workspace};
use crate::picker;
use crate::state::{
    BindingStore, LeaseSet, Paths, RouteState, RouteStore, RouteStrategy, SESSION_NAME, SyncGuard,
    canonical_git_root,
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
        match leases.has_live(&socket) {
            Ok(false) => return Ok(()),
            Ok(true) => {}
            Err(error) => return self.notify_and_return(error),
        }
        match self.sync_socket(&socket) {
            Ok(_) | Err(Error::NoLiveLease) => Ok(()),
            Err(error) => self.notify_and_return(error),
        }
    }

    pub fn sync_manual(&self) -> Result<()> {
        let socket = self.herdr.session_socket()?;
        self.sync_socket(&socket)?;
        Ok(())
    }

    pub fn navigate(&self, direction: isize) -> Result<()> {
        let socket = self.herdr.session_socket()?;
        let authority = self.acquire_manual_authority(&socket)?;
        let workspaces = self.herdr.workspaces()?;
        let current = workspaces
            .iter()
            .position(|workspace| workspace.focused)
            .ok_or_else(|| Error::User("Herdr has no focused workspace".to_owned()))?;
        if workspaces.is_empty() {
            return Err(Error::User("Herdr has no workspaces".to_owned()));
        }
        let len = workspaces.len() as isize;
        let target = (current as isize + direction).rem_euclid(len) as usize;
        self.switch_or_sync(&socket, &workspaces[target], authority)
    }

    pub fn pick(&self) -> Result<()> {
        let socket = self.herdr.session_socket()?;
        let authority = self.acquire_manual_authority(&socket)?;
        let mut workspaces = self.herdr.workspaces()?;
        let bindings = BindingStore::new(self.paths.bindings_file.clone()).load()?;
        for workspace in &mut workspaces {
            if let Some(root) = bindings
                .sessions
                .get(SESSION_NAME)
                .and_then(|session| session.get(&workspace.id))
            {
                workspace.checkout_path = Some(root.clone());
            }
        }
        drop(authority);
        let Some(index) = picker::choose(&workspaces)? else {
            return Ok(());
        };
        let authority = self.acquire_manual_authority(&socket)?;
        self.switch_or_sync(&socket, &workspaces[index], authority)
    }

    pub fn bind(&self, session_name: Option<&str>, candidate: Option<&Path>) -> Result<()> {
        let selection = self.binding_selection(session_name)?;
        let _guard = self.acquire_binding_authority(&selection.socket)?;
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
        if let Some(route) = self.binding_route(&selection.socket)? {
            self.apply_route(&selection.socket, &route, &root)?;
        }
        Ok(())
    }

    pub fn unbind(&self, session_name: Option<&str>) -> Result<()> {
        let selection = self.binding_selection(session_name)?;
        let _guard = self.acquire_binding_authority(&selection.socket)?;
        let workspace_id = self.binding_workspace_id(&selection)?;
        BindingStore::new(self.paths.bindings_file.clone())
            .unbind(&selection.session_name, &workspace_id)?;
        Ok(())
    }

    pub fn sync_socket(&self, socket: &Path) -> Result<PathBuf> {
        if let Some(marker) = std::env::var_os("ZERDR_TEST_SYNC_WAIT_MARKER") {
            std::fs::write(&marker, b"waiting")
                .map_err(|error| Error::io(PathBuf::from(marker), error))?;
        }
        let _guard = SyncGuard::acquire(&self.paths.sync_locks_dir, socket)?;
        let inspection = LeaseSet::new(self.paths.leases_dir.clone()).inspect(socket)?;
        let wrapper_pid = match inspection.live_wrapper_pids.as_slice() {
            [] => return Err(Error::NoLiveLease),
            [wrapper_pid] => *wrapper_pid,
            wrapper_pids => {
                return Err(Error::User(format!(
                    "the zerdr session has {} live wrappers ({wrapper_pids:?}); keep only one bare `zerdr` wrapper",
                    wrapper_pids.len()
                )));
            }
        };
        let routes = RouteStore::new(self.paths.routes_dir.clone());
        let route = routes.load(socket)?;
        if route.wrapper_pid != wrapper_pid {
            return Err(Error::User(format!(
                "route belongs to wrapper {}, but live wrapper is {wrapper_pid}; restart bare `zerdr`",
                route.wrapper_pid
            )));
        }
        let workspaces = self.herdr.workspaces()?;
        let focused = focused_workspace(&workspaces)?;
        let root = self.root_for_workspace(focused)?;
        match &route.routing {
            RouteStrategy::Internal { anchor_root } => {
                self.zed.activate_existing(anchor_root)?;
                self.zed.add_to_current(&root)?;
                routes.promote(socket, &root)?;
            }
            RouteStrategy::External { focus } => {
                crate::focus::with_external_focus(*focus, || self.zed.activate_existing(&root))?;
            }
        }
        Ok(root)
    }

    pub fn root_for_workspace(&self, workspace: &Workspace) -> Result<PathBuf> {
        let store = BindingStore::new(self.paths.bindings_file.clone());
        if let Some(root) = store.get(SESSION_NAME, &workspace.id)? {
            if !root.exists() {
                return Err(Error::User(format!(
                    "binding for {} points to missing path {}; run `zerdr bind PATH`",
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
            discovered_cwd = self.herdr.workspace_cwd(workspace)?;
            discovered_cwd.as_deref().ok_or_else(|| {
                Error::User(format!(
                    "workspace {} has no checkout path or working directory; run `zerdr bind PATH`",
                    workspace.id
                ))
            })?
        };
        store.bind_if_absent(SESSION_NAME, &workspace.id, candidate)
    }

    pub fn herdr(&self) -> &Herdr {
        &self.herdr
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    fn binding_selection(&self, explicit_session: Option<&str>) -> Result<BindingSelection> {
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
                session_name: SESSION_NAME.to_owned(),
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

    fn acquire_binding_authority(&self, socket: &Path) -> Result<SyncGuard> {
        let guard = SyncGuard::acquire(&self.paths.sync_locks_dir, socket)?;
        self.binding_route(socket)?;
        Ok(guard)
    }

    fn binding_route(&self, socket: &Path) -> Result<Option<RouteState>> {
        let inspection = LeaseSet::new(self.paths.leases_dir.clone()).inspect(socket)?;
        let wrapper_pid = match inspection.live_wrapper_pids.as_slice() {
            [] => return Ok(None),
            [wrapper_pid] => *wrapper_pid,
            wrapper_pids => {
                return Err(Error::User(format!(
                    "the Herdr session has {} live wrappers ({wrapper_pids:?}); keep only one bare `zerdr` wrapper",
                    wrapper_pids.len()
                )));
            }
        };
        let route = RouteStore::new(self.paths.routes_dir.clone()).load(socket)?;
        if route.wrapper_pid != wrapper_pid {
            return Err(Error::User(format!(
                "route belongs to wrapper {}, but live wrapper is {wrapper_pid}; restart bare `zerdr`",
                route.wrapper_pid
            )));
        }
        Ok(Some(route))
    }

    fn apply_route(&self, socket: &Path, route: &RouteState, root: &Path) -> Result<()> {
        match &route.routing {
            RouteStrategy::Internal { anchor_root } => {
                self.zed.activate_existing(anchor_root)?;
                self.zed.add_to_current(root)?;
                RouteStore::new(self.paths.routes_dir.clone()).promote(socket, root)?;
            }
            RouteStrategy::External { focus } => {
                crate::focus::with_external_focus(*focus, || self.zed.activate_existing(root))?;
            }
        }
        Ok(())
    }

    fn acquire_manual_authority(&self, socket: &Path) -> Result<SyncGuard> {
        let guard = SyncGuard::acquire(&self.paths.sync_locks_dir, socket)?;
        self.validate_route_authority(socket)?;
        Ok(guard)
    }

    fn validate_route_authority(&self, socket: &Path) -> Result<()> {
        let inspection = LeaseSet::new(self.paths.leases_dir.clone()).inspect(socket)?;
        let wrapper_pid = match inspection.live_wrapper_pids.as_slice() {
            [] => return Err(Error::NoLiveLease),
            [wrapper_pid] => *wrapper_pid,
            wrapper_pids => {
                return Err(Error::User(format!(
                    "the zerdr session has {} live wrappers ({wrapper_pids:?}); keep only one bare `zerdr` wrapper",
                    wrapper_pids.len()
                )));
            }
        };
        let route = RouteStore::new(self.paths.routes_dir.clone()).load(socket)?;
        if route.wrapper_pid != wrapper_pid {
            return Err(Error::User(format!(
                "route belongs to wrapper {}, but live wrapper is {wrapper_pid}; restart bare `zerdr`",
                route.wrapper_pid
            )));
        }
        Ok(())
    }

    fn switch_or_sync(
        &self,
        socket: &Path,
        target: &Workspace,
        authority: SyncGuard,
    ) -> Result<()> {
        self.validate_route_authority(socket)?;
        self.root_for_workspace(target)?;
        if target.focused {
            drop(authority);
            self.sync_socket(socket)?;
        } else {
            self.herdr.focus_workspace(&target.id)?;
        }
        Ok(())
    }

    fn notify_and_return(&self, error: Error) -> Result<()> {
        let message = error.to_string();
        match self.herdr.notify_error(&message) {
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
