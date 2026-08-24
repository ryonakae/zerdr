use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use serde_json::Value;
use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::{Handle as SignalHandle, Signals};

use crate::error::{Error, Result};
use crate::state::{
    DEFAULT_SESSION_NAME, LeaseSet, LifecycleGuard, Paths, RouteStore, RouteStrategy, SyncGuard,
};

const PLUGIN_ID: &str = "zerdr";
use crate::sync::Synchronizer;

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: String,
    pub label: String,
    pub number: Option<u64>,
    pub focused: bool,
    pub active_tab_id: Option<String>,
    pub checkout_path: Option<PathBuf>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub kind: String,
    /// Herdr's assigned agent name, absent for agents started outside `agent start`.
    pub name: Option<String>,
    pub status: String,
    pub pane_id: String,
    pub workspace_id: String,
    pub title: Option<String>,
    /// The terminal title as the agent set it, leading glyph intact.
    pub raw_title: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreatedWorkspace {
    pub workspace_id: String,
    pub root_pane_id: String,
}

#[derive(Debug, Clone)]
pub struct Herdr {
    program: OsString,
}

impl Herdr {
    pub fn from_env() -> Self {
        Self {
            program: std::env::var_os("ZERDR_HERDR_BIN").unwrap_or_else(|| "herdr".into()),
        }
    }

    pub fn with_program(program: OsString) -> Self {
        Self { program }
    }

    pub fn agents_for(&self, session_name: &str) -> Result<Vec<AgentInfo>> {
        let value = self.session_json_output_for(session_name, ["agent", "list"])?;
        let values = find_array(&value, "agents")
            .ok_or_else(|| Error::User("Herdr agent list did not contain agents".to_owned()))?;
        values.iter().map(parse_agent).collect()
    }

    pub fn agent_get_for(&self, session_name: &str, target: &str) -> Result<Option<AgentInfo>> {
        let value = self.session_json_output_for(session_name, ["agent", "get", target])?;
        let Some(agent) = value
            .pointer("/result/agent")
            .or_else(|| value.get("agent").filter(|agent| agent.is_object()))
        else {
            return Ok(None);
        };
        parse_agent(agent).map(Some)
    }

    pub fn agent_start_for(
        &self,
        session_name: &str,
        name: &str,
        kind: &str,
        pane_id: &str,
    ) -> Result<()> {
        self.session_output_for(
            session_name,
            [
                OsStr::new("agent"),
                OsStr::new("start"),
                OsStr::new(name),
                OsStr::new("--kind"),
                OsStr::new(kind),
                OsStr::new("--pane"),
                OsStr::new(pane_id),
            ],
        )?;
        Ok(())
    }

    pub fn tab_create_for(
        &self,
        session_name: &str,
        workspace_id: &str,
        cwd: &std::path::Path,
    ) -> Result<String> {
        let value = self.session_json_output_for(
            session_name,
            [
                OsStr::new("tab"),
                OsStr::new("create"),
                OsStr::new("--workspace"),
                OsStr::new(workspace_id),
                OsStr::new("--cwd"),
                cwd.as_os_str(),
                OsStr::new("--no-focus"),
            ],
        )?;
        root_pane_id(&value)
            .ok_or_else(|| Error::User("Herdr tab create did not return a root pane".to_owned()))
    }

    pub fn workspace_create_for(
        &self,
        session_name: &str,
        cwd: &std::path::Path,
        label: &str,
    ) -> Result<CreatedWorkspace> {
        let value = self.session_json_output_for(
            session_name,
            [
                OsStr::new("workspace"),
                OsStr::new("create"),
                OsStr::new("--cwd"),
                cwd.as_os_str(),
                OsStr::new("--label"),
                OsStr::new(label),
                OsStr::new("--no-focus"),
            ],
        )?;
        let workspace_id = value
            .pointer("/result/workspace/workspace_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::User("Herdr workspace create did not return a workspace".to_owned())
            })?
            .to_owned();
        let root_pane_id = root_pane_id(&value).ok_or_else(|| {
            Error::User("Herdr workspace create did not return a root pane".to_owned())
        })?;
        Ok(CreatedWorkspace {
            workspace_id,
            root_pane_id,
        })
    }

    pub fn spawn_agent_attach_for(&self, session_name: &str, target: &str) -> Result<Child> {
        self.spawn_attach(session_name, "agent", target)
    }

    pub fn spawn_terminal_attach_for(
        &self,
        session_name: &str,
        terminal_id: &str,
    ) -> Result<Child> {
        self.spawn_attach(session_name, "terminal", terminal_id)
    }

    fn spawn_attach(&self, session_name: &str, surface: &str, target: &str) -> Result<Child> {
        Command::new(&self.program)
            .args(["--session", session_name, surface, "attach", target])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| Error::User(format!("failed to attach to Herdr {surface}: {error}")))
    }

    pub fn pane_terminal_for(&self, session_name: &str, pane_id: &str) -> Result<String> {
        let value = self.session_json_output_for(session_name, ["pane", "get", pane_id])?;
        value
            .pointer("/result/pane/terminal_id")
            .or_else(|| value.pointer("/pane/terminal_id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                Error::User(format!("Herdr pane {pane_id} did not report a terminal id"))
            })
    }

    pub fn session_socket(&self) -> Result<PathBuf> {
        self.find_session_socket(DEFAULT_SESSION_NAME)?
            .ok_or(Error::SessionUnavailable)
    }

    pub fn session_socket_for(&self, session_name: &str) -> Result<PathBuf> {
        self.session_socket_if_running(session_name)?
            .ok_or_else(|| Error::User(format!("Herdr session {session_name:?} is not running")))
    }

    pub fn session_socket_if_running(&self, session_name: &str) -> Result<Option<PathBuf>> {
        self.find_session_socket(session_name)
    }

    pub fn session_name_for_socket(&self, socket_path: &std::path::Path) -> Result<String> {
        let expected = socket_path
            .canonicalize()
            .map_err(|error| Error::io(socket_path, error))?;
        let value = self.json_output(["session", "list", "--json"])?;
        let sessions = find_array(&value, "sessions")
            .ok_or_else(|| Error::User("Herdr session list did not contain sessions".to_owned()))?;
        for session in sessions {
            if !session
                .get("running")
                .and_then(Value::as_bool)
                .unwrap_or(true)
            {
                continue;
            }
            let Some(name) = string_field(session, &["name", "session_name"]) else {
                continue;
            };
            let Some(socket) = string_field(session, &["socket_path", "socket"]) else {
                continue;
            };
            let socket = PathBuf::from(socket);
            if socket.canonicalize().ok().as_ref() == Some(&expected) {
                return Ok(name);
            }
        }
        Err(Error::User(format!(
            "no running Herdr session matches socket {}",
            expected.display()
        )))
    }

    pub fn workspaces_for(&self, session_name: &str) -> Result<Vec<Workspace>> {
        let value = self.session_json_output_for(session_name, ["workspace", "list"])?;
        let values = find_array(&value, "workspaces").ok_or_else(|| {
            Error::User("Herdr workspace list did not contain workspaces".to_owned())
        })?;
        values.iter().map(parse_workspace).collect()
    }

    pub fn focus_workspace_for(&self, session_name: &str, workspace_id: &str) -> Result<()> {
        self.session_output_for(
            session_name,
            [
                OsStr::new("workspace"),
                OsStr::new("focus"),
                OsStr::new(workspace_id),
            ],
        )?;
        Ok(())
    }

    pub fn notify_error_for(&self, session_name: &str, message: &str) -> Result<bool> {
        let output = self.session_output_for(
            session_name,
            [
                OsStr::new("notification"),
                OsStr::new("show"),
                OsStr::new("zerdr: sync failed"),
                OsStr::new("--body"),
                OsStr::new(message),
                OsStr::new("--position"),
                OsStr::new("top-right"),
                OsStr::new("--sound"),
                OsStr::new("request"),
            ],
        )?;
        let value: Value =
            serde_json::from_slice(&output.stdout).map_err(|source| Error::Json {
                what: "Herdr notification response".to_owned(),
                source,
            })?;
        if value
            .pointer("/result/reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason == "no_foreground_client")
        {
            return Ok(false);
        }
        Ok(value
            .pointer("/result/shown")
            .and_then(Value::as_bool)
            .unwrap_or(true))
    }

    pub fn plugin_link(&self, plugin_root: &std::path::Path) -> Result<()> {
        self.output([
            OsStr::new("plugin"),
            OsStr::new("link"),
            plugin_root.as_os_str(),
            OsStr::new("--enabled"),
        ])?;
        Ok(())
    }

    pub fn plugin_uninstall(&self) -> Result<()> {
        self.output(["plugin", "uninstall", PLUGIN_ID])?;
        Ok(())
    }

    pub fn plugin_list(&self) -> Result<Value> {
        self.json_output(["plugin", "list", "--plugin", PLUGIN_ID, "--json"])
    }

    pub fn version(&self) -> Result<String> {
        let output = self.output(["--version"])?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    pub fn spawn_client(&self, session_name: &str) -> Result<Child> {
        let mut command = Command::new(&self.program);
        if session_name != DEFAULT_SESSION_NAME {
            command.args(["--session", session_name]);
        }
        command
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| Error::User(format!("failed to launch Herdr: {error}")))
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    pub fn workspace_cwd(
        &self,
        session_name: &str,
        workspace: &Workspace,
    ) -> Result<Option<PathBuf>> {
        if let Some(cwd) = workspace.cwd.as_ref() {
            return Ok(Some(cwd.clone()));
        }
        let Some(tab_id) = workspace.active_tab_id.as_deref() else {
            return Ok(None);
        };
        let value = self.session_json_output_for(session_name, ["api", "snapshot"])?;
        let snapshot = value
            .pointer("/result/snapshot")
            .or_else(|| value.get("result"))
            .unwrap_or(&value);
        let layouts = snapshot
            .get("layouts")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::User("Herdr snapshot did not contain layouts".to_owned()))?;
        let panes = snapshot
            .get("panes")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::User("Herdr snapshot did not contain panes".to_owned()))?;

        let focused_pane_id = layouts.iter().find_map(|entry| {
            let entry_tab = string_field(entry, &["tab_id"]).or_else(|| {
                entry
                    .pointer("/layout/tab_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            });
            if entry_tab.as_deref() != Some(tab_id) {
                return None;
            }
            string_field(entry, &["focused_pane_id"]).or_else(|| {
                entry
                    .pointer("/layout/focused_pane_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
        });
        Ok(focused_pane_id.as_deref().and_then(|pane_id| {
            panes.iter().find_map(|pane| {
                if string_field(pane, &["pane_id"]).as_deref() == Some(pane_id) {
                    string_field(pane, &["cwd"]).map(PathBuf::from)
                } else {
                    None
                }
            })
        }))
    }

    fn find_session_socket(&self, session_name: &str) -> Result<Option<PathBuf>> {
        let value = self.json_output(["session", "list", "--json"])?;
        let sessions = find_array(&value, "sessions")
            .ok_or_else(|| Error::User("Herdr session list did not contain sessions".to_owned()))?;
        for session in sessions {
            let name = string_field(session, &["name", "session_name"]);
            if name.as_deref() == Some(session_name)
                && session
                    .get("running")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            {
                let socket =
                    string_field(session, &["socket_path", "socket"]).ok_or_else(|| {
                        Error::User(format!("Herdr session {session_name:?} has no socket path"))
                    })?;
                return Ok(Some(PathBuf::from(socket)));
            }
        }
        Ok(None)
    }

    fn session_json_output_for<I, S>(&self, session_name: &str, args: I) -> Result<Value>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut complete = vec![OsString::from("--session"), OsString::from(session_name)];
        complete.extend(args.into_iter().map(|value| value.as_ref().to_os_string()));
        self.json_output(complete)
    }

    fn session_output_for<I, S>(&self, session_name: &str, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut complete = vec![OsString::from("--session"), OsString::from(session_name)];
        complete.extend(args.into_iter().map(|value| value.as_ref().to_os_string()));
        self.output(complete)
    }

    fn json_output<I, S>(&self, args: I) -> Result<Value>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.output(args)?;
        serde_json::from_slice(&output.stdout).map_err(|source| Error::Json {
            what: format!("{} output", self.program.to_string_lossy()),
            source,
        })
    }

    fn output<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new(&self.program)
            .args(args)
            .output()
            .map_err(|error| {
                Error::User(format!(
                    "failed to run {}: {error}",
                    self.program.to_string_lossy()
                ))
            })?;
        if !output.status.success() {
            return Err(Error::Process {
                program: self.program.to_string_lossy().into_owned(),
                status: output.status.code().unwrap_or(1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(output)
    }
}

pub fn run_wrapper(session_name: &str, routing: RouteStrategy) -> Result<()> {
    let herdr = Herdr::from_env();
    let paths = Paths::discover()?;
    crate::setup::validate_launcher_installation(&paths, &herdr)?;
    let mut child = ManagedChild::new(herdr.spawn_client(session_name)?);
    let _signals = SignalForwarder::new(child.id())?;
    let timeout_ms = std::env::var("ZERDR_READY_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(5_000);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let socket = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| Error::User(format!("failed to wait for Herdr: {error}")))?
        {
            return if status.success() {
                Ok(())
            } else {
                Err(Error::ChildExit(status.code().unwrap_or(1)))
            };
        }
        if let Ok(socket) = herdr.session_socket_for(session_name) {
            break socket;
        }
        if Instant::now() >= deadline {
            child.terminate();
            return Err(Error::User(format!(
                "timed out waiting for the {session_name} Herdr session socket"
            )));
        }
        thread::sleep(Duration::from_millis(25));
    };

    let lifecycle = LifecycleGuard::acquire(&paths.lifecycle_lock_file)?;
    if let Some(marker) = std::env::var_os("ZERDR_TEST_ADMISSION_LOCK_MARKER") {
        std::fs::write(&marker, b"locked")
            .map_err(|error| Error::io(PathBuf::from(marker), error))?;
    }
    if let Some(continue_file) = std::env::var_os("ZERDR_TEST_ADMISSION_CONTINUE") {
        let continue_file = PathBuf::from(continue_file);
        while !continue_file.exists() {
            thread::sleep(Duration::from_millis(10));
        }
    }
    let admission = SyncGuard::acquire(&paths.sync_locks_dir, &socket)?;
    let leases = LeaseSet::new(paths.leases_dir.clone());
    if leases.inspect_for(session_name, &socket)?.live {
        return Err(Error::User(format!(
            "the {session_name} Herdr session already has a live wrapper"
        )));
    }
    RouteStore::new(paths.routes_dir.clone()).initialize_strategy_for(
        session_name,
        &socket,
        routing.clone(),
        std::process::id(),
    )?;
    let _lease = leases.acquire_for(session_name, &socket, child.id())?;
    drop(admission);
    drop(lifecycle);
    let synchronizer = Synchronizer::from_env()?;
    if let Err(error) = synchronizer.sync_session_socket(session_name, &socket) {
        let message = format!("startup synchronization failed: {error}");
        let _ = herdr.notify_error_for(session_name, &message);
        eprintln!("zerdr: {message}");
    }

    let status = child
        .wait()
        .map_err(|error| Error::User(format!("failed to wait for Herdr: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::ChildExit(status.code().unwrap_or(1)))
    }
}

pub(crate) struct ManagedChild {
    child: Child,
    running: bool,
}

impl ManagedChild {
    pub(crate) fn new(child: Child) -> Self {
        Self {
            child,
            running: true,
        }
    }

    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.running = false;
        }
        Ok(status)
    }

    pub(crate) fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait()?;
        self.running = false;
        Ok(status)
    }

    pub(crate) fn terminate(&mut self) {
        if self.running {
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.running = false;
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

pub(crate) struct SignalForwarder {
    handle: SignalHandle,
    thread: Option<thread::JoinHandle<()>>,
}

impl SignalForwarder {
    pub(crate) fn new(child_pid: u32) -> Result<Self> {
        let mut signals = Signals::new([SIGINT, SIGTERM, SIGHUP])
            .map_err(|error| Error::User(format!("failed to register signal handlers: {error}")))?;
        let handle = signals.handle();
        let thread = thread::spawn(move || {
            for signal in signals.forever() {
                if let Ok(signal) = Signal::try_from(signal) {
                    let _ = kill(Pid::from_raw(child_pid as i32), signal);
                }
            }
        });
        Ok(Self {
            handle,
            thread: Some(thread),
        })
    }
}

impl Drop for SignalForwarder {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn parse_workspace(value: &Value) -> Result<Workspace> {
    let id = string_field(value, &["workspace_id", "id"])
        .ok_or_else(|| Error::User("Herdr workspace is missing workspace_id".to_owned()))?;
    let label = string_field(value, &["label", "name", "title"]).unwrap_or_else(|| id.clone());
    let number = value.get("number").and_then(Value::as_u64);
    let focused = value
        .get("focused")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let active_tab_id = string_field(value, &["active_tab_id"]);
    let checkout_path = value
        .pointer("/worktree/checkout_path")
        .and_then(Value::as_str)
        .or_else(|| value.get("checkout_path").and_then(Value::as_str))
        .map(PathBuf::from);
    let cwd = ["/focused_pane/cwd", "/pane/cwd", "/workspace/cwd", "/cwd"]
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .map(PathBuf::from);
    Ok(Workspace {
        id,
        label,
        number,
        focused,
        active_tab_id,
        checkout_path,
        cwd,
    })
}

fn parse_agent(value: &Value) -> Result<AgentInfo> {
    let pane_id = string_field(value, &["pane_id"])
        .ok_or_else(|| Error::User("Herdr agent is missing pane_id".to_owned()))?;
    let workspace_id = string_field(value, &["workspace_id"]).unwrap_or_else(|| {
        pane_id
            .split_once(':')
            .map_or_else(|| pane_id.clone(), |(workspace, _)| workspace.to_owned())
    });
    Ok(AgentInfo {
        kind: string_field(value, &["agent"]).unwrap_or_default(),
        name: string_field(value, &["name"]).filter(|name| !name.is_empty()),
        status: string_field(value, &["agent_status"]).unwrap_or_else(|| "unknown".to_owned()),
        pane_id,
        workspace_id,
        title: string_field(value, &["terminal_title_stripped", "terminal_title"])
            .filter(|title| !title.is_empty()),
        raw_title: string_field(value, &["terminal_title"]).filter(|title| !title.is_empty()),
    })
}

fn root_pane_id(value: &Value) -> Option<String> {
    value
        .pointer("/result/root_pane/pane_id")
        .or_else(|| value.pointer("/root_pane/pane_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn string_field(value: &Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| value.get(name).and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn find_array<'a>(value: &'a Value, name: &str) -> Option<&'a Vec<Value>> {
    if let Some(array) = value.as_array() {
        return Some(array);
    }
    if let Some(array) = value.get(name).and_then(Value::as_array) {
        return Some(array);
    }
    value
        .get("result")
        .and_then(|result| result.get(name))
        .and_then(Value::as_array)
}
