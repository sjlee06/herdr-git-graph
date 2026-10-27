use crossterm::event::{self, Event, KeyEventKind, MouseEventKind};
use herdr_git_graph::{theme::Theme, theme_probe::ThemeProbe};
use std::{
    io,
    time::{Duration, Instant},
};

const SCROLL_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const IDLE_TIMEOUT: Duration = Duration::from_millis(40);

pub fn read(
    probe: &mut Option<ThemeProbe>,
    theme: &mut Theme,
    timeout: Duration,
) -> io::Result<Option<Event>> {
    if let Some(probe) = probe {
        probe.read(timeout, theme)
    } else if event::poll(timeout)? {
        event::read().map(Some)
    } else {
        Ok(None)
    }
}

pub fn is_wheel(input: &Event) -> bool {
    matches!(input, Event::Mouse(mouse)
        if matches!(mouse.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown))
}

pub fn needs_current_view(input: &Event) -> bool {
    match input {
        Event::Key(key) => key.kind != KeyEventKind::Release,
        Event::Mouse(mouse) => matches!(mouse.kind, MouseEventKind::Down(_)),
        Event::Resize(_, _) => true,
        _ => false,
    }
}

/// Pace wheel frames from the last draw's start, including its rendering cost.
/// Idle input draws immediately; continuous input never extends a pending deadline.
#[derive(Default)]
pub struct ScrollFrames {
    last_rendered: Option<Instant>,
    deadline: Option<Instant>,
}

impl ScrollFrames {
    pub fn scrolled(&mut self, now: Instant) {
        self.deadline.get_or_insert_with(|| {
            self.last_rendered
                .map_or(now, |last| (last + SCROLL_FRAME_INTERVAL).max(now))
        });
    }

    pub fn pending(&self) -> bool {
        self.deadline.is_some()
    }

    pub fn ready(&self, now: Instant) -> bool {
        self.deadline.is_none_or(|deadline| now >= deadline)
    }

    pub fn timeout(&self, now: Instant) -> Duration {
        self.deadline.map_or(IDLE_TIMEOUT, |deadline| {
            deadline.saturating_duration_since(now).min(IDLE_TIMEOUT)
        })
    }

    pub fn flush(&mut self, now: Instant) {
        self.deadline = Some(now);
    }

    pub fn rendered(&mut self, started: Instant) {
        self.last_rendered = Some(started);
        self.deadline = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_wheel_after_idle_is_ready_without_a_fresh_frame_delay() {
        let now = Instant::now();
        let mut frames = ScrollFrames::default();
        frames.scrolled(now);
        assert!(frames.ready(now));
        assert_eq!(frames.timeout(now), Duration::ZERO);
        frames.rendered(now);
        let later = now + Duration::from_millis(100);
        frames.scrolled(later);
        assert!(frames.ready(later));
        assert_eq!(frames.timeout(later), Duration::ZERO);
    }

    #[test]
    fn wheel_burst_shares_the_previous_frames_deadline() {
        let now = Instant::now();
        let mut frames = ScrollFrames::default();
        assert_eq!(frames.timeout(now), IDLE_TIMEOUT);
        frames.rendered(now);
        for elapsed in 1..16 {
            let at = now + Duration::from_millis(elapsed);
            frames.scrolled(at);
            assert!(!frames.ready(at));
            assert_eq!(frames.timeout(at), Duration::from_millis(16 - elapsed));
        }
        assert!(frames.pending());
        assert!(frames.ready(now + SCROLL_FRAME_INTERVAL));
        frames.rendered(now + SCROLL_FRAME_INTERVAL);
        frames.scrolled(now + Duration::from_millis(17));
        assert_eq!(
            frames.timeout(now + Duration::from_millis(17)),
            Duration::from_millis(15)
        );
        assert!(frames.ready(now + Duration::from_millis(32)));
    }

    #[test]
    fn rendering_cost_counts_toward_the_frame_interval() {
        let now = Instant::now();
        let mut frames = ScrollFrames::default();
        frames.rendered(now);
        let finished = now + Duration::from_millis(10);
        frames.scrolled(finished);
        assert_eq!(frames.timeout(finished), Duration::from_millis(6));
        frames.rendered(now + SCROLL_FRAME_INTERVAL);
        let slow_finish = now + Duration::from_millis(40);
        frames.scrolled(slow_finish);
        assert!(frames.ready(slow_finish));
        assert_eq!(frames.timeout(slow_finish), Duration::ZERO);
    }

    #[test]
    fn non_wheel_input_can_flush_before_the_next_frame() {
        let now = Instant::now();
        let mut frames = ScrollFrames::default();
        frames.rendered(now);
        let next = now + Duration::from_millis(1);
        frames.scrolled(next);
        assert!(!frames.ready(next));
        frames.flush(next);
        assert!(frames.ready(next));
        frames.rendered(next);
        assert!(!frames.pending());
        assert_eq!(frames.timeout(next), IDLE_TIMEOUT);
    }
}
