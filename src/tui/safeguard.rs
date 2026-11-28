//! Safeguard prompts shown as modals on the terminal. Any terminal failure
//! counts as a refusal.

use crate::safeguard::Prompter;
use crate::ui::organisms::{ConfirmModal, NoticeModal, PasswordModal};
use crate::ui::term;

pub struct Modals;

impl Prompter for Modals {
    fn confirm(&mut self) -> bool {
        let mut modal =
            ConfirmModal::new("This query will UPDATE or DELETE data. Do you want to send it?");
        matches!(term::run(&mut modal), Ok(Some(())))
    }

    fn password(&mut self) -> Option<String> {
        term::run(&mut PasswordModal::default()).ok().flatten()
    }

    fn blocked(&mut self, message: &str) {
        if term::run(&mut NoticeModal::new(message)).is_err() {
            eprintln!("{}", super::error_line(message, super::use_color()));
        }
    }
}
