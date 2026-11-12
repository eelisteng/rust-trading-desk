//! Minimal deterministic PRNG (SplitMix64). Zero dependencies so that every
//! generated series is byte-for-byte reproducible across platforms.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1_u64 << 53) as f64
    }

    pub fn range(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.next_f64()
    }

    pub fn int_range(&mut self, low: i64, high: i64) -> i64 {
        if high <= low {
            return low;
        }
        low + (self.next_u64() % (high - low) as u64) as i64
    }

    /// Standard normal via Box-Muller.
    pub fn normal(&mut self) -> f64 {
        let u1 = self
            .next_f64()
            .clamp(f64::MIN_POSITIVE, 1.0 - f64::EPSILON);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

/// FNV-1a, used to derive a stable per-symbol seed from its ticker.
pub fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let seq_a: Vec<u64> = (0..32).map(|_| a.next_u64()).collect();
        let seq_b: Vec<u64> = (0..32).map(|_| b.next_u64()).collect();
        assert_eq!(seq_a, seq_b);
        assert_ne!(seq_a[0], seq_a[1]);
    }

    #[test]
    fn different_seed_different_sequence() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn normal_is_finite_and_bounded_range() {
        let mut rng = Rng::new(7);
        for _ in 0..1000 {
            assert!(rng.normal().is_finite());
            let v = rng.range(2.0, 3.0);
            assert!((2.0..3.0).contains(&v));
        }
    }

    #[test]
    fn symbol_hash_is_stable() {
        assert_eq!(fnv1a("600519.SH"), fnv1a("600519.SH"));
        assert_ne!(fnv1a("600519.SH"), fnv1a("AAPL"));
    }
}
