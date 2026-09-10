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

    /// Whether Zed is the frontmost application, or `None` when that cannot be known:
    /// no platform support (Linux), no frontmost application, or no answer from the
    /// test seam. Preview and Nightly builds share the `dev.zed.Zed` bundle prefix.
    pub fn frontmost_is_zed() -> Option<bool> {
        if std::env::var_os("ZERDR_TEST_ROOT").is_some() {
            let file = std::env::var_os("ZERDR_TEST_ZED_FRONTMOST_FILE")?;
            return match std::fs::read_to_string(file).ok()?.trim() {
                "1" => Some(true),
                "0" => Some(false),
                _ => None,
            };
        }
        frontmost_bundle_identifier().map(|identifier| identifier.starts_with("dev.zed.Zed"))
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

#[cfg(target_os = "macos")]
fn frontmost_bundle_identifier() -> Option<String> {
    use objc2_app_kit::NSWorkspace;
    let application = NSWorkspace::sharedWorkspace().frontmostApplication()?;
    Some(application.bundleIdentifier()?.to_string())
}

#[cfg(not(target_os = "macos"))]
fn frontmost_bundle_identifier() -> Option<String> {
    None
}
