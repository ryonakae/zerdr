mod support;

use std::fs;
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::Duration;

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use support::TestEnv;
use zerdr::state::{BindingStore, LeaseSet, Paths, RouteFocus, RouteStore, RouteStrategy};

#[test]
fn launcher_requires_a_compatible_plugin_before_spawning_herdr() {
    let env = TestEnv::new();
    env.prepare_launcher();

    env.command()
        .args(["--mode", "external"])
        .env("ZERDR_TEST_PLUGINS_JSON", r#"{"result":{"plugins":[]}}"#)
        .assert()
        .failure()
        .stderr(predicates::str::contains("run `zerdr setup`"));

    let log = env.read_log();
    assert!(
        log.contains("herdr\tplugin list --plugin zerdr --json"),
        "{log}"
    );
    assert!(!log.contains("herdr\t--session zerdr"), "{log}");
    let paths = Paths::for_test(env.root.path());
    assert!(!paths.routes_dir.exists());
    assert!(!paths.leases_dir.exists());
}

#[test]
fn launcher_preflight_rejects_unsupported_install_state_and_manifest() {
    let invalid_install = TestEnv::new();
    invalid_install.prepare_launcher();
    let install_path = Paths::for_test(invalid_install.root.path()).install_state_file;
    let mut install: serde_json::Value =
        serde_json::from_slice(&fs::read(&install_path).unwrap()).unwrap();
    install["schema_version"] = 99.into();
    fs::write(&install_path, serde_json::to_vec(&install).unwrap()).unwrap();
    invalid_install
        .command()
        .args(["--mode", "external"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("run `zerdr setup`"));
    assert!(
        !invalid_install
            .read_log()
            .contains("herdr\t--session zerdr")
    );

    let invalid_manifest = TestEnv::new();
    invalid_manifest.prepare_launcher();
    let manifest_path = Paths::for_test(invalid_manifest.root.path())
        .plugin_dir
        .join("herdr-plugin.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap()
        .replace("sync-from-herdr", "wrong-event-command");
    fs::write(&manifest_path, manifest).unwrap();
    invalid_manifest
        .command()
        .args(["--mode", "external"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("run `zerdr setup`"));
    assert!(
        !invalid_manifest
            .read_log()
            .contains("herdr\t--session zerdr")
    );
}

#[test]
fn launcher_preflight_uses_the_running_executable_not_the_setup_override() {
    let env = TestEnv::new();
    env.prepare_launcher();
    let installed = assert_cmd::cargo::cargo_bin!("zerdr");
    let alternate = env.root.path().join("alternate-zerdr");
    fs::copy(installed, &alternate).unwrap();

    env.command_for(&alternate)
        .args(["--mode", "external"])
        .env("ZERDR_SETUP_EXECUTABLE", installed)
        .assert()
        .failure()
        .stderr(predicates::str::contains("run `zerdr setup`"));

    let log = env.read_log();
    assert!(
        log.contains("herdr\tplugin list --plugin zerdr --json"),
        "{log}"
    );
    assert!(!log.contains("herdr\t--session zerdr"), "{log}");
}

#[test]
fn auto_mode_selects_internal_in_zed_and_external_elsewhere() {
    let internal = TestEnv::new();
    internal.prepare_launcher();
    let internal_socket = internal.root.path().join("herdr.sock");
    fs::write(&internal_socket, "").unwrap();
    let repo = internal.root.path().join("repo");
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
    let internal_sessions = serde_json::json!({
        "sessions": [{"name":"zerdr","running":true,"socket_path":internal_socket}]
    });
    internal
        .command()
        .current_dir(&repo)
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_HERDR_SLEEP", "0.2")
        .env("ZERDR_TEST_SESSIONS_JSON", internal_sessions.to_string())
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .assert()
        .success();
    assert_eq!(
        RouteStore::new(Paths::for_test(internal.root.path()).routes_dir)
            .load(&internal_socket)
            .unwrap()
            .internal_anchor(),
        Some(repo.as_path())
    );

    let external = TestEnv::new();
    external.prepare_launcher();
    let external_socket = external.root.path().join("herdr.sock");
    fs::write(&external_socket, "").unwrap();
    let external_sessions = serde_json::json!({
        "sessions": [{"name":"zerdr","running":true,"socket_path":external_socket}]
    });
    external
        .command()
        .env_remove("ZED_TERM")
        .env_remove("TERM_PROGRAM")
        .env("ZERDR_TEST_PLATFORM", "linux")
        .env("ZERDR_TEST_HERDR_SLEEP", "0.2")
        .env("ZERDR_TEST_SESSIONS_JSON", external_sessions.to_string())
        .assert()
        .success();
    assert_eq!(
        RouteStore::new(Paths::for_test(external.root.path()).routes_dir)
            .load(&external_socket)
            .unwrap()
            .routing,
        RouteStrategy::External {
            focus: RouteFocus::Zed,
        }
    );
}

#[test]
fn external_wrapper_syncs_the_initial_workspace_without_requiring_the_zed_task() {
    let env = TestEnv::new();
    env.prepare_launcher();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    fs::remove_file(&paths.zed_tasks_file).unwrap();
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
    BindingStore::new(paths.bindings_file.clone())
        .bind("zerdr", "w1", &repo)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions": [{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"repo","focused":true,
        "worktree":{"checkout_path":repo}
    }]}});

    env.command()
        .args(["--mode", "external"])
        .env("ZERDR_TEST_PLATFORM", "macos")
        .env("ZERDR_TEST_HERDR_SLEEP", "0.2")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_FOCUS_BACKEND", "1")
        .env("ZERDR_TEST_FRONTMOST_BEFORE", "com.mitchellh.ghostty")
        .env("ZERDR_TEST_FRONTMOST_AFTER", "dev.zed.Zed")
        .assert()
        .success();

    let log = env.read_log();
    assert!(log.contains("herdr\t--session zerdr"), "{log}");
    assert!(log.contains("workspace list"), "{log}");
    assert_eq!(log.matches("zed\t--existing").count(), 1, "{log}");
    assert!(
        log.contains(&format!("zed\t--existing {}", repo.display())),
        "{log}"
    );
    assert!(!log.contains("zed\t--add"), "{log}");
    assert!(
        log.contains("focus\tactivate com.mitchellh.ghostty"),
        "{log}"
    );
    assert_eq!(
        RouteStore::new(paths.routes_dir)
            .load(&socket)
            .unwrap()
            .routing,
        RouteStrategy::External {
            focus: RouteFocus::Terminal,
        }
    );
    assert!(!LeaseSet::new(paths.leases_dir).has_live(&socket).unwrap());
}

#[test]
fn wrapper_holds_a_lease_runs_startup_sync_and_preserves_session() {
    let env = TestEnv::new();
    env.prepare_launcher();
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
        .bind("zerdr", "w1", &repo)
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
        .args(["--mode", "internal", "--anchor"])
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
    env.prepare_launcher();
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
                .args(["--mode", "internal", "--anchor"])
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
    assert_eq!(route.internal_anchor(), Some(winner_anchor.as_path()));
    let loser_pid = fs::read_to_string(loser_pid_file).unwrap();
    assert!(!process_exists(loser_pid.trim()));

    kill(Pid::from_raw(winner.id() as i32), Signal::SIGTERM).unwrap();
    let _ = winner.wait().unwrap();
    wait_for_lease_count(&paths, 0);
    assert!(!LeaseSet::new(paths.leases_dir).has_live(&socket).unwrap());
}

#[test]
fn external_startup_sync_failure_notifies_without_terminating_the_client() {
    let env = TestEnv::new();
    env.prepare_launcher();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let target = env.root.path().join("target");
    fs::create_dir_all(&target).unwrap();
    assert!(
        ProcessCommand::new("git")
            .args(["init", "--quiet"])
            .current_dir(&target)
            .status()
            .unwrap()
            .success()
    );
    let target = target.canonicalize().unwrap();
    BindingStore::new(paths.bindings_file)
        .bind("zerdr", "w1", &target)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions": [{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"target","focused":true,
        "worktree":{"checkout_path":target}
    }]}});

    env.command()
        .args(["--mode", "external", "--focus", "zed"])
        .env("ZERDR_TEST_PLATFORM", "macos")
        .env("ZERDR_TEST_HERDR_SLEEP", "0.2")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_ZED_FAIL", "1")
        .assert()
        .success()
        .stderr(predicates::str::contains("startup synchronization failed"));

    let log = env.read_log();
    assert_eq!(log.matches("zed\t--existing").count(), 1, "{log}");
    assert_eq!(log.matches("notification show").count(), 1, "{log}");
}

#[test]
fn startup_sync_failure_notifies_but_keeps_the_client_until_its_normal_exit() {
    let env = TestEnv::new();
    env.prepare_launcher();
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
        .bind("zerdr", "w1", &target)
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
        .args(["--mode", "internal", "--anchor"])
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
            .internal_anchor(),
        Some(anchor.as_path())
    );

    let output = wrapper.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("startup synchronization failed"));
}

#[test]
fn post_readiness_initialization_failure_terminates_the_client() {
    let env = TestEnv::new();
    env.prepare_launcher();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
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
        .args(["--mode", "internal", "--anchor"])
        .arg(&anchor)
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_FAIL_ROUTE_INITIALIZE", "1")
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
    env.prepare_launcher();
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
        .args(["--mode", "internal", "--anchor"])
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
    env.prepare_launcher();
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
        .args(["--mode", "internal", "--anchor"])
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
