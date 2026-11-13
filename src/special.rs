//! Built-in non-SQL commands (`.tables`, `\d`, `\T`, `help`, ...).

mod commands;

use crate::tui::session::{NotConnected, Output, Session};

type Handler = fn(&mut Session, Option<&str>) -> Vec<Output>;

pub struct SpecialCommand {
    pub names: &'static [&'static str],
    pub syntax: &'static str,
    pub help: &'static str,
    /// Fails with a not-connected error while the session has no connection.
    pub needs_connection: bool,
    run: Handler,
}

pub static COMMANDS: &[SpecialCommand] = &[
    SpecialCommand {
        names: &[".tables", "\\dt"],
        syntax: ".tables [prefix]",
        help: "List tables and views.",
        needs_connection: true,
        run: commands::tables,
    },
    SpecialCommand {
        names: &[".views", "\\dv"],
        syntax: ".views [prefix]",
        help: "List views.",
        needs_connection: true,
        run: commands::views,
    },
    SpecialCommand {
        names: &[".schema"],
        syntax: ".schema [table]",
        help: "Show CREATE statements, for all objects or one table.",
        needs_connection: true,
        run: commands::schema,
    },
    SpecialCommand {
        names: &[".indexes", "\\di"],
        syntax: ".indexes [table]",
        help: "List indexes.",
        needs_connection: true,
        run: commands::indexes,
    },
    SpecialCommand {
        names: &[".databases", "\\l"],
        syntax: ".databases",
        help: "List attached databases.",
        needs_connection: true,
        run: commands::databases,
    },
    SpecialCommand {
        names: &["\\d", "describe"],
        syntax: "\\d [table]",
        help: "Describe a table's columns; without argument, list tables.",
        needs_connection: true,
        run: commands::describe,
    },
    SpecialCommand {
        names: &["\\T"],
        syntax: "\\T [format]",
        help: "Set the output format; without argument, show it.",
        needs_connection: false,
        run: commands::table_format,
    },
    SpecialCommand {
        names: &["\\connect"],
        syntax: "\\connect [server [args]]",
        help: "Connect to a database; without arguments, choose interactively.",
        needs_connection: false,
        run: commands::connect,
    },
    SpecialCommand {
        names: &["help", "\\?", ".help"],
        syntax: "help",
        help: "Show this help.",
        needs_connection: false,
        run: commands::help,
    },
    SpecialCommand {
        names: &["quit", "exit", "\\q", ".quit"],
        syntax: "quit",
        help: "Quit the shell.",
        needs_connection: false,
        run: commands::quit,
    },
];

fn is_symbolic(name: &str) -> bool {
    name.starts_with('.') || name.starts_with('\\')
}

fn split_command(input: &str) -> (&str, Option<&str>) {
    let t = input.trim().trim_end_matches(';').trim_end();
    match t.split_once(char::is_whitespace) {
        Some((cmd, arg)) => {
            let arg = arg.trim();
            (cmd, (!arg.is_empty()).then_some(arg))
        }
        None => (t, None),
    }
}

/// Dot and backslash commands match exactly; word commands ignore case.
fn lookup(cmd: &str) -> Option<&'static SpecialCommand> {
    COMMANDS.iter().find(|c| {
        c.names.iter().any(|n| {
            if is_symbolic(n) {
                *n == cmd
            } else {
                n.eq_ignore_ascii_case(cmd)
            }
        })
    })
}

/// Whether `input` is a special command rather than SQL.
pub fn is_special(input: &str) -> bool {
    let (cmd, _) = split_command(input);
    is_symbolic(cmd) || lookup(cmd).is_some()
}

/// Names (starting with `.` or `\`) offered by completion.
pub fn symbolic_names() -> impl Iterator<Item = &'static str> {
    COMMANDS
        .iter()
        .flat_map(|c| c.names.iter().copied())
        .filter(|n| is_symbolic(n))
}

pub fn dispatch(session: &mut Session, input: &str) -> Vec<Output> {
    let (cmd, arg) = split_command(input);
    match lookup(cmd) {
        Some(c) if c.needs_connection && !session.is_connected() => {
            vec![Output::Error(NotConnected.to_string())]
        }
        Some(c) => (c.run)(session, arg),
        None => vec![Output::Error(format!(
            "unknown command: {cmd} (type `help` for the list)"
        ))],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::sqlite::SqliteClient;
    use crate::render::Format;

    fn session() -> Session {
        Session::new(Box::new(SqliteClient::open_memory()), Format::Ascii, false)
    }

    fn text(out: &[Output]) -> String {
        out.iter()
            .map(|o| match o {
                Output::Text(t) | Output::Status(t) | Output::Error(t) => t.clone(),
                Output::Quit => "<quit>".into(),
                other => format!("{other:?}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn fixture() -> Session {
        let mut s = session();
        s.handle(
            "create table users(id integer primary key, email text not null default 'x'); \
             create index idx_users_email on users(email); \
             create table user_roles(a); create table orders(a); \
             create view v as select id from users;",
        );
        s
    }

    #[test]
    fn recognition() {
        assert!(is_special(".tables"));
        assert!(is_special("\\dt users"));
        assert!(is_special("HELP"));
        assert!(is_special("quit;"));
        assert!(is_special(".bogus"));
        assert!(!is_special("select 1"));
        assert!(is_special("\\T"));
    }

    #[test]
    fn dot_commands_are_case_sensitive() {
        assert!(lookup(".TABLES").is_none());
        assert!(lookup("QUIT").is_some());
        assert!(lookup("\\t").is_none());
    }

    #[test]
    fn unknown_command_errors_with_name() {
        let mut s = session();
        let out = s.handle(".bogus");
        assert!(matches!(&out[0], Output::Error(e) if e.contains(".bogus")));
    }

    #[test]
    fn tables_with_prefix() {
        let mut s = fixture();
        let t = text(&s.handle(".tables user"));
        assert!(t.contains("user_roles") && t.contains("users"));
        assert!(!t.contains("orders"));
        assert!(text(&s.handle("\\dt")).contains("orders"));
    }

    #[test]
    fn views_only() {
        let mut s = fixture();
        let t = text(&s.handle(".views"));
        assert!(t.contains(" v ") && !t.contains("users"), "{t}");
    }

    #[test]
    fn schema_one_table() {
        let mut s = fixture();
        let t = text(&s.handle(".schema users"));
        assert!(t.contains("CREATE TABLE users"));
        assert!(t.contains("CREATE INDEX idx_users_email"));
        assert!(!t.contains("orders"));
    }

    #[test]
    fn indexes_of_table() {
        let mut s = fixture();
        assert!(text(&s.handle(".indexes users")).contains("idx_users_email"));
        assert!(text(&s.handle("\\di")).contains("idx_users_email"));
    }

    #[test]
    fn databases_lists_main() {
        let mut s = fixture();
        assert!(text(&s.handle(".databases")).contains("main"));
    }

    #[test]
    fn describe_columns() {
        let mut s = fixture();
        let t = text(&s.handle("\\d users"));
        assert!(t.contains("id") && t.contains("email") && t.contains("INTEGER"));
        assert!(t.contains("'x'"));
        assert!(text(&s.handle("describe users")).contains("email"));
        assert!(text(&s.handle("\\d")).contains("orders"));
        let out = s.handle("\\d nope");
        assert!(matches!(&out[0], Output::Error(e) if e.contains("no such table")));
    }

    #[test]
    fn switch_format() {
        let mut s = session();
        s.handle("\\T csv");
        assert_eq!(text(&s.handle("select 1 as a;")), "a\n1\n");
        let info = text(&s.handle("\\T"));
        assert!(info.contains("csv") && info.contains("vertical"));
        assert!(matches!(&s.handle("\\T nope")[0], Output::Error(_)));
    }

    #[test]
    fn help_lists_commands() {
        let mut s = session();
        let t = text(&s.handle("help"));
        assert!(t.contains(".tables") && t.contains(".schema") && t.contains("\\T"));
    }

    #[test]
    fn disconnected_commands() {
        let mut s = Session::disconnected(Format::Ascii, false);
        assert!(text(&s.handle("help")).contains(".tables"));
        assert!(text(&s.handle("\\T csv")).contains("csv"));
        for cmd in [".tables", "\\d users", ".schema", ".databases"] {
            let out = s.handle(cmd);
            assert!(
                matches!(&out[0], Output::Error(e) if e.contains("\\connect")),
                "{cmd}: {out:?}"
            );
        }
        assert_eq!(s.handle("quit"), vec![Output::Quit]);
    }

    #[test]
    fn connect_command() {
        use crate::client::ConnectOptions;
        let mut s = Session::disconnected(Format::Ascii, false);
        assert_eq!(s.handle("\\connect"), vec![Output::ConnectWizard]);
        assert_eq!(
            s.handle("\\connect sqlite x.db"),
            vec![Output::Connect(ConnectOptions::sqlite(Some("x.db")))]
        );
        assert!(matches!(
            &s.handle("\\connect mysql -h db.local -u app -p shop")[0],
            Output::Connect(ConnectOptions::MySql(o))
                if o.host == "db.local" && o.password_required
        ));
        assert!(matches!(
            &s.handle("\\connect psql -h db.local -U app -W -d shop")[0],
            Output::Connect(ConnectOptions::Psql(o))
                if o.host == "db.local" && o.user == "app" && o.password_required
                    && o.dbname.as_deref() == Some("shop")
        ));
        assert!(matches!(
            &s.handle("\\connect PSQL -U app")[0],
            Output::Connect(ConnectOptions::Psql(_))
        ));
        // Another server's flag is rejected.
        assert!(matches!(
            &s.handle("\\connect psql -P 1")[0],
            Output::Error(_)
        ));
        assert!(matches!(
            &s.handle("\\connect mysql -U app")[0],
            Output::Error(_)
        ));
        for name in ["postgres", "oracle"] {
            let out = s.handle(&format!("\\connect {name}"));
            assert!(
                matches!(&out[0], Output::Error(e) if e.contains(name) && e.contains("sqlite, mysql, psql")),
                "{out:?}"
            );
        }
        assert!(matches!(
            &s.handle("\\connect mysql --bogus")[0],
            Output::Error(_)
        ));
        assert!(
            matches!(&s.handle("\\connect mysql --help")[0], Output::Text(t) if t.contains("--host"))
        );
        assert!(text(&s.handle("help")).contains("\\connect"));
        assert!(symbolic_names().any(|n| n == "\\connect"));
    }

    #[test]
    fn quit_commands() {
        let mut s = session();
        for q in ["quit", "exit", "\\q", ".quit"] {
            assert_eq!(s.handle(q), vec![Output::Quit], "{q}");
        }
    }
}
