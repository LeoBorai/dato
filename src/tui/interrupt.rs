//! Ctrl-C during query execution. While the REPL reads input the terminal is in raw
//! mode and Ctrl-C arrives as a key event; while a query runs it arrives as SIGINT,
//! which this module forwards to the active client.

use std::sync::{Arc, Mutex};

use crate::client::Interrupt;

static ACTIVE: Mutex<Option<Arc<dyn Interrupt>>> = Mutex::new(None);

/// Keeps the handle registered; clears it on drop.
pub struct Armed(());

pub fn arm(handle: Arc<dyn Interrupt>) -> Armed {
    *ACTIVE.lock().unwrap() = Some(handle);
    Armed(())
}

impl Drop for Armed {
    fn drop(&mut self) {
        *ACTIVE.lock().unwrap() = None;
    }
}

/// Interrupts the running statement, if any.
pub fn trigger() {
    if let Some(h) = ACTIVE.lock().unwrap().as_ref() {
        h.interrupt();
    }
}

/// Installs the process-wide Ctrl-C handler. Safe to call more than once.
pub fn install() {
    let _ = ctrlc::set_handler(trigger);
}
