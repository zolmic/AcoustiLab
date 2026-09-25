//! Rigid-wall acoustic eigenmodes of rectangular boxes and circular
//! cylinders, and the modal-sum machinery behind the L1 `modal_cavity`
//! element (spec Section 9, "L1: closed-form modal expansion"; errata E25
//! and E27).
//!
//! # Coordinates
//!
//! The origin is the centre of the driver face and z points into the cavity
//! towards the far face, as for the depth axis of the `cavity` element. A box
//! spans x ∈ [−lx/2, lx/2], y ∈ [−ly/2, ly/2], z ∈ [0, lz]; a cylinder has its
//! axis on z, radius a and depth d, with azimuth θ measured from +x towards +y.
//!
//! # Modes
//!
//! Rigid (Neumann) walls, so ∂φ/∂n = 0 on every face.
//!
//! * Box: φ = N·cos(nx·π·X/lx)·cos(ny·π·Y/ly)·cos(nz·π·z/lz) with corner
//!   coordinates X = x + lx/2, Y = y + ly/2, and k² = Σ (n_i·π/l_i)².
//! * Cylinder: φ = N·J_m(k_r·r)·{cos mθ | sin mθ}·cos(l·π·z/d) with
//!   k_r = j'_{m,q}/a, where j'_{m,q} is the (q+1)-th root of J'_m counting the
//!   root at 0 for m = 0, so q is the number of nodal circles. k² = k_r² +
//!   (l·π/d)². Modes with m > 0 come in degenerate cosine/sine pairs; both are
//!   listed.
//!
//! N normalises every mode to unit mean square over the volume:
//! N² = ε_x·ε_y·ε_z for the box and N² = ε_m·ε_l/⟨J_m²⟩ for the cylinder,
//! where ε is 1 for a zero index and 2 otherwise and
//! ⟨J_m²⟩ = (1 − m²/j'²)·J_m(j')² is the disk mean of J_m² at a root of J'_m
//! (from Lommel's integral ∫₀ᵃ J_m(kr)² r dr = (a²/2)(1 − m²/(ka)²)·J_m(ka)²
//! when J'_m(ka) = 0; checked by quadrature in the unit tests).
//!
//! # Wall integrals and patch averages
//!
//! [`Mode::wall_integrals`] returns ∮φ² dS and ∮|∇_t φ|² dS over all walls
//! (∇_t the gradient tangential to each wall), the two integrals the
//! Morse–Ingard boundary-layer perturbation needs. [`Mode::patch_average`]
//! returns the mean of φ over a port footprint. For a disk footprint on a
//! flat face it uses the mean-value theorem of the 2-D Helmholtz equation:
//! the restriction of a mode to a face solves ∇_t²u + k_t²u = 0, and the mean
//! of any such u over a disk of radius b is u(centre)·2J₁(k_t·b)/(k_t·b)
//! (the mean-value theorem of the Helmholtz equation, a consequence of Graf's
//! addition theorem; the unit tests check it by quadrature). The
//! same holds on the unrolled side wall of a cylinder, where footprints are
//! defined in (arc length, z) coordinates. Rectangles give products of sincs,
//! except on a cylinder end, where the mean is taken from the plane-wave
//! (Jacobi–Anger) representation of J_m·e^{imθ} by a periodic trapezoidal
//! rule.
//!
//! # Quasi-static sums of all modes
//!
//! [`static_sums`] evaluates S1 = Σ_{n≠0} φ̄_i·φ̄_j/k_n² and
//! S2 = Σ_{n≠0} φ̄_i·φ̄_j/k_n⁴ over *every* mode, for pairs of footprints
//! whose walls share a normal direction (box faces normal to the same axis,
//! cylinder end–end or side–side). The sum along the wall normal is done in
//! closed form (e.g. Σ_l ε_l/(κ² + (lπ/L)²) = (L/κ)·coth(κL)); the sum over
//! the transverse wavenumbers κ is done term by term up to a cutoff and the
//! remainder is replaced by its continuum (half-space) limit. The modal
//! cavity subtracts the retained modes' share of these sums to obtain the
//! quasi-static contribution of the truncated modes.

use crate::C64;
use std::f64::consts::PI;
use std::sync::OnceLock;

// ----- Special functions ---------------------------------------------------

/// J_0(x) … J_n(x) for real x ≥ 0 by Miller's backward recurrence
/// J_{k−1} = (2k/x)·J_k − J_{k+1}, started well above max(n, x) and
/// normalised with the identity 1 = J_0 + 2·Σ_{k≥1} J_{2k} (Abramowitz &
/// Stegun, §9.1 and §9.12).
pub fn bessel_j_all(n: usize, x: f64) -> Vec<f64> {
    assert!(
        x >= 0.0 && x.is_finite(),
        "bessel_j_all needs finite x >= 0"
    );
    let mut out = vec![0.0; n + 1];
    if x == 0.0 {
        out[0] = 1.0;
        return out;
    }
    let big = (n as f64).max(x);
    let mut start = (big + 20.0 + (40.0 * big).sqrt()).ceil() as usize;
    start += start % 2;
    let mut jk1 = 0.0; // J_{k+1}
    let mut jk = 1e-30; // J_k at k = start, arbitrary scale
    let mut sum = 2.0 * jk; // start is even
    for k in (1..=start).rev() {
        let jm1 = (2.0 * k as f64 / x) * jk - jk1;
        jk1 = jk;
        jk = jm1;
        let i = k - 1;
        if i <= n {
            out[i] = jk;
        }
        if i > 0 && i % 2 == 0 {
            sum += 2.0 * jk;
        }
        if jk.abs() > 1e250 {
            jk *= 1e-250;
            jk1 *= 1e-250;
            sum *= 1e-250;
            for v in out.iter_mut().skip(i) {
                *v *= 1e-250;
            }
        }
    }
    let norm = jk + sum;
    for v in &mut out {
        *v /= norm;
    }
    out
}

/// J_n(x) for real x ≥ 0.
pub fn bessel_jn(n: usize, x: f64) -> f64 {
    bessel_j_all(n, x)[n]
}

/// Hankel's asymptotic expansion of J_ν(x) for large x (DLMF 10.17.3),
/// summed while the terms decrease. Used for x ≥ 25, where the smallest term
/// is below 1e-20.
fn hankel_j(nu: u32, x: f64) -> f64 {
    let mu = 4.0 * (nu as f64).powi(2);
    let (mut p, mut q) = (1.0, 0.0);
    let mut term = 1.0;
    let mut prev = f64::INFINITY;
    for k in 1..80u32 {
        let m = (2 * k - 1) as f64;
        term *= (mu - m * m) / (k as f64 * 8.0 * x);
        let t = term.abs();
        if t > prev {
            break;
        }
        match k % 4 {
            1 => q += term,
            2 => p -= term,
            3 => q -= term,
            _ => p += term,
        }
        if t < 1e-18 {
            break;
        }
        prev = t;
    }
    let w = x - (0.5 * nu as f64 + 0.25) * PI;
    (2.0 / (PI * x)).sqrt() * (p * w.cos() - q * w.sin())
}

/// J₁(x) for real x: Miller recurrence below 25, Hankel asymptotics above.
pub fn bessel_j1(x: f64) -> f64 {
    let ax = x.abs();
    let v = if ax < 25.0 {
        bessel_j_all(1, ax)[1]
    } else {
        hankel_j(1, ax)
    };
    if x < 0.0 {
        -v
    } else {
        v
    }
}

/// 2·J₁(x)/x, the mean of a unit plane wave over a disk (1 at x = 0).
pub fn jinc(x: f64) -> f64 {
    if x.abs() < 1e-3 {
        let x2 = x * x;
        1.0 - x2 / 8.0 + x2 * x2 / 192.0
    } else {
        2.0 * bessel_j1(x) / x
    }
}

/// sin(x)/x (1 at x = 0).
pub fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-4 {
        1.0 - x * x / 6.0
    } else {
        x.sin() / x
    }
}

/// I_{m+1}(x)/I_m(x) for real x > 0, from the continued fraction of the
/// recurrence I_{m−1} − I_{m+1} = (2m/x)·I_m (modified Lentz algorithm).
pub fn bessel_i_ratio_real(m: usize, x: f64) -> f64 {
    assert!(x > 0.0);
    let tiny = 1e-300;
    let mut f = tiny;
    let mut c = f;
    let mut d = 0.0;
    for k in 1..10_000_000usize {
        let b = 2.0 * (m + k) as f64 / x;
        d += b;
        if d == 0.0 {
            d = tiny;
        }
        d = 1.0 / d;
        c = b + 1.0 / c;
        if c == 0.0 {
            c = tiny;
        }
        let delta = c * d;
        f *= delta;
        if (delta - 1.0).abs() < 1e-16 {
            break;
        }
    }
    f
}

/// J'_m from a table of J_0 … J_{m+1}.
fn jp_from(j: &[f64], m: usize) -> f64 {
    if m == 0 {
        -j[1]
    } else {
        0.5 * (j[m - 1] - j[m + 1])
    }
}

/// J'_m(x) and J''_m(x) (the latter from Bessel's equation).
fn jp_jpp(m: usize, x: f64) -> (f64, f64) {
    let j = bessel_j_all(m + 1, x);
    let d = jp_from(&j, m);
    let mf = m as f64;
    (d, -d / x - (1.0 - mf * mf / (x * x)) * j[m])
}

/// Root of J'_m bracketed by [a, b] (safeguarded Newton).
fn refine_jp_root(m: usize, a: f64, b: f64) -> f64 {
    let (mut a, mut b) = (a.max(1e-9), b);
    let mut fa = jp_jpp(m, a).0;
    let mut x = 0.5 * (a + b);
    for _ in 0..200 {
        let (f, fp) = jp_jpp(m, x);
        if f == 0.0 {
            return x;
        }
        if (f > 0.0) == (fa > 0.0) {
            a = x;
            fa = f;
        } else {
            b = x;
        }
        let mut xn = x - f / fp;
        if !(xn > a && xn < b) {
            xn = 0.5 * (a + b);
        }
        if (xn - x).abs() <= 1e-15 * x {
            return xn;
        }
        x = xn;
    }
    x
}

/// Positive roots of J'_m below `x_max` for every order m that has one; the
/// result is indexed by m. For m = 0 the root at x = 0 is not included.
///
/// Every root of J'_m (m ≥ 1) exceeds m (DLMF §10.21(i)), so orders up to
/// x_max suffice. Roots are bracketed on a grid of step 0.2 (consecutive
/// roots are more than 1.8 apart) and refined by Newton's method.
pub fn bessel_jp_zeros_all(x_max: f64) -> Vec<Vec<f64>> {
    if x_max <= 0.0 {
        return vec![Vec::new()];
    }
    let m_max = x_max.floor() as usize;
    let mut zeros = vec![Vec::new(); m_max + 1];
    // Sign of J'_m just above 0: negative for m = 0, positive otherwise.
    let mut sign: Vec<bool> = (0..=m_max).map(|m| m > 0).collect();
    let h = 0.2;
    let steps = (x_max / h).ceil() as usize;
    let mut prev_x = 0.0;
    for s in 1..=steps {
        let x = (s as f64 * h).min(x_max);
        let top = m_max.min(x.floor() as usize + 2);
        let j = bessel_j_all(top + 1, x);
        for (m, sg) in sign.iter_mut().enumerate().take(top + 1) {
            let d = jp_from(&j, m);
            if d == 0.0 {
                continue;
            }
            let pos = d > 0.0;
            if pos != *sg {
                let root = refine_jp_root(m, prev_x, x);
                if root < x_max {
                    zeros[m].push(root);
                }
                *sg = pos;
            }
        }
        prev_x = x;
    }
    zeros
}

/// Positive roots of J'_m below `x_max` (the root 0 of J'_0 excluded).
pub fn bessel_jp_zeros(m: usize, x_max: f64) -> Vec<f64> {
    bessel_jp_zeros_all(x_max)
        .get(m)
        .cloned()
        .unwrap_or_default()
}

/// 16-point Gauss–Legendre nodes and weights on [−1, 1].
fn gauss16() -> &'static ([f64; 16], [f64; 16]) {
    static GL: OnceLock<([f64; 16], [f64; 16])> = OnceLock::new();
    GL.get_or_init(|| {
        let n = 16usize;
        let mut x = [0.0; 16];
        let mut w = [0.0; 16];
        for i in 0..n / 2 {
            let mut t = (PI * (i as f64 + 0.75) / (n as f64 + 0.5)).cos();
            let mut dp = 0.0;
            for _ in 0..100 {
                let (mut p0, mut p1) = (1.0, t);
                for k in 2..=n {
                    let p2 = ((2 * k - 1) as f64 * t * p1 - (k - 1) as f64 * p0) / k as f64;
                    p0 = p1;
                    p1 = p2;
                }
                dp = n as f64 * (t * p1 - p0) / (t * t - 1.0);
                let dt = p1 / dp;
                t -= dt;
                if dt.abs() < 1e-16 {
                    break;
                }
            }
            x[i] = -t;
            x[n - 1 - i] = t;
            let wi = 2.0 / ((1.0 - t * t) * dp * dp);
            w[i] = wi;
            w[n - 1 - i] = wi;
        }
        (x, w)
    })
}

/// ∫_a^b f by composite 16-point Gauss–Legendre over `panels` panels.
fn integrate(a: f64, b: f64, panels: usize, mut f: impl FnMut(f64) -> f64) -> f64 {
    let (x, w) = gauss16();
    let panels = panels.max(1);
    let h = (b - a) / panels as f64;
    let mut s = 0.0;
    for p in 0..panels {
        let mid = a + (p as f64 + 0.5) * h;
        for (xi, wi) in x.iter().zip(w) {
            s += wi * f(mid + 0.5 * h * xi);
        }
    }
    s * 0.5 * h
}

// ----- Shapes and modes ------------------------------------------------------

/// Enclosure shape (lengths in metres).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// Rectangular box; `lz` is the depth from the driver face.
    Box { lx: f64, ly: f64, lz: f64 },
    /// Circular cylinder with its axis along the depth.
    Cylinder { radius: f64, depth: f64 },
}

impl Shape {
    pub fn volume(&self) -> f64 {
        match *self {
            Shape::Box { lx, ly, lz } => lx * ly * lz,
            Shape::Cylinder { radius, depth } => PI * radius * radius * depth,
        }
    }

    /// Total wall area.
    pub fn wall_area(&self) -> f64 {
        match *self {
            Shape::Box { lx, ly, lz } => 2.0 * (lx * ly + ly * lz + lx * lz),
            Shape::Cylinder { radius, depth } => {
                2.0 * PI * radius * radius + 2.0 * PI * radius * depth
            }
        }
    }

    /// Largest dimension (default length of the lumped-validity criterion).
    pub fn largest_dimension(&self) -> f64 {
        match *self {
            Shape::Box { lx, ly, lz } => lx.max(ly).max(lz),
            Shape::Cylinder { radius, depth } => (2.0 * radius).max(depth),
        }
    }

    /// All modes with k < `k_max`, sorted by k (ties by index). Includes the
    /// uniform mode k = 0.
    pub fn modes_below(&self, k_max: f64) -> Vec<Mode> {
        let mut v = match *self {
            Shape::Box { lx, ly, lz } => box_modes(lx, ly, lz, k_max),
            Shape::Cylinder { radius, depth } => cylinder_modes(radius, depth, k_max),
        };
        v.sort_by(|a, b| a.k.total_cmp(&b.k).then(a.index.cmp(&b.index)));
        v
    }
}

/// Mode indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModeIndex {
    /// Half-wavelength counts along x, y, z.
    Box { n: [u32; 3] },
    /// Azimuthal order m, nodal circles q, axial order l, and whether the
    /// azimuthal factor is sin mθ (only for m > 0).
    Cylinder { m: u32, q: u32, l: u32, sine: bool },
}

/// One rigid-wall eigenmode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mode {
    pub index: ModeIndex,
    /// Eigenwavenumber, rad/m.
    pub k: f64,
    /// Normalisation N (unit mean square over the volume).
    pub amplitude: f64,
    /// Radial wavenumber j'_{m,q}/a of a cylinder mode (0 for a box).
    pub kr: f64,
    /// J_m(k_r·a) for a cylinder mode (1 for a box or the uniform mode).
    pub jm_wall: f64,
}

fn eps(n: u32) -> f64 {
    if n == 0 {
        1.0
    } else {
        2.0
    }
}

fn box_modes(lx: f64, ly: f64, lz: f64, k_max: f64) -> Vec<Mode> {
    let kk = [PI / lx, PI / ly, PI / lz];
    let k2max = k_max * k_max;
    let mut v = Vec::new();
    let mut nx = 0u32;
    loop {
        let ax = (nx as f64 * kk[0]).powi(2);
        if ax >= k2max {
            break;
        }
        let mut ny = 0u32;
        loop {
            let axy = ax + (ny as f64 * kk[1]).powi(2);
            if axy >= k2max {
                break;
            }
            let mut nz = 0u32;
            loop {
                let a = axy + (nz as f64 * kk[2]).powi(2);
                if a >= k2max {
                    break;
                }
                v.push(Mode {
                    index: ModeIndex::Box { n: [nx, ny, nz] },
                    k: a.sqrt(),
                    amplitude: (eps(nx) * eps(ny) * eps(nz)).sqrt(),
                    kr: 0.0,
                    jm_wall: 1.0,
                });
                nz += 1;
            }
            ny += 1;
        }
        nx += 1;
    }
    v
}

/// Disk mean ⟨J_m²⟩ of a transverse cylinder mode with root `jp` of J'_m,
/// and J_m(jp).
fn disk_mean_jm2(m: usize, jp: f64) -> (f64, f64) {
    if jp == 0.0 {
        return (1.0, 1.0);
    }
    let jm = bessel_jn(m, jp);
    let mf = m as f64;
    ((1.0 - mf * mf / (jp * jp)) * jm * jm, jm)
}

fn cylinder_modes(a: f64, d: f64, k_max: f64) -> Vec<Mode> {
    let zeros = bessel_jp_zeros_all(k_max * a);
    let mut v = Vec::new();
    for (m, roots) in zeros.iter().enumerate() {
        let mut list = Vec::with_capacity(roots.len() + 1);
        if m == 0 {
            list.push(0.0);
        }
        list.extend_from_slice(roots);
        for (q, &jp) in list.iter().enumerate() {
            let kr = jp / a;
            if kr >= k_max {
                break;
            }
            let (mean, jm) = disk_mean_jm2(m, jp);
            let mut l = 0u32;
            loop {
                let kz = l as f64 * PI / d;
                let k = kr.hypot(kz);
                if k >= k_max {
                    break;
                }
                let amp = (eps(m as u32) * eps(l) / mean).sqrt();
                for sine in [false, true] {
                    if sine && m == 0 {
                        continue;
                    }
                    v.push(Mode {
                        index: ModeIndex::Cylinder {
                            m: m as u32,
                            q: q as u32,
                            l,
                            sine,
                        },
                        k,
                        amplitude: amp,
                        kr,
                        jm_wall: jm,
                    });
                }
                l += 1;
            }
        }
    }
    v
}

/// Distinct eigenfrequencies (Hz) of a sorted mode list, merging values
/// within `rel_tol` (degenerate modes), including 0 for the uniform mode.
pub fn distinct_frequencies(modes: &[Mode], c: f64, rel_tol: f64) -> Vec<f64> {
    let mut out: Vec<f64> = Vec::new();
    for m in modes {
        let f = m.frequency(c);
        match out.last() {
            Some(&last) if (f - last).abs() <= rel_tol * f.max(1e-300) => {}
            _ => out.push(f),
        }
    }
    out
}

impl Mode {
    pub fn frequency(&self, c: f64) -> f64 {
        self.k * c / (2.0 * PI)
    }

    fn trig(&self, m: u32, theta: f64) -> f64 {
        match self.index {
            ModeIndex::Cylinder { sine: true, .. } => (m as f64 * theta).sin(),
            _ => (m as f64 * theta).cos(),
        }
    }

    /// Mode shape φ at a point of the driver-face-centred frame.
    pub fn value(&self, shape: &Shape, p: [f64; 3]) -> f64 {
        match (*shape, self.index) {
            (Shape::Box { lx, ly, lz }, ModeIndex::Box { n }) => {
                let c = [p[0] + 0.5 * lx, p[1] + 0.5 * ly, p[2]];
                let l = [lx, ly, lz];
                self.amplitude
                    * (0..3)
                        .map(|i| (n[i] as f64 * PI * c[i] / l[i]).cos())
                        .product::<f64>()
            }
            (Shape::Cylinder { depth, .. }, ModeIndex::Cylinder { m, l, .. }) => {
                let r = p[0].hypot(p[1]);
                let th = p[1].atan2(p[0]);
                self.amplitude
                    * bessel_jn(m as usize, self.kr * r)
                    * self.trig(m, th)
                    * (l as f64 * PI * p[2] / depth).cos()
            }
            _ => panic!("mode does not belong to this shape"),
        }
    }

    /// (∮φ² dS, ∮|∇_t φ|² dS) over all walls, ∇_t being the gradient
    /// tangential to each wall. For separable modes each face contributes
    /// its tangential wavenumber squared times its ∮φ².
    pub fn wall_integrals(&self, shape: &Shape) -> (f64, f64) {
        match (*shape, self.index) {
            (Shape::Box { lx, ly, lz }, ModeIndex::Box { n }) => {
                let (a, b, c) = (
                    n[0] as f64 * PI / lx,
                    n[1] as f64 * PI / ly,
                    n[2] as f64 * PI / lz,
                );
                let (ex, ey, ez) = (eps(n[0]), eps(n[1]), eps(n[2]));
                // Face pairs normal to z, x and y: ∮φ² = 2·ε_normal·(face area).
                let fz = 2.0 * ez * lx * ly;
                let fx = 2.0 * ex * ly * lz;
                let fy = 2.0 * ey * lx * lz;
                (
                    fz + fx + fy,
                    fz * (a * a + b * b) + fx * (b * b + c * c) + fy * (a * a + c * c),
                )
            }
            (Shape::Cylinder { radius, depth }, ModeIndex::Cylinder { m, l, .. }) => {
                let kz = l as f64 * PI / depth;
                let caps = 2.0 * PI * radius * radius * eps(l);
                let jp = self.kr * radius;
                let side = if jp == 0.0 {
                    2.0 * PI * radius * depth
                } else {
                    let mf = m as f64;
                    2.0 * PI * radius * depth / (1.0 - mf * mf / (jp * jp))
                };
                let mt = m as f64 / radius;
                (
                    caps + side,
                    caps * self.kr * self.kr + side * (mt * mt + kz * kz),
                )
            }
            _ => panic!("mode does not belong to this shape"),
        }
    }

    /// Mean of φ over a footprint.
    pub fn patch_average(&self, shape: &Shape, patch: &Patch) -> f64 {
        match (*shape, self.index, patch.face) {
            (Shape::Box { lx, ly, lz }, ModeIndex::Box { n }, Face::Box { axis, far }) => {
                let dims = [lx, ly, lz];
                let normal = if far && n[axis] % 2 == 1 { -1.0 } else { 1.0 };
                let (ua, va) = transverse_axes(axis);
                let uc = patch.u + corner_offset(dims, ua);
                let vc = patch.v + corner_offset(dims, va);
                let ku = n[ua] as f64 * PI / dims[ua];
                let kv = n[va] as f64 * PI / dims[va];
                let t = match patch.footprint {
                    Footprint::Disk { radius } => {
                        (ku * uc).cos() * (kv * vc).cos() * jinc(ku.hypot(kv) * radius)
                    }
                    Footprint::Rect { du, dv } => {
                        (ku * uc).cos()
                            * sinc(0.5 * ku * du)
                            * (kv * vc).cos()
                            * sinc(0.5 * kv * dv)
                    }
                };
                self.amplitude * normal * t
            }
            (Shape::Cylinder { .. }, ModeIndex::Cylinder { m, l, sine, .. }, Face::End { far }) => {
                let normal = if far && l % 2 == 1 { -1.0 } else { 1.0 };
                let (c, s) = end_transverse_mean(m as usize, self.kr, patch);
                self.amplitude * normal * if sine { s } else { c }
            }
            (Shape::Cylinder { radius, depth }, ModeIndex::Cylinder { m, l, .. }, Face::Side) => {
                let kz = l as f64 * PI / depth;
                let th = patch.u / radius;
                let mt = m as f64 / radius;
                let t = match patch.footprint {
                    Footprint::Disk { radius: b } => (kz * patch.v).cos() * jinc(mt.hypot(kz) * b),
                    Footprint::Rect { du, dv } => {
                        sinc(0.5 * mt * du) * (kz * patch.v).cos() * sinc(0.5 * kz * dv)
                    }
                };
                self.amplitude * self.jm_wall * self.trig(m, th) * t
            }
            _ => panic!("patch face does not belong to this shape"),
        }
    }
}

/// Mean over a cylinder-end footprint of J_m(k_r·r)·(cos mθ, sin mθ).
fn end_transverse_mean(m: usize, kr: f64, patch: &Patch) -> (f64, f64) {
    let (x0, y0) = (patch.u, patch.v);
    match patch.footprint {
        Footprint::Disk { radius } => {
            let r0 = x0.hypot(y0);
            let th = y0.atan2(x0);
            let j = if kr == 0.0 {
                if m == 0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                bessel_jn(m, kr * r0)
            };
            let g = j * jinc(kr * radius);
            let mt = m as f64 * th;
            (g * mt.cos(), g * mt.sin())
        }
        Footprint::Rect { du, dv } => disk_mode_rect_mean(m, kr, x0, y0, du, dv),
    }
}

/// Mean of J_m(k·r)·e^{imθ} over the axis-aligned rectangle centred at
/// (x0, y0) with sides w (along x) and h (along y), returned as (real,
/// imaginary) = (cos part, sin part). From J_m(kr)e^{imθ} =
/// (2π·i^m)⁻¹ ∫₀^{2π} e^{ikr·cos(α−θ)} e^{imα} dα, the rectangle mean of each
/// plane wave being e^{ik·c}·sinc(k_x·w/2)·sinc(k_y·h/2); the smooth periodic
/// integrand is summed with a trapezoidal rule of more points than its
/// bandwidth k·(|c| + (w + h)/2) + m.
pub fn disk_mode_rect_mean(m: usize, k: f64, x0: f64, y0: f64, w: f64, h: f64) -> (f64, f64) {
    if k == 0.0 {
        return (if m == 0 { 1.0 } else { 0.0 }, 0.0);
    }
    let band = k * (x0.hypot(y0) + 0.5 * (w + h)) + m as f64;
    let n = 2 * band.ceil() as usize + 48;
    let mut acc = C64::new(0.0, 0.0);
    for j in 0..n {
        let al = 2.0 * PI * j as f64 / n as f64;
        let (s, c) = al.sin_cos();
        let amp = sinc(0.5 * k * w * c) * sinc(0.5 * k * h * s);
        acc += C64::from_polar(amp, k * (x0 * c + y0 * s) + m as f64 * al);
    }
    acc /= n as f64;
    // Multiply by i^{−m}.
    let v = match m % 4 {
        0 => acc,
        1 => C64::new(acc.im, -acc.re),
        2 => -acc,
        _ => C64::new(-acc.im, acc.re),
    };
    (v.re, v.im)
}

// ----- Faces, footprints, patches -------------------------------------------

/// A wall of the enclosure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Box face normal to `axis` (0 = x, 1 = y, 2 = z) at the low (`far =
    /// false`: x = −lx/2, y = −ly/2, z = 0) or high coordinate.
    Box { axis: usize, far: bool },
    /// Cylinder end at z = 0 (`far = false`) or z = depth.
    End { far: bool },
    /// Cylinder side wall r = a.
    Side,
}

/// Shape of a port footprint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Footprint {
    Disk {
        radius: f64,
    },
    /// Rectangle with extents along the face's first (u) and second (v)
    /// coordinates.
    Rect {
        du: f64,
        dv: f64,
    },
}

/// A port footprint on a wall. Face coordinates (u, v): box faces normal to
/// x use (y, z), normal to y (x, z), normal to z (x, y); cylinder ends (x, y);
/// the cylinder side wall (arc length a·θ, z).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Patch {
    pub face: Face,
    pub u: f64,
    pub v: f64,
    pub footprint: Footprint,
}

/// The two in-plane axes of a box face normal to `axis`.
pub fn transverse_axes(axis: usize) -> (usize, usize) {
    match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}

/// Offset from the centred frame to corner coordinates along an axis.
fn corner_offset(dims: [f64; 3], axis: usize) -> f64 {
    if axis == 2 {
        0.0
    } else {
        0.5 * dims[axis]
    }
}

impl Footprint {
    pub fn area(&self) -> f64 {
        match *self {
            Footprint::Disk { radius } => PI * radius * radius,
            Footprint::Rect { du, dv } => du * dv,
        }
    }

    /// Half extents along u and v.
    fn half(&self) -> (f64, f64) {
        match *self {
            Footprint::Disk { radius } => (radius, radius),
            Footprint::Rect { du, dv } => (0.5 * du, 0.5 * dv),
        }
    }
}

impl Patch {
    /// Centre of the footprint in the driver-face-centred frame.
    pub fn centre(&self, shape: &Shape) -> [f64; 3] {
        match (*shape, self.face) {
            (Shape::Box { lx, ly, lz }, Face::Box { axis, far }) => {
                let dims = [lx, ly, lz];
                let (ua, va) = transverse_axes(axis);
                let mut p = [0.0; 3];
                p[ua] = self.u;
                p[va] = self.v;
                p[axis] = if axis == 2 {
                    if far {
                        lz
                    } else {
                        0.0
                    }
                } else if far {
                    0.5 * dims[axis]
                } else {
                    -0.5 * dims[axis]
                };
                p
            }
            (Shape::Cylinder { depth, .. }, Face::End { far }) => {
                [self.u, self.v, if far { depth } else { 0.0 }]
            }
            (Shape::Cylinder { radius, .. }, Face::Side) => {
                let th = self.u / radius;
                [radius * th.cos(), radius * th.sin(), self.v]
            }
            _ => panic!("patch face does not belong to this shape"),
        }
    }

    /// Checks that the footprint lies on its face. Touching an edge is
    /// allowed (a full-face piston).
    pub fn check(&self, shape: &Shape) -> Result<(), String> {
        let tol = 1e-9 * shape.largest_dimension();
        let (hu, hv) = self.footprint.half();
        if !(hu > 0.0 && hv > 0.0) {
            return Err("footprint size must be positive".into());
        }
        let inside = |c: f64, h: f64, lo: f64, hi: f64| c - h >= lo - tol && c + h <= hi + tol;
        match (*shape, self.face) {
            (Shape::Box { lx, ly, lz }, Face::Box { axis, .. }) => {
                let dims = [lx, ly, lz];
                let (ua, va) = transverse_axes(axis);
                let range = |ax: usize| {
                    if ax == 2 {
                        (0.0, lz)
                    } else {
                        (-0.5 * dims[ax], 0.5 * dims[ax])
                    }
                };
                let (u0, u1) = range(ua);
                let (v0, v1) = range(va);
                if inside(self.u, hu, u0, u1) && inside(self.v, hv, v0, v1) {
                    Ok(())
                } else {
                    Err("footprint extends beyond its face".into())
                }
            }
            (Shape::Cylinder { radius, .. }, Face::End { .. }) => {
                let ok = match self.footprint {
                    Footprint::Disk { radius: b } => self.u.hypot(self.v) + b <= radius + tol,
                    Footprint::Rect { du, dv } => {
                        [(-1.0, -1.0), (-1.0, 1.0), (1.0, -1.0), (1.0, 1.0)]
                            .iter()
                            .all(|(su, sv)| {
                                (self.u + su * 0.5 * du).hypot(self.v + sv * 0.5 * dv)
                                    <= radius + tol
                            })
                    }
                };
                if ok {
                    Ok(())
                } else {
                    Err("footprint extends beyond the end disk".into())
                }
            }
            (Shape::Cylinder { radius, depth }, Face::Side) => {
                if !inside(self.v, hv, 0.0, depth) {
                    return Err("footprint extends beyond the side wall's depth".into());
                }
                let circ = 2.0 * PI * radius;
                let arc_ok = match self.footprint {
                    Footprint::Disk { radius: b } => 2.0 * b <= 0.5 * circ + tol,
                    Footprint::Rect { du, .. } => du <= circ + tol,
                };
                if arc_ok {
                    Ok(())
                } else {
                    Err("footprint wraps around the side wall".into())
                }
            }
            _ => Err("face does not belong to this shape".into()),
        }
    }

    /// True when two footprints on the same face overlap (touching allowed).
    pub fn overlaps(&self, other: &Patch, shape: &Shape) -> bool {
        if self.face != other.face {
            return false;
        }
        let tol = 1e-9 * shape.largest_dimension();
        let mut du = (self.u - other.u).abs();
        if let (Shape::Cylinder { radius, .. }, Face::Side) = (*shape, self.face) {
            let circ = 2.0 * PI * radius;
            du %= circ;
            du = du.min(circ - du);
            let full =
                |p: &Patch| matches!(p.footprint, Footprint::Rect { du, .. } if du >= circ - tol);
            if full(self) || full(other) {
                du = 0.0;
            }
        }
        let dv = (self.v - other.v).abs();
        match (self.footprint, other.footprint) {
            (Footprint::Disk { radius: a }, Footprint::Disk { radius: b }) => {
                du.hypot(dv) < a + b - tol
            }
            (Footprint::Rect { du: a1, dv: b1 }, Footprint::Rect { du: a2, dv: b2 }) => {
                du < 0.5 * (a1 + a2) - tol && dv < 0.5 * (b1 + b2) - tol
            }
            (Footprint::Disk { radius }, Footprint::Rect { du: a, dv: b })
            | (Footprint::Rect { du: a, dv: b }, Footprint::Disk { radius }) => {
                let cu = (du - 0.5 * a).max(0.0);
                let cv = (dv - 0.5 * b).max(0.0);
                cu.hypot(cv) < radius - tol
            }
        }
    }
}

/// Means of every mode over every patch, `[patch][mode]`. Equivalent to
/// calling [`Mode::patch_average`] for each pair, with the transverse means
/// of cylinder-end patches cached per (m, q).
pub fn patch_averages(shape: &Shape, modes: &[Mode], patches: &[Patch]) -> Vec<Vec<f64>> {
    patches
        .iter()
        .map(|p| {
            if let (Shape::Cylinder { .. }, Face::End { far }) = (*shape, p.face) {
                let mut cache: std::collections::HashMap<(u32, u32), (f64, f64)> =
                    std::collections::HashMap::new();
                modes
                    .iter()
                    .map(|md| {
                        let ModeIndex::Cylinder { m, q, l, sine } = md.index else {
                            unreachable!()
                        };
                        let (c, s) = *cache
                            .entry((m, q))
                            .or_insert_with(|| end_transverse_mean(m as usize, md.kr, p));
                        let normal = if far && l % 2 == 1 { -1.0 } else { 1.0 };
                        md.amplitude * normal * if sine { s } else { c }
                    })
                    .collect()
            } else {
                modes.iter().map(|md| md.patch_average(shape, p)).collect()
            }
        })
        .collect()
}

// ----- Quasi-static sums over all modes ----------------------------------

/// Quasi-static modal sums over all modes (see the module documentation).
#[derive(Debug, Clone)]
pub struct StaticSums {
    pub n: usize,
    /// S1 = Σ_{n≠0} φ̄_i φ̄_j / k_n² (m²), row-major n × n.
    pub s1: Vec<f64>,
    /// S2 = Σ_{n≠0} φ̄_i φ̄_j / k_n⁴ (m⁴), row-major n × n.
    pub s2: Vec<f64>,
    /// Whether (i, j) share a wall normal, i.e. whether s1 and s2 hold.
    pub known: Vec<bool>,
    /// Transverse cutoff used for each patch's family (rad/m).
    pub k_transverse: Vec<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Box(usize),
    End,
    Side,
}

fn family(face: Face) -> Family {
    match face {
        Face::Box { axis, .. } => Family::Box(axis),
        Face::End { .. } => Family::End,
        Face::Side => Family::Side,
    }
}

// The three constants below are numerical choices (cost against accuracy),
// not physical data. With them the modal cavity matches the independent
// waveguide-mode references of tools/cavity/generate.py to 1e-5 in the lower
// half of the band, including a 0.5 mm slit (tests/cavity.rs).

/// Upper bound on transverse terms per family in [`static_sums`] (cost).
const TRANSVERSE_CAP: f64 = 5.0e4;
/// Upper bound on transverse terms for cylinder ends (each needs a root of J'_m).
const END_CAP: f64 = 2.0e4;
/// Transverse cutoff target: this many radians across the smallest
/// footprint half-width, beyond which the continuum tail is accurate.
const CUTOFF_OVER_SIZE: f64 = 20.0;

/// Computes S1 and S2 for every pair of patches sharing a wall normal. The
/// transverse cutoff of each family is at least `k_min` (the modal cavity
/// passes its eigenwavenumber cutoff) and aims at 20/(smallest footprint
/// half-width), capped for cost.
pub fn static_sums(shape: &Shape, patches: &[Patch], k_min: f64) -> StaticSums {
    let n = patches.len();
    let mut out = StaticSums {
        n,
        s1: vec![0.0; n * n],
        s2: vec![0.0; n * n],
        known: vec![false; n * n],
        k_transverse: vec![0.0; n],
    };
    let mut fams: Vec<Family> = Vec::new();
    for p in patches {
        let f = family(p.face);
        if !fams.contains(&f) {
            fams.push(f);
        }
    }
    for fam in fams {
        let members: Vec<usize> = (0..n).filter(|&i| family(patches[i].face) == fam).collect();
        for &i in &members {
            for &j in &members {
                out.known[i * n + j] = true;
            }
        }
        // Smallest half-width among directions that are not full-span.
        let mut s_min = f64::INFINITY;
        for &i in &members {
            let (full_u, full_v) = full_span(shape, &patches[i]);
            match patches[i].footprint {
                Footprint::Disk { radius } => {
                    if !(fam == Family::End && full_end_disk(shape, &patches[i])) {
                        s_min = s_min.min(radius);
                    }
                }
                Footprint::Rect { du, dv } => {
                    if !full_u {
                        s_min = s_min.min(0.5 * du);
                    }
                    if !full_v {
                        s_min = s_min.min(0.5 * dv);
                    }
                }
            }
        }
        let k_cap = match (*shape, fam) {
            (Shape::Box { lx, ly, lz }, Family::Box(axis)) => {
                let dims = [lx, ly, lz];
                let (ua, va) = transverse_axes(axis);
                (4.0 * PI * TRANSVERSE_CAP / (dims[ua] * dims[va])).sqrt()
            }
            (Shape::Cylinder { radius, .. }, Family::End) => (4.0 * END_CAP).sqrt() / radius,
            (Shape::Cylinder { radius, depth }, _) => {
                (2.0 * TRANSVERSE_CAP / (radius * depth)).sqrt()
            }
            _ => unreachable!(),
        };
        let k_t = k_min.max((CUTOFF_OVER_SIZE / s_min).min(k_cap));
        for &i in &members {
            out.k_transverse[i] = k_t;
        }
        match (*shape, fam) {
            (Shape::Box { lx, ly, lz }, Family::Box(axis)) => {
                box_family([lx, ly, lz], axis, &members, patches, k_t, &mut out)
            }
            (Shape::Cylinder { radius, depth }, Family::End) => {
                end_family(radius, depth, &members, patches, k_t, &mut out)
            }
            (Shape::Cylinder { radius, depth }, Family::Side) => {
                side_family(radius, depth, &members, patches, k_t, &mut out)
            }
            _ => unreachable!(),
        }
    }
    out
}

/// Whether a rectangular footprint spans its whole face along u and v.
fn full_span(shape: &Shape, p: &Patch) -> (bool, bool) {
    let Footprint::Rect { du, dv } = p.footprint else {
        return (false, false);
    };
    let tol = 1e-9;
    match (*shape, p.face) {
        (Shape::Box { lx, ly, lz }, Face::Box { axis, .. }) => {
            let dims = [lx, ly, lz];
            let (ua, va) = transverse_axes(axis);
            (du >= dims[ua] * (1.0 - tol), dv >= dims[va] * (1.0 - tol))
        }
        (Shape::Cylinder { radius, depth }, Face::Side) => (
            du >= 2.0 * PI * radius * (1.0 - tol),
            dv >= depth * (1.0 - tol),
        ),
        _ => (false, false),
    }
}

/// Whether a disk footprint covers a whole cylinder end.
fn full_end_disk(shape: &Shape, p: &Patch) -> bool {
    match (*shape, p.footprint) {
        (Shape::Cylinder { radius, .. }, Footprint::Disk { radius: b }) => {
            b >= radius * (1.0 - 1e-9)
        }
        _ => false,
    }
}

/// Sums of Σ_l ε_l c_l c'_l / (κ² + (lπ/L)²)^s along a wall normal of length
/// L, for s = 1, 2 and ports on the same side (c_l = c'_l = 1) or opposite
/// sides (c_l·c'_l = (−1)^l): [g1 same, g1 opposite, g2 same, g2 opposite].
/// κ = 0 excludes the l = 0 term (the uniform mode).
fn axial_kernels(kap: f64, l: f64) -> [f64; 4] {
    if kap == 0.0 {
        // Σ 2/(lπ/L)² = L²/3, Σ 2(−1)^l/(lπ/L)² = −L²/6,
        // Σ 2/(lπ/L)⁴ = L⁴/45, Σ 2(−1)^l/(lπ/L)⁴ = −7L⁴/360.
        let l2 = l * l;
        return [l2 / 3.0, -l2 / 6.0, l2 * l2 / 45.0, -7.0 * l2 * l2 / 360.0];
    }
    let x = kap * l;
    let e1 = (-x).exp();
    let om = -(-2.0 * x).exp_m1(); // 1 − e^{−2x}
    let coth = (1.0 + e1 * e1) / om;
    let csch = 2.0 * e1 / om;
    let k2 = kap * kap;
    [
        l / kap * coth,
        l / kap * csch,
        l / (2.0 * k2 * kap) * coth + l * l / (2.0 * k2) * csch * csch,
        l / (2.0 * k2 * kap) * csch + l * l / (2.0 * k2) * coth * csch,
    ]
}

/// Adds t_a·t_b·g to the pair sums of the listed members.
fn accumulate(
    out: &mut StaticSums,
    members: &[usize],
    t: &[f64],
    same: &dyn Fn(usize, usize) -> bool,
    g: [f64; 4],
) {
    let n = out.n;
    for (a, &i) in members.iter().enumerate() {
        if t[a] == 0.0 {
            continue;
        }
        for (b, &j) in members.iter().enumerate().skip(a) {
            let w = t[a] * t[b];
            let (g1, g2) = if same(a, b) {
                (g[0], g[2])
            } else {
                (g[1], g[3])
            };
            out.s1[i * n + j] += w * g1;
            out.s2[i * n + j] += w * g2;
            if i != j {
                out.s1[j * n + i] += w * g1;
                out.s2[j * n + i] += w * g2;
            }
        }
    }
}

#[allow(clippy::needless_range_loop)]
fn box_family(
    dims: [f64; 3],
    axis: usize,
    members: &[usize],
    patches: &[Patch],
    k_t: f64,
    out: &mut StaticSums,
) {
    let (ua, va) = transverse_axes(axis);
    let (lu, lv, l) = (dims[ua], dims[va], dims[axis]);
    let pm: Vec<Patch> = members.iter().map(|&i| patches[i]).collect();
    let far: Vec<bool> = pm
        .iter()
        .map(|p| matches!(p.face, Face::Box { far: true, .. }))
        .collect();
    let same = |a: usize, b: usize| far[a] == far[b];
    let uc: Vec<f64> = pm.iter().map(|p| p.u + corner_offset(dims, ua)).collect();
    let vc: Vec<f64> = pm.iter().map(|p| p.v + corner_offset(dims, va)).collect();
    let pmax = (k_t * lu / PI).floor() as usize;
    let qmax = (k_t * lv / PI).floor() as usize;
    // Separable factors per member: along u (index p) and along v (index q).
    let fac = |len: f64, cmax: usize, centre: &[f64], ext: &dyn Fn(&Patch) -> Option<f64>| {
        pm.iter()
            .enumerate()
            .map(|(a, p)| {
                (0..=cmax)
                    .map(|i| {
                        let k = i as f64 * PI / len;
                        let e = if i == 0 { 1.0 } else { 2f64.sqrt() };
                        let s = ext(p).map_or(1.0, |w| sinc(0.5 * k * w));
                        e * (k * centre[a]).cos() * s
                    })
                    .collect::<Vec<f64>>()
            })
            .collect::<Vec<_>>()
    };
    let fu = fac(lu, pmax, &uc, &|p| match p.footprint {
        Footprint::Rect { du, .. } => Some(du),
        _ => None,
    });
    let fv = fac(lv, qmax, &vc, &|p| match p.footprint {
        Footprint::Rect { dv, .. } => Some(dv),
        _ => None,
    });
    let mut t = vec![0.0; pm.len()];
    for p in 0..=pmax {
        let kp = p as f64 * PI / lu;
        for q in 0..=qmax {
            let kq = q as f64 * PI / lv;
            let kap = kp.hypot(kq);
            if kap >= k_t {
                break;
            }
            for (a, pa) in pm.iter().enumerate() {
                t[a] = fu[a][p] * fv[a][q];
                if let Footprint::Disk { radius } = pa.footprint {
                    t[a] *= jinc(kap * radius);
                }
            }
            accumulate(out, members, &t, &same, axial_kernels(kap, l));
        }
    }
    // Continuum tails of the self terms.
    let v = dims[0] * dims[1] * dims[2];
    let n = out.n;
    for (a, &i) in members.iter().enumerate() {
        let p = &pm[a];
        let (full_u, full_v) = full_span(
            &Shape::Box {
                lx: dims[0],
                ly: dims[1],
                lz: dims[2],
            },
            p,
        );
        let kern = |k: f64| {
            let g = axial_kernels(k, l);
            (g[0], g[2])
        };
        let (t1, t2) = match (p.footprint, full_u, full_v) {
            (_, true, true) => (0.0, 0.0),
            (Footprint::Rect { dv, .. }, true, false) => {
                let (a1, a2) = tail_1d(dv, k_t, &kern);
                (lv / PI * a1, lv / PI * a2)
            }
            (Footprint::Rect { du, .. }, false, true) => {
                let (a1, a2) = tail_1d(du, k_t, &kern);
                (lu / PI * a1, lu / PI * a2)
            }
            (fp, _, _) => {
                let (a1, a2) = tail_2d(fp, k_t, Some(l));
                let f = v / (4.0 * PI * PI);
                (f * a1, f * a2)
            }
        };
        out.s1[i * n + i] += t1;
        out.s2[i * n + i] += t2;
    }
}

fn end_family(
    a: f64,
    d: f64,
    members: &[usize],
    patches: &[Patch],
    k_t: f64,
    out: &mut StaticSums,
) {
    let pm: Vec<Patch> = members.iter().map(|&i| patches[i]).collect();
    let far: Vec<bool> = pm
        .iter()
        .map(|p| matches!(p.face, Face::End { far: true }))
        .collect();
    let same = |x: usize, y: usize| far[x] == far[y];
    // Uniform transverse mode.
    let ones = vec![1.0; pm.len()];
    accumulate(out, members, &ones, &same, axial_kernels(0.0, d));
    let zeros = bessel_jp_zeros_all(k_t * a);
    let mut tc = vec![0.0; pm.len()];
    let mut ts = vec![0.0; pm.len()];
    for (m, roots) in zeros.iter().enumerate() {
        for &jp in roots {
            let kr = jp / a;
            if kr >= k_t {
                break;
            }
            let (mean, _) = disk_mean_jm2(m, jp);
            let nt = (eps(m as u32) / mean).sqrt();
            for (x, p) in pm.iter().enumerate() {
                let (c, s) = end_transverse_mean(m, kr, p);
                tc[x] = nt * c;
                ts[x] = nt * s;
            }
            let g = axial_kernels(kr, d);
            accumulate(out, members, &tc, &same, g);
            if m > 0 {
                accumulate(out, members, &ts, &same, g);
            }
        }
    }
    let v = PI * a * a * d;
    let n = out.n;
    let shape = Shape::Cylinder {
        radius: a,
        depth: d,
    };
    for (x, &i) in members.iter().enumerate() {
        if full_end_disk(&shape, &pm[x]) {
            continue;
        }
        let (a1, a2) = tail_2d(pm[x].footprint, k_t, Some(d));
        let f = v / (4.0 * PI * PI);
        out.s1[i * n + i] += f * a1;
        out.s2[i * n + i] += f * a2;
    }
}

/// Radial sums Σ_q φ_q(a)²/(k_q² + x²/a²)^s at the side wall for orders
/// 0..=m_max at x = k_z·a: s = 1 gives a²/(2h) and s = 2 gives
/// a⁴·h'(x)/(4x·h²), with h = x·I_m'(x)/I_m(x) = x·r_m + m, r_m = I_{m+1}/I_m,
/// and h' = x(1 − r_m²) − 2m·r_m. The uniform mode is excluded at m = x = 0.
fn radial_kernels(m_max: usize, x: f64, a: f64) -> Vec<(f64, f64)> {
    let a2 = a * a;
    if x == 0.0 {
        return (0..=m_max)
            .map(|m| {
                if m == 0 {
                    (a2 / 8.0, a2 * a2 / 192.0)
                } else {
                    let mf = m as f64;
                    (a2 / (2.0 * mf), a2 * a2 / (4.0 * (mf + 1.0) * mf * mf))
                }
            })
            .collect();
    }
    let mut r = vec![0.0; m_max + 1];
    r[m_max] = bessel_i_ratio_real(m_max, x);
    for m in (1..=m_max).rev() {
        r[m - 1] = 1.0 / (2.0 * m as f64 / x + r[m]);
    }
    r.iter()
        .enumerate()
        .map(|(m, &rm)| {
            let mf = m as f64;
            let h = x * rm + mf;
            let hp = x * (1.0 - rm * rm) - 2.0 * mf * rm;
            (a2 / (2.0 * h), a2 * a2 * hp / (4.0 * x * h * h))
        })
        .collect()
}

fn side_family(
    a: f64,
    d: f64,
    members: &[usize],
    patches: &[Patch],
    k_t: f64,
    out: &mut StaticSums,
) {
    let pm: Vec<Patch> = members.iter().map(|&i| patches[i]).collect();
    let same = |_: usize, _: usize| true;
    let mut tc = vec![0.0; pm.len()];
    let mut ts = vec![0.0; pm.len()];
    let mut l = 0usize;
    loop {
        let kz = l as f64 * PI / d;
        if kz >= k_t {
            break;
        }
        let m_max = (a * (k_t * k_t - kz * kz).sqrt()).floor() as usize;
        let kern = radial_kernels(m_max, kz * a, a);
        let el = eps(l as u32);
        for (m, &(g1, g2)) in kern.iter().enumerate() {
            let mt = m as f64 / a;
            let kap = mt.hypot(kz);
            if kap >= k_t {
                break;
            }
            let e = (eps(m as u32) * el).sqrt();
            for (x, p) in pm.iter().enumerate() {
                let common = e * match p.footprint {
                    Footprint::Disk { radius } => (kz * p.v).cos() * jinc(kap * radius),
                    Footprint::Rect { du, dv } => {
                        sinc(0.5 * mt * du) * (kz * p.v).cos() * sinc(0.5 * kz * dv)
                    }
                };
                let th = m as f64 * p.u / a;
                tc[x] = common * th.cos();
                ts[x] = common * th.sin();
            }
            let g = [g1, g1, g2, g2];
            accumulate(out, members, &tc, &same, g);
            if m > 0 {
                accumulate(out, members, &ts, &same, g);
            }
        }
        l += 1;
    }
    let v = PI * a * a * d;
    let n = out.n;
    let shape = Shape::Cylinder {
        radius: a,
        depth: d,
    };
    for (x, &i) in members.iter().enumerate() {
        let p = &pm[x];
        let (full_u, full_v) = full_span(&shape, p);
        let (t1, t2) = match (p.footprint, full_u, full_v) {
            (_, true, true) => (0.0, 0.0),
            // Full circumference: only m = 0; continuum over the axial order.
            (Footprint::Rect { dv, .. }, true, false) => {
                let kern = |k: f64| radial_kernels(0, k * a, a)[0];
                let (a1, a2) = tail_1d(dv, k_t, &kern);
                (d / PI * a1, d / PI * a2)
            }
            // Full depth: only l = 0; continuum over the azimuthal order,
            // cosine and sine together (weight 2 per order, spacing 1/a).
            (Footprint::Rect { du, .. }, false, true) => {
                let kern = |k: f64| {
                    let mf = k * a;
                    (a * a / (2.0 * mf), a.powi(4) / (4.0 * (mf + 1.0) * mf * mf))
                };
                let (a1, a2) = tail_1d(du, k_t, &kern);
                (2.0 * a * a1, 2.0 * a * a2)
            }
            (fp, _, _) => {
                let (a1, a2) = tail_2d(fp, k_t, None);
                let f = v / (4.0 * PI * PI);
                (f * a1, f * a2)
            }
        };
        out.s1[i * n + i] += t1;
        out.s2[i * n + i] += t2;
    }
}

/// Angular panels of the second-order tail integral.
const T2_ANGULAR_PANELS: usize = 48;

/// ∫∫∫∫ dS dS'/|r − r'| over a w × h rectangle and itself (the closed form
/// of the self-potential of a uniformly charged rectangle, checked against
/// quadrature in tools/cavity).
pub fn rect_self_potential(w: f64, h: f64) -> f64 {
    let d = w.hypot(h);
    2.0 * w * w * h * (h / w).asinh()
        + 2.0 * w * h * h * (w / h).asinh()
        + 2.0 / 3.0 * (w.powi(3) + h.powi(3))
        - 2.0 / 3.0 * d.powi(3)
}

/// ∫₀^{2π} F(κ cos θ, κ sin θ)² dθ for a footprint's plane-wave mean F.
/// Rectangles use Gauss–Legendre panels in θ, two per oscillation of the
/// sincs, at most `max_panels`.
fn angular_f2(fp: Footprint, kap: f64, max_panels: usize) -> f64 {
    match fp {
        Footprint::Disk { radius } => 2.0 * PI * jinc(kap * radius).powi(2),
        Footprint::Rect { du, dv } => {
            let panels = ((kap * du.max(dv) / PI).ceil() as usize + 2).min(max_panels);
            4.0 * integrate(0.0, 0.5 * PI, panels, |t| {
                (sinc(0.5 * kap * du * t.cos()) * sinc(0.5 * kap * dv * t.sin())).powi(2)
            })
        }
    }
}

/// Continuum tails beyond the transverse cutoff κ_t for a self term:
/// T1 = ∫_{|κ|>κ_t} F²·c1(κ)/κ d²κ and T2 = ∫_{|κ|>κ_t} F²·c2(κ)/(2κ³) d²κ,
/// with c1 = coth(κL), c2 = coth(κL) + κL/sinh²(κL) along a normal of length
/// L (box, cylinder end) or c1 = c2 = 1 (cylinder side). The caller
/// multiplies by V/(4π²). T1 is the half-space static self-mass integral
/// (32/(3b) for a disk of radius b; 2π·I/(w·h)² for a rectangle with
/// self-potential I) minus its part inside the cutoff.
fn tail_2d(fp: Footprint, k_t: f64, normal: Option<f64>) -> (f64, f64) {
    let size = match fp {
        Footprint::Disk { radius } => 2.0 * radius,
        Footprint::Rect { du, dv } => du.max(dv),
    };
    let full = match fp {
        Footprint::Disk { radius } => 32.0 / (3.0 * radius),
        Footprint::Rect { du, dv } => 2.0 * PI / (du * dv).powi(2) * rect_self_potential(du, dv),
    };
    let panels = (k_t * size / PI).ceil() as usize + 4;
    let inner = integrate(0.0, k_t, panels, |k| angular_f2(fp, k, 400));
    let mut t1 = full - inner;
    if let Some(l) = normal {
        if 2.0 * k_t * l < 40.0 {
            let span = 20.0 / l;
            let panels = (span * size / PI).ceil() as usize + 8;
            t1 += integrate(k_t, k_t + span, panels, |k| {
                let x = k * l;
                let coth_m1 = 2.0 / (2.0 * x).exp_m1();
                angular_f2(fp, k, 400) * coth_m1
            });
        }
    }
    let c2 = |k: f64| match normal {
        Some(l) => {
            let x = k * l;
            if x > 40.0 {
                1.0
            } else {
                1.0 / x.tanh() + x / x.sinh().powi(2)
            }
        }
        None => 1.0,
    };
    // κ = κ_t/t maps (κ_t, ∞) onto (0, 1]. T2 only scales the k² term of the
    // residual, so a coarser angular rule suffices (the unit tests bound its
    // effect on a slit).
    let t2 = integrate(0.0, 1.0, 32, |t| {
        let k = k_t / t;
        angular_f2(fp, k, T2_ANGULAR_PANELS) * c2(k) / (2.0 * k_t)
    });
    (t1, t2)
}

/// ∫_{κ_t}^∞ sinc²(κ·w/2)·g(κ) dκ for the two kernels g = (g1, g2), by the
/// map κ = κ_t/t.
fn tail_1d(w: f64, k_t: f64, g: &dyn Fn(f64) -> (f64, f64)) -> (f64, f64) {
    let mut a1 = 0.0;
    let mut a2 = 0.0;
    let (x, wts) = gauss16();
    let panels = 128;
    let h = 1.0 / panels as f64;
    for p in 0..panels {
        let mid = (p as f64 + 0.5) * h;
        for (xi, wi) in x.iter().zip(wts) {
            let t = mid + 0.5 * h * xi;
            let k = k_t / t;
            let s = sinc(0.5 * k * w).powi(2) * k_t / (t * t);
            let (g1, g2) = g(k);
            a1 += wi * s * g1;
            a2 += wi * s * g2;
        }
    }
    (a1 * 0.5 * h, a2 * 0.5 * h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bessel_reference_values() {
        // A&S Table 9.1 / mpmath.
        assert!((bessel_jn(0, 1.0) - 0.765_197_686_557_966_6).abs() < 1e-15);
        assert!((bessel_j1(1.0) - 0.440_050_585_744_933_5).abs() < 1e-15);
        assert!((bessel_j1(10.0) - 0.043_472_746_168_861_44).abs() < 1e-15);
        // Continuity across the asymptotic switch at 25.
        let exact = -0.125_350_249_580_289_9; // mpmath
        let a = bessel_j_all(1, 25.0)[1];
        let b = hankel_j(1, 25.0);
        assert!(
            (a - exact).abs() < 1e-15 && (b - exact).abs() < 1e-15,
            "{a} {b}"
        );
    }

    #[test]
    fn first_derivative_roots() {
        // DLMF Table 10.21.2 (j'_{m,s}): 1.84118, 3.05424, 3.83171.
        let z = bessel_jp_zeros_all(8.0);
        assert!((z[1][0] - 1.841_183_781_340_659).abs() < 1e-12);
        assert!((z[2][0] - 3.054_236_928_227_14).abs() < 1e-12);
        assert!((z[0][0] - 3.831_705_970_207_512).abs() < 1e-12);
        assert!((z[0][1] - 7.015_586_669_815_619).abs() < 1e-12);
    }

    #[test]
    fn cylinder_disk_mean_of_jm2_matches_quadrature() {
        // ⟨J_m²⟩ = (2/a²)∫₀ᵃ J_m(k r)² r dr at a root of J'_m.
        let a = 0.025;
        let zeros = bessel_jp_zeros_all(20.0);
        for (m, roots) in zeros.iter().enumerate().take(6) {
            for &jp in roots.iter().take(3) {
                let k = jp / a;
                let num = integrate(0.0, a, 8, |r| bessel_jn(m, k * r).powi(2) * r) * 2.0 / (a * a);
                let (mean, _) = disk_mean_jm2(m, jp);
                assert!((num - mean).abs() < 1e-12, "m={m} j'={jp}: {num} vs {mean}");
            }
        }
    }

    #[test]
    fn tail_integrals_are_consistent() {
        // Disk: T1 from the closed-form full integral minus the inner part
        // equals a direct integral of the exterior.
        let fp = Footprint::Disk { radius: 0.002 };
        let k_t = 3000.0;
        let (t1, _) = tail_2d(fp, k_t, None);
        // Direct: ∫ 2π·jinc²(κb) dκ from κ_t to K, two panels per period,
        // plus the mean of the asymptote 2π·4/(π(κb)³) beyond K.
        let big = 2.0e6;
        let direct = integrate(k_t, big, 2600, |k| angular_f2(fp, k, 400))
            + 4.0 / (0.002f64.powi(3) * big * big);
        assert!((t1 / direct - 1.0).abs() < 1e-6, "{t1} vs {direct}");
        // Slit: the coarse angular rule of T2 is within 1 % of a fine one.
        let fp = Footprint::Rect {
            du: 0.01,
            dv: 0.0005,
        };
        let k_t = 45_000.0;
        let (_, t2) = tail_2d(fp, k_t, None);
        let fine = integrate(0.0, 1.0, 96, |t| {
            let k = k_t / t;
            angular_f2(fp, k, 600) / (2.0 * k_t)
        });
        assert!((t2 / fine - 1.0).abs() < 1e-2, "{t2} vs {fine}");
    }

    #[test]
    fn disk_mean_value_theorem_matches_quadrature() {
        let shape = Shape::Cylinder {
            radius: 0.025,
            depth: 0.02,
        };
        let (xc, yc, b) = (0.008, -0.005, 0.006);
        let patch = Patch {
            face: Face::End { far: true },
            u: xc,
            v: yc,
            footprint: Footprint::Disk { radius: b },
        };
        for md in shape.modes_below(900.0).iter().take(40) {
            let total = integrate(0.0, b, 4, |r| {
                r * integrate(0.0, 2.0 * PI, 6, |th| {
                    md.value(&shape, [xc + r * th.cos(), yc + r * th.sin(), 0.02])
                })
            });
            let mean = total / (PI * b * b);
            let closed = md.patch_average(&shape, &patch);
            assert!(
                (mean - closed).abs() < 1e-10,
                "{:?}: {mean} vs {closed}",
                md.index
            );
        }
    }
}
