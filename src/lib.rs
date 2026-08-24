pub mod cli;
pub mod doctor;
pub mod error;
pub mod herdr;
pub mod runtime;
pub mod setup;
pub mod state;
pub mod suspend;
pub mod sync;
pub mod thread;
pub mod zed;

use clap::Parser;
use cli::{AutoState, Cli, Command, SetupCommand, WorkspaceCommand};
use error::{Error, Result};
use state::DEFAULT_SESSION_NAME;

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let remote = runtime::detect_remote_environment();
    // Detach and attach only touch local state files and processes — no Zed, no
    // Herdr — so they stay usable from an SSH session on this machine, which is
    // exactly where a phone-sized client needs to run `zerdr detach` first.
    if let Some(remote) = remote.as_ref()
        && !matches!(
            &cli.command,
            Command::Setup {
                command: SetupCommand::Doctor
            } | Command::Detach
                | Command::Attach
        )
    {
        return Err(remote.rejection());
    }

    // clap accepts a global flag once before and once after the subcommand
    // (the subcommand occurrence overwrites the propagated one), so the
    // once-only rule is enforced on the raw argument list.
    if cli.session.len() > 1 || session_flag_occurrences() > 1 {
        return Err(Error::User(
            "--session may be specified only once".to_owned(),
        ));
    }
    let explicit_session = cli.session.first().map(String::as_str);
    let accepts_session = matches!(
        &cli.command,
        Command::Connect { .. }
            | Command::Start { .. }
            | Command::Workspace { .. }
            | Command::Setup {
                command: SetupCommand::Doctor
            }
    );
    if explicit_session.is_some() && !accepts_session {
        return Err(Error::User(
            "--session cannot be used with this command".to_owned(),
        ));
    }

    match &cli.command {
        Command::Connect { auto: true, .. } => {
            thread::run_auto(explicit_session.unwrap_or(DEFAULT_SESSION_NAME))
        }
        Command::Connect {
            target,
            kind,
            create,
            ..
        } => thread::run(
            explicit_session.unwrap_or(DEFAULT_SESSION_NAME),
            target.as_deref(),
            kind.as_deref(),
            *create,
        ),
        Command::Start { anchor } => {
            let routing = runtime::resolve_launch(anchor.as_deref())?;
            herdr::run_wrapper(explicit_session.unwrap_or(DEFAULT_SESSION_NAME), routing)
        }
        Command::Detach => suspend::detach(),
        Command::Attach => suspend::attach(),
        Command::Workspace { command } => match command {
            WorkspaceCommand::Sync => run_manual(explicit_session, |synchronizer| {
                synchronizer.sync_manual(explicit_session)
            }),
            WorkspaceCommand::Bind { path } => run_manual(explicit_session, |synchronizer| {
                synchronizer.bind(explicit_session, path.as_deref())
            }),
            WorkspaceCommand::Unbind => run_manual(explicit_session, |synchronizer| {
                synchronizer.unbind(explicit_session)
            }),
        },
        Command::Setup { command } => match command {
            SetupCommand::Install => setup::setup(),
            SetupCommand::Uninstall { purge } => setup::uninstall(*purge),
            SetupCommand::Doctor => match remote {
                Some(remote) => doctor::doctor_remote(remote.markers()),
                None => doctor::doctor(explicit_session.unwrap_or(DEFAULT_SESSION_NAME)),
            },
            SetupCommand::Auto {
                state: AutoState::Enable,
            } => setup::thread_auto_enable(),
            SetupCommand::Auto {
                state: AutoState::Disable,
            } => setup::thread_auto_disable(),
        },
        Command::SyncFromHerdr => sync::Synchronizer::from_env()?.event(),
        Command::OpenFromHerdr => sync::Synchronizer::from_env()?.open_from_herdr(),
    }
}

/// Counts `--session` occurrences in the raw arguments. Runs only after a
/// successful parse, so every counted token is a flag clap accepted; tokens
/// behind the first bare `--` are positionals and are not counted.
fn session_flag_occurrences() -> usize {
    std::env::args()
        .skip(1)
        .take_while(|argument| argument != "--")
        .filter(|argument| argument == "--session" || argument.starts_with("--session="))
        .count()
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
