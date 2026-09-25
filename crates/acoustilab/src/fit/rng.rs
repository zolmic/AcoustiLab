//! Seeded pseudo-random numbers for the virtual rig and multi-start fits.
//!
//! xoshiro256** (Blackman and Vigna, "Scrambled linear pseudorandom number
//! generators", ACM TOMS 47(4), 2021) seeded through SplitMix64, the
//! seeding the authors recommend. The integer stream is identical on every
//! platform; normal deviates use the Box–Muller transform, whose `ln`, `cos`
//! and `sin` may differ in the last bit between math libraries.

/// xoshiro256** generator.
#[derive(Debug, Clone)]
pub struct Rng {
    s: [u64; 4],
    spare: Option<f64>,
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        let mut st = seed;
        let s = [
            splitmix64(&mut st),
            splitmix64(&mut st),
            splitmix64(&mut st),
            splitmix64(&mut st),
        ];
        Rng { s, spare: None }
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

    /// Uniform on [0, 1) with 53 random bits.
    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Standard normal deviate (Box–Muller, both values used).
    pub fn normal(&mut self) -> f64 {
        if let Some(z) = self.spare.take() {
            return z;
        }
        let u1 = 1.0 - self.uniform(); // (0, 1]
        let u2 = self.uniform();
        let r = (-2.0 * u1.ln()).sqrt();
        let (s, c) = (2.0 * std::f64::consts::PI * u2).sin_cos();
        self.spare = Some(r * s);
        r * c
    }

    /// A random permutation of 0..n (Fisher–Yates).
    pub fn permutation(&mut self, n: usize) -> Vec<usize> {
        let mut p: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            let j = (self.next_u64() % (i as u64 + 1)) as usize;
            p.swap(i, j);
        }
        p
    }
}

/// Latin hypercube sample of `n` points in [0, 1)^dims: in every dimension
/// each of the n strata [k/n, (k+1)/n) holds exactly one point.
pub fn latin_hypercube(rng: &mut Rng, n: usize, dims: usize) -> Vec<Vec<f64>> {
    let mut pts = vec![vec![0.0; dims]; n];
    for d in 0..dims {
        let perm = rng.permutation(n);
        for (i, p) in pts.iter_mut().enumerate() {
            p[d] = (perm[i] as f64 + rng.uniform()) / n as f64;
        }
    }
    pts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_stream() {
        // SplitMix64 from 0 gives 0xE220A8397B1DCDAF first (Vigna's
        // reference implementation, splitmix64.c).
        let mut st = 0u64;
        assert_eq!(splitmix64(&mut st), 0xE220_A839_7B1D_CDAF);
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
    }

    #[test]
    fn moments() {
        let mut r = Rng::new(7);
        let n = 200_000;
        let (mut s1, mut s2, mut u1) = (0.0, 0.0, 0.0);
        for _ in 0..n {
            let z = r.normal();
            s1 += z;
            s2 += z * z;
            u1 += r.uniform();
        }
        let n = n as f64;
        // Standard errors: 1/sqrt(n) = 0.0022 for the mean, sqrt(2/n) for the variance.
        assert!((s1 / n).abs() < 0.01);
        assert!((s2 / n - 1.0).abs() < 0.015);
        assert!((u1 / n - 0.5).abs() < 0.003);
    }

    #[test]
    fn latin_hypercube_strata() {
        let mut r = Rng::new(3);
        let pts = latin_hypercube(&mut r, 8, 3);
        for d in 0..3 {
            let mut strata: Vec<usize> = pts.iter().map(|p| (p[d] * 8.0) as usize).collect();
            strata.sort();
            assert_eq!(strata, (0..8).collect::<Vec<_>>());
        }
    }
}
