use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};

const HERDR_RUNTIME_ENV: &[&str] = &[
    "HERDR_ENV",
    "HERDR_SOCKET_PATH",
    "HERDR_CLIENT_SOCKET_PATH",
    "HERDR_BIN_PATH",
    "HERDR_ACTIVE_WORKSPACE_ID",
    "HERDR_ACTIVE_TAB_ID",
    "HERDR_ACTIVE_PANE_ID",
    "HERDR_ACTIVE_PANE_CWD",
    "HERDR_WORKSPACE_ID",
    "HERDR_TAB_ID",
    "HERDR_PANE_ID",
    "HERDR_PLUGIN_ID",
    "HERDR_PLUGIN_ROOT",
    "HERDR_PLUGIN_CONFIG_DIR",
    "HERDR_PLUGIN_STATE_DIR",
    "HERDR_PLUGIN_CONTEXT_JSON",
    "HERDR_PLUGIN_ACTION_ID",
    "HERDR_PLUGIN_EVENT",
    "HERDR_PLUGIN_EVENT_JSON",
    "HERDR_PLUGIN_ENTRYPOINT_ID",
    "HERDR_PLUGIN_CLICKED_URL",
    "HERDR_PLUGIN_LINK_HANDLER_ID",
];

#[derive(Debug, Clone)]
pub struct Zed {
    program: OsString,
}

impl Zed {
    pub fn from_env() -> Self {
        Self {
            program: std::env::var_os("ZERDR_ZED_BIN").unwrap_or_else(|| "zed".into()),
        }
    }

    pub fn activate_existing(&self, root: &Path) -> Result<()> {
        let output = self
            .command()
            .arg("--existing")
            .arg(root)
            .output()
            .map_err(|error| Error::User(format!("failed to run Zed: {error}")))?;
        if !output.status.success() {
            return Err(Error::Process {
                program: self.program.to_string_lossy().into_owned(),
                status: output.status.code().unwrap_or(1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(())
    }

    pub fn add_to_current(&self, root: &Path) -> Result<()> {
        let output = self
            .command()
            .arg("--add")
            .arg(root)
            .output()
            .map_err(|error| Error::User(format!("failed to run Zed: {error}")))?;
        if !output.status.success() {
            return Err(Error::Process {
                program: self.program.to_string_lossy().into_owned(),
                status: output.status.code().unwrap_or(1),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        Ok(())
    }

    pub fn supports_existing_and_add(&self) -> Result<bool> {
        let output = self
            .command()
            .arg("--help")
            .output()
            .map_err(|error| Error::User(format!("failed to run Zed: {error}")))?;
        let help = String::from_utf8_lossy(&output.stdout);
        Ok(output.status.success()
            && help.contains("The Zed CLI binary")
            && help.lines().any(|line| line.contains("--existing"))
            && help.lines().any(|line| line.contains("--add")))
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        // A Zed process first opened by a Herdr plugin becomes the environment
        // parent of every integrated terminal, so invocation context must stop here.
        for variable in HERDR_RUNTIME_ENV {
            command.env_remove(variable);
        }
        command
    }
}
