//! `\connect` wizard: pick a server, then fill in that server's connect form.

use crossterm::event::KeyEvent;
use nu_ansi_term::{Color, Style};

use crate::client::ConnectOptions;
use crate::server::Server;
use crate::ui::atoms::{PasswordInput, SelectList, TextInput};
use crate::ui::molecules::{Choice, FieldInput, LabeledField};
use crate::ui::{ConnectForm, Field, FieldValues, Line, Step, Widget};

enum Stage {
    Server(SelectList<Server>),
    Fields {
        server: Server,
        specs: Vec<Field>,
        fields: Vec<LabeledField>,
        current: usize,
    },
}

pub struct ConnectWizard {
    stage: Stage,
}

impl Default for ConnectWizard {
    fn default() -> Self {
        Self::new()
    }
}

fn field(spec: &Field) -> LabeledField {
    let input = if !spec.choices.is_empty() {
        FieldInput::Choice(Choice::new(spec.choices, &spec.default))
    } else if spec.secret {
        FieldInput::Password(PasswordInput::new())
    } else {
        FieldInput::Text(TextInput::new(&spec.default).with_placeholder(spec.placeholder))
    };
    LabeledField::new(spec.label, input)
}

impl ConnectWizard {
    pub fn new() -> Self {
        Self {
            stage: Stage::Server(SelectList::new(Server::ALL.iter().copied())),
        }
    }

    fn start_fields(&mut self, server: Server) {
        let specs = server.fields();
        let fields = specs.iter().map(field).collect();
        self.stage = Stage::Fields {
            server,
            specs,
            fields,
            current: 0,
        };
    }
}

impl Widget for ConnectWizard {
    type Output = ConnectOptions;

    fn on_key(&mut self, key: KeyEvent) -> Step<ConnectOptions> {
        match &mut self.stage {
            Stage::Server(list) => match list.on_key(key) {
                Step::Done(server) => {
                    self.start_fields(server);
                    Step::Continue
                }
                Step::Cancel => Step::Cancel,
                Step::Continue => Step::Continue,
            },
            Stage::Fields {
                server,
                specs,
                fields,
                current,
            } => match fields[*current].on_key(key) {
                Step::Cancel => Step::Cancel,
                Step::Continue => Step::Continue,
                Step::Done(_) if *current + 1 < fields.len() => {
                    *current += 1;
                    Step::Continue
                }
                Step::Done(_) => {
                    let values = FieldValues(
                        specs
                            .iter()
                            .zip(fields.iter())
                            .map(|(s, f)| (s.key, f.value()))
                            .collect(),
                    );
                    match server.build(&values) {
                        Ok(opts) => Step::Done(opts),
                        Err(e) => {
                            // Go back to the offending field and show the error.
                            if let Some(i) = specs.iter().position(|s| s.key == e.key) {
                                *current = i;
                                fields[i].set_error(Some(e.message));
                            }
                            Step::Continue
                        }
                    }
                }
            },
        }
    }

    fn render(&self) -> Vec<Line> {
        let title = Style::new().bold();
        let hint = Style::new().fg(Color::DarkGray);
        match &self.stage {
            Stage::Server(list) => {
                let mut out = vec![
                    Line::styled(title, "Connect to a database server"),
                    Line::styled(hint, "  Up/Down to choose, Enter to select, Esc to cancel"),
                ];
                out.extend(list.render());
                out
            }
            Stage::Fields {
                server,
                fields,
                current,
                ..
            } => {
                let mut out = vec![
                    Line::styled(title, format!("Connect to {server}")),
                    Line::styled(hint, "  Enter for next field, Esc to cancel"),
                ];
                for (i, f) in fields.iter().enumerate() {
                    if i < *current {
                        out.push(f.render_done());
                    } else if i == *current {
                        out.extend(f.render());
                    }
                }
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::MySqlOptions;
    use crate::ui::test_keys::{ctrl, enter, feed, key, typed};
    use crossterm::event::KeyCode;

    fn screen(w: &ConnectWizard) -> String {
        w.render()
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn lists_supported_servers() {
        let w = ConnectWizard::new();
        let s = screen(&w);
        assert!(
            s.contains("> sqlite") && s.contains("  mysql") && s.contains("  psql"),
            "{s}"
        );
    }

    #[test]
    fn sqlite_through_the_wizard() {
        let mut w = ConnectWizard::new();
        let keys = [enter()]
            .into_iter()
            .chain(typed("app.db"))
            .chain([enter()]);
        assert_eq!(
            feed(&mut w, keys),
            Step::Done(ConnectOptions::sqlite(Some("app.db")))
        );
    }

    #[test]
    fn mysql_defaults_and_masked_password() {
        let mut w = ConnectWizard::new();
        feed(&mut w, [key(KeyCode::Down), enter()]);
        let s = screen(&w);
        assert!(s.contains("Host: localhost"), "{s}");
        // host -> port
        feed(&mut w, [enter()]);
        assert!(screen(&w).contains("Port: 3306"));
        // port -> user: replace default user with "app"
        feed(&mut w, [enter(), ctrl('u')]);
        feed(&mut w, typed("app"));
        feed(&mut w, [enter()]);
        feed(&mut w, typed("secret"));
        let s = screen(&w);
        assert!(
            s.contains("Password: ******") && !s.contains("secret"),
            "{s}"
        );
        feed(&mut w, [enter()]);
        let done = feed(&mut w, typed("shop").into_iter().chain([enter()]));
        assert!(!screen(&w).contains("secret"));
        assert_eq!(
            done,
            Step::Done(ConnectOptions::MySql(MySqlOptions {
                host: "localhost".into(),
                port: 3306,
                user: "app".into(),
                password: Some("secret".into()),
                password_required: false,
                socket: None,
                database: Some("shop".into()),
            }))
        );
    }

    #[test]
    fn invalid_port_returns_to_the_field() {
        let mut w = ConnectWizard::new();
        feed(&mut w, [key(KeyCode::Down), enter(), enter(), ctrl('u')]);
        feed(&mut w, typed("nope"));
        assert_eq!(
            feed(&mut w, [enter(), enter(), enter(), enter()]),
            Step::Continue
        );
        let s = screen(&w);
        assert!(s.contains("not a port number: nope"), "{s}");
        assert!(s.contains("Port: nope"), "{s}");
    }

    #[test]
    fn esc_cancels_at_any_stage() {
        assert_eq!(
            feed(&mut ConnectWizard::new(), [key(KeyCode::Esc)]),
            Step::Cancel
        );
        assert_eq!(
            feed(&mut ConnectWizard::new(), [enter(), key(KeyCode::Esc)]),
            Step::Cancel
        );
        assert_eq!(
            feed(&mut ConnectWizard::new(), [enter(), ctrl('c')]),
            Step::Cancel
        );
    }

    #[test]
    fn psql_defaults_masked_password_and_result() {
        use crate::client::PsqlOptions;
        use crate::client::psql::SslMode;
        let mut w = ConnectWizard::new();
        feed(&mut w, [key(KeyCode::Down), key(KeyCode::Down), enter()]);
        let s = screen(&w);
        assert!(s.contains("Host: localhost"), "{s}");
        feed(&mut w, [enter()]);
        assert!(screen(&w).contains("Port: 5432"));
        // port -> user: replace the default user
        feed(&mut w, [enter(), ctrl('u')]);
        feed(&mut w, typed("app"));
        feed(&mut w, [enter()]);
        feed(&mut w, typed("secret"));
        let s = screen(&w);
        assert!(
            s.contains("Password: ******") && !s.contains("secret"),
            "{s}"
        );
        feed(&mut w, [enter()]);
        feed(&mut w, typed("shop").into_iter().chain([enter()]));
        // TLS is off unless picked, and picked rather than typed.
        assert!(
            screen(&w).contains("TLS mode: (*) disable  ( ) prefer"),
            "{}",
            screen(&w)
        );
        let done = feed(&mut w, [enter()]);
        assert_eq!(
            done,
            Step::Done(ConnectOptions::Psql(PsqlOptions {
                host: "localhost".into(),
                port: 5432,
                user: "app".into(),
                password: Some("secret".into()),
                password_required: false,
                dbname: Some("shop".into()),
                sslmode: SslMode::Disable,
                sslrootcert: None,
            }))
        );
    }

    #[test]
    fn psql_tls_mode_is_picked_with_the_arrow_keys() {
        use crate::client::psql::SslMode;
        let mut w = ConnectWizard::new();
        feed(&mut w, [key(KeyCode::Down), key(KeyCode::Down), enter()]);
        // host, port, user, password, database
        feed(&mut w, [enter(), enter(), enter(), enter(), enter()]);
        feed(&mut w, [key(KeyCode::Right), key(KeyCode::Right)]);
        assert!(screen(&w).contains("( ) disable  ( ) prefer  (*) require"));
        let done = feed(&mut w, [enter()]);
        assert!(
            matches!(done, Step::Done(ConnectOptions::Psql(p)) if p.sslmode == SslMode::Require)
        );
    }
}
