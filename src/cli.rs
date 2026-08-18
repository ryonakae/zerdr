use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "zerdr",
    version,
    about = "Keep a Herdr session aligned with Zed"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Launch or attach the dedicated Herdr session.
    Herdr {
        /// Git project already open in the target Zed window.
        #[arg(long)]
        anchor: PathBuf,
    },
    /// Fuzzily select a Herdr workspace.
    Pick,
    /// Focus the next Herdr workspace.
    Next,
    /// Focus the previous Herdr workspace.
    Previous,
    /// Synchronize Zed to the focused Herdr workspace.
    Sync,
    /// Bind the focused workspace to a Git checkout.
    Bind { path: Option<PathBuf> },
    /// Remove the focused workspace binding.
    Unbind,
    /// Install the Herdr plugin and Zed tasks.
    Setup,
    /// Remove the Herdr plugin and Zed tasks.
    Uninstall {
        /// Also remove zerdr state after verifying there are no live leases.
        #[arg(long)]
        purge: bool,
    },
    /// Diagnose zerdr, Herdr, and Zed integration.
    Doctor,
    #[command(hide = true)]
    SyncFromHerdr,
}

impl Command {
    pub fn requires_zed_terminal(&self) -> bool {
        matches!(
            self,
            Self::Herdr { .. }
                | Self::Pick
                | Self::Next
                | Self::Previous
                | Self::Sync
                | Self::Bind { .. }
                | Self::Unbind
        )
    }
}
