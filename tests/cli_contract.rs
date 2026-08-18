mod support;

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use support::TestEnv;

#[test]
fn help_lists_public_commands_and_hides_event_entry_point() {
    cargo_bin_cmd!("zerdr")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("  herdr").not())
        .stdout(predicate::str::contains("--mode <MODE>"))
        .stdout(predicate::str::contains("--anchor <ANCHOR>"))
        .stdout(predicate::str::contains("--focus <FOCUS>"))
        .stdout(predicate::str::contains("pick"))
        .stdout(predicate::str::contains("next"))
        .stdout(predicate::str::contains("previous"))
        .stdout(predicate::str::contains("sync"))
        .stdout(predicate::str::contains("bind"))
        .stdout(predicate::str::contains("unbind"))
        .stdout(predicate::str::contains("setup"))
        .stdout(predicate::str::contains("uninstall"))
        .stdout(predicate::str::contains("doctor"))
        .stdout(predicate::str::contains("sync-from-herdr").not());
}

#[test]
fn manual_commands_dispatch_outside_the_zed_terminal_environment() {
    for command in ["pick", "next", "previous", "sync", "bind", "unbind"] {
        let env = TestEnv::new();
        env.command()
            .arg(command)
            .env_remove("ZED_TERM")
            .env_remove("TERM_PROGRAM")
            .assert()
            .failure()
            .stderr(predicate::str::contains("Zed integrated terminal").not());
    }
}

#[test]
fn removed_herdr_subcommand_fails_with_clap_usage() {
    cargo_bin_cmd!("zerdr")
        .arg("herdr")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
}

#[test]
fn launch_options_are_rejected_with_every_subcommand_in_either_order() {
    let subcommands = [
        "pick",
        "next",
        "previous",
        "sync",
        "bind",
        "unbind",
        "setup",
        "uninstall",
        "doctor",
        "sync-from-herdr",
    ];
    let options: [&[&str]; 3] = [
        &["--mode", "external"],
        &["--anchor", "."],
        &["--focus", "zed"],
    ];
    for subcommand in subcommands {
        for option in options {
            let env = TestEnv::new();
            let mut before = option.to_vec();
            before.push(subcommand);
            env.command().args(before).assert().failure();
            let mut after = vec![subcommand];
            after.extend_from_slice(option);
            env.command().args(after).assert().failure();
            assert_eq!(env.read_log(), "");
        }
    }
}

#[test]
fn internal_launch_rejects_a_non_git_anchor_before_spawning_a_child() {
    let env = TestEnv::new();
    let anchor = env.root.path().join("not-a-repository");
    std::fs::create_dir_all(&anchor).unwrap();

    env.command()
        .args(["--mode", "internal", "--anchor"])
        .arg(&anchor)
        .assert()
        .failure()
        .stderr(predicate::str::contains("not inside a Git checkout"));

    assert_eq!(env.read_log(), "");
}

#[test]
fn every_remote_marker_and_runtime_command_is_rejected_before_any_process() {
    for marker in [
        "SSH_CONNECTION",
        "SSH_CLIENT",
        "SSH_TTY",
        "WSL_DISTRO_NAME",
        "WSL_INTEROP",
        "container",
        "REMOTE_CONTAINERS",
        "DEVCONTAINER",
        "CODESPACES",
    ] {
        let env = TestEnv::new();
        env.command()
            .args(["--mode", "external"])
            .env(marker, "detected")
            .assert()
            .failure()
            .stderr(predicate::str::contains(marker));
        assert_eq!(env.read_log(), "");
    }
    for marker in ["/.dockerenv", "/run/.containerenv"] {
        let env = TestEnv::new();
        env.command()
            .args(["--mode", "external"])
            .env("ZERDR_TEST_REMOTE_MARKERS", marker)
            .assert()
            .failure()
            .stderr(predicate::str::contains(marker));
        assert_eq!(env.read_log(), "");
    }
    for command in [
        None,
        Some("pick"),
        Some("next"),
        Some("previous"),
        Some("sync"),
        Some("bind"),
        Some("unbind"),
        Some("setup"),
        Some("uninstall"),
        Some("sync-from-herdr"),
    ] {
        let env = TestEnv::new();
        let mut invocation = env.command();
        if let Some(command) = command {
            invocation.arg(command);
        }
        invocation
            .env("SSH_CONNECTION", "client server")
            .assert()
            .failure()
            .stderr(predicate::str::contains("detected SSH_CONNECTION"));
        assert_eq!(env.read_log(), "");
    }
}

#[test]
fn external_anchor_and_linux_terminal_focus_fail_before_any_process() {
    let env = TestEnv::new();
    let anchor = env.root.path();

    env.command()
        .args(["--mode", "external", "--anchor"])
        .arg(anchor)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--anchor cannot be used with --mode external",
        ));
    env.command()
        .args(["--mode", "external", "--focus", "terminal"])
        .env("ZERDR_TEST_PLATFORM", "linux")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unsupported on Linux"));

    assert_eq!(env.read_log(), "");
}

#[test]
fn unknown_commands_fail_with_clap_usage() {
    cargo_bin_cmd!("zerdr")
        .arg("unknown")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
}
