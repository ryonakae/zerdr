use std::env;
use std::path::{Path, PathBuf};

use crate::cli::{FocusPolicy, LaunchMode};
use crate::error::{Error, Result};
use crate::state::{RouteFocus, RouteStrategy, canonical_git_root};

const REMOTE_ENV_MARKERS: [&str; 9] = [
    "SSH_CONNECTION",
    "SSH_CLIENT",
    "SSH_TTY",
    "WSL_DISTRO_NAME",
    "WSL_INTEROP",
    "container",
    "REMOTE_CONTAINERS",
    "DEVCONTAINER",
    "CODESPACES",
];
const REMOTE_FILE_MARKERS: [&str; 2] = ["/.dockerenv", "/run/.containerenv"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEnvironment {
    markers: Vec<String>,
}

impl RemoteEnvironment {
    pub fn markers(&self) -> &[String] {
        &self.markers
    }

    pub fn rejection(&self) -> Error {
        Error::User(format!(
            "zerdr runtime commands require a local macOS or Linux environment; detected {}",
            self.markers.join(", ")
        ))
    }
}

pub fn detect_remote_environment() -> Option<RemoteEnvironment> {
    let test_markers = env::var("ZERDR_TEST_REMOTE_MARKERS")
        .ok()
        .filter(|_| env::var_os("ZERDR_TEST_ROOT").is_some())
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut markers = Vec::new();
    for marker in REMOTE_ENV_MARKERS {
        if env::var_os(marker).is_some_and(|value| !value.is_empty()) {
            markers.push(marker.to_owned());
        }
    }
    for marker in REMOTE_FILE_MARKERS {
        if Path::new(marker).exists() || test_markers.iter().any(|value| value == marker) {
            markers.push(marker.to_owned());
        }
    }
    (!markers.is_empty()).then_some(RemoteEnvironment { markers })
}

pub fn resolve_launch(
    mode: LaunchMode,
    anchor: Option<&Path>,
    focus: Option<FocusPolicy>,
) -> Result<RouteStrategy> {
    if let Some(remote) = detect_remote_environment() {
        return Err(remote.rejection());
    }
    if mode == LaunchMode::External && anchor.is_some() {
        return Err(Error::User(
            "--anchor cannot be used with --mode external".to_owned(),
        ));
    }
    let resolved_mode = if anchor.is_some() {
        LaunchMode::Internal
    } else {
        match mode {
            LaunchMode::Auto if in_zed_terminal() => LaunchMode::Internal,
            LaunchMode::Auto => LaunchMode::External,
            explicit => explicit,
        }
    };
    match resolved_mode {
        LaunchMode::Internal => {
            if focus.is_some() {
                return Err(Error::User(
                    "--focus applies only to external routing".to_owned(),
                ));
            }
            let candidate = match anchor {
                Some(path) => path.to_path_buf(),
                None => env::current_dir().map_err(|error| {
                    Error::User(format!("could not read current directory: {error}"))
                })?,
            };
            Ok(RouteStrategy::Internal {
                anchor_root: canonical_git_root(&candidate)?,
            })
        }
        LaunchMode::External => {
            let focus = match focus {
                Some(FocusPolicy::Terminal) if platform() == Platform::Linux => {
                    return Err(Error::User(
                        "--focus terminal is unsupported on Linux; use --focus zed".to_owned(),
                    ));
                }
                Some(FocusPolicy::Terminal) => RouteFocus::Terminal,
                Some(FocusPolicy::Zed) => RouteFocus::Zed,
                None if platform() == Platform::MacOs => RouteFocus::Terminal,
                None => RouteFocus::Zed,
            };
            Ok(RouteStrategy::External { focus })
        }
        LaunchMode::Auto => unreachable!("auto mode is resolved above"),
    }
}

pub fn in_zed_terminal() -> bool {
    env::var("ZED_TERM").is_ok_and(|value| value == "true")
        && env::var("TERM_PROGRAM").is_ok_and(|value| value == "zed")
}

pub fn platform() -> Platform {
    if env::var_os("ZERDR_TEST_ROOT").is_some() {
        match env::var("ZERDR_TEST_PLATFORM").as_deref() {
            Ok("macos") => return Platform::MacOs,
            Ok("linux") => return Platform::Linux,
            _ => {}
        }
    }
    if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}

pub fn current_directory() -> Result<PathBuf> {
    env::current_dir()
        .map_err(|error| Error::User(format!("could not read current directory: {error}")))
}
