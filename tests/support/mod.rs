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

/// Dispatch body shared by the `PATH` fake `herdr` and the baked fakes built by
/// [`TestEnv::baked_herdr`]. Kept free of any per-invocation setup so the fake stays
/// as cheap as the timing-sensitive wrapper tests expect.
const FAKE_HERDR_BODY: &str = r##"printf 'herdr\t%s\n' "$*" >> "$ZERDR_TEST_LOG"
if [ "$1" = "--session" ] && [ "$3" = "server" ]; then
  if [ -n "$ZERDR_TEST_SESSIONS_STARTED_JSON" ] && [ -n "$ZERDR_TEST_SESSIONS_FILE" ]; then
    printf '%s\n' "$ZERDR_TEST_SESSIONS_STARTED_JSON" > "$ZERDR_TEST_SESSIONS_FILE"
  fi
  exec sleep "${ZERDR_TEST_SERVER_SLEEP:-5}"
fi
if [ "$1" = "--session" ] && [ "$3" = "workspace" ] && [ "$4" = "list" ]; then
  if [ -n "$ZERDR_TEST_WORKSPACE_LIST_MARKER" ]; then
    : > "$ZERDR_TEST_WORKSPACE_LIST_MARKER"
    while [ ! -e "$ZERDR_TEST_WORKSPACE_LIST_CONTINUE" ]; do sleep 0.01; done
  fi
  if [ -n "$ZERDR_TEST_WORKSPACES_FILE" ]; then
    while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line"; done < "$ZERDR_TEST_WORKSPACES_FILE"
  elif [ "$2" = "default" ] && [ -n "$ZERDR_TEST_WORKSPACES_DEFAULT_JSON" ]; then
    printf '%s\n' "$ZERDR_TEST_WORKSPACES_DEFAULT_JSON"
  elif [ "$2" = "zerdr" ] && [ -n "$ZERDR_TEST_WORKSPACES_ZERDR_JSON" ]; then
    printf '%s\n' "$ZERDR_TEST_WORKSPACES_ZERDR_JSON"
  else
    printf '%s\n' "$ZERDR_TEST_WORKSPACES_JSON"
  fi
  exit 0
fi
if [ "$1" = "--session" ] && [ "$3" = "workspace" ] && [ "$4" = "focus" ]; then
  printf '%s\n' '{"ok":true,"result":{}}'
  exit 0
fi
if [ "$1" = "--session" ] && [ "$3" = "api" ] && [ "$4" = "snapshot" ]; then
  printf '%s\n' "$ZERDR_TEST_SNAPSHOT_JSON"
  exit 0
fi
if [ "$1" = "--session" ] && [ "$3" = "notification" ] && [ "$4" = "show" ]; then
  if [ "${ZERDR_TEST_NOTIFICATION_RESULT:-shown}" = "shown" ]; then
    printf '%s\n' '{"ok":true,"result":{"shown":true}}'
  else
    printf '%s\n' '{"ok":true,"result":{"shown":false,"reason":"no_foreground_client"}}'
  fi
  exit 0
fi
if [ "$1" = "--session" ] && [ "$3" = "agent" ] && [ "$4" = "list" ]; then
  if [ -n "$ZERDR_TEST_AGENTS_DIR" ]; then
    printf '{"result":{"type":"agent_list","agents":['
    sep=''
    for entry in "$ZERDR_TEST_AGENTS_DIR"/*.json; do
      [ -f "$entry" ] || continue
      printf '%s' "$sep"
      tr -d '\n' < "$entry"
      sep=','
    done
    printf ']}}\n'
  elif [ -n "$ZERDR_TEST_AGENTS_JSON" ]; then
    printf '%s\n' "$ZERDR_TEST_AGENTS_JSON"
  else
    printf '%s\n' '{"result":{"type":"agent_list","agents":[]}}'
  fi
  exit 0
fi
if [ "$1" = "--session" ] && [ "$3" = "agent" ] && [ "$4" = "get" ]; then
  if [ -n "$ZERDR_TEST_AGENT_GET_SEQ" ]; then
    counter="$ZERDR_TEST_AGENT_GET_SEQ/counter"
    index=$(cat "$counter" 2>/dev/null || printf '0')
    index=$((index + 1))
    printf '%s' "$index" > "$counter"
    highest=0
    for entry in "$ZERDR_TEST_AGENT_GET_SEQ"/*.json; do
      [ -f "$entry" ] || continue
      candidate=$(basename "$entry" .json)
      case "$candidate" in *[!0-9]*) continue;; esac
      [ "$candidate" -gt "$highest" ] && highest="$candidate"
    done
    [ "$index" -gt "$highest" ] && index="$highest"
    entry="$ZERDR_TEST_AGENT_GET_SEQ/$index.json"
    if [ -f "$entry" ]; then
      first=$(head -n 1 "$entry")
      case "$first" in
        EXIT:*)
          printf '%s\n' '{"error":{"code":"pane_not_found"}}' >&2
          exit "${first#EXIT:}"
          ;;
      esac
      cat "$entry"
    fi
    exit 0
  fi
  if [ "${ZERDR_TEST_AGENT_GET_EXIT:-0}" != "0" ]; then
    printf '%s\n' '{"error":{"code":"pane_not_found"}}' >&2
    exit "$ZERDR_TEST_AGENT_GET_EXIT"
  fi
  if [ -n "$ZERDR_TEST_AGENT_GET_JSON" ]; then
    printf '%s\n' "$ZERDR_TEST_AGENT_GET_JSON"
    exit 0
  fi
  if [ -n "$ZERDR_TEST_AGENTS_DIR" ]; then
    for entry in "$ZERDR_TEST_AGENTS_DIR"/*.json; do
      [ -f "$entry" ] || continue
      found=$(tr -d '\n' < "$entry")
      case "$found" in
        *"\"pane_id\":\"$5\""*|*"\"name\":\"$5\""*)
          printf '{"result":{"type":"agent_info","agent":%s}}\n' "$found"
          exit 0
          ;;
      esac
    done
    printf '%s\n' '{"result":{"type":"agent_info"}}'
    exit 0
  fi
  printf '%s\n' "$ZERDR_TEST_AGENT_GET_JSON"
  exit 0
fi
if [ "$1" = "--session" ] && [ "$3" = "pane" ] && [ "$4" = "get" ]; then
  case " $ZERDR_TEST_PANE_GET_MISSING_IDS " in
    *" $5 "*)
      printf '%s\n' '{"error":"no such pane"}' >&2
      exit 1
      ;;
  esac
  found=''
  if [ -n "$ZERDR_TEST_AGENTS_DIR" ]; then
    for entry in "$ZERDR_TEST_AGENTS_DIR"/*.json; do
      [ -f "$entry" ] || continue
      body=$(tr -d '\n' < "$entry")
      case "$body" in
        *"\"pane_id\":\"$5\""*) found="$body";;
      esac
    done
  fi
  if [ -n "$found" ]; then
    printf '{"result":{"type":"pane_info","pane":%s}\n' "$found" | sed 's/}$/,"terminal_id":"term-'"$5"'"}}/'
  else
    printf '{"result":{"type":"pane_info","pane":{"pane_id":"%s","terminal_id":"term-%s"}}}\n' "$5" "$5"
  fi
  exit 0
fi
if [ "$1" = "--session" ] && [ "$3" = "terminal" ] && [ "$4" = "attach" ]; then
  if [ -n "$ZERDR_TEST_ATTACH_RELEASE_FILE" ]; then
    while [ ! -e "$ZERDR_TEST_ATTACH_RELEASE_FILE" ]; do sleep 0.01; done
  fi
  exit "${ZERDR_TEST_ATTACH_EXIT:-0}"
fi
if [ "$1" = "--session" ] && [ "$3" = "agent" ] && [ "$4" = "attach" ]; then
  if [ -n "$ZERDR_TEST_ATTACH_RELEASE_FILE" ]; then
    while [ ! -e "$ZERDR_TEST_ATTACH_RELEASE_FILE" ]; do sleep 0.01; done
  fi
  exit "${ZERDR_TEST_ATTACH_EXIT:-0}"
fi
if [ "$1" = "--session" ] && [ "$3" = "agent" ] && [ "$4" = "start" ]; then
  started_name="$5"
  started_kind=''
  started_pane=''
  shift 5
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --kind) started_kind="$2"; shift 2;;
      --pane) started_pane="$2"; shift 2;;
      *) shift;;
    esac
  done
  if [ -n "$ZERDR_TEST_AGENTS_DIR" ]; then
    printf '{"agent":"%s","name":"%s","agent_status":"idle","pane_id":"%s","workspace_id":"%s","terminal_title_stripped":"%s"}\n' \
      "$started_kind" "$started_name" "$started_pane" "${started_pane%%:*}" "$started_name" \
      > "$ZERDR_TEST_AGENTS_DIR/$started_name.json"
  fi
  printf '%s\n' '{"ok":true,"result":{}}'
  exit 0
fi
if [ "$1" = "--session" ] && [ "$3" = "tab" ] && [ "$4" = "create" ]; then
  if [ -n "$ZERDR_TEST_TAB_CREATE_JSON" ]; then
    printf '%s\n' "$ZERDR_TEST_TAB_CREATE_JSON"
    exit 0
  fi
  if [ -n "$ZERDR_TEST_PANE_COUNTER_FILE" ]; then
    minted=$(cat "$ZERDR_TEST_PANE_COUNTER_FILE" 2>/dev/null || printf '0')
    minted=$((minted + 1))
    printf '%s' "$minted" > "$ZERDR_TEST_PANE_COUNTER_FILE"
    printf '{"result":{"root_pane":{"pane_id":"%s:p%s"}}}\n' "$6" "$minted"
    exit 0
  fi
  printf '%s\n' '{"ok":true,"result":{}}'
  exit 0
fi
if [ "$1" = "--session" ] && [ "$3" = "workspace" ] && [ "$4" = "create" ]; then
  printf '%s\n' "$ZERDR_TEST_WORKSPACE_CREATE_JSON"
  exit 0
fi
if { [ "$#" = "0" ] || { [ "$1" = "--session" ] && [ "$#" = "2" ]; }; }; then
  if [ -n "$ZERDR_TEST_CHILD_PID_FILE" ]; then
    printf '%s\n' "$$" > "$ZERDR_TEST_CHILD_PID_FILE"
  fi
  if [ "${ZERDR_TEST_HERDR_EXIT:-0}" = "0" ]; then
    exec sleep "${ZERDR_TEST_HERDR_SLEEP:-0}"
  fi
  sleep "${ZERDR_TEST_HERDR_SLEEP:-0}"
  exit "$ZERDR_TEST_HERDR_EXIT"
fi
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
    if [ -n "$ZERDR_TEST_SESSIONS_FILE" ] && [ -f "$ZERDR_TEST_SESSIONS_FILE" ]; then
      while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line"; done < "$ZERDR_TEST_SESSIONS_FILE"
    else
      printf '%s\n' "$ZERDR_TEST_SESSIONS_JSON"
    fi
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

impl TestEnv {
    pub fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        let log = root.path().join("process.log");
        let zed = r##"#!/bin/sh
printf 'zed\t%s\n' "$*" >> "$ZERDR_TEST_LOG"
if [ -n "$ZERDR_TEST_ZED_CALL_MARKER" ]; then
  : > "$ZERDR_TEST_ZED_CALL_MARKER"
fi
if [ -n "$ZERDR_TEST_ZED_BLOCK_MARKER" ]; then
  : > "$ZERDR_TEST_ZED_BLOCK_MARKER"
  while [ ! -e "$ZERDR_TEST_ZED_BLOCK_CONTINUE" ]; do sleep 0.01; done
fi
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
        write_executable(&bin.join("herdr"), &format!("#!/bin/sh\n{FAKE_HERDR_BODY}"));
        write_executable(&bin.join("zed"), zed);
        Self { root, bin, log }
    }

    /// Build a private fake `herdr` whose `ZERDR_TEST_*` configuration is baked into the
    /// script, so library-level tests can drive the adapter directly without mutating this
    /// process's environment or slowing down the shared fake on `PATH`.
    pub fn baked_herdr(&self, name: &str, variables: &[(&str, String)]) -> zerdr::herdr::Herdr {
        let mut script = format!(
            "#!/bin/sh\nZERDR_TEST_LOG={}\nexport ZERDR_TEST_LOG\n",
            shell_quote(&self.log)
        );
        for (key, value) in variables {
            script.push_str(&format!("{key}={}\nexport {key}\n", shell_quote_str(value)));
        }
        script.push_str(FAKE_HERDR_BODY);
        let path = self.bin.join(name);
        write_executable(&path, &script);
        zerdr::herdr::Herdr::with_program(path.into())
    }

    pub fn command(&self) -> Command {
        self.command_for(assert_cmd::cargo::cargo_bin!("zerdr"))
    }

    pub fn command_for(&self, executable: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(executable);
        command
            .env("PATH", self.path())
            .env("ZERDR_TEST_ROOT", self.root.path())
            .env("ZERDR_TEST_LOG", &self.log)
            .env("ZERDR_HERDR_BIN", self.bin.join("herdr"))
            .env("ZERDR_ZED_BIN", self.bin.join("zed"))
            .env(
                "ZERDR_TEST_PLUGINS_JSON",
                compatible_plugins_json().to_string(),
            )
            .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_WORKSPACE_ID");
        command
    }

    pub fn std_command(&self) -> std::process::Command {
        let mut command = std::process::Command::new(assert_cmd::cargo::cargo_bin!("zerdr"));
        command
            .env("PATH", self.path())
            .env("ZERDR_TEST_ROOT", self.root.path())
            .env("ZERDR_TEST_LOG", &self.log)
            .env("ZERDR_HERDR_BIN", self.bin.join("herdr"))
            .env("ZERDR_ZED_BIN", self.bin.join("zed"))
            .env(
                "ZERDR_TEST_PLUGINS_JSON",
                compatible_plugins_json().to_string(),
            )
            .env("ZERDR_TEST_SESSIONS_JSON", r#"{"sessions":[]}"#)
            .env_remove("HERDR_SOCKET_PATH")
            .env_remove("HERDR_WORKSPACE_ID");
        command
    }

    pub fn prepare_launcher(&self) {
        self.command().args(["setup", "install"]).assert().success();
        fs::write(&self.log, "").unwrap();
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

pub fn compatible_plugins_json() -> serde_json::Value {
    serde_json::json!({
        "result": {
            "plugins": [{
                "plugin_id": "zerdr",
                "enabled": true,
                "actions": [{
                    "id": "open-zed",
                    "title": "Open Zed",
                    "contexts": ["workspace"],
                    "command": [
                        assert_cmd::cargo::cargo_bin!("zerdr").display().to_string(),
                        "open-from-herdr"
                    ]
                }],
                "events": [{
                    "on": "workspace.focused",
                    "command": [
                        assert_cmd::cargo::cargo_bin!("zerdr").display().to_string(),
                        "sync-from-herdr"
                    ]
                }],
            }]
        }
    })
}

fn shell_quote(path: &Path) -> String {
    shell_quote_str(&path.display().to_string())
}

fn shell_quote_str(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn write_executable(path: &Path, content: &str) {
    fs::write(path, content).unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}
