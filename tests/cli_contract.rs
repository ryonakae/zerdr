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
        .stdout(predicate::str::contains("--session <SESSION>"))
        .stdout(predicate::str::contains("--mode <MODE>").not())
        .stdout(predicate::str::contains("--anchor <ANCHOR>"))
        .stdout(predicate::str::contains("--focus <FOCUS>").not())
        .stdout(predicate::str::contains("pick").not())
        .stdout(predicate::str::contains("previous").not())
        .stdout(predicate::str::contains("sync"))
        .stdout(predicate::str::contains("bind"))
        .stdout(predicate::str::contains("unbind"))
        .stdout(predicate::str::contains("setup"))
        .stdout(predicate::str::contains("uninstall"))
        .stdout(predicate::str::contains("doctor"))
        .stdout(predicate::str::contains("sync-from-herdr").not())
        .stdout(predicate::str::contains("open-from-herdr").not());
}

#[test]
fn removed_workspace_subcommands_fail_with_clap_usage() {
    for command in ["pick", "next", "previous"] {
        let env = TestEnv::new();
        env.command()
            .arg(command)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unrecognized subcommand"));
        assert_eq!(env.read_log(), "");
    }
}

#[test]
fn every_manual_command_exposes_the_session_targeting_option() {
    for command in ["sync", "bind", "unbind"] {
        cargo_bin_cmd!("zerdr")
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--session <SESSION>"));
    }
}

#[test]
fn setup_and_uninstall_reject_session_targeting() {
    for command in ["setup", "uninstall"] {
        cargo_bin_cmd!("zerdr")
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains("--session").not());

        for args in [
            vec![command, "--session", "work"],
            vec!["--session", "work", command],
        ] {
            let env = TestEnv::new();
            env.command().args(args).assert().failure();
            assert_eq!(env.read_log(), "");
        }
    }
}

#[test]
fn hidden_plugin_commands_reject_session_targeting() {
    for command in ["sync-from-herdr", "open-from-herdr"] {
        for args in [
            vec![command, "--session", "work"],
            vec!["--session", "work", command],
        ] {
            let env = TestEnv::new();
            env.command().args(args).assert().failure();
            assert_eq!(env.read_log(), "");
        }
    }
}

#[test]
fn manual_commands_dispatch_outside_the_zed_terminal_environment() {
    for command in ["sync", "bind", "unbind"] {
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
        "sync",
        "bind",
        "unbind",
        "setup",
        "uninstall",
        "doctor",
        "sync-from-herdr",
        "open-from-herdr",
    ];
    let options: [&[&str]; 1] = [&["--anchor", "."]];
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
fn removed_mode_and_focus_flags_fail_as_unknown_arguments() {
    for option in [
        ["--mode", "internal"],
        ["--mode", "external"],
        ["--focus", "zed"],
    ] {
        let env = TestEnv::new();
        env.command()
            .args(option)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unexpected argument"));
        assert_eq!(env.read_log(), "");
    }
}

#[test]
fn internal_launch_rejects_a_non_git_anchor_before_spawning_a_child() {
    let env = TestEnv::new();
    let anchor = env.root.path().join("not-a-repository");
    std::fs::create_dir_all(&anchor).unwrap();

    env.command()
        .arg("--anchor")
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
            .env(marker, "detected")
            .assert()
            .failure()
            .stderr(predicate::str::contains(marker));
        assert_eq!(env.read_log(), "");
    }
    for marker in ["/.dockerenv", "/run/.containerenv"] {
        let env = TestEnv::new();
        env.command()
            .env("ZERDR_TEST_REMOTE_MARKERS", marker)
            .assert()
            .failure()
            .stderr(predicate::str::contains(marker));
        assert_eq!(env.read_log(), "");
    }
    for command in [
        None,
        Some("sync"),
        Some("bind"),
        Some("unbind"),
        Some("setup"),
        Some("uninstall"),
        Some("sync-from-herdr"),
        Some("open-from-herdr"),
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
fn unknown_commands_fail_with_clap_usage() {
    cargo_bin_cmd!("zerdr")
        .arg("unknown")
        .assert()
        .failure()
        .stderr(predicate::str::contains("unrecognized subcommand"));
}

#[test]
fn thread_exposes_session_kind_and_create_options() {
    cargo_bin_cmd!("zerdr")
        .args(["thread", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--session <SESSION>"))
        .stdout(predicate::str::contains("--kind <KIND>"))
        .stdout(predicate::str::contains("--create"))
        .stdout(predicate::str::contains("--auto"))
        .stdout(predicate::str::contains("--enable"))
        .stdout(predicate::str::contains("--disable"))
        .stdout(predicate::str::contains("[TARGET]"));
    cargo_bin_cmd!("zerdr")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("thread"));
}

#[test]
fn thread_rejects_auto_start_options_alongside_an_explicit_target() {
    for extra in [vec!["--kind", "pi"], vec!["--create"]] {
        let env = TestEnv::new();
        let mut args = vec!["thread", "wM:p8"];
        args.extend(extra);
        env.command()
            .args(args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("cannot be used with"));
        assert_eq!(env.read_log(), "");
    }
}

#[test]
fn thread_mode_flags_are_mutually_exclusive_and_reject_attach_options() {
    let conflicting: [&[&str]; 7] = [
        &["thread", "--enable", "--disable"],
        &["thread", "--enable", "--auto"],
        &["thread", "--disable", "--auto"],
        &["thread", "--enable", "wM:p8"],
        &["thread", "--enable", "--session", "work"],
        &["thread", "--auto", "--kind", "pi"],
        &["thread", "--auto", "wM:p8"],
    ];
    for args in conflicting {
        let env = TestEnv::new();
        env.command()
            .args(args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("cannot be used with"));
        assert_eq!(env.read_log(), "");
    }
}

#[test]
fn thread_mode_flags_reject_the_root_session_option() {
    for flag in ["--enable", "--disable"] {
        let env = TestEnv::new();
        env.command()
            .args(["--session", "work", "thread", flag])
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "--session cannot be used when toggling thread auto mode",
            ));
        assert_eq!(env.read_log(), "");
    }
}

#[test]
fn thread_auto_reports_the_disabled_mode_without_touching_herdr() {
    for args in [
        vec!["thread", "--auto"],
        vec!["thread", "--auto", "--session", "work"],
    ] {
        let env = TestEnv::new();
        let assert = env.command().args(args).assert().success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        assert_eq!(stdout.lines().count(), 1, "{stdout}");
        assert!(stdout.contains("auto mode"), "{stdout}");
        assert!(stdout.contains("disabled"), "{stdout}");
        assert!(assert.get_output().stderr.is_empty());
        assert_eq!(env.read_log(), "");
    }
}

#[test]
fn thread_accepts_session_targeting_only_once() {
    let env = TestEnv::new();
    env.command()
        .args(["--session", "work", "thread", "--session", "work"])
        .assert()
        .failure();
    assert_eq!(env.read_log(), "");
}
