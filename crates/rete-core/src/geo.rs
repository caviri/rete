//! Self-contained WKT parsing + planar geometry for the GeoSPARQL built-ins.
//!
//! wasm-safe: `core`/`alloc` + `f64` intrinsics only — no external crates, no
//! `std::time`/threads/rng. All coordinates are treated as **CRS84 lon/lat**
//! (`x` = longitude, `y` = latitude); a CRS-URI prefix on a `wktLiteral` is
//! accepted but ignored (axes are never swapped). Computations are planar
//! (Cartesian on lon/lat degrees); `distance` in metres switches to a haversine
//! great-circle on the closest pair. See `docs/geosparql.md` for the scope.

/// A coordinate. `x` = longitude, `y` = latitude (CRS84 axis order).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Coord {
    pub x: f64,
    pub y: f64,
}

/// A closed polygon ring (the parser auto-closes if the last point ≠ the first).
type Ring = Vec<Coord>;

/// A parsed WKT geometry (the subset rete understands).
// Variant names are the canonical OGC WKT geometry types — keep them verbatim.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Debug)]
pub(crate) enum Geometry {
    /// `POINT EMPTY` → `None`.
    Point(Option<Coord>),
    MultiPoint(Vec<Coord>),
    LineString(Vec<Coord>),
    /// `rings[0]` = exterior, `rings[1..]` = holes.
    Polygon(Vec<Ring>),
    MultiPolygon(Vec<Vec<Ring>>),
    /// Parsed for completeness; relations over it are a type error (`None`) in v1.
    GeometryCollection(Vec<Geometry>),
}

/// The topological relation to test — keeps this module independent of
/// `sparql::Builtin`.
#[derive(Clone, Copy)]
pub(crate) enum Rel {
    Contains,
    Within,
    Intersects,
    Disjoint,
    Equals,
}

/// Distance unit for `geof:distance`.
#[derive(Clone, Copy)]
pub(crate) enum Unit {
    Metre,
    Degree,
}

pub(crate) const GEO_WKT: &str = "http://www.opengis.net/ont/geosparql#wktLiteral";
pub(crate) const UOM_METRE: &str = "http://www.opengis.net/def/uom/OGC/1.0/metre";
pub(crate) const UOM_DEGREE: &str = "http://www.opengis.net/def/uom/OGC/1.0/degree";

/// Boundary / equality tolerance.
const EPS: f64 = 1e-9;
/// Mean Earth radius (metres) used by the haversine distance.
const EARTH_R_M: f64 = 6_371_008.8;

// ---------------------------------------------------------------------------
// WKT parser (recursive descent; error = None, never panics)
// ---------------------------------------------------------------------------

/// Parse a WKT lexical form (already unescaped) into a [`Geometry`]. A leading
/// CRS URI (`<…> POINT(…)`) is stripped and ignored. Returns `None` on any
/// malformed input (unbalanced parens, non-numeric/non-finite coordinate,
/// trailing garbage, unknown keyword).
pub(crate) fn parse_wkt(input: &str) -> Option<Geometry> {
    let mut s = input.trim_start();
    if let Some(rest) = s.strip_prefix('<') {
        let end = rest.find('>')?;
        s = rest[end + 1..].trim_start();
    }
    let mut p = Parser {
        b: s.as_bytes(),
        i: 0,
    };
    let g = p.geometry(0)?;
    p.ws();
    p.b.get(p.i).is_none().then_some(g)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.b.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    /// Consume the byte `c` if it is next (after whitespace).
    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.b.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, c: u8) -> Option<()> {
        self.eat(c).then_some(())
    }

    /// An uppercased run of ASCII letters (the type keyword / `EMPTY` / dim tag).
    fn keyword(&mut self) -> Option<String> {
        self.ws();
        let start = self.i;
        while self.b.get(self.i).is_some_and(u8::is_ascii_alphabetic) {
            self.i += 1;
        }
        (self.i > start).then(|| {
            std::str::from_utf8(&self.b[start..self.i])
                .unwrap()
                .to_ascii_uppercase()
        })
    }

    fn next_is_alpha(&mut self) -> bool {
        self.ws();
        self.b.get(self.i).is_some_and(u8::is_ascii_alphabetic)
    }

    fn number(&mut self) -> Option<f64> {
        self.ws();
        let start = self.i;
        while let Some(&c) = self.b.get(self.i) {
            if c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.' | b'e' | b'E') {
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i == start {
            return None;
        }
        // Restore the cursor on a malformed run (e.g. "9.9.9") so coord()'s
        // extra-ordinate drop-loop can't silently swallow it and accept garbage.
        match std::str::from_utf8(&self.b[start..self.i])
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
        {
            Some(v) if v.is_finite() => Some(v),
            _ => {
                self.i = start;
                None
            }
        }
    }

    /// One coordinate: two ordinates; any extra (Z/M) ordinates are consumed and
    /// dropped (they stop at the `,`/`)` that separates coordinates).
    fn coord(&mut self) -> Option<Coord> {
        let x = self.number()?;
        let y = self.number()?;
        while self.number().is_some() {}
        Some(Coord { x, y })
    }

    fn coord_list(&mut self) -> Option<Vec<Coord>> {
        let mut v = vec![self.coord()?];
        while self.eat(b',') {
            v.push(self.coord()?);
        }
        Some(v)
    }

    /// `( (ring), (hole), … )` — a polygon's parenthesised ring list.
    fn polygon_rings(&mut self) -> Option<Vec<Ring>> {
        self.expect(b'(')?;
        let mut rings = Vec::new();
        loop {
            self.expect(b'(')?;
            let mut r = self.coord_list()?;
            self.expect(b')')?;
            close_ring(&mut r);
            if r.len() < 4 {
                return None; // degenerate ring
            }
            rings.push(r);
            if !self.eat(b',') {
                break;
            }
        }
        self.expect(b')')?;
        Some(rings)
    }

    fn geometry(&mut self, depth: usize) -> Option<Geometry> {
        if depth > 32 {
            return None;
        }
        let kw = self.keyword()?;
        // Optional dimension tag (Z/M/ZM) or the EMPTY keyword.
        let mut empty = false;
        if self.next_is_alpha() {
            match self.keyword()?.as_str() {
                "EMPTY" => empty = true,
                "Z" | "M" | "ZM" => {}
                _ => return None,
            }
        }
        Some(match kw.as_str() {
            "POINT" => {
                if empty {
                    Geometry::Point(None)
                } else {
                    self.expect(b'(')?;
                    let c = self.coord()?;
                    self.expect(b')')?;
                    Geometry::Point(Some(c))
                }
            }
            "MULTIPOINT" => Geometry::MultiPoint(if empty {
                Vec::new()
            } else {
                self.multipoint()?
            }),
            "LINESTRING" => Geometry::LineString(if empty {
                Vec::new()
            } else {
                self.expect(b'(')?;
                let pts = self.coord_list()?;
                self.expect(b')')?;
                pts
            }),
            "POLYGON" => Geometry::Polygon(if empty {
                Vec::new()
            } else {
                self.polygon_rings()?
            }),
            "MULTIPOLYGON" => Geometry::MultiPolygon(if empty {
                Vec::new()
            } else {
                self.expect(b'(')?;
                let mut polys = vec![self.polygon_rings()?];
                while self.eat(b',') {
                    polys.push(self.polygon_rings()?);
                }
                self.expect(b')')?;
                polys
            }),
            "GEOMETRYCOLLECTION" => Geometry::GeometryCollection(if empty {
                Vec::new()
            } else {
                self.expect(b'(')?;
                let mut gs = vec![self.geometry(depth + 1)?];
                while self.eat(b',') {
                    gs.push(self.geometry(depth + 1)?);
                }
                self.expect(b')')?;
                gs
            }),
            _ => return None,
        })
    }

    /// `MULTIPOINT(1 2, 3 4)` or `MULTIPOINT((1 2),(3 4))` — both accepted.
    fn multipoint(&mut self) -> Option<Vec<Coord>> {
        self.expect(b'(')?;
        let mut v = Vec::new();
        loop {
            let paren = self.eat(b'(');
            v.push(self.coord()?);
            if paren {
                self.expect(b')')?;
            }
            if !self.eat(b',') {
                break;
            }
        }
        self.expect(b')')?;
        Some(v)
    }
}

fn close_ring(r: &mut Ring) {
    if let (Some(&first), Some(&last)) = (r.first(), r.last()) {
        if first != last {
            r.push(first);
        }
    }
}

// ---------------------------------------------------------------------------
// Geometry accessors
// ---------------------------------------------------------------------------

/// The polygons of an areal geometry (`Polygon` → one, `MultiPolygon` → each),
/// or `None` for a non-areal geometry.
fn polygons(g: &Geometry) -> Option<Vec<&Vec<Ring>>> {
    match g {
        Geometry::Polygon(rings) => Some(vec![rings]),
        Geometry::MultiPolygon(ps) => Some(ps.iter().collect()),
        _ => None,
    }
}

/// Every vertex the geometry mentions.
fn all_coords(g: &Geometry) -> Vec<Coord> {
    let mut v = Vec::new();
    collect_coords(g, &mut v);
    v
}

fn collect_coords(g: &Geometry, out: &mut Vec<Coord>) {
    match g {
        Geometry::Point(Some(c)) => out.push(*c),
        Geometry::Point(None) => {}
        Geometry::MultiPoint(cs) | Geometry::LineString(cs) => out.extend_from_slice(cs),
        Geometry::Polygon(rings) => rings.iter().for_each(|r| out.extend_from_slice(r)),
        Geometry::MultiPolygon(ps) => ps.iter().flatten().for_each(|r| out.extend_from_slice(r)),
        Geometry::GeometryCollection(gs) => gs.iter().for_each(|g| collect_coords(g, out)),
    }
}

/// Every line segment (LineString segments + ring edges).
fn edges(g: &Geometry) -> Vec<(Coord, Coord)> {
    let mut e = Vec::new();
    let mut ring = |r: &[Coord]| {
        for w in r.windows(2) {
            e.push((w[0], w[1]));
        }
    };
    match g {
        Geometry::LineString(cs) => ring(cs),
        Geometry::Polygon(rings) => rings.iter().for_each(|r| ring(r)),
        Geometry::MultiPolygon(ps) => ps.iter().flatten().for_each(|r| ring(r)),
        Geometry::GeometryCollection(gs) => return gs.iter().flat_map(edges).collect(),
        _ => {}
    }
    e
}

fn bbox(g: &Geometry) -> Option<(f64, f64, f64, f64)> {
    let cs = all_coords(g);
    let first = cs.first()?;
    let mut b = (first.x, first.y, first.x, first.y);
    for c in &cs {
        b.0 = b.0.min(c.x);
        b.1 = b.1.min(c.y);
        b.2 = b.2.max(c.x);
        b.3 = b.3.max(c.y);
    }
    Some(b)
}

fn bbox_overlap(a: &Geometry, b: &Geometry) -> bool {
    match (bbox(a), bbox(b)) {
        (Some(x), Some(y)) => {
            x.0 <= y.2 + EPS && y.0 <= x.2 + EPS && x.1 <= y.3 + EPS && y.1 <= x.3 + EPS
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Planar primitives
// ---------------------------------------------------------------------------

/// Twice the signed area of triangle `abc` (the robust orientation primitive).
fn orient(a: Coord, b: Coord, c: Coord) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

/// Is `p` on the closed segment `ab` (within tolerance)?
fn on_segment(p: Coord, a: Coord, b: Coord) -> bool {
    orient(a, b, p).abs() <= EPS
        && p.x >= a.x.min(b.x) - EPS
        && p.x <= a.x.max(b.x) + EPS
        && p.y >= a.y.min(b.y) - EPS
        && p.y <= a.y.max(b.y) + EPS
}

/// −1 outside / 0 on-boundary / +1 inside, by ray casting (half-open edge rule).
fn point_in_ring(p: Coord, ring: &[Coord]) -> i8 {
    let n = ring.len();
    if n < 2 {
        return -1;
    }
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (ring[i], ring[j]);
        if on_segment(p, a, b) {
            return 0;
        }
        if (a.y > p.y) != (b.y > p.y) {
            let x_int = a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x);
            if p.x < x_int {
                inside = !inside;
            }
        }
        j = i;
    }
    if inside {
        1
    } else {
        -1
    }
}

/// In-or-on a single polygon (exterior `rings[0]`, holes `rings[1..]`). A point
/// strictly inside a hole is outside the polygon; on a hole boundary counts in.
fn point_in_polygon(p: Coord, rings: &[Ring]) -> bool {
    match rings.first().map(|ext| point_in_ring(p, ext)) {
        Some(-1) | None => false,
        Some(0) => true,
        _ => !rings[1..].iter().any(|h| point_in_ring(p, h) == 1),
    }
}

fn point_in_any(p: Coord, polys: &[&Vec<Ring>]) -> bool {
    polys.iter().any(|rings| point_in_polygon(p, rings))
}

/// Strictly inside the filled area (interior of the exterior ring, and not on or
/// inside any hole).
fn point_strictly_in_polygon(p: Coord, rings: &[Ring]) -> bool {
    matches!(rings.first().map(|ext| point_in_ring(p, ext)), Some(1))
        && !rings[1..].iter().any(|h| point_in_ring(p, h) >= 0)
}

fn point_strictly_in_any(p: Coord, polys: &[&Vec<Ring>]) -> bool {
    polys
        .iter()
        .any(|rings| point_strictly_in_polygon(p, rings))
}

/// Do segments `p1p2` and `q1q2` intersect (including touching at an endpoint)?
fn seg_intersect(p1: Coord, p2: Coord, q1: Coord, q2: Coord) -> bool {
    let d1 = orient(q1, q2, p1);
    let d2 = orient(q1, q2, p2);
    let d3 = orient(p1, p2, q1);
    let d4 = orient(p1, p2, q2);
    if ((d1 > EPS && d2 < -EPS) || (d1 < -EPS && d2 > EPS))
        && ((d3 > EPS && d4 < -EPS) || (d3 < -EPS && d4 > EPS))
    {
        return true; // proper crossing
    }
    on_segment(p1, q1, q2)
        || on_segment(p2, q1, q2)
        || on_segment(q1, p1, p2)
        || on_segment(q2, p1, p2)
}

/// A transversal (proper, interior) crossing — boundary touching does NOT count.
fn proper_cross(p1: Coord, p2: Coord, q1: Coord, q2: Coord) -> bool {
    let d1 = orient(q1, q2, p1);
    let d2 = orient(q1, q2, p2);
    let d3 = orient(p1, p2, q1);
    let d4 = orient(p1, p2, q2);
    ((d1 > EPS && d2 < -EPS) || (d1 < -EPS && d2 > EPS))
        && ((d3 > EPS && d4 < -EPS) || (d3 < -EPS && d4 > EPS))
}

// ---------------------------------------------------------------------------
// Relations
// ---------------------------------------------------------------------------

/// Test a topological relation; `None` = undefined (a `GeometryCollection` operand).
pub(crate) fn relate(rel: Rel, a: &Geometry, b: &Geometry) -> Option<bool> {
    if matches!(a, Geometry::GeometryCollection(_)) || matches!(b, Geometry::GeometryCollection(_))
    {
        return None;
    }
    Some(match rel {
        Rel::Contains => contains(a, b),
        Rel::Within => contains(b, a),
        Rel::Intersects => intersects(a, b),
        Rel::Disjoint => !intersects(a, b),
        Rel::Equals => equals(a, b),
    })
}

/// `a` contains `b`: `b` ⊆ filled `a`. Requires `a` areal. Approximate for the
/// polygon/polygon case (sound for nested territories — the data here); exact
/// for the polygon ⊇ point path.
fn contains(a: &Geometry, b: &Geometry) -> bool {
    let Some(polys) = polygons(a) else {
        return false;
    };
    let bcoords = all_coords(b);
    if bcoords.is_empty() {
        return false;
    }
    // (1) every vertex of b is in-or-on a.
    if !bcoords.iter().all(|&p| point_in_any(p, &polys)) {
        return false;
    }
    // (2) no edge of b leaves a: its midpoint must stay in-or-on a, and it must
    // not transversally cross an a-edge. Midpoint sampling catches a b-edge that
    // exits and re-enters a through vertex/collinear touches (e.g. spanning the
    // mouth of a concavity), which the crossing test alone misses.
    let a_edges = edges(a);
    for &(p1, p2) in &edges(b) {
        let mid = Coord {
            x: (p1.x + p2.x) / 2.0,
            y: (p1.y + p2.y) / 2.0,
        };
        if !point_in_any(mid, &polys) {
            return false;
        }
        if a_edges.iter().any(|&(q1, q2)| proper_cross(p1, p2, q1, q2)) {
            return false;
        }
    }
    // (3) areal b must not engulf a hole of a: a vertex of one of a's holes
    // strictly inside b means b covers void that a doesn't fill (donut case).
    if let Some(bpolys) = polygons(b) {
        for rings in &polys {
            if rings[1..]
                .iter()
                .flatten()
                .any(|&h| point_strictly_in_any(h, &bpolys))
            {
                return false;
            }
        }
    }
    true
}

fn intersects(a: &Geometry, b: &Geometry) -> bool {
    let (ca, cb) = (all_coords(a), all_coords(b));
    if ca.is_empty() || cb.is_empty() || !bbox_overlap(a, b) {
        return false;
    }
    // A vertex of one inside the areal other.
    if let Some(pa) = polygons(a) {
        if cb.iter().any(|&p| point_in_any(p, &pa)) {
            return true;
        }
    }
    if let Some(pb) = polygons(b) {
        if ca.iter().any(|&p| point_in_any(p, &pb)) {
            return true;
        }
    }
    let (ea, eb) = (edges(a), edges(b));
    // Edge × edge (covers crossing + boundary touching).
    if ea
        .iter()
        .any(|&(p1, p2)| eb.iter().any(|&(q1, q2)| seg_intersect(p1, p2, q1, q2)))
    {
        return true;
    }
    // Vertex of one ON an edge of the other (point↔line/boundary contact).
    if ca
        .iter()
        .any(|&p| eb.iter().any(|&(q1, q2)| on_segment(p, q1, q2)))
        || cb
            .iter()
            .any(|&q| ea.iter().any(|&(p1, p2)| on_segment(q, p1, p2)))
    {
        return true;
    }
    // Coincident vertices (point↔point).
    ca.iter().any(|&p| {
        cb.iter()
            .any(|&q| (p.x - q.x).abs() <= EPS && (p.y - q.y).abs() <= EPS)
    })
}

/// Structural equality within ε: same geometry kind and the same multiset of
/// (rounded) vertices. Documented as approximate — not full point-set `sfEquals`.
fn equals(a: &Geometry, b: &Geometry) -> bool {
    if std::mem::discriminant(a) != std::mem::discriminant(b) {
        return false;
    }
    let norm = |g: &Geometry| -> Vec<(i64, i64)> {
        let mut v: Vec<(i64, i64)> = all_coords(g)
            .iter()
            .map(|c| ((c.x / EPS).round() as i64, (c.y / EPS).round() as i64))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    norm(a) == norm(b)
}

// ---------------------------------------------------------------------------
// Distance + envelope
// ---------------------------------------------------------------------------

/// Closest point on segment `ab` to `p`, and the squared planar distance to it.
fn closest_on_seg(p: Coord, a: Coord, b: Coord) -> (Coord, f64) {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= EPS {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0)
    };
    let q = Coord {
        x: a.x + t * dx,
        y: a.y + t * dy,
    };
    let (ex, ey) = (p.x - q.x, p.y - q.y);
    (q, ex * ex + ey * ey)
}

/// Minimum distance between `a` and `b`. `Degree` = planar Euclidean in degrees;
/// `Metre` = haversine great-circle on the closest pair. `None` if either is
/// empty (undefined).
pub(crate) fn distance(a: &Geometry, b: &Geometry, unit: Unit) -> Option<f64> {
    let (ca, cb) = (all_coords(a), all_coords(b));
    if ca.is_empty() || cb.is_empty() {
        return None;
    }
    if intersects(a, b) {
        return Some(0.0);
    }
    // Candidate closest points: every vertex of one against every edge of the
    // other (planar projection), plus vertex↔vertex (geometries with no edges).
    // Each candidate is SCORED by the requested metric — not always planar — so
    // the chosen pair is the one minimal in that metric. Haversine's periodic
    // sin also makes the metre score correct across the ±180° antimeridian,
    // where a raw planar longitude difference would pick the wrong pair.
    let (ea, eb) = (edges(a), edges(b));
    let metric = |p: Coord, q: Coord| -> f64 {
        match unit {
            Unit::Degree => {
                let (dx, dy) = (p.x - q.x, p.y - q.y);
                (dx * dx + dy * dy).sqrt()
            }
            Unit::Metre => haversine(p, q),
        }
    };
    let mut best = f64::INFINITY;
    let mut consider = |p: Coord, q: Coord| {
        let d = metric(p, q);
        if d < best {
            best = d;
        }
    };
    for &p in &ca {
        for &(q1, q2) in &eb {
            consider(p, closest_on_seg(p, q1, q2).0);
        }
    }
    for &q in &cb {
        for &(p1, p2) in &ea {
            consider(closest_on_seg(q, p1, p2).0, q);
        }
    }
    for &p in &ca {
        for &q in &cb {
            consider(p, q);
        }
    }
    Some(best)
}

fn haversine(p: Coord, q: Coord) -> f64 {
    let (lat1, lat2) = (p.y.to_radians(), q.y.to_radians());
    let dlat = (q.y - p.y).to_radians();
    let dlon = (q.x - p.x).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_R_M * h.sqrt().clamp(0.0, 1.0).asin()
}

/// Axis-aligned bounding box of `g` as a closed `POLYGON` (no CRS prefix), or
/// `None` for an empty geometry.
pub(crate) fn envelope_wkt(g: &Geometry) -> Option<String> {
    let (minx, miny, maxx, maxy) = bbox(g)?;
    Some(format!(
        "POLYGON(({minx} {miny}, {maxx} {miny}, {maxx} {maxy}, {minx} {maxy}, {minx} {miny}))"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(g: &Geometry) -> Coord {
        match g {
            Geometry::Point(Some(c)) => *c,
            _ => panic!("not a point"),
        }
    }

    #[test]
    fn parse_points_and_prefix() {
        assert_eq!(
            pt(&parse_wkt("POINT(5.4 49.5)").unwrap()),
            Coord { x: 5.4, y: 49.5 }
        );
        assert_eq!(
            pt(&parse_wkt("point (1 2)").unwrap()),
            Coord { x: 1.0, y: 2.0 }
        );
        assert_eq!(
            pt(&parse_wkt("POINT Z (1 2 3)").unwrap()),
            Coord { x: 1.0, y: 2.0 }
        );
        assert_eq!(
            pt(&parse_wkt("<http://www.opengis.net/def/crs/OGC/1.3/CRS84> POINT(1 2)").unwrap()),
            Coord { x: 1.0, y: 2.0 }
        );
        // EPSG axis order is NOT swapped (documented lon/lat-always policy).
        assert_eq!(
            pt(&parse_wkt("<http://www.opengis.net/def/crs/EPSG/0/4326> POINT(1 2)").unwrap()),
            Coord { x: 1.0, y: 2.0 }
        );
        assert!(matches!(
            parse_wkt("POINT EMPTY"),
            Some(Geometry::Point(None))
        ));
        assert_eq!(
            pt(&parse_wkt("POINT(1.5e2 -3E1)").unwrap()),
            Coord { x: 150.0, y: -30.0 }
        );
    }

    #[test]
    fn parse_polygons() {
        let a = parse_wkt("POLYGON((0 0,10 0,10 10,0 10,0 0))").unwrap();
        let b = parse_wkt("POLYGON((0 0,10 0,10 10,0 10))").unwrap(); // auto-closed
        assert!(equals(&a, &b));
        match parse_wkt("POLYGON((0 0,10 0,10 10,0 10,0 0),(2 2,2 4,4 4,4 2,2 2))").unwrap() {
            Geometry::Polygon(rings) => assert_eq!(rings.len(), 2),
            _ => panic!(),
        }
        match parse_wkt("MULTIPOLYGON(((0 0,1 0,1 1,0 1,0 0)),((5 5,6 5,6 6,5 6,5 5)))").unwrap() {
            Geometry::MultiPolygon(ps) => assert_eq!(ps.len(), 2),
            _ => panic!(),
        }
    }

    #[test]
    fn parse_multipoint_both_forms() {
        let a = parse_wkt("MULTIPOINT(1 2,3 4)").unwrap();
        let b = parse_wkt("MULTIPOINT((1 2),(3 4))").unwrap();
        assert!(equals(&a, &b));
    }

    #[test]
    fn parse_rejects_malformed() {
        for bad in [
            "POLYGON((1 2,3))",
            "FOO(1 2)",
            "POINT(a b)",
            "POINT(NaN 1)",
            "POINT(1 2) junk",
            "POLYGON((0 0,1 0",
            "",
        ] {
            assert!(parse_wkt(bad).is_none(), "should reject: {bad}");
        }
    }

    fn rel(r: Rel, a: &str, b: &str) -> Option<bool> {
        relate(r, &parse_wkt(a).unwrap(), &parse_wkt(b).unwrap())
    }

    #[test]
    fn within_and_contains() {
        let sq = "POLYGON((0 0,10 0,10 10,0 10,0 0))";
        assert_eq!(rel(Rel::Within, "POINT(5 5)", sq), Some(true));
        assert_eq!(rel(Rel::Within, "POINT(15 5)", sq), Some(false));
        assert_eq!(rel(Rel::Within, "POINT(0 5)", sq), Some(true)); // boundary
        assert_eq!(rel(Rel::Contains, sq, "POINT(5 5)"), Some(true));
        // hole: point inside the hole is outside the polygon.
        let holed = "POLYGON((0 0,10 0,10 10,0 10,0 0),(3 3,7 3,7 7,3 7,3 3))";
        assert_eq!(rel(Rel::Within, "POINT(5 5)", holed), Some(false));
        assert_eq!(rel(Rel::Within, "POINT(1 1)", holed), Some(true));
        // nested polygon containment.
        assert_eq!(
            rel(Rel::Contains, sq, "POLYGON((2 2,4 2,4 4,2 4,2 2))"),
            Some(true)
        );
    }

    #[test]
    fn intersects_disjoint() {
        let sq = "POLYGON((0 0,10 0,10 10,0 10,0 0))";
        assert_eq!(
            rel(Rel::Intersects, sq, "POLYGON((5 5,15 5,15 15,5 15,5 5))"),
            Some(true)
        );
        assert_eq!(
            rel(
                Rel::Intersects,
                sq,
                "POLYGON((20 20,21 20,21 21,20 21,20 20))"
            ),
            Some(false)
        );
        assert_eq!(
            rel(
                Rel::Disjoint,
                sq,
                "POLYGON((20 20,21 20,21 21,20 21,20 20))"
            ),
            Some(true)
        );
        // fully inside, no edge crossing.
        assert_eq!(
            rel(Rel::Intersects, sq, "POLYGON((2 2,3 2,3 3,2 3,2 2))"),
            Some(true)
        );
        // line crossing through.
        assert_eq!(
            rel(Rel::Intersects, sq, "LINESTRING(-1 5,11 5)"),
            Some(true)
        );
        assert_eq!(
            rel(Rel::Intersects, sq, "LINESTRING(-1 20,11 20)"),
            Some(false)
        );
    }

    #[test]
    fn equals_and_reflexive() {
        let sq = "POLYGON((0 0,10 0,10 10,0 10,0 0))";
        // different start vertex / orientation, same ring.
        assert_eq!(
            rel(Rel::Equals, sq, "POLYGON((10 10,0 10,0 0,10 0,10 10))"),
            Some(true)
        );
        assert_eq!(
            rel(Rel::Equals, sq, "POLYGON((0 0,20 0,20 20,0 20,0 0))"),
            Some(false)
        );
        assert_eq!(rel(Rel::Within, sq, sq), Some(true));
        assert_eq!(rel(Rel::Intersects, sq, sq), Some(true));
        assert_eq!(rel(Rel::Disjoint, sq, sq), Some(false));
    }

    #[test]
    fn multipolygon_member() {
        let mp = "MULTIPOLYGON(((0 0,2 0,2 2,0 2,0 0)),((10 10,12 10,12 12,10 12,10 10)))";
        assert_eq!(rel(Rel::Contains, mp, "POINT(11 11)"), Some(true));
        assert_eq!(rel(Rel::Contains, mp, "POINT(5 5)"), Some(false));
    }

    #[test]
    fn empty_geometry() {
        assert_eq!(
            rel(Rel::Disjoint, "POLYGON EMPTY", "POINT(1 1)"),
            Some(true)
        );
        assert_eq!(
            rel(Rel::Intersects, "POLYGON EMPTY", "POINT(1 1)"),
            Some(false)
        );
        assert_eq!(
            rel(
                Rel::Contains,
                "POLYGON((0 0,9 0,9 9,0 9,0 0))",
                "POINT EMPTY"
            ),
            Some(false)
        );
    }

    #[test]
    fn collection_is_type_error() {
        assert_eq!(
            relate(
                Rel::Intersects,
                &parse_wkt("GEOMETRYCOLLECTION(POINT(1 1))").unwrap(),
                &parse_wkt("POINT(1 1)").unwrap()
            ),
            None
        );
    }

    fn dist(a: &str, b: &str, u: Unit) -> Option<f64> {
        distance(&parse_wkt(a).unwrap(), &parse_wkt(b).unwrap(), u)
    }

    #[test]
    fn distances() {
        assert!((dist("POINT(0 0)", "POINT(3 4)", Unit::Degree).unwrap() - 5.0).abs() < 1e-9);
        // 1° of latitude ≈ 111195 m.
        assert!((dist("POINT(0 0)", "POINT(0 1)", Unit::Metre).unwrap() - 111195.0).abs() < 5.0);
        // Paris → Berlin ≈ 877 km.
        let d = dist("POINT(2.35 48.85)", "POINT(13.40 52.52)", Unit::Metre).unwrap();
        assert!((d - 877_000.0).abs() < 6_000.0, "paris-berlin {d}");
        // overlapping → 0.
        assert_eq!(
            dist(
                "POLYGON((0 0,10 0,10 10,0 10,0 0))",
                "POINT(5 5)",
                Unit::Metre
            ),
            Some(0.0)
        );
        assert_eq!(dist("POINT(1 1)", "POINT EMPTY", Unit::Metre), None);
    }

    #[test]
    fn envelope() {
        let e = envelope_wkt(&parse_wkt("LINESTRING(1 5,3 1,2 9)").unwrap()).unwrap();
        let g = parse_wkt(&e).unwrap();
        // bbox is x∈[1,3], y∈[1,9]; its corners contain the original points.
        assert_eq!(
            relate(Rel::Contains, &g, &parse_wkt("POINT(2 5)").unwrap()),
            Some(true)
        );
        assert_eq!(
            relate(Rel::Contains, &g, &parse_wkt("POINT(1 1)").unwrap()),
            Some(true)
        );
    }

    // Regressions for bugs found by the adversarial review.
    #[test]
    fn contains_rejects_concavity_mouth_span() {
        // A b-edge spanning the open mouth of a concavity leaves `a` between its
        // (boundary-touching) endpoints — vertex + crossing checks alone miss it.
        let slot = "POLYGON((0 0,10 0,10 10,6 10,6 3,4 3,4 10,0 10,0 0))";
        assert_eq!(
            rel(Rel::Contains, slot, "LINESTRING(4 10,6 10)"),
            Some(false)
        );
        assert_eq!(
            rel(Rel::Contains, slot, "LINESTRING(2 10,8 10)"),
            Some(false)
        );
        // A genuinely-contained inner segment is still true.
        assert_eq!(rel(Rel::Contains, slot, "LINESTRING(1 1,3 1)"), Some(true));
    }

    #[test]
    fn contains_rejects_b_bridging_a_hole() {
        let donut = "POLYGON((0 0,20 0,20 20,0 20,0 0),(8 8,12 8,12 12,8 12,8 8))";
        // b brackets the hole → not contained (its interior covers a's void).
        assert_eq!(
            rel(Rel::Contains, donut, "POLYGON((4 4,16 4,16 16,4 16,4 4))"),
            Some(false)
        );
        assert_eq!(
            rel(Rel::Within, "POLYGON((4 4,16 4,16 16,4 16,4 4))", donut),
            Some(false)
        );
        // A small b in the solid annulus is still contained.
        assert_eq!(
            rel(Rel::Contains, donut, "POLYGON((1 1,3 1,3 3,1 3,1 1))"),
            Some(true)
        );
    }

    #[test]
    fn distance_metre_ranks_by_great_circle() {
        // Off the equator the planar-nearest vertex is not the geodesic-nearest:
        // (11.8,60) is ~100 km away, (10,61.2) ~133 km; metre must pick 100 km.
        let d = dist("POINT(10 60)", "MULTIPOINT(11.8 60, 10 61.2)", Unit::Metre).unwrap();
        assert!((d - 100_072.0).abs() < 300.0, "off-equator nearest: {d}");
        // Across the antimeridian, (-179.5,0) is 1° away, (178,0) is 1.5°.
        let d = dist("POINT(179.5 0)", "MULTIPOINT(-179.5 0, 178 0)", Unit::Metre).unwrap();
        assert!((d - 111_195.0).abs() < 100.0, "antimeridian nearest: {d}");
    }

    #[test]
    fn parser_rejects_malformed_extra_ordinate() {
        // A malformed extra (Z/M) ordinate must fail the parse, not be swallowed.
        assert!(parse_wkt("POINT(1 2 9.9.9)").is_none());
        assert!(parse_wkt("LINESTRING(0 0,1 1 9.9.9,2 2)").is_none());
        // A well-formed Z ordinate is still accepted (and dropped).
        assert!(parse_wkt("POINT Z (1 2 3)").is_some());
    }
}
