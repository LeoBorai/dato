//! `\connect` wizard: pick a server, then fill in that server's connect form.

use crossterm::event::KeyEvent;
use nu_ansi_term::{Color, Style};

use crate::client::ConnectOptions;
use crate::safeguard::Safeguard;
use crate::server::Server;
use crate::store::{SavedConnection, Tag};
use crate::ui::atoms::{PasswordInput, PathInput, SelectList, TextInput};
use crate::ui::form::{AUTH_PASSWORD, FieldError, form_values, is_visible};
use crate::ui::molecules::{Choice, FieldInput, LabeledField};
use crate::ui::{ConnectForm, Field, FieldValues, Line, Step, Widget, server_color, server_title};

/// Choices of the "Environment tag" field.
const TAGS: &[&str] = &[
    "none",
    "local",
    "testing",
    "development",
    "staging",
    "production",
];

/// What the wizard collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConnection {
    pub opts: ConnectOptions,
    /// Name to save the connection as; `None` when left empty.
    pub name: Option<String>,
    /// Environment of the connection; `None` when left at `none`.
    pub tag: Option<Tag>,
    /// Checks before `UPDATE` and `DELETE`; `None` when left at `none`.
    pub safeguard: Option<Safeguard>,
    /// Keep the password with the saved connection.
    pub save_password: bool,
}

enum Stage {
    Server(SelectList<Server>),
    Fields {
        server: Server,
        specs: Vec<Field>,
        fields: Vec<LabeledField>,
        current: usize,
    },
}

/// The saved connection being edited.
struct Editing {
    original: SavedConnection,
    /// Names of the other saved connections.
    taken: Vec<String>,
}

pub struct ConnectWizard {
    stage: Stage,
    editing: Option<Editing>,
    /// Terminal `(columns, rows)`; when set, the wizard is drawn in its middle.
    size: Option<(usize, usize)>,
}

/// Columns and rows the wizard is laid out in when centered. Fixed, so the
/// form does not move as fields are filled in or menus open.
const CENTER_WIDTH: usize = 64;
const CENTER_HEIGHT: usize = 14;

impl Default for ConnectWizard {
    fn default() -> Self {
        Self::new()
    }
}

fn field(spec: &Field) -> LabeledField {
    let input = if !spec.choices.is_empty() {
        FieldInput::Choice(Choice::new(spec.choices, &spec.default))
    } else if spec.path {
        FieldInput::Path(PathInput::new(&spec.default).with_placeholder(spec.placeholder))
    } else if spec.secret {
        FieldInput::Password(PasswordInput::new().with_placeholder(spec.placeholder))
    } else {
        FieldInput::Text(TextInput::new(&spec.default).with_placeholder(spec.placeholder))
    };
    LabeledField::new(spec.label, input)
}

impl ConnectWizard {
    pub fn new() -> Self {
        Self {
            stage: Stage::Server(SelectList::new(Server::ALL.iter().copied())),
            editing: None,
            size: None,
        }
    }

    /// Edits `original`: its form, filled in with the current settings. The
    /// name cannot be changed to one of `taken`.
    pub fn edit(original: SavedConnection, taken: Vec<String>) -> Self {
        let mut wizard = Self::new();
        let server = original.server();
        wizard.editing = Some(Editing { original, taken });
        wizard.start_fields(server);
        wizard
    }

    /// Draws the wizard in the middle of a terminal of `columns` x `rows`.
    pub fn centered(mut self, columns: usize, rows: usize) -> Self {
        self.size = Some((columns, rows));
        self
    }

    fn start_fields(&mut self, server: Server) {
        let mut specs = server.fields();
        let has_password = specs.iter().any(|s| s.secret);
        specs.push(Field {
            placeholder: "auto",
            ..Field::text("name", "Connection name", "")
        });
        specs.push(Field::choice("tag", "Environment tag", TAGS, "none"));
        specs.push(Field::choice(
            "safeguard",
            "Safeguard level",
            &["none", "1", "2"],
            "none",
        ));
        if has_password {
            let save = Field::choice("save_password", "Save password", &["no", "yes"], "no");
            // Vault provides the password: there is none to save.
            specs.push(if specs.iter().any(|s| s.key == "auth") {
                save.when("auth", &[AUTH_PASSWORD])
            } else {
                save
            });
        }
        if let Some(Editing { original, .. }) = &self.editing {
            let values = form_values(&original.opts);
            let stored = stored_password(&original.opts).is_some();
            for spec in &mut specs {
                if let Some((_, v)) = values.0.iter().find(|(k, _)| *k == spec.key) {
                    spec.default = v.clone();
                }
                match spec.key {
                    "name" => {
                        spec.default = original.name.clone();
                        spec.placeholder = "unchanged";
                    }
                    "tag" => spec.default = original.tag.map_or("none", Tag::name).into(),
                    "safeguard" => {
                        spec.default = original.safeguard.map_or("none", Safeguard::name).into()
                    }
                    "password" if stored => spec.placeholder = "unchanged",
                    "save_password" if stored => spec.default = "yes".into(),
                    _ => {}
                }
            }
        }
        let fields = specs.iter().map(field).collect();
        self.stage = Stage::Fields {
            server,
            specs,
            fields,
            current: 0,
        };
    }
}

fn stored_password(opts: &ConnectOptions) -> Option<&str> {
    match opts {
        ConnectOptions::Sqlite { .. } => None,
        ConnectOptions::MySql(o) => o.password.as_deref(),
        ConnectOptions::Psql(o) => o.password.as_deref(),
    }
}

/// Carries over from `old` what the form does not show or was left empty: the
/// password, the MySQL socket and the PostgreSQL root certificate.
fn keep_unedited(new: &mut ConnectOptions, old: &ConnectOptions) {
    match (new, old) {
        (ConnectOptions::MySql(n), ConnectOptions::MySql(o)) => {
            n.socket = o.socket.clone();
            if n.vault.is_none() && n.password.is_none() {
                n.password = o.password.clone();
                n.password_required = o.password_required;
            }
        }
        (ConnectOptions::Psql(n), ConnectOptions::Psql(o)) => {
            n.sslrootcert = o.sslrootcert.clone();
            if n.vault.is_none() && n.password.is_none() {
                n.password = o.password.clone();
                n.password_required = o.password_required;
            }
        }
        _ => {}
    }
}

impl Widget for ConnectWizard {
    type Output = NewConnection;

    fn on_key(&mut self, key: KeyEvent) -> Step<NewConnection> {
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
                Step::Done(_) => {
                    let value = |j: usize| fields[j].value();
                    let next = (*current + 1..fields.len()).find(|&j| is_visible(specs, &value, j));
                    if let Some(next) = next {
                        *current = next;
                        return Step::Continue;
                    }
                    // Fields that were not asked for count as empty.
                    let values = FieldValues(
                        specs
                            .iter()
                            .enumerate()
                            .map(|(j, s)| {
                                let v = if is_visible(specs, &value, j) {
                                    value(j)
                                } else {
                                    String::new()
                                };
                                (s.key, v)
                            })
                            .collect(),
                    );
                    let name =
                        Some(values.get("name").trim().to_string()).filter(|n| !n.is_empty());
                    let editing = self.editing.as_ref();
                    let built = server.build(&values).and_then(|opts| match &name {
                        Some(n) if editing.is_some_and(|e| e.taken.contains(n)) => {
                            Err(FieldError {
                                key: "name",
                                message: format!("a connection called {n} already exists"),
                            })
                        }
                        _ => Ok(opts),
                    });
                    match built {
                        Ok(mut opts) => {
                            if let Some(e) = editing {
                                keep_unedited(&mut opts, &e.original.opts);
                            }
                            Step::Done(NewConnection {
                                opts,
                                name: name.or_else(|| editing.map(|e| e.original.name.clone())),
                                tag: Tag::parse(values.get("tag")),
                                safeguard: Safeguard::parse(values.get("safeguard")),
                                save_password: values.get("save_password") == "yes",
                            })
                        }
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
        let lines = self.render_stage();
        let Some((columns, rows)) = self.size else {
            return lines;
        };
        let pad = " ".repeat(columns.saturating_sub(CENTER_WIDTH) / 2);
        let mut out = vec![Line::default(); rows.saturating_sub(CENTER_HEIGHT) / 2];
        out.extend(lines.into_iter().map(|mut l| {
            l.0.insert(0, (Style::new(), pad.clone()));
            l
        }));
        out
    }
}

impl ConnectWizard {
    fn render_stage(&self) -> Vec<Line> {
        let title = Style::new().bold();
        let hint = Style::new().fg(Color::DarkGray);
        match &self.stage {
            Stage::Server(list) => {
                let mut out = vec![
                    Line::styled(title, "Choose a database provider"),
                    Line::styled(hint, "  Up/Down to choose, Enter to select, Esc to cancel"),
                ];
                for (i, server) in list.items().iter().enumerate() {
                    let on = i == list.selected_index();
                    let text = if on {
                        Style::new().bold().fg(Color::Cyan)
                    } else {
                        Style::new()
                    };
                    let mut line = Line::styled(text, if on { "> " } else { "  " });
                    line.push(Style::new().fg(server_color(*server)), "●");
                    line.push(text, format!(" {} ({server})", server_title(*server)));
                    out.push(line);
                }
                out
            }
            Stage::Fields {
                server,
                specs,
                fields,
                current,
            } => {
                let value = |j: usize| fields[j].value();
                let heading = match &self.editing {
                    Some(e) => format!("Edit {} ({server})", e.original.name),
                    None => format!("Connect to {server}"),
                };
                let mut out = vec![
                    Line::styled(title, heading),
                    Line::styled(hint, "  Enter for next field, Esc to cancel"),
                ];
                for (i, f) in fields.iter().enumerate() {
                    if i < *current && is_visible(specs, &value, i) {
                        out.push(f.render_done());
                    } else if i == *current {
                        out.extend(f.render());
                        if specs[i].key == "safeguard" {
                            let level = Safeguard::parse(&f.value());
                            out.push(Line::styled(hint, format!("  {}", Safeguard::hint(level))));
                        }
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
            s.contains("> ● SQLite (sqlite)")
                && s.contains("  ● MySQL (mysql)")
                && s.contains("  ● PostgreSQL (psql)"),
            "{s}"
        );
    }

    #[test]
    fn sqlite_through_the_wizard() {
        let mut w = ConnectWizard::new();
        // provider, file, then name, tag and safeguard left as they are.
        let keys = [enter()].into_iter().chain(typed("app.db")).chain([
            enter(),
            enter(),
            enter(),
            enter(),
        ]);
        assert_eq!(
            feed(&mut w, keys),
            Step::Done(NewConnection {
                opts: ConnectOptions::sqlite(Some("app.db")),
                name: None,
                tag: None,
                safeguard: None,
                save_password: false,
            })
        );
    }

    #[test]
    fn tag_is_picked_with_the_arrow_keys() {
        let mut w = ConnectWizard::new();
        // provider, file, connection name.
        feed(&mut w, [enter()].into_iter().chain(typed("app.db")));
        feed(&mut w, [enter(), enter()]);
        let s = screen(&w);
        assert!(s.contains("Environment tag: < none >"), "{s}");
        let done = feed(&mut w, [key(KeyCode::Left), enter(), enter()]);
        assert!(
            matches!(&done, Step::Done(n) if n.tag == Some(Tag::Production)),
            "{done:?}"
        );
    }

    #[test]
    fn safeguard_hint_follows_the_choice() {
        let mut w = ConnectWizard::new();
        // provider, file, connection name, tag.
        feed(&mut w, [enter()].into_iter().chain(typed("app.db")));
        feed(&mut w, std::iter::repeat_n(enter(), 3));
        for level in [None, Some(Safeguard::Confirm), Some(Safeguard::Password)] {
            let s = screen(&w);
            assert!(s.contains(Safeguard::hint(level)), "{s}");
            feed(&mut w, [key(KeyCode::Right)]);
        }
    }

    fn saved(password: Option<&str>, required: bool) -> SavedConnection {
        SavedConnection {
            name: "shop".into(),
            opts: ConnectOptions::MySql(MySqlOptions {
                host: "db".into(),
                port: 3307,
                user: "app".into(),
                password: password.map(String::from),
                password_required: required,
                socket: Some("/tmp/my.sock".into()),
                database: Some("shop".into()),
                vault: None,
            }),
            last_used: 1,
            tag: Some(Tag::Staging),
            safeguard: None,
        }
    }

    /// Presses Enter until the wizard stops asking.
    fn accept(w: &mut ConnectWizard) -> Step<NewConnection> {
        feed(w, std::iter::repeat_n(enter(), 20))
    }

    #[test]
    fn editing_without_changes_keeps_everything() {
        for (password, required) in [(Some("secret"), false), (None, true), (None, false)] {
            let original = saved(password, required);
            let mut w = ConnectWizard::edit(original.clone(), vec![]);
            assert!(screen(&w).contains("Edit shop (mysql)"), "{}", screen(&w));
            assert_eq!(
                accept(&mut w),
                Step::Done(NewConnection {
                    opts: original.opts,
                    name: Some("shop".into()),
                    tag: Some(Tag::Staging),
                    safeguard: None,
                    save_password: password.is_some(),
                })
            );
            assert!(!screen(&w).contains("secret"));
        }
    }

    #[test]
    fn editing_changes_a_field_and_the_password() {
        let mut w = ConnectWizard::edit(saved(Some("secret"), false), vec![]);
        feed(&mut w, [ctrl('u')]);
        feed(&mut w, typed("other"));
        // host, port, authentication, user
        feed(&mut w, [enter(), enter(), enter(), enter()]);
        assert!(
            screen(&w).contains("Password:  unchanged"),
            "{}",
            screen(&w)
        );
        feed(&mut w, typed("new"));
        let Step::Done(done) = accept(&mut w) else {
            panic!("{}", screen(&w))
        };
        assert!(matches!(done.opts, ConnectOptions::MySql(o)
            if o.host == "other" && o.password.as_deref() == Some("new")));
    }

    #[test]
    fn editing_cannot_take_another_name() {
        let mut w = ConnectWizard::edit(saved(None, false), vec!["other".into()]);
        // host, port, authentication, user, password, database
        feed(&mut w, std::iter::repeat_n(enter(), 6));
        feed(&mut w, [ctrl('u')]);
        feed(&mut w, typed("other"));
        // name, tag, safeguard, save password
        assert_eq!(
            feed(&mut w, std::iter::repeat_n(enter(), 4)),
            Step::Continue
        );
        let s = screen(&w);
        assert!(
            s.contains("a connection called other already exists"),
            "{s}"
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
        // port -> authentication (password) -> user: type "app"
        feed(&mut w, [enter(), enter(), ctrl('u')]);
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
        // Connection name, tag, safeguard, then "Save password" (no by default).
        feed(
            &mut w,
            typed("shopify")
                .into_iter()
                .chain([enter(), enter(), enter()]),
        );
        assert!(screen(&w).contains("Save password: (*) no  ( ) yes"));
        let done = feed(&mut w, [key(KeyCode::Right), enter()]);
        assert!(!screen(&w).contains("secret"));
        assert_eq!(
            done,
            Step::Done(NewConnection {
                opts: ConnectOptions::MySql(MySqlOptions {
                    host: "localhost".into(),
                    port: 3306,
                    user: "app".into(),
                    password: Some("secret".into()),
                    password_required: false,
                    socket: None,
                    database: Some("shop".into()),
                    vault: None,
                }),
                name: Some("shopify".into()),
                tag: None,
                safeguard: None,
                save_password: true,
            })
        );
    }

    #[test]
    fn invalid_port_returns_to_the_field() {
        let mut w = ConnectWizard::new();
        feed(&mut w, [key(KeyCode::Down), enter(), enter(), ctrl('u')]);
        feed(&mut w, typed("nope"));
        assert_eq!(
            feed(&mut w, std::iter::repeat_n(enter(), 9)),
            Step::Continue
        );
        let s = screen(&w);
        assert!(s.contains("not a port number: nope"), "{s}");
        assert!(s.contains("Port: nope"), "{s}");
    }

    #[test]
    fn centered_wizard_is_indented_and_pushed_down() {
        let w = ConnectWizard::new().centered(100, 40);
        let lines = w.render();
        let top = lines.iter().take_while(|l| l.text().is_empty()).count();
        assert_eq!(top, 13);
        let title = lines[top].text();
        assert_eq!(
            title.chars().take_while(|c| *c == ' ').count(),
            18,
            "{title}"
        );
        // Keeps working after a pick.
        let mut w = w;
        feed(&mut w, [enter()]);
        assert!(screen(&w).contains("Database file"));
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
        // port -> authentication (password) -> user: type "app"
        feed(&mut w, [enter(), enter(), ctrl('u')]);
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
        // TLS mode, connection name, tag, safeguard, save password.
        let done = feed(&mut w, std::iter::repeat_n(enter(), 5));
        assert_eq!(
            done,
            Step::Done(NewConnection {
                opts: ConnectOptions::Psql(PsqlOptions {
                    host: "localhost".into(),
                    port: 5432,
                    user: "app".into(),
                    password: Some("secret".into()),
                    password_required: false,
                    dbname: Some("shop".into()),
                    sslmode: SslMode::Disable,
                    sslrootcert: None,
                    vault: None,
                }),
                name: None,
                tag: None,
                safeguard: None,
                save_password: false,
            })
        );
    }

    #[test]
    fn vault_replaces_user_and_password() {
        use crate::auth::vault::{SecretType, VaultConfig};
        let mut w = ConnectWizard::new();
        // PostgreSQL; host, port; pick Vault.
        feed(&mut w, [key(KeyCode::Down), key(KeyCode::Down), enter()]);
        feed(&mut w, [enter(), enter(), key(KeyCode::Right)]);
        assert!(screen(&w).contains("Authentication: ( ) password  (*) vault"));
        feed(&mut w, [enter()]);
        let s = screen(&w);
        assert!(s.contains("Vault secret"), "{s}");
        assert!(!s.contains("User:"), "{s}");
        feed(&mut w, typed("kv/data/shop").into_iter().chain([enter()]));
        feed(
            &mut w,
            typed("http://vault:8200").into_iter().chain([enter()]),
        );
        assert!(screen(&w).contains("Token file: ~/.dato/.vault-token"));
        // token file, certificate, namespace
        feed(&mut w, [enter(), enter(), enter()]);
        // Secret type: kv2 asks for the keys.
        feed(
            &mut w,
            [
                key(KeyCode::Right),
                key(KeyCode::Right),
                key(KeyCode::Right),
            ],
        );
        assert!(screen(&w).contains("(*) kv2"), "{}", screen(&w));
        feed(&mut w, [enter()]);
        assert!(screen(&w).contains("Username key: username"));
        feed(&mut w, [enter(), enter()]);
        // database, TLS mode, connection name, tag, safeguard; no "Save password".
        feed(&mut w, typed("shop").into_iter().chain([enter(), enter()]));
        let done = feed(&mut w, [enter(), enter(), enter()]);
        assert!(!screen(&w).contains("Save password"), "{}", screen(&w));
        let Step::Done(NewConnection {
            opts: ConnectOptions::Psql(p),
            save_password: false,
            ..
        }) = done
        else {
            panic!("{done:?}")
        };
        assert_eq!(p.dbname.as_deref(), Some("shop"));
        assert_eq!(p.user, "");
        assert_eq!(
            p.vault,
            Some(VaultConfig {
                secret: "kv/data/shop".into(),
                address: Some("http://vault:8200".into()),
                token_file: Some("~/.dato/.vault-token".into()),
                secret_type: SecretType::Kv2,
                ..Default::default()
            })
        );
    }

    #[test]
    fn psql_tls_mode_is_picked_with_the_arrow_keys() {
        use crate::client::psql::SslMode;
        let mut w = ConnectWizard::new();
        feed(&mut w, [key(KeyCode::Down), key(KeyCode::Down), enter()]);
        // host, port, authentication
        feed(&mut w, [enter(), enter(), enter()]);
        feed(&mut w, typed("app"));
        // user, password, database
        feed(&mut w, [enter(), enter(), enter()]);
        feed(&mut w, [key(KeyCode::Right), key(KeyCode::Right)]);
        assert!(screen(&w).contains("( ) disable  ( ) prefer  (*) require"));
        let done = feed(&mut w, std::iter::repeat_n(enter(), 5));
        assert!(
            matches!(done, Step::Done(NewConnection { opts: ConnectOptions::Psql(p), .. }) if p.sslmode == SslMode::Require)
        );
    }
}
