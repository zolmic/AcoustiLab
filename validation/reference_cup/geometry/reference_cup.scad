// Open reference cup (spec Section 17): parametric CAD.
// Licence: CC-BY-4.0 (https://creativecommons.org/licenses/by/4.0/).
//
// Every dimension comes from dimensions.scad, which
// tools/refcup/dims_to_scad.py generates from ../dimensions.json, the same
// file the netlist examples/reference_cup.json is checked against.
//
// Render one part in its print orientation:
//   openscad -D 'part="baffle"' -o baffle.stl reference_cup.scad
// Parts: baffle, retainer, pad_ring, rear_shell, plug_rear_sealed,
// plug_rear_hole, plug_rear_mesh, plug_front_sealed, plug_front_mesh.
// part="assembly" draws the assembled cup cut open (gasket and driver
// outlines included), for inspection only.
//
// Assembly coordinates: the axis is z, the baffle's front face is z = 0,
// the ear side is -z. Units are mm.

include <dimensions.scad>

part = "assembly";
$fn = 128;
e = 0.01;

// Derived dimensions (kept here so that the JSON holds only independent
// ones).
outer_d = cup_inner_diameter_mm + 2 * baffle_rim_mm;
baffle_t = lip_thickness_mm + driver_gasket_mm + driver_body_depth_mm;
driver_bore_d = driver_outer_diameter_mm + 2 * seat_clearance_mm;
shell_od = cup_inner_diameter_mm + 2 * shell_wall_mm;
shell_len = rear_depth_mm + back_plate_mm;
port_d = plug_diameter_mm + 2 * plug_clearance_mm;
plug_d = plug_diameter_mm - 2 * plug_clearance_mm;
groove_mid_r = (joint_oring_id_mm + joint_oring_cs_mm) / 2;
groove_w = joint_oring_cs_mm + 0.5;
groove_depth = 0.75 * joint_oring_cs_mm;
m3_clear = 3.4;
m3_insert = 4.0;
m3_insert_depth = 6;
m3_head = 6.5;
m2_clear = 2.4;
m2_insert = 3.2;
m2_insert_depth = 4;

function joint_angle(i) = 30 + 60 * i;
function retainer_angle(i) = 90 + 120 * i;

// Face groove for an O-ring, open towards +z (up = true) or -z.
module oring_groove(z, up) {
    translate([0, 0, up ? z - groove_depth : z - e])
        difference() {
            cylinder(r = groove_mid_r + groove_w / 2, h = groove_depth + e);
            translate([0, 0, -e]) cylinder(r = groove_mid_r - groove_w / 2, h = groove_depth + 3 * e);
        }
}

module baffle() {
    difference() {
        cylinder(d = outer_d, h = baffle_t);
        translate([0, 0, -e]) cylinder(d = lip_inner_diameter_mm, h = baffle_t + 2 * e);
        translate([0, 0, lip_thickness_mm]) cylinder(d = driver_bore_d, h = baffle_t);
        oring_groove(0, false);
        oring_groove(baffle_t, true);
        for (i = [0 : joint_screws - 1])
            rotate(joint_angle(i)) translate([joint_screw_circle_mm / 2, 0, -e])
                cylinder(d = m3_clear, h = baffle_t + 2 * e, $fn = 32);
        for (i = [0 : retainer_screws - 1])
            rotate(retainer_angle(i)) translate([retainer_screw_circle_mm / 2, 0, baffle_t - m2_insert_depth])
                cylinder(d = m2_insert, h = m2_insert_depth + e, $fn = 32);
    }
}

module retainer() {
    translate([0, 0, baffle_t])
        difference() {
            cylinder(d = retainer_outer_diameter_mm, h = retainer_thickness_mm);
            translate([0, 0, -e]) cylinder(d = retainer_inner_diameter_mm, h = retainer_thickness_mm + 2 * e);
            for (i = [0 : retainer_screws - 1])
                rotate(retainer_angle(i)) translate([retainer_screw_circle_mm / 2, 0, -e])
                    cylinder(d = m2_clear, h = retainer_thickness_mm + 2 * e, $fn = 24);
        }
}

// Radial port through the pad ring at angle a, half-way up the ring,
// printed undersize (drill_allowance_mm) and reamed to port_d.
module front_port(a) {
    rotate(a) translate([cup_inner_diameter_mm / 2 - 1, 0, -pad_ring_height_mm / 2])
        rotate([0, 90, 0]) cylinder(d = port_d - drill_allowance_mm, h = pad_ring_width_mm + 2, $fn = 64);
}

module pad_ring() {
    difference() {
        translate([0, 0, -pad_ring_height_mm]) cylinder(d = outer_d, h = pad_ring_height_mm);
        translate([0, 0, -pad_ring_height_mm - e]) cylinder(d = cup_inner_diameter_mm, h = pad_ring_height_mm + 2 * e);
        for (k = [0 : front_ports - 1]) front_port(360 / front_ports * k);
        for (i = [0 : joint_screws - 1])
            rotate(joint_angle(i)) translate([joint_screw_circle_mm / 2, 0, -m3_insert_depth])
                cylinder(d = m3_insert, h = m3_insert_depth + e, $fn = 32);
    }
}

module rear_shell() {
    z0 = baffle_t;
    difference() {
        union() {
            translate([0, 0, z0]) cylinder(d = shell_od, h = shell_len);
            translate([0, 0, z0]) cylinder(d = outer_d, h = shell_flange_mm);
            // 45-degree skirt under the flange, so that it prints back
            // plate down without support.
            translate([0, 0, z0 + shell_flange_mm])
                cylinder(d1 = outer_d, d2 = shell_od, h = (outer_d - shell_od) / 2);
        }
        translate([0, 0, z0 - e]) cylinder(d = cup_inner_diameter_mm, h = rear_depth_mm + e);
        // Rear port, printed undersize and reamed to port_d.
        translate([0, 0, z0 + rear_depth_mm - e])
            cylinder(d = port_d - drill_allowance_mm, h = back_plate_mm + 2 * e, $fn = 64);
        for (i = [0 : joint_screws - 1])
            rotate(joint_angle(i)) translate([joint_screw_circle_mm / 2, 0, 0]) {
                translate([0, 0, z0 - e]) cylinder(d = m3_clear, h = shell_flange_mm + 2 * e, $fn = 32);
                translate([0, 0, z0 + shell_flange_mm])
                    cylinder(d = m3_head, h = (outer_d - shell_od) / 2 + 1, $fn = 32);
            }
        // Lead exit, sealed with putty after assembly.
        translate([0, 0, z0 + shell_flange_mm + 4]) rotate([0, 90, 0])
            cylinder(d = cable_hole_diameter_mm, h = outer_d, $fn = 24);
    }
}

// A plug of length len along z from 0; hole_d = 0 is a sealed plug with a
// pilot hole for an M3 extraction screw in its outer face (z = len).
module plug(len, hole_d) {
    difference() {
        cylinder(d = plug_d, h = len, $fn = 96);
        translate([0, 0, (len - plug_groove_width_mm) / 2])
            difference() {
                cylinder(d = plug_d + 1, h = plug_groove_width_mm, $fn = 96);
                translate([0, 0, -e]) cylinder(d = plug_groove_diameter_mm, h = plug_groove_width_mm + 2 * e, $fn = 96);
            }
        if (hole_d > 0)
            translate([0, 0, -e]) cylinder(d = hole_d - drill_allowance_mm, h = len + 2 * e, $fn = 64);
        else
            translate([0, 0, len - 4]) cylinder(d = 2.5, h = 4 + e, $fn = 24);
    }
}

module gasket() {
    color("DimGray")
        translate([0, 0, -pad_ring_height_mm - gasket_thickness_mm])
            difference() {
                cylinder(d = outer_d, h = gasket_thickness_mm);
                translate([0, 0, -e]) cylinder(d = cup_inner_diameter_mm, h = gasket_thickness_mm + 2 * e);
            }
}

module driver_outline() {
    color("Goldenrod", 0.6)
        translate([0, 0, lip_thickness_mm + driver_gasket_mm]) {
            cylinder(d = driver_outer_diameter_mm, h = driver_body_depth_mm);
            cylinder(d = driver_rear_boss_diameter_mm, h = driver_overall_depth_mm);
        }
}

module assembly() {
    difference() {
        union() {
            color("SteelBlue") baffle();
            color("LightSteelBlue") retainer();
            color("CadetBlue") pad_ring();
            color("SlateGray") rear_shell();
            gasket();
            driver_outline();
            color("Orange") translate([0, 0, baffle_t + rear_depth_mm]) plug(back_plate_mm, rear_mesh_hole_diameter_mm);
            for (k = [0 : front_ports - 1])
                color("Orange") rotate(360 / front_ports * k)
                    translate([cup_inner_diameter_mm / 2, 0, -pad_ring_height_mm / 2]) rotate([0, 90, 0])
                        plug(pad_ring_width_mm, front_hole_diameter_mm);
        }
        // Cut away the +y half.
        translate([-outer_d, 0, -100]) cube([2 * outer_d, outer_d, 200]);
    }
}

if (part == "assembly") assembly();
if (part == "baffle") baffle();
if (part == "retainer") translate([0, 0, -baffle_t]) retainer();
if (part == "pad_ring") rotate([180, 0, 0]) pad_ring();
if (part == "rear_shell") translate([0, 0, baffle_t + shell_len]) rotate([180, 0, 0]) rear_shell();
if (part == "plug_rear_sealed") plug(back_plate_mm, 0);
if (part == "plug_rear_hole") plug(back_plate_mm, rear_hole_diameter_mm);
if (part == "plug_rear_mesh") plug(back_plate_mm, rear_mesh_hole_diameter_mm);
if (part == "plug_front_sealed") plug(pad_ring_width_mm, 0);
if (part == "plug_front_mesh") plug(pad_ring_width_mm, front_hole_diameter_mm);
