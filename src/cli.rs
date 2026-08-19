use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "zerdr",
    version,
    about = "Keep a Herdr session aligned with Zed",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    /// Select terminal routing behavior.
    #[arg(long, value_enum, default_value_t = LaunchMode::Auto)]
    pub mode: LaunchMode,
    /// Git project already open in the target Zed window.
    #[arg(long)]
    pub anchor: Option<PathBuf>,
    /// Select which application remains foreground after external routing.
    #[arg(long, value_enum)]
    pub focus: Option<FocusPolicy>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LaunchMode {
    Auto,
    Internal,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FocusPolicy {
    Terminal,
    Zed,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Fuzzily select a Herdr workspace.
    Pick,
    /// Focus the next Herdr workspace.
    Next,
    /// Focus the previous Herdr workspace.
    Previous,
    /// Synchronize Zed to the focused Herdr workspace.
    Sync,
    /// Bind the selected workspace to a Git checkout.
    Bind {
        /// Target the focused workspace in this Herdr session.
        #[arg(long)]
        session: Option<String>,
        path: Option<PathBuf>,
    },
    /// Remove the selected workspace binding.
    Unbind {
        /// Target the focused workspace in this Herdr session.
        #[arg(long)]
        session: Option<String>,
    },
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
