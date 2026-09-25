//! Modified nodal analysis stamping.
//!
//! Convention (see docs/conventions.md): every domain uses the across/through
//! pair as MNA potential/current —
//! electrical voltage/current, mechanical velocity/force,
//! acoustic pressure/volume velocity. Each KCL row sums the flows *leaving*
//! its node through elements; independent source injections go on the
//! right-hand side. Ground (reference) is `None`.

use crate::linalg::Matrix;
use crate::C64;

/// Index of an unknown, or `None` for the reference (ground) node.
pub type Unknown = Option<usize>;

pub const ZERO: C64 = C64::new(0.0, 0.0);
pub const ONE: C64 = C64::new(1.0, 0.0);

/// The MNA system for one frequency.
pub struct Mna {
    pub n_nodes: usize,
    pub a: Matrix,
    pub rhs: Vec<C64>,
}

impl Mna {
    pub fn new(n_nodes: usize, n_branches: usize) -> Self {
        let n = n_nodes + n_branches;
        Mna {
            n_nodes,
            a: Matrix::zeros(n),
            rhs: vec![ZERO; n],
        }
    }

    pub fn dim(&self) -> usize {
        self.a.n
    }

    pub fn clear(&mut self) {
        self.a.clear();
        self.rhs.iter_mut().for_each(|x| *x = ZERO);
    }

    /// Adds `v` at (row, col), ignoring ground.
    #[inline]
    pub fn add(&mut self, r: Unknown, c: Unknown, v: C64) {
        if let (Some(r), Some(c)) = (r, c) {
            self.a.add(r, c, v);
        }
    }

    #[inline]
    pub fn add_rhs(&mut self, r: Unknown, v: C64) {
        if let Some(r) = r {
            self.rhs[r] += v;
        }
    }

    /// Two-terminal admittance `y` between `n1` and `n2`.
    pub fn admittance(&mut self, n1: Unknown, n2: Unknown, y: C64) {
        self.add(n1, n1, y);
        self.add(n2, n2, y);
        self.add(n1, n2, -y);
        self.add(n2, n1, -y);
    }

    /// Independent flow source: injects `i` into `into`, drawn from `from`.
    pub fn flow_source(&mut self, into: Unknown, from: Unknown, i: C64) {
        self.add_rhs(into, i);
        self.add_rhs(from, -i);
    }

    /// Voltage-controlled flow source: injects g·(v(cp) − v(cn)) into `op`,
    /// drawn from `on`.
    pub fn vccs(&mut self, op: Unknown, on: Unknown, cp: Unknown, cn: Unknown, g: C64) {
        // Flow leaving `op` through the element is −g·Vc.
        self.add(op, cp, -g);
        self.add(op, cn, g);
        self.add(on, cp, g);
        self.add(on, cn, -g);
    }

    /// Ideal potential source with series impedance: v(p) − v(n) = e − z·i,
    /// where the branch unknown `br` is the flow the source delivers out of
    /// its `p` terminal into the external network.
    pub fn potential_source(&mut self, p: Unknown, n: Unknown, br: usize, e: C64, z: C64) {
        let b = Some(br);
        // Flow leaving p through the source is −i; leaving n is +i.
        self.add(p, b, -ONE);
        self.add(n, b, ONE);
        // Branch equation: v(p) − v(n) + z·i = e.
        self.add(b, p, ONE);
        self.add(b, n, -ONE);
        self.add(b, b, z);
        self.add_rhs(b, e);
    }

    /// Ideal transformer: v(p1) − v(n1) = ratio · (v(p2) − v(n2)).
    /// Branch unknown `br` is the flow entering port 1 at `p1`; the element
    /// injects ratio·i into `p2` (drawn from `n2`), so power is conserved.
    pub fn transformer(
        &mut self,
        (p1, n1): (Unknown, Unknown),
        (p2, n2): (Unknown, Unknown),
        br: usize,
        ratio: C64,
    ) {
        let b = Some(br);
        self.add(p1, b, ONE);
        self.add(n1, b, -ONE);
        self.add(p2, b, -ratio);
        self.add(n2, b, ratio);
        self.add(b, p1, ONE);
        self.add(b, n1, -ONE);
        self.add(b, p2, -ratio);
        self.add(b, n2, ratio);
    }

    /// Reciprocal two-port given by its transfer matrix, with I1 entering
    /// port 1 at `p1` and I2 leaving port 2 at `p2` carried by the branch
    /// unknowns `br1` and `br2`. It is stamped in transmission form, which
    /// stays valid where B or C vanish (a lossless half-wave line, erratum
    /// E14), unless the factored-out growth exceeds
    /// [`ADMITTANCE_FORM_SCALE`]: then the transmission rows are swamped by
    /// A ≈ D ≈ cosh Γl and the admittance form is used, where B cannot
    /// vanish (|sinh Γl| ≥ sinh Re Γl).
    pub fn two_port(
        &mut self,
        port1: (Unknown, Unknown),
        port2: (Unknown, Unknown),
        branches: (usize, usize),
        t: &Transfer,
    ) {
        if t.scale > ADMITTANCE_FORM_SCALE {
            self.two_port_admittance(port1, port2, branches, t.admittance());
        } else {
            self.two_port_abcd(port1, port2, branches, t.abcd());
        }
    }

    /// KCL entries of a two-port's branch unknowns: `br1` enters the
    /// element at `p1`, `br2` leaves it into `p2`.
    fn two_port_flows(
        &mut self,
        (p1, n1): (Unknown, Unknown),
        (p2, n2): (Unknown, Unknown),
        (br1, br2): (usize, usize),
    ) {
        let (i1, i2) = (Some(br1), Some(br2));
        self.add(p1, i1, ONE);
        self.add(n1, i1, -ONE);
        self.add(p2, i2, -ONE);
        self.add(n2, i2, ONE);
    }

    /// General two-port in transmission (ABCD) form:
    /// [V1; I1] = [[A, B], [C, D]] · [V2; I2], with I1 entering port 1 at
    /// `p1` and I2 leaving port 2 at `p2`. Branch unknowns `br1` (I1) and
    /// `br2` (I2) make the stamp valid even where B or C vanish.
    pub fn two_port_abcd(
        &mut self,
        (p1, n1): (Unknown, Unknown),
        (p2, n2): (Unknown, Unknown),
        (br1, br2): (usize, usize),
        [a, b, c, d]: [C64; 4],
    ) {
        self.two_port_flows((p1, n1), (p2, n2), (br1, br2));
        let (i1, i2) = (Some(br1), Some(br2));
        // V1 − A·V2 − B·I2 = 0
        self.add(i1, p1, ONE);
        self.add(i1, n1, -ONE);
        self.add(i1, p2, -a);
        self.add(i1, n2, a);
        self.add(i1, i2, -b);
        // I1 − C·V2 − D·I2 = 0
        self.add(i2, i1, ONE);
        self.add(i2, p2, -c);
        self.add(i2, n2, c);
        self.add(i2, i2, -d);
    }

    /// General two-port in admittance form:
    /// [I1; −I2] = [[Y11, Y12], [Y21, Y22]] · [V1; V2], with the same branch
    /// unknowns as [`Mna::two_port_abcd`] (I1 entering port 1, I2 leaving
    /// port 2), so port flows are read the same way in either form.
    pub fn two_port_admittance(
        &mut self,
        (p1, n1): (Unknown, Unknown),
        (p2, n2): (Unknown, Unknown),
        (br1, br2): (usize, usize),
        [y11, y12, y21, y22]: [C64; 4],
    ) {
        self.two_port_flows((p1, n1), (p2, n2), (br1, br2));
        let (i1, i2) = (Some(br1), Some(br2));
        // I1 − Y11·V1 − Y12·V2 = 0
        self.add(i1, i1, ONE);
        self.add(i1, p1, -y11);
        self.add(i1, n1, y11);
        self.add(i1, p2, -y12);
        self.add(i1, n2, y12);
        // I2 + Y21·V1 + Y22·V2 = 0
        self.add(i2, i2, ONE);
        self.add(i2, p1, y21);
        self.add(i2, n1, -y21);
        self.add(i2, p2, y22);
        self.add(i2, n2, -y22);
    }

    /// Ideal short (zero-potential branch) between two nodes; the branch
    /// unknown is the flow from `p` to `n`.
    pub fn short(&mut self, p: Unknown, n: Unknown, br: usize) {
        self.potential_source(p, n, br, ZERO, ZERO);
    }
}

/// Potential of an unknown in a solution vector (ground reads zero).
#[inline]
pub fn potential(x: &[C64], u: Unknown) -> C64 {
    u.map_or(ZERO, |i| x[i])
}

/// Multiplies two ABCD matrices (cascade).
pub fn abcd_mul(m: [C64; 4], n: [C64; 4]) -> [C64; 4] {
    [
        m[0] * n[0] + m[1] * n[2],
        m[0] * n[1] + m[1] * n[3],
        m[2] * n[0] + m[3] * n[2],
        m[2] * n[1] + m[3] * n[3],
    ]
}

/// ABCD of a series impedance.
pub fn abcd_series(z: C64) -> [C64; 4] {
    [ONE, z, ZERO, ONE]
}

/// ABCD of a shunt admittance.
pub fn abcd_shunt(y: C64) -> [C64; 4] {
    [ONE, ZERO, y, ONE]
}

/// Growth factor, as the exponent s of a [`Transfer`], above which a
/// two-port is stamped in admittance form. A lossy line's A = D = cosh Γl
/// swamp the input relation of the transmission rows as they grow towards
/// 1/ε: the solve reported a singular system from Re Γl ≈ 33
/// (cosh Γl ≈ 10¹⁴), which a pad leak of a few µm reaches in the audio
/// band. At e^10 ≈ 2·10⁴ the rows still keep about twelve digits.
pub const ADMITTANCE_FORM_SCALE: f64 = 10.0;

/// Transfer (ABCD) matrix e^s·m of a reciprocal two-port (det = 1), with
/// the exponential growth of its lossy lines kept apart in `scale` = s, so
/// that neither overflow (beyond Re Γl ≈ 710) nor the magnitude of cosh Γl
/// reaches the stamp (see [`Mna::two_port`]). Every two-port the engine
/// builds is reciprocal: lines, cones, series and shunt elements, parallel
/// copies and cascades of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transfer {
    pub m: [C64; 4],
    pub scale: f64,
}

impl Transfer {
    /// A uniform line of electrical length `gl` = Γl and characteristic
    /// impedance `zc`, with s = |Re Γl|.
    pub fn line(gl: C64, zc: C64) -> Self {
        let s = gl.re.abs();
        let (ch, sh) = if s < 20.0 {
            let k = (-s).exp();
            (gl.cosh() * k, gl.sinh() * k)
        } else {
            // e^{±Γl − s} directly: cosh and sinh overflow beyond s ≈ 710.
            let fwd = C64::from_polar((gl.re - s).exp(), gl.im);
            let back = C64::from_polar((-gl.re - s).exp(), -gl.im);
            (0.5 * (fwd + back), 0.5 * (fwd - back))
        };
        Transfer {
            m: [ch, zc * sh, sh / zc, ch],
            scale: s,
        }
    }

    /// `n` identical copies in parallel.
    pub fn parallel(self, n: f64) -> Self {
        let [a, b, c, d] = self.m;
        Transfer {
            m: [a, b / n, c * n, d],
            ..self
        }
    }

    /// The transfer matrix itself. Its entries overflow for a scale beyond
    /// about 700.
    pub fn abcd(&self) -> [C64; 4] {
        let g = self.scale.exp();
        self.m.map(|x| x * g)
    }

    /// Short-circuit admittance matrix [Y11, Y12, Y21, Y22], both flows
    /// entering: Y11 = D/B, Y22 = A/B and, with det = 1, Y12 = Y21 = −1/B.
    /// For a line, coth(Γl)/Zc and −1/(Zc·sinh Γl).
    pub fn admittance(&self) -> [C64; 4] {
        let [a, b, _, d] = self.m;
        let y12 = -(-self.scale).exp() / b;
        [d / b, y12, y12, a / b]
    }
}

impl From<[C64; 4]> for Transfer {
    fn from(m: [C64; 4]) -> Self {
        Transfer { m, scale: 0.0 }
    }
}

/// Cascade: `self` followed by `rhs`.
impl std::ops::Mul for Transfer {
    type Output = Transfer;
    // The scales are exponents: e^a·e^b = e^(a+b).
    #[allow(clippy::suspicious_arithmetic_impl)]
    fn mul(self, rhs: Transfer) -> Transfer {
        Transfer {
            m: abcd_mul(self.m, rhs.m),
            scale: self.scale + rhs.scale,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn rel(a: C64, b: C64) -> f64 {
        (a - b).norm() / b.norm()
    }

    #[test]
    fn line_factors_out_its_growth() {
        let zc = C64::new(3e7, -2.5e7);
        for gl in [
            C64::new(1e-9, 2e-9),
            C64::new(0.3, 2.0),
            C64::new(15.0, 14.0),
            C64::new(25.0, 25.5),
            C64::new(-25.0, 3.0),
        ] {
            let (ch, sh) = (gl.cosh(), gl.sinh());
            let t = Transfer::line(gl, zc);
            assert_eq!(t.scale, gl.re.abs());
            for (x, y) in t.abcd().iter().zip([ch, zc * sh, sh / zc, ch]) {
                assert!(rel(*x, y) < 1e-14, "{gl}: {x} vs {y}");
            }
        }
        // Far beyond the range of cosh: coth Γl/Zc → 1/Zc, 1/sinh Γl → 0.
        let [y11, y12, y21, y22] = Transfer::line(C64::new(1000.0, 1000.0), zc).admittance();
        assert!(rel(y11, zc.inv()) < 1e-15 && rel(y22, zc.inv()) < 1e-15);
        assert!(y12 == y21 && y12.norm() * zc.norm() < 1e-300);
    }

    /// Flow source into node 0, the two-port from node 0 to node 1, load
    /// admittance `yl` from node 1 to ground.
    fn solve_loaded(stamp: impl Fn(&mut Mna), yl: C64) -> Vec<C64> {
        let mut m = Mna::new(2, 2);
        m.flow_source(Some(0), None, ONE);
        m.admittance(Some(1), None, yl);
        stamp(&mut m);
        crate::linalg::solve(m.a, &m.rhs).unwrap()
    }

    #[test]
    fn admittance_form_stamps_the_same_two_port() {
        // A line between series end masses, three copies in parallel, where
        // both forms are accurate: same node potentials and branch flows.
        let zc = C64::new(2e6, -1e6);
        let end = Transfer::from(abcd_series(C64::new(0.0, 3e5)));
        let t = (end * Transfer::line(C64::new(4.0, 5.0), zc) * end).parallel(3.0);
        let [a, b, c, d] = t.abcd();
        assert!((a * d - b * c - ONE).norm() < 1e-9);
        for (y, e) in t
            .admittance()
            .iter()
            .zip([d / b, -b.inv(), -b.inv(), a / b])
        {
            assert!(rel(*y, e) < 1e-13, "{y} vs {e}");
        }
        let ports = ((Some(0), None), (Some(1), None), (2, 3));
        let yl = C64::new(1e-7, 3e-7);
        let x_abcd = solve_loaded(|m| m.two_port_abcd(ports.0, ports.1, ports.2, t.abcd()), yl);
        let x_y = solve_loaded(
            |m| m.two_port_admittance(ports.0, ports.1, ports.2, t.admittance()),
            yl,
        );
        for (p, q) in x_abcd.iter().zip(&x_y) {
            assert!(rel(*q, *p) < 1e-12, "{p} vs {q}");
        }
    }

    #[test]
    fn two_port_keeps_the_transmission_form_for_a_half_wave_line() {
        // E14: a lossless half-wave line has B = C = 0, where the admittance
        // form does not exist; it repeats its load, Z_in = 1/y_L.
        let t = Transfer::line(C64::new(0.0, PI), C64::new(4e6, 0.0));
        let yl = C64::new(1e-7, 3e-7);
        let x = solve_loaded(
            |m| m.two_port((Some(0), None), (Some(1), None), (2, 3), &t),
            yl,
        );
        assert!((x[0] * yl - ONE).norm() < 1e-12, "{}", x[0] * yl);
    }
}
