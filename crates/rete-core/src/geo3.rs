//! A 3D extension of the GeoSPARQL built-ins (`geo3`).
//!
//! GeoSPARQL (see [`crate::geo`]) is planar: its `wktLiteral` parser drops the Z
//! ordinate and every relation/distance is 2D. `geo3` adds true three-dimensional
//! topology and distance over **axis-aligned bounding boxes** (AABBs) derived from
//! 3D geometry literals — `POINT Z`, `MULTIPOINT Z`, `LINESTRING Z`,
//! `POLYHEDRALSURFACE Z`, and a compact `BOX3D(minx miny minz, maxx maxy maxz)`.
//!
//! It is deliberately AABB-level (not exact solid geometry): that is what makes it
//! cheap enough to evaluate inside a FILTER over a whole graph, and it is exactly
//! the model materialised by the z-anatomy dataset. Coordinates are an abstract
//! Cartesian 3-space in the literal's own unit (z-anatomy uses millimetres); no CRS
//! reprojection is performed. wasm-safe: `core`/`alloc` + `f64` only.
//!
//! Exposed as SPARQL functions (base `https://w3id.org/rete/geo3/function/`):
//!   * `distance3D(g1, g2)`          → xsd:double, min gap between the AABBs (0 if they meet)
//!   * `contains3D(g1, g2)`          → xsd:boolean, AABB(g1) ⊇ AABB(g2)
//!   * `within3D(g1, g2)`            → xsd:boolean, AABB(g1) ⊆ AABB(g2)
//!   * `adjacent3D(g1, g2 [, gap])`  → xsd:boolean, AABBs within `gap` (default 0) on every axis

/// Datatype IRI for a 3D WKT literal (`"POINT Z(x y z)"^^geo3:wktLiteral3D`).
pub(crate) const WKT3: &str = "https://w3id.org/rete/geo3#wktLiteral3D";
/// Datatype IRI for a bounding-box literal (`"BOX3D(...)"^^geo3:box3dLiteral`).
pub(crate) const BOX3D: &str = "https://w3id.org/rete/geo3#box3dLiteral";

const EPS: f64 = 1e-9;

/// An axis-aligned 3D bounding box (a point is the degenerate `min == max`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

/// The 3D topological relation to test.
#[derive(Clone, Copy)]
pub(crate) enum Rel3 {
    Contains,
    Within,
    Adjacent,
}

const KEYWORDS: &[&str] = &[
    "POINT",
    "MULTIPOINT",
    "LINESTRING",
    "MULTILINESTRING",
    "POLYGON",
    "MULTIPOLYGON",
    "POLYHEDRALSURFACE",
    "TIN",
    "GEOMETRYCOLLECTION",
    "BOX3D",
];

/// Parse a 3D WKT / BOX3D lexical form into its AABB. A leading CRS URI
/// (`<…> POINT Z(…)`) is stripped and ignored. Returns `None` on an unknown
/// keyword, an EMPTY/degenerate geometry, or no finite coordinate. Coordinates
/// are grouped by the WKT commas; nested parens are flattened (only the AABB is
/// needed), and a coordinate's 3rd ordinate defaults to 0 if absent.
pub(crate) fn parse(input: &str) -> Option<Aabb> {
    let mut s = input.trim();
    if let Some(rest) = s.strip_prefix('<') {
        let end = rest.find('>')?;
        s = rest[end + 1..].trim_start();
    }
    let open = s.find('(')?;
    // leading token before '(' is the geometry keyword; a "Z"/"M"/"ZM" dimension
    // tag (if any) is a separate whitespace-delimited word and is ignored.
    let head = s[..open].trim().to_ascii_uppercase();
    let kw = head.split_whitespace().next()?;
    if !KEYWORDS.contains(&kw) {
        return None;
    }
    let close = s.rfind(')')?;
    if close <= open {
        return None;
    }
    let body = &s[open + 1..close];
    // flatten nested rings/parens; group coordinates by comma
    let flat: String = body
        .chars()
        .map(|c| if c == '(' || c == ')' { ' ' } else { c })
        .collect();

    let mut mn = [f64::INFINITY; 3];
    let mut mx = [f64::NEG_INFINITY; 3];
    let mut any = false;
    for group in flat.split(',') {
        let mut ords = [0.0_f64; 3];
        let mut nread = 0usize;
        for tok in group.split_whitespace() {
            if nread >= 3 {
                break;
            }
            match tok.parse::<f64>() {
                Ok(v) if v.is_finite() => {
                    ords[nread] = v;
                    nread += 1;
                }
                _ => {} // ignore an "EMPTY" token or stray text
            }
        }
        if nread >= 2 {
            for i in 0..3 {
                mn[i] = mn[i].min(ords[i]);
                mx[i] = mx[i].max(ords[i]);
            }
            any = true;
        }
    }
    any.then_some(Aabb { min: mn, max: mx })
}

/// Per-axis clearance between two boxes (0 if they overlap on that axis).
fn axis_gap(a_min: f64, a_max: f64, b_min: f64, b_max: f64) -> f64 {
    (a_min - b_max).max(b_min - a_max).max(0.0)
}

/// Minimum Euclidean distance between the two AABBs (0 if they intersect).
/// For point geometries this is the exact point-to-point distance.
pub(crate) fn distance(a: &Aabb, b: &Aabb) -> f64 {
    let mut sum = 0.0;
    for i in 0..3 {
        let g = axis_gap(a.min[i], a.max[i], b.min[i], b.max[i]);
        sum += g * g;
    }
    sum.sqrt()
}

/// `a` contains `b`: b's box lies inside a's box on every axis (within EPS).
fn contains(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| a.min[i] <= b.min[i] + EPS && a.max[i] >= b.max[i] - EPS)
}

/// Boxes are within `gap` on every axis (gap 0 = touching/overlapping).
fn adjacent(a: &Aabb, b: &Aabb, gap: f64) -> bool {
    let g = gap.max(0.0);
    (0..3).all(|i| a.min[i] <= b.max[i] + g + EPS && a.max[i] >= b.min[i] - g - EPS)
}

/// Evaluate a 3D relation. `gap` is only used by `Adjacent`.
pub(crate) fn relate(rel: Rel3, a: &Aabb, b: &Aabb, gap: f64) -> bool {
    match rel {
        Rel3::Contains => contains(a, b),
        Rel3::Within => contains(b, a),
        Rel3::Adjacent => adjacent(a, b, gap),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Aabb {
        parse(s).unwrap()
    }

    #[test]
    fn parse_point_box_multipoint() {
        assert_eq!(
            p("POINT Z(1 2 3)"),
            Aabb {
                min: [1.0, 2.0, 3.0],
                max: [1.0, 2.0, 3.0]
            }
        );
        assert_eq!(
            p("BOX3D(0 0 0, 10 20 30)"),
            Aabb {
                min: [0.0, 0.0, 0.0],
                max: [10.0, 20.0, 30.0]
            }
        );
        // multipoint / linestring → bounding box over all coords
        assert_eq!(
            p("MULTIPOINT Z(1 1 1, 5 -2 9)"),
            Aabb {
                min: [1.0, -2.0, 1.0],
                max: [5.0, 1.0, 9.0]
            }
        );
        // CRS prefix ignored; 2D point → z defaults to 0
        assert_eq!(p("<urn:crs> POINT(4 5)").min, [4.0, 5.0, 0.0]);
        // nested parens (a box-like polyhedral surface) flatten to the AABB
        let a = p("POLYHEDRALSURFACE Z(((0 0 0, 1 0 0, 1 1 2)))");
        assert_eq!(a.min, [0.0, 0.0, 0.0]);
        assert_eq!(a.max, [1.0, 1.0, 2.0]);
        assert!(parse("FOO(1 2 3)").is_none());
        assert!(parse("POINT EMPTY").is_none());
    }

    #[test]
    fn distance_3d() {
        // axis-aligned separation on Z only
        let d = distance(&p("POINT Z(0 0 0)"), &p("POINT Z(0 0 5)"));
        assert!((d - 5.0).abs() < 1e-9);
        // 3-4-0 -> 5 in the XY plane, plus 12 on Z -> 13
        let d = distance(&p("POINT Z(0 0 0)"), &p("POINT Z(3 4 12)"));
        assert!((d - 13.0).abs() < 1e-9);
        // overlapping boxes -> 0
        assert_eq!(
            distance(&p("BOX3D(0 0 0, 10 10 10)"), &p("POINT Z(5 5 5)")),
            0.0
        );
        // box-to-box gap on one axis
        let d = distance(&p("BOX3D(0 0 0, 1 1 1)"), &p("BOX3D(0 0 4, 1 1 5)"));
        assert!((d - 3.0).abs() < 1e-9);
    }

    #[test]
    fn relations_3d() {
        let big = p("BOX3D(0 0 0, 10 10 10)");
        let small = p("BOX3D(2 2 2, 4 4 4)");
        assert!(relate(Rel3::Contains, &big, &small, 0.0));
        assert!(!relate(Rel3::Contains, &small, &big, 0.0));
        assert!(relate(Rel3::Within, &small, &big, 0.0));
        // adjacency: 2 mm apart on Z, not touching at gap 0 but adjacent at gap 3
        let a = p("BOX3D(0 0 0, 1 1 1)");
        let b = p("BOX3D(0 0 3, 1 1 4)");
        assert!(!relate(Rel3::Adjacent, &a, &b, 0.0));
        assert!(relate(Rel3::Adjacent, &a, &b, 3.0));
        // touching faces are adjacent at gap 0
        let c = p("BOX3D(1 0 0, 2 1 1)");
        assert!(relate(Rel3::Adjacent, &a, &c, 0.0));
    }
}
