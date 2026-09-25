//! Network property tests on pseudo-random netlists (spec Section 17,
//! "Property tests on every solve", with errata E4, E16 and E17).
//!
//! A small deterministic generator (xorshift64*, no dependencies) builds
//! valid passive netlists over all three domains: a voice-coil driver with
//! optional electrical extras, a one- or two-mass mechanical system, one or
//! two pistons, and a random acoustic network of cavities (one- and
//! two-node), ducts, slits, radiation and lumped R/M/C elements. Every
//! network is then checked for
//!
//! * Tellegen power balance through `Circuit::power_absorbed`: the engine's
//!   own solution to the spec's 1e-9 of the total real power, and the
//!   solution after one step of iterative refinement (`refined_solve`) to
//!   1e-10, each with a round-off floor proportional to the apparent power;
//! * passivity: `Re(Zin) ≥ 0` at the only source (E17's general form), every
//!   passive element absorbing non-negative power, couplers absorbing none,
//!   and `Re(Zin) ≥ Re` where a single coil is driven directly;
//! * reciprocity of passive acoustic sub-networks, `p_i/U_j = p_j/U_i`, and
//!   the (anti-)reciprocity of the couplers, to the spec's 1e-9 relative on
//!   refined solutions (the engine's LU is only norm-wise accurate);
//! * `det T = 1` for every reciprocal two-port and `−1` for the piston
//!   gyrator, measured through the solver;
//! * continuity between level 0 and level 1 within 0.1 dB below an
//!   electrical length of 0.17 (E4): k·d for two-node cavities, |Γ|·l for
//!   ducts, the bound scaled at each node by its first-order sensitivity to
//!   the level-dependent elements, plus the ducts' neglected compressibility
//!   and the far-wall ratio at far faces;
//!
//! plus malformed netlists, whose errors must name the offending element,
//! key or node, and the shipped examples. Failures print the seed and the
//! netlist. Every tolerance was checked against 100 times as many seeds on
//! other streams.

use acoustilab::elements::TwoPort;
use acoustilab::linalg;
use acoustilab::mna::Mna;
use acoustilab::thermoviscous::{propagation, Section};
use acoustilab::{AirState, Circuit, C64};
use serde_json::{json, Value};
use std::f64::consts::{LOG10_E, PI};

// ----- Deterministic PRNG ----------------------------------------------------

/// xorshift64* (Marsaglia's xorshift with Vigna's multiplier).
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        // Scramble so that small consecutive seeds give unrelated streams.
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03);
        if r.0 == 0 {
            r.0 = 1;
        }
        for _ in 0..4 {
            r.next_u64();
        }
        r
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }

    /// Log-uniform in [lo, hi].
    fn log_range(&mut self, lo: f64, hi: f64) -> f64 {
        lo * (hi / lo).powf(self.unit())
    }

    fn below(&mut self, n: usize) -> usize {
        ((self.unit() * n as f64) as usize).min(n - 1)
    }

    fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

// ----- Random network generator ------------------------------------------------

/// Spec analytical-check air (spec_reference): ρc² for compliances.
const K0: f64 = 1.204 * 343.0 * 343.0;

#[derive(Default)]
struct Net {
    nodes: Vec<(String, &'static str)>,
    elements: Vec<Value>,
    acoustic: Vec<String>,
    /// Level-dependent ducts: section and length including end corrections.
    ducts: Vec<(Section, f64)>,
    /// Depths of the two-node cavities.
    depths: Vec<f64>,
    /// Far-face nodes of the two-node cavities.
    far_faces: Vec<String>,
    /// (driver face, far face) of each two-node cavity.
    face_pairs: Vec<(String, String)>,
    /// Re of the coil when the electrical source drives it directly with
    /// nothing else on the electrical side.
    lone_coil: Option<f64>,
    /// Keep two-node cavities closed at their far face. Level 0 joins the
    /// faces with an ideal short, dropping the depth line's air inertance
    /// ρ·d/S; an opening at the far face whose own inertance is comparable
    /// (a radiating hole, a short wide vent) then makes the levels differ at
    /// *every* frequency, which the closed-duct kL metric does not cover.
    /// The continuity test therefore uses closed far faces.
    closed_far_faces: bool,
    count: usize,
}

impl Net {
    fn node(&mut self, prefix: &str, domain: &'static str) -> String {
        self.count += 1;
        let id = format!("{prefix}{}", self.count);
        self.nodes.push((id.clone(), domain));
        if domain == "acoustic" {
            self.acoustic.push(id.clone());
        }
        id
    }

    fn push(&mut self, mut e: Value) -> String {
        self.count += 1;
        let id = format!("{}{}", e["type"].as_str().unwrap(), self.count);
        e["id"] = json!(id);
        self.elements.push(e);
        id
    }

    fn document(&self, level: u8, air: &str, probes: Vec<Value>) -> Value {
        json!({
            "schema": "acoustilab-netlist/0.1",
            "air": {"preset": air},
            "sweep": {"frequencies_Hz": [1000.0]},
            "level": level,
            "nodes": self.nodes.iter().map(|(id, d)| json!({"id": id, "domain": d}))
                .collect::<Vec<_>>(),
            "elements": self.elements,
            "probes": probes,
        })
    }
}

/// A cavity from `node` to ambient with a random geometry; returns its
/// element JSON (without id). Volumes of 10–100 cm³ keep the air volume of
/// the small ducts below (≤ 0.06 cm³) under 0.6 % of any compliance, so
/// the lumped duct's neglected compressibility stays inside the 0.1 dB
/// continuity bound (as it does for real vents and leaks).
fn cavity(rng: &mut Rng, nodes: Value, two_node: bool) -> (Value, f64) {
    let v = rng.log_range(10.0, if two_node { 60.0 } else { 100.0 });
    let depth = rng.range(5.0, 30.0);
    let mut e = match rng.below(if two_node { 3 } else { 4 }) {
        0 => json!({"type": "cavity", "volume_cm3": v, "depth_mm": depth}),
        1 => {
            let r = (v * 1e3 / (PI * depth)).sqrt();
            json!({"type": "cavity", "radius_mm": r, "depth_mm": depth})
        }
        2 => {
            let area = v * 1e3 / depth;
            let aspect = rng.range(1.0, 2.0);
            let ly = (area / aspect).sqrt();
            json!({"type": "cavity", "lx_mm": aspect * ly, "ly_mm": ly, "lz_mm": depth})
        }
        _ => json!({"type": "cavity", "volume_cm3": v}),
    };
    e["nodes"] = nodes;
    if rng.chance(0.2) {
        e["wall_loss"] = json!(false);
    }
    if rng.chance(0.3) {
        e["surface_factor"] = json!(rng.range(1.0, 3.0));
    }
    if !two_node && rng.chance(0.1) {
        e["wall_area_cm2"] = json!(rng.range(20.0, 150.0));
    }
    (e, depth * 1e-3)
}

/// A random series element between `a` and `b` (b may be ambient).
fn series(rng: &mut Rng, net: &mut Net, a: &str, b: &str) {
    let ends = ["none", "flanged", "piston", "unflanged"];
    let e = match rng.below(6) {
        0 => {
            let r = rng.log_range(0.3, 1.5);
            let l = rng.range(0.5, 4.0);
            let section = Section::Circle { radius: r * 1e-3 };
            net.ducts.push((section, (l + 2.0 * 0.8488 * r) * 1e-3));
            json!({"type": "tube", "radius_mm": r, "length_mm": l,
                   "count": 1 + rng.below(2), "inlet": rng.pick(&ends), "outlet": rng.pick(&ends)})
        }
        1 => {
            let gap = rng.log_range(0.05, 0.3);
            let l = rng.range(1.0, 5.0);
            let width = rng.range((5.0 * gap).max(3.0), 20.0);
            let section = Section::Slit {
                gap: gap * 1e-3,
                width: width * 1e-3,
            };
            net.ducts.push((section, l * 1e-3));
            json!({"type": "slit", "gap_mm": gap, "width_mm": width, "length_mm": l,
                   "count": 1 + rng.below(2)})
        }
        2 => json!({"type": "acoustic_resistance", "R_Pa_s_per_m3": rng.log_range(1e5, 1e9)}),
        3 => json!({"type": "acoustic_inertance", "M_kg_per_m4": rng.log_range(50.0, 1e4)}),
        4 => json!({"type": "acoustic_impedance", "R_Pa_s_per_m3": rng.log_range(1e5, 1e9),
                    "M_kg_per_m4": rng.log_range(50.0, 1e4)}),
        _ => json!({"type": "radiation", "radius_mm": rng.range(0.5, 5.0),
                    "baffle": rng.pick(&["infinite", "free"]), "count": 1 + rng.below(2)}),
    };
    let mut e = e;
    e["nodes"] = json!([a, b]);
    net.push(e);
}

/// Grows a random acoustic network from the given seed nodes (which must
/// already exist): extra nodes, a shunt at every node, a spanning chain of
/// series elements, extra series links and leaks, and radiation.
fn acoustic_network(rng: &mut Rng, net: &mut Net, seeds: &[String]) {
    let mut nodes: Vec<String> = seeds.to_vec();
    if nodes.is_empty() {
        nodes.push(net.node("a", "acoustic"));
    }
    for _ in 0..rng.below(4) {
        let n = net.node("a", "acoustic");
        // Spanning chain: every new node hangs off an earlier one.
        let prev = rng.pick(&nodes).clone();
        series(rng, net, &prev, &n);
        nodes.push(n);
    }
    for n in nodes.clone() {
        match rng.below(5) {
            0 => {
                let far = net.node("a", "acoustic");
                let (e, depth) = cavity(rng, json!([n, far]), true);
                net.depths.push(depth);
                net.far_faces.push(far.clone());
                net.face_pairs.push((n.clone(), far.clone()));
                net.push(e);
                if !net.closed_far_faces {
                    if rng.chance(0.5) {
                        series(rng, net, &far, "ambient");
                    }
                    nodes.push(far);
                }
            }
            1 => {
                let v = rng.log_range(10.0, 100.0) * 1e-6;
                net.push(
                    json!({"type": "acoustic_compliance", "nodes": [n], "C_m3_per_Pa": v / K0}),
                );
            }
            _ => {
                let (e, _) = cavity(rng, json!([n]), false);
                net.push(e);
            }
        }
    }
    for _ in 0..rng.below(4) {
        let a = rng.pick(&nodes).clone();
        let b = if rng.chance(0.5) {
            "ambient".to_string()
        } else {
            rng.pick(&nodes).clone()
        };
        if a != b {
            series(rng, net, &a, &b);
        }
    }
}

/// Where the generated network is driven.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drive {
    /// Voltage source with random output impedance.
    Voltage,
    Current,
    Force,
    Flow,
    Pressure,
    /// Voltage source plus up to three more sources of other kinds.
    Many,
}

const SINGLE_DRIVES: [Drive; 5] = [
    Drive::Voltage,
    Drive::Current,
    Drive::Force,
    Drive::Flow,
    Drive::Pressure,
];

/// A full electro-mechano-acoustic network. The source element has id
/// "src" (and "src2".. for extra sources with `Drive::Many`).
fn driver_network(rng: &mut Rng, drive: Drive, closed_far_faces: bool) -> Net {
    let mut net = Net {
        closed_far_faces,
        ..Net::default()
    };
    let e_in = net.node("e", "electrical");
    let mut e_coil_in = e_in.clone();
    let mut lone = true;
    // Electrical extras: series resistance, Zobel, shunt inductor.
    if rng.chance(0.25) {
        let e_x = net.node("e", "electrical");
        net.push(
            json!({"type": "resistor", "nodes": [e_in, e_x], "R_ohm": rng.log_range(0.1, 50.0)}),
        );
        e_coil_in = e_x;
        lone = false;
    }
    if rng.chance(0.25) {
        let e_z = net.node("e", "electrical");
        net.push(
            json!({"type": "resistor", "nodes": [e_in, e_z], "R_ohm": rng.log_range(5.0, 300.0)}),
        );
        net.push(json!({"type": "capacitor", "nodes": [e_z], "C_uF": rng.log_range(0.1, 10.0)}));
        lone = false;
    }
    if rng.chance(0.1) {
        net.push(json!({"type": "inductor", "nodes": [e_in], "L_mH": rng.log_range(1.0, 100.0)}));
        lone = false;
    }
    let e_coil = net.node("e", "electrical");
    let re = rng.log_range(8.0, 300.0);
    let mut coil = json!({"type": "coil", "nodes": [e_coil_in, e_coil], "Re_ohm": re});
    if rng.chance(0.5) {
        coil["Le_uH"] = json!(rng.log_range(1.0, 100.0));
    }
    if rng.chance(0.3) {
        coil["L2_mH"] = json!(rng.log_range(0.01, 1.0));
        coil["R2_ohm"] = json!(rng.log_range(1.0, 100.0));
    }
    net.push(coil);
    // Mechanical: diaphragm, optionally a decoupled second mass and a
    // compliantly mounted basket carrying the motor's reaction.
    let m_dia = net.node("m", "mechanical");
    let frame = if rng.chance(0.2) {
        let m_b = net.node("m", "mechanical");
        net.push(json!({"type": "mass", "node": m_b, "M_g": rng.log_range(5.0, 50.0)}));
        net.push(json!({"type": "spring", "nodes": [m_b], "K_N_per_m": rng.log_range(1e3, 1e5)}));
        net.push(json!({"type": "damper", "nodes": [m_b, "frame"], "R_Ns_per_m": rng.log_range(0.1, 10.0)}));
        m_b
    } else {
        "frame".to_string()
    };
    net.push(
        json!({"type": "motor", "nodes": [e_coil, "gnd", m_dia, frame],
                    "Bl_Tm": rng.log_range(0.3, 3.0)}),
    );
    let rms = if rng.chance(0.2) {
        0.0
    } else {
        rng.log_range(0.01, 0.5)
    };
    net.push(
        json!({"type": "suspension", "node": m_dia, "Mms_g": rng.log_range(0.05, 1.0),
                    "Cms_mm_per_N": rng.log_range(0.2, 20.0), "Rms_Ns_per_m": rms}),
    );
    let a_front = net.node("a", "acoustic");
    let a_rear = if rng.chance(0.7) {
        net.node("a", "acoustic")
    } else {
        "ambient".to_string()
    };
    net.push(
        json!({"type": "piston", "nodes": [m_dia, frame, a_front, a_rear],
                    "Sd_cm2": rng.log_range(2.0, 20.0)}),
    );
    if rng.chance(0.4) {
        let m2 = net.node("m", "mechanical");
        net.push(json!({"type": "mass", "node": m2, "M_mg": rng.log_range(10.0, 500.0)}));
        net.push(
            json!({"type": "spring", "nodes": [m_dia, m2], "C_mm_per_N": rng.log_range(0.01, 1.0)}),
        );
        net.push(
            json!({"type": "damper", "nodes": [m_dia, m2], "R_Ns_per_m": rng.log_range(1e-3, 0.5)}),
        );
        net.push(
            json!({"type": "piston", "nodes": [m2, frame, a_front, a_rear],
                        "Sd_cm2": rng.log_range(0.5, 5.0)}),
        );
    }
    let mut seeds = vec![a_front];
    if a_rear != "ambient" {
        seeds.push(a_rear);
    }
    acoustic_network(rng, &mut net, &seeds);

    // With closed far faces no source may drive a far face either.
    let acoustic_node = |rng: &mut Rng, net: &Net| {
        let candidates: Vec<&String> = net
            .acoustic
            .iter()
            .filter(|n| !net.closed_far_faces || !net.far_faces.contains(n))
            .collect();
        rng.pick(&candidates).to_string()
    };
    let add_source = |rng: &mut Rng, net: &mut Net, kind: Drive, id: &str| {
        let mut e = match kind {
            Drive::Voltage | Drive::Many => {
                let zs = if rng.chance(0.5) {
                    0.0
                } else {
                    rng.range(0.0, 50.0)
                };
                json!({"type": "vsource", "nodes": [e_in], "V_V": 1.0, "Zs_ohm": zs})
            }
            Drive::Current => json!({"type": "isource", "nodes": [e_in], "I_mA": 10.0}),
            Drive::Force => json!({"type": "force_source", "nodes": [m_dia], "F_mN": 10.0}),
            Drive::Flow => {
                let a = acoustic_node(rng, net);
                let mut from = if rng.chance(0.5) {
                    "ambient".to_string()
                } else {
                    acoustic_node(rng, net)
                };
                // Level 0 joins a two-node cavity's faces with an ideal short:
                // a source across them would drive nothing, leaving every
                // power and potential at round-off level.
                if net
                    .face_pairs
                    .iter()
                    .any(|(x, y)| (*x == a && *y == from) || (*x == from && *y == a))
                {
                    from = "ambient".to_string();
                }
                let nodes = if from == a {
                    json!([a])
                } else {
                    json!([a, from])
                };
                json!({"type": "flow_source", "nodes": nodes, "U_cm3_per_s": 1.0})
            }
            Drive::Pressure => {
                let zs = if rng.chance(0.3) {
                    0.0
                } else {
                    rng.log_range(1e5, 1e8)
                };
                json!({"type": "pressure_source", "nodes": [acoustic_node(rng, net)], "p_Pa": 1.0,
                       "Zs_Pa_s_per_m3": zs})
            }
        };
        e["id"] = json!(id);
        net.elements.push(e);
    };
    add_source(rng, &mut net, drive, "src");
    if drive == Drive::Many {
        let extra = [Drive::Current, Drive::Force, Drive::Flow, Drive::Pressure];
        for (k, kind) in extra.iter().enumerate() {
            if rng.chance(0.6) {
                add_source(rng, &mut net, *kind, &format!("src{}", k + 2));
            }
        }
    }
    if matches!(drive, Drive::Voltage | Drive::Current | Drive::Many) {
        if lone {
            net.lone_coil = Some(re);
        }
    } else if rng.chance(0.5) {
        // Amplifier output impedance terminating the undriven coil.
        net.push(json!({"type": "resistor", "nodes": [e_in], "R_ohm": rng.log_range(0.1, 100.0)}));
    }
    net
}

/// A purely acoustic network (no source).
fn acoustic_only(rng: &mut Rng, closed_far_faces: bool) -> Net {
    let mut net = Net {
        closed_far_faces,
        ..Net::default()
    };
    acoustic_network(rng, &mut net, &[]);
    // Make sure there are at least two nodes to swap between.
    if net.acoustic.len() < 2 {
        let a0 = net.acoustic[0].clone();
        let n = net.node("a", "acoustic");
        series(rng, &mut net, &a0, &n);
        let (e, _) = cavity(rng, json!([n]), false);
        net.push(e);
    }
    net
}

fn frequencies(rng: &mut Rng, n: usize, lo: f64, hi: f64) -> Vec<f64> {
    (0..n).map(|_| rng.log_range(lo, hi)).collect()
}

fn circuit(doc: &Value, seed: u64) -> Circuit {
    Circuit::from_json(&doc.to_string())
        .unwrap_or_else(|e| panic!("seed {seed}: generated netlist rejected: {e}\n{doc:#}"))
}

fn solve(c: &Circuit, f: f64, seed: u64, doc: &Value) -> Vec<C64> {
    c.solve_at(f)
        .unwrap_or_else(|e| panic!("seed {seed}, f {f}: {e}\n{doc:#}"))
}

fn probe(c: &Circuit, x: &[C64], f: f64, id: &str) -> C64 {
    let p = c.probes.iter().find(|p| p.id == id).unwrap();
    c.probe_value(p, f, x).unwrap()
}

/// `Circuit::solve_at` followed by one step of iterative refinement on the
/// same assembled system (residual and correction in working precision).
/// Returns (engine solution, refined solution).
///
/// The engine's equilibrated LU is norm-wise accurate, but a potential far
/// below the largest in the solve can lose most of its own digits: on one
/// generated acoustic network a pressure 1e-5 of the largest came out
/// 2.3e-6 off in relative terms, where one refinement step (or an
/// unequilibrated partial-pivoting LU) gives 1e-15. Reciprocity is a
/// property of the stamped network, so it is checked on refined solutions
/// to the spec's 1e-9 relative.
fn refined_solve(c: &Circuit, f: f64, seed: u64, doc: &Value) -> (Vec<C64>, Vec<C64>) {
    let x = solve(c, f, seed, doc);
    let cx = c.cx(f);
    let mut mna = Mna::new(c.nodes.len(), c.dim - c.nodes.len());
    for (i, e) in c.elements.iter().enumerate() {
        e.stamp(&cx, &mut mna, &c.branches(i));
    }
    let ax = mna.a.mul_vec(&x);
    let residual: Vec<C64> = mna.rhs.iter().zip(&ax).map(|(b, y)| b - y).collect();
    let dx = linalg::solve(mna.a, &residual).expect("the system solved once already");
    let refined = x.iter().zip(&dx).map(|(x, d)| x + d).collect();
    (x, refined)
}

// ----- Energy balance and passivity ------------------------------------------

/// Σ over all element ports of |V|·|I| (apparent power).
fn apparent_power(c: &Circuit, f: f64, x: &[C64]) -> f64 {
    let cx = c.cx(f);
    c.elements
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let br = c.branches(i);
            (0..e.port_count())
                .map(|k| {
                    let v = e.port_potential(x, k).unwrap();
                    let fl = e.port_flow(&cx, x, &br, k).unwrap();
                    v.norm() * fl.norm()
                })
                .sum::<f64>()
        })
        .sum()
}

/// (relative, apparent-power floor) of the power balance of the engine's
/// own solution: the spec's 1e-9.
const ENGINE_BALANCE: (f64, f64) = (1e-9, 1e-10);
/// The same for the refined solution: 1e-10.
const REFINED_BALANCE: (f64, f64) = (1e-10, 1e-11);

/// Checks Tellegen's theorem, |Σ P| ≤ rel·Σ|P| + floor·Σ|V|·|I|, and
/// returns (per-element powers, tolerance).
///
/// The imbalance is the node potentials weighted by the solution's KCL
/// residuals, so its round-off level scales with the apparent power (and
/// more where a port's potential is a small difference of large node
/// potentials). Callers check the engine's own solution against the spec's
/// 1e-9 with a floor of 1e-10 of the apparent power, and the refined
/// solution (`refined_solve`) against 1e-10 with a floor of 1e-11. Worst
/// seen over 20 000 networks: the engine's solution 2.2e-10 of the real
/// and 5.6e-11 of the apparent power, the refined one 7.3e-13 of the
/// apparent power (in a network whose apparent power is 2e8 times its real
/// power).
fn check_tellegen(
    c: &Circuit,
    f: f64,
    x: &[C64],
    (rel, floor): (f64, f64),
    ctx: &dyn Fn() -> String,
) -> (Vec<f64>, f64) {
    let p = c.power_absorbed(f, x);
    let powers: Vec<f64> = p
        .iter()
        .map(|(id, v)| v.unwrap_or_else(|| panic!("element {id} exposes no ports; {}", ctx())))
        .collect();
    let sum: f64 = powers.iter().sum();
    let real: f64 = powers.iter().map(|v| v.abs()).sum();
    let tol = rel * real + floor * apparent_power(c, f, x);
    assert!(
        sum.abs() <= tol,
        "Tellegen: Σ P = {sum:e} (tolerance {tol:e}, rel {rel:e}) at {f} Hz: {p:?}\n{}",
        ctx()
    );
    (powers, tol)
}

#[test]
fn generator_is_deterministic() {
    let a =
        driver_network(&mut Rng::new(7), Drive::Many, false).document(1, "spec_reference", vec![]);
    let b =
        driver_network(&mut Rng::new(7), Drive::Many, false).document(1, "spec_reference", vec![]);
    let c =
        driver_network(&mut Rng::new(8), Drive::Many, false).document(1, "spec_reference", vec![]);
    assert_eq!(a, b);
    assert_ne!(a, c);
}

#[test]
fn tellegen_power_balance_on_random_networks() {
    for seed in 0..150u64 {
        let mut rng = Rng::new(seed);
        let net = driver_network(&mut rng, Drive::Many, false);
        let level = (seed % 2) as u8;
        let air = if seed % 3 == 0 {
            "standard_23C"
        } else {
            "spec_reference"
        };
        let doc = net.document(level, air, vec![]);
        let c = circuit(&doc, seed);
        for f in frequencies(&mut rng, 6, 10.0, 40_000.0) {
            let (raw, x) = refined_solve(&c, f, seed, &doc);
            let ctx = || format!("seed {seed}\n{doc:#}");
            check_tellegen(&c, f, &raw, ENGINE_BALANCE, &ctx);
            let (powers, _) = check_tellegen(&c, f, &x, REFINED_BALANCE, &ctx);
            // Not a trivial balance: something (at least the coil) dissipates.
            assert!(powers.iter().filter(|p| **p > 0.0).count() > 0, "{}", ctx());
        }
    }
}

#[test]
fn passivity_at_the_source_and_per_element() {
    let mut driven = [0usize; 5];
    for seed in 0..200u64 {
        let mut rng = Rng::new(1000 + seed);
        let kind = *rng.pick(&SINGLE_DRIVES);
        driven[SINGLE_DRIVES.iter().position(|d| *d == kind).unwrap()] += 1;
        let net = driver_network(&mut rng, kind, false);
        let level = (seed % 2) as u8;
        let doc = net.document(
            level,
            "spec_reference",
            vec![
                json!({"id": "zin", "quantity": "impedance", "element": "src"}),
                json!({"id": "isrc", "quantity": "flow", "element": "src"}),
            ],
        );
        let c = circuit(&doc, seed);
        for f in frequencies(&mut rng, 6, 10.0, 40_000.0) {
            let (raw, x) = refined_solve(&c, f, seed, &doc);
            let ctx = || format!("seed {}, {kind:?}, {f} Hz\n{doc:#}", 1000 + seed);
            check_tellegen(&c, f, &raw, ENGINE_BALANCE, &ctx);
            let (powers, tol) = check_tellegen(&c, f, &x, REFINED_BALANCE, &ctx);
            // E17: the general condition is Re(Zin) ≥ 0 at the source. The
            // source delivers Re(Zin)·|I|², so the sign of Re(Zin) is only
            // resolved down to the power balance's round-off floor over |I|²;
            // a bound relative to |Zin| alone fails spuriously where Zin is
            // itself round-off (a source across an ideal short).
            let zin = probe(&c, &x, f, "zin");
            let i_sq = probe(&c, &x, f, "isrc").norm_sqr();
            assert!(
                zin.re >= -(1e-12 * zin.norm()).max(tol / i_sq),
                "Re(Zin) = {} ; {}",
                zin.re,
                ctx()
            );
            // Only for a single moving coil driven directly: Re(Zin) ≥ Re.
            if let Some(re) = net.lone_coil {
                if kind == Drive::Voltage || kind == Drive::Current {
                    assert!(
                        zin.re >= re * (1.0 - 1e-12),
                        "Re(Zin) {} < Re {re}; {}",
                        zin.re,
                        ctx()
                    );
                }
            }
            // E16: passivity on port powers, not on element values.
            for (e, p) in c.elements.iter().zip(&powers) {
                match e.type_name() {
                    _ if e.is_source() => {
                        assert!(*p <= tol, "source {} absorbs {p}; {}", e.id(), ctx())
                    }
                    "motor" | "piston" => {
                        assert!(p.abs() <= tol, "coupler {} absorbs {p}; {}", e.id(), ctx())
                    }
                    _ => assert!(*p >= -tol, "{} generates {p}; {}", e.id(), ctx()),
                }
            }
        }
    }
    assert!(driven.iter().all(|&n| n > 10), "drive coverage {driven:?}");
}

// ----- Reciprocity -------------------------------------------------------------

#[test]
fn passive_acoustic_networks_are_reciprocal() {
    for seed in 0..120u64 {
        let mut rng = Rng::new(5000 + seed);
        let net = acoustic_only(&mut rng, false);
        let i = rng.below(net.acoustic.len());
        let j = (i + 1 + rng.below(net.acoustic.len() - 1)) % net.acoustic.len();
        let (ni, nj) = (net.acoustic[i].clone(), net.acoustic[j].clone());
        let level = (seed % 2) as u8;
        let rig = |at: &str| {
            let mut n = Net {
                nodes: net.nodes.clone(),
                elements: net.elements.clone(),
                ..Net::default()
            };
            n.elements
                .push(json!({"id": "u", "type": "flow_source", "node": at}));
            n.document(
                level,
                "standard_23C",
                vec![
                    json!({"id": "pi", "quantity": "pressure", "node": ni}),
                    json!({"id": "pj", "quantity": "pressure", "node": nj}),
                ],
            )
        };
        let (doc_j, doc_i) = (rig(&nj), rig(&ni));
        let (cj, ci) = (circuit(&doc_j, seed), circuit(&doc_i, seed));
        for f in frequencies(&mut rng, 5, 10.0, 40_000.0) {
            let (raw_j, xj) = refined_solve(&cj, f, seed, &doc_j);
            let (raw_i, xi) = refined_solve(&ci, f, seed, &doc_i);
            // U = 1 cm³/s (default) in both: compare p_i|U_j with p_j|U_i,
            // to the spec's 1e-9 relative on the refined solutions (see
            // `refined_solve`), however far the transfer is below the
            // driving-point level.
            let z_ij = probe(&cj, &xj, f, "pi");
            let z_ji = probe(&ci, &xi, f, "pj");
            assert!(
                (z_ij - z_ji).norm() <= 1e-9 * z_ij.norm(),
                "seed {}, L{level}, {f} Hz: {z_ij} vs {z_ji}\n{doc_j:#}",
                5000 + seed
            );
            // The engine's own solution is norm-wise accurate: every pressure
            // within 1e-9 of the largest (worst seen over 20 000 networks:
            // 1.1e-10).
            for (c, raw, x) in [(&cj, &raw_j, &xj), (&ci, &raw_i, &xi)] {
                let n = c.nodes.len();
                let largest = x[..n].iter().map(|p| p.norm()).fold(0.0, f64::max);
                let err = (0..n).map(|k| (raw[k] - x[k]).norm()).fold(0.0, f64::max);
                assert!(
                    err <= 1e-9 * largest,
                    "seed {}, L{level}, {f} Hz: LU error {err:e} of {largest:e}\n{doc_j:#}",
                    5000 + seed
                );
            }
        }
    }
}

#[test]
fn motor_is_reciprocal_and_piston_anti_reciprocal() {
    // Across/through: a transformer keeps V_e/F_m = v_m/I_e; a gyrator
    // flips the sign, p/F = −v/U; the two in cascade give p/I = −V/U.
    for seed in 0..40u64 {
        let mut rng = Rng::new(9000 + seed);
        // Undriven network (a terminating resistor on the coil input).
        let mut net = driver_network(&mut rng, Drive::Force, false);
        net.elements.retain(|e| e["id"] != "src");
        if !net
            .elements
            .iter()
            .any(|e| e["type"] == "resistor" && e["nodes"].as_array().unwrap().len() == 1)
        {
            net.push(json!({"type": "resistor", "nodes": [net.nodes[0].0], "R_ohm": 10.0}));
        }
        let e_in = net.nodes[0].0.clone();
        let m_dia = net
            .elements
            .iter()
            .find(|e| e["type"] == "suspension")
            .unwrap()["node"]
            .as_str()
            .unwrap()
            .to_string();
        let a_front = net.elements.iter().find(|e| e["type"] == "piston").unwrap()["nodes"][2]
            .as_str()
            .unwrap()
            .to_string();
        let level = (seed % 2) as u8;
        let rig = |src: Value| {
            let mut n = Net {
                nodes: net.nodes.clone(),
                elements: net.elements.clone(),
                ..Net::default()
            };
            let mut s = src;
            s["id"] = json!("drive");
            n.elements.push(s);
            n.document(
                level,
                "spec_reference",
                vec![
                    json!({"id": "v", "quantity": "voltage", "node": e_in}),
                    json!({"id": "vel", "quantity": "velocity", "node": m_dia}),
                    json!({"id": "p", "quantity": "pressure", "node": a_front}),
                ],
            )
        };
        let docs = [
            rig(json!({"type": "isource", "node": e_in, "I_A": 1.0})),
            rig(json!({"type": "force_source", "node": m_dia, "F_N": 1.0})),
            rig(json!({"type": "flow_source", "node": a_front, "U_m3_per_s": 1.0})),
        ];
        let cs: Vec<Circuit> = docs.iter().map(|d| circuit(d, seed)).collect();
        for f in frequencies(&mut rng, 4, 10.0, 20_000.0) {
            // Per drive, the probes [V, v, p] of the refined solution (see
            // `refined_solve`: a transfer that is a small difference of large
            // potentials, e.g. across a piston whose front and rear a vent
            // short-circuits, otherwise carries the LU's norm-wise error).
            let r: Vec<Vec<C64>> = cs
                .iter()
                .zip(&docs)
                .map(|(c, d)| {
                    let (_, x) = refined_solve(c, f, seed, d);
                    ["v", "vel", "p"]
                        .iter()
                        .map(|id| probe(c, &x, f, id))
                        .collect()
                })
                .collect();
            let ctx = || format!("seed {}, {f} Hz\n{:#}", 9000 + seed, docs[0]);
            // Transfer (i → j) read in drive i against its reciprocal (j → i)
            // read in drive j, to 1e-9 relative. A sign or stamping error
            // shows as an O(1) difference.
            let check = |what: &str, (i, j): (usize, usize), sign: f64| {
                let (a, b) = (r[i][j], sign * r[j][i]);
                assert!(
                    (a - b).norm() <= 1e-9 * a.norm().max(b.norm()),
                    "{what}: {a} vs {b}; {}",
                    ctx()
                );
            };
            // velocity per current == voltage per force (motor, reciprocal)
            check("motor", (0, 1), 1.0);
            // pressure per force == −velocity per volume velocity (piston)
            check("piston", (1, 2), -1.0);
            // pressure per current == −voltage per volume velocity
            check("chain", (0, 2), -1.0);
        }
    }
}

// ----- Two-port determinants ---------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum Load {
    /// Port 2 tied to the reference.
    Short,
    /// Port 2 node connected to nothing else.
    Open,
    /// Resistive load of twice the driving-point impedance measured with
    /// the first load (or 1 in SI units when that is zero). Only valid as
    /// the second load.
    Matched,
}

/// Measures the ABCD matrix of `dut` (JSON without id and nodes) through
/// the solver, with a flow-type source at port 1 and two port-2 loads.
/// Port terminals: [p1, (n1), p2, (n2)] with negative terminals grounded.
fn measure_abcd(
    dut: &Value,
    domains: (&str, &str),
    level: u8,
    f: f64,
    loads: [Load; 2],
) -> [C64; 4] {
    let four = matches!(dut["type"].as_str(), Some("motor") | Some("piston"));
    let source = |d: &str| match d {
        "electrical" => json!({"id": "s", "type": "isource", "node": "p1"}),
        "mechanical" => json!({"id": "s", "type": "force_source", "node": "p1"}),
        _ => json!({"id": "s", "type": "flow_source", "node": "p1"}),
    };
    let resistor = |d: &str, r: f64| match d {
        "electrical" => json!({"id": "load", "type": "resistor", "node": "p2", "R_ohm": r}),
        "mechanical" => json!({"id": "load", "type": "damper", "node": "p2", "R_Ns_per_m": r}),
        _ => json!({"id": "load", "type": "acoustic_resistance", "node": "p2", "R_Pa_s_per_m3": r}),
    };
    let run = |load: Load, r_match: f64| {
        let mut e = dut.clone();
        e["id"] = json!("dut");
        let t2 = if matches!(load, Load::Short) {
            "gnd"
        } else {
            "p2"
        };
        e["nodes"] = if four {
            json!(["p1", "gnd", t2, "gnd"])
        } else {
            json!(["p1", t2])
        };
        let mut nodes = vec![json!({"id": "p1", "domain": domains.0})];
        let mut elements = vec![source(domains.0), e];
        if !matches!(load, Load::Short) {
            nodes.push(json!({"id": "p2", "domain": domains.1}));
        }
        if matches!(load, Load::Matched) {
            elements.push(resistor(domains.1, r_match));
        }
        let doc = json!({
            "air": {"preset": "spec_reference"}, "level": level, "nodes": nodes,
            "elements": elements,
            "probes": [
                {"id": "v1", "quantity": "port_potential", "element": "dut", "port": 0},
                {"id": "i1", "quantity": "flow", "element": "dut", "port": 0},
                {"id": "v2", "quantity": "port_potential", "element": "dut", "port": 1},
                {"id": "i2", "quantity": "flow", "element": "dut", "port": 1},
                {"id": "n1", "quantity": "potential", "node": "p1"},
            ],
        });
        let c = Circuit::from_json(&doc.to_string()).unwrap_or_else(|e| panic!("{e}\n{doc:#}"));
        let x = c.solve_at(f).unwrap_or_else(|e| panic!("{e}\n{doc:#}"));
        let g = |id: &str| probe(&c, &x, f, id);
        let port2 = c.probes.iter().find(|p| p.id == "i2").unwrap();
        match c.probe_value(port2, f, &x) {
            // I2 is the flow leaving port 2 towards the load.
            Ok(i2_in) => (g("v1"), g("i1"), g("v2"), -i2_in),
            // A series one-port (a duct at L0) between p1 and p2: its port
            // is the difference of the two ground-referenced ports, and the
            // flow entering at p1 leaves at p2.
            Err(_) => {
                let v1 = g("n1");
                (v1, g("i1"), v1 - g("v1"), g("i1"))
            }
        }
    };
    let a = run(loads[0], 1.0);
    let z = (a.0 / a.1).norm();
    let b = run(loads[1], if z > 0.0 { 2.0 * z } else { 1.0 });
    // [V1; I1] = [[A, B], [C, D]]·[V2; I2] for both loads: two 2×2 solves.
    let det = a.2 * b.3 - a.3 * b.2;
    let solve2 = |ya: C64, yb: C64| ((ya * b.3 - yb * a.3) / det, (yb * a.2 - ya * b.2) / det);
    let (aa, bb) = solve2(a.0, b.0);
    let (cc, dd) = solve2(a.1, b.1);
    [aa, bb, cc, dd]
}

fn random_two_port(rng: &mut Rng, kind: usize) -> (Value, (&'static str, &'static str), f64) {
    let ends = ["none", "flanged", "piston", "unflanged"];
    match kind {
        0 => (
            json!({"type": "tube", "radius_mm": rng.log_range(0.2, 5.0),
                   "length_mm": rng.log_range(0.5, 100.0), "count": 1 + rng.below(3),
                   "inlet": rng.pick(&ends), "outlet": rng.pick(&ends)}),
            ("acoustic", "acoustic"),
            1.0,
        ),
        1 => {
            let gap = rng.log_range(0.05, 1.0);
            (
                json!({"type": "slit", "gap_mm": gap, "width_mm": rng.range(5.0 * gap, 40.0),
                       "length_mm": rng.log_range(1.0, 50.0), "count": 1 + rng.below(2)}),
                ("acoustic", "acoustic"),
                1.0,
            )
        }
        2 => {
            let mut e = json!({"type": "cavity", "radius_mm": rng.range(5.0, 25.0),
                               "depth_mm": rng.range(5.0, 80.0)});
            if rng.chance(0.3) {
                e["wall_loss"] = json!(false);
            }
            if rng.chance(0.3) {
                e["surface_factor"] = json!(rng.range(1.0, 5.0));
            }
            (e, ("acoustic", "acoustic"), 1.0)
        }
        3 => (
            json!({"type": "motor", "Bl_Tm": rng.log_range(0.3, 5.0)}),
            ("electrical", "mechanical"),
            1.0,
        ),
        _ => (
            json!({"type": "piston", "Sd_cm2": rng.log_range(0.5, 50.0)}),
            ("mechanical", "acoustic"),
            -1.0,
        ),
    }
}

#[test]
fn two_port_determinants() {
    let names = ["tube", "slit", "cavity", "motor", "piston"];
    for (kind, name) in names.iter().enumerate() {
        for seed in 0..30u64 {
            let mut rng = Rng::new(20_000 + 100 * kind as u64 + seed);
            let (dut, domains, expect) = random_two_port(&mut rng, kind);
            for level in [0u8, 1] {
                // Open circuits are singular for series one-ports (L0 ducts)
                // and for the transformer, a short for the gyrator (it turns
                // the acoustic short into a mechanical open circuit); use a
                // resistive load there.
                let loads = match (kind, level) {
                    (0 | 1, 0) | (3, _) => [Load::Short, Load::Matched],
                    (4, _) => [Load::Open, Load::Matched],
                    _ => [Load::Short, Load::Open],
                };
                for f in frequencies(&mut rng, 3, 10.0, 40_000.0) {
                    let [a, b, c, d] = measure_abcd(&dut, domains, level, f, loads);
                    let det = a * d - b * c;
                    let scale = (a * d).norm().max((b * c).norm()).max(1.0);
                    assert!(
                        (det - expect).norm() <= 1e-9 * scale,
                        "{} L{level} at {f} Hz: det = {det}; {dut}",
                        name
                    );
                    // Symmetric reciprocal elements have A = D.
                    if kind <= 2 {
                        assert!(
                            (a - d).norm() <= 1e-9 * a.norm().max(1.0),
                            "{dut}: A {a} D {d}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn measured_abcd_matches_the_stamped_transfer_matrix() {
    // Cross-check the measurement against the element's own ABCD closure.
    for seed in 0..40u64 {
        let mut rng = Rng::new(30_000 + seed);
        let (dut, domains, _) = random_two_port(&mut rng, (seed % 2) as usize);
        let mut e = dut.clone();
        e["id"] = json!("dut");
        e["nodes"] = json!(["a", "b"]);
        let doc = json!({"air": {"preset": "spec_reference"}, "level": 1,
                         "nodes": [{"id": "a", "domain": "acoustic"}, {"id": "b", "domain": "acoustic"}],
                         "elements": [e]});
        let c = Circuit::from_json(&doc.to_string()).unwrap();
        let tp = c.elements[0]
            .as_any()
            .downcast_ref::<TwoPort>()
            .expect("duct is a TwoPort at L1");
        for f in frequencies(&mut rng, 3, 10.0, 40_000.0) {
            let stamped = (tp.abcd)(&c.cx(f));
            let measured = measure_abcd(&dut, domains, 1, f, [Load::Short, Load::Open]);
            for (m, s) in measured.iter().zip(&stamped) {
                assert!(
                    (m - s).norm() <= 1e-9 * s.norm(),
                    "{dut} at {f}: {measured:?} vs {stamped:?}"
                );
            }
        }
    }
}

// ----- Continuity between fidelity levels (E4) ---------------------------------

/// The largest level-dependent "electrical length" at `f`: k·d for the
/// two-node cavities and |Γ|·l for the ducts. For narrow ducts |Γ| exceeds
/// k = ω/c severalfold (the viscous effective density), so the lossless
/// k·l would understate how far L0 and L1 differ.
fn electrical_length(net: &Net, air: &AirState, f: f64) -> f64 {
    let omega = 2.0 * PI * f;
    let cav = net.depths.iter().map(|d| omega / air.c * d);
    let ducts = net
        .ducts
        .iter()
        .map(|(s, l)| propagation(s, air, omega).0.norm() * l);
    cav.chain(ducts).fold(0.0, f64::max)
}

/// For each level-dependent element of a netlist (ducts and two-node
/// cavities), a copy of the netlist in which only that element's impedance
/// level is scaled by `factor`: a duct's line length (its end corrections
/// are the same lumped inertance at both levels), or a two-node cavity's
/// volume.
fn level_dependent_perturbations(doc: &Value, factor: f64) -> Vec<Value> {
    let elements = doc["elements"].as_array().unwrap();
    (0..elements.len())
        .filter_map(|k| {
            let e = &elements[k];
            let two_node = e["nodes"].as_array().is_some_and(|n| n.len() == 2);
            let (key, by) = match e["type"].as_str().unwrap() {
                "tube" | "slit" => ("length_mm", factor),
                "cavity" if two_node && e.get("volume_cm3").is_some() => ("volume_cm3", factor),
                "cavity" if two_node && e.get("radius_mm").is_some() => {
                    ("radius_mm", factor.sqrt())
                }
                "cavity" if two_node => ("lx_mm", factor),
                _ => return None,
            };
            let mut d = doc.clone();
            let v = d["elements"][k][key].as_f64().unwrap();
            d["elements"][k][key] = json!(v * by);
            Some(d)
        })
        .collect()
}

/// The netlist with every duct's air volume added as an isothermal
/// compliance V/P0, half at each end that is not ambient: the first-order
/// part of a level-1 duct that the level-0 series impedance neglects (its
/// compressibility), at its largest magnitude (the adiabatic value is
/// V/(γP0)).
fn with_duct_compressibility(doc: &Value, p0: f64) -> Value {
    let mut doc = doc.clone();
    let mut shunts = Vec::new();
    for e in doc["elements"].as_array().unwrap() {
        let num = |k: &str| e[k].as_f64().unwrap();
        let area_mm2 = match e["type"].as_str().unwrap() {
            "tube" => PI * num("radius_mm").powi(2),
            "slit" => num("gap_mm") * num("width_mm"),
            _ => continue,
        };
        let volume = area_mm2 * num("length_mm") * num("count") * 1e-9;
        for end in e["nodes"].as_array().unwrap() {
            if end != "ambient" {
                shunts.push(json!({"id": format!("shunt{}", shunts.len()),
                                   "type": "acoustic_compliance", "node": end,
                                   "C_m3_per_Pa": 0.5 * volume / p0}));
            }
        }
    }
    doc["elements"].as_array_mut().unwrap().extend(shunts);
    doc
}

#[test]
fn level_0_and_level_1_agree_within_0_1_db_below_kl_0_17() {
    // E4: below kL = 0.17 each level-dependent element's representation
    // differs between levels by at most about 1 % (0.1 dB is 1.16 %): the
    // depth line's input compliance by 1 − kd·cot(kd), a duct's series
    // impedance by |Γl|²/3 (`duct_levels_agree_below_gamma_l_0_17`). At a
    // node the level change is, to first order, Σ_e S_e·δ_e, where δ_e is
    // element e's complex relative change and S_e = ∂ln p/∂ln Z_e the node's
    // complex sensitivity to it. The pressure is analytic in Z_e, so |S_e|
    // can be measured with a real 1 % scaling of the element alone and
    // bounds the effect of a complex δ_e of the same size. The bound at each
    // node is therefore 0.1 dB · max(1, Σ_e |S_e|): the plain 0.1 dB away
    // from resonances, more where a resonance amplifies an element's error
    // (a 1 % compliance error moves a Q = 10 flank by ~1 dB) or several
    // elements add.
    //
    // Level 0 also neglects each duct's compressibility, which level 1
    // carries as the line's shunt compliance. The generator keeps duct
    // volumes under 0.6 % of any compliance, but at a node near an
    // anti-resonance (a pressure minimum between an inertance and a
    // compliance) a 0.05 % compliance change can move the level by 0.25 dB,
    // so its first-order effect, measured with the isothermal volume
    // compliance (`with_duct_compressibility`), is added to the bound.
    // Two-node cavities keep a closed far face (see `closed_far_faces`).
    let mut checked = 0usize;
    let mut amplified = 0usize;
    let mut worst: f64 = 0.0;
    for seed in 0..150u64 {
        let mut rng = Rng::new(40_000 + seed);
        let mut net = if seed % 2 == 0 {
            acoustic_only(&mut rng, true)
        } else {
            let kind = *rng.pick(&SINGLE_DRIVES);
            driver_network(&mut rng, kind, true)
        };
        if seed % 2 == 0 {
            let a = net.acoustic[0].clone();
            net.elements
                .push(json!({"id": "src", "type": "flow_source", "node": a}));
        }
        let probes: Vec<Value> = net
            .acoustic
            .iter()
            .map(|n| json!({"id": n, "quantity": "pressure", "node": n}))
            .collect();
        let air = if seed % 3 == 0 {
            "standard_23C"
        } else {
            "spec_reference"
        };
        let docs = [0u8, 1].map(|l| net.document(l, air, probes.clone()));
        let perturbed: Vec<(Circuit, Value)> = level_dependent_perturbations(&docs[0], 1.01)
            .into_iter()
            .map(|d| (circuit(&d, seed), d))
            .collect();
        let (c0, c1) = (circuit(&docs[0], seed), circuit(&docs[1], seed));
        let doc_v = with_duct_compressibility(&docs[0], c0.air.p0);
        let cv = circuit(&doc_v, seed);
        let freqs: Vec<f64> = frequencies(&mut rng, 12, 10.0, 20_000.0)
            .into_iter()
            .filter(|&f| electrical_length(&net, &c0.air, f) <= 0.17)
            .take(5)
            .collect();
        for f in freqs {
            let x0 = solve(&c0, f, seed, &docs[0]);
            let x1 = solve(&c1, f, seed, &docs[1]);
            let xv = solve(&cv, f, seed, &doc_v);
            let xp: Vec<Vec<C64>> = perturbed
                .iter()
                .map(|(c, d)| solve(c, f, seed, d))
                .collect();
            for n in &net.acoustic {
                let p0 = probe(&c0, &x0, f, n);
                let d = 20.0 * (probe(&c1, &x1, f, n) / p0).norm().log10();
                // Σ_e |S_e| at this node.
                let amp: f64 = perturbed
                    .iter()
                    .zip(&xp)
                    .map(|((c, _), x)| (probe(c, x, f, n) / p0 - 1.0).norm() / 0.01)
                    .sum::<f64>()
                    .max(1.0);
                // First-order effect of the ducts' compressibility, in dB
                // (its complex size, which bounds its effect on |p|).
                let volume = 20.0 * LOG10_E * (probe(&cv, &xv, f, n) / p0 - 1.0).norm();
                if amp > 1.5 || volume > 0.05 {
                    amplified += 1;
                }
                // A far face also carries the far-wall ratio 1/cos(kd) of
                // E3 (0.126 dB at kd = 0.17), which the input-compliance
                // metric does not bound: the driven face is held by the
                // drive, the far face rises above it.
                let far_wall = net
                    .far_faces
                    .iter()
                    .zip(&net.depths)
                    .filter(|(ff, _)| *ff == n)
                    .map(|(_, depth)| -20.0 * (2.0 * PI * f / c0.air.c * depth).cos().log10())
                    .fold(0.0, f64::max);
                let tol = 0.1 * amp + volume + far_wall;
                worst = worst.max(d.abs() / tol);
                assert!(
                    d.abs() < tol,
                    "seed {}, node {n}, {f} Hz (amplification {amp}, duct volume {volume} dB): \
                     {d} dB\n{:#}",
                    40_000 + seed,
                    docs[1]
                );
                checked += 1;
            }
        }
    }
    eprintln!("continuity: {checked} checks, {amplified} amplified, worst |ΔdB|/bound {worst:.3}");
    assert!(checked > 1000, "{checked}");
    // Most checks run at the plain 0.1 dB bound.
    assert!(
        amplified * 5 < checked,
        "{amplified} amplified of {checked}"
    );
}

#[test]
fn duct_levels_agree_below_gamma_l_0_17() {
    // Element-level form of E4 for ducts: the L1 line's input impedance with
    // its far end at ambient (B/D) against the L0 series impedance differs by
    // about |Γl|²/3, i.e. by less than 0.1 dB while |Γ|·l ≤ 0.17.
    let air = AirState::spec_reference();
    let rig = |duct: &Value, level: u8| {
        let mut d = duct.clone();
        d["id"] = json!("duct");
        d["nodes"] = json!(["a", "ambient"]);
        let doc = json!({"air": {"preset": "spec_reference"}, "level": level,
                         "nodes": [{"id": "a", "domain": "acoustic"}],
                         "elements": [{"id": "u", "type": "flow_source", "node": "a"}, d],
                         "probes": [{"id": "z", "quantity": "impedance", "element": "u"}]});
        circuit(&doc, 0)
    };
    let mut checked = 0;
    for seed in 0..60u64 {
        let mut rng = Rng::new(60_000 + seed);
        let (duct, section, l) = if seed % 2 == 0 {
            let (r, l) = (rng.log_range(0.2, 3.0), rng.log_range(1.0, 100.0));
            (
                json!({"type": "tube", "radius_mm": r, "length_mm": l}),
                Section::Circle { radius: r * 1e-3 },
                l * 1e-3,
            )
        } else {
            let (g, l) = (rng.log_range(0.03, 1.0), rng.log_range(1.0, 50.0));
            let w = rng.range(5.0 * g, 40.0);
            (
                json!({"type": "slit", "gap_mm": g, "width_mm": w, "length_mm": l}),
                Section::Slit {
                    gap: g * 1e-3,
                    width: w * 1e-3,
                },
                l * 1e-3,
            )
        };
        let (c0, c1) = (rig(&duct, 0), rig(&duct, 1));
        for f in frequencies(&mut rng, 20, 1.0, 20_000.0) {
            if propagation(&section, &air, 2.0 * PI * f).0.norm() * l > 0.17 {
                continue;
            }
            let z0 = probe(&c0, &c0.solve_at(f).unwrap(), f, "z");
            let z1 = probe(&c1, &c1.solve_at(f).unwrap(), f, "z");
            let d = 20.0 * (z1.norm() / z0.norm()).log10();
            assert!(d.abs() < 0.1, "{duct} at {f} Hz: {d} dB");
            // Complex: |Γl|²/3 ≤ 0.96 % (plus the thin-layer terms).
            assert!(
                (z1 / z0 - 1.0).norm() < 0.012,
                "{duct} at {f} Hz: {z1} vs {z0}"
            );
            checked += 1;
        }
    }
    assert!(checked > 200, "{checked}");
    // The lossless k·l understates the difference for narrow ducts: a
    // 0.05 mm × 10 mm slit at k·l = 0.17 (928 Hz) has |Γ|·l ≈ 0.7.
    let slit = json!({"type": "slit", "gap_mm": 0.05, "width_mm": 10, "length_mm": 10});
    let f = 0.17 * air.c / (2.0 * PI * 0.01);
    let gl = propagation(
        &Section::Slit {
            gap: 0.05e-3,
            width: 10e-3,
        },
        &air,
        2.0 * PI * f,
    )
    .0
    .norm()
        * 0.01;
    let (c0, c1) = (rig(&slit, 0), rig(&slit, 1));
    let z0 = probe(&c0, &c0.solve_at(f).unwrap(), f, "z");
    let z1 = probe(&c1, &c1.solve_at(f).unwrap(), f, "z");
    // (Γl)² is nearly imaginary in the viscous regime, so the difference is
    // mostly phase: |Γl|²/3 ≈ 17 % complex, against (kl)²/3 ≈ 1 %.
    let rel = (z1 / z0 - 1.0).norm();
    assert!(
        gl > 0.5 && rel > 0.1,
        "|Γl| = {gl}, relative difference {rel}"
    );
}

// ----- Malformed netlists --------------------------------------------------------

fn parse_error(doc: Value) -> String {
    match Circuit::from_json(&doc.to_string()) {
        Ok(_) => panic!("accepted a malformed netlist:\n{doc:#}"),
        Err(e) => e.to_string(),
    }
}

fn one_element(domain: &str, element: Value) -> Value {
    json!({"nodes": [{"id": "n1", "domain": domain}], "elements": [element]})
}

#[test]
fn malformed_netlists_name_the_offender() {
    let cases: Vec<(Value, Vec<&str>)> = vec![
        // Unitless key.
        (
            one_element(
                "acoustic",
                json!({"id": "cup", "type": "cavity", "node": "n1", "volume": 30}),
            ),
            vec!["cup", "'volume' needs a unit suffix", "volume_cm3"],
        ),
        // Misspelled parameter key.
        (
            one_element(
                "acoustic",
                json!({"id": "cup", "type": "cavity", "node": "n1",
                                           "volume_cm3": 30, "wal_loss": false}),
            ),
            vec!["cup", "wal_loss"],
        ),
        // Misspelled unit suffix leaves the quantity missing.
        (
            one_element(
                "electrical",
                json!({"id": "vc", "type": "coil", "node": "n1", "Re_Ohm": 32}),
            ),
            vec!["vc", "missing 'Re'"],
        ),
        // Two spellings of one quantity.
        (
            one_element(
                "acoustic",
                json!({"id": "vent", "type": "tube", "node": "n1",
                                           "radius_mm": 1, "radius_m": 0.001, "length_mm": 2}),
            ),
            vec!["vent", "radius_m", "radius_mm"],
        ),
        // Non-positive value.
        (
            one_element(
                "mechanical",
                json!({"id": "m", "type": "mass", "node": "n1", "M_g": -1}),
            ),
            vec!["'m'", "must be positive"],
        ),
        // Wrong domain.
        (
            one_element(
                "acoustic",
                json!({"id": "cone", "type": "mass", "node": "n1", "M_g": 1}),
            ),
            vec!["cone", "n1", "Acoustic", "Mechanical"],
        ),
        (
            json!({"nodes": [{"id": "m1", "domain": "mechanical"}, {"id": "e1", "domain": "electrical"}],
                   "elements": [{"id": "dia", "type": "piston", "nodes": ["m1", "gnd", "e1", "gnd"],
                                 "Sd_cm2": 10}]}),
            vec!["dia", "e1", "Electrical", "Acoustic"],
        ),
        // Ground alias of the wrong domain.
        (
            one_element(
                "acoustic",
                json!({"id": "leak", "type": "slit", "nodes": ["n1", "frame"],
                                           "gap_mm": 0.1, "width_mm": 10, "length_mm": 5}),
            ),
            vec!["leak", "frame", "Mechanical", "Acoustic"],
        ),
        // Unknown node.
        (
            one_element(
                "acoustic",
                json!({"id": "vent", "type": "tube", "nodes": ["n1", "n9"],
                                           "radius_mm": 1, "length_mm": 2}),
            ),
            vec!["vent", "unknown node 'n9'"],
        ),
        // Duplicate ids.
        (
            json!({"nodes": [{"id": "n1", "domain": "acoustic"}],
                   "elements": [
                       {"id": "cup", "type": "cavity", "node": "n1", "volume_cm3": 30},
                       {"id": "cup", "type": "cavity", "node": "n1", "volume_cm3": 20}]}),
            vec!["duplicate element id 'cup'"],
        ),
        (
            json!({"nodes": [{"id": "n1", "domain": "acoustic"}, {"id": "n1", "domain": "acoustic"}]}),
            vec!["duplicate node 'n1'"],
        ),
        (
            json!({"nodes": [{"id": "ambient", "domain": "acoustic"}]}),
            vec!["'ambient' is reserved"],
        ),
        // Unknown element type and domain.
        (
            one_element(
                "acoustic",
                json!({"id": "x1", "type": "helmholtz", "node": "n1"}),
            ),
            vec!["x1", "helmholtz"],
        ),
        (
            json!({"nodes": [{"id": "h1", "domain": "hydraulic"}]}),
            vec!["h1", "hydraulic"],
        ),
        // Wrong terminal count.
        (
            one_element(
                "acoustic",
                json!({"id": "cup", "type": "cavity", "nodes": ["n1", "n1", "n1"],
                                           "volume_cm3": 30}),
            ),
            vec!["cup", "expects 1 to 2 node(s), got 3"],
        ),
        // Probes.
        (
            json!({"nodes": [{"id": "n1", "domain": "mechanical"}],
                   "probes": [{"id": "spl", "quantity": "pressure", "node": "n1"}]}),
            vec!["spl", "n1", "Mechanical"],
        ),
        (
            json!({"probes": [{"id": "zin", "quantity": "impedance", "element": "amp"}]}),
            vec!["zin", "unknown element 'amp'"],
        ),
        (
            json!({"probes": [{"id": "p", "quantity": "pressure", "node": "n1", "units": "Pa"}]}),
            vec!["'p'", "units"],
        ),
        // Top level.
        (json!({"sweeep": {}}), vec!["sweeep"]),
        (json!({"level": 3}), vec!["'level'"]),
        (json!({"air": {"preset": "mars"}}), vec!["mars"]),
    ];
    for (doc, needles) in cases {
        let msg = parse_error(doc.clone());
        for n in needles {
            assert!(msg.contains(n), "error {msg:?} lacks {n:?} for\n{doc:#}");
        }
    }
}

#[test]
fn floating_subnetworks_fail_to_solve_and_name_a_node() {
    // A declared node nothing connects to, and islands with no path to the
    // reference in each domain. (At level 1 a duct island is *not*
    // floating: its line compliance references it to ambient.)
    let base = |nodes: Value, elements: Value| {
        json!({"level": 0, "nodes": nodes, "elements": elements,
               "sweep": {"frequencies_Hz": [100.0]}})
    };
    let cases = [
        (
            base(
                json!([{"id": "a1", "domain": "acoustic"}, {"id": "orphan", "domain": "acoustic"}]),
                json!([{"id": "u", "type": "flow_source", "node": "a1"},
                       {"id": "cup", "type": "cavity", "node": "a1", "volume_cm3": 30}]),
            ),
            vec!["orphan"],
        ),
        (
            base(
                json!([{"id": "a1", "domain": "acoustic"}, {"id": "ax", "domain": "acoustic"},
                       {"id": "ay", "domain": "acoustic"}]),
                json!([{"id": "u", "type": "flow_source", "node": "a1"},
                       {"id": "cup", "type": "cavity", "node": "a1", "volume_cm3": 30},
                       {"id": "island", "type": "slit", "nodes": ["ax", "ay"],
                        "gap_mm": 0.2, "width_mm": 10, "length_mm": 5}]),
            ),
            vec!["ax", "ay"],
        ),
        (
            base(
                json!([{"id": "e1", "domain": "electrical"}, {"id": "ex", "domain": "electrical"},
                       {"id": "ey", "domain": "electrical"}]),
                json!([{"id": "v", "type": "vsource", "node": "e1"},
                       {"id": "r", "type": "resistor", "node": "e1", "R_ohm": 8},
                       {"id": "island", "type": "resistor", "nodes": ["ex", "ey"], "R_ohm": 8}]),
            ),
            vec!["ex", "ey"],
        ),
        (
            base(
                json!([{"id": "m1", "domain": "mechanical"}, {"id": "mx", "domain": "mechanical"},
                       {"id": "my", "domain": "mechanical"}]),
                json!([{"id": "f", "type": "force_source", "node": "m1"},
                       {"id": "m", "type": "mass", "node": "m1", "M_g": 1},
                       {"id": "island", "type": "spring", "nodes": ["mx", "my"], "K_N_per_m": 100}]),
            ),
            vec!["mx", "my"],
        ),
    ];
    for (doc, names) in cases {
        let c = Circuit::from_json(&doc.to_string()).unwrap();
        let msg = match c.solve() {
            Ok(_) => panic!("floating network solved:\n{doc:#}"),
            Err(e) => e.to_string(),
        };
        assert!(msg.contains("singular"), "{msg}");
        assert!(
            names.iter().any(|n| msg.contains(&format!("node {n}"))),
            "error {msg:?} names none of {names:?}"
        );
    }
    // The same slit island at level 1 is referenced by its own compliance.
    let mut doc = slit_island_netlist();
    doc["level"] = json!(1);
    let c = Circuit::from_json(&doc.to_string()).unwrap();
    let x = c.solve_at(100.0).unwrap();
    assert!(c.node_value(&x, "ax").unwrap().norm() == 0.0);
}

fn slit_island_netlist() -> Value {
    json!({"nodes": [{"id": "a1", "domain": "acoustic"}, {"id": "ax", "domain": "acoustic"},
                     {"id": "ay", "domain": "acoustic"}],
           "elements": [{"id": "u", "type": "flow_source", "node": "a1"},
                        {"id": "cup", "type": "cavity", "node": "a1", "volume_cm3": 30},
                        {"id": "island", "type": "slit", "nodes": ["ax", "ay"],
                         "gap_mm": 0.2, "width_mm": 10, "length_mm": 5}],
           "sweep": {"frequencies_Hz": [100.0]}})
}

// ----- Shipped examples ------------------------------------------------------------

#[test]
fn examples_solve_balance_power_and_are_passive() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    assert!(paths.len() >= 4, "{paths:?}");
    for path in paths {
        let text = std::fs::read_to_string(&path).unwrap();
        let c = Circuit::from_json(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let r = c
            .solve()
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(r.probes.iter().all(|p| p
            .values
            .iter()
            .all(|v| v.re.is_finite() && v.im.is_finite())));
        let sources: Vec<usize> = (0..c.elements.len())
            .filter(|&i| c.elements[i].is_source())
            .collect();
        let doc: Value = serde_json::from_str(&text).unwrap();
        for &f in c.freqs.iter().step_by(7) {
            let (raw, x) = refined_solve(&c, f, 0, &doc);
            let ctx = || path.display().to_string();
            check_tellegen(&c, f, &raw, ENGINE_BALANCE, &ctx);
            let (powers, tol) = check_tellegen(&c, f, &x, REFINED_BALANCE, &ctx);
            for (e, p) in c.elements.iter().zip(&powers) {
                if !e.is_source() {
                    assert!(*p >= -tol, "{}: {} generates {p} at {f} Hz", ctx(), e.id());
                }
            }
            // Independent sources each deliver non-negative power when they
            // drive separate islands, as every shipped example does.
            for &i in &sources {
                assert!(
                    powers[i] <= tol,
                    "{}: source {} absorbs power",
                    ctx(),
                    c.elements[i].id()
                );
            }
        }
    }
}
