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
    LeaseSet, LifecycleGuard, Paths, RouteStore, RouteStrategy, SESSION_NAME, SyncGuard,
};
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
pub struct Herdr {
    program: OsString,
}

impl Herdr {
    pub fn from_env() -> Self {
        Self {
            program: std::env::var_os("ZERDR_HERDR_BIN").unwrap_or_else(|| "herdr".into()),
        }
    }

    pub fn session_socket(&self) -> Result<PathBuf> {
        let value = self.json_output(["session", "list", "--json"])?;
        let sessions = find_array(&value, "sessions")
            .ok_or_else(|| Error::User("Herdr session list did not contain sessions".to_owned()))?;
        for session in sessions {
            let name = string_field(session, &["name", "session_name"]);
            if name.as_deref() == Some(SESSION_NAME)
                && session
                    .get("running")
                    .and_then(Value::as_bool)
                    .unwrap_or(true)
            {
                let socket =
                    string_field(session, &["socket_path", "socket"]).ok_or_else(|| {
                        Error::User("zerdr Herdr session has no socket path".to_owned())
                    })?;
                return Ok(PathBuf::from(socket));
            }
        }
        Err(Error::SessionUnavailable)
    }

    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        let value = self.session_json_output(["workspace", "list"])?;
        let values = find_array(&value, "workspaces").ok_or_else(|| {
            Error::User("Herdr workspace list did not contain workspaces".to_owned())
        })?;
        values.iter().map(parse_workspace).collect()
    }

    pub fn focus_workspace(&self, workspace_id: &str) -> Result<()> {
        self.session_output([
            OsStr::new("workspace"),
            OsStr::new("focus"),
            OsStr::new(workspace_id),
        ])?;
        Ok(())
    }

    pub fn notify_error(&self, message: &str) -> Result<bool> {
        let output = self.session_output([
            OsStr::new("notification"),
            OsStr::new("show"),
            OsStr::new("zerdr sync failed"),
            OsStr::new("--body"),
            OsStr::new(message),
            OsStr::new("--position"),
            OsStr::new("top-right"),
            OsStr::new("--sound"),
            OsStr::new("request"),
        ])?;
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
        self.output(["plugin", "uninstall", SESSION_NAME])?;
        Ok(())
    }

    pub fn plugin_list(&self) -> Result<Value> {
        self.json_output(["plugin", "list", "--plugin", SESSION_NAME, "--json"])
    }

    pub fn version(&self) -> Result<String> {
        let output = self.output(["--version"])?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    pub fn spawn_client(&self) -> Result<Child> {
        Command::new(&self.program)
            .args(["--session", SESSION_NAME])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|error| Error::User(format!("failed to launch Herdr: {error}")))
    }

    pub fn program(&self) -> &OsStr {
        &self.program
    }

    pub fn workspace_cwd(&self, workspace: &Workspace) -> Result<Option<PathBuf>> {
        if let Some(cwd) = workspace.cwd.as_ref() {
            return Ok(Some(cwd.clone()));
        }
        let Some(tab_id) = workspace.active_tab_id.as_deref() else {
            return Ok(None);
        };
        let value = self.session_json_output(["api", "snapshot"])?;
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

    fn session_json_output<I, S>(&self, args: I) -> Result<Value>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut complete = vec![OsString::from("--session"), OsString::from(SESSION_NAME)];
        complete.extend(args.into_iter().map(|value| value.as_ref().to_os_string()));
        self.json_output(complete)
    }

    fn session_output<I, S>(&self, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut complete = vec![OsString::from("--session"), OsString::from(SESSION_NAME)];
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

pub fn run_wrapper(routing: RouteStrategy) -> Result<()> {
    let herdr = Herdr::from_env();
    let paths = Paths::discover()?;
    crate::setup::validate_launcher_installation(&paths, &herdr)?;
    let mut child = ManagedChild::new(herdr.spawn_client()?);
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
        if let Ok(socket) = herdr.session_socket() {
            break socket;
        }
        if Instant::now() >= deadline {
            child.terminate();
            return Err(Error::User(format!(
                "timed out waiting for the {SESSION_NAME} Herdr session socket"
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
    if leases.has_live(&socket)? {
        return Err(Error::User(format!(
            "the {SESSION_NAME} Herdr session already has a live wrapper"
        )));
    }
    RouteStore::new(paths.routes_dir.clone()).initialize_strategy(
        &socket,
        routing.clone(),
        std::process::id(),
    )?;
    let _lease = leases.acquire(&socket, child.id())?;
    drop(admission);
    drop(lifecycle);
    let synchronizer = Synchronizer::from_env()?;
    if let Err(error) = synchronizer.sync_socket(&socket) {
        let message = format!("startup synchronization failed: {error}");
        let _ = herdr.notify_error(&message);
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

struct ManagedChild {
    child: Child,
    running: bool,
}

impl ManagedChild {
    fn new(child: Child) -> Self {
        Self {
            child,
            running: true,
        }
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.running = false;
        }
        Ok(status)
    }

    fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        let status = self.child.wait()?;
        self.running = false;
        Ok(status)
    }

    fn terminate(&mut self) {
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

struct SignalForwarder {
    handle: SignalHandle,
    thread: Option<thread::JoinHandle<()>>,
}

impl SignalForwarder {
    fn new(child_pid: u32) -> Result<Self> {
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
