use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::herdr::{AgentInfo, Herdr, ManagedChild, SignalForwarder, Workspace};
use crate::state::{
    BindingStore, OperationGuard, Paths, ThreadLeaseGuard, ThreadLeaseSet, canonical_git_root,
};

const DEFAULT_POLL_MS: u64 = 2_000;
const SETTLED_STATES: [&str; 3] = ["idle", "done", "blocked"];

/// The one Herdr session this thread talks to, resolved once up front.
struct Session<'a> {
    herdr: &'a Herdr,
    leases: &'a ThreadLeaseSet,
    name: &'a str,
    socket: &'a Path,
}

/// Best-effort attach for the auto-mode `terminal_init_command`. While the mode is on,
/// a missing workspace is created rather than reported, and any remaining failure
/// (outside a Git checkout, Herdr unavailable) leaves the thread usable as a plain
/// local shell instead of surfacing a fatal error.
pub fn run_auto(session_name: &str) -> Result<()> {
    let paths = Paths::discover()?;
    if !crate::setup::thread_auto_enabled(&paths) {
        return Ok(());
    }
    if let Err(error) = run(session_name, None, None, true) {
        let message = error.to_string().replace('\n', " ");
        eprintln!("zerdr: {message}; starting a plain shell");
    }
    Ok(())
}

pub fn run(
    session_name: &str,
    target: Option<&str>,
    kind: Option<&str>,
    create: bool,
) -> Result<()> {
    let herdr = Herdr::from_env();
    let paths = Paths::discover()?;
    let socket = herdr.session_socket_for(session_name)?;
    let leases = ThreadLeaseSet::new(paths.thread_leases_dir.clone());

    let session = Session {
        herdr: &herdr,
        leases: &leases,
        name: session_name,
        socket: &socket,
    };

    let (agent, _lease, terminal) = match target {
        Some(target) => {
            let agent = herdr
                .agent_get_for(session_name, target)?
                .ok_or_else(|| Error::User(format!("no Herdr agent matches {target:?}")))?;
            let lease = leases.acquire(session_name, &socket, &agent.pane_id)?;
            (agent, lease, None)
        }
        None => resolve_or_create(&session, &paths, kind, create)?,
    };

    // Without the workspace list there is no way to tell whether this workspace is
    // already focused, and focusing it blindly would re-fire `workspace.focused` and pull
    // Zed forward under follow mode. Skipping the focus is the harmless direction.
    let workspaces = match herdr.workspaces_for(session_name) {
        Ok(workspaces) => {
            focus_workspace(&herdr, session_name, &agent.workspace_id, &workspaces);
            workspaces
        }
        Err(error) => {
            eprintln!("zerdr: could not read Herdr workspaces, leaving focus alone: {error}");
            Vec::new()
        }
    };
    let label = workspaces
        .iter()
        .find(|workspace| workspace.id == agent.workspace_id)
        .map(|workspace| workspace.label.clone());

    // A fresh pane holds only a shell, which `agent attach` refuses, so it is reached
    // through its terminal instead.
    let mut child = ManagedChild::new(match terminal.as_deref() {
        Some(terminal_id) => herdr.spawn_terminal_attach_for(session_name, terminal_id)?,
        None => herdr.spawn_agent_attach_for(session_name, &agent.pane_id)?,
    });
    let _signals = SignalForwarder::new(child.id())?;
    let monitor = Monitor::start(
        herdr.clone(),
        session_name.to_owned(),
        agent.pane_id.clone(),
        label,
        agent,
    );

    let status = child
        .wait()
        .map_err(|error| Error::User(format!("failed to wait for the Herdr agent: {error}")))?;
    monitor.stop();
    if status.success() {
        Ok(())
    } else {
        Err(Error::ChildExit(status.code().unwrap_or(1)))
    }
}

/// Resolve the pane for a bare invocation, creating a tab or workspace when needed.
/// The whole sequence runs under one lock per session socket so two threads racing on an
/// empty workspace cannot claim the same pane.
fn resolve_or_create(
    session: &Session<'_>,
    paths: &Paths,
    kind: Option<&str>,
    create: bool,
) -> Result<(AgentInfo, ThreadLeaseGuard, Option<String>)> {
    let cwd = std::env::current_dir()
        .map_err(|error| Error::User(format!("failed to read the current directory: {error}")))?;
    let root = canonical_git_root(&cwd).map_err(|error| {
        Error::User(format!(
            "{error}; run `zerdr thread` from a Git checkout or pass a Herdr pane id"
        ))
    })?;

    let _serialize = OperationGuard::acquire(
        &session
            .leases
            .resolve_lock_path(session.name, session.socket)?,
    )?;
    let bindings = BindingStore::new(paths.bindings_file.clone());
    let workspaces = session.herdr.workspaces_for(session.name)?;
    let agents = session.herdr.agents_for(session.name)?;
    let workspace_id = match match_workspace(session, &bindings, &workspaces, &root)? {
        Some(workspace_id) => workspace_id,
        None => {
            if !create {
                return Err(Error::User(format!(
                    "no Herdr workspace matches {}; bind an existing workspace with `zerdr bind` or run `zerdr thread --create` to make one",
                    root.display()
                )));
            }
            let label = root.file_name().map_or_else(
                || root.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            let created = session
                .herdr
                .workspace_create_for(session.name, &root, &label)?;
            bindings.bind_if_absent(session.name, &created.workspace_id, &root)?;
            return start_and_lease(
                session,
                &created.workspace_id,
                &created.root_pane_id,
                kind,
                &agents,
            );
        }
    };

    let leased = session.leases.leased_panes(session.name, session.socket)?;
    let free = agents
        .iter()
        .find(|agent| agent.workspace_id == workspace_id && !leased.contains(&agent.pane_id));
    if let Some(agent) = free {
        let lease = session
            .leases
            .acquire(session.name, session.socket, &agent.pane_id)?;
        return Ok((agent.clone(), lease, None));
    }

    let pane_id = session
        .herdr
        .tab_create_for(session.name, &workspace_id, &root)?;
    start_and_lease(session, &workspace_id, &pane_id, kind, &agents)
}

/// A fresh pane starts as a plain shell, matching a new Herdr tab; an agent is started
/// in it only when a kind was explicitly requested via `--kind` or `ZERDR_THREAD_KIND`.
fn start_and_lease(
    session: &Session<'_>,
    workspace_id: &str,
    pane_id: &str,
    kind: Option<&str>,
    live_agents: &[AgentInfo],
) -> Result<(AgentInfo, ThreadLeaseGuard, Option<String>)> {
    let Some(kind) = kind
        .map(ToOwned::to_owned)
        .or_else(|| std::env::var("ZERDR_THREAD_KIND").ok())
    else {
        let terminal_id = session.herdr.pane_terminal_for(session.name, pane_id)?;
        let lease = session
            .leases
            .acquire(session.name, session.socket, pane_id)?;
        let shell = AgentInfo {
            kind: String::new(),
            name: None,
            status: "unknown".to_owned(),
            pane_id: pane_id.to_owned(),
            workspace_id: workspace_id.to_owned(),
            title: None,
        };
        return Ok((shell, lease, Some(terminal_id)));
    };
    let name = generate_agent_name(live_agents);
    session
        .herdr
        .agent_start_for(session.name, &name, &kind, pane_id)?;
    let lease = session
        .leases
        .acquire(session.name, session.socket, pane_id)?;
    // The agent is running and its pane is known, so a lookup that has not caught up yet
    // must not fail the attach. The monitor picks up the real title on its first poll.
    let agent = session
        .herdr
        .agent_get_for(session.name, pane_id)
        .ok()
        .flatten()
        .unwrap_or_else(|| AgentInfo {
            kind,
            name: Some(name.clone()),
            status: "unknown".to_owned(),
            pane_id: pane_id.to_owned(),
            workspace_id: workspace_id.to_owned(),
            title: None,
        });
    Ok((agent, lease, None))
}

/// Explicit bindings win, then Herdr's own checkout metadata, then where the
/// workspace's panes actually sit. Herdr records `worktree.checkout_path` only when it
/// detected the checkout at creation time, so most hand-made workspaces are matched by
/// the cwd pass; a match found that way is recorded as a binding so the next resolution
/// is direct.
fn match_workspace(
    session: &Session<'_>,
    bindings: &BindingStore,
    workspaces: &[Workspace],
    root: &Path,
) -> Result<Option<String>> {
    for workspace in workspaces {
        if bindings
            .get(session.name, &workspace.id)?
            .is_some_and(|bound| bound == root)
        {
            return Ok(Some(workspace.id.clone()));
        }
    }
    if let Some(workspace) = workspaces.iter().find(|workspace| {
        workspace
            .checkout_path
            .as_deref()
            .and_then(|checkout| checkout.canonicalize().ok())
            .is_some_and(|checkout| checkout == root)
    }) {
        return Ok(Some(workspace.id.clone()));
    }
    for workspace in workspaces {
        // A workspace already pinned elsewhere, or carrying checkout metadata that did
        // not match above, must not be claimed just because a pane wandered into the
        // project directory.
        if workspace.checkout_path.is_some() || bindings.get(session.name, &workspace.id)?.is_some()
        {
            continue;
        }
        let Some(cwd) = session.herdr.workspace_cwd(session.name, workspace)? else {
            continue;
        };
        let Ok(candidate) = canonical_git_root(&cwd) else {
            continue;
        };
        if candidate == root {
            bindings.bind_if_absent(session.name, &workspace.id, root)?;
            return Ok(Some(workspace.id.clone()));
        }
    }
    Ok(None)
}

/// Lowest `zed-<n>` that no live agent already answers to. Herdr requires live agent
/// names to be unique and to match `[a-z][a-z0-9_-]{0,31}`.
fn generate_agent_name(agents: &[AgentInfo]) -> String {
    let taken = agents
        .iter()
        .filter_map(|agent| agent.name.as_deref())
        .collect::<Vec<_>>();
    (1..)
        .map(|index| format!("zed-{index}"))
        .find(|candidate| !taken.contains(&candidate.as_str()))
        .expect("the candidate range is unbounded")
}

/// Focusing an already-focused workspace would re-fire Herdr's `workspace.focused` event
/// and, with follow mode running, pull Zed forward on every thread start.
fn focus_workspace(
    herdr: &Herdr,
    session_name: &str,
    workspace_id: &str,
    workspaces: &[Workspace],
) {
    if workspaces
        .iter()
        .any(|workspace| workspace.focused && workspace.id == workspace_id)
    {
        return;
    }
    if let Err(error) = herdr.focus_workspace_for(session_name, workspace_id) {
        eprintln!("zerdr: could not focus Herdr workspace {workspace_id}: {error}");
    }
}

/// Mirrors the attached agent into the Zed threads sidebar: an OSC 0 title whenever the
/// label changes, and a bell when the agent settles after working so Zed notifies.
struct Monitor {
    stop: Arc<(Mutex<bool>, Condvar)>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Monitor {
    fn start(
        herdr: Herdr,
        session_name: String,
        pane_id: String,
        fallback_label: Option<String>,
        initial: AgentInfo,
    ) -> Self {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let interval = Duration::from_millis(
            std::env::var("ZERDR_THREAD_POLL_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(DEFAULT_POLL_MS),
        );
        let worker_stop = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let mut last_label = None;
            let mut last_status = initial.status.clone();
            emit_title(&mut last_label, &initial, fallback_label.as_deref());
            // Waiting on the condvar rather than sleeping keeps detaching immediate: the
            // agent exits, `stop` wakes this thread, and zerdr does not linger for a
            // whole poll interval before returning the terminal to Zed.
            while !wait_for_stop(&worker_stop, interval) {
                // A failed poll must never disturb the attached agent, so keep the last
                // known state and try again on the next tick.
                let Ok(Some(agent)) = herdr.agent_get_for(&session_name, &pane_id) else {
                    continue;
                };
                emit_title(&mut last_label, &agent, fallback_label.as_deref());
                if last_status == "working" && SETTLED_STATES.contains(&agent.status.as_str()) {
                    emit(b"\x07");
                }
                last_status = agent.status;
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }

    fn stop(mut self) {
        let (stopped, waker) = &*self.stop;
        if let Ok(mut stopped) = stopped.lock() {
            *stopped = true;
        }
        waker.notify_all();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Waits up to `interval` for the stop signal. Returns whether the monitor should exit.
fn wait_for_stop(stop: &(Mutex<bool>, Condvar), interval: Duration) -> bool {
    let (stopped, waker) = stop;
    let Ok(guard) = stopped.lock() else {
        return true;
    };
    if *guard {
        return true;
    }
    match waker.wait_timeout(guard, interval) {
        Ok((guard, _)) => *guard,
        Err(_) => true,
    }
}

/// The title is forwarded verbatim: Zed's agent panel renders the raw OSC title (and
/// promotes a leading decorative glyph to the row icon), so any zerdr-added prefix
/// would diverge from how a natively-run agent looks. An empty title intentionally
/// falls back to the workspace label so a plain-shell tab still says where it lives.
fn emit_title(last: &mut Option<String>, agent: &AgentInfo, fallback: Option<&str>) {
    let label = agent
        .title
        .as_deref()
        .or(fallback)
        .unwrap_or(&agent.workspace_id)
        .to_owned();
    if last.as_deref() == Some(label.as_str()) {
        return;
    }
    emit(format!("\x1b]0;{label}\x07").as_bytes());
    *last = Some(label);
}

fn emit(bytes: &[u8]) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}
