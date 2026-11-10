//! Query editor: reedline wired with SQL validation, completion and highlighting.

mod completer;
pub mod external;
mod highlighter;
pub mod keywords;
mod validator;

use std::sync::{Arc, RwLock};

use reedline::{
    ColumnarMenu, Emacs, KeyCode, KeyModifiers, MenuBuilder, Reedline, ReedlineEvent, ReedlineMenu,
    default_emacs_keybindings,
};

use crate::tui::session::SchemaCache;

pub use completer::{SqlCompleter, candidates};
pub use highlighter::{SqlHighlighter, spans};
pub use validator::SqlValidator;

const MENU: &str = "completion_menu";

/// Builds the line editor. Ctrl-O opens `$VISUAL`/`$EDITOR`; Alt-Enter submits
/// even when the statement looks unfinished.
pub fn build(schema: Arc<RwLock<SchemaCache>>) -> Reedline {
    let mut keys = default_emacs_keybindings();
    keys.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu(MENU.to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    keys.add_binding(KeyModifiers::ALT, KeyCode::Enter, ReedlineEvent::Submit);

    let mut editor = Reedline::create()
        .with_completer(Box::new(SqlCompleter::new(schema)))
        .with_menu(ReedlineMenu::EngineCompleter(Box::new(
            ColumnarMenu::default().with_name(MENU),
        )))
        .with_highlighter(Box::new(SqlHighlighter))
        .with_validator(Box::new(SqlValidator))
        .with_edit_mode(Box::new(Emacs::new(keys)));

    let command = external::editor_command();
    let mut parts = command.split_whitespace();
    if let Some(program) = parts.next() {
        let mut cmd = std::process::Command::new(program);
        cmd.args(parts);
        editor = editor.with_buffer_editor(cmd, std::env::temp_dir().join("dato-edit.sql"));
    }
    editor
}
