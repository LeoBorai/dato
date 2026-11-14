//! The welcome screen shown when `dato` starts without a target.

use std::io::{Write, stdout};
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, execute};

use super::session::Session;
use super::{establish, term};
use crate::paths;
use crate::store::{Store, default_name};
use crate::ui::organisms::{ConnectWizard, Welcome, WelcomeChoice, WelcomeItem};

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Loads the saved connections. An unreadable file is set aside as
/// `connections.json.bak` so saving never overwrites it.
pub fn open_store(err: &mut dyn Write) -> Store {
    let Some(path) = paths::connections_file() else {
        return Store::empty(None);
    };
    match Store::load(path.clone()) {
        Ok(store) => store,
        Err(e) => {
            let backup = path.with_extension("json.bak");
            let _ = writeln!(
                err,
                "warning: cannot read {}: {e}; moved to {}",
                path.display(),
                backup.display()
            );
            let _ = std::fs::rename(&path, &backup);
            Store::empty(Some(path))
        }
    }
}

/// The welcome screen, centered in the terminal when its size is known.
fn welcome_screen(items: Vec<WelcomeItem>) -> Welcome {
    let welcome = Welcome::new(items);
    match crossterm::terminal::size() {
        Ok((columns, rows)) => welcome.centered(columns as usize, rows as usize),
        Err(_) => welcome,
    }
}

/// The connect wizard, centered like the welcome screen.
fn wizard_screen() -> ConnectWizard {
    let wizard = ConnectWizard::new();
    match crossterm::terminal::size() {
        Ok((columns, rows)) => wizard.centered(columns as usize, rows as usize),
        Err(_) => wizard,
    }
}

/// Shows the welcome screen until a connection is made or the user presses
/// Esc, which leaves the session disconnected. A failed connection returns to
/// the screen.
pub fn run(session: &mut Session) {
    let (mut out, mut err) = (stdout(), std::io::stderr());
    let mut store = open_store(&mut err);
    loop {
        let _ = execute!(out, Clear(ClearType::All), cursor::MoveTo(0, 0));
        let items = store
            .items()
            .iter()
            .map(|c| WelcomeItem {
                name: c.name.clone(),
                detail: c.detail(),
                server: c.server(),
            })
            .collect();
        let choice = match term::run(&mut welcome_screen(items)) {
            Ok(Some(c)) => c,
            Ok(None) => return,
            Err(e) => {
                let _ = writeln!(err, "Error: cannot show the welcome screen: {e}");
                return;
            }
        };
        let (name, mut opts, save_password) = match choice {
            WelcomeChoice::Saved(i) => {
                let c = &store.items()[i];
                (c.name.clone(), c.opts.clone(), None)
            }
            WelcomeChoice::New => {
                let _ = execute!(out, Clear(ClearType::All), cursor::MoveTo(0, 0));
                match term::run(&mut wizard_screen()) {
                    Ok(Some(new)) => {
                        let name = new.name.clone().unwrap_or_else(|| default_name(&new.opts));
                        (name, new.opts, Some(new.save_password))
                    }
                    Ok(None) => continue,
                    Err(e) => {
                        let _ = writeln!(err, "Error: cannot run the connect wizard: {e}");
                        return;
                    }
                }
            }
        };
        if !establish(session, &mut opts, Some(name.clone()), &mut out, &mut err) {
            let _ = writeln!(err, "Press Enter to go back.");
            let _ = std::io::stdin().read_line(&mut String::new());
            continue;
        }
        // Leave the shell at the top of a clean screen.
        let _ = execute!(out, Clear(ClearType::All), cursor::MoveTo(0, 0));
        match save_password {
            Some(keep) => store.remember(&name, &opts, keep, now()),
            None => store.touch(&name, now()),
        }
        if let Err(e) = store.save() {
            let _ = writeln!(err, "warning: cannot save the connection: {e}");
        }
        return;
    }
}
