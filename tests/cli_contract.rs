mod support;

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use support::TestEnv;

/// Bare invocations print help-like output; clap routes it to stdout or stderr
/// depending on the trigger, so the contract is checked on the combined output.
fn assert_usage_lists(args: &[&str], expected: &[&str]) {
    let env = TestEnv::new();
    let assert = env.command().args(args).assert().failure();
    let output = assert.get_output();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for name in expected {
        assert!(
            combined.contains(name),
            "expected {args:?} output to mention {name:?}:\n{combined}"
        );
    }
    assert_eq!(env.read_log(), "");
}

#[test]
fn help_lists_public_commands_and_hides_plugin_entry_points() {
    cargo_bin_cmd!("zerdr")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("connect"))
        .stdout(predicate::str::contains("start"))
        .stdout(predicate::str::contains("detach"))
        .stdout(predicate::str::contains("attach"))
        .stdout(predicate::str::contains("workspace"))
        .stdout(predicate::str::contains("setup"))
        .stdout(predicate::str::contains("--session <SESSION>"))
        .stdout(predicate::str::contains("--anchor").not())
        .stdout(predicate::str::contains("  thread").not())
        .stdout(predicate::str::contains("  uninstall").not())
        .stdout(predicate::str::contains("  doctor").not())
        .stdout(predicate::str::contains("sync-from-herdr").not())
        .stdout(predicate::str::contains("open-from-herdr").not());
}

#[test]
fn bare_invocations_show_their_subcommands() {
    assert_usage_lists(
        &[],
        &["connect", "start", "detach", "attach", "workspace", "setup"],
    );
    assert_usage_lists(&["workspace"], &["bind", "unbind", "sync"]);
    assert_usage_lists(&["setup"], &["install", "uninstall", "doctor", "auto"]);
}

/// The auto toggle reads `enable`/`disable`; the old `on`/`off` spellings are gone
/// without aliases, so clap teaches the new values on a stale invocation.
#[test]
fn setup_auto_takes_enable_or_disable_not_on_off() {
    for state in ["on", "off"] {
        let env = TestEnv::new();
        env.command()
            .args(["setup", "auto", state])
            .assert()
            .code(2)
            .stderr(predicate::str::contains("possible values: enable, disable"));
    }
}

#[test]
fn old_top_level_spellings_fail_with_clap_usage() {
    for command in ["thread", "sync", "bind", "unbind", "doctor", "uninstall"] {
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
fn setup_no_longer_installs_without_a_subcommand() {
    let env = TestEnv::new();
    env.command().arg("setup").assert().failure();
    assert_eq!(env.read_log(), "");
}

#[test]
fn session_commands_expose_the_global_session_option() {
    for args in [
        vec!["connect", "--help"],
        vec!["start", "--help"],
        vec!["workspace", "bind", "--help"],
        vec!["workspace", "unbind", "--help"],
        vec!["workspace", "sync", "--help"],
    ] {
        cargo_bin_cmd!("zerdr")
            .args(args)
            .assert()
            .success()
            .stdout(predicate::str::contains("--session <SESSION>"));
    }
}

#[test]
fn session_option_is_accepted_before_and_after_the_subcommand() {
    for args in [
        vec!["--session", "work", "connect"],
        vec!["connect", "--session", "work"],
    ] {
        let env = TestEnv::new();
        // The fake herdr serves an empty session list, so dispatch fails later
        // at runtime; the contract here is that clap accepted the flag.
        env.command()
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("unexpected argument").not())
            .stderr(predicate::str::contains("unrecognized subcommand").not());
    }
}

#[test]
fn session_option_is_accepted_only_once() {
    for args in [
        vec!["--session", "work", "connect", "--session", "work"],
        vec!["connect", "--session", "a", "--session", "b"],
        vec!["--session", "a", "workspace", "sync", "--session", "b"],
    ] {
        let env = TestEnv::new();
        env.command()
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "--session may be specified only once",
            ));
        assert_eq!(env.read_log(), "");
    }
}

#[test]
fn a_positional_behind_the_options_marker_is_not_counted_as_a_session_flag() {
    let env = TestEnv::new();
    // TARGET is literally "--session", escaped with `--`: one genuine flag.
    env.command()
        .args(["--session", "work", "connect", "--", "--session"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--session may be specified only once").not());
}

#[test]
fn sessionless_setup_commands_reject_session_targeting() {
    let commands: [&[&str]; 3] = [
        &["setup", "install"],
        &["setup", "uninstall"],
        &["setup", "auto", "enable"],
    ];
    for command in commands {
        for session_first in [true, false] {
            let env = TestEnv::new();
            let mut args: Vec<&str> = Vec::new();
            if session_first {
                args.extend_from_slice(&["--session", "work"]);
                args.extend_from_slice(command);
            } else {
                args.extend_from_slice(command);
                args.extend_from_slice(&["--session", "work"]);
            }
            env.command()
                .args(args)
                .assert()
                .failure()
                .stderr(predicate::str::contains(
                    "--session cannot be used with this command",
                ));
            assert_eq!(env.read_log(), "");
        }
    }
}

/// Detach mode is global by design; per-session scoping stays an explicit error
/// until it exists.
#[test]
fn detach_and_attach_reject_session_targeting() {
    let commands: [&[&str]; 2] = [&["detach"], &["attach"]];
    for command in commands {
        for session_first in [true, false] {
            let env = TestEnv::new();
            let mut args: Vec<&str> = Vec::new();
            if session_first {
                args.extend_from_slice(&["--session", "work"]);
                args.extend_from_slice(command);
            } else {
                args.extend_from_slice(command);
                args.extend_from_slice(&["--session", "work"]);
            }
            env.command()
                .args(args)
                .assert()
                .failure()
                .stderr(predicate::str::contains(
                    "--session cannot be used with this command",
                ));
            assert_eq!(env.read_log(), "");
        }
    }
}

/// Detach and attach only touch local state and processes, so they join
/// `setup doctor` as the commands that still run over SSH: that is exactly
/// where a phone-sized client needs them.
#[test]
fn detach_and_attach_run_under_remote_markers() {
    let env = TestEnv::new();
    env.command()
        .arg("detach")
        .env("SSH_CONNECTION", "client server")
        .assert()
        .success()
        .stderr(predicate::str::contains("detected").not());
    env.command()
        .arg("attach")
        .env("ZERDR_TEST_REMOTE_MARKERS", "/.dockerenv")
        .assert()
        .success()
        .stderr(predicate::str::contains("detected").not());
    assert_eq!(env.read_log(), "");
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
fn workspace_commands_dispatch_outside_the_zed_terminal_environment() {
    for command in ["sync", "bind", "unbind"] {
        let env = TestEnv::new();
        env.command()
            .args(["workspace", command])
            .env_remove("ZED_TERM")
            .env_remove("TERM_PROGRAM")
            .assert()
            .failure()
            .stderr(predicate::str::contains("Zed integrated terminal").not());
    }
}

#[test]
fn anchor_is_rejected_everywhere_but_start() {
    let commands: [&[&str]; 4] = [
        &[],
        &["connect"],
        &["workspace", "sync"],
        &["setup", "install"],
    ];
    for command in commands {
        let env = TestEnv::new();
        let mut args = command.to_vec();
        args.extend_from_slice(&["--anchor", "."]);
        env.command()
            .args(args)
            .assert()
            .code(2)
            .stderr(predicate::str::contains("unexpected argument"));
        assert_eq!(env.read_log(), "");
    }
}

#[test]
fn start_rejects_a_non_git_anchor_before_spawning_a_child() {
    let env = TestEnv::new();
    let anchor = env.root.path().join("not-a-repository");
    std::fs::create_dir_all(&anchor).unwrap();

    env.command()
        .arg("start")
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
            .arg("start")
            .env(marker, "detected")
            .assert()
            .failure()
            .stderr(predicate::str::contains(marker));
        assert_eq!(env.read_log(), "");
    }
    for marker in ["/.dockerenv", "/run/.containerenv"] {
        let env = TestEnv::new();
        env.command()
            .arg("start")
            .env("ZERDR_TEST_REMOTE_MARKERS", marker)
            .assert()
            .failure()
            .stderr(predicate::str::contains(marker));
        assert_eq!(env.read_log(), "");
    }
    let commands: [&[&str]; 10] = [
        &["connect"],
        &["start"],
        &["workspace", "sync"],
        &["workspace", "bind"],
        &["workspace", "unbind"],
        &["setup", "install"],
        &["setup", "uninstall"],
        &["setup", "auto", "enable"],
        &["sync-from-herdr"],
        &["open-from-herdr"],
    ];
    for command in commands {
        let env = TestEnv::new();
        env.command()
            .args(command)
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
fn connect_exposes_kind_and_hides_internal_options() {
    cargo_bin_cmd!("zerdr")
        .args(["connect", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--kind <KIND>"))
        .stdout(predicate::str::contains("--create").not())
        .stdout(predicate::str::contains("[TARGET]"))
        .stdout(predicate::str::contains("--auto").not())
        .stdout(predicate::str::contains("--enable").not())
        .stdout(predicate::str::contains("--disable").not());
}

#[test]
fn connect_rejects_the_removed_create_option_without_contacting_herdr() {
    let env = TestEnv::new();
    env.command()
        .args(["connect", "--create"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unexpected argument '--create'"));
    assert_eq!(env.read_log(), "");
}

#[test]
fn connect_rejects_kind_alongside_an_explicit_target() {
    let env = TestEnv::new();
    env.command()
        .args(["connect", "wM:p8", "--kind", "pi"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
    assert_eq!(env.read_log(), "");
}

#[test]
fn connect_auto_rejects_attach_options() {
    let conflicting: [&[&str]; 2] = [
        &["connect", "--auto", "--kind", "pi"],
        &["connect", "--auto", "wM:p8"],
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

/// With the mode disabled but the init command still installed, every new thread runs
/// `connect --auto`; a silent exit there reads as a bug, so the run explains itself and
/// still exits zero without touching Herdr.
#[test]
fn connect_auto_explains_itself_when_disabled_without_touching_herdr() {
    for args in [
        vec!["connect", "--auto"],
        vec!["connect", "--auto", "--session", "work"],
    ] {
        let env = TestEnv::new();
        let assert = env.command().args(args).assert().success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
        assert!(stdout.contains("auto mode is disabled"), "{stdout:?}");
        assert!(stdout.contains("zerdr connect"), "{stdout:?}");
        assert!(stdout.contains("zerdr setup auto enable"), "{stdout:?}");
        assert!(assert.get_output().stderr.is_empty());
        assert_eq!(env.read_log(), "");
    }
}

#[test]
fn setup_auto_requires_an_explicit_state() {
    let env = TestEnv::new();
    env.command().args(["setup", "auto"]).assert().code(2);
    assert_eq!(env.read_log(), "");

    let env = TestEnv::new();
    env.command()
        .args(["setup", "auto", "maybe"])
        .assert()
        .code(2);
    assert_eq!(env.read_log(), "");
}
