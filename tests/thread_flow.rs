mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::Duration;

use predicates::prelude::*;
use support::TestEnv;
use zerdr::state::{
    BindingStore, Paths, ThreadPaneMemory, thread_detach_clear, thread_detach_set,
};

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
        self.agent_of_kind("pi", name, pane_id, workspace_id, status, title);
    }

    fn agent_of_kind(
        &self,
        kind: &str,
        name: &str,
        pane_id: &str,
        workspace_id: &str,
        status: &str,
        title: &str,
    ) {
        self.agent_with_raw_title(kind, name, pane_id, workspace_id, status, title, title);
    }

    #[expect(clippy::too_many_arguments, reason = "test fixture mirrors Herdr JSON")]
    fn agent_with_raw_title(
        &self,
        kind: &str,
        name: &str,
        pane_id: &str,
        workspace_id: &str,
        status: &str,
        raw_title: &str,
        title: &str,
    ) {
        fs::write(
            self.agents_dir.join(format!("{name}.json")),
            serde_json::json!({
                "agent": kind,
                "name": name,
                "agent_status": status,
                "pane_id": pane_id,
                "workspace_id": workspace_id,
                "terminal_title": raw_title,
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

    /// A linked worktree of the fixture repo, created with plain `git worktree add` the
    /// way external tools do, canonicalized like `repo`.
    fn linked_worktree(&self, branch: &str) -> PathBuf {
        let git = |args: &[&str]| {
            assert!(
                ProcessCommand::new("git")
                    .args(args)
                    .current_dir(&self.repo)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        // `git worktree add` needs a commit to check out.
        git(&[
            "-c",
            "user.name=zerdr",
            "-c",
            "user.email=zerdr@invalid",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "init",
        ]);
        let path = self.env.root.path().join(branch);
        git(&[
            "worktree",
            "add",
            "--quiet",
            "-b",
            branch,
            path.to_str().unwrap(),
        ]);
        path.canonicalize().unwrap()
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

/// Waits until the fake herdr has served `count` `agent get` polls. The monitor emits
/// for poll N before starting poll N+1, so `count` = sequence length + 1 guarantees
/// every entry has been observed and emitted; a fixed sleep cannot, under load.
fn wait_for_sequence(directory: &Path, count: u64) {
    for _ in 0..400 {
        let served = fs::read_to_string(directory.join("counter"))
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(0);
        if served >= count {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {count} agent get polls");
}

fn agent_response(status: &str, title: &str) -> String {
    agent_response_of_kind("pi", status, title, title)
}

fn agent_response_of_kind(kind: &str, status: &str, raw_title: &str, title: &str) -> String {
    serde_json::json!({
        "result": {"agent": {
            "agent": kind,
            "name": "zed-1",
            "agent_status": status,
            "pane_id": "w1:p1",
            "workspace_id": "w1",
            "terminal_title": raw_title,
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

    let child = fixture.std_thread_command().arg("connect").spawn().unwrap();
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
    assert!(!log.contains("send-text"), "{log}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}○ [herdr] Pi - review")),
        "{stdout:?}"
    );
    let status = stdout.lines().next().unwrap_or_default();
    assert!(status.starts_with("zerdr: "), "{stdout:?}");
    assert!(status.contains("attached"), "{stdout:?}");
    assert!(status.contains("w1:p1"), "{stdout:?}");
    assert!(status.contains("checkout"), "{stdout:?}");
    assert!(!stdout.contains("auto mode"), "{stdout:?}");
}

#[test]
fn a_second_thread_picks_a_different_agent_pane() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "first");
    fixture.agent("zed-2", "w1:p2", "w1", "idle", "second");

    let mut first = fixture.std_thread_command().arg("connect").spawn().unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p");
    let mut second = fixture.std_thread_command().arg("connect").spawn().unwrap();
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
        .arg("connect")
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
        !log.contains("send-text"),
        "nothing is ever typed into the pane: {log}"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}[herdr] checkout")),
        "the sidebar shows the workspace label until an agent appears: {stdout:?}"
    );
    let status = stdout.lines().next().unwrap_or_default();
    assert!(status.starts_with("zerdr: "), "{stdout:?}");
    assert!(status.contains("new Herdr tab"), "{stdout:?}");
    assert!(status.contains("w1:p9"), "{stdout:?}");
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
            .arg("connect")
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
        .args(["connect", "--kind", "pi"])
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
        .arg("connect")
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .assert()
        .code(1)
        .stderr(predicate::str::contains("zerdr workspace bind"))
        .stderr(predicate::str::contains("zerdr connect --create"))
        .stderr(predicate::str::contains(fixture.repo.display().to_string()));

    assert!(!fixture.env.read_log().contains("workspace create"));
}

/// In an unmatched linked worktree the refusal explains what `--create` would do there:
/// open the worktree as a Herdr workspace, not just "make one".
#[test]
fn a_bare_thread_in_a_worktree_explains_worktree_registration() {
    let fixture = Fixture::new();
    let worktree = fixture.linked_worktree("feature");

    fixture
        .thread_command()
        .current_dir(&worktree)
        .arg("connect")
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "open this Git worktree as a Herdr workspace",
        ))
        .stderr(predicate::str::contains("zerdr connect --create"))
        .stderr(predicate::str::contains("zerdr workspace bind"))
        .stderr(predicate::str::contains(worktree.display().to_string()));

    let log = fixture.env.read_log();
    assert!(!log.contains("workspace create"), "{log}");
    assert!(!log.contains("worktree open"), "{log}");
}

/// With auto mode enabled, `--auto` behaves exactly like a manual `zerdr connect`.
#[test]
fn auto_attaches_a_free_agent_while_the_mode_is_enabled() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "review the diff");
    let paths = fixture.paths();
    fs::create_dir_all(&paths.state_dir).unwrap();
    fs::write(&paths.thread_auto_flag_file, b"").unwrap();

    let child = fixture
        .std_thread_command()
        .args(["connect", "--auto"])
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");

    fixture.release_attach();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}○ [herdr] Pi - review")),
        "{stdout:?}"
    );
    let status = stdout.lines().next().unwrap_or_default();
    assert!(status.starts_with("zerdr: "), "{stdout:?}");
    assert!(status.contains("auto mode is enabled"), "{stdout:?}");
    assert!(status.contains("w1:p1"), "{stdout:?}");
    assert!(status.contains("checkout"), "{stdout:?}");
    assert!(
        stdout.find(status).unwrap() < stdout.find(OSC_PREFIX).unwrap(),
        "the status line comes before the first title: {stdout:?}"
    );
    assert!(String::from_utf8_lossy(&output.stderr).is_empty());
}

/// An unmatched project no longer dead-ends: auto creates the workspace like an
/// explicit `--create`, binds it, and lands in its plain shell.
#[test]
fn auto_without_a_matching_workspace_creates_and_binds_one() {
    let fixture = Fixture::new();
    let paths = fixture.paths();
    fs::create_dir_all(&paths.state_dir).unwrap();
    fs::write(&paths.thread_auto_flag_file, b"").unwrap();
    let workspace = serde_json::json!({
        "result": {
            "workspace": {"workspace_id": "w7", "label": "checkout"},
            "root_pane": {"pane_id": "w7:p1"}
        }
    });

    let output = fixture
        .std_thread_command()
        .args(["connect", "--auto"])
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .env("ZERDR_TEST_WORKSPACE_CREATE_JSON", workspace.to_string())
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).is_empty());
    let log = fixture.env.read_log();
    assert!(
        log.contains("herdr\t--session default workspace create --cwd"),
        "{log}"
    );
    assert!(log.contains("--label checkout --no-focus"), "{log}");
    assert!(!log.contains("agent start"), "{log}");
    assert!(log.contains("terminal attach term-w7:p1"), "{log}");
    let bound = BindingStore::new(paths.bindings_file)
        .get("default", "w7")
        .unwrap();
    assert_eq!(bound, Some(fixture.repo.clone()));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let status = stdout.lines().next().unwrap_or_default();
    assert!(status.contains("auto mode is enabled"), "{stdout:?}");
    assert!(status.contains("created Herdr workspace"), "{stdout:?}");
    assert!(status.contains("checkout"), "{stdout:?}");
    assert!(status.contains("w7:p1"), "{stdout:?}");
}

/// Auto attach stays best-effort where creation cannot help: outside a Git checkout the
/// thread is left silently as a plain local shell.
#[test]
fn auto_outside_a_git_checkout_silently_leaves_a_plain_shell() {
    let fixture = Fixture::new();
    let paths = fixture.paths();
    fs::create_dir_all(&paths.state_dir).unwrap();
    fs::write(&paths.thread_auto_flag_file, b"").unwrap();
    let plain_dir = fixture.env.root.path().join("not-a-repository");
    fs::create_dir_all(&plain_dir).unwrap();

    let output = fixture
        .std_thread_command()
        .args(["connect", "--auto"])
        .current_dir(&plain_dir)
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
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
        .args(["connect", "--auto"])
        .env(
            "ZERDR_HERDR_BIN",
            fixture.env.root.path().join("missing-herdr"),
        )
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
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
        .arg("connect")
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
        .arg("connect")
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

    let output = fixture
        .std_thread_command()
        .args(["connect", "--create"])
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .env("ZERDR_TEST_WORKSPACE_CREATE_JSON", workspace.to_string())
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

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
    let stdout = String::from_utf8_lossy(&output.stdout);
    let status = stdout.lines().next().unwrap_or_default();
    assert!(status.contains("created Herdr workspace"), "{stdout:?}");
    assert!(status.contains("w7:p1"), "{stdout:?}");
    assert!(!stdout.contains("auto mode"), "{stdout:?}");
}

/// A linked worktree is registered with `herdr worktree open` so Herdr knows the
/// checkout's provenance, whatever tool created the worktree. The label comes from
/// Herdr's response, not the directory name.
#[test]
fn create_registers_a_linked_worktree_via_worktree_open() {
    let fixture = Fixture::new();
    let worktree = fixture.linked_worktree("feature");
    let opened = serde_json::json!({
        "result": {
            "workspace": {"workspace_id": "w8", "label": "checkout/feature"},
            "root_pane": {"pane_id": "w8:p1"}
        }
    });

    let output = fixture
        .std_thread_command()
        .current_dir(&worktree)
        .args(["connect", "--create"])
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .env("ZERDR_TEST_WORKTREE_OPEN_JSON", opened.to_string())
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let log = fixture.env.read_log();
    // Herdr resolves worktree actions from the repo parent, so the open must anchor
    // there explicitly; ambient context (process cwd, caller env) is not enough in a
    // Zed terminal.
    assert!(
        log.contains(&format!(
            "herdr\t--session default worktree open --cwd {} --path {} --no-focus",
            fixture.repo.display(),
            worktree.display()
        )),
        "{log}"
    );
    assert!(!log.contains("workspace create"), "{log}");
    assert!(log.contains("terminal attach term-w8:p1"), "{log}");

    let bound = BindingStore::new(fixture.paths().bindings_file)
        .get("default", "w8")
        .unwrap();
    assert_eq!(bound, Some(worktree));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let status = stdout.lines().next().unwrap_or_default();
    assert!(status.contains("created Herdr workspace"), "{stdout:?}");
    assert!(status.contains("checkout/feature"), "{stdout:?}");
    assert!(status.contains("w8:p1"), "{stdout:?}");
}

/// Auto mode's create-on-miss shares the create path, so a worktree opened in Zed gets
/// the same registration without an explicit `--create`.
#[test]
fn auto_in_a_linked_worktree_creates_via_worktree_open() {
    let fixture = Fixture::new();
    let worktree = fixture.linked_worktree("feature");
    let paths = fixture.paths();
    fs::create_dir_all(&paths.state_dir).unwrap();
    fs::write(&paths.thread_auto_flag_file, b"").unwrap();
    let opened = serde_json::json!({
        "result": {
            "workspace": {"workspace_id": "w8", "label": "checkout/feature"},
            "root_pane": {"pane_id": "w8:p1"}
        }
    });

    let output = fixture
        .std_thread_command()
        .current_dir(&worktree)
        .args(["connect", "--auto"])
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .env("ZERDR_TEST_WORKTREE_OPEN_JSON", opened.to_string())
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).is_empty());

    let log = fixture.env.read_log();
    assert!(
        log.contains(&format!("worktree open --cwd {}", fixture.repo.display())),
        "{log}"
    );
    assert!(!log.contains("workspace create"), "{log}");
    let bound = BindingStore::new(paths.bindings_file)
        .get("default", "w8")
        .unwrap();
    assert_eq!(bound, Some(worktree));
}

/// A failing `worktree open` must abort the create: falling back to a plain workspace
/// would recreate exactly the unregistered-worktree state this path removes.
#[test]
fn a_failing_worktree_open_aborts_create_without_fallback() {
    let fixture = Fixture::new();
    let worktree = fixture.linked_worktree("feature");

    fixture
        .thread_command()
        .current_dir(&worktree)
        .args(["connect", "--create"])
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .env("ZERDR_TEST_WORKTREE_OPEN_EXIT", "13")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("fake worktree open failure"));

    let log = fixture.env.read_log();
    assert!(!log.contains("workspace create"), "{log}");
    let bound = BindingStore::new(fixture.paths().bindings_file)
        .get("default", "w8")
        .unwrap();
    assert_eq!(bound, None);
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
                .arg("connect")
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
        .args(["connect", "w1:p1"])
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
    let stdout = String::from_utf8_lossy(&output.stdout);
    let status = stdout.lines().next().unwrap_or_default();
    assert!(status.starts_with("zerdr: "), "{stdout:?}");
    assert!(status.contains("attached"), "{stdout:?}");
    assert!(status.contains("w1:p1"), "{stdout:?}");
    assert!(!stdout.contains("auto mode"), "{stdout:?}");
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
        .arg("connect")
        .env("ZERDR_TEST_AGENT_GET_SEQ", &sequence)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    // Let the sequence drain so the settling transition is observed before detaching.
    wait_for_sequence(&sequence, 4);
    fixture.release_attach();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(
        stdout.matches(OSC_PREFIX).count(),
        2,
        "one title per change, deduplicated: {stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}◐ [herdr] Pi - first title")),
        "{stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}○ [herdr] Pi - second title")),
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

    let child = fixture.std_thread_command().arg("connect").spawn().unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    fixture.release_attach();
    let output = child.wait_with_output().unwrap();

    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains(&format!("{OSC_PREFIX}○ [herdr] Pi - checkout")),
        "{:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// Kind display names: known kinds map to friendly names, others are capitalized, and
/// pi's own "π - " title prefix is stripped from the detail.
#[test]
fn titles_carry_the_herdr_marker_and_kind_display_names() {
    for (kind, title, expected) in [
        (
            "claude",
            "コード内の重複パターン洗い出し",
            "○ [herdr] Claude - コード内の重複パターン洗い出し",
        ),
        (
            "pi",
            "π - 施策を進める - mog-app",
            "○ [herdr] Pi - 施策を進める - mog-app",
        ),
        ("codex", "t", "○ [herdr] Codex - t"),
        // A degenerate pi title that is only the stripped prefix falls back to the
        // workspace label instead of leaving a dangling separator.
        ("pi", "\u{3c0} - ", "○ [herdr] Pi - checkout"),
    ] {
        let fixture = Fixture::new();
        fixture.agent_of_kind(kind, "zed-1", "w1:p1", "w1", "idle", title);

        let child = fixture.std_thread_command().arg("connect").spawn().unwrap();
        wait_for_log(&fixture.env, "agent attach w1:p1");
        fixture.release_attach();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(&format!("{OSC_PREFIX}{expected}")),
            "{kind}: {stdout:?}"
        );
    }
}

/// An agent that decorates its own title with a leading glyph (Claude Code's title
/// spinner) keeps that glyph in the emitted title instead of the status fallback, and
/// a frame advance alone re-emits the title.
#[test]
fn a_raw_title_glyph_passes_through_and_frame_changes_reemit() {
    let fixture = Fixture::new();
    fixture.agent_with_raw_title(
        "claude",
        "zed-1",
        "w1:p1",
        "w1",
        "working",
        "⠐ fix tests",
        "fix tests",
    );
    let sequence = fixture.env.root.path().join("agent-get-seq");
    write_sequence(
        &sequence,
        &[
            &agent_response_of_kind("claude", "working", "⠐ fix tests", "fix tests"),
            &agent_response_of_kind("claude", "working", "⠙ fix tests", "fix tests"),
        ],
    );

    let child = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_AGENT_GET_SEQ", &sequence)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    wait_for_sequence(&sequence, 3);
    fixture.release_attach();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains(&format!("{OSC_PREFIX}⠐ [herdr] Claude - fix tests")),
        "{stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}⠙ [herdr] Claude - fix tests")),
        "the frame advance alone re-emits the title: {stdout:?}"
    );
    assert!(
        !stdout.contains("◐"),
        "the native glyph wins over the status fallback: {stdout:?}"
    );
}

/// Without a usable raw-title glyph, the status maps to Herdr's Symbols indicator set.
#[test]
fn status_glyphs_follow_the_herdr_symbol_set() {
    for (status, glyph) in [
        ("working", "◐"),
        ("blocked", "×"),
        ("done", "✓"),
        ("idle", "○"),
        ("unknown", "·"),
    ] {
        let fixture = Fixture::new();
        fixture.agent("zed-1", "w1:p1", "w1", status, "review the diff");

        let child = fixture.std_thread_command().arg("connect").spawn().unwrap();
        wait_for_log(&fixture.env, "agent attach w1:p1");
        fixture.release_attach();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(&format!("{OSC_PREFIX}{glyph} [herdr] Pi - review the diff")),
            "{status}: {stdout:?}"
        );
    }
}

/// A glyph run not followed by whitespace is not a prefix, and an alphanumeric lead
/// (pi's `π`) never is; both fall back to the status glyph. So do a glyph with
/// nothing after it and a run carrying control characters, which must never reach
/// the emitted OSC payload.
#[test]
fn unusable_raw_prefixes_fall_back_to_the_status_glyph() {
    for (kind, raw_title, title, expected) in [
        (
            "claude",
            "✳Thinking",
            "Thinking",
            "◐ [herdr] Claude - Thinking",
        ),
        ("pi", "π - review", "π - review", "◐ [herdr] Pi - review"),
        ("claude", "✳ ", "", "◐ [herdr] Claude - checkout"),
        ("claude", "\u{1b}\u{7} go", "go", "◐ [herdr] Claude - go"),
    ] {
        let fixture = Fixture::new();
        fixture.agent_with_raw_title(kind, "zed-1", "w1:p1", "w1", "working", raw_title, title);

        let child = fixture.std_thread_command().arg("connect").spawn().unwrap();
        wait_for_log(&fixture.env, "agent attach w1:p1");
        fixture.release_attach();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(&format!("{OSC_PREFIX}{expected}")),
            "{kind}: {stdout:?}"
        );
    }
}

/// A settling transition with an unchanged detail still re-emits: the glyph is part of
/// the deduplicated label, and the bell keeps marking the settle.
#[test]
fn a_status_transition_reemits_the_title_with_the_new_glyph() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "working", "same title");
    let sequence = fixture.env.root.path().join("agent-get-seq");
    write_sequence(
        &sequence,
        &[
            &agent_response("working", "same title"),
            &agent_response("idle", "same title"),
        ],
    );

    let child = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_AGENT_GET_SEQ", &sequence)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    wait_for_sequence(&sequence, 3);
    fixture.release_attach();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(
        stdout.matches(OSC_PREFIX).count(),
        2,
        "the glyph change alone re-emits: {stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}◐ [herdr] Pi - same title")),
        "{stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}○ [herdr] Pi - same title")),
        "{stdout:?}"
    );
    assert_eq!(
        stdout.matches('\u{7}').count(),
        3,
        "two OSC terminators plus exactly one bell: {stdout:?}"
    );
}

#[test]
fn a_failing_poll_leaves_the_attached_agent_and_the_last_title_alone() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "steady title");
    let sequence = fixture.env.root.path().join("agent-get-seq");
    write_sequence(
        &sequence,
        &["EXIT:1\n", &agent_response("idle", "steady title")],
    );

    let child = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_AGENT_GET_SEQ", &sequence)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    wait_for_sequence(&sequence, 3);
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

/// `agent get` answering without an agent is definitive, unlike a failed poll: the
/// agent was quit and the pane is a plain shell again. The sidebar reverts to the
/// workspace label, and quitting mid-work settles the thread, so the bell still
/// rings — once, not on every subsequent agentless poll.
#[test]
fn a_quit_agent_reverts_the_title_to_the_workspace_label_and_bells_once() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "working", "in flight");
    let sequence = fixture.env.root.path().join("agent-get-seq");
    write_sequence(
        &sequence,
        &[
            &agent_response("working", "in flight"),
            &serde_json::json!({"result": {"type": "agent_info"}}).to_string(),
        ],
    );

    let child = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_AGENT_GET_SEQ", &sequence)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    wait_for_sequence(&sequence, 3);
    fixture.release_attach();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(
        stdout.matches(OSC_PREFIX).count(),
        2,
        "the agent title once, then the shell fallback once: {stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}◐ [herdr] Pi - in flight")),
        "{stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}[herdr] checkout\u{7}")),
        "the fallback carries no glyph and no agent detail: {stdout:?}"
    );
    assert_eq!(
        stdout.matches('\u{7}').count(),
        3,
        "two OSC terminators plus exactly one settle bell: {stdout:?}"
    );
}

/// An agent gone from `idle` is not a settle, so no bell rings; and the monitor keeps
/// polling after the revert, so an agent started later in the same pane retakes the
/// sidebar title.
#[test]
fn a_vanished_idle_agent_reverts_silently_and_a_restart_retakes_the_title() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "first run");
    let sequence = fixture.env.root.path().join("agent-get-seq");
    write_sequence(
        &sequence,
        &[
            &agent_response("idle", "first run"),
            &serde_json::json!({"result": {"type": "agent_info"}}).to_string(),
            &agent_response("working", "second run"),
        ],
    );

    let child = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_AGENT_GET_SEQ", &sequence)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    wait_for_sequence(&sequence, 4);
    fixture.release_attach();
    let output = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(
        stdout.matches(OSC_PREFIX).count(),
        3,
        "agent title, shell fallback, then the restarted agent: {stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}○ [herdr] Pi - first run")),
        "{stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}[herdr] checkout\u{7}")),
        "{stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}◐ [herdr] Pi - second run")),
        "{stdout:?}"
    );
    assert_eq!(
        stdout.matches('\u{7}').count(),
        3,
        "three OSC terminators and no bell: {stdout:?}"
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
        .arg("connect")
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
        .args(["connect", "w1:p1"])
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
        .arg("connect")
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
            .arg("connect")
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
        .arg("connect")
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
        .arg("connect")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Herdr pane id"));
}

#[test]
fn remote_environments_are_rejected_before_touching_herdr() {
    let fixture = Fixture::new();

    fixture
        .thread_command()
        .arg("connect")
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
        .arg("connect")
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

/// A thread reopened later (e.g. after a Zed restart) reattaches to the remembered
/// shell pane instead of creating another tab.
#[test]
fn a_restored_thread_reattaches_the_remembered_shell_pane() {
    let fixture = Fixture::new();
    let tab = serde_json::json!({"result": {"root_pane": {"pane_id": "w1:p9"}}});

    for _ in 0..2 {
        let output = fixture
            .std_thread_command()
            .arg("connect")
            .env("ZERDR_TEST_TAB_CREATE_JSON", tab.to_string())
            .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
            .spawn()
            .unwrap()
            .wait_with_output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.lines().next().unwrap_or_default().contains("w1:p9"),
            "{stdout:?}"
        );
        if stdout.contains("reattached") {
            // Second run: sidebar still shows the workspace label for a shell pane.
            assert!(
                stdout.contains(&format!("{OSC_PREFIX}[herdr] checkout")),
                "{stdout:?}"
            );
        }
    }

    let log = fixture.env.read_log();
    assert_eq!(log.matches("tab create").count(), 1, "{log}");
    assert_eq!(
        log.matches("terminal attach term-w1:p9").count(),
        2,
        "{log}"
    );
    assert!(!log.contains("send-text"), "{log}");
}

/// With several remembered panes the most recently attached one wins.
#[test]
fn reattach_prefers_the_most_recently_attached_pane() {
    let fixture = Fixture::new();
    let counter = fixture.env.root.path().join("pane-counter");
    let first_release = fixture.env.root.path().join("first-release");
    let second_release = fixture.env.root.path().join("second-release");

    let first = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_PANE_COUNTER_FILE", &counter)
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", &first_release)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "terminal attach term-w1:p1");
    let second = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_PANE_COUNTER_FILE", &counter)
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", &second_release)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "terminal attach term-w1:p2");
    fs::write(&first_release, "go").unwrap();
    fs::write(&second_release, "go").unwrap();
    assert!(first.wait_with_output().unwrap().status.success());
    assert!(second.wait_with_output().unwrap().status.success());

    let output = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_PANE_COUNTER_FILE", &counter)
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let log = fixture.env.read_log();
    assert_eq!(log.matches("tab create").count(), 2, "{log}");
    assert_eq!(
        log.matches("terminal attach term-w1:p2").count(),
        2,
        "the most recently attached pane is reattached: {log}"
    );
}

/// A remembered pane that no longer exists is pruned and never blocks resolution.
#[test]
fn a_dead_remembered_pane_is_pruned_and_a_new_tab_is_created() {
    let fixture = Fixture::new();
    let paths = fixture.paths();
    let first_tab = serde_json::json!({"result": {"root_pane": {"pane_id": "w1:p9"}}});
    let output = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_TAB_CREATE_JSON", first_tab.to_string())
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let second_tab = serde_json::json!({"result": {"root_pane": {"pane_id": "w1:p10"}}});
    let output = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_TAB_CREATE_JSON", second_tab.to_string())
        .env("ZERDR_TEST_PANE_GET_MISSING_IDS", "w1:p9")
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let log = fixture.env.read_log();
    assert!(log.contains("terminal attach term-w1:p10"), "{log}");
    let memory = ThreadPaneMemory::new(paths.thread_memory_dir);
    let panes = memory.load("default", &fixture.socket);
    assert!(
        panes.iter().all(|record| record.pane_id != "w1:p9"),
        "{panes:?}"
    );
    assert!(
        panes.iter().any(|record| record.pane_id == "w1:p10"),
        "{panes:?}"
    );
}

/// Free agents keep their priority over remembered shell panes.
#[test]
fn a_free_agent_still_wins_over_a_remembered_shell_pane() {
    let fixture = Fixture::new();
    let tab = serde_json::json!({"result": {"root_pane": {"pane_id": "w1:p9"}}});
    let output = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_TAB_CREATE_JSON", tab.to_string())
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    fixture.agent("zed-1", "w1:p1", "w1", "idle", "ready");
    let child = fixture.std_thread_command().arg("connect").spawn().unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    fixture.release_attach();
    assert!(child.wait_with_output().unwrap().status.success());

    let log = fixture.env.read_log();
    assert_eq!(
        log.matches("terminal attach term-w1:p9").count(),
        1,
        "the remembered shell pane must not outrank the free agent: {log}"
    );
}

/// Memory is scoped per workspace: a pane remembered elsewhere is never probed.
#[test]
fn a_pane_remembered_for_another_workspace_is_not_considered() {
    let fixture = Fixture::new();
    let paths = fixture.paths();
    ThreadPaneMemory::new(paths.thread_memory_dir)
        .record("default", &fixture.socket, "w2", "w2:p1")
        .unwrap();
    let tab = serde_json::json!({"result": {"root_pane": {"pane_id": "w1:p9"}}});

    let output = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_TAB_CREATE_JSON", tab.to_string())
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let log = fixture.env.read_log();
    assert!(!log.contains("pane get w2:p1"), "{log}");
    assert!(log.contains("terminal attach term-w1:p9"), "{log}");
}

/// The explicit-TARGET path records its pane too, outside resolve_or_create.
#[test]
fn an_explicit_target_attach_is_remembered() {
    let fixture = Fixture::new();
    let paths = fixture.paths();
    let agent = agent_response("idle", "explicit target");

    let child = fixture
        .std_thread_command()
        .args(["connect", "w1:p1"])
        .env("ZERDR_TEST_AGENT_GET_JSON", agent)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    fixture.release_attach();
    assert!(child.wait_with_output().unwrap().status.success());

    let panes = ThreadPaneMemory::new(paths.thread_memory_dir).load("default", &fixture.socket);
    assert!(
        panes
            .iter()
            .any(|record| record.pane_id == "w1:p1" && record.workspace_id == "w1"),
        "{panes:?}"
    );
}

/// `--create` also covers the session: a not-running named session is started
/// headless, and only then does the normal workspace create/attach flow run.
#[test]
fn create_starts_a_not_running_named_session_headless() {
    let fixture = Fixture::new();
    let sessions_file = fixture.env.root.path().join("sessions-live.json");
    let work_socket = fixture.env.root.path().join("work.sock");
    fs::write(&work_socket, "").unwrap();
    let started = serde_json::json!({
        "sessions": [
            {"name": "default", "running": true, "socket_path": fixture.socket},
            {"name": "work", "running": true, "socket_path": work_socket}
        ]
    });
    let workspace = serde_json::json!({
        "result": {
            "workspace": {"workspace_id": "w7", "label": "checkout"},
            "root_pane": {"pane_id": "w7:p1"}
        }
    });

    let output = fixture
        .std_thread_command()
        .args(["connect", "--create", "--session", "work"])
        .env("ZERDR_TEST_SESSIONS_FILE", &sessions_file)
        .env("ZERDR_TEST_SESSIONS_STARTED_JSON", started.to_string())
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .env("ZERDR_TEST_WORKSPACE_CREATE_JSON", workspace.to_string())
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let log = fixture.env.read_log();
    let server = log.find("herdr\t--session work server").expect(&log);
    let workspaces = log
        .find("herdr\t--session work workspace list")
        .expect(&log);
    assert!(server < workspaces, "{log}");
    assert!(
        log.contains("--session work workspace create --cwd"),
        "{log}"
    );
    assert!(log.contains("terminal attach term-w7:p1"), "{log}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("started Herdr session work"), "{stdout:?}");
    assert!(stdout.contains("created Herdr workspace"), "{stdout:?}");

    let paths = fixture.paths();
    let bound = BindingStore::new(paths.bindings_file.clone())
        .get("work", "w7")
        .unwrap();
    assert_eq!(bound, Some(fixture.repo.clone()));
    // Sessions started by connect get no route: sync stays inert until a
    // `zerdr start` wrapper attaches.
    assert!(!paths.routes_dir.exists());
}

/// Without `--create`, a not-running named session stays an error that points
/// at `--create`, and no server process is spawned.
#[test]
fn connect_without_create_names_the_missing_session_and_starts_nothing() {
    let fixture = Fixture::new();
    fixture
        .thread_command()
        .args(["connect", "--session", "work"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "zerdr connect --create --session work",
        ));
    assert!(!fixture.env.read_log().contains(" server"));
}

/// The default session is only ever started by `zerdr start`, never by
/// `connect --create`.
#[test]
fn create_never_starts_the_default_session() {
    for args in [
        vec!["connect", "--create"],
        vec!["connect", "--create", "--session", "default"],
    ] {
        let fixture = Fixture::new();
        fixture
            .thread_command()
            .args(args)
            .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
            .assert()
            .code(1)
            .stderr(predicate::str::contains("zerdr start"));
        assert!(!fixture.env.read_log().contains(" server"));
    }
}

/// The auto path is best-effort and must never spawn a session server: a
/// not-running session degrades silently to a plain local shell.
#[test]
fn auto_never_starts_a_session_server() {
    let fixture = Fixture::new();
    let paths = fixture.paths();
    fs::create_dir_all(&paths.state_dir).unwrap();
    fs::write(&paths.thread_auto_flag_file, b"").unwrap();
    let sessions_file = fixture.env.root.path().join("sessions-live.json");

    let assert = fixture
        .thread_command()
        .args(["connect", "--auto", "--session", "work"])
        .env("ZERDR_TEST_SESSIONS_FILE", &sessions_file)
        .env("ZERDR_TEST_SESSIONS_STARTED_JSON", fixture.sessions())
        .assert()
        .success();
    assert!(assert.get_output().stdout.is_empty());
    assert!(!fixture.env.read_log().contains(" server"));
}

/// A server that never registers its session fails the attach with a timeout
/// naming the session instead of hanging.
#[test]
fn server_readiness_timeout_fails_with_the_session_name() {
    let fixture = Fixture::new();
    let sessions_file = fixture.env.root.path().join("sessions-live.json");

    fixture
        .thread_command()
        .args(["connect", "--create", "--session", "work"])
        .env("ZERDR_TEST_SESSIONS_FILE", &sessions_file)
        .env("ZERDR_READY_TIMEOUT_MS", "200")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("timed out"))
        .stderr(predicate::str::contains("work"));
    assert!(
        fixture
            .env
            .read_log()
            .contains("herdr\t--session work server")
    );
}

/// `--create` against an already-running named session must not spawn a
/// second server; the existing attach flow runs unchanged.
#[test]
fn create_with_a_running_named_session_does_not_start_a_server() {
    let fixture = Fixture::new();
    let work_socket = fixture.env.root.path().join("work.sock");
    fs::write(&work_socket, "").unwrap();
    let sessions = serde_json::json!({
        "sessions": [
            {"name": "default", "running": true, "socket_path": fixture.socket},
            {"name": "work", "running": true, "socket_path": work_socket}
        ]
    });

    let output = fixture
        .std_thread_command()
        .args(["connect", "--create", "--session", "work"])
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env(
            "ZERDR_TEST_PANE_COUNTER_FILE",
            fixture.env.root.path().join("pane-counter"),
        )
        .env("ZERDR_TEST_ATTACH_RELEASE_FILE", "")
        .spawn()
        .unwrap()
        .wait_with_output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    let log = fixture.env.read_log();
    assert!(!log.contains(" server"), "{log}");
    assert!(log.contains("--session work tab create"), "{log}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("started Herdr session"), "{stdout:?}");
}

fn count_detach_markers(paths: &Paths) -> usize {
    fs::read_dir(&paths.thread_leases_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .flat_map(|entry| fs::read_dir(entry.path()).into_iter().flatten().flatten())
        .filter(|entry| {
            entry.path().extension().and_then(|value| value.to_str()) == Some("detached")
        })
        .count()
}

fn wait_until(description: &str, mut predicate: impl FnMut() -> bool) {
    for _ in 0..400 {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {description}");
}

#[test]
fn the_detach_flag_suspends_the_attach_and_clearing_it_reattaches() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "review the diff");
    let paths = fixture.paths();

    let child = fixture.std_thread_command().arg("connect").spawn().unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");

    thread_detach_set(&paths).unwrap();
    wait_until("the detach marker", || count_detach_markers(&paths) == 1);
    assert_eq!(count_leases(&paths), 1, "the lease survives the detach");
    assert!(!fixture.env.read_log().contains("terminal attach"));

    thread_detach_clear(&paths).unwrap();
    wait_for_log(&fixture.env, "terminal attach term-w1:p1");
    wait_until("the marker to clear", || count_detach_markers(&paths) == 0);
    let log = fixture.env.read_log();
    assert!(log.contains("pane get w1:p1"), "{log}");

    fixture.release_attach();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(count_leases(&paths), 0);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("zerdr: detached from Herdr; run `zerdr attach` to reconnect"),
        "{stdout:?}"
    );
}

#[test]
fn a_detached_thread_keeps_a_marked_title_and_never_rings_the_bell() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "working", "compiling");
    let paths = fixture.paths();
    let sequence = fixture.env.root.path().join("agent-get-seq");
    write_sequence(&sequence, &[&agent_response("working", "compiling")]);

    let child = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_AGENT_GET_SEQ", &sequence)
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");

    thread_detach_set(&paths).unwrap();
    wait_until("the detach marker", || count_detach_markers(&paths) == 1);
    let polled = |directory: &Path| {
        fs::read_to_string(directory.join("counter"))
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    // Guarantee at least one full poll after the detach so the marked title is emitted.
    wait_for_sequence(&sequence, polled(&sequence) + 2);

    // The settling transition happens while detached, so it must not ring the bell.
    fs::write(
        sequence.join("2.json"),
        agent_response("idle", "compiling"),
    )
    .unwrap();
    wait_for_sequence(&sequence, polled(&sequence) + 2);

    let pid = child.id().to_string();
    assert!(
        ProcessCommand::new("kill")
            .args(["-INT", &pid])
            .status()
            .unwrap()
            .success()
    );
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}◐ [herdr] Pi - compiling")),
        "{stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}◐ [herdr⏸] Pi - compiling")),
        "{stdout:?}"
    );
    assert!(
        stdout.contains(&format!("{OSC_PREFIX}○ [herdr⏸] Pi - compiling")),
        "{stdout:?}"
    );
    assert_eq!(
        stdout.matches('\u{7}').count(),
        stdout.matches(OSC_PREFIX).count(),
        "every BEL is an OSC terminator, no settle bell: {stdout:?}"
    );
}

#[test]
fn reattaching_to_a_missing_pane_ends_the_thread_gracefully() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "review the diff");
    let paths = fixture.paths();

    let child = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_PANE_GET_MISSING_IDS", "w1:p1")
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");

    thread_detach_set(&paths).unwrap();
    wait_until("the detach marker", || count_detach_markers(&paths) == 1);
    thread_detach_clear(&paths).unwrap();

    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("zerdr: Herdr pane w1:p1 is gone; closing this thread connection"),
        "{stdout:?}"
    );
    assert_eq!(count_leases(&paths), 0);
    assert_eq!(count_detach_markers(&paths), 0);
    assert!(!fixture.env.read_log().contains("terminal attach"));
}

#[test]
fn a_thread_started_during_detach_waits_and_defers_the_focus() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "review the diff");
    let paths = fixture.paths();
    thread_detach_set(&paths).unwrap();

    let child = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_TEST_WORKSPACES_JSON", fixture.workspaces(false))
        .spawn()
        .unwrap();
    wait_until("the detach marker", || count_detach_markers(&paths) == 1);
    assert_eq!(count_leases(&paths), 1);
    let log = fixture.env.read_log();
    assert!(!log.contains("agent attach"), "{log}");
    assert!(!log.contains("terminal attach"), "{log}");
    assert!(!log.contains("workspace focus"), "{log}");

    thread_detach_clear(&paths).unwrap();
    wait_for_log(&fixture.env, "terminal attach term-w1:p1");
    let log = fixture.env.read_log();
    assert!(log.contains("workspace focus w1"), "{log}");

    fixture.release_attach();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("zerdr: detached from Herdr; run `zerdr attach` to reconnect"),
        "{stdout:?}"
    );
}

#[test]
fn a_signal_during_the_detached_wait_releases_the_lease_and_marker() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "review the diff");
    let paths = fixture.paths();

    let child = fixture.std_thread_command().arg("connect").spawn().unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");
    thread_detach_set(&paths).unwrap();
    wait_until("the detach marker", || count_detach_markers(&paths) == 1);

    let pid = child.id().to_string();
    assert!(
        ProcessCommand::new("kill")
            .args(["-INT", &pid])
            .status()
            .unwrap()
            .success()
    );
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(count_leases(&paths), 0);
    assert_eq!(count_detach_markers(&paths), 0);
}

#[test]
fn zerdr_detach_and_attach_drive_a_live_thread() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "review the diff");
    let paths = fixture.paths();

    let child = fixture.std_thread_command().arg("connect").spawn().unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");

    fixture
        .env
        .command()
        .arg("detach")
        .assert()
        .success()
        .stdout(predicate::str::contains("detached 1 thread(s)"));
    assert!(paths.thread_detach_flag_file.exists());
    assert_eq!(count_detach_markers(&paths), 1);
    assert!(!fixture.env.read_log().contains("terminal attach"));

    fixture
        .env
        .command()
        .arg("attach")
        .assert()
        .success()
        .stdout(predicate::str::contains("reattached 1 thread(s)"));
    assert!(!paths.thread_detach_flag_file.exists());
    assert_eq!(count_detach_markers(&paths), 0);
    wait_for_log(&fixture.env, "terminal attach term-w1:p1");

    fixture.release_attach();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn zerdr_detach_without_threads_sets_the_flag_for_future_threads() {
    let fixture = Fixture::new();
    let paths = fixture.paths();

    fixture
        .env
        .command()
        .arg("detach")
        .assert()
        .success()
        .stdout(predicate::str::contains("new threads will start detached"));
    assert!(paths.thread_detach_flag_file.exists());

    fixture
        .env
        .command()
        .arg("attach")
        .assert()
        .success()
        .stdout(predicate::str::contains("detach mode is off"));
    assert!(!paths.thread_detach_flag_file.exists());
}

#[test]
fn zerdr_attach_when_not_detached_is_a_noop() {
    let fixture = Fixture::new();

    fixture
        .env
        .command()
        .arg("attach")
        .assert()
        .success()
        .stdout(predicate::str::contains("detach mode is not active"));
}

#[test]
fn zerdr_detach_warns_when_a_thread_does_not_confirm_in_time() {
    let fixture = Fixture::new();
    fixture.agent("zed-1", "w1:p1", "w1", "idle", "review the diff");

    let mut child = fixture
        .std_thread_command()
        .arg("connect")
        .env("ZERDR_THREAD_CYCLE_POLL_MS", "60000")
        .spawn()
        .unwrap();
    wait_for_log(&fixture.env, "agent attach w1:p1");

    fixture
        .env
        .command()
        .arg("detach")
        .env("ZERDR_DETACH_WAIT_MS", "200")
        .assert()
        .failure()
        .stderr(predicate::str::contains("did not confirm"));

    // The connect's cycle poll is deliberately enormous, so waiting for a natural
    // exit would stall the suite; tear the processes down instead.
    child.kill().unwrap();
    child.wait().unwrap();
    fixture.release_attach();
}
