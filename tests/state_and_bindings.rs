use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::symlink;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use tempfile::TempDir;
use zerdr::state::{
    BindingStore, LeaseSet, LifecycleGuard, Paths, RouteStore, SyncGuard, ThreadLeaseScan,
    ThreadLeaseSet, ThreadPaneMemory, thread_detach_active, thread_detach_clear, thread_detach_set,
};

fn git_repo() -> (TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(temp.path())
            .status()
            .unwrap()
            .success()
    );
    let nested = temp.path().join("nested/deep");
    fs::create_dir_all(&nested).unwrap();
    (temp, nested)
}

#[test]
fn nested_bind_path_is_persisted_as_the_canonical_git_root() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let store = BindingStore::new(paths.bindings_file.clone());
    let (repo, nested) = git_repo();

    let root = store.bind("zerdr", "w1", &nested).unwrap();
    assert_eq!(root, repo.path().canonicalize().unwrap());

    let loaded = store.load().unwrap();
    assert_eq!(loaded.schema_version, 2);
    assert_eq!(loaded.sessions["zerdr"]["w1"], root);
}

#[test]
fn lazy_binding_does_not_replace_an_existing_explicit_binding() {
    let state = tempfile::tempdir().unwrap();
    let store = BindingStore::new(Paths::for_test(state.path()).bindings_file);
    let (first_repo, first_nested) = git_repo();
    let first_root = store.bind("zerdr", "w1", &first_nested).unwrap();

    let resolved = store
        .bind_if_absent("zerdr", "w1", state.path().join("missing").as_path())
        .unwrap();

    assert_eq!(resolved, first_root);
    assert_eq!(resolved, first_repo.path().canonicalize().unwrap());
}

#[test]
fn symlinked_bind_path_resolves_to_the_canonical_checkout_root() {
    let state = tempfile::tempdir().unwrap();
    let store = BindingStore::new(Paths::for_test(state.path()).bindings_file);
    let (repo, _) = git_repo();
    let links = tempfile::tempdir().unwrap();
    let link = links.path().join("checkout-link");
    symlink(repo.path(), &link).unwrap();

    let resolved = store
        .bind("zerdr", "w1", &link.join("nested/deep"))
        .unwrap();

    assert_eq!(resolved, repo.path().canonicalize().unwrap());
}

#[test]
fn duplicate_roots_are_preserved_for_distinct_workspaces() {
    let state = tempfile::tempdir().unwrap();
    let store = BindingStore::new(Paths::for_test(state.path()).bindings_file);
    let (repo, _) = git_repo();

    store.bind("zerdr", "w1", repo.path()).unwrap();
    store.bind("zerdr", "w2", repo.path()).unwrap();

    let loaded = store.load().unwrap();
    assert_eq!(loaded.sessions["zerdr"].len(), 2);
    assert_eq!(
        loaded.sessions["zerdr"]["w1"],
        loaded.sessions["zerdr"]["w2"]
    );
}

#[test]
fn legacy_v1_loads_as_zerdr_session_without_rewriting_bytes() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    fs::create_dir_all(paths.bindings_file.parent().unwrap()).unwrap();
    let (first, _) = git_repo();
    let (second, _) = git_repo();
    let original = serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "session_name": "zerdr",
        "bindings": {
            "w1": first.path().canonicalize().unwrap(),
            "w2": second.path().canonicalize().unwrap(),
        },
    }))
    .unwrap();
    fs::write(&paths.bindings_file, &original).unwrap();

    let loaded = BindingStore::new(paths.bindings_file.clone())
        .load()
        .unwrap();

    assert_eq!(loaded.schema_version, 2);
    assert_eq!(loaded.sessions.len(), 1);
    assert_eq!(loaded.sessions["zerdr"].len(), 2);
    assert_eq!(fs::read(paths.bindings_file).unwrap(), original);
}

#[test]
fn no_op_unbind_migrates_valid_v1_state_to_v2() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    fs::create_dir_all(paths.bindings_file.parent().unwrap()).unwrap();
    let (repo, _) = git_repo();
    fs::write(
        &paths.bindings_file,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "session_name": "zerdr",
            "bindings": {"w1": repo.path().canonicalize().unwrap()},
        }))
        .unwrap(),
    )
    .unwrap();
    let store = BindingStore::new(paths.bindings_file.clone());

    assert!(!store.unbind("default", "missing").unwrap());

    let persisted: serde_json::Value =
        serde_json::from_slice(&fs::read(paths.bindings_file).unwrap()).unwrap();
    assert_eq!(persisted["schema_version"], 2);
    assert_eq!(
        persisted["sessions"]["zerdr"]["w1"],
        repo.path().canonicalize().unwrap().display().to_string()
    );
    assert!(persisted.get("session_name").is_none());
    assert!(persisted.get("bindings").is_none());
}

#[test]
fn identical_workspace_ids_are_isolated_by_session() {
    let state = tempfile::tempdir().unwrap();
    let store = BindingStore::new(Paths::for_test(state.path()).bindings_file);
    let (default_repo, _) = git_repo();
    let (zerdr_repo, _) = git_repo();

    store.bind("default", "w1", default_repo.path()).unwrap();
    store.bind("zerdr", "w1", zerdr_repo.path()).unwrap();

    assert_eq!(
        store.get("default", "w1").unwrap().unwrap(),
        default_repo.path().canonicalize().unwrap()
    );
    assert_eq!(
        store.get("zerdr", "w1").unwrap().unwrap(),
        zerdr_repo.path().canonicalize().unwrap()
    );
    assert!(store.unbind("default", "w1").unwrap());
    assert!(store.get("default", "w1").unwrap().is_none());
    assert!(store.get("zerdr", "w1").unwrap().is_some());
}

#[test]
fn invalid_or_unsupported_state_is_not_overwritten_by_bind_or_unbind() {
    let invalid_states = [
        b"{".as_slice(),
        br#"{"schema_version":99,"sessions":{}}"#,
        br#"{"schema_version":1,"session_name":"default","bindings":{}}"#,
        br#"{"schema_version":1,"session_name":"zerdr","bindings":{},"sessions":{}}"#,
        br#"{"schema_version":2,"sessions":{},"session_name":"zerdr","bindings":{}}"#,
    ];
    let (repo, _) = git_repo();

    for original in invalid_states {
        let state = tempfile::tempdir().unwrap();
        let paths = Paths::for_test(state.path());
        fs::create_dir_all(paths.bindings_file.parent().unwrap()).unwrap();
        fs::write(&paths.bindings_file, original).unwrap();
        let store = BindingStore::new(paths.bindings_file.clone());

        assert!(store.bind("zerdr", "w1", repo.path()).is_err());
        assert_eq!(fs::read(&paths.bindings_file).unwrap(), original);
        assert!(store.unbind("zerdr", "w1").is_err());
        assert_eq!(fs::read(&paths.bindings_file).unwrap(), original);
    }
}

#[test]
fn route_state_tracks_the_canonical_dynamic_anchor() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let socket = state.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let (first, _) = git_repo();
    let (second, _) = git_repo();
    let first = first.path().canonicalize().unwrap();
    let second = second.path().canonicalize().unwrap();
    let routes = RouteStore::new(paths.routes_dir);

    routes.initialize(&socket, &first, 41).unwrap();
    let initial = routes.load(&socket).unwrap();
    assert_eq!(initial.schema_version, 2);
    assert_eq!(initial.session_name, "default");
    assert_eq!(initial.socket_path, socket.canonicalize().unwrap());
    assert_eq!(initial.internal_anchor(), Some(first.as_path()));
    assert_eq!(initial.wrapper_pid, 41);

    let route_json: serde_json::Value =
        serde_json::from_slice(&fs::read(routes.path(&socket).unwrap()).unwrap()).unwrap();
    assert_eq!(
        route_json,
        serde_json::json!({
            "schema_version": 2,
            "session_name": "default",
            "socket_path": socket.canonicalize().unwrap(),
            "wrapper_pid": 41,
            "routing": {
                "mode": "internal",
                "anchor_root": first,
            },
        })
    );

    routes.promote(&socket, &second).unwrap();
    assert_eq!(
        routes.load(&socket).unwrap().internal_anchor(),
        Some(second.as_path())
    );
}

#[test]
fn v1_route_loads_as_internal_and_promotes_to_v2() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let socket = state.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let (first, _) = git_repo();
    let (second, _) = git_repo();
    let first = first.path().canonicalize().unwrap();
    let second = second.path().canonicalize().unwrap();
    let routes = RouteStore::new(paths.routes_dir);
    let route_path = routes.path(&socket).unwrap();
    fs::create_dir_all(route_path.parent().unwrap()).unwrap();
    fs::write(
        &route_path,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "session_name": "zerdr",
            "socket_path": socket.canonicalize().unwrap(),
            "anchor_root": first,
            "wrapper_pid": 79,
        }))
        .unwrap(),
    )
    .unwrap();

    let loaded = routes.load(&socket).unwrap();
    assert_eq!(loaded.schema_version, 1);
    assert_eq!(loaded.internal_anchor(), Some(first.as_path()));

    routes.promote(&socket, &second).unwrap();
    let promoted: serde_json::Value =
        serde_json::from_slice(&fs::read(route_path).unwrap()).unwrap();
    assert_eq!(promoted["schema_version"], 2);
    assert_eq!(promoted["routing"]["mode"], "internal");
    assert_eq!(
        promoted["routing"]["anchor_root"],
        second.display().to_string()
    );
    assert!(promoted.get("anchor_root").is_none());
}

#[test]
fn mixed_v1_v2_route_shape_is_rejected_without_overwrite() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let socket = state.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let (repo, _) = git_repo();
    let repo = repo.path().canonicalize().unwrap();
    let routes = RouteStore::new(paths.routes_dir);
    let route_path = routes.path(&socket).unwrap();
    fs::create_dir_all(route_path.parent().unwrap()).unwrap();
    let original = serde_json::to_vec(&serde_json::json!({
        "schema_version": 1,
        "session_name": "zerdr",
        "socket_path": socket.canonicalize().unwrap(),
        "anchor_root": repo,
        "wrapper_pid": 81,
        "routing": {
            "mode": "internal",
            "anchor_root": repo,
        },
    }))
    .unwrap();
    fs::write(&route_path, &original).unwrap();

    assert!(routes.load(&socket).is_err());
    assert_eq!(fs::read(route_path).unwrap(), original);
}

#[test]
fn one_of_two_locked_leases_keeps_the_socket_live() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let socket = state.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let leases = LeaseSet::new(paths.leases_dir);

    let first = leases.acquire(&socket, 101).unwrap();
    let second = leases.acquire(&socket, 102).unwrap();
    assert!(leases.has_live(&socket).unwrap());

    drop(first);
    assert!(leases.has_live(&socket).unwrap());
    drop(second);
    assert!(!leases.has_live(&socket).unwrap());
}

#[test]
fn concurrent_first_migration_and_multi_session_updates_preserve_every_workspace() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    fs::create_dir_all(paths.bindings_file.parent().unwrap()).unwrap();
    let (repo, _) = git_repo();
    fs::write(
        &paths.bindings_file,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "session_name": "zerdr",
            "bindings": {"legacy": repo.path().canonicalize().unwrap()},
        }))
        .unwrap(),
    )
    .unwrap();
    let store = BindingStore::new(paths.bindings_file);
    let mut threads = Vec::new();
    for index in 0..16 {
        let store = store.clone();
        let repo = repo.path().to_path_buf();
        threads.push(thread::spawn(move || {
            let session = if index % 2 == 0 { "zerdr" } else { "default" };
            store.bind(session, &format!("w{index}"), &repo).unwrap();
        }));
    }
    for thread in threads {
        thread.join().unwrap();
    }
    let loaded = store.load().unwrap();
    assert_eq!(loaded.schema_version, 2);
    assert_eq!(loaded.sessions["zerdr"].len(), 9);
    assert_eq!(loaded.sessions["default"].len(), 8);
    assert!(loaded.sessions["zerdr"].contains_key("legacy"));
}

#[test]
fn lifecycle_lock_serializes_admission_cleanup_and_purge() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let first = LifecycleGuard::acquire(&paths.lifecycle_lock_file).unwrap();
    let acquired = Arc::new(AtomicBool::new(false));
    let thread_acquired = Arc::clone(&acquired);
    let lock_path = paths.lifecycle_lock_file.clone();

    let waiter = thread::spawn(move || {
        let _second = LifecycleGuard::acquire(&lock_path).unwrap();
        thread_acquired.store(true, Ordering::SeqCst);
    });
    thread::sleep(Duration::from_millis(50));
    assert!(!acquired.load(Ordering::SeqCst));
    drop(first);
    waiter.join().unwrap();
    assert!(acquired.load(Ordering::SeqCst));
}

#[test]
fn sync_lock_serializes_all_callers_for_the_same_socket() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let socket = state.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let first = SyncGuard::acquire(&paths.sync_locks_dir, &socket).unwrap();
    let acquired = Arc::new(AtomicBool::new(false));
    let thread_acquired = Arc::clone(&acquired);
    let lock_root = paths.sync_locks_dir.clone();
    let thread_socket = socket.clone();

    let waiter = thread::spawn(move || {
        let _second = SyncGuard::acquire(&lock_root, &thread_socket).unwrap();
        thread_acquired.store(true, Ordering::SeqCst);
    });
    thread::sleep(Duration::from_millis(50));
    assert!(!acquired.load(Ordering::SeqCst));
    drop(first);
    waiter.join().unwrap();
    assert!(acquired.load(Ordering::SeqCst));
}

#[test]
fn another_socket_does_not_authorize_the_event_socket() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let first_socket = state.path().join("first.sock");
    let second_socket = state.path().join("second.sock");
    fs::write(&first_socket, "").unwrap();
    fs::write(&second_socket, "").unwrap();
    let leases = LeaseSet::new(paths.leases_dir);

    let _guard = leases.acquire(&first_socket, 101).unwrap();
    assert!(!leases.has_live(&second_socket).unwrap());
}

#[test]
fn thread_leases_are_per_pane_and_reject_a_live_duplicate() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let socket = state.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let leases = ThreadLeaseSet::new(paths.thread_leases_dir.clone());

    let first = leases.acquire("default", &socket, "w1:p1").unwrap();
    assert_eq!(
        leases.leased_panes("default", &socket).unwrap(),
        BTreeSet::from(["w1:p1".to_owned()])
    );

    assert!(leases.acquire("default", &socket, "w1:p1").is_err());
    let _second = leases.acquire("default", &socket, "w1:p2").unwrap();
    assert_eq!(
        leases.leased_panes("default", &socket).unwrap(),
        BTreeSet::from(["w1:p1".to_owned(), "w1:p2".to_owned()])
    );

    drop(first);
    assert_eq!(
        leases.leased_panes("default", &socket).unwrap(),
        BTreeSet::from(["w1:p2".to_owned()])
    );
    assert!(leases.acquire("default", &socket, "w1:p1").is_ok());
}

#[test]
fn thread_leases_treat_an_unlocked_record_as_stale() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let socket = state.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let leases = ThreadLeaseSet::new(paths.thread_leases_dir.clone());

    // Reproduce what a killed thread leaves behind: a valid record whose lock is free.
    let guard = leases.acquire("default", &socket, "w1:p1").unwrap();
    let path = guard.path().to_path_buf();
    let record = fs::read(&path).unwrap();
    drop(guard);
    fs::write(&path, &record).unwrap();

    assert_eq!(
        leases.leased_panes("default", &socket).unwrap(),
        BTreeSet::new()
    );
    assert!(leases.acquire("default", &socket, "w1:p1").is_ok());
}

#[test]
fn thread_leases_are_scoped_by_session_and_socket() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let first_socket = state.path().join("first.sock");
    let second_socket = state.path().join("second.sock");
    fs::write(&first_socket, "").unwrap();
    fs::write(&second_socket, "").unwrap();
    let leases = ThreadLeaseSet::new(paths.thread_leases_dir.clone());

    let _held = leases.acquire("default", &first_socket, "w1:p1").unwrap();

    let _other_session = leases.acquire("work", &first_socket, "w1:p1").unwrap();
    let _other_socket = leases.acquire("default", &second_socket, "w1:p1").unwrap();
    assert_eq!(
        leases.leased_panes("default", &second_socket).unwrap(),
        BTreeSet::from(["w1:p1".to_owned()])
    );
}

#[test]
fn thread_detach_flag_round_trips() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());

    assert!(!thread_detach_active(&paths));
    thread_detach_set(&paths).unwrap();
    assert!(thread_detach_active(&paths));
    thread_detach_set(&paths).unwrap();
    thread_detach_clear(&paths).unwrap();
    assert!(!thread_detach_active(&paths));
    thread_detach_clear(&paths).unwrap();
}

#[test]
fn lease_detach_marker_follows_the_guard() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let socket = state.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let leases = ThreadLeaseSet::new(paths.thread_leases_dir.clone());

    let guard = leases.acquire("default", &socket, "w1:p1").unwrap();
    let marker = guard.path().with_extension("detached");
    assert!(!marker.exists());

    guard.mark_detached().unwrap();
    assert!(marker.exists());
    guard.clear_detached().unwrap();
    assert!(!marker.exists());

    guard.mark_detached().unwrap();
    let lease_path = guard.path().to_path_buf();
    drop(guard);
    assert!(!marker.exists(), "drop removes the marker");
    assert!(!lease_path.exists(), "drop removes the lease");
}

#[test]
fn suspend_scan_counts_live_leases_and_their_markers() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let first_socket = state.path().join("first.sock");
    let second_socket = state.path().join("second.sock");
    fs::write(&first_socket, "").unwrap();
    fs::write(&second_socket, "").unwrap();
    let leases = ThreadLeaseSet::new(paths.thread_leases_dir.clone());

    assert_eq!(leases.scan_all().unwrap(), ThreadLeaseScan::default());

    // Two live leases across different sessions and sockets are all in scope.
    let first = leases.acquire("default", &first_socket, "w1:p1").unwrap();
    let second = leases.acquire("work", &second_socket, "w2:p1").unwrap();
    assert_eq!(
        leases.scan_all().unwrap(),
        ThreadLeaseScan {
            live: 2,
            detached: 0
        }
    );

    first.mark_detached().unwrap();
    assert_eq!(
        leases.scan_all().unwrap(),
        ThreadLeaseScan {
            live: 2,
            detached: 1
        }
    );

    second.mark_detached().unwrap();
    assert_eq!(
        leases.scan_all().unwrap(),
        ThreadLeaseScan {
            live: 2,
            detached: 2
        }
    );
}

#[test]
fn suspend_scan_removes_stale_records_and_orphan_markers() {
    let state = tempfile::tempdir().unwrap();
    let paths = Paths::for_test(state.path());
    let socket = state.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let leases = ThreadLeaseSet::new(paths.thread_leases_dir.clone());

    // Reproduce a SIGKILLed detached thread: a valid record whose lock is free,
    // plus its marker.
    let guard = leases.acquire("default", &socket, "w1:p1").unwrap();
    guard.mark_detached().unwrap();
    let lease_path = guard.path().to_path_buf();
    let marker_path = lease_path.with_extension("detached");
    let record = fs::read(&lease_path).unwrap();
    drop(guard);
    fs::write(&lease_path, &record).unwrap();
    fs::write(&marker_path, b"").unwrap();

    // An orphan marker without any lease record is also cleaned up.
    let orphan = lease_path.parent().unwrap().join("orphan.detached");
    fs::write(&orphan, b"").unwrap();

    assert_eq!(leases.scan_all().unwrap(), ThreadLeaseScan::default());
    assert!(!lease_path.exists());
    assert!(!marker_path.exists());
    assert!(!orphan.exists());
}

#[test]
fn thread_pane_memory_dedups_by_pane_and_orders_by_recency() {
    let state = tempfile::tempdir().unwrap();
    let socket = state.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let memory = ThreadPaneMemory::new(Paths::for_test(state.path()).thread_memory_dir);

    memory.record("default", &socket, "w1", "w1:p1").unwrap();
    thread::sleep(Duration::from_millis(2));
    memory.record("default", &socket, "w1", "w1:p2").unwrap();
    thread::sleep(Duration::from_millis(2));
    memory.record("default", &socket, "w1", "w1:p1").unwrap();

    let panes = memory.load("default", &socket);
    assert_eq!(panes.len(), 2, "{panes:?}");
    assert_eq!(panes[0].pane_id, "w1:p1", "refreshed record is most recent");
    assert_eq!(panes[1].pane_id, "w1:p2");

    memory
        .prune("default", &socket, &["w1:p1".to_owned()])
        .unwrap();
    let panes = memory.load("default", &socket);
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].pane_id, "w1:p2");
}

#[test]
fn thread_pane_memory_treats_foreign_content_as_empty() {
    let state = tempfile::tempdir().unwrap();
    let socket = state.path().join("herdr.sock");
    fs::write(&socket, "").unwrap();
    let paths = Paths::for_test(state.path());
    let memory = ThreadPaneMemory::new(paths.thread_memory_dir.clone());

    memory.record("default", &socket, "w1", "w1:p1").unwrap();
    let file = fs::read_dir(&paths.thread_memory_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .unwrap();
    for foreign in ["{", "[1, 2]", "{\"schema_version\": 99, \"panes\": []}"] {
        fs::write(&file, foreign).unwrap();
        assert!(memory.load("default", &socket).is_empty(), "{foreign}");
    }

    memory.record("default", &socket, "w1", "w1:p3").unwrap();
    let panes = memory.load("default", &socket);
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].pane_id, "w1:p3");
}
