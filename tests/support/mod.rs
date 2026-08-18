#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;

pub struct TestEnv {
    pub root: TempDir,
    pub bin: PathBuf,
    pub log: PathBuf,
}

impl TestEnv {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let log = root.path().join("process.log");
        let herdr = r##"#!/bin/sh
printf 'herdr\t%s\n' "$*" >> "$ZERDR_TEST_LOG"
case "$*" in
  "--version")
    printf '%s\n' 'herdr 0.8.0 protocol 19'
    ;;
  "plugin list --plugin zerdr --json")
    if [ -n "$ZERDR_TEST_PLUGINS_JSON" ]; then
      printf '%s\n' "$ZERDR_TEST_PLUGINS_JSON"
    else
      printf '%s\n' '{"result":{"plugins":[]}}'
    fi
    ;;
  "session list --json")
    if [ "${ZERDR_TEST_SESSION_WAIT_FOR_PID:-0}" = "1" ]; then
      while [ ! -s "$ZERDR_TEST_CHILD_PID_FILE" ]; do sleep 0.01; done
    fi
    if [ -n "$ZERDR_TEST_SESSION_READY_FILE" ]; then
      : > "$ZERDR_TEST_SESSION_READY_FILE"
      while [ ! -e "$ZERDR_TEST_SESSION_RELEASE_FILE" ]; do sleep 0.01; done
    fi
    printf '%s\n' "$ZERDR_TEST_SESSIONS_JSON"
    ;;
  "--session zerdr workspace list")
    printf '%s\n' "$ZERDR_TEST_WORKSPACES_JSON"
    ;;
  "--session zerdr workspace focus "*)
    printf '%s\n' '{"ok":true,"result":{}}'
    ;;
  "--session zerdr api snapshot")
    printf '%s\n' "$ZERDR_TEST_SNAPSHOT_JSON"
    ;;
  "--session zerdr notification show "*)
    if [ "${ZERDR_TEST_NOTIFICATION_RESULT:-shown}" = "shown" ]; then
      printf '%s\n' '{"ok":true,"result":{"shown":true}}'
    else
      printf '%s\n' '{"ok":true,"result":{"shown":false,"reason":"no_foreground_client"}}'
    fi
    ;;
  "--session zerdr")
    if [ -n "$ZERDR_TEST_CHILD_PID_FILE" ]; then
      printf '%s\n' "$$" > "$ZERDR_TEST_CHILD_PID_FILE"
    fi
    if [ "${ZERDR_TEST_HERDR_EXIT:-0}" = "0" ]; then
      exec sleep "${ZERDR_TEST_HERDR_SLEEP:-0}"
    fi
    sleep "${ZERDR_TEST_HERDR_SLEEP:-0}"
    exit "$ZERDR_TEST_HERDR_EXIT"
    ;;
  "plugin link "*)
    if [ "${ZERDR_TEST_PLUGIN_LINK_FAIL:-0}" = "1" ]; then
      printf '%s\n' 'fake plugin link failure' >&2
      exit 19
    fi
    printf '%s\n' '{"ok":true,"result":{}}'
    ;;
  *)
    printf '%s\n' '{"ok":true,"result":{}}'
    ;;
esac
"##;
        let zed = r##"#!/bin/sh
printf 'zed\t%s\n' "$*" >> "$ZERDR_TEST_LOG"
if [ "$*" = "--help" ]; then
  printf '%s\n' 'The Zed CLI binary'
  if [ "${ZERDR_TEST_ZED_EXISTING:-1}" = "1" ]; then
    printf '%s\n' '  -e, --existing <PATH>'
  fi
  if [ "${ZERDR_TEST_ZED_ADD:-1}" = "1" ]; then
    printf '%s\n' '  -a, --add <PATH>'
  fi
  exit 0
fi
if [ -n "$ZERDR_TEST_ZED_FAIL_ON" ] && [ "$1" = "$ZERDR_TEST_ZED_FAIL_ON" ]; then
  printf '%s\n' "fake Zed $1 failure" >&2
  exit 17
fi
if [ "${ZERDR_TEST_ZED_FAIL:-0}" = "1" ]; then
  printf '%s\n' 'fake Zed failure' >&2
  exit 17
fi
"##;
        write_executable(&bin.join("herdr"), herdr);
        write_executable(&bin.join("zed"), zed);
        Self { root, bin, log }
    }

    pub fn command(&self) -> Command {
        let mut command = assert_cmd::cargo::cargo_bin_cmd!("zerdr");
        command
            .env("PATH", self.path())
            .env("ZERDR_TEST_ROOT", self.root.path())
            .env("ZERDR_TEST_LOG", &self.log)
            .env("ZERDR_HERDR_BIN", self.bin.join("herdr"))
            .env("ZERDR_ZED_BIN", self.bin.join("zed"));
        command
    }

    pub fn std_command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(assert_cmd::cargo::cargo_bin!("zerdr"));
        command
            .env("PATH", self.path())
            .env("ZERDR_TEST_ROOT", self.root.path())
            .env("ZERDR_TEST_LOG", &self.log)
            .env("ZERDR_HERDR_BIN", self.bin.join("herdr"))
            .env("ZERDR_ZED_BIN", self.bin.join("zed"));
        command
    }

    fn path(&self) -> std::ffi::OsString {
        std::env::join_paths(
            std::iter::once(self.bin.clone()).chain(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            )),
        )
        .unwrap()
    }

    pub fn read_log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}
