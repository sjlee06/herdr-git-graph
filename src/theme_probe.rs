//! Read OSC color replies through Crossterm's existing input reader so color
//! detection never competes with keyboard input or leaves late replies as keys.
use crate::theme::Theme;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use std::{
    collections::VecDeque,
    io::{self, Write},
    time::{Duration, Instant},
};

// Esc and an OSC introducer share the same first byte. Allow for separate PTY
// reads and scheduler delays before treating a lone Esc as a user key.
const ESCAPE_TIMEOUT: Duration = Duration::from_millis(250);
const RESPONSE_IDLE_TIMEOUT: Duration = Duration::from_millis(700);

#[derive(Default)]
pub struct ThemeProbe {
    body: Option<String>,
    last_response_input: Option<Instant>,
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
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            // Consume already queued fragments before expiring parser state.
            // A busy frame or a delayed wakeup must not turn buffered OSC bytes
            // into keyboard input. A zero timeout still checks the input queue.
            if !event::poll(remaining)? {
                self.expire();
                return Ok(self.pending.pop_front());
            }
            self.feed(event::read()?, theme);
            if Instant::now() >= deadline {
                return Ok(self.pending.pop_front());
            }
        }
    }

    fn expire(&mut self) {
        // Bound idle, unterminated responses, not the total time spent receiving
        // a valid response. Each fragment gets a fresh window to complete.
        if self
            .last_response_input
            .is_some_and(|at| at.elapsed() >= RESPONSE_IDLE_TIMEOUT)
        {
            self.body = None;
            self.last_response_input = None;
            self.escape = None;
        }
        // Within an OSC response, Esc belongs to the ST terminator. Keep it
        // until the next fragment or the response's idle timeout.
        if self.body.is_none() && self.escape.is_some_and(|at| at.elapsed() >= ESCAPE_TIMEOUT) {
            self.escape = None;
            self.pending.push_back(Event::Key(KeyCode::Esc.into()));
        }
    }

    fn feed(&mut self, event: Event, theme: &mut Theme) {
        let Event::Key(key) = event else {
            self.pending.push_back(event);
            return;
        };
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.body = None;
            self.last_response_input = None;
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
            self.last_response_input = Some(Instant::now());
            return;
        }
        if let Some(body) = &mut self.body {
            self.last_response_input = Some(Instant::now());
            if (escaped && key.code == KeyCode::Char('\\'))
                || (key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL))
            {
                theme.apply_response(body);
                self.body = None;
                self.last_response_input = None;
            } else if key.code == KeyCode::Esc {
                self.escape = Some(Instant::now());
            } else if let KeyCode::Char(c) = key.code
                && body.len() < 512
            {
                body.push(c);
            } else {
                // An overlong response or an unrelated key cannot complete a
                // color reply. Recover without swallowing the user's key.
                self.body = None;
                self.last_response_input = None;
                self.pending.push_back(event);
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

    #[test]
    fn keeps_a_delayed_osc_introducer_out_of_keyboard_input() {
        let mut probe = ThemeProbe::default();
        let mut theme = Theme::default();
        probe.feed(Event::Key(KeyCode::Esc.into()), &mut theme);
        probe.escape = Some(Instant::now() - Duration::from_millis(100));
        probe.expire();
        assert!(probe.pending.is_empty());
        for c in "]11;rgb:12/18/20".chars() {
            probe.feed(key(c, KeyModifiers::NONE), &mut theme);
        }
        probe.feed(key('g', KeyModifiers::CONTROL), &mut theme);
        assert_eq!(theme.background, Some((18, 24, 32)));
        assert!(probe.pending.is_empty());
    }

    #[test]
    fn slow_response_fragments_renew_the_idle_timeout() {
        let mut probe = ThemeProbe::default();
        let mut theme = Theme::default();
        probe.feed(key(']', KeyModifiers::ALT), &mut theme);
        for c in "11;rgb:12/18/20".chars() {
            // Each fragment can arrive near the idle deadline, even when the
            // complete response takes much longer than that deadline.
            probe.last_response_input = Some(Instant::now() - Duration::from_millis(600));
            probe.expire();
            let before = Instant::now();
            probe.feed(key(c, KeyModifiers::NONE), &mut theme);
            assert!(probe.last_response_input.is_some_and(|at| at >= before));
        }
        probe.feed(Event::Key(KeyCode::Esc.into()), &mut theme);
        probe.escape = Some(Instant::now() - Duration::from_millis(500));
        probe.last_response_input = probe.escape;
        probe.expire();
        probe.feed(key('\\', KeyModifiers::NONE), &mut theme);
        assert_eq!(theme.background, Some((18, 24, 32)));
        assert!(probe.pending.is_empty());
        probe.feed(key('q', KeyModifiers::NONE), &mut theme);
        assert_eq!(
            probe.pending.pop_front(),
            Some(key('q', KeyModifiers::NONE))
        );
    }

    #[test]
    fn lone_escape_and_idle_response_recover_without_leaking_terminators() {
        let mut probe = ThemeProbe::default();
        let mut theme = Theme::default();
        probe.feed(Event::Key(KeyCode::Esc.into()), &mut theme);
        probe.escape = Some(Instant::now() - ESCAPE_TIMEOUT);
        probe.expire();
        assert_eq!(
            probe.pending.pop_front(),
            Some(Event::Key(KeyCode::Esc.into()))
        );
        probe.feed(key(']', KeyModifiers::ALT), &mut theme);
        probe.feed(Event::Key(KeyCode::Esc.into()), &mut theme);
        probe.last_response_input = Some(Instant::now() - RESPONSE_IDLE_TIMEOUT);
        probe.expire();
        assert!(probe.pending.is_empty());
        assert!(probe.escape.is_none());
        probe.feed(key('q', KeyModifiers::NONE), &mut theme);
        assert_eq!(
            probe.pending.pop_front(),
            Some(key('q', KeyModifiers::NONE))
        );
    }

    #[test]
    fn malformed_responses_preserve_unrelated_keys_and_bound_buffer_size() {
        let mut probe = ThemeProbe::default();
        let mut theme = Theme::default();
        probe.feed(key(']', KeyModifiers::ALT), &mut theme);
        let enter = Event::Key(KeyCode::Enter.into());
        probe.feed(enter.clone(), &mut theme);
        assert_eq!(probe.pending.pop_front(), Some(enter));
        assert!(probe.body.is_none());

        probe.feed(key(']', KeyModifiers::ALT), &mut theme);
        for _ in 0..512 {
            probe.feed(key('1', KeyModifiers::NONE), &mut theme);
        }
        assert!(probe.pending.is_empty());
        probe.feed(key('q', KeyModifiers::NONE), &mut theme);
        assert!(probe.body.is_none());
        assert_eq!(
            probe.pending.pop_front(),
            Some(key('q', KeyModifiers::NONE))
        );
    }
}
