use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use dialoguer::{FuzzySelect, theme::ColorfulTheme};

use crate::error::{Error, Result};
use crate::herdr::Workspace;

pub fn choose(workspaces: &[Workspace]) -> Result<Option<usize>> {
    if let Ok(value) = std::env::var("ZERDR_TEST_PICK_INDEX") {
        if value == "cancel" {
            return Ok(None);
        }
        let index = value
            .parse::<usize>()
            .map_err(|_| Error::User("invalid test picker index".to_owned()))?;
        if index >= workspaces.len() {
            return Err(Error::User("test picker index is out of range".to_owned()));
        }
        wait_at_test_gate()?;
        return Ok(Some(index));
    }

    let labels = workspaces
        .iter()
        .map(|workspace| {
            let marker = if workspace.focused { "*" } else { " " };
            let number = workspace
                .number
                .map(|value| value.to_string())
                .unwrap_or_else(|| "?".to_owned());
            let path = workspace
                .checkout_path
                .as_ref()
                .or(workspace.cwd.as_ref())
                .map(|value| value.display().to_string())
                .unwrap_or_else(|| "unbound".to_owned());
            format!("{marker} {number}: {} — {path}", workspace.label)
        })
        .collect::<Vec<_>>();
    let default = workspaces
        .iter()
        .position(|workspace| workspace.focused)
        .unwrap_or(0);
    FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Herdr workspace")
        .items(&labels)
        .default(default)
        .interact_opt()
        .map_err(|error| Error::User(format!("workspace picker failed: {error}")))
}

fn wait_at_test_gate() -> Result<()> {
    let Some(marker) = std::env::var_os("ZERDR_TEST_PICK_READY") else {
        return Ok(());
    };
    let marker = PathBuf::from(marker);
    std::fs::write(&marker, b"ready").map_err(|error| Error::io(&marker, error))?;
    let continue_file = std::env::var_os("ZERDR_TEST_PICK_CONTINUE")
        .map(PathBuf::from)
        .ok_or_else(|| Error::User("missing picker test continuation path".to_owned()))?;
    for _ in 0..500 {
        if continue_file.exists() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(Error::User("timed out at picker test gate".to_owned()))
}
