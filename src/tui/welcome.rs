//! The welcome screen shown when `dato` starts without a target.

use std::io::{Write, stdout};
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, execute};

use super::session::Session;
use super::{error_line, establish, term, use_color};
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

/// Centers the connect wizard like the welcome screen.
fn centered(wizard: ConnectWizard) -> ConnectWizard {
    match crossterm::terminal::size() {
        Ok((columns, rows)) => wizard.centered(columns as usize, rows as usize),
        Err(_) => wizard,
    }
}

/// Runs the wizard on the saved connection at `i` and saves the changes,
/// without connecting. A cancelled wizard changes nothing.
fn edit(store: &mut Store, i: usize) -> std::io::Result<()> {
    let original = store.items()[i].clone();
    let old = original.name.clone();
    let taken = store
        .items()
        .iter()
        .map(|c| c.name.clone())
        .filter(|n| *n != old)
        .collect();
    let mut wizard = centered(ConnectWizard::edit(original, taken));
    let Some(new) = term::run(&mut wizard)? else {
        return Ok(());
    };
    let name = new.name.as_deref().unwrap_or(&old);
    store.update(&old, name, &new.opts, new.tag, new.save_password);
    store.set_safeguard(name, new.safeguard);
    store.save()
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
                tag: c.tag,
            })
            .collect();
        let choice = match term::run(&mut welcome_screen(items)) {
            Ok(Some(c)) => c,
            Ok(None) => return,
            Err(e) => {
                let _ = writeln!(
                    err,
                    "{}",
                    error_line(format!("cannot show the welcome screen: {e}"), use_color())
                );
                return;
            }
        };
        let (name, mut opts, safeguard, wizard) = match choice {
            WelcomeChoice::Saved(i) => {
                let c = &store.items()[i];
                (c.name.clone(), c.opts.clone(), c.safeguard, None)
            }
            WelcomeChoice::Edit(i) => {
                let _ = execute!(out, Clear(ClearType::All), cursor::MoveTo(0, 0));
                if let Err(e) = edit(&mut store, i) {
                    let _ = writeln!(
                        err,
                        "{}",
                        error_line(format!("cannot edit the connection: {e}"), use_color())
                    );
                    let _ = writeln!(err, "Press Enter to go back.");
                    let _ = std::io::stdin().read_line(&mut String::new());
                }
                continue;
            }
            WelcomeChoice::New => {
                let _ = execute!(out, Clear(ClearType::All), cursor::MoveTo(0, 0));
                match term::run(&mut centered(ConnectWizard::new())) {
                    Ok(Some(new)) => {
                        let name = new.name.clone().unwrap_or_else(|| default_name(&new.opts));
                        let save = Some((new.tag, new.save_password));
                        (name, new.opts, new.safeguard, save)
                    }
                    Ok(None) => continue,
                    Err(e) => {
                        let _ = writeln!(
                            err,
                            "{}",
                            error_line(format!("cannot run the connect wizard: {e}"), use_color())
                        );
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
        session.set_safeguard(safeguard);
        // Leave the shell at the top of a clean screen.
        let _ = execute!(out, Clear(ClearType::All), cursor::MoveTo(0, 0));
        match wizard {
            Some((tag, keep)) => {
                store.remember(&name, &opts, tag, keep, now());
                store.set_safeguard(&name, safeguard);
            }
            None => store.touch(&name, now()),
        }
        if let Err(e) = store.save() {
            let _ = writeln!(err, "warning: cannot save the connection: {e}");
        }
        return;
    }
}
