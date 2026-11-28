//! Safeguard levels: checks made before `UPDATE` and `DELETE` statements reach
//! the server, to reduce human mistakes.
//!
//! This is not a security boundary: it reads the statement text only, so writes
//! made by functions, procedures or triggers are not seen. Use database
//! permissions to forbid writes.

use crate::client::ConnectOptions;
use crate::dialect::LexRules;
use crate::sql::{TokenKind, tokenize_with};

/// Shown when level 2 cannot ask for a password.
pub const BLOCKED_MESSAGE: &str = "You cant perform UPDATE or DELETE queries against this \
                                   database due to Safeguard Level 2 security rules.";

/// Shown when level 2 gets a password other than the connection's.
pub const WRONG_PASSWORD_MESSAGE: &str = "Wrong password. The query was not sent.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Safeguard {
    /// Level 1: confirm before sending.
    Confirm,
    /// Level 2: ask for the connection's password; block other auth methods.
    Password,
}

impl Safeguard {
    /// The level as stored in the file and shown in the wizard.
    pub fn name(self) -> &'static str {
        match self {
            Safeguard::Confirm => "1",
            Safeguard::Password => "2",
        }
    }

    /// What `level` does, for the connect wizard.
    pub fn hint(level: Option<Self>) -> &'static str {
        match level {
            None => "UPDATE and DELETE are sent right away",
            Some(Safeguard::Confirm) => "Confirm before sending UPDATE or DELETE",
            Some(Safeguard::Password) => "UPDATE/DELETE need the password; blocked without one",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "1" => Some(Safeguard::Confirm),
            "2" => Some(Safeguard::Password),
            _ => None,
        }
    }
}

/// Asks the user on behalf of a safeguard.
pub trait Prompter {
    /// Level 1: whether to send the query.
    fn confirm(&mut self) -> bool;
    /// Level 2: the password typed; `None` when cancelled.
    fn password(&mut self) -> Option<String>;
    /// Tells the user why the query was not sent.
    fn blocked(&mut self, message: &str);
}

/// Refuses everything: for sessions with nobody to ask.
pub struct Deny;

impl Prompter for Deny {
    fn confirm(&mut self) -> bool {
        false
    }

    fn password(&mut self) -> Option<String> {
        None
    }

    fn blocked(&mut self, _message: &str) {}
}

/// The password the connection authenticates with; `None` for any other
/// authentication method (Vault, SQLite, no password).
fn auth_password(opts: &ConnectOptions) -> Option<&str> {
    let (password, vault) = match opts {
        ConnectOptions::Sqlite { .. } => return None,
        ConnectOptions::MySql(o) => (&o.password, &o.vault),
        ConnectOptions::Psql(o) => (&o.password, &o.vault),
    };
    password
        .as_deref()
        .filter(|p| vault.is_none() && !p.is_empty())
}

/// Whether a guarded query may be sent over the connection made with `opts`.
pub fn allows(
    level: Safeguard,
    opts: Option<&ConnectOptions>,
    prompter: &mut dyn Prompter,
) -> bool {
    match level {
        Safeguard::Confirm => prompter.confirm(),
        Safeguard::Password => {
            let Some(expected) = opts.and_then(auth_password) else {
                prompter.blocked(BLOCKED_MESSAGE);
                return false;
            };
            match prompter.password() {
                Some(typed) if typed == expected => true,
                Some(_) => {
                    prompter.blocked(WRONG_PASSWORD_MESSAGE);
                    false
                }
                None => false,
            }
        }
    }
}

/// Whether `sql` updates or deletes rows. `UPDATE` and `DELETE` count where
/// they start an operation, so `FOR UPDATE`, `ON DELETE CASCADE` or
/// `GRANT UPDATE` do not.
pub fn is_guarded(sql: &str, rules: &LexRules) -> bool {
    let mut words = Vec::new();
    for t in tokenize_with(sql, rules) {
        let text = t.text(sql);
        match t.kind {
            TokenKind::Whitespace | TokenKind::LineComment => {}
            TokenKind::BlockComment => {
                // MySQL runs the content of `/*! ... */`.
                if let Some(body) = text.strip_prefix("/*!") {
                    let body = body.strip_suffix("*/").unwrap_or(body);
                    let body = body.trim_start_matches(|c: char| c.is_ascii_digit());
                    if is_guarded(body, rules) {
                        return true;
                    }
                }
            }
            _ => words.push(text.to_ascii_lowercase()),
        }
    }
    // A plain `EXPLAIN` only shows the plan.
    if words.first().is_some_and(|w| w == "explain")
        && !words.iter().any(|w| w == "analyze" || w == "analyse")
    {
        return false;
    }
    words.iter().enumerate().any(|(i, word)| {
        if word != "update" && word != "delete" {
            return false;
        }
        let Some(previous) = i.checked_sub(1).map(|p| words[p].as_str()) else {
            return true;
        };
        match previous {
            // CTEs, prepared statements, `EXPLAIN ANALYZE`, a new statement.
            "(" | ")" | ";" | "as" | "analyze" | "analyse" | "verbose" => true,
            // Upserts: `ON CONFLICT DO UPDATE`, `ON DUPLICATE KEY UPDATE`.
            "do" => true,
            "key" => i >= 2 && words[i - 2] == "duplicate",
            _ => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::PsqlOptions;
    use crate::dialect::{mysql, psql};

    #[test]
    fn detects_updates_and_deletes() {
        for sql in [
            "update t set a = 1",
            "  DELETE FROM t",
            "-- c\n/* d */ delete from t",
            "with x as (select 1) delete from t",
            "with x as (delete from t returning *) select * from x",
            "explain analyze delete from t",
            "explain (analyze, verbose) update t set a = 1",
            "prepare p as delete from t",
            "insert into t values (1) on conflict (a) do update set a = 2",
        ] {
            assert!(is_guarded(sql, &psql::RULES), "{sql}");
        }
        for sql in [
            "insert into t values (1) on duplicate key update a = 2",
            "/*!40000 delete from t */",
        ] {
            assert!(is_guarded(sql, &mysql::RULES), "{sql}");
        }
    }

    #[test]
    fn ignores_other_statements() {
        for sql in [
            "select 'delete from t'",
            "select \"update\" from t",
            "select * from t for update",
            "select * from t for no key update",
            "create table a(b int references c on delete cascade on update cascade)",
            "grant update, delete on t to u",
            "explain delete from t",
            "select updated_at, deleted from t",
            "-- delete from t",
        ] {
            assert!(!is_guarded(sql, &psql::RULES), "{sql}");
        }
    }

    #[derive(Default)]
    struct Fake {
        typed: Option<&'static str>,
        blocked: Vec<String>,
    }

    impl Prompter for Fake {
        fn confirm(&mut self) -> bool {
            true
        }
        fn password(&mut self) -> Option<String> {
            self.typed.map(String::from)
        }
        fn blocked(&mut self, message: &str) {
            self.blocked.push(message.into());
        }
    }

    #[test]
    fn level_two_needs_the_connection_password() {
        let opts = ConnectOptions::Psql(PsqlOptions {
            password: Some("pw".into()),
            ..Default::default()
        });
        let check = |typed, opts: &ConnectOptions| {
            let mut f = Fake {
                typed,
                ..Default::default()
            };
            let ok = allows(Safeguard::Password, Some(opts), &mut f);
            (ok, f.blocked)
        };
        assert_eq!(check(Some("pw"), &opts), (true, vec![]));
        assert_eq!(
            check(Some("no"), &opts),
            (false, vec![WRONG_PASSWORD_MESSAGE.to_string()])
        );
        assert_eq!(check(None, &opts), (false, vec![]));

        let vault = ConnectOptions::Psql(PsqlOptions {
            password: Some("pw".into()),
            vault: Some(Default::default()),
            ..Default::default()
        });
        for other in [vault, ConnectOptions::sqlite(None)] {
            assert_eq!(
                check(Some("pw"), &other),
                (false, vec![BLOCKED_MESSAGE.to_string()])
            );
        }
    }
}
