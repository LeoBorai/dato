//! Query editor: reedline wired with SQL validation, completion and highlighting.

mod completer;
mod context;
pub mod external;
mod highlighter;
mod menu;
mod validator;

use std::sync::{Arc, RwLock};

use reedline::{
    EditCommand, Emacs, IdeMenu, KeyCode, KeyModifiers, MenuBuilder, Reedline, ReedlineEvent,
    ReedlineMenu, default_emacs_keybindings,
};

use crate::server::Server;
use crate::tui::session::SchemaCache;

pub use completer::{Candidate, Kind, SqlCompleter, candidates};
pub use highlighter::{SqlHighlighter, spans, spans_with};
pub use validator::SqlValidator;

use menu::QuietMenu;

const MENU: &str = "completion_menu";

/// Builds the line editor. Ctrl-O opens `$VISUAL`/`$EDITOR`; Alt-Enter submits
/// even when the statement looks unfinished.
/// Completion follows `schema` (which names its server); highlighting and
/// statement completeness follow `dialect`.
pub fn build(schema: Arc<RwLock<SchemaCache>>, dialect: Arc<RwLock<Server>>) -> Reedline {
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

    // Open the menu as the user types instead of waiting for Tab. The menu
    // refreshes itself while open; a space or punctuation closes it, and `.`
    // reopens it for `alias.` completion.
    for c in ('a'..='z')
        .chain('A'..='Z')
        .chain('0'..='9')
        .chain(['_', '$', '.', '\\'])
    {
        let modifiers = if c.is_ascii_uppercase() {
            KeyModifiers::SHIFT
        } else {
            KeyModifiers::NONE
        };
        keys.add_binding(
            modifiers,
            KeyCode::Char(c),
            ReedlineEvent::Multiple(vec![
                ReedlineEvent::Edit(vec![EditCommand::InsertChar(c)]),
                ReedlineEvent::Menu(MENU.to_string()),
            ]),
        );
    }
    keys.add_binding(
        KeyModifiers::NONE,
        KeyCode::Backspace,
        ReedlineEvent::Multiple(vec![
            ReedlineEvent::Edit(vec![EditCommand::Backspace]),
            ReedlineEvent::Menu(MENU.to_string()),
        ]),
    );

    let mut editor = Reedline::create()
        .with_completer(Box::new(SqlCompleter::new(schema)))
        .with_menu(ReedlineMenu::EngineCompleter(Box::new(QuietMenu::new(
            IdeMenu::default()
                .with_name(MENU)
                .with_default_border()
                .with_word_chars("_$"),
        ))))
        .with_highlighter(Box::new(SqlHighlighter::new(Arc::clone(&dialect))))
        .with_validator(Box::new(SqlValidator::new(dialect)))
        .with_edit_mode(Box::new(Emacs::new(keys)));

    let command = external::editor_command();
    let mut parts = command.split_whitespace();
    if let Some(program) = parts.next() {
        let mut cmd = std::process::Command::new(program);
        cmd.args(parts);
        // Ctrl-O buffer: `~/.dato/edit.sql`, or the temp dir without a home.
        let buffer = crate::paths::edit_buffer()
            .filter(|p| {
                p.parent()
                    .is_some_and(|d| crate::paths::ensure_dir(d).is_ok())
            })
            .unwrap_or_else(|| std::env::temp_dir().join("dato-edit.sql"));
        editor = editor.with_buffer_editor(cmd, buffer);
    }
    editor
}
