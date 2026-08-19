use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use crate::error::{Error, Result};

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

    pub fn open(&self, root: &Path) -> Result<()> {
        let output = Command::new(&self.program)
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

    pub fn activate_existing(&self, root: &Path) -> Result<()> {
        let output = Command::new(&self.program)
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
        let output = Command::new(&self.program)
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
        let output = Command::new(&self.program)
            .arg("--help")
            .output()
            .map_err(|error| Error::User(format!("failed to run Zed: {error}")))?;
        let help = String::from_utf8_lossy(&output.stdout);
        Ok(output.status.success()
            && help.contains("The Zed CLI binary")
            && help.lines().any(|line| line.contains("--existing"))
            && help.lines().any(|line| line.contains("--add")))
    }
}
