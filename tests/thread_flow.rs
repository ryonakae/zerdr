mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::Duration;

use predicates::prelude::*;
use support::TestEnv;
use zerdr::state::{BindingStore, Paths};

const OSC_PREFIX: &str = "\u{1b}]0;";

struct Fixture {
    env: TestEnv,
    socket: PathBuf,
    repo: PathBuf,
    agents_dir: PathBuf,
    release: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let env = TestEnv::new();
        let socket = env.root.path().join("herdr.sock");
        fs::write(&socket, "").unwrap();
        let repo = env.root.path().join("checkout");
        fs::create_dir_all(&repo).unwrap();
        assert!(
            ProcessCommand::new("git")
                .args(["init", "--quiet"])
                .current_dir(&repo)
                .status()
                .unwrap()
                .success()
        );
        let repo = repo.canonicalize().unwrap();
        let agents_dir = env.root.path().join("agents");
        fs::create_dir_all(&agents_dir).unwrap();
        let release = env.root.path().join("attach-release");
        Self {
            env,
            socket,
            repo,
            agents_dir,
            release,
        }
    }

    fn sessions(&self) -> String {
        serde_json::json!({
            "sessions": [{"name": "default", "running": true, "socket_path": self.socket}]
        })
        .to_string()
    }

    /// One workspace bound to the fixture checkout via `worktree.checkout_path`.
    fn workspaces(&self, focused: bool) -> String {
        serde_json::json!({
            "result": {"workspaces": [{
                "workspace_id": "w1",
                "label": "checkout",
                "focused": focused,
                "worktree": {"checkout_path": self.repo}
            }]}
        })
        .to_string()
    }

    fn agent(&self, name: &str, pane_id: &str, workspace_id: &str, status: &str, title: &str) {
        fs::write(
            self.agents_dir.join(format!("{name}.json")),
            serde_json::json!({
                "agent": "pi",
                "name": name,
                "agent_status": status,
                "pane_id": pane_id,
                "workspace_id": workspace_id,
                "terminal_title_stripped": title
            })
            .to_string(),
        )
        .unwrap();
    }

    fn thread_command(&self) -> assert_cmd::Command {
        let mut command = self.env.command();
        command
            .current_dir(&self.repo)
            .env("ZERDR_TEST_SESSIONS_JSON", self.sessions())
            .env("ZERDR_TEST_WORKSPACES_JSON", self.workspaces(true))
            .env("ZERDR_TEST_AGENTS_DIR", &self.agents_dir)
            .env("ZERDR_THREAD_POLL_MS", "20")
            .env("ZERDR_TEST_ATTACH_RELEASE_FILE", &self.release);
        command
    }

    fn std_thread_command(&self) -> ProcessCommand {
        let mut command = self.env.std_command();
        command
            .current_dir(&self.repo)
            .env("ZERDR_TEST_SESSIONS_JSON", self.sessions())
            .env("ZERDR_TEST_WORKSPACES_JSON", self.workspaces(true))
            .env("ZERDR_TEST_AGENTS_DIR", &self.agents_dir)
            .env("ZERDR_THREAD_POLL_MS", "20")
            .env("ZERDR_TEST_ATTACH_RELEASE_FILE", &self.release)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn release_attach(&self) {
        fs::write(&self.release, "go").unwrap();
    }

    fn paths(&self) -> Paths {
        Paths::for_test(self.env.root.path())
    }
}

fn wait_for_log(env: &TestEnv, needle: &str) {
    for _ in 0..400 {
        if env.read_log().contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {needle:?} in\n{}", env.read_log());
}

fn count_leases(paths: &Paths) -> usize {
    fs::read_dir(&paths.thread_leases_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .flat_map(|entry| fs::read_dir(entry.path()).into_iter().flatten().flatten())
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("json"))
        .count()
}

fn write_sequence(directory: &Path, responses: &[&str]) {
    fs::create_dir_all(directory).unwrap();
    for (index, response) in responses.iter().enumerate() {
        fs::write(directory.join(format!("{}.json", index + 1)), response).unwrap();
    }
}

fn agent_response(status: &str, title: &str) -> String {
    serde_json::json!({
        "result": {"agent": {
            "agent": "pi",
            "name": "zed-1",
            "agent_status": status,
            "pane_id": "w1:p1",
            "workspace_id": "w1",
            "terminal_title_stripped": title
        }}
    })
    .to_string()
}

#[test]
fn bare_thread_attaches_a_free_agent_in_the_matching_workspace() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "review the diff");
    let paths = fixture.paths();

    let child = fixture.std_thread_command().arg("thread").spawn().unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    assert_eq!(count_leases(&paths), 1);

    fixture.release_attach();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(count_leases(&paths), 0);

    let log = fixture.env.read_log();
    assert!(
        log.contains("herdr\t--session default agent attach w1:p1"),
        "{log}"
    );
    assert!(!log.contains("tab create"), "{log}");
    assert!(!log.contains("agent start"), "{log}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(&format!("{OSC_PREFIX}pi \u{b7} review")),
        "{:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn a_second_thread_picks_a_different_agent_pane() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "first");
    fixture.agent("zed-2", "w1:p2", "w1", "idle", "second");

    let mut first = fixture.std_thread_command().arg("thread").spawn().unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p");
    let mut second = fixture.std_thread_command().arg("thread").spawn().unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p2");

    fixture.release_attach();
    assert!(first.wait().unwrap().success());
    assert!(second.wait().unwrap().success());

    let log = fixture.env.read_log();
    assert!(log.contains("agent attach w1:p1"), "{log}");
    assert!(log.contains("agent attach w1:p2"), "{log}");
}

/// With no free agent the thread gets a fresh tab holding a plain shell — the same
/// starting point as creating a tab in Herdr — and never an auto-started agent.
#[test]
fn an_empty_workspace_gets_a_new_tab_with_a_plain_shell() {
    let fixture = Fixture::new();
    let tab = serde_json::json!({"result": {"root_pane": {"pane_id": "w1:p9"}}});

    let output = fixture
        .std_thread_command()
        .arg("thread")
        .env("ZERDR_TEST_TAB_CREATE_JSON", tab.to_string())
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();

    assert!(output.status.success());
    let log = fixture.env.read_log();
    assert!(
        log.contains("herdr\t--session default tab create --workspace w1 --cwd"),
        "{log}"
    );
    assert!(log.contains("--no-focus"), "{log}");
    assert!(!log.contains("agent start"), "{log}");
    assert!(
        log.contains("herdr\t--session default terminal attach term-w1:p9"),
        "{log}"
    );
    assert!(!log.contains("agent attach"), "{log}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(&format!("{OSC_PREFIX}checkout")),
        "the sidebar shows the workspace label until an agent appears: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// Auto-starting an agent in the fresh tab is opt-in via the flag or the environment.
#[test]
fn auto_start_kind_comes_from_the_flag_then_the_environment() {
    for (flag, environment, expected) in [
        (Some("claude"), None, "claude"),
        (None, Some("codex"), "codex"),
        (Some("claude"), Some("codex"), "claude"),
    ] {
        let fixture = Fixture::new();
        let tab = serde_json::json!({"result": {"root_pane": {"pane_id": "w1:p9"}}});
        let mut command = fixture.thread_command();
        command
            .arg("thread")
            .env("ZERDR_TEST_TAB_CREATE_JSON", tab.to_string())
            .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "");
        if let Some(flag) = flag {
            command.args(["--kind", flag]);
        }
        if let Some(environment) = environment {
            command.env("ZERDR_THREAD_KIND", environment);
        }
        command.assert().success();

        let log = fixture.env.read_log();
        assert!(
            log.contains(&format!("agent start zed-1 --kind {expected} --pane w1:p9")),
            "{log}"
        );
    }
}

#[test]
fn generated_agent_names_skip_live_names() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w2:p1", "w2", "idle", "other workspace");
    let tab = serde_json::json!({"result": {"root_pane": {"pane_id": "w1:p9"}}});

    fixture
        .thread_command()
        .args(["thread", "--kind", "pi"])
        .env("ZERDR_TEST_TAB_CREATE_JSON", tab.to_string())
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .assert()
        .success();

    let log = fixture.env.read_log();
    assert!(log.contains("agent start zed-2 --kind pi"), "{log}");
}

#[test]
fn a_bare_thread_without_a_matching_workspace_explains_the_create_flag() {
    let fixture = Fixture::new();

    fixture
        .thread_command()
        .arg("thread")
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .assert()
        .code(1)
        .stderr(predicate::str::contains("zerdr bind"))
        .stderr(predicate::str::contains("zerdr thread --create"))
        .stderr(predicate::str::contains(fixture.repo.display().to_string()));

    assert!(!fixture.env.read_log().contains("workspace create"));
}

/// With auto mode enabled, `--auto` behaves exactly like a manual `zerdr thread`.
#[test]
fn auto_attaches_a_free_agent_while_the_mode_is_enabled() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "review the diff");
    let paths = fixture.paths();
    fs::create_dir_all(&paths.state_dir).unwrap();
    fs::write(&paths.thread_auto_flag_file, b"").unwrap();

    let child = fixture
        .std_thread_command()
        .args(["thread", "--auto"])
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");

    fixture.release_attach();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}pi \u{b7} review")),
        "{stdout:?}"
    );
    assert!(String::from_utf8_lossy(&output.stderr).is_empty());
}

/// Auto attach is best-effort: an unmatched project leaves the thread as a plain local
/// shell with a single note, never a fatal error or a new Herdr workspace.
#[test]
fn auto_without_a_matching_workspace_leaves_a_plain_shell_with_one_note() {
    let fixture = Fixture::new();
    let paths = fixture.paths();
    fs::create_dir_all(&paths.state_dir).unwrap();
    fs::write(&paths.thread_auto_flag_file, b"").unwrap();

    let output = fixture
        .std_thread_command()
        .args(["thread", "--auto"])
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.starts_with("zerdr: "), "{stderr}");
    assert!(stderr.contains("starting a plain shell"), "{stderr}");
    let log = fixture.env.read_log();
    assert!(!log.contains("workspace create"), "{log}");
    assert!(!log.contains("tab create"), "{log}");
}

/// A missing or failing Herdr must not block the thread's shell either.
#[test]
fn auto_exits_zero_when_herdr_is_unavailable() {
    let fixture = Fixture::new();
    let paths = fixture.paths();
    fs::create_dir_all(&paths.state_dir).unwrap();
    fs::write(&paths.thread_auto_flag_file, b"").unwrap();

    let output = fixture
        .std_thread_command()
        .args(["thread", "--auto"])
        .env(
            "ZERDR_HERDR_BIN",
            fixture.env.root.path().join("missing-herdr"),
        )
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.starts_with("zerdr: "), "{stderr}");
}

/// Herdr only records `worktree.checkout_path` when it detected the checkout at
/// creation time, so most hand-made workspaces lack it. A workspace whose pane sits in
/// the project directory must still match, and the match is remembered as a binding.
#[test]
fn a_workspace_without_checkout_metadata_matches_by_pane_cwd_and_is_bound() {
    let fixture = Fixture::new();
    fixture.agent("zed-9", "w1:p1", "w1", "idle", "cwd match");
    let workspaces = serde_json::json!({
        "result": {"workspaces": [{
            "workspace_id": "w1",
            "label": "checkout",
            "focused": true,
            "cwd": fixture.repo
        }]}
    });

    fixture
        .thread_command()
        .arg("thread")
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .assert()
        .success();

    let log = fixture.env.read_log();
    assert!(log.contains("agent attach w1:p1"), "{log}");
    assert!(!log.contains("workspace create"), "{log}");
    let bound = BindingStore::new(fixture.paths().bindings_file)
        .get("default", "w1")
        .unwrap();
    assert_eq!(bound, Some(fixture.repo.clone()));
}

/// An explicit binding to another checkout wins over where the panes happen to sit.
#[test]
fn a_workspace_bound_elsewhere_is_not_matched_by_cwd() {
    let fixture = Fixture::new();
    let other = fixture.env.root.path().join("other-checkout");
    fs::create_dir_all(&other).unwrap();
    assert!(
        ProcessCommand::new("git")
            .args(["init", "--quiet"])
            .current_dir(&other)
            .status()
            .unwrap()
            .success()
    );
    let other = other.canonicalize().unwrap();
    BindingStore::new(fixture.paths().bindings_file)
        .bind("default", "w1", &other)
        .unwrap();
    let workspaces = serde_json::json!({
        "result": {"workspaces": [{
            "workspace_id": "w1",
            "label": "checkout",
            "focused": true,
            "cwd": fixture.repo
        }]}
    });

    fixture
        .thread_command()
        .arg("thread")
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .code(1)
        .stderr(predicate::str::contains("no Herdr workspace matches"));
}

#[test]
fn create_makes_the_workspace_binds_it_and_starts_an_agent() {
    let fixture = Fixture::new();
    let workspace = serde_json::json!({
        "result": {
            "workspace": {"workspace_id": "w7", "label": "checkout"},
            "root_pane": {"pane_id": "w7:p1"}
        }
    });

    fixture
        .thread_command()
        .args(["thread", "--create"])
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .env("ZERDR_TEST_WORKSPACE_CREATE_JSON", workspace.to_string())
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .assert()
        .success();

    let log = fixture.env.read_log();
    assert!(
        log.contains("herdr\t--session default workspace create --cwd"),
        "{log}"
    );
    assert!(log.contains("--label checkout --no-focus"), "{log}");
    assert!(!log.contains("agent start"), "{log}");
    assert!(log.contains("terminal attach term-w7:p1"), "{log}");

    let bound = BindingStore::new(fixture.paths().bindings_file)
        .get("default", "w7")
        .unwrap();
    assert_eq!(bound, Some(fixture.repo.clone()));
}

/// Two threads racing on an empty workspace must each end up with their own tab and
/// never contend for one pane: serializing resolve-then-lease is what makes the second
/// invocation observe the first pane as leased instead of failing to lease it.
#[test]
fn concurrent_bare_threads_each_get_their_own_tab() {
    let fixture = Fixture::new();
    let counter = fixture.env.root.path().join("pane-counter");

    let mut children = Vec::new();
    for _ in 0..2 {
        children.push(
            fixture
                .std_thread_command()
                .arg("thread")
                .env("ZERDR_TEST_PANE_COUNTER_FILE", &counter)
                .spawn()
                .unwrap(),
        );
    }
    wait_for_log(&fixture.env, "terminal attach term-w1:p1");
    wait_for_log(&fixture.env, "terminal attach term-w1:p2");
    fixture.release_attach();
    for child in children {
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let log = fixture.env.read_log();
    assert_eq!(log.matches("tab create").count(), 2, "{log}");
    assert!(!log.contains("agent start"), "{log}");
    assert_eq!(log.matches("terminal attach").count(), 2, "{log}");
}

#[test]
fn an_explicit_target_attaches_without_creating_anything() {
    let fixture = Fixture::new();
    let agent = agent_response("idle", "explicit target");

    let child = fixture
        .std_thread_command()
        .args(["thread", "w1:p1"])
        .env("ZERDR_TEST_AGENT_GET_JSON", agent)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    fixture.release_attach();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success());
    let log = fixture.env.read_log();
    assert!(log.contains("agent get w1:p1"), "{log}");
    assert!(!log.contains("tab create"), "{log}");
    assert!(!log.contains("agent start"), "{log}");
}

#[test]
fn titles_are_emitted_once_per_change_and_a_bell_marks_settling() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "working", "first title");
    let sequence = fixture.env.root.path().join("agent-get-seq");
    write_sequence(
        &sequence,
        &[
            &agent_response("working", "first title"),
            &agent_response("working", "first title"),
            &agent_response("idle", "second title"),
        ],
    );

    let child = fixture
        .std_thread_command()
        .arg("thread")
        .env("ZERDR_TEST_AGENT_GET_SEQ", &sequence)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    // Let the sequence drain so the settling transition is observed before detaching.
    thread::sleep(Duration::from_millis(300));
    fixture.release_attach();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(
        stdout.matches(OSC_PREFIX).count(),
        2,
        "one title per change, deduplicated: {stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}pi \u{b7} first title")),
        "{stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}pi \u{b7} second title")),
        "{stdout:?}"
    );
    assert_eq!(
        stdout.matches('\u{7}').count(),
        3,
        "two OSC terminators plus exactly one bell: {stdout:?}"
    );
}

#[test]
fn an_empty_title_falls_back_to_the_workspace_label() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "");

    let child = fixture.std_thread_command().arg("thread").spawn().unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    fixture.release_attach();
    let output = child.wait_with_output().unwrap();

    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(&format!("{OSC_PREFIX}pi \u{b7} checkout")),
        "{:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn a_failing_poll_leaves_the_attached_agent_and_the_last_title_alone() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "steady title");
    let sequence = fixture.env.root.path().join("agent-get-seq");
    write_sequence(
        &sequence,
        &[
            "EXIT:1\n",
            &serde_json::json!({"result": {"type": "agent_info"}}).to_string(),
            &agent_response("idle", "steady title"),
        ],
    );

    let child = fixture
        .std_thread_command()
        .arg("thread")
        .env("ZERDR_TEST_AGENT_GET_SEQ", &sequence)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    thread::sleep(Duration::from_millis(300));
    fixture.release_attach();
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "the attach child must survive");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.matches(OSC_PREFIX).count(),
        1,
        "the initial title only: {stdout:?}"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "",
        "polling failures must stay silent"
    );
}

/// A detach must return the terminal to Zed immediately. The monitor waits on a condvar
/// rather than sleeping, so a long poll interval cannot delay the exit.
#[test]
fn detaching_does_not_wait_for_the_next_poll() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "prompt detach");

    let started = std::time::Instant::now();
    fixture
        .thread_command()
        .arg("thread")
        .env("ZERDR_THREAD_POLL_MS", "30000")
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .assert()
        .success();

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "exit took {:?}, so the monitor blocked on its poll interval",
        started.elapsed()
    );
}

/// Focus is left alone when the workspace list is unavailable: guessing would re-fire
/// `workspace.focused` and drag Zed forward under follow mode.
#[test]
fn an_unreadable_workspace_list_leaves_herdr_focus_untouched() {
    let fixture = Fixture::new();
    let agent = agent_response("idle", "focus untouched");

    let output = fixture
        .std_thread_command()
        .args(["thread", "w1:p1"])
        .env("ZERDR_TEST_WORKSPACES_JSON", "")
        .env("ZERDR_TEST_AGENT_GET_JSON", agent)
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();

    assert!(output.status.success(), "the attach must still happen");
    let log = fixture.env.read_log();
    assert!(log.contains("agent attach w1:p1"), "{log}");
    assert!(!log.contains("workspace focus"), "{log}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("leaving focus alone"),
        "{:?}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Threads in unrelated Herdr sessions must not serialize against each other.
#[test]
fn the_resolve_lock_is_scoped_to_one_session_and_socket() {
    let fixture = Fixture::new();
    let leases = zerdr::state::ThreadLeaseSet::new(fixture.paths().thread_leases_dir);
    let other_socket = fixture.env.root.path().join("other.sock");
    fs::write(&other_socket, "").unwrap();

    let default_lock = leases
        .resolve_lock_path("default", &fixture.socket)
        .unwrap();
    let named_lock = leases.resolve_lock_path("work", &fixture.socket).unwrap();
    let other_socket_lock = leases.resolve_lock_path("default", &other_socket).unwrap();

    assert_ne!(default_lock, named_lock);
    assert_ne!(default_lock, other_socket_lock);
    assert_ne!(named_lock, other_socket_lock);
}

#[test]
fn the_attach_exit_status_is_propagated() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "exiting");

    fixture
        .thread_command()
        .arg("thread")
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .env("ZERDR_TEST_ATTACH_EXIT", "3")
        .assert()
        .code(3);
}

#[test]
fn an_unfocused_workspace_is_focused_exactly_once() {
    for focused in [true, false] {
        let fixture = Fixture::new();
        fixture.agent("zed-1", "w1:p1", "w1", "idle", "focus check");

        fixture
            .thread_command()
            .arg("thread")
            .env("ZERDR_TEST_WORKSPACES_JSON", fixture.workspaces(focused))
            .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
            .assert()
            .success();

        let log = fixture.env.read_log();
        let focus_calls = log.matches("workspace focus w1").count();
        assert_eq!(focus_calls, usize::from(!focused), "{log}");
    }
}

#[test]
fn a_missing_session_is_reported_without_starting_herdr() {
    let fixture = Fixture::new();

    fixture
        .thread_command()
        .arg("thread")
        .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not running"));

    let log = fixture.env.read_log();
    assert!(!log.contains("agent attach"), "{log}");
    assert!(!log.lines().any(|line| line == "herdr\t"), "{log}");
}

#[test]
fn a_non_git_directory_is_reported_with_the_target_hint() {
    let fixture = Fixture::new();
    let outside = fixture.env.root.path().join("not-a-checkout");
    fs::create_dir_all(&outside).unwrap();

    let mut command = fixture.thread_command();
    command
        .current_dir(&outside)
        .arg("thread")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Herdr pane id"));
}

#[test]
fn remote_environments_are_rejected_before_touching_herdr() {
    let fixture = Fixture::new();

    fixture
        .thread_command()
        .arg("thread")
        .env("SSH_CONNECTION", "10.0.0.1 22 10.0.0.2 22")
        .assert()
        .failure();

    assert_eq!(fixture.env.read_log(), "");
}

#[test]
fn thread_runs_without_the_plugin_or_install_state() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "no install state");

    fixture
        .thread_command()
        .arg("thread")
        .env("ZERDR_TEST_PLUGINS_JSON", r#"{"result":{"plugins":[]}}"#)
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .assert()
        .success();

    assert!(!fixture.paths().install_state_file.exists());
    assert!(
        !fixture.env.read_log().contains("plugin list"),
        "{}",
        fixture.env.read_log()
    );
}
