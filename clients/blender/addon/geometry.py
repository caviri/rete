"""RDF geometry literals to Blender space.

Handles the shapes that actually appear in ``.rete`` graphs: OGC WKT (with and
without a ``Z`` coordinate, optionally prefixed by a CRS IRI), the ``BOX3D``
axis-aligned bounding boxes rete's geo3 vocabulary emits, and bare numeric
x/y/z columns.

Two coordinate problems have to be solved before anything can be placed:

* **Projection.** Geographic literals are degrees of longitude/latitude, which
  are useless as Blender coordinates. They are projected to metres on an
  equirectangular around a reference point (accurate enough for a city or a
  country, and it keeps north up and shapes recognisable).
* **Scale and orientation.** Datasets are authored in millimetres (anatomy),
  metres (buildings), or degrees (maps), on either a Z-up or a Y-up axis.
  :class:`Placement` folds unit scale, axis convention, an X flip and a recentre
  into one transform applied to every coordinate.
"""

from __future__ import annotations

import math
import re
from typing import List, Optional, Sequence, Tuple

Vec3 = Tuple[float, float, float]

#: Mean Earth radius (m) — the equirectangular projection's only constant.
EARTH_R = 6371008.8

_NUM = r"[-+]?(?:\d+\.?\d*|\.\d+)(?:[eE][-+]?\d+)?"
_COORD_RE = re.compile(_NUM)
# A leading CRS IRI is legal in a geo:wktLiteral: "<http://...CRS84> POINT(...)"
_CRS_RE = re.compile(r"^\s*<[^>]*>\s*")
_TAG_RE = re.compile(
    r"^\s*(POINT|MULTIPOINT|LINESTRING|MULTILINESTRING|POLYGON|MULTIPOLYGON|"
    r"GEOMETRYCOLLECTION|TRIANGLE|POLYHEDRALSURFACE|TIN|BOX3D|BOX)\s*(ZM|Z|M)?",
    re.IGNORECASE,
)

#: Geometry kinds this module produces.
POINT, LINE, AREA, BOX = "POINT", "LINE", "AREA", "BOX"


class Geometry:
    """A parsed geometry: a kind, one or more coordinate rings, and a centroid.

    ``rings`` is a list of coordinate sequences — a single point is one ring of
    one coordinate, a polygon is one ring per exterior boundary. Interior rings
    (holes) are dropped: none of the published datasets rely on them, and an
    ngon with a hole cannot be built without a triangulation pass that would
    cost more than it buys here.
    """

    __slots__ = ("kind", "rings", "closed")

    def __init__(self, kind: str, rings: List[List[Vec3]], closed: bool = False):
        self.kind = kind
        self.rings = rings
        self.closed = closed

    @property
    def coords(self) -> List[Vec3]:
        return [c for ring in self.rings for c in ring]

    @property
    def centroid(self) -> Vec3:
        pts = self.coords
        if not pts:
            return (0.0, 0.0, 0.0)
        n = len(pts)
        return (
            sum(p[0] for p in pts) / n,
            sum(p[1] for p in pts) / n,
            sum(p[2] for p in pts) / n,
        )

    @property
    def bounds(self) -> Tuple[Vec3, Vec3]:
        pts = self.coords or [(0.0, 0.0, 0.0)]
        return (
            (min(p[0] for p in pts), min(p[1] for p in pts), min(p[2] for p in pts)),
            (max(p[0] for p in pts), max(p[1] for p in pts), max(p[2] for p in pts)),
        )

    @property
    def size(self) -> Vec3:
        lo, hi = self.bounds
        return (hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2])

    def __repr__(self) -> str:
        return f"Geometry({self.kind}, rings={len(self.rings)}, pts={len(self.coords)})"


def _triples(nums: Sequence[float], dims: int) -> List[Vec3]:
    """Group a flat number run into xyz triples, padding Z when absent."""
    out: List[Vec3] = []
    for i in range(0, len(nums) - dims + 1, dims):
        x, y = nums[i], nums[i + 1]
        z = nums[i + 2] if dims >= 3 else 0.0
        out.append((x, y, z))
    return out


def parse(text: str) -> Optional[Geometry]:
    """Parse a WKT or BOX3D literal. Returns ``None`` if it is not geometry."""
    if not text:
        return None
    body = _CRS_RE.sub("", text)
    m = _TAG_RE.match(body)
    if not m:
        return None
    tag = m.group(1).upper()
    zm = (m.group(2) or "").upper()
    rest = body[m.end() :]

    if tag in ("BOX3D", "BOX"):
        nums = [float(n) for n in _COORD_RE.findall(rest)]
        if len(nums) >= 6:
            lo, hi = (nums[0], nums[1], nums[2]), (nums[3], nums[4], nums[5])
        elif len(nums) >= 4:
            lo, hi = (nums[0], nums[1], 0.0), (nums[2], nums[3], 0.0)
        else:
            return None
        return Geometry(BOX, [[lo, hi]])

    # Dimensionality: an explicit Z/ZM tag, otherwise inferred per group below.
    dims = 3 if zm in ("Z", "ZM") else 0
    groups = _split_groups(rest)
    if not groups:
        return None

    rings: List[List[Vec3]] = []
    for group in groups:
        nums = [float(n) for n in _COORD_RE.findall(group)]
        if not nums:
            continue
        d = dims or _infer_dims(group, len(nums))
        pts = _triples(nums, d)
        if pts:
            rings.append(pts)
    if not rings:
        return None

    if tag in ("POINT", "MULTIPOINT"):
        kind, closed = POINT, False
    elif tag in ("LINESTRING", "MULTILINESTRING"):
        kind, closed = LINE, False
    else:
        kind, closed = AREA, True
    return Geometry(kind, rings, closed)


def _infer_dims(group: str, count: int) -> int:
    """Numbers per coordinate, from the first comma-separated tuple."""
    first = group.split(",", 1)[0]
    per = len(_COORD_RE.findall(first))
    if per >= 3:
        return 3
    if per == 2:
        return 2
    return 3 if count % 3 == 0 else 2


def _split_groups(text: str) -> List[str]:
    """Split a WKT body into its innermost parenthesised coordinate groups.

    ``POLYGON((a),(b))`` yields the two rings; ``POINT(a)`` yields one group.
    Only groups containing a digit are kept, so empty geometries drop out.
    """
    groups: List[str] = []
    depth = 0
    start = -1
    for i, ch in enumerate(text):
        if ch == "(":
            depth += 1
            start = i + 1 if depth >= 1 else start
            if depth >= 1:
                start = i + 1
        elif ch == ")":
            if depth >= 1 and start >= 0:
                chunk = text[start:i]
                if "(" not in chunk and any(c.isdigit() for c in chunk):
                    groups.append(chunk)
            depth -= 1
            start = -1
    if not groups and any(c.isdigit() for c in text):
        groups.append(text)
    return groups


#: Datatypes that assert a *local* 3D frame. rete's geo3 does no CRS
#: reprojection by design, so its literals are never degrees.
LOCAL_3D_DATATYPES = frozenset(
    (
        "https://w3id.org/rete/geo3#wktLiteral3D",
        "https://w3id.org/rete/geo3#box3dLiteral",
    )
)


def looks_geographic(coords: Sequence[Vec3]) -> bool:
    """True when every coordinate falls inside the lon/lat envelope.

    Necessary but **not sufficient** — see :func:`is_geographic`. Plenty of
    local data lives inside ±180/±90: a football pitch is 105 × 68 metres, and
    projecting that as degrees scatters it over half a continent.
    """
    if not coords:
        return False
    return all(abs(c[0]) <= 180.0 and abs(c[1]) <= 90.0 for c in coords)


def is_geographic(
    coords: Sequence[Vec3],
    *,
    from_wkt: bool = False,
    lonlat_named: bool = False,
    datatypes: Sequence[str] = (),
) -> bool:
    """Whether coordinates should be projected from degrees to metres.

    Range alone cannot decide this, so positive evidence is required: the
    coordinates came out of a WKT literal, or the columns they came from are
    named longitude and latitude. Bare ``x``/``y`` columns are treated as a
    local frame, which is what they nearly always are.
    """
    if any(dt in LOCAL_3D_DATATYPES for dt in datatypes):
        return False
    if not (from_wkt or lonlat_named):
        return False
    return looks_geographic(coords)


class Placement:
    """The transform from dataset coordinates to Blender world coordinates.

    Built once per import from the scene settings and applied to every
    coordinate, so a whole result set lands consistently.
    """

    def __init__(
        self,
        *,
        scale: float = 1.0,
        axis_up: str = "Z",
        flip_x: bool = False,
        offset: Vec3 = (0.0, 0.0, 0.0),
        geographic: bool = False,
        ref_lon: float = 0.0,
        ref_lat: float = 0.0,
    ):
        self.scale = scale or 1.0
        self.axis_up = axis_up
        self.flip_x = flip_x
        self.offset = offset
        self.geographic = geographic
        self.ref_lon = ref_lon
        self.ref_lat = ref_lat

    def project(self, c: Vec3) -> Vec3:
        """Degrees to metres, east/north/up, around the reference point."""
        if not self.geographic:
            return c
        x = math.radians(c[0] - self.ref_lon) * EARTH_R * math.cos(math.radians(self.ref_lat))
        y = math.radians(c[1] - self.ref_lat) * EARTH_R
        return (x, y, c[2])

    def apply(self, c: Vec3) -> Vec3:
        x, y, z = self.project(c)
        x -= self.offset[0]
        y -= self.offset[1]
        z -= self.offset[2]
        if self.axis_up == "Y":
            # glTF-style Y-up to Blender Z-up: (x, y, z) -> (x, -z, y)
            x, y, z = x, -z, y
        if self.flip_x:
            x = -x
        s = self.scale
        return (x * s, y * s, z * s)

    def apply_all(self, coords: Sequence[Vec3]) -> List[Vec3]:
        return [self.apply(c) for c in coords]

    def apply_size(self, s: Vec3) -> Vec3:
        """Scale an extent (no translation, no axis flip sign)."""
        k = self.scale
        if self.axis_up == "Y":
            return (abs(s[0]) * k, abs(s[2]) * k, abs(s[1]) * k)
        return (abs(s[0]) * k, abs(s[1]) * k, abs(s[2]) * k)


def _projected_bounds(coords: Sequence[Vec3], placement: Placement) -> Optional[Tuple[Vec3, Vec3]]:
    lo = [float("inf")] * 3
    hi = [float("-inf")] * 3
    for c in coords:
        p = placement.project(c)
        for i in range(3):
            lo[i] = min(lo[i], p[i])
            hi[i] = max(hi[i], p[i])
    if lo[0] == float("inf"):
        return None
    return (tuple(lo), tuple(hi))  # type: ignore[return-value]


def fit_scale(coords: Sequence[Vec3], target: float, placement: Placement) -> float:
    """Uniform scale putting the whole set inside a ``target``-metre cube."""
    bounds = _projected_bounds(coords, placement)
    if bounds is None:
        return 1.0
    lo, hi = bounds
    extent = max(hi[i] - lo[i] for i in range(3))
    return 1.0 if extent <= 1e-9 else target / extent


def centre_of(coords: Sequence[Vec3], placement: Placement) -> Vec3:
    """Mid-point of the projected bounding box — the recentre offset."""
    bounds = _projected_bounds(coords, placement)
    if bounds is None:
        return (0.0, 0.0, 0.0)
    lo, hi = bounds
    return tuple((lo[i] + hi[i]) / 2.0 for i in range(3))  # type: ignore[return-value]


def reference_lonlat(coords: Sequence[Vec3]) -> Tuple[float, float]:
    """Mean lon/lat of a geographic set — the projection's tangent point."""
    if not coords:
        return (0.0, 0.0)
    return (
        sum(c[0] for c in coords) / len(coords),
        sum(c[1] for c in coords) / len(coords),
    )
