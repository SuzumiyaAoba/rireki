//! 記録された時刻を、倍率付きの単調時計で再生する。

use std::time::Instant;

pub const SPEEDS: &[f64] = &[0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0];

pub struct Playback {
    start: f64,
    duration: f64,
    position: f64,
    speed: f64,
    anchor: Option<Instant>,
}

impl Playback {
    pub fn new(start: f64, end: f64, speed: f64) -> Self {
        let start = if start.is_finite() { start } else { 0.0 };
        let duration = end - start;
        Self {
            start,
            duration: if duration.is_finite() { duration.max(0.0) } else { 0.0 },
            position: 0.0,
            speed: valid_speed(speed),
            anchor: None,
        }
    }

    pub fn position(&self) -> f64 {
        self.position
    }
    pub fn duration(&self) -> f64 {
        self.duration
    }
    pub fn timestamp(&self) -> f64 {
        self.start + self.position
    }
    pub fn is_playing(&self) -> bool {
        self.anchor.is_some()
    }
    pub fn is_finished(&self) -> bool {
        self.position >= self.duration
    }

    pub fn play(&mut self, now: Instant) {
        if self.is_playing() {
            return;
        }
        if self.is_finished() {
            self.position = 0.0;
        }
        if self.duration > 0.0 {
            self.anchor = Some(now);
        }
    }

    pub fn tick(&mut self, now: Instant) {
        if let Some(anchor) = self.anchor {
            self.position =
                (self.position + now.saturating_duration_since(anchor).as_secs_f64() * self.speed).min(self.duration);
            self.anchor = (!self.is_finished()).then_some(now);
        }
    }

    pub fn pause(&mut self, now: Instant) {
        self.tick(now);
        self.anchor = None;
    }

    pub fn rewind(&mut self, now: Instant) {
        self.position = 0.0;
        if self.is_playing() {
            self.anchor = Some(now);
        }
    }

    pub fn set_speed(&mut self, speed: f64, now: Instant) {
        // 変更する直前までの経過時間には、それまでの倍率を使う。
        self.tick(now);
        self.speed = valid_speed(speed);
    }
}

fn valid_speed(speed: f64) -> f64 {
    if speed.is_finite() && speed > 0.0 { speed.clamp(SPEEDS[0], SPEEDS[SPEEDS.len() - 1]) } else { 1.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn real_time_preserves_gaps_and_final_event_duration() {
        let now = Instant::now();
        let mut replay = Playback::new(1_700_000_000.0, 1_700_000_030.0, 1.0);
        replay.play(now);
        replay.tick(now + Duration::from_secs(10));
        assert_eq!(replay.timestamp(), 1_700_000_010.0);
        assert!(replay.is_playing());
        replay.tick(now + Duration::from_secs(40));
        assert_eq!(replay.position(), 30.0);
        assert!(replay.is_finished());
        assert!(!replay.is_playing());
    }

    #[test]
    fn speed_changes_apply_only_to_future_time_and_pause_keeps_position() {
        let now = Instant::now();
        let at = |s| now + Duration::from_secs(s);
        let mut replay = Playback::new(100.0, 200.0, 1.0);
        replay.play(now);
        replay.set_speed(4.0, at(3));
        replay.pause(at(5));
        assert_eq!(replay.position(), 11.0);
        replay.tick(at(50));
        assert_eq!(replay.position(), 11.0);
        replay.set_speed(0.5, at(50));
        replay.play(at(50));
        replay.tick(at(54));
        assert_eq!(replay.position(), 13.0);
        replay.rewind(at(54));
        replay.tick(at(56));
        assert_eq!(replay.position(), 1.0);
    }

    #[test]
    fn completion_can_restart_and_zero_duration_needs_no_timer() {
        let now = Instant::now();
        let mut replay = Playback::new(10.0, 11.0, 64.0);
        replay.play(now);
        replay.tick(now + Duration::from_secs(1));
        assert!(replay.is_finished());
        replay.play(now + Duration::from_secs(2));
        assert_eq!(replay.position(), 0.0);
        assert!(replay.is_playing());
        for end in [10.0, 9.0, f64::NAN, f64::INFINITY] {
            let mut replay = Playback::new(10.0, end, f64::NAN);
            replay.play(now);
            assert!(replay.is_finished());
            assert!(!replay.is_playing());
            assert_eq!(replay.timestamp(), 10.0);
        }
    }
}
