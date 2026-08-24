# Repository guide

`zerdr` is a Rust CLI that keeps the focused Herdr workspace aligned with its Git checkout in Zed.

## Common commands

The repository pins Rust 1.93.1 in both `rust-toolchain.toml` and `mise.toml`.

```bash
cargo build --locked
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Run a focused integration test while changing one subsystem:

```bash
cargo test --test cli_contract
cargo test --test herdr_wrapper
cargo test --test setup_and_doctor
cargo test --test state_and_bindings
cargo test --test sync_flow
cargo test --test thread_flow
```

Use `cargo run --locked -- --help` to inspect the public CLI without invoking an integration command.

## Required validation

For Rust changes, run the closest integration test first, then the three CI checks in this order:

1. `cargo fmt --all -- --check`
2. `cargo clippy --all-targets --all-features -- -D warnings`
3. `cargo test --all-targets --all-features`

CI runs these checks on macOS and Ubuntu. Platform-specific behavior needs coverage for both platforms where applicable.

## Repository map

- `src/cli.rs` defines public commands and launch options; `src/lib.rs` dispatches them.
- `src/runtime.rs` resolves local versus remote execution and the wrapper anchor.
- `src/herdr.rs` wraps Herdr JSON commands and owns the child process lifecycle.
- `src/sync.rs` maps focused workspaces to Git roots and routes them into Zed.
- `src/thread.rs` backs `zerdr connect`: it resolves and attaches Zed terminal threads to Herdr agents, starts a not-running named session headless under `--create`, and mirrors agent titles and bells into the threads sidebar.
- `src/state.rs` owns bindings, route schemas, leases, locks, and atomic persistence.
- `src/setup.rs` merges the Herdr plugin and Zed tasks into user configuration, and owns auto mode: `zerdr setup auto on` installs `agent.terminal_init_command` with a recorded fingerprint that setup preserves and uninstall consumes.
- `src/doctor.rs` checks capabilities, installation state, bindings, routes, and leases.
- `src/zed.rs` wraps the Zed CLI.
- `assets/herdr/` and `assets/zed/` contain templates embedded by `setup`.
- `tests/support/mod.rs` provides isolated fake `herdr` and `zed` executables; `FAKE_HERDR_BODY` is shared by the `PATH` fake and the private fakes from `TestEnv::baked_herdr`.
- `docs/plans/` records historical implementation plans. Treat current code and tests as the source of truth.

## Code and test conventions

- Add or update an integration test before changing observable behavior.
- Keep CLI contract changes in `src/cli.rs` and `tests/cli_contract.rs` aligned.
- Keep external process calls behind the Herdr and Zed adapters so tests can use fake binaries.
- Preserve one-live-wrapper ownership checks when changing routes, leases, or manual commands.
- Preserve backward compatibility for persisted state, or add an explicit migration and tests.
- Keep setup merges ownership-aware. Never overwrite foreign or user-modified Zed tasks or settings, and back up a user-owned Zed file before mutating it.
- Keep platform and remote-environment decisions in `src/runtime.rs` rather than scattering environment checks.
- `sync-from-herdr` is a hidden plugin entry point, not a public user command.

## Safety and workflow

- Do not run `zerdr setup install`, `zerdr setup uninstall`, or local `zerdr setup doctor` against the developer's real environment during automated validation. They can modify Herdr plugins, global Zed tasks, or zerdr state.
- Integration tests that invoke Herdr, Zed, or environment-facing commands must use `TestEnv`, which sets `ZERDR_TEST_ROOT` and points zerdr at fake binaries. Pure state tests may use `tempfile` directly.
- Keep the shared fake `herdr` on `PATH` free of per-invocation setup. Wrapper tests budget roughly two seconds for a multi-process dance, so even one extra command that runs on every call makes them fail under parallel load. Bake per-test configuration into a private fake with `TestEnv::baked_herdr` instead.
- `zerdr setup uninstall --purge` recursively removes zerdr state and data after checking live leases. Cover purge changes with isolated tests.
- Do not create release tags, GitHub releases, or Homebrew tap commits as part of normal development. `.github/workflows/release.yml` owns publication.
- Do not edit `target/`; Cargo generates it.
- Keep `README.md` focused on people installing and using zerdr. Put maintainer commands and repository structure here.
