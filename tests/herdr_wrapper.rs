mod support;

use std::fs;
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::Duration;

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use support::TestEnv;
use zerdr::state::{BindingStore, LeaseSet, Paths, RouteStore};

fn anchor_repo(env: &TestEnv) -> std::path::PathBuf {
    let repo = env.root.path().join("anchor-repo");
    fs::create_dir_all(&repo).unwrap();
    assert!(
        ProcessCommand::new("git")
            .args(["init", "--quiet"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );
    repo.canonicalize().unwrap()
}

#[test]
fn launcher_requires_a_compatible_plugin_before_spawning_herdr() {
    let env = TestEnv::new();
    env.prepare_launcher();
    let anchor = anchor_repo(&env);

    env.command()
        .arg("--anchor")
        .arg(&anchor)
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
fn bare_launcher_attaches_the_default_herdr_session() {
    let env = TestEnv::new();
    env.prepare_launcher();
    let anchor = anchor_repo(&env);
    let socket = env.root.path().join("default.sock");
    fs::write(&socket, "").unwrap();
    let sessions = serde_json::json!({
        "sessions": [{"name":"default","running":true,"socket_path":socket}]
    });

    env.command()
        .current_dir(&anchor)
        .env("ZERDR_TEST_HERDR_SLEEP", "0.1")
        .env("ZERDR_READY_TIMEOUT_MS", "100")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .assert()
        .success();

    let log = env.read_log();
    assert!(log.lines().any(|line| line == "herdr\t"), "{log}");
    assert!(!log.contains("herdr\t--session zerdr\n"), "{log}");
}

#[test]
fn named_launcher_attaches_the_matching_herdr_session() {
    let env = TestEnv::new();
    env.prepare_launcher();
    let anchor = anchor_repo(&env);
    let socket = env.root.path().join("work.sock");
    fs::write(&socket, "").unwrap();
    let sessions = serde_json::json!({
        "sessions": [{"name":"work","running":true,"socket_path":socket}]
    });

    env.command()
        .args(["--session", "work"])
        .current_dir(&anchor)
        .env("ZERDR_TEST_HERDR_SLEEP", "0.1")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .assert()
        .success();

    let log = env.read_log();
    assert!(log.contains("herdr\t--session work\n"), "{log}");
    assert_eq!(
        RouteStore::new(Paths::for_test(env.root.path()).routes_dir)
            .load_for("work", &socket)
            .unwrap()
            .session_name,
        "work"
    );
}

#[test]
fn launcher_accepts_an_event_only_pre_action_installation() {
    let env = TestEnv::new();
    env.prepare_launcher();
    let paths = Paths::for_test(env.root.path());
    let executable = assert_cmd::cargo::cargo_bin!("zerdr").display().to_string();
    fs::write(
        paths.plugin_dir.join("herdr-plugin.toml"),
        format!(
            r#"id = "zerdr"
name = "zerdr"
version = "0.1.0"
min_herdr_version = "0.8.0"
platforms = ["macos", "linux"]

[[events]]
on = "workspace.focused"
command = [{executable:?}, "sync-from-herdr"]
"#
        ),
    )
    .unwrap();
    let plugins = serde_json::json!({
        "result":{"plugins":[{
            "plugin_id":"zerdr",
            "enabled":true,
            "events":[{
                "on":"workspace.focused",
                "command":[executable,"sync-from-herdr"]
            }]
        }]}
    });
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"default","running":true,"socket_path":socket}]
    });

    let anchor = anchor_repo(&env);
    env.command()
        .arg("--anchor")
        .arg(&anchor)
        .env("ZERDR_TEST_HERDR_SLEEP", "0.1")
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .assert()
        .success();

    assert!(env.read_log().lines().any(|line| line == "herdr\t"));
}

#[test]
fn launcher_rejects_duplicate_focus_event_identities_in_manifest_or_plugin_registry() {
    for source in ["manifest", "registry"] {
        let env = TestEnv::new();
        env.prepare_launcher();
        let paths = Paths::for_test(env.root.path());
        let anchor = anchor_repo(&env);
        let mut invocation = env.command();
        invocation.arg("--anchor").arg(&anchor);

        if source == "manifest" {
            let manifest_path = paths.plugin_dir.join("herdr-plugin.toml");
            let mut manifest = fs::read_to_string(&manifest_path).unwrap();
            manifest.push_str(
                r#"
[[events]]
on = "workspace.focused"
command = ["wrong", "sync-from-herdr"]
"#,
            );
            fs::write(manifest_path, manifest).unwrap();
        } else {
            let mut plugins = serde_json::from_str::<serde_json::Value>(
                &support::compatible_plugins_json().to_string(),
            )
            .unwrap();
            plugins["result"]["plugins"][0]["events"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "on":"workspace.focused",
                    "command":["wrong","sync-from-herdr"]
                }));
            invocation.env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string());
        }

        invocation
            .assert()
            .failure()
            .stderr(predicates::str::contains("run `zerdr setup`"));
        assert!(!env.read_log().contains("herdr\t--session zerdr"));
    }
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
    let anchor = anchor_repo(&invalid_install);
    invalid_install
        .command()
        .arg("--anchor")
        .arg(&anchor)
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
    let anchor = anchor_repo(&invalid_manifest);
    invalid_manifest
        .command()
        .arg("--anchor")
        .arg(&anchor)
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

    let anchor = anchor_repo(&env);
    env.command_for(&alternate)
        .arg("--anchor")
        .arg(&anchor)
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
fn bare_wrapper_routes_internally_regardless_of_terminal_environment() {
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
    let sessions = serde_json::json!({
        "sessions": [{"name":"default","running":true,"socket_path":socket}]
    });
    env.command()
        .current_dir(&repo)
        .env_remove("ZED_TERM")
        .env_remove("TERM_PROGRAM")
        .env("ZERDR_TEST_HERDR_SLEEP", "0.2")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env(
            "ZERDR_TEST_WORKSPACES_JSON",
            r#"{"result":{"workspaces":[]}}"#,
        )
        .assert()
        .success();
    assert_eq!(
        RouteStore::new(Paths::for_test(env.root.path()).routes_dir)
            .load(&socket)
            .unwrap()
            .internal_anchor(),
        Some(repo.as_path())
    );
}

#[test]
fn wrapper_syncs_the_initial_workspace_without_requiring_the_zed_task() {
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
        .bind("default", "w1", &repo)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions": [{"name":"default","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"repo","focused":true,
        "worktree":{"checkout_path":repo}
    }]}});

    env.command()
        .arg("--anchor")
        .arg(&repo)
        .env("ZERDR_TEST_HERDR_SLEEP", "0.2")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    let log = env.read_log();
    assert!(log.lines().any(|line| line == "herdr\t"), "{log}");
    assert!(log.contains("workspace list"), "{log}");
    assert_eq!(log.matches("zed\t--existing").count(), 1, "{log}");
    assert!(
        log.contains(&format!("zed\t--existing {}", repo.display())),
        "{log}"
    );
    assert!(
        log.contains(&format!("zed\t--add {}", repo.display())),
        "{log}"
    );
    assert_eq!(
        RouteStore::new(paths.routes_dir)
            .load(&socket)
            .unwrap()
            .internal_anchor(),
        Some(repo.as_path())
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
        .bind("default", "w1", &repo)
        .unwrap();
    let sessions = serde_json::json!({
        "ok": true,
        "result": {"sessions": [{"name": "default", "socket_path": socket}]}
    });
    let workspaces = serde_json::json!({
        "ok": true,
        "result": {"workspaces": [{
            "workspace_id": "w1", "label": "repo", "number": 1,
            "focused": true, "cwd": repo
        }]}
    });

    env.command()
        .arg("--anchor")
        .arg(&repo)
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_HERDR_SLEEP", "0.2")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    let log = env.read_log();
    assert!(log.lines().any(|line| line == "herdr\t"), "{log}");
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
        "sessions": [{"name":"default","running":true,"socket_path":socket}]
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
                .arg("--anchor")
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
fn wrappers_for_different_named_sessions_can_coexist() {
    let env = TestEnv::new();
    env.prepare_launcher();
    let paths = Paths::for_test(env.root.path());
    let first_socket = env.root.path().join("first.sock");
    let second_socket = env.root.path().join("second.sock");
    fs::write(&first_socket, "").unwrap();
    fs::write(&second_socket, "").unwrap();
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
    let sessions = serde_json::json!({"sessions":[
        {"name":"first","running":true,"socket_path":first_socket},
        {"name":"second","running":true,"socket_path":second_socket}
    ]});
    let spawn_wrapper = |session: &str, anchor: &std::path::Path| {
        let mut command = env.std_command();
        command
            .args(["--session", session, "--anchor"])
            .arg(anchor)
            .env("ZED_TERM", "true")
            .env("TERM_PROGRAM", "zed")
            .env("ZERDR_TEST_HERDR_SLEEP", "10")
            .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
            .env(
                "ZERDR_TEST_WORKSPACES_JSON",
                r#"{"result":{"workspaces":[]}}"#,
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command.spawn().unwrap()
    };

    let mut first = spawn_wrapper("first", &first_anchor);
    let mut second = spawn_wrapper("second", &second_anchor);
    wait_for_lease_count(&paths, 2);

    let routes = RouteStore::new(paths.routes_dir.clone());
    assert_eq!(
        routes
            .load_for("first", &first_socket)
            .unwrap()
            .internal_anchor(),
        Some(first_anchor.as_path())
    );
    assert_eq!(
        routes
            .load_for("second", &second_socket)
            .unwrap()
            .internal_anchor(),
        Some(second_anchor.as_path())
    );

    kill(Pid::from_raw(first.id() as i32), Signal::SIGTERM).unwrap();
    kill(Pid::from_raw(second.id() as i32), Signal::SIGTERM).unwrap();
    let _ = first.wait().unwrap();
    let _ = second.wait().unwrap();
    wait_for_lease_count(&paths, 0);
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
        .bind("default", "w1", &target)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions": [{"name":"default","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"target","number":1,"focused":true,
        "worktree":{"checkout_path":target}
    }]}});

    let mut command = env.std_command();
    command
        .arg("--anchor")
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
        "sessions": [{"name":"default","running":true,"socket_path":socket}]
    });

    env.command()
        .arg("--anchor")
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
        .arg("--anchor")
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
        "sessions": [{"name":"default","running":true,"socket_path":socket}]
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
        .arg("--anchor")
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

#[test]
fn agent_list_parses_the_live_agent_payload_shape() {
    let env = TestEnv::new();
    let agents = serde_json::json!({
        "id": "cli:agent:list",
        "result": {
            "type": "agent_list",
            "agents": [
                {
                    "agent": "pi",
                    "agent_status": "idle",
                    "cwd": "/Users/example/Dev/mog-app",
                    "focused": false,
                    "pane_id": "wM:p8",
                    "tab_id": "wM:t3",
                    "workspace_id": "wM",
                    "terminal_title": "\u{3c0} - mog-app",
                    "terminal_title_stripped": "\u{3c0} - mog-app"
                },
                {
                    "agent": "claude",
                    "agent_status": "working",
                    "pane_id": "w13:p1",
                    "workspace_id": "w13",
                    "terminal_title_stripped": "recipe fix"
                }
            ]
        }
    });
    let herdr = fake_herdr(
        &env,
        "herdr-agents",
        &[("ZERDR_TEST_AGENTS_JSON", agents.to_string())],
    );

    let parsed = herdr.agents_for("default").unwrap();

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].kind, "pi");
    assert_eq!(parsed[0].status, "idle");
    assert_eq!(parsed[0].pane_id, "wM:p8");
    assert_eq!(parsed[0].workspace_id, "wM");
    assert_eq!(parsed[0].title.as_deref(), Some("\u{3c0} - mog-app"));
    assert_eq!(parsed[1].kind, "claude");
    assert_eq!(parsed[1].status, "working");
    assert_eq!(parsed[1].pane_id, "w13:p1");
}

#[test]
fn agent_get_surfaces_a_failing_herdr_exit_as_a_process_error() {
    let env = TestEnv::new();
    let herdr = fake_herdr(
        &env,
        "herdr-get-fail",
        &[("ZERDR_TEST_AGENT_GET_EXIT", "1".to_owned())],
    );

    let error = herdr.agent_get_for("default", "wM:p8").unwrap_err();

    assert!(
        matches!(error, zerdr::error::Error::Process { status: 1, .. }),
        "{error:?}"
    );
}

#[test]
fn agent_get_parses_a_single_agent_payload() {
    let env = TestEnv::new();
    let agent = serde_json::json!({
        "result": {
            "type": "agent_info",
            "agent": {
                "agent": "pi",
                "agent_status": "blocked",
                "pane_id": "w0:p1",
                "workspace_id": "w0",
                "terminal_title_stripped": "review the diff"
            }
        }
    });
    let herdr = fake_herdr(
        &env,
        "herdr-get",
        &[("ZERDR_TEST_AGENT_GET_JSON", agent.to_string())],
    );

    let parsed = herdr.agent_get_for("default", "w0:p1").unwrap().unwrap();

    assert_eq!(parsed.kind, "pi");
    assert_eq!(parsed.status, "blocked");
    assert_eq!(parsed.pane_id, "w0:p1");
    assert_eq!(parsed.workspace_id, "w0");
    assert_eq!(parsed.title.as_deref(), Some("review the diff"));
}

#[test]
fn tab_and_workspace_creation_surface_the_root_pane() {
    let env = TestEnv::new();
    let tab = serde_json::json!({
        "result": {"tab": {"tab_id": "wM:t9"}, "root_pane": {"pane_id": "wM:p9"}}
    });
    let workspace = serde_json::json!({
        "result": {
            "workspace": {"workspace_id": "w7", "label": "mog-app"},
            "tab": {"tab_id": "w7:t1"},
            "root_pane": {"pane_id": "w7:p1"}
        }
    });
    let herdr = fake_herdr(
        &env,
        "herdr-create",
        &[
            ("ZERDR_TEST_TAB_CREATE_JSON", tab.to_string()),
            ("ZERDR_TEST_WORKSPACE_CREATE_JSON", workspace.to_string()),
        ],
    );

    let created_tab = herdr
        .tab_create_for("default", "wM", std::path::Path::new("/tmp"))
        .unwrap();
    assert_eq!(created_tab, "wM:p9");

    let created_workspace = herdr
        .workspace_create_for("default", std::path::Path::new("/tmp"), "mog-app")
        .unwrap();
    assert_eq!(created_workspace.workspace_id, "w7");
    assert_eq!(created_workspace.root_pane_id, "w7:p1");

    let log = env.read_log();
    assert!(
        log.contains("herdr\t--session default tab create --workspace wM --cwd /tmp --no-focus"),
        "{log}"
    );
    assert!(
        log.contains(
            "herdr\t--session default workspace create --cwd /tmp --label mog-app --no-focus"
        ),
        "{log}"
    );
}

fn fake_herdr(env: &TestEnv, name: &str, variables: &[(&str, String)]) -> zerdr::herdr::Herdr {
    env.baked_herdr(name, variables)
}
