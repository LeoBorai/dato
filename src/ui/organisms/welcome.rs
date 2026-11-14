//! The welcome screen: the logo and a modal with the saved connections.

use crossterm::event::{KeyCode, KeyEvent};
use nu_ansi_term::{Color, Style};
use unicode_width::UnicodeWidthStr;

use crate::server::Server;
use crate::ui::{Line, Step, Widget, is_cancel, server_color};

const LOGO: &str = "\
██████╗  █████╗ ████████╗ ██████╗
██╔══██╗██╔══██╗╚══██╔══╝██╔═══██╗
██║  ██║███████║   ██║   ██║   ██║
██║  ██║██╔══██║   ██║   ██║   ██║
██████╔╝██║  ██║   ██║   ╚██████╔╝
╚═════╝ ╚═╝  ╚═╝   ╚═╝    ╚═════╝";

/// Gradient stops, left to right: PostgreSQL, MySQL and SQLite.
const GRADIENT: [(u8, u8, u8); 3] = [(0, 200, 230), (255, 200, 0), (170, 90, 230)];

/// The gradient color at `t` in `0.0..=1.0`.
fn gradient_at(t: f32) -> Color {
    let scaled = t.clamp(0.0, 1.0) * (GRADIENT.len() - 1) as f32;
    let i = (scaled as usize).min(GRADIENT.len() - 2);
    let f = scaled - i as f32;
    let (a, b) = (GRADIENT[i], GRADIENT[i + 1]);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f).round() as u8;
    Color::Rgb(mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// One line with the gradient spread over `width` columns, .
fn gradient_line(text: &str, width: usize) -> Line {
    let mut line = Line::default();
    for (col, c) in text.chars().enumerate() {
        let t = col as f32 / width.saturating_sub(1).max(1) as f32;
        line.push(Style::new().bold().fg(gradient_at(t)), c.to_string());
    }
    line
}

/// A saved connection as the list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WelcomeItem {
    pub name: String,
    /// `user@host:port` or a file.
    pub detail: String,
    pub server: Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WelcomeChoice {
    /// Run the connect wizard.
    New,
    /// Index into the items given to [`Welcome::new`].
    Saved(usize),
}

pub struct Welcome {
    items: Vec<WelcomeItem>,
    /// 0 is "Create a new connection"; `n` is saved connection `n - 1`.
    selected: usize,
    /// Terminal `(columns, rows)`; when set, the screen is centered in it.
    size: Option<(usize, usize)>,
}

/// Indents every line of `block` so the block is centered in `columns`.
fn center_block(block: &mut [Line], columns: usize) {
    let width = block.iter().map(|l| l.text().width()).max().unwrap_or(0);
    let pad = columns.saturating_sub(width) / 2;
    for line in block {
        line.0.insert(0, (Style::new(), " ".repeat(pad)));
    }
}

impl Welcome {
    pub fn new(items: Vec<WelcomeItem>) -> Self {
        Self {
            items,
            selected: 0,
            size: None,
        }
    }

    /// Centers the screen in a terminal of `columns` x `rows`.
    pub fn centered(mut self, columns: usize, rows: usize) -> Self {
        self.size = Some((columns, rows));
        self
    }

    fn rows(&self) -> usize {
        self.items.len() + 1
    }

    fn choice(&self) -> WelcomeChoice {
        match self.selected {
            0 => WelcomeChoice::New,
            n => WelcomeChoice::Saved(n - 1),
        }
    }
}

impl Widget for Welcome {
    type Output = WelcomeChoice;

    fn on_key(&mut self, key: KeyEvent) -> Step<WelcomeChoice> {
        if is_cancel(&key) {
            return Step::Cancel;
        }
        let n = self.rows();
        match key.code {
            KeyCode::Up | KeyCode::BackTab => self.selected = (self.selected + n - 1) % n,
            KeyCode::Down | KeyCode::Tab => self.selected = (self.selected + 1) % n,
            KeyCode::Home => self.selected = 0,
            KeyCode::End => self.selected = n - 1,
            KeyCode::Enter => return Step::Done(self.choice()),
            _ => {}
        }
        Step::Continue
    }

    fn render(&self) -> Vec<Line> {
        let (mut logo, mut modal, mut hint) = self.sections();
        let Some((columns, rows)) = self.size else {
            logo.push(Line::default());
            logo.extend(modal);
            logo.extend(hint);
            return logo;
        };
        center_block(&mut logo, columns);
        center_block(&mut modal, columns);
        center_block(&mut hint, columns);
        let height = logo.len() + 1 + modal.len() + hint.len();
        let mut out = vec![Line::default(); rows.saturating_sub(height) / 2];
        out.extend(logo);
        out.push(Line::default());
        out.extend(modal);
        out.extend(hint);
        out
    }
}

impl Welcome {
    /// The logo, the modal box and the hint line.
    fn sections(&self) -> (Vec<Line>, Vec<Line>, Vec<Line>) {
        let accent = Style::new().bold().fg(Color::Cyan);
        let border = Style::new().fg(Color::DarkGray);
        let logo_width = LOGO.lines().map(|l| l.width()).max().unwrap_or(0);
        let logo: Vec<Line> = LOGO.lines().map(|l| gradient_line(l, logo_width)).collect();
        let mut out: Vec<Line> = Vec::new();

        // Modal body: each row is its spans, without the border.
        let mut rows: Vec<Line> = Vec::new();
        let marker = |i: usize| if i == self.selected { "> " } else { "  " };
        let text = |i: usize| {
            if i == self.selected {
                accent
            } else {
                Style::new()
            }
        };
        rows.push(Line::styled(
            text(0),
            format!("{}+ Create a new connection", marker(0)),
        ));
        let mut width = rows[0].text().width();
        let saved: Vec<Line> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let i = i + 1;
                let mut line = Line::styled(text(i), marker(i));
                line.push(Style::new().fg(server_color(item.server)), "●");
                line.push(text(i), format!(" {} ", item.name));
                line.push(
                    Style::new().fg(Color::DarkGray),
                    format!("({})", item.detail),
                );
                line
            })
            .collect();
        width = saved
            .iter()
            .map(|l| l.text().width())
            .fold(width, usize::max);
        if !saved.is_empty() {
            rows.push(Line::styled(border, "─".repeat(width)));
            rows.extend(saved);
        }

        out.push(Line::styled(border, format!("┌─{}─┐", "─".repeat(width))));
        for row in rows {
            let pad = width - row.text().width();
            let mut line = Line::styled(border, "│ ");
            line.extend(row);
            line.push(Style::new(), " ".repeat(pad));
            line.push(border, " │");
            out.push(line);
        }
        out.push(Line::styled(border, format!("└─{}─┘", "─".repeat(width))));
        let hint = vec![Line::styled(
            border,
            "Up/Down to choose, Enter to select, Esc for an empty shell",
        )];
        (logo, out, hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::test_keys::{ctrl, enter, feed, key};

    fn item(name: &str, server: Server) -> WelcomeItem {
        WelcomeItem {
            name: name.into(),
            detail: "username@localhost:5432".into(),
            server,
        }
    }

    fn screen(w: &Welcome) -> String {
        w.render()
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn shows_logo_new_option_and_saved_connections() {
        let w = Welcome::new(vec![item("shopify-database", Server::PostgreSql)]);
        let s = screen(&w);
        assert!(s.starts_with("██████╗  █████╗"), "{s}");
        assert!(s.contains("> + Create a new connection"), "{s}");
        assert!(
            s.contains("  ● shopify-database (username@localhost:5432)"),
            "{s}"
        );
        // The box is rectangular.
        let widths: Vec<_> = w.render()[7..11].iter().map(|l| l.text().width()).collect();
        assert!(widths.windows(2).all(|p| p[0] == p[1]), "{widths:?}\n{s}");
    }

    #[test]
    fn dots_use_the_server_color() {
        let w = Welcome::new(vec![
            item("a", Server::PostgreSql),
            item("b", Server::MySql),
            item("c", Server::Sqlite),
        ]);
        let colors: Vec<_> = w
            .render()
            .iter()
            .flat_map(|l| l.0.iter())
            .filter(|(_, t)| t == "●")
            .map(|(s, _)| s.foreground)
            .collect();
        assert_eq!(
            colors,
            [Some(Color::Cyan), Some(Color::Yellow), Some(Color::Purple)]
        );
    }

    #[test]
    fn arrows_pick_new_or_saved() {
        let items = vec![item("a", Server::Sqlite), item("b", Server::MySql)];
        assert_eq!(
            feed(&mut Welcome::new(items.clone()), [enter()]),
            Step::Done(WelcomeChoice::New)
        );
        assert_eq!(
            feed(
                &mut Welcome::new(items.clone()),
                [key(KeyCode::Down), enter()]
            ),
            Step::Done(WelcomeChoice::Saved(0))
        );
        // Wraps to the last saved connection.
        assert_eq!(
            feed(&mut Welcome::new(items), [key(KeyCode::Up), enter()]),
            Step::Done(WelcomeChoice::Saved(1))
        );
    }

    #[test]
    fn without_saved_connections_there_is_no_separator() {
        let s = screen(&Welcome::new(vec![]));
        assert!(
            !s.contains("├") && !s.contains("────────────────\n│"),
            "{s}"
        );
        assert_eq!(
            feed(&mut Welcome::new(vec![]), [key(KeyCode::Down), enter()]),
            Step::Done(WelcomeChoice::New)
        );
    }

    #[test]
    fn logo_fades_from_cyan_through_yellow_to_purple() {
        let w = Welcome::new(vec![]);
        let colors: Vec<_> = w.render()[0].0.iter().map(|(s, _)| s.foreground).collect();
        assert_eq!(colors[0], Some(Color::Rgb(0, 200, 230)));
        assert_eq!(gradient_at(0.5), Color::Rgb(255, 200, 0));
        assert_eq!(gradient_at(1.0), Color::Rgb(170, 90, 230));
        assert_ne!(colors[0], colors[colors.len() - 1]);
    }

    #[test]
    fn centers_in_the_terminal() {
        let w = Welcome::new(vec![item("a", Server::Sqlite)]).centered(100, 40);
        let lines = w.render();
        // Vertically: blank rows above, and the content is not taller than the screen.
        let top = lines.iter().take_while(|l| l.text().is_empty()).count();
        assert!(
            top > 5 && lines.len() <= 40,
            "top {top}, len {}",
            lines.len()
        );
        // Horizontally: the box has equal margins on both sides.
        let border = lines
            .iter()
            .find(|l| l.text().contains('┌'))
            .unwrap()
            .text();
        let left = border.chars().take_while(|c| *c == ' ').count();
        let right = 100 - left - border.trim_start().width();
        assert!(left.abs_diff(right) <= 1, "{left} {right}");
        // Logo rows share one margin.
        let logo: Vec<_> = lines
            .iter()
            .filter(|l| l.text().contains('█'))
            .map(|l| l.text().chars().take_while(|c| *c == ' ').count())
            .collect();
        assert!(
            logo.iter().all(|m| *m == logo[0]) && logo[0] > 20,
            "{logo:?}"
        );
    }

    #[test]
    fn esc_and_ctrl_c_cancel() {
        assert_eq!(
            feed(&mut Welcome::new(vec![]), [key(KeyCode::Esc)]),
            Step::Cancel
        );
        assert_eq!(feed(&mut Welcome::new(vec![]), [ctrl('c')]), Step::Cancel);
    }
}
