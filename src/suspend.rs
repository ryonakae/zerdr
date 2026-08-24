//! `zerdr detach` / `zerdr attach`: suspend and resume every thread's Herdr attach.
//!
//! A direct attach client pins its pane's PTY to the thread terminal's size, which
//! breaks the session for differently sized clients (a phone over SSH). Both commands
//! only touch zerdr's own state files and wait for the live `zerdr connect` processes
//! to confirm through their lease markers, so they work from any local shell,
//! including an SSH session on the same machine.

use std::thread;
use std::time::{Duration, Instant};

use crate::error::{Error, Result};
use crate::state::{
    Paths, ThreadLeaseScan, ThreadLeaseSet, thread_detach_active, thread_detach_clear,
    thread_detach_set,
};

const DEFAULT_WAIT_MS: u64 = 5_000;
const POLL_MS: u64 = 50;

pub fn detach() -> Result<()> {
    let (paths, leases) = stores()?;
    thread_detach_set(&paths)?;
    let (scan, settled) = wait(&leases, |scan| scan.detached >= scan.live)?;
    if !settled {
        return Err(pending_error(
            scan.live - scan.detached,
            "confirm the detach",
        ));
    }
    if scan.live == 0 {
        println!("zerdr: no live threads; new threads will start detached");
    } else {
        println!("zerdr: detached {} thread(s)", scan.live);
    }
    Ok(())
}

pub fn attach() -> Result<()> {
    let (paths, leases) = stores()?;
    let initial = leases.scan_all()?;
    if !thread_detach_active(&paths) && initial.detached == 0 {
        println!("zerdr: detach mode is not active");
        return Ok(());
    }
    thread_detach_clear(&paths)?;
    let (scan, settled) = wait(&leases, |scan| scan.detached == 0)?;
    if !settled {
        return Err(pending_error(scan.detached, "reattach"));
    }
    if initial.detached == 0 {
        println!("zerdr: detach mode is off; no threads were waiting");
    } else {
        println!("zerdr: reattached {} thread(s)", initial.detached);
    }
    Ok(())
}

fn stores() -> Result<(Paths, ThreadLeaseSet)> {
    let paths = Paths::discover()?;
    let leases = ThreadLeaseSet::new(paths.thread_leases_dir.clone());
    Ok((paths, leases))
}

/// Polls the lease scan until `settled` or the wait budget runs out. Returns the
/// final scan and whether it settled, so callers can report what is still pending.
fn wait(
    leases: &ThreadLeaseSet,
    settled: impl Fn(&ThreadLeaseScan) -> bool,
) -> Result<(ThreadLeaseScan, bool)> {
    let deadline = Instant::now() + wait_budget();
    loop {
        let scan = leases.scan_all()?;
        if settled(&scan) {
            return Ok((scan, true));
        }
        if Instant::now() >= deadline {
            return Ok((scan, false));
        }
        thread::sleep(Duration::from_millis(POLL_MS));
    }
}

fn pending_error(pending: usize, action: &str) -> Error {
    Error::User(format!(
        "{pending} thread(s) did not {action} within {} ms; rerun once they settle",
        wait_budget().as_millis()
    ))
}

fn wait_budget() -> Duration {
    Duration::from_millis(
        std::env::var("ZERDR_DETACH_WAIT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_WAIT_MS),
    )
}
