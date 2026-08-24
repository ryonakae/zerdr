use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::herdr::{AgentInfo, Herdr, ManagedChild, SignalForwarder, Workspace};
use crate::state::{
    BindingStore, DEFAULT_SESSION_NAME, OperationGuard, Paths, ThreadLeaseGuard, ThreadLeaseSet,
    ThreadPaneMemory, canonical_git_root, linked_worktree_parent,
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

/// How the pane this thread ended up on was obtained, for the status line.
enum Attachment {
    Agent,
    Remembered,
    NewTab,
    NewWorkspace { label: String },
}

/// Best-effort attach for the auto-mode `terminal_init_command`. While the mode is on,
/// a missing workspace is created rather than reported, and any remaining failure
/// (outside a Git checkout, Herdr unavailable) leaves the thread usable as a plain
/// local shell instead of surfacing a fatal error.
pub fn run_auto(session_name: &str) -> Result<()> {
    let paths = Paths::discover()?;
    if !crate::setup::thread_auto_enabled(&paths) {
        // The init command keeps firing after `setup auto disable`, and a silent exit
        // there is indistinguishable from a bug, so the no-op explains itself.
        println!(
            "zerdr: auto mode is disabled; run `zerdr connect` to attach this thread to a Herdr pane, or run `zerdr setup auto enable` to attach new threads automatically"
        );
        return Ok(());
    }
    let _ = run_with_mode(session_name, None, None, true, true);
    Ok(())
}

pub fn run(
    session_name: &str,
    target: Option<&str>,
    kind: Option<&str>,
    create: bool,
) -> Result<()> {
    run_with_mode(session_name, target, kind, create, false)
}

fn run_with_mode(
    session_name: &str,
    target: Option<&str>,
    kind: Option<&str>,
    create: bool,
    auto: bool,
) -> Result<()> {
    let herdr = Herdr::from_env();
    let paths = Paths::discover()?;
    let leases = ThreadLeaseSet::new(paths.thread_leases_dir.clone());
    let (socket, started_session) =
        resolve_session_socket(&herdr, &leases, session_name, create, auto)?;
    if started_session {
        println!("zerdr: started Herdr session {session_name}");
    }

    let session = Session {
        herdr: &herdr,
        leases: &leases,
        name: session_name,
        socket: &socket,
    };
    let memory = ThreadPaneMemory::new(paths.thread_memory_dir.clone());

    let (agent, _lease, terminal, attachment) = match target {
        Some(target) => {
            let agent = herdr
                .agent_get_for(session_name, target)?
                .ok_or_else(|| Error::User(format!("no Herdr agent matches {target:?}")))?;
            let lease = leases.acquire(session_name, &socket, &agent.pane_id)?;
            {
                let _serialize =
                    OperationGuard::acquire(&leases.resolve_lock_path(session_name, &socket)?)?;
                remember_pane(&memory, &session, &agent.workspace_id, &agent.pane_id);
            }
            (agent, lease, None, Attachment::Agent)
        }
        None => resolve_or_create(&session, &memory, &paths, kind, create)?,
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
    print_status(auto, &attachment, &agent, label.as_deref());

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

/// Resolves the session socket, starting a headless server for a not-running
/// named session when `--create` allows it. The default session is only ever
/// started by `zerdr start` (which sets up routing and sync), and the auto
/// path never spawns servers: a best-effort init command must not create
/// background processes. Returns whether this call started the session.
fn resolve_session_socket(
    herdr: &Herdr,
    leases: &ThreadLeaseSet,
    session_name: &str,
    create: bool,
    auto: bool,
) -> Result<(PathBuf, bool)> {
    if let Some(socket) = herdr.session_socket_if_running(session_name)? {
        return Ok((socket, false));
    }
    if session_name == DEFAULT_SESSION_NAME {
        return Err(Error::User(
            "the default Herdr session is not running; launch it with `zerdr start`".to_owned(),
        ));
    }
    if !create || auto {
        return Err(Error::User(format!(
            "Herdr session {session_name:?} is not running; run `zerdr connect --create --session {session_name}` to start it"
        )));
    }

    // Two connects racing on the same missing session must not spawn two
    // servers, so the start sequence is serialized per session name and the
    // session list is re-checked under the lock.
    let _serialize = OperationGuard::acquire(&leases.session_start_lock_path(session_name))?;
    if let Some(socket) = herdr.session_socket_if_running(session_name)? {
        return Ok((socket, false));
    }
    let mut server = herdr.spawn_server_detached_for(session_name)?;
    let timeout_ms = std::env::var("ZERDR_READY_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(5_000);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if let Some(socket) = herdr.session_socket_if_running(session_name)? {
            return Ok((socket, true));
        }
        match server.try_wait() {
            Ok(Some(status)) => {
                return Err(Error::User(format!(
                    "the Herdr server for session {session_name:?} exited with status {} before its socket appeared",
                    status.code().unwrap_or(1)
                )));
            }
            Ok(None) => {}
            Err(error) => {
                return Err(Error::User(format!(
                    "failed to wait for the Herdr session server: {error}"
                )));
            }
        }
        if Instant::now() >= deadline {
            let _ = server.kill();
            let _ = server.wait();
            return Err(Error::User(format!(
                "timed out waiting for the {session_name} Herdr session socket"
            )));
        }
        thread::sleep(Duration::from_millis(25));
    }
}

/// Resolve the pane for a bare invocation, creating a tab or workspace when needed.
/// The whole sequence runs under one lock per session socket so two threads racing on an
/// empty workspace cannot claim the same pane.
fn resolve_or_create(
    session: &Session<'_>,
    memory: &ThreadPaneMemory,
    paths: &Paths,
    kind: Option<&str>,
    create: bool,
) -> Result<(AgentInfo, ThreadLeaseGuard, Option<String>, Attachment)> {
    let cwd = std::env::current_dir()
        .map_err(|error| Error::User(format!("failed to read the current directory: {error}")))?;
    let root = canonical_git_root(&cwd).map_err(|error| {
        Error::User(format!(
            "{error}; run `zerdr connect` from a Git checkout or pass a Herdr pane id"
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
                // In a linked worktree the refusal names what `--create` does there, so
                // the user knows registration (not a plain workspace) is one flag away.
                let message = if linked_worktree_parent(&root).is_ok_and(|parent| parent.is_some())
                {
                    format!(
                        "no Herdr workspace matches {}; run `zerdr connect --create` to open this Git worktree as a Herdr workspace, or bind one with `zerdr workspace bind`",
                        root.display()
                    )
                } else {
                    format!(
                        "no Herdr workspace matches {}; bind an existing workspace with `zerdr workspace bind` or run `zerdr connect --create` to make one",
                        root.display()
                    )
                };
                return Err(Error::User(message));
            }
            let label = root.file_name().map_or_else(
                || root.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            // A linked worktree becomes a worktree-backed workspace so `herdr worktree
            // list`/`remove` manage it like one Herdr created itself; on failure nothing
            // is created — a plain-workspace fallback would hide the very inconsistency
            // this registration removes.
            let created = if let Some(parent) = linked_worktree_parent(&root)? {
                session
                    .herdr
                    .worktree_open_for(session.name, &parent, &root)?
            } else {
                session
                    .herdr
                    .workspace_create_for(session.name, &root, &label)?
            };
            let label = created.label.clone().unwrap_or(label);
            bindings.bind_if_absent(session.name, &created.workspace_id, &root)?;
            let (agent, lease, terminal) = start_and_lease(
                session,
                &created.workspace_id,
                &created.root_pane_id,
                kind,
                &agents,
            )?;
            remember_pane(memory, session, &created.workspace_id, &agent.pane_id);
            return Ok((agent, lease, terminal, Attachment::NewWorkspace { label }));
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
        remember_pane(memory, session, &workspace_id, &agent.pane_id);
        return Ok((agent.clone(), lease, None, Attachment::Agent));
    }

    // A remembered shell pane (typically left behind by a thread a Zed restart closed)
    // is reattached before a new tab is created. Agent panes are excluded: the pass
    // above owns them. Dead records are pruned as they are encountered.
    let mut dead = Vec::new();
    let mut reattach = None;
    for record in memory.load(session.name, session.socket) {
        if record.workspace_id != workspace_id
            || leased.contains(&record.pane_id)
            || agents.iter().any(|agent| agent.pane_id == record.pane_id)
        {
            continue;
        }
        match session
            .herdr
            .pane_terminal_for(session.name, &record.pane_id)
        {
            Ok(terminal_id) => {
                reattach = Some((record.pane_id, terminal_id));
                break;
            }
            Err(_) => dead.push(record.pane_id),
        }
    }
    if !dead.is_empty()
        && let Err(error) = memory.prune(session.name, session.socket, &dead)
    {
        eprintln!("zerdr: could not prune dead thread panes: {error}");
    }
    if let Some((pane_id, terminal_id)) = reattach {
        let lease = session
            .leases
            .acquire(session.name, session.socket, &pane_id)?;
        remember_pane(memory, session, &workspace_id, &pane_id);
        let shell = AgentInfo {
            kind: String::new(),
            name: None,
            status: "unknown".to_owned(),
            pane_id,
            workspace_id: workspace_id.clone(),
            title: None,
            raw_title: None,
        };
        return Ok((shell, lease, Some(terminal_id), Attachment::Remembered));
    }

    let pane_id = session
        .herdr
        .tab_create_for(session.name, &workspace_id, &root)?;
    let (agent, lease, terminal) =
        start_and_lease(session, &workspace_id, &pane_id, kind, &agents)?;
    remember_pane(memory, session, &workspace_id, &agent.pane_id);
    Ok((agent, lease, terminal, Attachment::NewTab))
}

/// Recording is advisory (R4): a store hiccup must not fail an otherwise good attach.
fn remember_pane(
    memory: &ThreadPaneMemory,
    session: &Session<'_>,
    workspace_id: &str,
    pane_id: &str,
) {
    if let Err(error) = memory.record(session.name, session.socket, workspace_id, pane_id) {
        eprintln!("zerdr: could not record the thread pane: {error}");
    }
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
            raw_title: None,
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
            raw_title: None,
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

/// One line telling the user what this thread is now connected to, so a plain local
/// shell and a Herdr pane are distinguishable at a glance. On the auto path the line
/// also names the mode so `terminal_init_command` runs are self-explanatory.
fn print_status(auto: bool, attachment: &Attachment, agent: &AgentInfo, label: Option<&str>) {
    let workspace = label.unwrap_or(&agent.workspace_id);
    let outcome = match attachment {
        Attachment::Agent => {
            let subject = if agent.kind.is_empty() {
                "agent".to_owned()
            } else {
                agent.kind.clone()
            };
            format!(
                "attached to {subject} (pane {}) in Herdr workspace {workspace}",
                agent.pane_id
            )
        }
        Attachment::Remembered => format!(
            "reattached to Herdr pane {} in workspace {workspace}",
            agent.pane_id
        ),
        Attachment::NewTab => format!(
            "opened a new Herdr tab (pane {}) in workspace {workspace}",
            agent.pane_id
        ),
        Attachment::NewWorkspace { label } => {
            format!("created Herdr workspace {label} (pane {})", agent.pane_id)
        }
    };
    if auto {
        println!("zerdr: auto mode is enabled; {outcome}");
    } else {
        println!("zerdr: {outcome}");
    }
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

/// The sidebar title is what tells a Herdr pane apart from a plain Zed shell, so it
/// carries a `[herdr]` marker followed by a friendly agent name and the agent's own
/// (live) title. An empty title falls back to the workspace label so a plain-shell
/// tab still says where it lives. Agent titles lead with a status glyph, which Zed
/// promotes into the thread's sidebar row icon.
fn emit_title(last: &mut Option<String>, agent: &AgentInfo, fallback: Option<&str>) {
    let label = if agent.kind.is_empty() {
        format!("[herdr] {}", fallback.unwrap_or(&agent.workspace_id))
    } else {
        let glyph = agent
            .raw_title
            .as_deref()
            .and_then(title_glyph_prefix)
            .unwrap_or_else(|| status_glyph(&agent.status));
        let detail = agent
            .title
            .as_deref()
            .map(|title| strip_kind_prefix(&agent.kind, title))
            .filter(|detail| !detail.is_empty())
            .or(fallback)
            .unwrap_or(&agent.workspace_id);
        format!("{glyph} [herdr] {} - {detail}", display_kind(&agent.kind))
    };
    if last.as_deref() == Some(label.as_str()) {
        return;
    }
    emit(format!("\x1b]0;{label}\x07").as_bytes());
    *last = Some(label);
}

/// Mirrors Zed's `terminal_title_prefix`: a leading run of non-whitespace,
/// non-alphanumeric characters followed by whitespace and a non-empty remainder.
/// This is how agents like Claude Code animate a spinner in their own titles, so a
/// matching run is passed through verbatim. Control characters disqualify the run:
/// it is re-emitted inside zerdr's own OSC 0 sequence, which a stray ESC or BEL
/// would corrupt.
fn title_glyph_prefix(title: &str) -> Option<&str> {
    let mut prefix_end = 0;
    let mut rest = None;
    for (index, character) in title.char_indices() {
        if character.is_whitespace() {
            if prefix_end == 0 {
                return None;
            }
            rest = Some(&title[index..]);
            break;
        }
        if character.is_alphanumeric() || character.is_control() {
            return None;
        }
        prefix_end = index + character.len_utf8();
    }
    if rest?.trim_start().is_empty() {
        return None;
    }
    Some(&title[..prefix_end])
}

/// Herdr's Symbols-style indicator set (its `state_icon_symbol`), fixed rather than
/// following the configured style because Zed renders title glyphs without the color
/// that distinguishes the Dots style.
fn status_glyph(status: &str) -> &'static str {
    match status {
        "working" => "◐",
        "blocked" => "×",
        "done" => "✓",
        "idle" => "○",
        _ => "·",
    }
}

fn display_kind(kind: &str) -> String {
    match kind {
        "pi" => "Pi".to_owned(),
        "claude" => "Claude".to_owned(),
        other => {
            let mut characters = other.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
                None => String::new(),
            }
        }
    }
}

/// Pi prefixes its own terminal titles with `π - `, which would double up with the
/// friendly name in the marker.
fn strip_kind_prefix<'a>(kind: &str, title: &'a str) -> &'a str {
    if kind == "pi" {
        title.strip_prefix("\u{3c0} - ").unwrap_or(title)
    } else {
        title
    }
}

fn emit(bytes: &[u8]) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}
