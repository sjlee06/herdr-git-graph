//! Read OSC color replies through Crossterm's existing input reader so color
//! detection never competes with keyboard input or leaves late replies as keys.
use crate::theme::Theme;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use std::{
    collections::VecDeque,
    io::{self, Write},
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct ThemeProbe {
    body: Option<String>,
    started: Option<Instant>,
    escape: Option<Instant>,
    pending: VecDeque<Event>,
}

impl ThemeProbe {
    pub fn query(&self) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        stdout.write_all(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\")?;
        for index in 1..=6 {
            write!(stdout, "\x1b]4;{index};?\x1b\\")?;
        }
        stdout.flush()
    }

    pub fn read(&mut self, timeout: Duration, theme: &mut Theme) -> io::Result<Option<Event>> {
        let deadline = Instant::now() + timeout;
        loop {
            self.expire();
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || !event::poll(remaining)? {
                self.expire();
                return Ok(self.pending.pop_front());
            }
            self.feed(event::read()?, theme);
        }
    }

    fn expire(&mut self) {
        // Bound malformed or unterminated responses; normal input remains usable.
        if self
            .started
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(700))
        {
            self.body = None;
            self.started = None;
        }
        if self
            .escape
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(40))
        {
            self.escape = None;
            if self.body.is_none() {
                self.pending.push_back(Event::Key(KeyCode::Esc.into()));
            }
        }
    }

    fn feed(&mut self, event: Event, theme: &mut Theme) {
        let Event::Key(key) = event else {
            self.pending.push_back(event);
            return;
        };
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.body = None;
            self.started = None;
            self.escape = None;
            self.pending.push_back(event);
            return;
        }
        let pending_escape = self.escape.take().is_some();
        if pending_escape && self.body.is_none() && key.modifiers.contains(KeyModifiers::ALT) {
            self.pending.push_back(Event::Key(KeyCode::Esc.into()));
        }
        let escaped = key.modifiers.contains(KeyModifiers::ALT) || pending_escape;
        // Crossterm 0.29 decodes an OSC introducer as Alt+], and ST as Alt+\.
        // Keep a separate Esc briefly as terminals may split those bytes across reads.
        if escaped && key.code == KeyCode::Char(']') {
            self.body = Some(String::new());
            self.started = Some(Instant::now());
            return;
        }
        if let Some(body) = &mut self.body {
            if (escaped && key.code == KeyCode::Char('\\'))
                || (key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                theme.apply_response(body);
                self.body = None;
                self.started = None;
            } else if key.code == KeyCode::Esc {
                self.escape = Some(Instant::now());
            } else if let KeyCode::Char(c) = key.code
                && body.len() < 512
            {
                body.push(c);
            }
            return;
        }
        if escaped && !key.modifiers.contains(KeyModifiers::ALT) {
            self.pending.push_back(Event::Key(KeyCode::Esc.into()));
        }
        if key.code == KeyCode::Esc {
            self.escape = Some(Instant::now());
        } else {
            self.pending.push_back(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;

    fn key(code: char, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(code), modifiers))
    }

    #[test]
    fn consumes_bel_and_st_replies_but_preserves_input_and_resize() {
        let mut probe = ThemeProbe::default();
        let mut theme = Theme::default();
        let navigate = key('j', KeyModifiers::NONE);
        probe.feed(navigate.clone(), &mut theme);
        for end in [
            key('g', KeyModifiers::CONTROL),
            key('\\', KeyModifiers::ALT),
        ] {
            probe.feed(key(']', KeyModifiers::ALT), &mut theme);
            for c in "11;rgb:eeee/eeee/eeee".chars() {
                probe.feed(key(c, KeyModifiers::NONE), &mut theme);
            }
            probe.feed(Event::Resize(80, 24), &mut theme);
            probe.feed(end, &mut theme);
        }
        assert_eq!(theme.background, Some((238, 238, 238)));
        assert_eq!(
            probe.pending,
            [navigate, Event::Resize(80, 24), Event::Resize(80, 24)]
        );
    }

    #[test]
    fn handles_fragmented_escapes_and_never_swallows_ctrl_c() {
        let mut probe = ThemeProbe::default();
        let mut theme = Theme::default();
        probe.feed(Event::Key(KeyCode::Esc.into()), &mut theme);
        for c in "]10;rgb:22/33/44".chars() {
            probe.feed(key(c, KeyModifiers::NONE), &mut theme);
        }
        probe.feed(Event::Key(KeyCode::Esc.into()), &mut theme);
        probe.feed(key('\\', KeyModifiers::NONE), &mut theme);
        assert_eq!(theme.foreground, Some((34, 51, 68)));
        assert!(probe.pending.is_empty());
        probe.feed(key(']', KeyModifiers::ALT), &mut theme);
        probe.feed(key('c', KeyModifiers::CONTROL), &mut theme);
        assert_eq!(
            probe.pending.pop_front(),
            Some(key('c', KeyModifiers::CONTROL))
        );
    }

    #[test]
    fn preserves_user_escape_immediately_before_a_complete_reply() {
        let mut probe = ThemeProbe::default();
        let mut theme = Theme::default();
        probe.feed(Event::Key(KeyCode::Esc.into()), &mut theme);
        probe.feed(key(']', KeyModifiers::ALT), &mut theme);
        for c in "11;rgb:00/11/22".chars() {
            probe.feed(key(c, KeyModifiers::NONE), &mut theme);
        }
        probe.feed(key('\\', KeyModifiers::ALT), &mut theme);
        assert_eq!(theme.background, Some((0, 17, 34)));
        assert_eq!(
            probe.pending.pop_front(),
            Some(Event::Key(KeyCode::Esc.into()))
        );
        assert!(probe.pending.is_empty());
    }
}
