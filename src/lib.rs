pub mod cli;
pub mod doctor;
pub mod error;
pub mod herdr;
pub mod picker;
pub mod runtime;
pub mod setup;
pub mod state;
pub mod sync;
pub mod zed;

use clap::Parser;
use cli::{Cli, Command};
use error::{Error, Result};

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.command.requires_zed_terminal() {
        runtime::require_zed_terminal()?;
    }

    match cli.command {
        Command::Herdr { anchor } => herdr::run_wrapper(&anchor),
        Command::Pick => run_manual(|synchronizer| synchronizer.pick()),
        Command::Next => run_manual(|synchronizer| synchronizer.navigate(1)),
        Command::Previous => run_manual(|synchronizer| synchronizer.navigate(-1)),
        Command::Sync => run_manual(|synchronizer| synchronizer.sync_manual()),
        Command::Bind { path } => run_manual(|synchronizer| synchronizer.bind(path.as_deref())),
        Command::Unbind => run_manual(|synchronizer| synchronizer.unbind()),
        Command::Setup => setup::setup(),
        Command::Uninstall { purge } => setup::uninstall(purge),
        Command::Doctor => doctor::doctor(),
        Command::SyncFromHerdr => sync::Synchronizer::from_env()?.event(),
    }
}

fn run_manual(operation: impl FnOnce(&sync::Synchronizer) -> Result<()>) -> Result<()> {
    let synchronizer = sync::Synchronizer::from_env()?;
    let result = operation(&synchronizer);
    let Err(error) = result else {
        return Ok(());
    };

    let force_visible = matches!(error, Error::SessionUnavailable | Error::NoLiveLease);
    let message = error.to_string();
    let delivered = synchronizer.herdr().notify_error(&message).unwrap_or(false);
    if std::env::var("ZERDR_TASK_MODE").is_ok_and(|value| value == "1") {
        if delivered && !force_visible {
            Ok(())
        } else {
            Err(Error::User(format!(
                "{message}. Start `zerdr herdr` in a Zed integrated terminal, then retry"
            )))
        }
    } else {
        Err(error)
    }
}
