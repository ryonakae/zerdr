use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstInputValue, CstRootNode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::herdr::Herdr;
use crate::state::{LeaseSet, LifecycleGuard, Paths};

const OWNED_LABELS: [&str; 1] = ["zerdr: Herdr"];
const INSTALL_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct InstallState {
    pub(crate) schema_version: u32,
    pub(crate) executable: PathBuf,
    pub(crate) task_fingerprints: BTreeMap<String, String>,
    /// Present while zerdr owns Zed's `agent.terminal_init_command`, recorded by
    /// `zerdr thread --enable` and consumed by uninstall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) terminal_init_command_fingerprint: Option<String>,
}

#[derive(Deserialize)]
struct PluginManifest {
    id: String,
    min_herdr_version: String,
    #[serde(default)]
    events: Vec<PluginEvent>,
}

#[derive(Deserialize)]
struct PluginEvent {
    on: String,
    command: Vec<String>,
}

pub(crate) fn validate_launcher_installation(paths: &Paths, herdr: &Herdr) -> Result<()> {
    let plugins = herdr.plugin_list().map_err(setup_guidance)?;
    if !plugin_is_compatible(&plugins) {
        return Err(setup_guidance(Error::User(
            "Herdr zerdr plugin is missing, disabled, or lacks workspace.focused".to_owned(),
        )));
    }
    let install = load_install_state(&paths.install_state_file)
        .map_err(setup_guidance)?
        .ok_or_else(|| {
            setup_guidance(Error::User(
                "zerdr install ownership state is missing".to_owned(),
            ))
        })?;
    let manifest_path = paths.plugin_dir.join("herdr-plugin.toml");
    let manifest_text = fs::read_to_string(&manifest_path)
        .map_err(|error| setup_guidance(Error::io(&manifest_path, error)))?;
    let manifest: PluginManifest = toml::from_str(&manifest_text).map_err(|error| {
        setup_guidance(Error::User(format!(
            "generated Herdr manifest is invalid: {error}"
        )))
    })?;
    let current = std::env::current_exe()
        .map_err(|error| {
            setup_guidance(Error::User(format!(
                "could not determine the running zerdr executable: {error}"
            )))
        })?
        .canonicalize()
        .map_err(|error| {
            setup_guidance(Error::User(format!(
                "could not resolve the running zerdr executable: {error}"
            )))
        })?;
    let installed = install
        .executable
        .canonicalize()
        .map_err(|error| setup_guidance(Error::io(&install.executable, error)))?;
    let focus_events = manifest
        .events
        .iter()
        .filter(|event| event.on == "workspace.focused")
        .collect::<Vec<_>>();
    let compatible_event = focus_events.len() == 1
        && focus_events[0].command.len() == 2
        && focus_events[0].command[1] == "sync-from-herdr"
        && Path::new(&focus_events[0].command[0])
            .canonicalize()
            .is_ok_and(|event_executable| event_executable == current);
    let compatible_manifest = manifest.id == "zerdr"
        && manifest.min_herdr_version == "0.8.0"
        && installed == current
        && compatible_event;
    if compatible_manifest {
        Ok(())
    } else {
        Err(setup_guidance(Error::User(
            "generated Herdr manifest or installed executable is incompatible".to_owned(),
        )))
    }
}

pub(crate) fn plugin_is_compatible(value: &Value) -> bool {
    compatible_plugin(value).is_some_and(|plugin| has_focus_event(plugin, None))
}

pub(crate) fn plugin_has_complete_action(value: &Value, executable: &Path) -> bool {
    let expected = executable.display().to_string();
    compatible_plugin(value).is_some_and(|plugin| {
        has_focus_event(plugin, Some(&expected))
            && plugin
                .get("actions")
                .and_then(Value::as_array)
                .map(|actions| {
                    actions
                        .iter()
                        .filter(|action| {
                            action.get("id").and_then(Value::as_str) == Some("open-zed")
                        })
                        .collect::<Vec<_>>()
                })
                .is_some_and(|actions| {
                    actions.len() == 1
                        && actions[0].get("title").and_then(Value::as_str) == Some("Open Zed")
                        && actions[0]
                            .get("contexts")
                            .and_then(Value::as_array)
                            .is_some_and(|contexts| {
                                contexts.len() == 1 && contexts[0].as_str() == Some("workspace")
                            })
                        && action_command_matches(actions[0], &expected, "open-from-herdr")
                })
    })
}

fn compatible_plugin(value: &Value) -> Option<&Value> {
    value
        .pointer("/result/plugins")
        .or_else(|| value.get("plugins"))
        .and_then(Value::as_array)?
        .iter()
        .find(|plugin| {
            plugin.get("plugin_id").and_then(Value::as_str) == Some("zerdr")
                && plugin.get("enabled").and_then(Value::as_bool) == Some(true)
        })
}

fn has_focus_event(plugin: &Value, executable: Option<&str>) -> bool {
    plugin
        .get("events")
        .and_then(Value::as_array)
        .map(|events| {
            events
                .iter()
                .filter(|event| {
                    event.get("on").and_then(Value::as_str) == Some("workspace.focused")
                })
                .collect::<Vec<_>>()
        })
        .is_some_and(|events| {
            events.len() == 1
                && executable.is_none_or(|executable| {
                    action_command_matches(events[0], executable, "sync-from-herdr")
                })
        })
}

fn action_command_matches(value: &Value, executable: &str, subcommand: &str) -> bool {
    value
        .get("command")
        .and_then(Value::as_array)
        .is_some_and(|command| {
            command.len() == 2
                && command[0].as_str() == Some(executable)
                && command[1].as_str() == Some(subcommand)
        })
}

fn setup_guidance(error: Error) -> Error {
    Error::User(format!("{error}; run `zerdr setup`"))
}

pub fn setup() -> Result<()> {
    let paths = Paths::discover()?;
    let executable = stable_executable()?;
    let generated = generated_tasks(&executable)?;
    let old_install = load_install_state(&paths.install_state_file)?;
    let tasks_file = resolve_config_file(&paths.zed_tasks_file)?;
    let original = read_optional_file(&tasks_file)?;
    let original_text = original
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()
        .map_err(|error| Error::User(format!("Zed tasks file is not UTF-8: {error}")))?
        .unwrap_or("[]\n");
    let merged = merge_tasks(original_text, &generated, old_install.as_ref())?;
    let install = InstallState {
        schema_version: INSTALL_SCHEMA_VERSION,
        executable: executable.clone(),
        task_fingerprints: generated
            .iter()
            .map(|task| (task_label(task).unwrap().to_owned(), fingerprint(task)))
            .collect(),
        // Ownership recorded by `zerdr thread --enable` must survive rerunning setup,
        // or uninstall could no longer clean the settings value up.
        terminal_init_command_fingerprint: old_install
            .as_ref()
            .and_then(|state| state.terminal_init_command_fingerprint.clone()),
    };

    let manifest_path = paths.plugin_dir.join("herdr-plugin.toml");
    let previous_manifest = read_optional_file(&manifest_path)?;
    backup_before_mutation(&paths, "tasks", &original)?;
    materialize_manifest(&paths, &executable)?;
    let herdr = Herdr::from_env();
    if let Err(error) = herdr.plugin_link(&paths.plugin_dir) {
        rollback_plugin(&herdr, &paths, previous_manifest.as_deref());
        return Err(error);
    }

    if merged.as_bytes() != original_text.as_bytes()
        && let Err(error) = write_checked(&tasks_file, original.as_deref(), merged.as_bytes())
    {
        rollback_plugin(&herdr, &paths, previous_manifest.as_deref());
        return Err(error);
    }
    let install_write =
        if std::env::var("ZERDR_TEST_FAIL_INSTALL_STATE_WRITE").is_ok_and(|value| value == "1") {
            Err(Error::User(
                "injected install-state write failure".to_owned(),
            ))
        } else {
            write_json(&paths.install_state_file, &install)
        };
    if let Err(error) = install_write {
        let _ = restore_optional(&tasks_file, original.as_deref());
        rollback_plugin(&herdr, &paths, previous_manifest.as_deref());
        return Err(error);
    }

    println!("zerdr setup complete");
    println!(
        "Attach a Zed terminal thread to Herdr by running `zerdr thread` inside it.\nOptional automation: run `zerdr thread --enable` to attach every new terminal thread automatically."
    );
    println!(
        "Add this Herdr keybinding manually if desired:\n{}",
        include_str!("../assets/herdr/keymap.example.toml")
    );
    println!(
        "Add these Zed keybindings manually if desired:\n{}",
        include_str!("../assets/zed/keymap.example.json")
    );
    Ok(())
}

pub fn uninstall(purge: bool) -> Result<()> {
    let paths = Paths::discover()?;
    let _lifecycle = if purge {
        Some(LifecycleGuard::acquire(&paths.lifecycle_lock_file)?)
    } else {
        None
    };
    if purge && LeaseSet::new(paths.leases_dir.clone()).any_live()? {
        return Err(Error::User(
            "cannot purge zerdr state while a live bare `zerdr` wrapper exists".to_owned(),
        ));
    }

    let install = load_install_state(&paths.install_state_file)?;
    let tasks_file = resolve_config_file(&paths.zed_tasks_file)?;
    let settings_file = resolve_config_file(&paths.zed_settings_file)?;
    let original = read_optional_file(&tasks_file)?;
    let original_text = original
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()
        .map_err(|error| Error::User(format!("Zed tasks file is not UTF-8: {error}")))?
        .unwrap_or("[]\n");
    let (updated, preserved) = if let Some(install) = install.as_ref() {
        remove_owned_tasks(original_text, install)?
    } else {
        (original_text.to_owned(), Vec::new())
    };

    let herdr = Herdr::from_env();
    herdr.plugin_uninstall()?;
    if updated.as_bytes() != original_text.as_bytes() {
        backup_before_mutation(&paths, "tasks", &original)?;
        if let Err(error) = write_checked(&tasks_file, original.as_deref(), updated.as_bytes()) {
            let _ = herdr.plugin_link(&paths.plugin_dir);
            return Err(error);
        }
    }
    if let Some(install) = install.as_ref() {
        let original_settings = read_optional_file(&settings_file)?;
        if let Some(bytes) = original_settings.as_deref() {
            let text = std::str::from_utf8(bytes)
                .map_err(|error| Error::User(format!("Zed settings file is not UTF-8: {error}")))?;
            let updated = remove_owned_init_command(text, install)?;
            if updated.as_bytes() != bytes {
                backup_before_mutation(&paths, "settings", &original_settings)?;
                write_checked(
                    &settings_file,
                    original_settings.as_deref(),
                    updated.as_bytes(),
                )?;
            }
        }
    }
    if let Err(error) = fs::remove_file(&paths.thread_auto_flag_file)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(Error::io(&paths.thread_auto_flag_file, error));
    }
    if paths.plugin_dir.exists() {
        fs::remove_dir_all(&paths.plugin_dir)
            .map_err(|error| Error::io(&paths.plugin_dir, error))?;
    }
    if paths.install_state_file.exists() {
        fs::remove_file(&paths.install_state_file)
            .map_err(|error| Error::io(&paths.install_state_file, error))?;
    }
    for label in preserved {
        eprintln!("zerdr: preserving modified or foreign Zed task {label:?}");
    }
    if purge {
        if paths.state_dir.exists() {
            fs::remove_dir_all(&paths.state_dir)
                .map_err(|error| Error::io(&paths.state_dir, error))?;
        }
        if paths.data_dir.exists() {
            fs::remove_dir_all(&paths.data_dir)
                .map_err(|error| Error::io(&paths.data_dir, error))?;
        }
    }
    println!("zerdr uninstall complete");
    Ok(())
}

pub fn owned_labels() -> &'static [&'static str] {
    &OWNED_LABELS
}

fn merge_tasks(text: &str, generated: &[Value], previous: Option<&InstallState>) -> Result<String> {
    let root = parse_tasks(text)?;
    let array = root
        .array_value()
        .ok_or_else(|| Error::User("Zed tasks file must contain a top-level array".to_owned()))?;
    let generated_by_label = generated
        .iter()
        .map(|task| (task_label(task).unwrap(), task))
        .collect::<BTreeMap<_, _>>();
    let mut present = BTreeSet::new();
    let mut remove = Vec::new();
    let mut stale_preserved = Vec::new();

    for element in array.elements() {
        let Some(value) = element.to_serde_value() else {
            continue;
        };
        let Some(label) = task_label(&value) else {
            continue;
        };
        let Some(generated_task) = generated_by_label.get(label) else {
            // A task an older install owned but that is no longer generated is cleaned
            // up while still byte-owned; a modified copy stays with the user.
            match previous.and_then(|state| state.task_fingerprints.get(label)) {
                Some(recorded) if *recorded == fingerprint(&value) => remove.push(element),
                Some(_) => stale_preserved.push(label.to_owned()),
                None => {}
            }
            continue;
        };
        if !present.insert(label.to_owned()) {
            return Err(Error::User(format!(
                "conflicting Zed task label {label:?} appears more than once"
            )));
        }
        let current_fingerprint = fingerprint(&value);
        let previous_fingerprint = previous.and_then(|state| state.task_fingerprints.get(label));
        if previous_fingerprint != Some(&current_fingerprint) {
            return Err(Error::User(format!(
                "conflicting Zed task {label:?} is not owned by zerdr"
            )));
        }
        if fingerprint(generated_task) != current_fingerprint {
            remove.push(element);
            present.remove(label);
        }
    }

    for element in remove {
        element.remove();
    }
    for task in generated {
        let label = task_label(task).unwrap();
        if !present.contains(label) {
            array.append(value_to_cst(task));
        }
    }
    for label in stale_preserved {
        eprintln!("zerdr: preserving modified or foreign Zed task {label:?}");
    }
    array.ensure_multiline();
    Ok(root.to_string())
}

fn remove_owned_tasks(text: &str, install: &InstallState) -> Result<(String, Vec<String>)> {
    let root = parse_tasks(text)?;
    let array = root
        .array_value()
        .ok_or_else(|| Error::User("Zed tasks file must contain a top-level array".to_owned()))?;
    let mut remove = Vec::new();
    let mut preserved = Vec::new();
    for element in array.elements() {
        let Some(value) = element.to_serde_value() else {
            continue;
        };
        let Some(label) = task_label(&value) else {
            continue;
        };
        let Some(recorded) = install.task_fingerprints.get(label) else {
            continue;
        };
        if &fingerprint(&value) == recorded {
            remove.push(element);
        } else {
            preserved.push(label.to_owned());
        }
    }
    for element in remove {
        element.remove();
    }
    Ok((root.to_string(), preserved))
}

fn parse_tasks(text: &str) -> Result<CstRootNode> {
    CstRootNode::parse(text, &ParseOptions::default())
        .map_err(|error| Error::User(format!("failed to parse Zed tasks JSONC: {error}")))
}

pub(crate) fn terminal_init_command(executable: &Path) -> String {
    format!("{} thread --auto", executable.display())
}

pub(crate) fn thread_auto_enabled(paths: &Paths) -> bool {
    paths.thread_auto_flag_file.exists()
}

pub fn thread_auto_enable() -> Result<()> {
    let paths = Paths::discover()?;
    let install = load_install_state(&paths.install_state_file)?
        .ok_or_else(|| setup_guidance(Error::User("zerdr is not set up".to_owned())))?;
    let expected = terminal_init_command(&install.executable);
    let expected_fingerprint = fingerprint(&Value::String(expected.clone()));
    let settings_file = resolve_config_file(&paths.zed_settings_file)?;
    let original = read_optional_file(&settings_file)?;
    let text = original
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()
        .map_err(|error| Error::User(format!("Zed settings file is not UTF-8: {error}")))?
        .unwrap_or("{}\n")
        .to_owned();
    match installed_init_command(&text)? {
        None => {
            let updated = insert_init_command(&text, &expected)?;
            // Ownership is recorded before the settings write: a fingerprint without a
            // value is harmless, while a value without a fingerprint could never be
            // cleaned up by uninstall.
            record_init_command_fingerprint(&paths, &install, &expected_fingerprint)?;
            backup_before_mutation(&paths, "settings", &original)?;
            write_checked(&settings_file, original.as_deref(), updated.as_bytes())?;
            println!("zerdr: installed Zed agent.terminal_init_command {expected:?}");
        }
        Some(current) if current == expected => {
            record_init_command_fingerprint(&paths, &install, &expected_fingerprint)?;
        }
        Some(current) => {
            return Err(Error::User(format!(
                "Zed agent.terminal_init_command is already set to {current:?}; remove it before enabling thread auto mode"
            )));
        }
    }
    fs::create_dir_all(&paths.state_dir).map_err(|error| Error::io(&paths.state_dir, error))?;
    fs::write(&paths.thread_auto_flag_file, b"")
        .map_err(|error| Error::io(&paths.thread_auto_flag_file, error))?;
    println!("zerdr: thread auto mode is enabled");
    Ok(())
}

pub fn thread_auto_disable() -> Result<()> {
    let paths = Paths::discover()?;
    if let Err(error) = fs::remove_file(&paths.thread_auto_flag_file)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(Error::io(&paths.thread_auto_flag_file, error));
    }
    println!("zerdr: thread auto mode is disabled");
    Ok(())
}

fn record_init_command_fingerprint(
    paths: &Paths,
    install: &InstallState,
    expected: &str,
) -> Result<()> {
    if install.terminal_init_command_fingerprint.as_deref() == Some(expected) {
        return Ok(());
    }
    let updated = InstallState {
        terminal_init_command_fingerprint: Some(expected.to_owned()),
        ..install.clone()
    };
    write_json(&paths.install_state_file, &updated)
}

fn insert_init_command(text: &str, value: &str) -> Result<String> {
    let root = CstRootNode::parse(text, &ParseOptions::default())
        .map_err(|error| Error::User(format!("failed to parse Zed settings JSONC: {error}")))?;
    if root.value().is_some() && root.object_value().is_none() {
        return Err(Error::User(
            "Zed settings file must contain a top-level object".to_owned(),
        ));
    }
    let root_object = root.object_value_or_set();
    let agent = match root_object.object_value("agent") {
        Some(agent) => agent,
        None => {
            root_object.append("agent", CstInputValue::Object(Vec::new()));
            root_object
                .object_value("agent")
                .expect("the agent object was just appended")
        }
    };
    agent.append(
        "terminal_init_command",
        CstInputValue::String(value.to_owned()),
    );
    Ok(root.to_string())
}

/// The `agent.terminal_init_command` currently in a Zed settings document, if any.
pub(crate) fn installed_init_command(text: &str) -> Result<Option<String>> {
    let value: Option<Value> =
        jsonc_parser::parse_to_serde_value(text, &ParseOptions::default())
            .map_err(|error| Error::User(format!("failed to parse Zed settings JSONC: {error}")))?;
    Ok(value
        .as_ref()
        .and_then(|value| value.pointer("/agent/terminal_init_command"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned))
}

/// Drop `agent.terminal_init_command` when it still matches what setup recorded.
fn remove_owned_init_command(text: &str, install: &InstallState) -> Result<String> {
    let Some(current) = installed_init_command(text)? else {
        return Ok(text.to_owned());
    };
    let Some(recorded) = install.terminal_init_command_fingerprint.as_deref() else {
        return Ok(text.to_owned());
    };
    if recorded != fingerprint(&Value::String(current)) {
        return Ok(text.to_owned());
    }
    let root = CstRootNode::parse(text, &ParseOptions::default())
        .map_err(|error| Error::User(format!("failed to parse Zed settings JSONC: {error}")))?;
    let Some(agent) = root
        .object_value()
        .and_then(|object| object.object_value("agent"))
    else {
        return Ok(text.to_owned());
    };
    if let Some(property) = agent.get("terminal_init_command") {
        property.remove();
    }
    Ok(root.to_string())
}

pub(crate) fn generated_tasks(executable: &Path) -> Result<Vec<Value>> {
    let command = shell_quote(executable);
    let rendered = include_str!("../assets/zed/tasks.json.in").replace(
        "@ZERDR_EXECUTABLE@",
        &serde_json::to_string(&command).expect("serializing a string cannot fail"),
    );
    serde_json::from_str(&rendered).map_err(|source| Error::Json {
        what: "embedded Zed task template".to_owned(),
        source,
    })
}

fn materialize_manifest(paths: &Paths, executable: &Path) -> Result<()> {
    fs::create_dir_all(&paths.plugin_dir).map_err(|error| Error::io(&paths.plugin_dir, error))?;
    let rendered = include_str!("../assets/herdr/herdr-plugin.toml.in")
        .replace("@VERSION@", env!("CARGO_PKG_VERSION"))
        .replace(
            "@ZERDR_EXECUTABLE@",
            &serde_json::to_string(&executable.display().to_string())
                .expect("serializing a string cannot fail"),
        );
    let manifest = paths.plugin_dir.join("herdr-plugin.toml");
    write_unchecked(&manifest, rendered.as_bytes())
}

fn stable_executable() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ZERDR_SETUP_EXECUTABLE") {
        return absolute_path(PathBuf::from(path));
    }
    let arg0 = std::env::args_os()
        .next()
        .ok_or_else(|| Error::User("could not determine zerdr executable path".to_owned()))?;
    let arg0_path = PathBuf::from(&arg0);
    if arg0_path.is_absolute() {
        return Ok(arg0_path);
    }
    if arg0_path.components().count() > 1 {
        return absolute_path(arg0_path);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(&arg0);
        if candidate.is_file() {
            return absolute_path(candidate);
        }
    }
    Err(Error::User(
        "could not find the invoked zerdr executable in PATH".to_owned(),
    ))
}

fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()
            .map_err(|error| Error::User(format!("failed to read current directory: {error}")))?
            .join(path))
    }
}

fn shell_quote(path: &Path) -> String {
    let value = path.as_os_str().to_string_lossy();
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn task_label(value: &Value) -> Option<&str> {
    value.get("label").and_then(Value::as_str)
}

pub(crate) fn fingerprint(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).expect("serializing JSON value cannot fail");
    hex::encode(Sha256::digest(bytes))
}

fn value_to_cst(value: &Value) -> CstInputValue {
    match value {
        Value::Null => CstInputValue::Null,
        Value::Bool(value) => CstInputValue::Bool(*value),
        Value::Number(value) => CstInputValue::Number(value.to_string()),
        Value::String(value) => CstInputValue::String(value.clone()),
        Value::Array(values) => CstInputValue::Array(values.iter().map(value_to_cst).collect()),
        Value::Object(values) => CstInputValue::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), value_to_cst(value)))
                .collect(),
        ),
    }
}

pub(crate) fn load_install_state(path: &Path) -> Result<Option<InstallState>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path).map_err(|error| Error::io(path, error))?;
    let state: InstallState = serde_json::from_slice(&bytes).map_err(|source| Error::Json {
        what: path.display().to_string(),
        source,
    })?;
    if state.schema_version != INSTALL_SCHEMA_VERSION {
        return Err(Error::User(format!(
            "unsupported install schema version {}",
            state.schema_version
        )));
    }
    Ok(Some(state))
}

/// Zed configuration is often a symlink into a dotfiles checkout, so resolve one level of
/// indirection and operate on the real file. Writing to the link path itself would replace
/// the link with a regular file and quietly disconnect it from dotfiles.
fn resolve_config_file(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => path.canonicalize().map_err(|error| {
            Error::User(format!(
                "Zed configuration file {} is a broken symlink: {error}",
                path.display()
            ))
        }),
        Ok(_) => Ok(path.to_path_buf()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(error) => Err(Error::io(path, error)),
    }
}

fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::io(path, error)),
    }
}

fn backup_before_mutation(paths: &Paths, label: &str, original: &Option<Vec<u8>>) -> Result<()> {
    let Some(original) = original else {
        return Ok(());
    };
    let backup_dir = paths.state_dir.join("backups");
    fs::create_dir_all(&backup_dir).map_err(|error| Error::io(&backup_dir, error))?;
    let backup = backup_dir.join(format!("{label}-{}.jsonc", now_millis()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&backup)
        .map_err(|error| Error::io(&backup, error))?;
    file.write_all(original)
        .map_err(|error| Error::io(&backup, error))?;
    file.sync_all().map_err(|error| Error::io(&backup, error))
}

fn write_checked(path: &Path, original: Option<&[u8]>, contents: &[u8]) -> Result<()> {
    let current = read_optional_file(path)?;
    if current.as_deref() != original {
        return Err(Error::User(format!(
            "{} changed while zerdr was preparing the update; retry",
            path.display()
        )));
    }
    write_unchecked(path, contents)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|source| Error::Json {
        what: path.display().to_string(),
        source,
    })?;
    bytes.push(b'\n');
    write_unchecked(path, &bytes)
}

fn write_unchecked(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::User(format!("path has no parent: {}", path.display())))?;
    fs::create_dir_all(parent).map_err(|error| Error::io(parent, error))?;
    let temporary = parent.join(format!(".zerdr-{}.tmp", now_millis()));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| Error::io(&temporary, error))?;
    if let Ok(metadata) = fs::metadata(path) {
        fs::set_permissions(&temporary, metadata.permissions())
            .map_err(|error| Error::io(&temporary, error))?;
    }
    file.write_all(contents)
        .map_err(|error| Error::io(&temporary, error))?;
    file.sync_all()
        .map_err(|error| Error::io(&temporary, error))?;
    fs::rename(&temporary, path).map_err(|error| Error::io(path, error))
}

fn rollback_plugin(herdr: &Herdr, paths: &Paths, previous_manifest: Option<&[u8]>) {
    let manifest = paths.plugin_dir.join("herdr-plugin.toml");
    if let Some(previous_manifest) = previous_manifest {
        let _ = write_unchecked(&manifest, previous_manifest);
        let _ = herdr.plugin_link(&paths.plugin_dir);
    } else {
        let _ = herdr.plugin_uninstall();
        let _ = fs::remove_dir_all(&paths.plugin_dir);
    }
}

fn restore_optional(path: &Path, original: Option<&[u8]>) -> Result<()> {
    if let Some(original) = original {
        write_unchecked(path, original)
    } else if path.exists() {
        fs::remove_file(path).map_err(|error| Error::io(path, error))
    } else {
        Ok(())
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
