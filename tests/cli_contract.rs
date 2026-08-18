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
        .stdout(predicate::str::contains("herdr"))
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
fn manual_commands_require_the_zed_terminal_environment() {
    for command in ["pick", "next", "previous", "sync", "bind", "unbind"] {
        cargo_bin_cmd!("zerdr")
            .arg(command)
            .env_remove("ZED_TERM")
            .env_remove("TERM_PROGRAM")
            .assert()
            .failure()
            .stderr(predicate::str::contains("Zed integrated terminal"));
    }
}

#[test]
fn herdr_requires_an_explicit_anchor() {
    cargo_bin_cmd!("zerdr")
        .arg("herdr")
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--anchor <ANCHOR>"));
}

#[test]
fn herdr_rejects_a_non_git_anchor_before_spawning_a_child() {
    let env = TestEnv::new();
    let anchor = env.root.path().join("not-a-repository");
    std::fs::create_dir_all(&anchor).unwrap();

    env.command()
        .args(["herdr", "--anchor"])
        .arg(&anchor)
        .env("ZED_TERM", "true")
        .env("TERM_PROGRAM", "zed")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not inside a Git checkout"));

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
