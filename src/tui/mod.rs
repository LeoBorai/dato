//! The interactive shell (litecli-style REPL).

pub mod interrupt;
pub mod session;

use std::borrow::Cow;
use std::io::Write;
use std::path::PathBuf;

use reedline::{
    EditCommand, FileBackedHistory, Prompt, PromptEditMode, PromptHistorySearch,
    PromptHistorySearchStatus, Signal,
};

use crate::editor::{self, external};
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

pub fn history_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("dato").join("history"))
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
        }
    }
    let _ = out.flush();
    quit
}

/// Runs the interactive shell until the user quits. Returns the process exit code.
pub fn run(mut session: Session) -> i32 {
    interrupt::install();
    let mut line_editor = editor::build(session.schema());
    if let Some(path) = history_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match FileBackedHistory::with_file(1000, path) {
            Ok(h) => line_editor = line_editor.with_history(Box::new(h)),
            Err(e) => eprintln!("warning: history disabled: {e}"),
        }
    }
    let prompt = SqlPrompt {
        name: session.display_name(),
    };
    let (mut stdout, mut stderr) = (std::io::stdout(), std::io::stderr());

    loop {
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
                let outputs = session.handle(&buffer);
                if emit(&outputs, &mut stdout, &mut stderr) {
                    return 0;
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
    fn history_lives_under_data_dir() {
        if let Some(p) = history_path() {
            assert!(p.ends_with("dato/history"));
        }
    }
}
