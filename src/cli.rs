use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "zerdr",
    version,
    about = "Keep a Herdr session aligned with Zed"
)]
pub struct Cli {
    /// Use or create a named persistent Herdr session.
    #[arg(long)]
    pub session: Option<String>,
    /// Select terminal routing behavior.
    #[arg(long, value_enum)]
    pub mode: Option<LaunchMode>,
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
    Pick {
        /// Target a named persistent Herdr session.
        #[arg(long)]
        session: Option<String>,
    },
    /// Focus the next Herdr workspace.
    Next {
        /// Target a named persistent Herdr session.
        #[arg(long)]
        session: Option<String>,
    },
    /// Focus the previous Herdr workspace.
    Previous {
        /// Target a named persistent Herdr session.
        #[arg(long)]
        session: Option<String>,
    },
    /// Synchronize Zed to the focused Herdr workspace.
    Sync {
        /// Target a named persistent Herdr session.
        #[arg(long)]
        session: Option<String>,
    },
    /// Bind the selected workspace to a Git checkout.
    Bind {
        /// Target a named persistent Herdr session.
        #[arg(long)]
        session: Option<String>,
        path: Option<PathBuf>,
    },
    /// Remove the selected workspace binding.
    Unbind {
        /// Target a named persistent Herdr session.
        #[arg(long)]
        session: Option<String>,
    },
    /// Attach a Zed terminal thread to a Herdr agent.
    Thread {
        /// Herdr pane id or unique live agent name to attach to.
        target: Option<String>,
        /// Target a named persistent Herdr session.
        #[arg(long)]
        session: Option<String>,
        /// Agent kind to start when the workspace has no free agent.
        #[arg(long, conflicts_with = "target")]
        kind: Option<String>,
        /// Create the Herdr workspace when none matches this Git checkout.
        #[arg(long, conflicts_with = "target")]
        create: bool,
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
    Doctor {
        /// Target a named persistent Herdr session.
        #[arg(long)]
        session: Option<String>,
    },
    #[command(hide = true)]
    SyncFromHerdr,
    #[command(hide = true)]
    OpenFromHerdr,
}
