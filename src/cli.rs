use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "zerdr",
    version,
    about = "Keep a Herdr session aligned with Zed",
    arg_required_else_help = true
)]
pub struct Cli {
    /// Target a named persistent Herdr session.
    //
    // clap accepts a global flag once before and once after the subcommand,
    // so occurrences are collected and the once-only rule is enforced in
    // the dispatcher.
    #[arg(long, global = true, value_name = "SESSION")]
    pub session: Vec<String>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Connect this Zed terminal thread to a Herdr pane.
    Connect {
        /// Herdr pane id or unique live agent name to attach to.
        target: Option<String>,
        /// Start an agent of this kind in a fresh pane instead of a plain shell.
        #[arg(long, conflicts_with = "target")]
        kind: Option<String>,
        /// Create the Herdr session and workspace when they are missing.
        #[arg(long, conflicts_with = "target")]
        create: bool,
        /// Attach best-effort, and only while auto mode is enabled.
        #[arg(long, hide = true, conflicts_with_all = ["target", "kind", "create"])]
        auto: bool,
    },
    /// Launch Herdr wrapped with Zed focus sync.
    Start {
        /// Git project already open in the target Zed window.
        #[arg(long)]
        anchor: Option<PathBuf>,
    },
    /// Manage the selected Herdr workspace's Zed integration.
    #[command(arg_required_else_help = true)]
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// Install, diagnose, or remove the Herdr and Zed integration.
    #[command(arg_required_else_help = true)]
    Setup {
        #[command(subcommand)]
        command: SetupCommand,
    },
    #[command(hide = true)]
    SyncFromHerdr,
    #[command(hide = true)]
    OpenFromHerdr,
}

#[derive(Debug, Subcommand)]
pub enum WorkspaceCommand {
    /// Bind the selected workspace to a Git checkout.
    Bind { path: Option<PathBuf> },
    /// Remove the selected workspace binding.
    Unbind,
    /// Synchronize Zed to the focused Herdr workspace.
    Sync,
}

#[derive(Debug, Subcommand)]
pub enum SetupCommand {
    /// Install the Herdr plugin and Zed tasks.
    Install,
    /// Remove the Herdr plugin and Zed tasks.
    Uninstall {
        /// Also remove zerdr state after verifying there are no live leases.
        #[arg(long)]
        purge: bool,
    },
    /// Diagnose zerdr, Herdr, and Zed integration.
    Doctor,
    /// Toggle auto mode, installing Zed's terminal_init_command once.
    Auto {
        /// Enable or disable auto mode; disable leaves Zed settings as they are.
        state: AutoState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AutoState {
    Enable,
    Disable,
}
