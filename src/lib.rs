pub mod cli;
pub mod doctor;
pub mod error;
pub mod herdr;
pub mod runtime;
pub mod setup;
pub mod state;
pub mod sync;
pub mod thread;
pub mod zed;

use clap::Parser;
use cli::{Cli, Command};
use error::{Error, Result};
use state::DEFAULT_SESSION_NAME;

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let remote = runtime::detect_remote_environment();
    if let Some(remote) = remote.as_ref()
        && !matches!(&cli.command, Some(Command::Doctor { .. }))
    {
        return Err(remote.rejection());
    }

    let Some(command) = cli.command else {
        let session_name = cli.session.as_deref().unwrap_or(DEFAULT_SESSION_NAME);
        let routing = runtime::resolve_launch(cli.anchor.as_deref())?;
        return herdr::run_wrapper(session_name, routing);
    };

    if cli.anchor.is_some() {
        return Err(Error::User(
            "--anchor cannot be used with a subcommand".to_owned(),
        ));
    }

    let command_session = match &command {
        Command::Sync { session }
        | Command::Bind { session, .. }
        | Command::Unbind { session }
        | Command::Doctor { session }
        | Command::Thread { session, .. } => session.as_deref(),
        Command::Setup
        | Command::Uninstall { .. }
        | Command::SyncFromHerdr
        | Command::OpenFromHerdr => None,
    };
    if cli.session.is_some() && command_session.is_some() {
        return Err(Error::User(
            "--session may be specified only once".to_owned(),
        ));
    }
    let accepts_session = matches!(
        &command,
        Command::Sync { .. }
            | Command::Bind { .. }
            | Command::Unbind { .. }
            | Command::Doctor { .. }
            | Command::Thread { .. }
    );
    if cli.session.is_some() && !accepts_session {
        return Err(Error::User(
            "--session cannot be used with this subcommand".to_owned(),
        ));
    }
    let explicit_session = command_session.or(cli.session.as_deref());

    match &command {
        Command::Sync { .. } => run_manual(explicit_session, |synchronizer| {
            synchronizer.sync_manual(explicit_session)
        }),
        Command::Bind { path, .. } => run_manual(explicit_session, |synchronizer| {
            synchronizer.bind(explicit_session, path.as_deref())
        }),
        Command::Unbind { .. } => run_manual(explicit_session, |synchronizer| {
            synchronizer.unbind(explicit_session)
        }),
        Command::Thread { enable: true, .. } | Command::Thread { disable: true, .. }
            if explicit_session.is_some() =>
        {
            Err(Error::User(
                "--session cannot be used when toggling thread auto mode".to_owned(),
            ))
        }
        Command::Thread { enable: true, .. } => setup::thread_auto_enable(),
        Command::Thread { disable: true, .. } => setup::thread_auto_disable(),
        Command::Thread {
            target,
            kind,
            create,
            auto,
            ..
        } => {
            if *auto && !setup::thread_auto_enabled(&state::Paths::discover()?) {
                return Ok(());
            }
            thread::run(
                explicit_session.unwrap_or(DEFAULT_SESSION_NAME),
                target.as_deref(),
                kind.as_deref(),
                *create,
            )
        }
        Command::Setup => setup::setup(),
        Command::Uninstall { purge } => setup::uninstall(*purge),
        Command::Doctor { .. } => match remote {
            Some(remote) => doctor::doctor_remote(remote.markers()),
            None => doctor::doctor(explicit_session.unwrap_or(DEFAULT_SESSION_NAME)),
        },
        Command::SyncFromHerdr => sync::Synchronizer::from_env()?.event(),
        Command::OpenFromHerdr => sync::Synchronizer::from_env()?.open_from_herdr(),
    }
}

fn run_manual(
    explicit_session: Option<&str>,
    operation: impl FnOnce(&sync::Synchronizer) -> Result<()>,
) -> Result<()> {
    let synchronizer = sync::Synchronizer::from_env()?;
    let result = operation(&synchronizer);
    let Err(error) = result else {
        return Ok(());
    };

    let force_visible = matches!(error, Error::SessionUnavailable | Error::NoLiveLease { .. });
    let message = error.to_string();
    let session_name = synchronizer.notification_session_name(explicit_session);
    let delivered = synchronizer
        .herdr()
        .notify_error_for(&session_name, &message)
        .unwrap_or(false);
    if std::env::var("ZERDR_TASK_MODE").is_ok_and(|value| value == "1") {
        if delivered && !force_visible {
            Ok(())
        } else {
            Err(Error::User(message))
        }
    } else {
        Err(error)
    }
}
