//! Seeded, platform-independent pseudo-random numbers for Monte Carlo and
//! Latin hypercube sampling.
//!
//! * [`SplitMix64`] (Steele, Lea and Flood, "Fast splittable pseudorandom
//!   number generators", OOPSLA 2014) expands a 64-bit seed into the state
//!   of the main generator, as its authors recommend.
//! * [`Xoshiro256`] is xoshiro256** (Blackman and Vigna, "Scrambled linear
//!   pseudorandom number generators", ACM TOMS 47(4), 2021; reference C code
//!   at prng.di.unimi.it).
//!
//! Both use only 64-bit integer arithmetic, so a seed gives the same stream
//! on every platform. Floating-point deviates are formed exactly (see
//! [`Xoshiro256::uniform`]); `tools/analysis/reference.py` reimplements the
//! stream independently for the tests.

/// SplitMix64: state += 0x9E3779B97F4A7C15, then two xor-shift-multiply
/// rounds.
#[derive(Debug, Clone)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        SplitMix64 { state: seed }
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// xoshiro256**.
#[derive(Debug, Clone)]
pub struct Xoshiro256 {
    s: [u64; 4],
}

impl Xoshiro256 {
    /// State from four SplitMix64 outputs of `seed`.
    pub fn seed_from(seed: u64) -> Self {
        let mut sm = SplitMix64::new(seed);
        Xoshiro256 {
            s: [sm.next_u64(), sm.next_u64(), sm.next_u64(), sm.next_u64()],
        }
    }

    /// Explicit state (must not be all zero).
    pub fn from_state(s: [u64; 4]) -> Self {
        Xoshiro256 { s }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform deviate in the open interval (0, 1): ((x >> 12) + ½)·2⁻⁵²,
    /// exact in binary, so never 0 or 1 (the normal quantile stays finite).
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 12) as f64 + 0.5) * (1.0 / 4_503_599_627_370_496.0)
    }

    /// Unbiased integer in [0, n) by Lemire's multiply-and-reject method
    /// ("Fast random integer generation in an interval", ACM TOMACS 29(1),
    /// 2019). `n` must be positive.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "below(0)");
        let mut m = self.next_u64() as u128 * n as u128;
        if (m as u64) < n {
            let t = n.wrapping_neg() % n;
            while (m as u64) < t {
                m = self.next_u64() as u128 * n as u128;
            }
        }
        (m >> 64) as u64
    }

    /// Fisher–Yates shuffle, drawing `below(i + 1)` for i from the last
    /// index down to 1.
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i as u64 + 1) as usize;
            v.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_streams() {
        // SplitMix64 from seed 0: first output 0xE220A8397B1DCDAF (the
        // value quoted with the reference implementation).
        assert_eq!(SplitMix64::new(0).next_u64(), 0xE220_A839_7B1D_CDAF);
        // xoshiro256** from state {1, 2, 3, 4}: rotl(2·5, 7)·9 = 11520, 0.
        let mut x = Xoshiro256::from_state([1, 2, 3, 4]);
        assert_eq!(x.next_u64(), 11_520);
        assert_eq!(x.next_u64(), 0);
    }

    #[test]
    fn uniform_is_open_and_below_is_in_range() {
        let mut x = Xoshiro256::seed_from(7);
        for _ in 0..10_000 {
            let u = x.uniform();
            assert!(u > 0.0 && u < 1.0);
            assert!(x.below(3) < 3);
        }
        let mut v: Vec<usize> = (0..50).collect();
        x.shuffle(&mut v);
        let mut s = v.clone();
        s.sort_unstable();
        assert_eq!(s, (0..50).collect::<Vec<_>>());
        assert_ne!(v, s);
    }
}
