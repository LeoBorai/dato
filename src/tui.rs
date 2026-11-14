//! The interactive shell (litecli-style REPL).

pub mod interrupt;
pub mod session;
pub mod welcome;

use std::borrow::Cow;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use reedline::{
    EditCommand, FileBackedHistory, Prompt, PromptEditMode, PromptHistorySearch,
    PromptHistorySearchStatus, Signal,
};

use crate::cli::cmd::ask_password;
use crate::client::ConnectOptions;
use crate::editor::{self, external};
use crate::paths;
use crate::server::Server;
use crate::store::default_name;
use crate::ui::organisms::{ConnectWizard, NewConnection, Pager, too_wide};
use crate::ui::{server_color, term};
use session::{Output, Session};

/// Marks the end of the connection name; takes the server's color.
const DIVIDER: &str = "✱";
/// Ends the prompt; `...` marks the continuation lines of a statement.
const INDICATOR: &str = " ▶︎ ";
const MULTILINE_INDICATOR: &str = "... ";

struct SqlPrompt {
    name: String,
    /// `user@host:port` or the SQLite file.
    detail: Option<String>,
    server: Option<Server>,
}

impl SqlPrompt {
    /// `name ✱ detail`, the ✱ in the server's color.
    fn left(&self) -> String {
        let Some(detail) = &self.detail else {
            return self.name.clone();
        };
        let divider = match self.server {
            Some(s) => server_color(s).bold().paint(DIVIDER).to_string(),
            None => DIVIDER.to_string(),
        };
        format!("{} {divider} {detail}", self.name)
    }
}

impl Prompt for SqlPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Owned(self.left())
    }
    /// The left side carries its own colors.
    fn get_prompt_color(&self) -> reedline::Color {
        reedline::Color::Default
    }
    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }
    fn render_prompt_indicator(&self, _mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed(INDICATOR)
    }
    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed(MULTILINE_INDICATOR)
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
/// Whether to color messages on stderr: it is a terminal and `NO_COLOR` is not set.
pub fn use_color() -> bool {
    std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

/// `⚠ Error: message`, in red when `color`.
pub fn error_line(message: impl std::fmt::Display, color: bool) -> String {
    let text = format!("⚠ Error: {message}");
    if color {
        nu_ansi_term::Color::Red.paint(text).to_string()
    } else {
        text
    }
}

pub fn emit(outputs: &[Output], out: &mut dyn Write, err: &mut dyn Write, color: bool) -> bool {
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
                let _ = writeln!(err, "{}", error_line(e, color));
            }
            Output::Quit => quit = true,
            // Connecting needs a terminal; the shell handles these before emitting.
            Output::ConnectWizard | Output::Connect(_) => {
                let _ = writeln!(err, "{}", error_line(NOT_INTERACTIVE, color));
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
    name: Option<String>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> bool {
    if let Err(e) = ask_password(opts) {
        let _ = writeln!(
            err,
            "{}",
            error_line(format!("cannot read password: {e}"), use_color())
        );
        return false;
    }
    match session.connect(opts) {
        Ok(()) => {
            if let Some(name) = name {
                session.set_connection_name(name);
            }
            let _ = writeln!(
                out,
                "Connected to {} ({}).",
                session.prompt_label(),
                opts.server()
            );
            true
        }
        Err(e) => {
            let _ = writeln!(err, "{}", error_line(e, use_color()));
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
                let _ = writeln!(
                    err,
                    "{}",
                    error_line(format!("cannot run the connect wizard: {e}"), use_color())
                );
                return;
            }
        },
    };
    let name = wizard
        .as_ref()
        .map(|n| n.name.clone().unwrap_or_else(|| default_name(&n.opts)));
    if establish(session, &mut opts, name, out, err)
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
        let (name, detail) = session.prompt_parts();
        let prompt = SqlPrompt {
            name,
            detail,
            server: session.server(),
        };
        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(buffer)) => {
                if let Some(rest) = external::strip_edit_suffix(&buffer) {
                    match external::edit(rest.trim_end()) {
                        Ok(text) => {
                            line_editor.run_edit_commands(&[EditCommand::InsertString(text)]);
                        }
                        Err(e) => eprintln!(
                            "{}",
                            error_line(format!("cannot run editor: {e}"), use_color())
                        ),
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
                        other => emit(&[other], &mut stdout, &mut stderr, use_color()),
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
                eprintln!("{}", error_line(e, use_color()));
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
            false,
        );
        assert!(quit);
        assert_eq!(String::from_utf8(out).unwrap(), "a\n1 row in set\n");
        assert_eq!(String::from_utf8(err).unwrap(), "⚠ Error: boom\n");
    }

    #[test]
    fn errors_are_red_only_with_color() {
        assert_eq!(error_line("boom", false), "⚠ Error: boom");
        let red = error_line("boom", true);
        assert!(
            red.starts_with("\x1b[31m") && red.contains("⚠ Error: boom"),
            "{red:?}"
        );
        let mut err = Vec::new();
        emit(
            &[Output::Error("boom".into())],
            &mut Vec::new(),
            &mut err,
            true,
        );
        assert_eq!(String::from_utf8(err).unwrap(), format!("{red}\n"));
    }

    #[test]
    fn prompt_has_the_name_a_colored_divider_and_the_details() {
        let prompt = |server| SqlPrompt {
            name: "shopify".into(),
            detail: Some("app@localhost:5432".into()),
            server,
        };
        let strip = |s: String| {
            let mut out = String::new();
            let mut esc = false;
            for c in s.chars() {
                match (esc, c) {
                    (_, '\x1b') => esc = true,
                    (true, 'm') => esc = false,
                    (false, c) => out.push(c),
                    _ => {}
                }
            }
            out
        };
        let colored = prompt(Some(Server::PostgreSql)).left();
        assert!(colored.contains("\x1b["), "{colored:?}");
        assert_eq!(strip(colored), "shopify ✱ app@localhost:5432");
        assert_eq!(prompt(None).left(), "shopify ✱ app@localhost:5432");
        let none = SqlPrompt {
            name: "(none)".into(),
            detail: None,
            server: None,
        };
        assert_eq!(none.left(), "(none)");
        assert_eq!(INDICATOR, " ▶\u{fe0e} ");
    }

    #[test]
    fn history_lives_in_dot_dato() {
        if let Some(p) = history_path() {
            assert!(p.ends_with(".dato/history"));
        }
    }
}
