mod support;

use std::fs;
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::Duration;

use zerdr::state::{LeaseSet, LifecycleGuard, Paths, RouteFocus, RouteStore, RouteStrategy};

use jsonc_parser::ParseOptions;
use jsonc_parser::cst::CstRootNode;
use support::{TestEnv, compatible_plugins_json};

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
fn setup_is_idempotent_and_installs_exact_plugin_and_tasks_without_config_changes() {
    let env = TestEnv::new();
    let herdr_config = env.root.path().join("herdr/config.toml");
    fs::create_dir_all(herdr_config.parent().unwrap()).unwrap();
    fs::write(&herdr_config, "# user-owned\n").unwrap();

    let first_output = env.command().arg("setup").assert().success();
    let stdout = String::from_utf8_lossy(&first_output.get_output().stdout);
    assert!(stdout.contains("prefix+z"), "{stdout}");
    assert!(stdout.contains("plugin_action"), "{stdout}");
    assert!(stdout.contains("zerdr.open-zed"), "{stdout}");
    assert_eq!(fs::read_to_string(&herdr_config).unwrap(), "# user-owned\n");

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
    let manifest_path = env.root.path().join("data/plugin-v1/herdr-plugin.toml");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    let parsed: toml::Value = toml::from_str(&manifest).unwrap();
    let executable = assert_cmd::cargo::cargo_bin!("zerdr").display().to_string();
    assert_eq!(
        parsed["events"],
        toml::Value::Array(vec![toml::Value::Table(toml::Table::from_iter([
            (
                "on".to_owned(),
                toml::Value::String("workspace.focused".to_owned()),
            ),
            (
                "command".to_owned(),
                toml::Value::Array(vec![
                    toml::Value::String(executable.clone()),
                    toml::Value::String("sync-from-herdr".to_owned()),
                ]),
            ),
        ]))])
    );
    assert_eq!(
        parsed["actions"],
        toml::Value::Array(vec![toml::Value::Table(toml::Table::from_iter([
            ("id".to_owned(), toml::Value::String("open-zed".to_owned()),),
            (
                "title".to_owned(),
                toml::Value::String("Open Zed".to_owned()),
            ),
            (
                "contexts".to_owned(),
                toml::Value::Array(vec![toml::Value::String("workspace".to_owned())]),
            ),
            (
                "command".to_owned(),
                toml::Value::Array(vec![
                    toml::Value::String(executable),
                    toml::Value::String("open-from-herdr".to_owned()),
                ]),
            ),
        ]))])
    );

    let second_output = env.command().arg("setup").assert().success();
    assert_eq!(fs::read_to_string(tasks_path).unwrap(), first);
    assert_eq!(fs::read_to_string(manifest_path).unwrap(), manifest);
    assert_eq!(
        second_output.get_output().stdout,
        first_output.get_output().stdout
    );
    assert_eq!(fs::read_to_string(herdr_config).unwrap(), "# user-owned\n");
}

#[test]
fn setup_upgrades_event_only_and_malformed_action_manifests() {
    for action in [
        "",
        r#"
[[actions]]
id = "open-zed"
title = "Wrong"
contexts = ["global"]
command = ["wrong", "command"]
"#,
    ] {
        let env = TestEnv::new();
        env.command().arg("setup").assert().success();
        let paths = Paths::for_test(env.root.path());
        let manifest_path = paths.plugin_dir.join("herdr-plugin.toml");
        let executable = assert_cmd::cargo::cargo_bin!("zerdr").display().to_string();
        let old_manifest = format!(
            r#"id = "zerdr"
name = "zerdr"
version = "0.1.0"
min_herdr_version = "0.8.0"
platforms = ["macos", "linux"]

[[events]]
on = "workspace.focused"
command = [{executable:?}, "sync-from-herdr"]
{action}"#
        );
        fs::write(&manifest_path, old_manifest).unwrap();
        let tasks_before = fs::read(&paths.zed_tasks_file).unwrap();
        let install_before = fs::read(&paths.install_state_file).unwrap();

        env.command().arg("setup").assert().success();

        let upgraded: toml::Value =
            toml::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        let actions = upgraded["actions"].as_array().unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0]["id"].as_str(), Some("open-zed"));
        assert_eq!(actions[0]["title"].as_str(), Some("Open Zed"));
        assert_eq!(
            actions[0]["contexts"].as_array().unwrap(),
            &[toml::Value::String("workspace".to_owned())]
        );
        assert_eq!(
            actions[0]["command"].as_array().unwrap(),
            &[
                toml::Value::String(executable),
                toml::Value::String("open-from-herdr".to_owned()),
            ]
        );
        assert_eq!(fs::read(&paths.zed_tasks_file).unwrap(), tasks_before);
        assert_eq!(fs::read(&paths.install_state_file).unwrap(), install_before);
    }
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
fn failed_plugin_link_restores_the_action_manifest_tasks_and_ownership_state() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
    let tasks_before = fs::read(&paths.zed_tasks_file).unwrap();
    let manifest_path = paths.plugin_dir.join("herdr-plugin.toml");
    let manifest_before = fs::read(&manifest_path).unwrap();
    assert!(String::from_utf8_lossy(&manifest_before).contains("open-from-herdr"));
    let state_before = fs::read(&paths.install_state_file).unwrap();

    env.command()
        .arg("setup")
        .env(
            "ZERDR_SETUP_EXECUTABLE",
            env.root.path().join("different/zerdr"),
        )
        .env("ZERDR_TEST_PLUGIN_LINK_FAIL", "1")
        .assert()
        .failure()
        .stderr(predicates::str::contains("fake plugin link failure"));

    assert_eq!(fs::read(&paths.zed_tasks_file).unwrap(), tasks_before);
    assert_eq!(fs::read(manifest_path).unwrap(), manifest_before);
    assert_eq!(fs::read(&paths.install_state_file).unwrap(), state_before);
}

#[test]
fn purge_does_not_break_a_live_one_shot_zed_lock() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    fs::write(&env.log, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let socket = env.root.path().join("default.sock");
    fs::write(&socket, "").unwrap();
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
    let sessions = serde_json::json!({
        "sessions":[{"name":"default","running":true,"socket_path":socket}]
    });
    let first_blocked = env.root.path().join("first-zed-blocked");
    let release_first = env.root.path().join("release-first-zed");
    let second_blocked = env.root.path().join("second-zed-lock-blocked");
    let second_called = env.root.path().join("second-zed-called");
    let action = || {
        let context = serde_json::json!({
            "workspace_id":"w1",
            "worktree":{"checkout_path":target.clone()}
        });
        let mut command = env.std_command();
        command
            .arg("open-from-herdr")
            .env("HERDR_PLUGIN_ACTION_ID", "open-zed")
            .env("HERDR_SOCKET_PATH", &socket)
            .env("HERDR_PLUGIN_CONTEXT_JSON", context.to_string())
            .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string());
        command
    };

    let mut first = action();
    first
        .env("ZERDR_TEST_ZED_BLOCK_MARKER", &first_blocked)
        .env("ZERDR_TEST_ZED_BLOCK_CONTINUE", &release_first);
    let mut first = first.spawn().unwrap();
    for _ in 0..300 {
        if first_blocked.exists() {
            break;
        }
        if let Some(status) = first.try_wait().unwrap() {
            panic!("first one-shot action exited before blocking: {status}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(first_blocked.exists());

    env.command()
        .args(["uninstall", "--purge"])
        .assert()
        .success();
    assert!(paths.zed_lock_file.exists());

    let mut second = action();
    second
        .env("ZERDR_TEST_ZED_LOCK_BLOCKED_MARKER", &second_blocked)
        .env("ZERDR_TEST_ZED_CALL_MARKER", &second_called);
    let mut second = second.spawn().unwrap();
    for _ in 0..300 {
        if second_blocked.exists() || second_called.exists() {
            break;
        }
        if let Some(status) = second.try_wait().unwrap() {
            panic!("second one-shot action exited before contending: {status}");
        }
        thread::sleep(Duration::from_millis(10));
    }
    let interleaved = second_called.exists();
    assert!(
        second_blocked.exists(),
        "second one-shot action did not observe the held Zed lock"
    );

    fs::write(&release_first, "go").unwrap();
    assert!(first.wait().unwrap().success());
    assert!(second.wait().unwrap().success());
    assert!(
        !interleaved,
        "purge allowed a second one-shot action to bypass the Zed lock"
    );
    assert!(second_called.exists());
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
        .stderr(predicates::str::contains("live bare `zerdr` wrapper"));

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
        "sessions":[{"name":"default","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[]}});
    let plugins = compatible_plugins_json();
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("live bare `zerdr` wrapper"));
    assert!(paths.install_state_file.exists());
}

#[test]
fn doctor_passes_installed_capabilities_and_prints_resolved_paths() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let plugins = compatible_plugins_json();

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
fn doctor_requires_the_exact_action_and_recommends_setup() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
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
    let event_only = serde_json::json!({
        "result":{"plugins":[{
            "plugin_id":"zerdr",
            "enabled":true,
            "events":[{
                "on":"workspace.focused",
                "command":[executable, "sync-from-herdr"]
            }]
        }]}
    });

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_PLUGINS_JSON", event_only.to_string())
        .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
        .assert()
        .failure()
        .stdout(predicates::str::contains("Open Zed action"))
        .stdout(predicates::str::contains("run `zerdr setup`"));
    let stdout = String::from_utf8_lossy(
        &env.command()
            .arg("doctor")
            .env("ZERDR_TEST_PLUGINS_JSON", event_only.to_string())
            .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
            .output()
            .unwrap()
            .stdout,
    )
    .into_owned();
    assert!(
        !stdout.contains("PASS one-shot Open Zed is available"),
        "{stdout}"
    );
}

#[test]
fn doctor_rejects_duplicate_zerdr_action_or_event_identities() {
    for duplicate in ["action", "event"] {
        let env = TestEnv::new();
        env.command().arg("setup").assert().success();
        let mut plugins = compatible_plugins_json();
        let plugin = &mut plugins["result"]["plugins"][0];
        if duplicate == "action" {
            plugin["actions"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "id":"open-zed",
                    "title":"Wrong",
                    "contexts":["global"],
                    "command":["wrong","command"]
                }));
        } else {
            plugin["events"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "on":"workspace.focused",
                    "command":["wrong","command"]
                }));
        }

        env.command()
            .arg("doctor")
            .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
            .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
            .assert()
            .failure()
            .stdout(predicates::str::contains("Open Zed action"));
    }
}

#[test]
fn doctor_rejects_duplicate_action_or_event_identities_in_the_materialized_manifest() {
    for duplicate in ["action", "event"] {
        let env = TestEnv::new();
        env.command().arg("setup").assert().success();
        let paths = Paths::for_test(env.root.path());
        let manifest_path = paths.plugin_dir.join("herdr-plugin.toml");
        let mut manifest = fs::read_to_string(&manifest_path).unwrap();
        if duplicate == "action" {
            manifest.push_str(
                r#"
[[actions]]
id = "open-zed"
title = "Wrong"
contexts = ["global"]
command = ["wrong", "command"]
"#,
            );
        } else {
            manifest.push_str(
                r#"
[[events]]
on = "workspace.focused"
command = ["wrong", "command"]
"#,
            );
        }
        fs::write(&manifest_path, manifest).unwrap();

        env.command()
            .arg("doctor")
            .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
            .assert()
            .failure()
            .stdout(predicates::str::contains(
                "generated Herdr manifest lacks the exact event or Open Zed action command",
            ));
    }
}

#[test]
fn doctor_matches_plugin_actions_and_events_semantically_without_order_dependence() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
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

[[actions]]
id = "unrelated"
title = "Unrelated"
contexts = ["global"]
command = ["other"]

[[actions]]
id = "open-zed"
title = "Open Zed"
contexts = ["workspace"]
command = [{executable:?}, "open-from-herdr"]

[[events]]
on = "other.event"
command = ["other"]

[[events]]
on = "workspace.focused"
command = [{executable:?}, "sync-from-herdr"]
"#
        ),
    )
    .unwrap();
    let plugins = serde_json::json!({
        "result":{"plugins":[
            {"plugin_id":"unrelated","enabled":false,"actions":[],"events":[]},
            {
                "plugin_id":"zerdr",
                "enabled":true,
                "actions":[
                    {"id":"unrelated","title":"Unrelated","contexts":["global"],"command":["other"]},
                    {"id":"open-zed","title":"Open Zed","contexts":["workspace"],"command":[executable.clone(),"open-from-herdr"]}
                ],
                "events":[
                    {"on":"other.event","command":["other"]},
                    {"on":"workspace.focused","command":[executable,"sync-from-herdr"]}
                ]
            }
        ]}
    });

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "PASS Herdr zerdr Open Zed action is registered",
        ));
}

#[test]
fn doctor_rejects_each_malformed_or_disabled_action_installation() {
    for mutation in [
        "id",
        "title",
        "contexts",
        "command",
        "executable",
        "disabled",
    ] {
        let env = TestEnv::new();
        env.command().arg("setup").assert().success();
        let mut plugins = compatible_plugins_json();
        let plugin = &mut plugins["result"]["plugins"][0];
        match mutation {
            "id" => plugin["actions"][0]["id"] = "wrong".into(),
            "title" => plugin["actions"][0]["title"] = "Wrong".into(),
            "contexts" => plugin["actions"][0]["contexts"] = serde_json::json!(["global"]),
            "command" => {
                plugin["actions"][0]["command"][1] = "wrong-command".into();
            }
            "executable" => plugin["actions"][0]["command"][0] = "wrong-executable".into(),
            "disabled" => plugin["enabled"] = false.into(),
            _ => unreachable!(),
        }

        env.command()
            .arg("doctor")
            .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
            .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
            .assert()
            .failure()
            .stdout(predicates::str::contains("Open Zed action"))
            .stdout(predicates::str::contains("run `zerdr setup`"));
    }
}

#[test]
fn doctor_treats_absent_session_or_wrapper_as_healthy_plugin_only_state() {
    for has_session in [false, true] {
        let env = TestEnv::new();
        env.command().arg("setup").assert().success();
        let socket = env.root.path().join("herdr.sock");
        let sessions = if has_session {
            fs::write(&socket, "").unwrap();
            serde_json::json!({
                "sessions":[{
                    "name":"default",
                    "running":true,
                    "socket_path":socket
                }]
            })
        } else {
            serde_json::json!({"sessions":[]})
        };

        let assert = env
            .command()
            .arg("doctor")
            .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
            .assert()
            .success()
            .stdout(predicates::str::contains(
                "PASS one-shot Open Zed is available",
            ));
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
        assert!(!stdout.contains("run bare `zerdr`"), "{stdout}");
    }
}

#[test]
fn doctor_validates_every_session_binding_and_preserves_legacy_bytes() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
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
    fs::create_dir_all(paths.bindings_file.parent().unwrap()).unwrap();
    fs::write(
        &paths.bindings_file,
        serde_json::to_vec(&serde_json::json!({
            "schema_version":2,
            "sessions":{
                "zerdr":{"shared":repo},
                "default":{"shared":env.root.path().join("missing")}
            }
        }))
        .unwrap(),
    )
    .unwrap();

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
        .assert()
        .failure()
        .stdout(predicates::str::contains("binding default/shared"));

    let legacy = serde_json::to_vec(&serde_json::json!({
        "schema_version":1,
        "session_name":"zerdr",
        "bindings":{"legacy":repo}
    }))
    .unwrap();
    fs::write(&paths.bindings_file, &legacy).unwrap();
    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
        .assert()
        .success();
    assert_eq!(fs::read(paths.bindings_file).unwrap(), legacy);
}

#[test]
fn doctor_rejects_corrupt_session_discovery_without_live_authority() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", r#"{"unexpected":[]}"#)
        .assert()
        .failure()
        .stdout(predicates::str::contains(
            "FAIL could not inspect Herdr session \"default\"",
        ));
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
        "sessions":[{"name":"default","running":true,"socket_path":socket}]
    });
    let plugins = compatible_plugins_json();

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
fn doctor_targets_an_explicit_named_session() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
    let socket = env.root.path().join("work.sock");
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
        .initialize_for("work", &socket, &anchor, std::process::id())
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir)
        .acquire_for("work", &socket, 99)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"work","running":true,"socket_path":socket}]
    });

    env.command()
        .args(["doctor", "--session", "work"])
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Herdr session \"work\" has one live wrapper",
        ));
}

#[test]
fn doctor_does_not_treat_another_sessions_wrapper_as_an_error() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
    let socket = env.root.path().join("work.sock");
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
        .initialize_for("work", &socket, &anchor, std::process::id())
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir)
        .acquire_for("work", &socket, 99)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"work","running":true,"socket_path":socket}]
    });

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Herdr session \"default\" is not running",
        ));
}

#[test]
fn doctor_reports_external_route_focus_and_platform_capability() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().success();
    let paths = Paths::for_test(env.root.path());
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    RouteStore::new(paths.routes_dir.clone())
        .initialize_strategy(
            &socket,
            RouteStrategy::External {
                focus: RouteFocus::Terminal,
            },
            std::process::id(),
        )
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir)
        .acquire(&socket, 99)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"default","running":true,"socket_path":socket}]
    });

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_PLATFORM", "macos")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .assert()
        .success()
        .stdout(predicates::str::contains("route mode: external"))
        .stdout(predicates::str::contains("focus policy: terminal"))
        .stdout(predicates::str::contains(
            "terminal focus restoration is supported on macOS",
        ));
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
    let plugins = compatible_plugins_json();

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .assert()
        .failure()
        .stdout(predicates::str::contains(
            "FAIL Herdr session \"default\" has live lease state",
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
        "sessions":[{"name":"default","running":true,"socket_path":live_socket}]
    });
    let plugins = compatible_plugins_json();

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
        "sessions":[{"name":"default","running":true,"socket_path":socket}]
    });
    let plugins = compatible_plugins_json();

    env.command()
        .arg("doctor")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_PLUGINS_JSON", plugins.to_string())
        .assert()
        .failure()
        .stdout(predicates::str::contains(
            "FAIL Herdr session \"default\" has 2 live wrappers",
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
        "sessions":[{"name":"default","running":true,"socket_path":socket}]
    });
    let plugins = compatible_plugins_json();

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
    let plugins = compatible_plugins_json();

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
    let plugins = compatible_plugins_json();

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
