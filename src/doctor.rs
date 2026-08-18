use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use jsonc_parser::ParseOptions;
use jsonc_parser::cst::CstRootNode;
use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::herdr::Herdr;
use crate::setup::{InstallState, fingerprint, generated_tasks, load_install_state, owned_labels};
use crate::state::{BindingStore, LeaseSet, LifecycleGuard, Paths, RouteStore, canonical_git_root};
use crate::zed::Zed;

pub fn doctor() -> Result<()> {
    let paths = Paths::discover()?;
    let mut report = Report::default();
    report.pass(format!("state directory: {}", paths.state_dir.display()));
    report.pass(format!("data directory: {}", paths.data_dir.display()));
    report.pass(format!(
        "Zed tasks file: {}",
        paths.zed_tasks_file.display()
    ));

    let herdr = Herdr::from_env();
    match herdr.version() {
        Ok(version) if version.contains("0.8.0") => {
            report.pass(format!("Herdr baseline available: {version}"));
        }
        Ok(version) => report.warn(format!(
            "Herdr version differs from verified 0.8.0 baseline: {version}"
        )),
        Err(error) => report.fail(format!("Herdr executable is unavailable: {error}")),
    }

    match herdr.plugin_list() {
        Ok(value) if plugin_is_compatible(&value) => {
            report.pass("Herdr zerdr plugin is enabled for workspace.focused");
        }
        Ok(_) => report.fail(
            "Herdr zerdr plugin is missing, disabled, or lacks workspace.focused; run `zerdr setup`",
        ),
        Err(error) => report.fail(format!("could not inspect Herdr plugins: {error}")),
    }

    match Zed::from_env().supports_existing_and_add() {
        Ok(true) => report.pass("Zed supports --existing and --add"),
        Ok(false) => {
            report.fail("Zed does not expose --existing and --add; install Zed 1.15.0 or newer")
        }
        Err(error) => report.fail(format!("could not inspect the Zed CLI: {error}")),
    }

    let install = match load_install_state(&paths.install_state_file) {
        Ok(Some(install)) => Some(install),
        Ok(None) => {
            report.fail("zerdr install ownership state is missing; run `zerdr setup`");
            None
        }
        Err(error) => {
            report.fail(format!("zerdr install ownership state is invalid: {error}"));
            None
        }
    };
    if let Some(install) = install.as_ref() {
        if is_executable(&install.executable) {
            report.pass(format!(
                "installed zerdr executable exists: {}",
                install.executable.display()
            ));
        } else {
            report.fail(format!(
                "installed zerdr executable is missing: {}; rerun `zerdr setup` from the installed binary",
                install.executable.display()
            ));
        }
        match inspect_manifest(&paths, install) {
            Ok(()) => report.pass("generated Herdr manifest command is compatible"),
            Err(error) => report.fail(error.to_string()),
        }
        match inspect_tasks(&paths, install) {
            Ok(()) => report.pass("all five owned Zed task payloads are valid"),
            Err(error) => report.fail(error.to_string()),
        }
    }

    match BindingStore::new(paths.bindings_file.clone()).load() {
        Ok(state) => {
            let mut missing = false;
            for (workspace, root) in state.bindings {
                match canonical_git_root(&root) {
                    Ok(canonical) if canonical == root => {}
                    Ok(canonical) => {
                        missing = true;
                        report.fail(format!(
                            "binding {workspace} is not canonical: {} resolves to {}; run `zerdr bind PATH`",
                            root.display(),
                            canonical.display()
                        ));
                    }
                    Err(error) => {
                        missing = true;
                        report.fail(format!(
                            "binding {workspace} is not a valid Git checkout root: {error}"
                        ));
                    }
                }
            }
            if !missing {
                report.pass("binding state schema and roots are valid");
            }
        }
        Err(error) => report.fail(format!("binding state is invalid: {error}")),
    }

    let _lifecycle = LifecycleGuard::acquire(&paths.lifecycle_lock_file)?;
    let leases = LeaseSet::new(paths.leases_dir);
    let routes = RouteStore::new(paths.routes_dir);
    let lease_sweep = match leases.sweep_all() {
        Ok(sweep) => {
            if sweep.stale_removed > 0 {
                report.warn(format!(
                    "removed {} stale lease file(s) across zerdr socket scopes",
                    sweep.stale_removed
                ));
            }
            Some(sweep)
        }
        Err(error) => {
            report.fail(format!("could not inspect zerdr lease scopes: {error}"));
            None
        }
    };
    if let Some(sweep) = lease_sweep.as_ref() {
        match routes.remove_stale_except(&sweep.live_scope_hashes) {
            Ok(0) => {}
            Ok(count) => report.warn(format!("removed {count} stale route file(s)")),
            Err(error) => report.fail(format!("could not remove stale route state: {error}")),
        }
    }
    match herdr.session_socket() {
        Ok(socket) => match leases.inspect(&socket) {
            Ok(inspection) => {
                if inspection.stale_removed > 0 {
                    report.warn(format!(
                        "removed {} stale lease file(s) for {}",
                        inspection.stale_removed,
                        socket.display()
                    ));
                }
                if let Some(sweep) = lease_sweep.as_ref()
                    && sweep.live_count != inspection.live_wrapper_pids.len()
                {
                    report.fail(format!(
                        "found {} live lease(s) outside the current zerdr session socket",
                        sweep.live_count - inspection.live_wrapper_pids.len()
                    ));
                }
                match inspection.live_wrapper_pids.as_slice() {
                    [] => report.warn(
                        "zerdr session has no live Zed client; run `zerdr herdr --anchor PATH` in Zed",
                    ),
                    [wrapper_pid] => match routes.load(&socket) {
                        Ok(route) if route.wrapper_pid == *wrapper_pid => {
                            report.pass(format!(
                                "zerdr session has one live wrapper: {}",
                                socket.display()
                            ));
                            report.pass(format!(
                                "route anchor is valid: {}",
                                route.anchor_root.display()
                            ));
                        }
                        Ok(route) => report.fail(format!(
                            "route belongs to wrapper {}, but live wrapper is {}; restart `zerdr: Herdr`",
                            route.wrapper_pid, wrapper_pid
                        )),
                        Err(error) => report.fail(format!(
                            "live wrapper route state is invalid: {error}; restart `zerdr: Herdr`"
                        )),
                    },
                    wrapper_pids => report.fail(format!(
                        "zerdr session has {} live wrappers ({wrapper_pids:?}); keep only one `zerdr: Herdr` task",
                        wrapper_pids.len()
                    )),
                }
            }
            Err(error) => report.fail(format!("lease state is invalid: {error}")),
        },
        Err(_) if lease_sweep.as_ref().is_some_and(|sweep| sweep.live_count > 0) => report.fail(
            "zerdr has live lease state but the Herdr session socket is unavailable; stop the stale wrapper or remove its state",
        ),
        Err(_) => report.warn("zerdr Herdr session is not running"),
    }

    report.finish()
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn plugin_is_compatible(value: &Value) -> bool {
    let plugins = value
        .pointer("/result/plugins")
        .or_else(|| value.get("plugins"))
        .and_then(Value::as_array);
    plugins.is_some_and(|plugins| {
        plugins.iter().any(|plugin| {
            plugin.get("plugin_id").and_then(Value::as_str) == Some("zerdr")
                && plugin
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                && plugin
                    .get("events")
                    .and_then(Value::as_array)
                    .is_some_and(|events| {
                        events.iter().any(|event| {
                            event.get("on").and_then(Value::as_str) == Some("workspace.focused")
                        })
                    })
        })
    })
}

fn inspect_manifest(paths: &Paths, install: &InstallState) -> Result<()> {
    let path = paths.plugin_dir.join("herdr-plugin.toml");
    let text = fs::read_to_string(&path).map_err(|error| Error::io(&path, error))?;
    let manifest: PluginManifest = toml::from_str(&text)
        .map_err(|error| Error::User(format!("generated Herdr manifest is invalid: {error}")))?;
    let expected_command = vec![
        install.executable.display().to_string(),
        "sync-from-herdr".to_owned(),
    ];
    let compatible = manifest.id == "zerdr"
        && manifest.min_herdr_version == "0.8.0"
        && manifest.events.len() == 1
        && manifest.events[0].on == "workspace.focused"
        && manifest.events[0].command == expected_command;
    if compatible {
        Ok(())
    } else {
        Err(Error::User(format!(
            "generated Herdr manifest has the wrong event command; run `zerdr setup` ({})",
            path.display()
        )))
    }
}

fn inspect_tasks(paths: &Paths, install: &InstallState) -> Result<()> {
    let text = fs::read_to_string(&paths.zed_tasks_file)
        .map_err(|error| Error::io(&paths.zed_tasks_file, error))?;
    let root = CstRootNode::parse(&text, &ParseOptions::default())
        .map_err(|error| Error::User(format!("Zed tasks JSONC is invalid: {error}")))?;
    let array = root
        .array_value()
        .ok_or_else(|| Error::User("Zed tasks file must contain a top-level array".to_owned()))?;
    let values = array
        .elements()
        .into_iter()
        .filter_map(|element| element.to_serde_value())
        .collect::<Vec<_>>();
    let expected = generated_tasks(&install.executable)?;
    if install.task_fingerprints.len() != owned_labels().len() {
        return Err(Error::User(
            "zerdr task ownership state does not contain exactly five tasks; run `zerdr setup`"
                .to_owned(),
        ));
    }
    for expected_task in expected {
        let label = expected_task["label"]
            .as_str()
            .expect("embedded task label");
        let matches = values
            .iter()
            .filter(|value| value.get("label").and_then(Value::as_str) == Some(label))
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(Error::User(format!(
                "Zed task {label:?} is missing or duplicated; run `zerdr setup`"
            )));
        }
        let current = fingerprint(matches[0]);
        let recorded = install.task_fingerprints.get(label);
        let expected = fingerprint(&expected_task);
        if recorded != Some(&current) || current != expected {
            return Err(Error::User(format!(
                "Zed task payload {label:?} was modified or has the wrong command; run `zerdr setup`"
            )));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct PluginManifest {
    id: String,
    min_herdr_version: String,
    events: Vec<PluginEvent>,
}

#[derive(Deserialize)]
struct PluginEvent {
    on: String,
    command: Vec<String>,
}

#[derive(Default)]
struct Report {
    failures: usize,
}

impl Report {
    fn pass(&self, message: impl AsRef<str>) {
        println!("PASS {}", message.as_ref());
    }

    fn warn(&self, message: impl AsRef<str>) {
        println!("WARN {}", message.as_ref());
    }

    fn fail(&mut self, message: impl AsRef<str>) {
        self.failures += 1;
        println!("FAIL {}", message.as_ref());
    }

    fn finish(self) -> Result<()> {
        if self.failures == 0 {
            Ok(())
        } else {
            Err(Error::User(format!(
                "doctor found {} blocking problem(s)",
                self.failures
            )))
        }
    }
}
