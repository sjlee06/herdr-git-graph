use std::time::{Duration, Instant};

const SCROLL_FRAME_INTERVAL: Duration = Duration::from_millis(16);
const IDLE_TIMEOUT: Duration = Duration::from_millis(40);

/// Apply wheel events in order, but paint at most once per scroll frame.
/// The deadline is not extended by new input, so continuous scrolling still draws.
#[derive(Default)]
pub struct ScrollFrames {
    deadline: Option<Instant>,
}

impl ScrollFrames {
    pub fn scrolled(&mut self, now: Instant) {
        self.deadline.get_or_insert(now + SCROLL_FRAME_INTERVAL);
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

    pub fn rendered(&mut self) {
        self.deadline = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_burst_shares_a_frame_without_postponing_its_deadline() {
        let now = Instant::now();
        let mut frames = ScrollFrames::default();
        assert!(frames.ready(now));
        assert_eq!(frames.timeout(now), IDLE_TIMEOUT);
        frames.scrolled(now);
        for elapsed in 1..16 {
            let at = now + Duration::from_millis(elapsed);
            frames.scrolled(at);
            assert!(!frames.ready(at));
            assert_eq!(frames.timeout(at), Duration::from_millis(16 - elapsed));
        }
        assert!(frames.pending());
        assert!(frames.ready(now + SCROLL_FRAME_INTERVAL));
        assert_eq!(frames.timeout(now + SCROLL_FRAME_INTERVAL), Duration::ZERO);
        frames.rendered();
        assert!(!frames.pending());
        assert_eq!(frames.timeout(now), IDLE_TIMEOUT);
    }

    #[test]
    fn non_wheel_input_can_flush_before_the_next_frame() {
        let now = Instant::now();
        let mut frames = ScrollFrames::default();
        frames.scrolled(now);
        frames.flush(now);
        assert!(frames.ready(now));
        frames.rendered();
        frames.scrolled(now + Duration::from_millis(1));
        assert!(!frames.ready(now + SCROLL_FRAME_INTERVAL));
        assert!(frames.ready(now + SCROLL_FRAME_INTERVAL + Duration::from_millis(1)));
    }
}
