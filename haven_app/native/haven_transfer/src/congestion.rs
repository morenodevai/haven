/// Delay-based congestion control, modeled after Aspera FASP.
///
/// Core principle: measure queuing delay, not loss. Loss triggers retransmission
/// but does NOT reduce the sending rate.
///
/// Algorithm:
///   queuing_delay = smoothed_rtt - min_rtt
///   rate_next = rate_current + K * (alpha - rate_current * queuing_delay)
///
/// Where:
///   min_rtt        = minimum RTT observed (sliding window, represents no-congestion baseline)
///   smoothed_rtt   = EWMA of recent RTT samples
///   alpha          = target queue accumulation rate (controls aggressiveness)
///   K              = gain constant (controls convergence speed)
///
/// The rate is expressed in bytes per second and enforced by the sender's
/// token bucket pacer.
///
/// Key behaviors:
///   - When queuing_delay ≈ 0: rate increases (link not full yet)
///   - When queuing_delay grows: rate stabilizes or decreases
///   - Packet loss: does NOT affect rate. Only triggers selective retransmission.
///   - Fairness: multiple flows converge because they all see the same queuing delay

use std::time::{Duration, Instant};
use std::collections::VecDeque;

/// Congestion controller configuration.
pub struct CongestionConfig {
    /// Initial sending rate in bytes/sec.
    pub initial_rate: u64,
    /// Minimum sending rate (floor). Never go below this.
    pub min_rate: u64,
    /// Maximum sending rate (ceiling). Caps at link speed.
    pub max_rate: u64,
    /// Gain constant K — how fast we react to delay changes.
    /// Higher = faster convergence but more oscillation.
    /// Typical: 0.3 - 1.0
    pub gain: f64,
    /// Target queue accumulation alpha (bytes).
    /// Higher = more aggressive (fills buffers more).
    /// Typical: 1000 - 50000
    pub alpha: f64,
    /// EWMA smoothing factor for RTT (0..1). Lower = smoother.
    pub rtt_smoothing: f64,
    /// Sliding window duration for min_rtt tracking.
    pub min_rtt_window: Duration,
    /// How often to update the rate (control interval).
    pub update_interval: Duration,
}

impl Default for CongestionConfig {
    fn default() -> Self {
        CongestionConfig {
            initial_rate: 10_000_000,           // 10 MB/s starting rate
            min_rate: 100_000,                   // 100 KB/s floor
            max_rate: 1_000_000_000,             // 1 GB/s ceiling
            gain: 0.5,
            alpha: 10_000.0,
            rtt_smoothing: 0.125,                // same as TCP's EWMA factor
            min_rtt_window: Duration::from_secs(10),
            update_interval: Duration::from_millis(50), // 20 Hz updates
        }
    }
}

/// An RTT sample with timestamp.
#[derive(Clone, Copy)]
struct RttSample {
    rtt: Duration,
    time: Instant,
}

/// The congestion controller.
pub struct CongestionController {
    config: CongestionConfig,
    /// Current sending rate in bytes/sec.
    current_rate: f64,
    /// Smoothed RTT (EWMA).
    smoothed_rtt: Option<Duration>,
    /// Minimum RTT observed within the sliding window.
    min_rtt: Option<Duration>,
    /// RTT samples for min_rtt sliding window.
    rtt_history: VecDeque<RttSample>,
    /// Last time we updated the rate.
    last_update: Instant,
    /// Total bytes acknowledged (for throughput tracking).
    bytes_acked: u64,
    /// Start time (for throughput tracking).
    start_time: Instant,
}

impl CongestionController {
    pub fn new(config: CongestionConfig) -> Self {
        let rate = config.initial_rate as f64;
        let now = Instant::now();
        CongestionController {
            config,
            current_rate: rate,
            smoothed_rtt: None,
            min_rtt: None,
            rtt_history: VecDeque::with_capacity(256),
            last_update: now,
            bytes_acked: 0,
            start_time: now,
        }
    }

    /// Report an RTT measurement (from NACK round-trip or explicit timestamp echo).
    /// This is the primary input to the controller.
    pub fn on_rtt_sample(&mut self, rtt: Duration) {
        let now = Instant::now();
        let sample = RttSample { rtt, time: now };

        // Update EWMA smoothed RTT
        self.smoothed_rtt = Some(match self.smoothed_rtt {
            None => rtt,
            Some(prev) => {
                let a = self.config.rtt_smoothing;
                let prev_us = prev.as_micros() as f64;
                let new_us = rtt.as_micros() as f64;
                let smoothed = (1.0 - a) * prev_us + a * new_us;
                Duration::from_micros(smoothed as u64)
            }
        });

        // Add to history and prune old samples
        self.rtt_history.push_back(sample);
        let window_start = now - self.config.min_rtt_window;
        while let Some(front) = self.rtt_history.front() {
            if front.time < window_start {
                self.rtt_history.pop_front();
            } else {
                break;
            }
        }

        // Recompute min_rtt from window
        self.min_rtt = self.rtt_history.iter().map(|s| s.rtt).min();
    }

    /// Report that bytes were acknowledged by the receiver.
    pub fn on_ack(&mut self, bytes: u64) {
        self.bytes_acked += bytes;
    }

    /// Called periodically by the sender. Returns the updated rate in bytes/sec.
    /// The sender should call this at `config.update_interval` frequency.
    pub fn update_rate(&mut self) -> u64 {
        let now = Instant::now();
        if now.duration_since(self.last_update) < self.config.update_interval {
            return self.current_rate as u64;
        }
        self.last_update = now;

        let (smoothed, min) = match (self.smoothed_rtt, self.min_rtt) {
            (Some(s), Some(m)) => (s, m),
            _ => {
                // Not enough data yet — hold at current rate
                return self.current_rate as u64;
            }
        };

        // Queuing delay = smoothed_rtt - min_rtt
        let queuing_delay_us = smoothed
            .as_micros()
            .saturating_sub(min.as_micros()) as f64;
        let queuing_delay_sec = queuing_delay_us / 1_000_000.0;

        // Rate update: rate_next = rate + K * (alpha - rate * queuing_delay)
        let delta = self.config.gain
            * (self.config.alpha - self.current_rate * queuing_delay_sec);
        self.current_rate += delta;

        // Clamp to [min_rate, max_rate]
        self.current_rate = self
            .current_rate
            .clamp(self.config.min_rate as f64, self.config.max_rate as f64);

        self.current_rate as u64
    }

    /// Get the current sending rate in bytes/sec.
    pub fn rate(&self) -> u64 {
        self.current_rate as u64
    }

    /// Get the inter-packet interval for the current rate, given a packet size.
    /// Returns the duration to wait between sending each packet.
    pub fn packet_interval(&self, packet_size: usize) -> Duration {
        if self.current_rate <= 0.0 {
            return Duration::from_millis(1);
        }
        let interval_sec = packet_size as f64 / self.current_rate;
        let interval_us = (interval_sec * 1_000_000.0) as u64;
        Duration::from_micros(interval_us.max(1))
    }

    /// Get the smoothed RTT, if available.
    pub fn rtt(&self) -> Option<Duration> {
        self.smoothed_rtt
    }

    /// Get the minimum RTT observed in the current window.
    pub fn min_rtt(&self) -> Option<Duration> {
        self.min_rtt
    }

    /// Get the estimated queuing delay.
    pub fn queuing_delay(&self) -> Option<Duration> {
        match (self.smoothed_rtt, self.min_rtt) {
            (Some(s), Some(m)) => {
                let delay = s.as_micros().saturating_sub(m.as_micros());
                Some(Duration::from_micros(delay as u64))
            }
            _ => None,
        }
    }

    /// Average throughput since start, in bytes/sec.
    pub fn throughput(&self) -> u64 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed <= 0.0 {
            return 0;
        }
        (self.bytes_acked as f64 / elapsed) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_initial_rate() {
        let cc = CongestionController::new(CongestionConfig {
            initial_rate: 50_000_000,
            ..Default::default()
        });
        assert_eq!(cc.rate(), 50_000_000);
    }

    #[test]
    fn rate_increases_when_no_queuing_delay() {
        let mut cc = CongestionController::new(CongestionConfig {
            initial_rate: 1_000_000,
            gain: 0.5,
            alpha: 10_000.0,
            update_interval: Duration::ZERO, // update every call
            ..Default::default()
        });

        // Simulate constant low RTT (no congestion)
        let base_rtt = Duration::from_millis(10);
        for _ in 0..20 {
            cc.on_rtt_sample(base_rtt);
        }

        let initial = cc.rate();
        cc.update_rate();
        let after = cc.rate();

        // With zero queuing delay, delta = K * alpha = 0.5 * 10000 = 5000
        // Rate should increase
        assert!(after > initial, "rate should increase: {} vs {}", after, initial);
    }

    #[test]
    fn rate_decreases_under_congestion() {
        let mut cc = CongestionController::new(CongestionConfig {
            initial_rate: 100_000_000, // 100 MB/s
            gain: 0.5,
            alpha: 10_000.0,
            rtt_smoothing: 1.0, // no smoothing for test clarity
            update_interval: Duration::ZERO,
            ..Default::default()
        });

        // First establish a low baseline
        let base_rtt = Duration::from_millis(5);
        cc.on_rtt_sample(base_rtt);

        // Now simulate high RTT (congestion — large queuing delay)
        let congested_rtt = Duration::from_millis(50);
        cc.on_rtt_sample(congested_rtt);

        let before = cc.rate();
        cc.update_rate();
        let after = cc.rate();

        // queuing_delay = 50ms - 5ms = 45ms = 0.045s
        // delta = 0.5 * (10000 - 100_000_000 * 0.045) = 0.5 * (10000 - 4500000) = very negative
        assert!(after < before, "rate should decrease: {} vs {}", after, before);
    }

    #[test]
    fn rate_clamped_to_bounds() {
        let mut cc = CongestionController::new(CongestionConfig {
            initial_rate: 500,
            min_rate: 1000,
            max_rate: 2000,
            update_interval: Duration::ZERO,
            ..Default::default()
        });

        // Rate should be clamped to min on first update
        cc.on_rtt_sample(Duration::from_millis(100));
        cc.on_rtt_sample(Duration::from_millis(100));
        cc.update_rate();
        assert!(cc.rate() >= 1000);
    }

    #[test]
    fn packet_interval_reasonable() {
        let cc = CongestionController::new(CongestionConfig {
            initial_rate: 100_000_000, // 100 MB/s
            ..Default::default()
        });
        let interval = cc.packet_interval(1400);
        // At 100 MB/s with 1400-byte packets: 1400/100M = 14 microseconds
        assert!(interval.as_micros() > 0 && interval.as_micros() < 100);
    }
}
