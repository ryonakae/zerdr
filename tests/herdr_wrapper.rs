mod support;

use std::fs;
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::Duration;

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use support::TestEnv;
use zerdr::state::{BindingStore, LeaseSet, Paths, RouteStore};

#[test]
fn wrapper_holds_a_lease_runs_startup_sync_and_preserves_session() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let repo = env.root.path().join("repo");
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
    BindingStore::new(Paths::for_test(env.root.path()).bindings_file)
        .bind("w1", &repo)
        .unwrap();
    let sessions = serde_json::json!({
        "ok": true,
        "result": {"sessions": [{"name": "zerdr", "socket_path": socket}]}
    });
    let workspaces = serde_json::json!({
        "ok": true,
        "result": {"workspaces": [{
            "workspace_id": "w1", "label": "repo", "number": 1,
            "focused": true, "cwd": repo
        }]}
    });

    env.command()
        .args(["herdr", "--anchor"])
        .arg(&repo)
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_HERDR_SLEEP", "0.2")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    let log = env.read_log();
    assert!(log.contains("herdr\t--session zerdr\n"), "{log}");
    assert!(log.contains("herdr\tsession list --json"), "{log}");
    assert_eq!(log.matches("zed\t--existing").count(), 1, "{log}");
    assert!(!log.contains("stop"), "{log}");
    assert!(!log.contains("delete"), "{log}");

    let leases = Paths::for_test(env.root.path()).leases_dir;
    let remaining = fs::read_dir(leases)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|entry| fs::read_dir(entry.path()).into_iter().flatten().flatten())
        .count();
    assert_eq!(remaining, 0);
}

#[test]
fn concurrent_wrappers_admit_one_owner_and_reap_the_loser() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let first_anchor = env.root.path().join("first-anchor");
    let second_anchor = env.root.path().join("second-anchor");
    for anchor in [&first_anchor, &second_anchor] {
        fs::create_dir_all(anchor).unwrap();
        assert!(
            ProcessCommand::new("git")
                .args(["init", "--quiet"])
                .current_dir(anchor)
                .status()
                .unwrap()
                .success()
        );
    }
    let first_anchor = first_anchor.canonicalize().unwrap();
    let second_anchor = second_anchor.canonicalize().unwrap();
    let sessions = serde_json::json!({
        "sessions": [{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[]}});
    let release = env.root.path().join("release-session-discovery");
    let first_ready = env.root.path().join("first-ready");
    let second_ready = env.root.path().join("second-ready");
    let first_client_pid = env.root.path().join("first-client.pid");
    let second_client_pid = env.root.path().join("second-client.pid");
    let spawn_wrapper =
        |anchor: &std::path::Path, ready: &std::path::Path, child_pid: &std::path::Path| {
            let mut command = env.std_command();
            command
                .args(["herdr", "--anchor"])
                .arg(anchor)
                .env("ZED_TERM", "true")
                .env("TERM_PROGRAM", "zed")
                .env("ZERDR_TEST_HERDR_SLEEP", "10")
                .env("ZERDR_TEST_CHILD_PID_FILE", child_pid)
                .env("ZERDR_TEST_SESSION_READY_FILE", ready)
                .env("ZERDR_TEST_SESSION_RELEASE_FILE", &release)
                .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
                .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
                .stderr(Stdio::null());
            command.spawn().unwrap()
        };
    let mut first = spawn_wrapper(&first_anchor, &first_ready, &first_client_pid);
    let mut second = spawn_wrapper(&second_anchor, &second_ready, &second_client_pid);
    wait_for_file(&first_ready);
    wait_for_file(&second_ready);
    fs::write(&release, "go").unwrap();

    let (winner, winner_anchor, loser_pid_file) = loop {
        let first_status = first.try_wait().unwrap();
        let second_status = second.try_wait().unwrap();
        match (first_status, second_status) {
            (Some(status), None) => {
                assert!(!status.success());
                break (&mut second, &second_anchor, &first_client_pid);
            }
            (None, Some(status)) => {
                assert!(!status.success());
                break (&mut first, &first_anchor, &second_client_pid);
            }
            (Some(first_status), Some(second_status)) => {
                panic!("both wrappers exited: first={first_status}, second={second_status}")
            }
            (None, None) => thread::sleep(Duration::from_millis(10)),
        }
    };

    wait_for_lease_count(&paths, 1);
    let route = RouteStore::new(paths.routes_dir.clone())
        .load(&socket)
        .unwrap();
    assert_eq!(route.wrapper_pid, winner.id());
    assert_eq!(&route.anchor_root, winner_anchor);
    let loser_pid = fs::read_to_string(loser_pid_file).unwrap();
    assert!(!process_exists(loser_pid.trim()));

    kill(Pid::from_raw(winner.id() as i32), Signal::SIGTERM).unwrap();
    let _ = winner.wait().unwrap();
    wait_for_lease_count(&paths, 0);
    assert!(!LeaseSet::new(paths.leases_dir).has_live(&socket).unwrap());
}

#[test]
fn startup_sync_failure_notifies_but_keeps_the_client_until_its_normal_exit() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let anchor = env.root.path().join("anchor");
    let target = env.root.path().join("target");
    for repo in [&anchor, &target] {
        fs::create_dir_all(repo).unwrap();
        assert!(
            ProcessCommand::new("git")
                .args(["init", "--quiet"])
                .current_dir(repo)
                .status()
                .unwrap()
                .success()
        );
    }
    let anchor = anchor.canonicalize().unwrap();
    let target = target.canonicalize().unwrap();
    BindingStore::new(paths.bindings_file)
        .bind("w1", &target)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions": [{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"target","number":1,"focused":true,
        "worktree":{"checkout_path":target}
    }]}});

    let mut command = env.std_command();
    command
        .args(["herdr", "--anchor"])
        .arg(&anchor)
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_HERDR_SLEEP", "2")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_ZED_FAIL_ON", "--add")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut wrapper = command.spawn().unwrap();
    wait_for_log(&env, "notification show");

    assert!(wrapper.try_wait().unwrap().is_none());
    assert!(
        LeaseSet::new(paths.leases_dir.clone())
            .has_live(&socket)
            .unwrap()
    );
    let log = env.read_log();
    assert!(log.contains("zed\t--existing"), "{log}");
    assert!(log.contains("zed\t--add"), "{log}");
    assert_eq!(
        RouteStore::new(paths.routes_dir)
            .load(&socket)
            .unwrap()
            .anchor_root,
        anchor
    );

    let output = wrapper.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("startup synchronization failed"));
}

#[test]
fn post_readiness_initialization_failure_terminates_the_client() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let blocked_root = env.root.path().join("blocked-root");
    fs::write(&blocked_root, "not a directory").unwrap();
    let anchor = env.root.path().join("anchor");
    fs::create_dir_all(&anchor).unwrap();
    assert!(
        ProcessCommand::new("git")
            .args(["init", "--quiet"])
            .current_dir(&anchor)
            .status()
            .unwrap()
            .success()
    );
    let pid_file = env.root.path().join("failed-child.pid");
    let sessions = serde_json::json!({
        "sessions": [{"name":"zerdr","running":true,"socket_path":socket}]
    });

    env.command()
        .args(["herdr", "--anchor"])
        .arg(&anchor)
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_ROOT", &blocked_root)
        .env("ZERDR_TEST_HERDR_SLEEP", "10")
        .env("ZERDR_TEST_CHILD_PID_FILE", &pid_file)
        .env("ZERDR_TEST_SESSION_WAIT_FOR_PID", "1")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .assert()
        .failure();

    let pid = fs::read_to_string(pid_file).unwrap();
    assert!(
        !ProcessCommand::new("kill")
            .args(["-0", pid.trim()])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn readiness_timeout_terminates_the_spawned_herdr_client() {
    let env = TestEnv::new();
    let pid_file = env.root.path().join("child.pid");
    let anchor = env.root.path().join("anchor");
    fs::create_dir_all(&anchor).unwrap();
    assert!(
        ProcessCommand::new("git")
            .args(["init", "--quiet"])
            .current_dir(&anchor)
            .status()
            .unwrap()
            .success()
    );

    env.command()
        .args(["herdr", "--anchor"])
        .arg(&anchor)
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_HERDR_SLEEP", "5")
        .env("ZERDR_TEST_CHILD_PID_FILE", &pid_file)
        .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
        .env("ZERDR_READY_TIMEOUT_MS", "500")
        .assert()
        .failure()
        .stderr(predicates::str::contains("timed out"));

    let pid = fs::read_to_string(pid_file).unwrap();
    assert!(
        !ProcessCommand::new("kill")
            .args(["-0", pid.trim()])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn wrapper_propagates_the_herdr_client_exit_status() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let sessions = serde_json::json!({
        "sessions": [{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[]}});
    let anchor = env.root.path().join("anchor");
    fs::create_dir_all(&anchor).unwrap();
    assert!(
        ProcessCommand::new("git")
            .args(["init", "--quiet"])
            .current_dir(&anchor)
            .status()
            .unwrap()
            .success()
    );

    env.command()
        .args(["herdr", "--anchor"])
        .arg(&anchor)
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_HERDR_SLEEP", "0.2")
        .env("ZERDR_TEST_HERDR_EXIT", "7")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .code(7);
}

fn wait_for_log(env: &TestEnv, needle: &str) {
    for _ in 0..300 {
        if env.read_log().contains(needle) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for log entry {needle:?}");
}

fn wait_for_file(path: &std::path::Path) {
    for _ in 0..300 {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {}", path.display());
}

fn process_exists(pid: &str) -> bool {
    ProcessCommand::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn wait_for_lease_count(paths: &Paths, expected: usize) {
    for _ in 0..200 {
        let count = fs::read_dir(&paths.leases_dir)
            .into_iter()
            .flatten()
            .flatten()
            .flat_map(|entry| fs::read_dir(entry.path()).into_iter().flatten().flatten())
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("json")
            })
            .count();
        if count == expected {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {expected} lease files");
}
