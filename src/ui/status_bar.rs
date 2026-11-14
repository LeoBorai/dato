//! The status bar: one line pinned to the bottom of the shell.

use nu_ansi_term::{Color, Style};
use unicode_width::UnicodeWidthChar;

use crate::server::Server;
use crate::ui::{Line, contrast_color, server_color};

/// What the bar shows about the current connection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StatusInfo {
    /// `None` while disconnected.
    pub server: Option<Server>,
    /// The saved connection's name.
    pub name: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
}

const SEPARATOR: &str = " │ ";

impl StatusInfo {
    /// The left side: host and port, database and connection name; parts that
    /// are not known are left out.
    pub fn left(&self) -> String {
        if self.server.is_none() {
            return "not connected".into();
        }
        let address = match (&self.host, self.port) {
            (Some(h), Some(p)) => Some(format!("{h}:{p}")),
            (Some(h), None) => Some(h.clone()),
            (None, Some(p)) => Some(format!(":{p}")),
            (None, None) => None,
        };
        [address, self.database.clone(), self.name.clone()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(SEPARATOR)
    }

    /// The bar as one line of exactly `width` columns: the server's color as
    /// background and a contrasting text color. The right side is empty.
    pub fn render(&self, width: usize) -> Line {
        let bg = self.server.map_or(Color::DarkGray, server_color);
        let style = Style::new().on(bg).fg(contrast_color(bg));
        let mut text = String::from(" ");
        let mut used = 1;
        for c in self.left().chars() {
            let w = c.width().unwrap_or(0);
            if used + w > width {
                break;
            }
            text.push(c);
            used += w;
        }
        text.push_str(&" ".repeat(width - used));
        Line::styled(style.bold(), text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    fn pg() -> StatusInfo {
        StatusInfo {
            server: Some(Server::PostgreSql),
            name: Some("shopify".into()),
            host: Some("localhost".into()),
            port: Some(5432),
            database: Some("shop".into()),
        }
    }

    #[test]
    fn left_side_has_host_port_database_and_name() {
        assert_eq!(pg().left(), "localhost:5432 │ shop │ shopify");
        let lite = StatusInfo {
            server: Some(Server::Sqlite),
            database: Some("app.db".into()),
            ..Default::default()
        };
        assert_eq!(lite.left(), "app.db");
        assert_eq!(StatusInfo::default().left(), "not connected");
    }

    #[test]
    fn fills_the_width_in_the_server_color_with_contrast_text() {
        for (server, bg, fg) in [
            (Server::PostgreSql, Color::Cyan, Color::Black),
            (Server::MySql, Color::Yellow, Color::Black),
            (Server::Sqlite, Color::Purple, Color::White),
        ] {
            let line = StatusInfo {
                server: Some(server),
                ..pg()
            }
            .render(60);
            assert_eq!(line.text().width(), 60);
            let style = line.0[0].0;
            assert_eq!((style.background, style.foreground), (Some(bg), Some(fg)));
        }
    }

    #[test]
    fn long_text_is_cut_to_the_width() {
        let line = pg().render(10);
        assert_eq!(line.text().width(), 10);
        assert_eq!(line.text(), " localhost");
    }
}
