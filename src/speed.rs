use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
const WINDOW: Duration = Duration::from_secs(6);
const DISPLAY_INTERVAL: Duration = Duration::from_secs(1);
const WARMUP: Duration = Duration::from_secs(2);
const STALL: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct DownloadSpeed {
    samples: VecDeque<(Instant, u64)>,
    rate: f64,
    last_progress: Option<Instant>,
    last_estimate: Option<Instant>,
}

impl DownloadSpeed {
    pub fn update(&mut self, now: Instant, downloaded: u64, downloading: bool) {
        if !downloading
            || self.samples.back().is_some_and(|&(time, bytes)| {
                downloaded < bytes || now.duration_since(time) > WINDOW
            })
        {
            *self = Self::default();
        }
        if !downloading {
            return;
        }
        if self
            .samples
            .back()
            .is_none_or(|&(_, bytes)| downloaded > bytes)
        {
            self.last_progress = Some(now);
        }
        if self
            .samples
            .back()
            .is_some_and(|&(time, _)| now.duration_since(time) < SAMPLE_INTERVAL)
        {
            return;
        }
        self.samples.push_back((now, downloaded));
        if self
            .last_progress
            .is_some_and(|time| now.duration_since(time) >= STALL)
        {
            self.samples.clear();
            self.samples.push_back((now, downloaded));
            self.rate = 0.;
            self.last_estimate = None;
            return;
        }
        // Keep the sample just before the window so irregular refreshes still count all bytes.
        while self.samples.len() > 2 && now.duration_since(self.samples[1].0) >= WINDOW {
            self.samples.pop_front();
        }
        let &(time, bytes) = self.samples.front().unwrap();
        let elapsed = now.duration_since(time);
        if elapsed < WARMUP
            || self
                .last_estimate
                .is_some_and(|time| now.duration_since(time) < DISPLAY_INTERVAL)
        {
            return;
        }
        let measured = (downloaded - bytes) as f64 / elapsed.as_secs_f64();
        self.rate = match self.last_estimate {
            Some(previous) => {
                self.rate
                    + (measured - self.rate)
                        * (1. - (-now.duration_since(previous).as_secs_f64() / 3.).exp())
            }
            None => measured,
        };
        self.last_estimate = Some(now);
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_refreshes_cannot_create_spikes_or_lose_progress() {
        let start = Instant::now();
        let mut speed = DownloadSpeed::default();
        speed.update(start, 0, true);
        speed.update(start + Duration::from_secs(2), 2_000_000, true);
        assert_eq!(speed.rate(), 1_000_000.);
        speed.update(start + Duration::from_millis(2001), 2_800_000, true);
        speed.update(start + Duration::from_millis(2002), 2_800_000, true);
        assert_eq!(speed.rate(), 1_000_000.);
        speed.update(start + Duration::from_secs(3), 3_000_000, true);
        assert_eq!(speed.rate(), 1_000_000.);
    }

    #[test]
    fn bursty_progress_settles_to_the_real_average_and_stalls_reach_zero() {
        let start = Instant::now();
        let mut speed = DownloadSpeed::default();
        for tick in 0..=48 {
            speed.update(
                start + SAMPLE_INTERVAL * tick,
                u64::from(tick / 4) * 500_000,
                true,
            );
            if tick >= 24 {
                assert!((speed.rate() - 500_000.).abs() < 50_000.);
            }
            assert!(speed.samples.len() <= 26);
        }
        for tick in 49..=60 {
            speed.update(start + SAMPLE_INTERVAL * tick, 6_000_000, true);
        }
        assert_eq!(speed.rate(), 0.);
    }

    #[test]
    fn pause_reset_and_long_gaps_do_not_reuse_stale_speed() {
        let start = Instant::now();
        let mut speed = DownloadSpeed::default();
        speed.update(start, 0, true);
        speed.update(start + Duration::from_secs(2), 2_000_000, true);
        assert_eq!(speed.rate(), 1_000_000.);
        speed.update(start + Duration::from_millis(2001), 2_000_000, false);
        assert_eq!(speed.rate(), 0.);
        speed.update(start + Duration::from_secs(20), 1_000_000, true);
        speed.update(start + Duration::from_secs(22), 3_000_000, true);
        assert_eq!(speed.rate(), 1_000_000.);
        speed.update(start + Duration::from_secs(22), 0, true);
        assert_eq!(speed.rate(), 0.);
        speed.update(start + Duration::from_secs(24), 2_000_000, true);
        assert_eq!(speed.rate(), 1_000_000.);
        speed.update(start + Duration::from_secs(40), 2_000_000, true);
        assert_eq!(speed.rate(), 0.);
    }

    #[test]
    fn short_bursts_do_not_replace_the_sustained_rate_and_changes_eventually_register() {
        let start = Instant::now();
        let mut speed = DownloadSpeed::default();
        let mut bytes = 0;
        for second in 0..=12 {
            bytes = second * 1_000_000;
            speed.update(start + Duration::from_secs(second), bytes, true);
        }
        speed.update(start + Duration::from_secs(13), bytes + 4_000_000, true);
        assert!(speed.rate() < 1_200_000.);
        for second in 14..=40 {
            speed.update(
                start + Duration::from_secs(second),
                bytes + (second - 12) * 4_000_000,
                true,
            );
        }
        assert!((speed.rate() - 4_000_000.).abs() < 10_000.);
    }
}
