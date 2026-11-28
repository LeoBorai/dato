//! Records whether the last key typed was a space, which the completer cannot
//! tell from the line alone: Tab after a space asks for more than typing it does.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::event::{Event, KeyCode};
use reedline::{EditMode, Emacs, PromptEditMode, ReedlineEvent, ReedlineRawEvent};

pub struct SpaceTracker {
    inner: Emacs,
    after_space: Arc<AtomicBool>,
}

impl SpaceTracker {
    pub fn new(inner: Emacs, after_space: Arc<AtomicBool>) -> Self {
        Self { inner, after_space }
    }
}

impl EditMode for SpaceTracker {
    fn parse_event(&mut self, event: ReedlineRawEvent) -> ReedlineEvent {
        let raw: Event = event.into();
        if let Event::Key(key) = &raw {
            self.after_space
                .store(key.code == KeyCode::Char(' '), Ordering::Relaxed);
        }
        match ReedlineRawEvent::try_from(raw) {
            Ok(event) => self.inner.parse_event(event),
            Err(()) => ReedlineEvent::None,
        }
    }

    fn edit_mode(&self) -> PromptEditMode {
        self.inner.edit_mode()
    }
}
