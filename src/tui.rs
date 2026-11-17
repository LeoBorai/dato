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
use crate::store::default_name;
use crate::ui::organisms::{ConnectWizard, NewConnection, Pager, too_wide};
use crate::ui::{server_color, server_title, term};
use nu_ansi_term::{Color, Style};
use session::{Output, PromptInfo, Session, Target};
use std::time::Duration;

/// Marks the end of the connection name; takes the server's color.
const DIVIDER: &str = "✱";
/// Ends the prompt; `...` marks the continuation lines of a statement.
const INDICATOR: &str = "▶︎ ";
const MULTILINE_INDICATOR: &str = "↩︎ ";

struct SqlPrompt {
    info: PromptInfo,
}

/// Shortest command time worth showing.
const SLOW: Duration = Duration::from_secs(2);

/// `2s`, `1m 5s`, `1h 2m`.
fn format_took(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, s % 3600 / 60),
    }
}

impl SqlPrompt {
    /// The prompt's own line, like `shopify ✱ app@localhost:5432 on shop via ● PostgreSQL took 2s`,
    /// or `shopify ⚠ Disconnected` after `\\disconnect`.
    /// The ✱ and ● take the server's color. The input goes on the next line,
    /// after the indicator.
    fn left(&self) -> String {
        let i = &self.info;
        let dim = dim();
        let accent = i.server.map(|s| Style::new().bold().fg(server_color(s)));
        let mut out = Style::new().bold().paint(&i.name).to_string();

        if let Some(detail) = &i.detail {
            let divider = accent.map_or(DIVIDER.to_string(), |a| a.paint(DIVIDER).to_string());
            out += &format!(" {divider} {detail}");
        }

        if i.disconnected {
            let yellow = Style::new().fg(Color::Yellow);
            out += &format!(
                " {} {}",
                yellow.paint("⚠"),
                yellow.underline().paint("Disconnected")
            );
        }

        if let Some(db) = &i.database {
            out += &format!(" {} {db}", dim.paint("on"));
        }

        if let Some(server) = i.server {
            let dot = Style::new().fg(server_color(server)).paint("●");
            out += &format!(" {} {dot} {}", dim.paint("via"), server_title(server));
        }

        if let Some(took) = i.took.filter(|t| *t >= SLOW) {
            out += &format!(" {} {}", dim.paint("took"), format_took(took));
        }

        out.push('\n');
        out
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

/// Whether to color messages on stderr: it is a terminal and `NO_COLOR` is not set.
pub fn use_color() -> bool {
    std::io::stderr().is_terminal() && no_color_unset()
}

fn no_color_unset() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

/// Which of the output streams get colors.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Colors {
    pub stdout: bool,
    pub stderr: bool,
}

impl Colors {
    /// Colors for each stream that is a terminal, unless `NO_COLOR` is set.
    pub fn detect() -> Self {
        Self {
            stdout: std::io::stdout().is_terminal() && no_color_unset(),
            stderr: use_color(),
        }
    }
}

/// The soft gray of secondary text: the `via` in the prompt and the row
/// count after a result.
pub fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
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

/// `<icon> <verb> <name> (<detail>)<end>` in `style`: the name in bold, the
/// detail in italics. Plain text when `color` is off.
fn target_line(
    style: Style,
    icon: &str,
    verb: &str,
    target: &Target,
    end: &str,
    color: bool,
) -> String {
    let detail = (!target.detail.is_empty()).then_some(target.detail.as_str());
    if !color {
        let detail = detail.map(|d| format!(" ({d})")).unwrap_or_default();
        return format!("{icon}{verb} {}{detail}{end}", target.name);
    }
    let mut line = style.paint(format!("{icon}{verb} ")).to_string();
    line += &style.bold().paint(&target.name).to_string();
    if let Some(d) = detail {
        line += &style.paint(" (").to_string();
        line += &style.italic().paint(d).to_string();
        line += &style.paint(")").to_string();
    }
    line + &style.paint(end).to_string()
}

/// `✓ Connected to name (detail).` in green.
pub fn connected_line(target: &Target, color: bool) -> String {
    let style = Style::new().fg(Color::Green);
    target_line(style, "✓ ", "Connected to", target, ".", color)
}

/// `⚠ Disconnected from name (detail).` in yellow.
pub fn disconnected_line(target: &Target, color: bool) -> String {
    let style = Style::new().fg(Color::Yellow);
    target_line(style, "⚠ ", "Disconnected from", target, ".", color)
}

/// `Reconnecting to name (detail)...` in gray.
pub fn reconnecting_line(target: &Target, color: bool) -> String {
    target_line(dim(), "", "Reconnecting to", target, "...", color)
}

/// Prints outputs: text and status to stdout, errors to stderr.
/// Returns `true` when a quit was requested.
pub fn emit(outputs: &[Output], out: &mut dyn Write, err: &mut dyn Write, colors: Colors) -> bool {
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
                if colors.stdout {
                    let _ = writeln!(out, "{}", dim().paint(s));
                } else {
                    let _ = writeln!(out, "{s}");
                }
            }
            Output::Disconnected(t) => {
                let _ = writeln!(out, "{}", disconnected_line(t, colors.stdout));
            }
            Output::Reconnecting(t) => {
                let _ = writeln!(out, "{}", reconnecting_line(t, colors.stdout));
            }
            Output::Error(e) => {
                let _ = writeln!(err, "{}", error_line(e, colors.stderr));
            }
            Output::Quit => quit = true,
            // Connecting needs a terminal; the shell handles these before emitting.
            Output::ConnectWizard | Output::Connect(_) => {
                let _ = writeln!(err, "{}", error_line(NOT_INTERACTIVE, colors.stderr));
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
            if let Some(target) = session.target() {
                let _ = writeln!(out, "{}", connected_line(&target, Colors::detect().stdout));
            }
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
        let prompt = SqlPrompt {
            info: session.prompt_info(),
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
                            connect(&mut session, Some(*opts), &mut stdout, &mut stderr);
                            false
                        }
                        Output::Text(text) if page(&text) => false,
                        other => emit(&[other], &mut stdout, &mut stderr, Colors::detect()),
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
    use crate::server::Server;

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
            Colors::default(),
        );
        assert!(quit);
        assert_eq!(String::from_utf8(out).unwrap(), "a\n1 row in set\n");
        assert_eq!(String::from_utf8(err).unwrap(), "⚠ Error: boom\n");
    }

    #[test]
    fn connection_lines_share_one_layout() {
        let t = Target {
            name: "shop".into(),
            detail: "app@db:5432".into(),
        };
        assert_eq!(
            connected_line(&t, false),
            "✓ Connected to shop (app@db:5432)."
        );
        assert_eq!(
            disconnected_line(&t, false),
            "⚠ Disconnected from shop (app@db:5432)."
        );
        assert_eq!(
            reconnecting_line(&t, false),
            "Reconnecting to shop (app@db:5432)..."
        );
        let green = connected_line(&t, true);
        assert!(green.contains("\x1b[32m"), "{green:?}");
        assert!(green.contains("\x1b[1;32mshop"), "{green:?}");
        assert!(green.contains("\x1b[3;32mapp@db:5432"), "{green:?}");
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
            Colors {
                stdout: false,
                stderr: true,
            },
        );
        assert_eq!(String::from_utf8(err).unwrap(), format!("{red}\n"));
    }

    #[test]
    fn status_line_is_soft_gray_only_with_color() {
        let status = [Output::Status("17 rows in set, 0.002s".into())];
        let mut out = Vec::new();
        emit(&status, &mut out, &mut Vec::new(), Colors::default());
        assert_eq!(String::from_utf8(out).unwrap(), "17 rows in set, 0.002s\n");
        let mut out = Vec::new();
        let colors = Colors {
            stdout: true,
            stderr: false,
        };
        emit(&status, &mut out, &mut Vec::new(), colors);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(text, format!("{}\n", dim().paint("17 rows in set, 0.002s")));
        assert!(text.starts_with("\x1b["), "{text:?}");
    }

    fn plain(s: &str) -> String {
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
    }

    fn info() -> PromptInfo {
        PromptInfo {
            name: "shopify".into(),
            detail: Some("app@localhost:5432".into()),
            database: Some("shop".into()),
            server: Some(Server::PostgreSql),
            took: Some(Duration::from_millis(2400)),
            disconnected: false,
        }
    }

    #[test]
    fn disconnected_prompt_names_the_old_connection() {
        let info = PromptInfo {
            name: "Patrimonio Nacional Web".into(),
            disconnected: true,
            ..Default::default()
        };
        let left = SqlPrompt { info }.left();
        assert_eq!(plain(&left), "Patrimonio Nacional Web ⚠ Disconnected\n");
        assert!(left.contains("\x1b[4;33mDisconnected"), "{left:?}");
    }

    #[test]
    fn prompt_line_has_name_details_database_server_and_time() {
        let left = SqlPrompt { info: info() }.left();
        assert!(left.contains("\x1b["), "{left:?}");
        assert_eq!(
            plain(&left),
            "shopify ✱ app@localhost:5432 on shop via ● PostgreSQL took 2s\n"
        );
    }

    #[test]
    fn quick_commands_and_missing_parts_are_left_out() {
        let quick = PromptInfo {
            took: Some(Duration::from_millis(1900)),
            database: None,
            ..info()
        };
        assert_eq!(
            plain(&SqlPrompt { info: quick }.left()),
            "shopify ✱ app@localhost:5432 via ● PostgreSQL\n"
        );
        let none = PromptInfo {
            name: "(none)".into(),
            ..Default::default()
        };
        assert_eq!(plain(&SqlPrompt { info: none }.left()), "(none)\n");
        assert_eq!(format_took(Duration::from_secs(65)), "1m 5s");
        assert_eq!(format_took(Duration::from_secs(3700)), "1h 1m");
        assert_eq!(INDICATOR, "▶\u{fe0e} ");
    }

    #[test]
    fn history_lives_in_dot_dato() {
        if let Some(p) = history_path() {
            assert!(p.ends_with(".dato/history"));
        }
    }
}
