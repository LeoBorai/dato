//! The interactive shell (litecli-style REPL).

pub mod interrupt;
pub mod session;
pub mod welcome;

use std::borrow::Cow;
use std::io::Write;
use std::path::PathBuf;

use reedline::{
    EditCommand, FileBackedHistory, Prompt, PromptEditMode, PromptHistorySearch,
    PromptHistorySearchStatus, Signal,
};

use crate::cli::cmd::ask_password;
use crate::client::ConnectOptions;
use crate::editor::{self, external};
use crate::paths;
use crate::store::default_name;
use crate::ui::organisms::{ConnectWizard, NewConnection, Pager, too_wide};
use crate::ui::term;
use session::{Output, Session};

struct SqlPrompt {
    name: String,
}

impl Prompt for SqlPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Owned(self.name.clone())
    }
    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }
    fn render_prompt_indicator(&self, _mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("> ")
    }
    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("... ")
    }
    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "failing ",
        };
        Cow::Owned(format!(
            "({prefix}reverse-search: {}) ",
            history_search.term
        ))
    }
}

/// Error for `\\connect` outside the interactive shell.
pub const NOT_INTERACTIVE: &str = "\\connect is only available in the interactive shell";

/// Shell history: `~/.dato/history`.
pub fn history_path() -> Option<PathBuf> {
    paths::history_file()
}

/// Prints outputs: text and status to stdout, errors to stderr.
/// Returns `true` when a quit was requested.
pub fn emit(outputs: &[Output], out: &mut dyn Write, err: &mut dyn Write) -> bool {
    let mut quit = false;
    for o in outputs {
        match o {
            Output::Text(t) => {
                let _ = out.write_all(t.as_bytes());
                if !t.ends_with('\n') {
                    let _ = out.write_all(b"\n");
                }
            }
            Output::Status(s) => {
                let _ = writeln!(out, "{s}");
            }
            Output::Error(e) => {
                let _ = writeln!(err, "Error: {e}");
            }
            Output::Quit => quit = true,
            // Connecting needs a terminal; the shell handles these before emitting.
            Output::ConnectWizard | Output::Connect(_) => {
                let _ = writeln!(err, "Error: {NOT_INTERACTIVE}");
            }
        }
    }
    let _ = out.flush();
    quit
}

/// Shows `text` in the pager when it is wider than the terminal, so rows stay
/// aligned and Left/Right scroll through the columns. Returns `false` (and shows
/// nothing) when the text fits or stdout is not a terminal; the caller prints it.
fn page(text: &str) -> bool {
    use std::io::IsTerminal;
    if !std::io::stdout().is_terminal() {
        return false;
    }
    let Ok((columns, rows)) = crossterm::terminal::size() else {
        return false;
    };
    if !too_wide(text, columns as usize) {
        return false;
    }
    // No pager (e.g. raw mode unavailable): fall back to plain output.
    term::run(&mut Pager::new(text, columns as usize, rows as usize)).is_ok()
}

/// Asks for the password when the options need one, connects and reports the
/// outcome. A failed connect keeps the current connection. Returns whether it
/// connected.
fn establish(
    session: &mut Session,
    opts: &mut ConnectOptions,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> bool {
    if let Err(e) = ask_password(opts) {
        let _ = writeln!(err, "Error: cannot read password: {e}");
        return false;
    }
    match session.connect(opts) {
        Ok(()) => {
            let _ = writeln!(
                out,
                "Connected to {} ({}).",
                session.prompt_label(),
                opts.server()
            );
            true
        }
        Err(e) => {
            let _ = writeln!(err, "Error: {e}");
            false
        }
    }
}

/// Saves a connection made with the wizard under its name (or a default one).
fn remember(new: &NewConnection, opts: &ConnectOptions, err: &mut dyn Write) {
    let mut store = welcome::open_store(err);
    let name = new.name.clone().unwrap_or_else(|| default_name(opts));
    store.remember(&name, opts, new.save_password, welcome::now());
    if let Err(e) = store.save() {
        let _ = writeln!(err, "warning: cannot save the connection: {e}");
    }
}

/// Handles `\\connect`: runs the wizard when `opts` is `None`, asks for the
/// password when the options need one, then replaces the session's connection.
/// A cancelled wizard or a failed connect keeps the current connection. A
/// connection made with the wizard is saved for the welcome screen.
fn connect(
    session: &mut Session,
    opts: Option<ConnectOptions>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) {
    let (mut opts, wizard) = match opts {
        Some(o) => (o, None),
        None => match term::run(&mut ConnectWizard::new()) {
            Ok(Some(new)) => (new.opts.clone(), Some(new)),
            Ok(None) => {
                let _ = writeln!(out, "Connect cancelled.");
                return;
            }
            Err(e) => {
                let _ = writeln!(err, "Error: cannot run the connect wizard: {e}");
                return;
            }
        },
    };
    if establish(session, &mut opts, out, err)
        && let Some(new) = wizard
    {
        remember(&new, &opts, err);
    }
}

/// Runs the interactive shell until the user quits. Returns the process exit code.
pub fn run(mut session: Session) -> i32 {
    interrupt::install();
    let mut line_editor = editor::build(session.schema(), session.dialect());
    match paths::ensure_app_dir() {
        Ok(_) => {
            paths::migrate_legacy_history();
            if let Some(path) = history_path() {
                match FileBackedHistory::with_file(1000, path) {
                    Ok(h) => line_editor = line_editor.with_history(Box::new(h)),
                    Err(e) => eprintln!("warning: history disabled: {e}"),
                }
            }
        }
        Err(e) => eprintln!("warning: history disabled: cannot create ~/.dato: {e}"),
    }
    let (mut stdout, mut stderr) = (std::io::stdout(), std::io::stderr());

    loop {
        // Rebuilt each time: `\\connect` and MySQL `USE` change the label.
        let prompt = SqlPrompt {
            name: session.prompt_label(),
        };
        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(buffer)) => {
                if let Some(rest) = external::strip_edit_suffix(&buffer) {
                    match external::edit(rest.trim_end()) {
                        Ok(text) => {
                            line_editor.run_edit_commands(&[EditCommand::InsertString(text)]);
                        }
                        Err(e) => eprintln!("Error: cannot run editor: {e}"),
                    }
                    continue;
                }
                for output in session.handle(&buffer) {
                    let quit = match output {
                        Output::ConnectWizard => {
                            connect(&mut session, None, &mut stdout, &mut stderr);
                            false
                        }
                        Output::Connect(opts) => {
                            connect(&mut session, Some(opts), &mut stdout, &mut stderr);
                            false
                        }
                        Output::Text(text) if page(&text) => false,
                        other => emit(&[other], &mut stdout, &mut stderr),
                    };
                    if quit {
                        return 0;
                    }
                }
            }
            Ok(Signal::CtrlC(_)) => {}
            Ok(Signal::CtrlD) => return 0,
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error: {e}");
                return 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_routes_streams_and_detects_quit() {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let quit = emit(
            &[
                Output::Text("a".into()),
                Output::Status("1 row in set".into()),
                Output::Error("boom".into()),
                Output::Quit,
            ],
            &mut out,
            &mut err,
        );
        assert!(quit);
        assert_eq!(String::from_utf8(out).unwrap(), "a\n1 row in set\n");
        assert_eq!(String::from_utf8(err).unwrap(), "Error: boom\n");
    }

    #[test]
    fn history_lives_in_dot_dato() {
        if let Some(p) = history_path() {
            assert!(p.ends_with(".dato/history"));
        }
    }
}
