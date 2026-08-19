mod support;

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::process::Command as ProcessCommand;
use std::thread;
use std::time::Duration;

use fs2::FileExt;

use predicates::prelude::*;
use support::TestEnv;
use zerdr::state::{
    BindingStore, LeaseGuard, LeaseSet, Paths, RouteFocus, RouteStore, RouteStrategy, SyncGuard,
};

fn git_repo(parent: &std::path::Path) -> std::path::PathBuf {
    let repo = parent.join("repo");
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

fn authorize(paths: &Paths, socket: &std::path::Path) -> LeaseGuard {
    let anchor = git_repo(&paths.state_dir.join("test-anchor"));
    RouteStore::new(paths.routes_dir.clone())
        .initialize(socket, &anchor, std::process::id())
        .unwrap();
    LeaseSet::new(paths.leases_dir.clone())
        .acquire(socket, 99)
        .unwrap()
}

fn assert_route_corruption_blocks_sync(
    corrupt: impl FnOnce(&Paths, &std::path::Path, &std::path::Path),
) {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    let routes = RouteStore::new(paths.routes_dir.clone());
    routes
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    let route_path = routes.path(&socket).unwrap();
    corrupt(&paths, &socket, &route_path);
    let corrupted = fs::read(&route_path).unwrap();

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .assert()
        .failure();

    let log = env.read_log();
    assert_eq!(log.matches("notification show").count(), 1, "{log}");
    assert!(!log.contains("workspace list"), "{log}");
    assert!(!log.contains("zed\t"), "{log}");
    assert_eq!(fs::read(route_path).unwrap(), corrupted);
}

#[test]
fn every_manual_command_rejects_a_route_owner_mismatch_before_workspace_or_state_changes() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    RouteStore::new(paths.routes_dir.clone())
        .initialize(&socket, &anchor, 424_242)
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    let sessions = serde_json::json!({
        "result":{"sessions":[{"name":"zerdr","socket_path":socket}]}
    });

    for args in [
        vec!["pick"],
        vec!["next"],
        vec!["previous"],
        vec!["sync"],
        vec!["bind", anchor.to_str().unwrap()],
        vec!["unbind"],
    ] {
        fs::write(&env.log, "").unwrap();
        env.command()
            .args(args)
            .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
            .assert()
            .failure()
            .stderr(predicate::str::contains("route belongs to wrapper"));
        let log = env.read_log();
        assert!(!log.contains("workspace list"), "{log}");
        assert!(!log.contains("workspace focus"), "{log}");
        assert!(!log.contains("zed\t"), "{log}");
    }
    assert!(!paths.bindings_file.exists());
}

#[test]
fn automatic_event_without_live_lease_is_a_successful_noop() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .assert()
        .success();

    assert_eq!(env.read_log(), "");
}

#[test]
fn malformed_route_notifies_without_workspace_or_zed_calls() {
    assert_route_corruption_blocks_sync(|_, _, route_path| {
        fs::write(route_path, "{").unwrap();
    });
}

#[test]
fn unsupported_route_schema_notifies_without_workspace_or_zed_calls() {
    assert_route_corruption_blocks_sync(|_, _, route_path| {
        let mut route: serde_json::Value =
            serde_json::from_slice(&fs::read(route_path).unwrap()).unwrap();
        route["schema_version"] = 99.into();
        fs::write(route_path, serde_json::to_vec_pretty(&route).unwrap()).unwrap();
    });
}

#[test]
fn missing_route_anchor_notifies_without_workspace_or_zed_calls() {
    assert_route_corruption_blocks_sync(|_, _, route_path| {
        let route: serde_json::Value =
            serde_json::from_slice(&fs::read(route_path).unwrap()).unwrap();
        fs::remove_dir_all(route["routing"]["anchor_root"].as_str().unwrap()).unwrap();
    });
}

#[test]
fn route_socket_mismatch_notifies_without_workspace_or_zed_calls() {
    assert_route_corruption_blocks_sync(|paths, _, route_path| {
        let other_socket = paths.state_dir.join("other.sock");
        fs::write(&other_socket, "").unwrap();
        let route: serde_json::Value =
            serde_json::from_slice(&fs::read(route_path).unwrap()).unwrap();
        RouteStore::new(paths.routes_dir.clone())
            .initialize(
                &other_socket,
                std::path::Path::new(route["routing"]["anchor_root"].as_str().unwrap()),
                std::process::id(),
            )
            .unwrap();
        let other_path = RouteStore::new(paths.routes_dir.clone())
            .path(&other_socket)
            .unwrap();
        fs::write(route_path, fs::read(other_path).unwrap()).unwrap();
    });
}

#[test]
fn route_owner_mismatch_notifies_without_calling_zed() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    RouteStore::new(paths.routes_dir)
        .initialize(&socket, &anchor, std::process::id() + 1)
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir)
        .acquire(&socket, 99)
        .unwrap();

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .assert()
        .failure();

    let log = env.read_log();
    assert_eq!(log.matches("notification show").count(), 1, "{log}");
    assert!(!log.contains("workspace list"), "{log}");
    assert!(!log.contains("zed\t"), "{log}");
}

#[test]
fn multiple_live_leases_notify_without_calling_zed() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    RouteStore::new(paths.routes_dir)
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let leases = LeaseSet::new(paths.leases_dir);
    let _first = leases.acquire(&socket, 99).unwrap();
    let _second = leases.acquire(&socket, 100).unwrap();

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .assert()
        .failure();

    let log = env.read_log();
    assert_eq!(log.matches("notification show").count(), 1, "{log}");
    assert!(!log.contains("workspace list"), "{log}");
    assert!(!log.contains("zed\t"), "{log}");
}

#[test]
fn external_terminal_focus_restores_after_zed_success_and_failure() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let target = git_repo(&env.root.path().join("target-parent"));
    RouteStore::new(paths.routes_dir.clone())
        .initialize_strategy(
            &socket,
            RouteStrategy::External {
                focus: RouteFocus::Terminal,
            },
            std::process::id(),
        )
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    BindingStore::new(paths.bindings_file)
        .bind("zerdr", "w1", &target)
        .unwrap();
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"target","focused":true,
        "worktree":{"checkout_path":target}
    }]}});

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_FOCUS_BACKEND", "1")
        .env("ZERDR_TEST_FRONTMOST_BEFORE", "com.mitchellh.ghostty")
        .env("ZERDR_TEST_FRONTMOST_AFTER", "dev.zed.Zed")
        .assert()
        .success();

    assert_eq!(
        env.read_log().lines().collect::<Vec<_>>(),
        vec![
            "herdr\t--session zerdr workspace list",
            "focus\tcapture com.mitchellh.ghostty",
            &format!("zed\t--existing {}", target.display()),
            "focus\tinspect dev.zed.Zed",
            "focus\tactivate com.mitchellh.ghostty",
        ]
    );

    fs::write(&env.log, "").unwrap();
    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_FOCUS_BACKEND", "1")
        .env("ZERDR_TEST_FRONTMOST_BEFORE", "com.mitchellh.ghostty")
        .env("ZERDR_TEST_FRONTMOST_AFTER", "com.apple.finder")
        .assert()
        .success();
    let switched_log = env.read_log();
    assert!(
        switched_log.contains("focus\tinspect com.apple.finder"),
        "{switched_log}"
    );
    assert!(!switched_log.contains("focus\tactivate"), "{switched_log}");

    fs::write(&env.log, "").unwrap();
    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_FOCUS_BACKEND", "1")
        .env("ZERDR_TEST_FRONTMOST_BEFORE", "com.mitchellh.ghostty")
        .env("ZERDR_TEST_FRONTMOST_AFTER", "dev.zed.Zed")
        .env("ZERDR_TEST_ZED_FAIL", "1")
        .assert()
        .failure()
        .stderr(predicate::str::contains("fake Zed failure"));
    let failed_log = env.read_log();
    assert!(
        failed_log.contains("focus\tactivate com.mitchellh.ghostty"),
        "{failed_log}"
    );
    assert_eq!(failed_log.matches("notification show").count(), 1);
}

#[test]
fn repeated_external_events_each_run_one_existing_call_without_mutating_route() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let target = git_repo(&env.root.path().join("target-parent"));
    let routes = RouteStore::new(paths.routes_dir.clone());
    routes
        .initialize_strategy(
            &socket,
            RouteStrategy::External {
                focus: RouteFocus::Zed,
            },
            std::process::id(),
        )
        .unwrap();
    let route_path = routes.path(&socket).unwrap();
    let original_route = fs::read(&route_path).unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    BindingStore::new(paths.bindings_file)
        .bind("zerdr", "w1", &target)
        .unwrap();
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"target","number":1,"focused":true,
        "worktree":{"checkout_path":target}
    }]}});

    for _ in 0..2 {
        env.command()
            .arg("sync-from-herdr")
            .env("HERDR_PLUGIN_EVENT", "workspace.focused")
            .env("HERDR_SOCKET_PATH", &socket)
            .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
            .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
            .env("ZERDR_TEST_FOCUS_BACKEND", "1")
            .env("ZERDR_TEST_FRONTMOST_BEFORE", "com.mitchellh.ghostty")
            .env("ZERDR_TEST_FRONTMOST_AFTER", "dev.zed.Zed")
            .assert()
            .success();
    }

    let zed_lines = env
        .read_log()
        .lines()
        .filter(|line| line.starts_with("zed\t"))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(
        zed_lines,
        vec![
            format!("zed\t--existing {}", target.display()),
            format!("zed\t--existing {}", target.display()),
        ]
    );
    assert!(!env.read_log().contains("focus\t"));
    assert_eq!(fs::read(route_path).unwrap(), original_route);
}

#[test]
fn live_v1_route_syncs_and_upgrades_only_after_successful_promotion() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    let target = git_repo(&env.root.path().join("target-parent"));
    let routes = RouteStore::new(paths.routes_dir.clone());
    let route_path = routes.path(&socket).unwrap();
    fs::create_dir_all(route_path.parent().unwrap()).unwrap();
    fs::write(
        &route_path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "session_name": "zerdr",
            "socket_path": socket.canonicalize().unwrap(),
            "anchor_root": anchor,
            "wrapper_pid": std::process::id(),
        }))
        .unwrap(),
    )
    .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    BindingStore::new(paths.bindings_file)
        .bind("zerdr", "w1", &target)
        .unwrap();
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"target","focused":true,
        "worktree":{"checkout_path":target}
    }]}});

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    let promoted: serde_json::Value =
        serde_json::from_slice(&fs::read(route_path).unwrap()).unwrap();
    assert_eq!(promoted["schema_version"], 2);
    assert_eq!(promoted["routing"]["mode"], "internal");
    assert_eq!(
        promoted["routing"]["anchor_root"],
        target.display().to_string()
    );
    let log = env.read_log();
    assert!(log.contains(&format!("zed\t--existing {}", anchor.display())));
    assert!(log.contains(&format!("zed\t--add {}", target.display())));
}

#[test]
fn automatic_event_routes_through_anchor_adds_target_and_promotes_it() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    let repo = git_repo(&env.root.path().join("target-parent"));
    RouteStore::new(paths.routes_dir.clone())
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let leases = LeaseSet::new(paths.leases_dir.clone());
    let _lease = leases.acquire(&socket, 99).unwrap();
    BindingStore::new(paths.bindings_file)
        .bind("zerdr", "w1", &repo)
        .unwrap();
    let workspaces = serde_json::json!({
        "ok": true,
        "result": {
            "workspaces": [
                {
                    "workspace_id": "w1",
                    "label": "repo",
                    "number": 1,
                    "focused": true,
                    "cwd": repo
                },
                {
                    "workspace_id": "w2",
                    "label": "unrelated",
                    "number": 2,
                    "focused": false,
                    "active_tab_id": "w2:t1"
                }
            ]
        }
    });

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success()
        .stderr(predicate::str::is_empty());

    let log = env.read_log();
    assert!(
        log.contains("herdr\t--session zerdr workspace list"),
        "{log}"
    );
    let existing = format!("zed\t--existing {}", anchor.display());
    let add = format!("zed\t--add {}", repo.display());
    assert_eq!(
        log.lines()
            .filter(|line| line.starts_with("zed\t"))
            .collect::<Vec<_>>(),
        vec![existing.as_str(), add.as_str()],
        "{log}"
    );
    assert_eq!(
        RouteStore::new(paths.routes_dir)
            .load(&socket)
            .unwrap()
            .internal_anchor(),
        Some(repo.as_path())
    );
    assert!(!log.contains("api snapshot"), "{log}");
    assert!(!log.contains("--new"), "{log}");
    assert!(!log.contains("--reuse"), "{log}");
}

#[test]
fn repeated_focus_of_current_anchor_still_runs_existing_then_add() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    let routes = RouteStore::new(paths.routes_dir.clone());
    routes
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    BindingStore::new(paths.bindings_file)
        .bind("zerdr", "w1", &anchor)
        .unwrap();
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"anchor","number":1,"focused":true,
        "worktree":{"checkout_path":anchor}
    }]}});

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    assert_eq!(
        env.read_log()
            .lines()
            .filter(|line| line.starts_with("zed\t"))
            .collect::<Vec<_>>(),
        vec![
            format!("zed\t--existing {}", anchor.display()),
            format!("zed\t--add {}", anchor.display()),
        ]
    );
    assert_eq!(
        routes.load(&socket).unwrap().internal_anchor(),
        Some(anchor.as_path())
    );
}

#[test]
fn existing_phase_failure_keeps_the_previous_anchor_and_skips_add() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    let target = git_repo(&env.root.path().join("target-parent"));
    let routes = RouteStore::new(paths.routes_dir.clone());
    routes
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    BindingStore::new(paths.bindings_file)
        .bind("zerdr", "w1", &target)
        .unwrap();
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"target","number":1,"focused":true,
        "worktree":{"checkout_path":target}
    }]}});

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_ZED_FAIL_ON", "--existing")
        .assert()
        .failure();

    let log = env.read_log();
    assert!(log.contains("zed\t--existing"), "{log}");
    assert!(!log.contains("zed\t--add"), "{log}");
    assert_eq!(log.matches("notification show").count(), 1, "{log}");
    assert_eq!(
        routes.load(&socket).unwrap().internal_anchor(),
        Some(anchor.as_path())
    );
}

#[test]
fn add_phase_failure_keeps_the_previous_anchor() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    let target = git_repo(&env.root.path().join("target-parent"));
    let routes = RouteStore::new(paths.routes_dir.clone());
    routes
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    BindingStore::new(paths.bindings_file)
        .bind("zerdr", "w1", &target)
        .unwrap();
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"target","number":1,"focused":true,
        "worktree":{"checkout_path":target}
    }]}});

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_ZED_FAIL_ON", "--add")
        .assert()
        .failure();

    let log = env.read_log();
    assert!(log.contains("zed\t--existing"), "{log}");
    assert!(log.contains("zed\t--add"), "{log}");
    assert_eq!(log.matches("notification show").count(), 1, "{log}");
    assert_eq!(
        routes.load(&socket).unwrap().internal_anchor(),
        Some(anchor.as_path())
    );
}

#[test]
fn route_write_failure_after_zed_success_preserves_the_previous_anchor() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    let target = git_repo(&env.root.path().join("target-parent"));
    let routes = RouteStore::new(paths.routes_dir.clone());
    routes
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    BindingStore::new(paths.bindings_file)
        .bind("zerdr", "w1", &target)
        .unwrap();
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"target","number":1,"focused":true,
        "worktree":{"checkout_path":target}
    }]}});

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_FAIL_ROUTE_WRITE", "1")
        .assert()
        .failure();

    let log = env.read_log();
    assert!(log.contains("zed\t--existing"), "{log}");
    assert!(log.contains("zed\t--add"), "{log}");
    assert_eq!(
        routes.load(&socket).unwrap().internal_anchor(),
        Some(anchor.as_path())
    );
}

#[test]
fn locked_malformed_lease_notifies_and_blocks_automatic_sync() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let _valid = authorize(&paths, &socket);
    let socket_scope = fs::read_dir(&paths.leases_dir)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let malformed_path = socket_scope.join("malformed.json");
    let mut malformed = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(&malformed_path)
        .unwrap();
    malformed.lock_exclusive().unwrap();
    malformed.write_all(b"{").unwrap();
    malformed.flush().unwrap();

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .assert()
        .failure()
        .stderr(predicate::str::contains("locked lease"));

    let log = env.read_log();
    assert!(log.contains("notification show"), "{log}");
    assert!(!log.contains("zed\t"), "{log}");
}

#[test]
fn queued_event_rechecks_lease_after_acquiring_sync_lock() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let lease = authorize(&paths, &socket);
    let lock = SyncGuard::acquire(&paths.sync_locks_dir, &socket).unwrap();
    let marker = env.root.path().join("sync-waiting");
    let mut command = env.std_command();
    command
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .env("ZERDR_TEST_SYNC_WAIT_MARKER", &marker);
    let mut child = command.spawn().unwrap();
    for _ in 0..100 {
        if marker.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists());
    assert!(child.try_wait().unwrap().is_none());

    drop(lease);
    drop(lock);
    assert!(child.wait().unwrap().success());
    assert!(!env.read_log().contains("zed\t"));
}

#[test]
fn automatic_event_resolves_an_unbound_workspace_from_snapshot_cwd() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let _lease = authorize(&paths, &socket);
    let repo = git_repo(env.root.path());
    let nested = repo.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let workspaces = serde_json::json!({
        "result": {"workspaces": [{
            "workspace_id":"w1","label":"repo","number":1,"focused":true,
            "active_tab_id":"w1:t1"
        }]}
    });
    let snapshot = serde_json::json!({
        "result": {"snapshot": {
            "layouts": [{"tab_id":"w1:t1","layout":{"focused_pane_id":"w1:p1"}}],
            "panes": [{"pane_id":"w1:p1","cwd":nested}]
        }}
    });

    env.command()
        .arg("sync-from-herdr")
        .env("HERDR_PLUGIN_EVENT", "workspace.focused")
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_PLUGIN_CONTEXT_JSON", r#"{"workspace_id":"w1"}"#)
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_SNAPSHOT_JSON", snapshot.to_string())
        .assert()
        .success();

    assert_eq!(
        BindingStore::new(paths.bindings_file)
            .load()
            .unwrap()
            .sessions["zerdr"]["w1"],
        repo
    );
    assert!(env.read_log().contains("api snapshot"));
}

#[test]
fn manual_previous_wraps_and_focuses_without_a_direct_zed_call() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let _lease = authorize(&paths, &socket);
    let first = git_repo(&env.root.path().join("first-parent"));
    let second = git_repo(&env.root.path().join("second-parent"));
    let store = BindingStore::new(paths.bindings_file);
    store.bind("zerdr", "w1", &first).unwrap();
    store.bind("zerdr", "w2", &second).unwrap();
    let sessions = serde_json::json!({
        "ok": true,
        "result": {"sessions": [{"name": "zerdr", "socket_path": socket}]}
    });
    let workspaces = serde_json::json!({
        "ok": true,
        "result": {"workspaces": [
            {"workspace_id":"w1","label":"first","number":1,"focused":true,"cwd":first},
            {"workspace_id":"w2","label":"second","number":2,"focused":false,"cwd":second}
        ]}
    });

    env.command()
        .arg("previous")
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    let log = env.read_log();
    assert!(
        log.contains("herdr\t--session zerdr workspace focus w2"),
        "{log}"
    );
    assert!(!log.contains("zed\t"), "{log}");
}

#[test]
fn manual_next_wraps_from_the_last_workspace_to_the_first() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let _lease = authorize(&paths, &socket);
    let first = git_repo(&env.root.path().join("first-parent"));
    let second = git_repo(&env.root.path().join("second-parent"));
    let store = BindingStore::new(paths.bindings_file);
    store.bind("zerdr", "w1", &first).unwrap();
    store.bind("zerdr", "w2", &second).unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[
        {"workspace_id":"w1","label":"first","number":1,"focused":false,"worktree":{"checkout_path":first}},
        {"workspace_id":"w2","label":"second","number":2,"focused":true,"worktree":{"checkout_path":second}}
    ]}});

    env.command()
        .arg("next")
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    let log = env.read_log();
    assert!(log.contains("workspace focus w1"), "{log}");
    assert!(!log.contains("zed\t"), "{log}");
}

#[test]
fn manual_bind_normalizes_nested_path_and_synchronizes() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let _lease = authorize(&paths, &socket);
    let repo = git_repo(env.root.path());
    let nested = repo.join("nested/deep");
    fs::create_dir_all(&nested).unwrap();
    let sessions = serde_json::json!({
        "ok": true,
        "result": {"sessions": [{"name": "zerdr", "socket_path": socket}]}
    });
    let workspaces = serde_json::json!({
        "ok": true,
        "result": {"workspaces": [{
            "workspace_id":"w1","label":"repo","number":1,"focused":true,"cwd":repo
        }]}
    });

    env.command()
        .args(["bind", nested.to_str().unwrap()])
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    let state = BindingStore::new(paths.bindings_file).load().unwrap();
    assert_eq!(state.sessions["zerdr"]["w1"], repo);
    assert!(
        env.read_log()
            .contains(&format!("zed\t--add {}", repo.display()))
    );
}

#[test]
fn picker_cancellation_changes_neither_herdr_nor_zed() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let _lease = authorize(&paths, &socket);
    let repo = git_repo(env.root.path());
    let sessions = serde_json::json!({
        "sessions": [{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({
        "result":{"workspaces":[{
            "workspace_id":"w1","label":"repo","number":1,"focused":true,
            "worktree":{"checkout_path":repo}
        }]}
    });

    env.command()
        .arg("pick")
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_PICK_INDEX", "cancel")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    let log = env.read_log();
    assert!(!log.contains("workspace focus"), "{log}");
    assert!(!log.contains("zed\t"), "{log}");
}

#[test]
fn notified_zed_task_failure_hides_by_returning_success() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let _lease = authorize(&paths, &socket);
    let repo = git_repo(env.root.path());
    BindingStore::new(paths.bindings_file)
        .bind("zerdr", "w1", &repo)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions": [{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({
        "result":{"workspaces":[{
            "workspace_id":"w1","label":"repo","number":1,"focused":true,
            "worktree":{"checkout_path":repo}
        }]}
    });

    env.command()
        .arg("sync")
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TASK_MODE", "1")
        .env("ZERDR_TEST_ZED_FAIL", "1")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    let log = env.read_log();
    assert!(log.contains("zed\t--existing"), "{log}");
    assert!(log.contains("notification show"), "{log}");
}

#[test]
fn duplicate_root_workspaces_remain_distinct_picker_targets() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let _lease = authorize(&paths, &socket);
    let repo = git_repo(env.root.path());
    let store = BindingStore::new(paths.bindings_file);
    store.bind("zerdr", "w1", &repo).unwrap();
    store.bind("zerdr", "w2", &repo).unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[
        {"workspace_id":"w1","label":"first","number":1,"focused":true,"worktree":{"checkout_path":repo}},
        {"workspace_id":"w2","label":"second","number":2,"focused":false,"worktree":{"checkout_path":repo}}
    ]}});

    env.command()
        .arg("pick")
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_PICK_INDEX", "1")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    let log = env.read_log();
    assert!(log.contains("workspace focus w2"), "{log}");
    assert!(!log.contains("zed\t"), "{log}");
}

#[test]
fn unbind_removes_only_the_focused_binding_without_calling_zed() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let _lease = authorize(&paths, &socket);
    let repo = git_repo(env.root.path());
    BindingStore::new(paths.bindings_file.clone())
        .bind("zerdr", "w1", &repo)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"repo","number":1,"focused":true,"worktree":{"checkout_path":repo}
    }]}});

    env.command()
        .arg("unbind")
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    assert!(
        BindingStore::new(paths.bindings_file)
            .get("zerdr", "w1")
            .unwrap()
            .is_none()
    );
    assert!(!env.read_log().contains("zed\t"));
}

#[test]
fn invalid_target_preflight_changes_neither_herdr_nor_zed() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let _lease = authorize(&paths, &socket);
    let repo = git_repo(env.root.path());
    BindingStore::new(paths.bindings_file.clone())
        .bind("zerdr", "w1", &repo)
        .unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[
        {"workspace_id":"w1","label":"valid","number":1,"focused":true,"worktree":{"checkout_path":repo}},
        {"workspace_id":"w2","label":"invalid","number":2,"focused":false}
    ]}});

    env.command()
        .arg("next")
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .failure();

    let log = env.read_log();
    assert!(!log.contains("workspace focus"), "{log}");
    assert!(!log.contains("zed\t"), "{log}");
    assert!(
        BindingStore::new(paths.bindings_file)
            .get("zerdr", "w2")
            .unwrap()
            .is_none()
    );
}

#[test]
fn picker_rechecks_lease_before_focusing_the_selected_workspace() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let lease = authorize(&paths, &socket);
    let first = git_repo(&env.root.path().join("first"));
    let second = git_repo(&env.root.path().join("second"));
    let store = BindingStore::new(paths.bindings_file.clone());
    store.bind("zerdr", "w1", &first).unwrap();
    let original_bindings = fs::read(&paths.bindings_file).unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[
        {"workspace_id":"w1","label":"first","number":1,"focused":true,"worktree":{"checkout_path":first}},
        {"workspace_id":"w2","label":"second","number":2,"focused":false,"worktree":{"checkout_path":second}}
    ]}});
    let ready = env.root.path().join("picker-ready");
    let proceed = env.root.path().join("picker-continue");
    let mut command = env.std_command();
    command
        .arg("pick")
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TEST_PICK_INDEX", "1")
        .env("ZERDR_TEST_PICK_READY", &ready)
        .env("ZERDR_TEST_PICK_CONTINUE", &proceed)
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string());
    let mut child = command.spawn().unwrap();
    for _ in 0..200 {
        if ready.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists());
    drop(lease);
    fs::write(proceed, "go").unwrap();

    assert!(!child.wait().unwrap().success());
    let log = env.read_log();
    assert!(!log.contains("workspace focus"), "{log}");
    assert!(!log.contains("zed\t"), "{log}");
    assert_eq!(fs::read(paths.bindings_file).unwrap(), original_bindings);
}

#[test]
fn bind_explicit_session_without_wrapper_targets_that_sessions_focus_and_skips_zed() {
    let env = TestEnv::new();
    let socket = env.root.path().join("default.sock");
    fs::write(&socket, "").unwrap();
    let repo = git_repo(&env.root.path().join("target-parent"));
    let nested = repo.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let sessions = serde_json::json!({
        "result":{"sessions":[{"name":"default","running":true,"socket_path":socket}]}
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w-default","label":"default","focused":true
    }]}});

    env.command()
        .args(["bind", "--session", "default"])
        .arg(&nested)
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_DEFAULT_JSON", workspaces.to_string())
        .assert()
        .success();

    assert_eq!(
        BindingStore::new(Paths::for_test(env.root.path()).bindings_file)
            .get("default", "w-default")
            .unwrap(),
        Some(repo)
    );
    let log = env.read_log();
    assert!(
        log.contains("herdr\t--session default workspace list"),
        "{log}"
    );
    assert!(!log.contains("zed\t"), "{log}");
}

#[test]
fn complete_pane_context_targets_the_injected_workspace_without_reading_focus() {
    let env = TestEnv::new();
    let socket = env.root.path().join("default.sock");
    fs::write(&socket, "").unwrap();
    let repo = git_repo(&env.root.path().join("target-parent"));
    let sessions = serde_json::json!({
        "result":{"sessions":[{"name":"default","running":true,"socket_path":socket}]}
    });

    env.command()
        .arg("bind")
        .arg(&repo)
        .env("HERDR_SOCKET_PATH", &socket)
        .env("HERDR_WORKSPACE_ID", "w-injected")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .assert()
        .success();

    assert_eq!(
        BindingStore::new(Paths::for_test(env.root.path()).bindings_file)
            .get("default", "w-injected")
            .unwrap(),
        Some(repo)
    );
    let log = env.read_log();
    assert!(!log.contains("workspace list"), "{log}");
    assert!(!log.contains("zed\t"), "{log}");
}

#[test]
fn partial_pane_context_fails_before_binding_or_zed() {
    for command in ["bind", "unbind"] {
        for (present, absent) in [
            ("HERDR_SOCKET_PATH", "HERDR_WORKSPACE_ID"),
            ("HERDR_WORKSPACE_ID", "HERDR_SOCKET_PATH"),
        ] {
            let env = TestEnv::new();
            let value = if present == "HERDR_SOCKET_PATH" {
                let socket = env.root.path().join("partial.sock");
                fs::write(&socket, "").unwrap();
                socket.display().to_string()
            } else {
                "w-partial".to_owned()
            };
            let mut invocation = env.command();
            invocation.arg(command);
            if command == "bind" {
                invocation.arg(env.root.path());
            }

            invocation
                .env(present, value)
                .env_remove(absent)
                .assert()
                .failure()
                .stderr(predicate::str::contains(
                    "HERDR_SOCKET_PATH and HERDR_WORKSPACE_ID must be set together",
                ));

            let paths = Paths::for_test(env.root.path());
            assert!(!paths.bindings_file.exists());
            assert!(!env.read_log().contains("zed\t"));
        }
    }
}

#[test]
fn explicit_session_wins_over_complete_pane_context() {
    let env = TestEnv::new();
    let default_socket = env.root.path().join("default.sock");
    let zerdr_socket = env.root.path().join("zerdr.sock");
    fs::write(&default_socket, "").unwrap();
    fs::write(&zerdr_socket, "").unwrap();
    let repo = git_repo(&env.root.path().join("target-parent"));
    let sessions = serde_json::json!({"result":{"sessions":[
        {"name":"default","running":true,"socket_path":default_socket},
        {"name":"zerdr","running":true,"socket_path":zerdr_socket}
    ]}});
    let default_workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w-default","label":"default","focused":true
    }]}});

    env.command()
        .args(["bind", "--session", "default"])
        .arg(&repo)
        .env("HERDR_SOCKET_PATH", &zerdr_socket)
        .env("HERDR_WORKSPACE_ID", "w-pane")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env(
            "ZERDR_TEST_WORKSPACES_DEFAULT_JSON",
            default_workspaces.to_string(),
        )
        .assert()
        .success();

    let store = BindingStore::new(Paths::for_test(env.root.path()).bindings_file);
    assert_eq!(store.get("default", "w-default").unwrap(), Some(repo));
    assert!(store.get("zerdr", "w-pane").unwrap().is_none());
    let log = env.read_log();
    assert!(log.contains("--session default workspace list"), "{log}");
    assert!(!log.contains("--session zerdr workspace list"), "{log}");
}

#[test]
fn unbind_explicit_session_without_wrapper_removes_only_that_mapping_and_skips_zed() {
    let env = TestEnv::new();
    let socket = env.root.path().join("default.sock");
    fs::write(&socket, "").unwrap();
    let default_repo = git_repo(&env.root.path().join("default-parent"));
    let zerdr_repo = git_repo(&env.root.path().join("zerdr-parent"));
    let paths = Paths::for_test(env.root.path());
    let store = BindingStore::new(paths.bindings_file.clone());
    store.bind("default", "w1", &default_repo).unwrap();
    store.bind("zerdr", "w1", &zerdr_repo).unwrap();
    let sessions = serde_json::json!({
        "result":{"sessions":[{"name":"default","running":true,"socket_path":socket}]}
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"default","focused":true
    }]}});

    env.command()
        .args(["unbind", "--session", "default"])
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_DEFAULT_JSON", workspaces.to_string())
        .assert()
        .success();

    assert!(store.get("default", "w1").unwrap().is_none());
    assert_eq!(store.get("zerdr", "w1").unwrap(), Some(zerdr_repo));
    assert!(!env.read_log().contains("zed\t"));
}

#[test]
fn bind_with_live_external_wrapper_persists_then_uses_the_existing_route() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let target = git_repo(&env.root.path().join("target-parent"));
    RouteStore::new(paths.routes_dir.clone())
        .initialize_strategy(
            &socket,
            RouteStrategy::External {
                focus: RouteFocus::Zed,
            },
            std::process::id(),
        )
        .unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    let sessions = serde_json::json!({
        "result":{"sessions":[{"name":"zerdr","running":true,"socket_path":socket}]}
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"target","focused":true
    }]}});

    env.command()
        .arg("bind")
        .arg(&target)
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    assert_eq!(
        BindingStore::new(paths.bindings_file)
            .get("zerdr", "w1")
            .unwrap(),
        Some(target.clone())
    );
    let log = env.read_log();
    assert!(
        log.contains(&format!("zed\t--existing {}", target.display())),
        "{log}"
    );
    assert!(!log.contains("zed\t--add"), "{log}");
    assert!(!log.contains("focus\t"), "{log}");
}

#[test]
fn multiple_live_wrappers_block_bind_before_binding_or_zed_changes() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let old = git_repo(&env.root.path().join("old-parent"));
    let target = git_repo(&env.root.path().join("target-parent"));
    let store = BindingStore::new(paths.bindings_file.clone());
    store.bind("zerdr", "w1", &old).unwrap();
    let original = fs::read(&paths.bindings_file).unwrap();
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    RouteStore::new(paths.routes_dir.clone())
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let leases = LeaseSet::new(paths.leases_dir.clone());
    let _first = leases.acquire(&socket, 99).unwrap();
    let _second = leases.acquire(&socket, 100).unwrap();
    let sessions = serde_json::json!({
        "result":{"sessions":[{"name":"zerdr","running":true,"socket_path":socket}]}
    });

    env.command()
        .arg("bind")
        .arg(&target)
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .assert()
        .failure()
        .stderr(predicate::str::contains("2 live wrappers"));

    assert_eq!(fs::read(paths.bindings_file).unwrap(), original);
    let log = env.read_log();
    assert!(!log.contains("workspace list"), "{log}");
    assert!(!log.contains("zed\t"), "{log}");
}

#[test]
fn malformed_live_route_blocks_bind_before_binding_or_zed_changes() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let old = git_repo(&env.root.path().join("old-parent"));
    let target = git_repo(&env.root.path().join("target-parent"));
    let store = BindingStore::new(paths.bindings_file.clone());
    store.bind("zerdr", "w1", &old).unwrap();
    let original = fs::read(&paths.bindings_file).unwrap();
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    let routes = RouteStore::new(paths.routes_dir.clone());
    routes
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    fs::write(routes.path(&socket).unwrap(), "{").unwrap();
    let _lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    let sessions = serde_json::json!({
        "result":{"sessions":[{"name":"zerdr","running":true,"socket_path":socket}]}
    });

    env.command()
        .arg("bind")
        .arg(&target)
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .assert()
        .failure();

    assert_eq!(fs::read(paths.bindings_file).unwrap(), original);
    let log = env.read_log();
    assert!(!log.contains("workspace list"), "{log}");
    assert!(!log.contains("zed\t"), "{log}");
}

#[test]
fn bind_rechecks_a_live_wrapper_after_workspace_resolution_before_routing() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let target = git_repo(&env.root.path().join("target-parent"));
    let anchor = git_repo(&env.root.path().join("anchor-parent"));
    RouteStore::new(paths.routes_dir.clone())
        .initialize(&socket, &anchor, std::process::id())
        .unwrap();
    let lease = LeaseSet::new(paths.leases_dir.clone())
        .acquire(&socket, 99)
        .unwrap();
    let sessions = serde_json::json!({
        "result":{"sessions":[{"name":"zerdr","running":true,"socket_path":socket}]}
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"target","focused":true
    }]}});
    let marker = env.root.path().join("workspace-list-ready");
    let proceed = env.root.path().join("workspace-list-continue");
    let mut command = env.std_command();
    command
        .arg("bind")
        .arg(&target)
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .env("ZERDR_TEST_WORKSPACE_LIST_MARKER", &marker)
        .env("ZERDR_TEST_WORKSPACE_LIST_CONTINUE", &proceed);
    let mut child = command.spawn().unwrap();
    for _ in 0..200 {
        if marker.exists() {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(marker.exists());
    drop(lease);
    fs::write(proceed, "go").unwrap();

    assert!(child.wait().unwrap().success());
    assert_eq!(
        BindingStore::new(paths.bindings_file)
            .get("zerdr", "w1")
            .unwrap(),
        Some(target)
    );
    let log = env.read_log();
    assert!(!log.contains("zed\t"), "{log}");
}

#[test]
fn follow_sync_uses_the_zerdr_binding_when_workspace_ids_overlap_sessions() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(env.root.path());
    let _lease = authorize(&paths, &socket);
    let default_root = git_repo(&env.root.path().join("default-parent"));
    let zerdr_root = git_repo(&env.root.path().join("zerdr-parent"));
    let store = BindingStore::new(paths.bindings_file);
    store.bind("default", "w1", &default_root).unwrap();
    store.bind("zerdr", "w1", &zerdr_root).unwrap();
    let sessions = serde_json::json!({
        "sessions":[{"name":"zerdr","running":true,"socket_path":socket}]
    });
    let workspaces = serde_json::json!({"result":{"workspaces":[{
        "workspace_id":"w1","label":"target","focused":true
    }]}});

    env.command()
        .arg("sync")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_WORKSPACES_JSON", workspaces.to_string())
        .assert()
        .success();

    let log = env.read_log();
    assert!(
        log.contains(&format!("zed\t--add {}", zerdr_root.display())),
        "{log}"
    );
    assert!(!log.contains(&default_root.display().to_string()), "{log}");
}

#[test]
fn zed_task_without_a_live_lease_leaves_actionable_terminal_error() {
    let env = TestEnv::new();
    let socket = env.root.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let sessions = serde_json::json!({
        "ok": true,
        "result": {"sessions": [{"name": "zerdr", "socket_path": socket}]}
    });

    env.command()
        .arg("sync")
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .env("ZERDR_TASK_MODE", "1")
        .env("ZERDR_TEST_SESSIONS_JSON", sessions.to_string())
        .env("ZERDR_TEST_NOTIFICATION_RESULT", "shown")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Start bare `zerdr`"));
}
