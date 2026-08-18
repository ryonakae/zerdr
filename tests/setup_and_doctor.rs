mod support;

use std::fs;
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::Duration;

use zerdr::state::{LeaseSet, LifecycleGuard, Paths, RouteStore};

use jsonc_parser::ParseOptions;
use jsonc_parser::cst::CstRootNode;
use support::TestEnv;

const OWNED_LABELS: [&str; 5] = [
    "zerdr: Herdr",
    "zerdr: Pick Workspace",
    "zerdr: Next Workspace",
    "zerdr: Previous Workspace",
    "zerdr: Sync Workspace",
];

#[test]
fn remote_doctor_reports_all_markers_without_processes_locks_or_cleanup() {
    let env = TestEnv::new();
    let paths = Paths::for_test(env.root.path());
    let stale_route = paths.routes_dir.join("stale.json");
    let stale_lease = paths.leases_dir.join("scope/stale.json");
    fs::create_dir_all(stale_route.parent().unwrap()).unwrap();
    fs::create_dir_all(stale_lease.parent().unwrap()).unwrap();
    fs::write(&stale_route, b"stale route bytes").unwrap();
    fs::write(&stale_lease, b"stale lease bytes").unwrap();

    let assert = env
        .command()
        .arg("doctor")
        .env("WSL_INTEROP", "socket")
        .env("SSH_CLIENT", "client")
        .env("container", "podman")
        .env(
            "ZERDR_TEST_REMOTE_MARKERS",
            "/run/.containerenv,/.dockerenv",
        )
        .assert()
        .failure();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    let markers = "SSH_CLIENT, WSL_INTEROP, container, /.dockerenv, /run/.containerenv";
    assert!(stdout.contains(markers), "{stdout}");

    assert_eq!(env.read_log(), "");
    assert_eq!(fs::read(stale_route).unwrap(), b"stale route bytes");
    assert_eq!(fs::read(stale_lease).unwrap(), b"stale lease bytes");
    assert!(!paths.lifecycle_lock_file.exists());
}

fn wait_for_path(path: &std::path::Path) {
    for _ in 0..300 {
        if path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {}", path.display());
}

#[test]
fn setup_is_idempotent_and_installs_exact_owned_tasks_without_keymap_changes() {
    let env = TestEnv::new();

    env.command().arg("setup").assert().success();
    let tasks_path = env.root.path().join("zed/tasks.json");
    let first = fs::read_to_string(&tasks_path).unwrap();
    let root = CstRootNode::parse(&first, &ParseOptions::default()).unwrap();
    let values = root.array_value().unwrap().elements();
    assert_eq!(values.len(), 5);
    for label in OWNED_LABELS {
        assert!(first.contains(label), "{first}");
    }
    assert!(first.contains(r#""reveal_target": "center""#));
    assert!(first.contains(r#""ZERDR_TASK_MODE": "1""#));
    assert!(first.contains(r#""args": ["--mode", "internal", "--anchor", "$ZED_WORKTREE_ROOT"]"#));
    assert!(first.contains(r#""allow_concurrent_runs": true"#));
    assert!(first.contains(r#""use_new_terminal": true"#));
    assert!(first.contains(r#""hide": "never""#));
    assert!(!env.root.path().join("zed/keymap.json").exists());
    let manifest =
        fs::read_to_string(env.root.path().join("data/plugin-v1/herdr-plugin.toml")).unwrap();
    assert!(manifest.contains("workspace.focused"));
    assert!(manifest.contains("sync-from-herdr"));

    env.command().arg("setup").assert().success();
    assert_eq!(fs::read_to_string(tasks_path).unwrap(), first);
}

#[test]
fn setup_migrates_a_valid_four_task_install_to_five_tasks() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
    let mut tasks: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(&paths.zed_tasks_file).unwrap()).unwrap();
    tasks.retain(|task| task["label"] != "zerdr: Herdr");
    fs::write(
        &paths.zed_tasks_file,
        serde_json::to_vec_pretty(&tasks).unwrap(),
    )
    .unwrap();
    let mut install: serde_json::Value =
        serde_json::from_slice(&fs::read(&paths.install_state_file).unwrap()).unwrap();
    install["task_fingerprints"]
        .as_object_mut()
        .unwrap()
        .remove("zerdr: Herdr");
    fs::write(
        &paths.install_state_file,
        serde_json::to_vec_pretty(&install).unwrap(),
    )
    .unwrap();

    env.command().arg("setup").assert().success();

    let installed = fs::read_to_string(paths.zed_tasks_file).unwrap();
    for label in OWNED_LABELS {
        assert_eq!(installed.matches(label).count(), 1, "{installed}");
    }
}

#[test]
fn generated_task_command_executes_when_the_binary_path_contains_spaces() {
    let env = TestEnv::new();
    let installed_dir = env.root.path().join("installed bin");
    fs::create_dir_all(&installed_dir).unwrap();
    let installed = installed_dir.join("zerdr tool");
    fs::copy(assert_cmd::cargo::cargo_bin!("zerdr"), &installed).unwrap();

    env.command()
        .arg("setup")
        .env("ZERDR_SETUP_EXECUTABLE", &installed)
        .assert()
        .success();
    let tasks = fs::read_to_string(env.root.path().join("zed/tasks.json")).unwrap();
    let root = CstRootNode::parse(&tasks, &ParseOptions::default()).unwrap();
    let task = root
        .array_value()
        .unwrap()
        .elements()
        .iter()
        .map(|element| element.to_serde_value().unwrap())
        .find(|task| task["label"] == "zerdr: Pick Workspace")
        .unwrap();
    let command = task["command"].as_str().unwrap();
    let args = task["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        ProcessCommand::new("sh")
            .args(["-c", &format!("{command} {args} --help >/dev/null")])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn setup_and_uninstall_preserve_unrelated_jsonc_content() {
    let env = TestEnv::new();
    let tasks_path = env.root.path().join("zed/tasks.json");
    fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();
    fs::write(
        &tasks_path,
        "[\n  // keep this comment\n  { \"label\": \"user task\", \"command\": \"echo ok\" },\n]\n",
    )
    .unwrap();

    env.command().arg("setup").assert().success();
    let installed = fs::read_to_string(&tasks_path).unwrap();
    assert!(installed.contains("// keep this comment"));
    assert!(installed.contains("user task"));

    env.command().arg("uninstall").assert().success();
    let uninstalled = fs::read_to_string(&tasks_path).unwrap();
    assert!(uninstalled.contains("// keep this comment"));
    assert!(uninstalled.contains("user task"));
    for label in OWNED_LABELS {
        assert!(!uninstalled.contains(label), "{uninstalled}");
    }
}

#[test]
fn setup_refuses_a_foreign_owned_label_without_changing_original_bytes() {
    let env = TestEnv::new();
    let tasks_path = env.root.path().join("zed/tasks.json");
    fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();
    let original = "[\n  {\"label\":\"zerdr: Sync Workspace\",\"command\":\"danger\"}\n]\n";
    fs::write(&tasks_path, original).unwrap();

    env.command()
        .arg("setup")
        .assert()
        .failure()
        .stderr(predicates::str::contains("conflicting Zed task"));

    assert_eq!(fs::read_to_string(tasks_path).unwrap(), original);
    assert_eq!(env.read_log(), "");
}

#[test]
fn uninstall_preserves_an_owned_task_modified_after_setup() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let tasks_path = env.root.path().join("zed/tasks.json");
    let installed = fs::read_to_string(&tasks_path).unwrap();
    let modified = installed.replacen(r#""args": ["next"]"#, r#""args": ["hacked"]"#, 1);
    fs::write(&tasks_path, modified).unwrap();

    env.command().arg("uninstall").assert().success();
    let remaining = fs::read_to_string(tasks_path).unwrap();
    assert!(remaining.contains("zerdr: Next Workspace"));
    assert!(remaining.contains("hacked"));
    assert!(!remaining.contains("zerdr: Pick Workspace"));
}

#[test]
fn failed_setup_update_restores_previous_tasks_manifest_and_ownership_state() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
    let tasks_before = fs::read(&paths.zed_tasks_file).unwrap();
    let manifest_path = paths.plugin_dir.join("herdr-plugin.toml");
    let manifest_before = fs::read(&manifest_path).unwrap();
    let state_before = fs::read(&paths.install_state_file).unwrap();

    env.command()
        .arg("setup")
        .env(
            "ZERDR_SETUP_EXECUTABLE",
            env.root.path().join("different/zerdr"),
        )
        .env("ZERDR_TEST_FAIL_INSTALL_STATE_WRITE", "1")
        .assert()
        .failure()
        .stderr(predicates::str::contains("injected install-state"));

    assert_eq!(fs::read(&paths.zed_tasks_file).unwrap(), tasks_before);
    assert_eq!(fs::read(manifest_path).unwrap(), manifest_before);
    assert_eq!(fs::read(&paths.install_state_file).unwrap(), state_before);
    assert_eq!(env.read_log().matches("herdr\tplugin link").count(), 3);
}

#[test]
fn purge_refuses_to_change_installation_while_a_live_lease_exists() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();

    env.command()
        .args(["uninstall", "--purge"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("live `zerdr herdr` client"));

    assert!(paths.install_state_file.exists());
    assert!(paths.plugin_dir.exists());
    assert!(paths.zed_tasks_file.exists());
}

#[test]
fn doctor_waits_for_admission_and_preserves_the_new_live_route() {
    let env = TestEnv::new();
    env.prepare_launcher();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let stale_anchor = env.root.path().join("stale-anchor");
    let new_anchor = env.root.path().join("new-anchor");
    for anchor in [&stale_anchor, &new_anchor] {
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
    let stale_anchor = stale_anchor.canonicalize().unwrap();
    let new_anchor = new_anchor.canonicalize().unwrap();
    let routes = RouteStore::new(paths.routes_dir.clone());
    routes.initialize(&socket, &stale_anchor, 1).unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[]}});
    let plugins = serde_json::json!({
        "result":{"plugins":[{
            "plugin_id":"zerdr","enabled":true,
            "events":[{"on":"workspace.focused"}]
        }]}
    });
    let marker = env.root.path().join("admission-locked");
    let proceed = env.root.path().join("admission-continue");
    let mut wrapper_command = env.std_command();
    wrapper_command
        .args(["--mode", "internal", "--anchor"])
        .arg(&new_anchor)
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_HERDR_SLEEP", "2")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_ADMISSION_LOCK_MARKER", &marker)
        .env("ZERDR_TEST_ADMISSION_CONTINUE", &proceed)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let wrapper = wrapper_command.spawn().unwrap();
    wait_for_path(&marker);

    let mut doctor_command = env.std_command();
    doctor_command
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut doctor = doctor_command.spawn().unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(doctor.try_wait().unwrap().is_none());
    fs::write(&proceed, "go").unwrap();

    let doctor_output = doctor.wait_with_output().unwrap();
    assert!(doctor_output.status.success());
    assert!(
        String::from_utf8_lossy(&doctor_output.stdout)
            .contains(&format!("route anchor is valid: {}", new_anchor.display()))
    );
    assert_eq!(
        routes.load(&socket).unwrap().internal_anchor(),
        Some(new_anchor.as_path())
    );
    assert!(wrapper.wait_with_output().unwrap().status.success());
}

#[test]
fn purge_rechecks_live_leases_after_waiting_for_wrapper_admission() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
    let lifecycle = LifecycleGuard::acquire(&paths.lifecycle_lock_file).unwrap();
    let mut command = env.std_command();
    command
        .args(["uninstall", "--purge"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut purge = command.spawn().unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(purge.try_wait().unwrap().is_none());

    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let _lease = LeaseSet::new(paths.leases_dir)
        .acquire(&socket, 99)
        .unwrap();
    drop(lifecycle);

    let output = purge.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("live `zerdr herdr` client"));
    assert!(paths.install_state_file.exists());
}

#[test]
fn doctor_passes_installed_capabilities_and_prints_resolved_paths() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let plugins = serde_json::json!({
        "result":{"plugins":[{
            "plugin_id":"zerdr","enabled":true,
            "events":[{"on":"workspace.focused"}]
        }]}
    });

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "PASS Zed supports --existing and --add",
        ))
        .stdout(predicates::str::contains("state directory"));
}

#[test]
fn doctor_validates_the_live_wrapper_route_and_anchor() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
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
    let anchor = anchor.canonicalize().unwrap();
    RouteStore::new(paths.routes_dir.clone())
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let plugins = serde_json::json!({
        "result":{"plugins":[{
            "plugin_id":"zerdr","enabled":true,
            "events":[{"on":"workspace.focused"}]
        }]}
    });

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .assert()
        .success()
        .stdout(predicates::str::contains(format!(
            "route anchor is valid: {}",
            anchor.display()
        )));
}

#[test]
fn doctor_rejects_live_lease_state_when_session_discovery_fails() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
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
    RouteStore::new(paths.routes_dir)
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir)
        .acquire(&socket, 99)
        .unwrap();
    let plugins = serde_json::json!({
        "result":{"plugins":[{
            "plugin_id":"zerdr","enabled":true,
            "events":[{"on":"workspace.focused"}]
        }]}
    });

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .assert()
        .failure()
        .stdout(predicates::str::contains(
            "FAIL zerdr has live lease state but the Herdr session socket is unavailable",
        ));
}

#[test]
fn doctor_removes_a_stale_route_while_preserving_another_live_scope() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
    let live_socket = env.root.path().join("live.sock");
    let stale_socket = env.root.path().join("stale.sock");
    fs::write(&live_socket, "").unwrap();
    fs::write(&stale_socket, "").unwrap();
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
    let routes = RouteStore::new(paths.routes_dir.clone());
    routes
        .initialize(&live_socket, &anchor, std::process::id())
        .unwrap();
    routes
        .initialize(&stale_socket, &anchor, std::process::id())
        .unwrap();
    let live_route = routes.path(&live_socket).unwrap();
    let stale_route = routes.path(&stale_socket).unwrap();
    let _lease = LeaseSet::new(paths.leases_dir)
        .acquire(&live_socket, 99)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"zerdr","running":true,"socket_path":live_socket}]
    });
    let plugins = serde_json::json!({
        "result":{"plugins":[{
            "plugin_id":"zerdr","enabled":true,
            "events":[{"on":"workspace.focused"}]
        }]}
    });

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .assert()
        .success()
        .stdout(predicates::str::contains("removed 1 stale route"));

    assert!(live_route.exists());
    assert!(!stale_route.exists());
}

#[test]
fn doctor_rejects_multiple_live_wrappers() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
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
    RouteStore::new(paths.routes_dir)
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let leases = LeaseSet::new(paths.leases_dir);
    let _first = leases.acquire(&socket, 99).unwrap();
    let _second = leases.acquire(&socket, 100).unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let plugins = serde_json::json!({
        "result":{"plugins":[{
            "plugin_id":"zerdr","enabled":true,
            "events":[{"on":"workspace.focused"}]
        }]}
    });

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .assert()
        .failure()
        .stdout(predicates::str::contains(
            "FAIL zerdr session has 2 live wrappers",
        ));
}

#[test]
fn doctor_rejects_a_live_wrapper_without_route_state() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let _lease = LeaseSet::new(paths.leases_dir)
        .acquire(&socket, 99)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let plugins = serde_json::json!({
        "result":{"plugins":[{
            "plugin_id":"zerdr","enabled":true,
            "events":[{"on":"workspace.focused"}]
        }]}
    });

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .assert()
        .failure()
        .stdout(predicates::str::contains(
            "FAIL live wrapper route state is invalid",
        ));
}

#[test]
fn doctor_reports_and_removes_stale_lease_files() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
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
    let routes = RouteStore::new(paths.routes_dir.clone());
    routes
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let route_path = routes.path(&socket).unwrap();
    let malformed_route = paths.routes_dir.join("malformed.json");
    fs::write(&malformed_route, "{").unwrap();
    let lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    let scope = fs::read_dir(&paths.leases_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let live_path = fs::read_dir(&scope)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    fs::copy(live_path, scope.join("stale.json")).unwrap();
    drop(lease);
    let plugins = serde_json::json!({
        "result":{"plugins":[{
            "plugin_id":"zerdr","enabled":true,
            "events":[{"on":"workspace.focused"}]
        }]}
    });

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .assert()
        .success()
        .stdout(predicates::str::contains("removed 1 stale lease"))
        .stdout(predicates::str::contains("removed 2 stale route"));

    assert!(!scope.join("stale.json").exists());
    assert!(!route_path.exists());
    assert!(!malformed_route.exists());
}

#[test]
fn doctor_rejects_a_modified_owned_task_payload() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
    let installed = fs::read_to_string(&paths.zed_tasks_file).unwrap();
    fs::write(
        &paths.zed_tasks_file,
        installed.replacen(r#""args": ["sync"]"#, r#""args": ["unexpected"]"#, 1),
    )
    .unwrap();
    let plugins = serde_json::json!({
        "result":{"plugins":[{
            "plugin_id":"zerdr","enabled":true,
            "events":[{"on":"workspace.focused"}]
        }]}
    });

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .assert()
        .failure()
        .stdout(predicates::str::contains("task payload"));
}

#[test]
fn doctor_fails_when_zed_lacks_existing_capability() {
    let env = TestEnv::new();
    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_ZED_EXISTING", "0")
        .assert()
        .failure()
        .stdout(predicates::str::contains(
            "FAIL Zed does not expose --existing and --add",
        ));
}

#[test]
fn doctor_fails_when_zed_lacks_add_capability() {
    let env = TestEnv::new();
    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_ZED_ADD", "0")
        .assert()
        .failure()
        .stdout(predicates::str::contains(
            "FAIL Zed does not expose --existing and --add",
        ));
}
