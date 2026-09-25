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

    /// General two-port in transmission (ABCD) form:
    /// [V1; I1] = [[A, B], [C, D]] · [V2; I2], with I1 entering port 1 at
    /// `p1` and I2 leaving port 2 at `p2`. Branch unknowns `br1` (I1) and
    /// `br2` (I2) make the stamp valid even where B or C vanish.
    ///
    /// A long, lossy line has |A|, |B|, |C|, |D| ≈ e^{|Γl|}/2, and the
    /// transmission rows then lose the port-1 relation to rounding (for a
    /// thermoviscous slit this happens near |Γl| ≈ 39, e.g. a 4 µm gap,
    /// 15 mm deep, at 12 kHz). There the rows are written in admittance
    /// form instead, I1 = (D/B)·V1 − (1/B)·V2 and I2 = (1/B)·V1 − (A/B)·V2,
    /// whose coefficients stay of order 1/Z_c. That form needs B far from
    /// zero, which |A| or |D| > 1e3 together with |B·C| > |A·D|/2
    /// guarantees, and a reciprocal two-port (AD − BC = 1, true of every
    /// transfer-matrix element of the library; the determinant cannot be
    /// computed from entries this large). The half-wave case, where B
    /// vanishes, keeps the transmission rows (erratum E14).
    pub fn two_port_abcd(
        &mut self,
        (p1, n1): (Unknown, Unknown),
        (p2, n2): (Unknown, Unknown),
        (br1, br2): (usize, usize),
        [a, b, c, d]: [C64; 4],
    ) {
        let (i1, i2) = (Some(br1), Some(br2));
        // KCL: I1 leaves p1 into the element; I2 leaves the element into p2.
        self.add(p1, i1, ONE);
        self.add(n1, i1, -ONE);
        self.add(p2, i2, -ONE);
        self.add(n2, i2, ONE);
        // Ratios, not products: the entries themselves may approach 1e300.
        let long_lossy =
            a.norm().max(d.norm()) > 1e3 && (b.norm() / a.norm()) * (c.norm() / d.norm()) > 0.5;
        if long_lossy {
            // `fdiv` scales before dividing: |B|² alone may overflow.
            let (y11, b_inv, y22) = (d.fdiv(b), ONE.fdiv(b), -a.fdiv(b));
            let (y12, y21) = (-b_inv, b_inv);
            // I1 − Y11·V1 − Y12·V2 = 0
            self.add(i1, i1, ONE);
            self.add(i1, p1, -y11);
            self.add(i1, n1, y11);
            self.add(i1, p2, -y12);
            self.add(i1, n2, y12);
            // I2 − Y21·V1 − Y22·V2 = 0
            self.add(i2, i2, ONE);
            self.add(i2, p1, -y21);
            self.add(i2, n1, y21);
            self.add(i2, p2, -y22);
            self.add(i2, n2, y22);
            return;
        }
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
