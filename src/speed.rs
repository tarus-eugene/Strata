use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(250);
const WINDOW: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct DownloadSpeed {
    samples: VecDeque<(Instant, u64)>,
    rate: f64,
}

impl DownloadSpeed {
    pub fn update(&mut self, now: Instant, downloaded: u64, downloading: bool) {
        if !downloading
            || self.samples.back().is_some_and(|&(time, bytes)| {
                downloaded < bytes || now.duration_since(time) > WINDOW
            })
        {
            self.samples.clear();
            self.rate = 0.;
        }
        if !downloading {
            return;
        }
        if self
            .samples
            .back()
            .is_some_and(|&(time, _)| now.duration_since(time) < SAMPLE_INTERVAL)
        {
            return;
        }
        self.samples.push_back((now, downloaded));
        // Keep the sample just before the window so irregular refreshes still count all bytes.
        while self.samples.len() > 2 && now.duration_since(self.samples[1].0) >= WINDOW {
            self.samples.pop_front();
        }
        let &(time, bytes) = self.samples.front().unwrap();
        let elapsed = now.duration_since(time);
        if elapsed >= SAMPLE_INTERVAL {
            self.rate = (downloaded - bytes) as f64 / elapsed.as_secs_f64();
        }
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
        speed.update(start + Duration::from_millis(400), 400_000, true);
        assert_eq!(speed.rate(), 1_000_000.);
        speed.update(start + Duration::from_millis(401), 800_000, true);
        speed.update(start + Duration::from_millis(402), 800_000, true);
        assert_eq!(speed.rate(), 1_000_000.);
        speed.update(start + Duration::from_millis(800), 800_000, true);
        assert_eq!(speed.rate(), 1_000_000.);
    }

    #[test]
    fn bursty_progress_settles_to_the_real_average_and_stalls_reach_zero() {
        let start = Instant::now();
        let mut speed = DownloadSpeed::default();
        for tick in 0..=24 {
            speed.update(
                start + SAMPLE_INTERVAL * tick,
                u64::from(tick / 2) * 250_000,
                true,
            );
            if tick >= 8 {
                assert_eq!(speed.rate(), 500_000.);
            }
            assert!(speed.samples.len() <= 10);
        }
        for tick in 25..=32 {
            speed.update(start + SAMPLE_INTERVAL * tick, 3_000_000, true);
        }
        assert_eq!(speed.rate(), 0.);
    }

    #[test]
    fn pause_reset_and_long_gaps_do_not_reuse_stale_speed() {
        let start = Instant::now();
        let mut speed = DownloadSpeed::default();
        speed.update(start, 0, true);
        speed.update(start + Duration::from_secs(1), 1_000_000, true);
        speed.update(start + Duration::from_millis(1001), 1_000_000, false);
        assert_eq!(speed.rate(), 0.);
        speed.update(start + Duration::from_secs(20), 1_000_000, true);
        speed.update(start + Duration::from_secs(21), 2_000_000, true);
        assert_eq!(speed.rate(), 1_000_000.);
        speed.update(start + Duration::from_secs(21), 0, true);
        assert_eq!(speed.rate(), 0.);
        speed.update(start + Duration::from_secs(22), 1_000_000, true);
        assert_eq!(speed.rate(), 1_000_000.);
        speed.update(start + Duration::from_secs(30), 1_000_000, true);
        assert_eq!(speed.rate(), 0.);
    }
}
