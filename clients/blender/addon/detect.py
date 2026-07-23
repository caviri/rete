"""Working out what a query's columns *mean*, so a scene can be built from them.

The add-on deliberately knows no dataset's vocabulary. A column earns a role
from evidence — mostly the shape of its values, with the variable name as a
tie-breaker — so an arbitrary SPARQL query against an arbitrary ``.rete`` file
still produces a sensible scene, and the user only has to intervene when the
guess is wrong.

Value evidence outranks name evidence: a column called ``?model`` holding
``POINT Z(...)`` is geometry, whatever it is called.
"""

from __future__ import annotations

import os
import re
from typing import Dict, List, Optional, Sequence, Tuple
from urllib.parse import urlparse

from . import geometry

# Roles a column can play when building the scene.
ENTITY = "ENTITY"
LABEL = "LABEL"
ASSET = "ASSET"
MESH_NODE = "MESH_NODE"
GEOMETRY = "GEOMETRY"
IMAGE = "IMAGE"
COLOR = "COLOR"
TIME = "TIME"
TIME_END = "TIME_END"
NUMBER = "NUMBER"
CLASS = "CLASS"
TEXT = "TEXT"
VIDEO = "VIDEO"
MAP = "MAP"
SPLAT = "SPLAT"
POINTS = "POINTS"
IGNORE = "IGNORE"

ROLE_ITEMS = [
    ("AUTO", "Auto", "Use the detected role"),
    (ENTITY, "Entity", "The IRI identifying the thing this row is about"),
    (LABEL, "Label", "Human-readable name — becomes the object name"),
    (ASSET, "3D asset", "URL of a 3D model to import (.glb, .gltf, .obj, .ifc, …)"),
    (MESH_NODE, "Mesh node", "Name of the node to isolate inside a shared asset"),
    (GEOMETRY, "Geometry", "WKT or BOX3D literal — becomes position and shape"),
    (IMAGE, "Image", "Image or IIIF URL — a texture, image plane, or 360° world"),
    (VIDEO, "Video", "Video URL — a movie-textured plane synced to the timeline"),
    (MAP, "Map (PMTiles)", "A .pmtiles map — becomes vector or raster map geometry"),
    (SPLAT, "Gaussian splat", "A 3DGS .ply/.splat — via the 3DGS add-on, or a preview"),
    (POINTS, "Point cloud", "A LAS/LAZ/COPC point cloud — becomes a coloured point mesh"),
    (COLOR, "Colour", "Colour literal — becomes the material's base colour"),
    (TIME, "Time", "A date, time or number placing the row on the timeline"),
    (TIME_END, "Time (end)", "End of the row's interval on the timeline"),
    (NUMBER, "Number", "Numeric value — drivable, and usable for scale/colour"),
    (CLASS, "Class", "The type of the thing — groups and colours the scene"),
    (TEXT, "Text", "Kept as a custom property"),
    (IGNORE, "Ignore", "Left out of the scene"),
]

#: Video container extensions Blender can load as a movie image datablock.
VIDEO_EXT = {".mp4", ".webm", ".mov", ".mkv", ".m4v", ".ogv", ".avi"}

#: Gaussian-splat container extensions. ``.ply`` is deliberately absent — it is
#: ambiguous (mesh or splat), and is disambiguated by sniffing the file's header
#: at import time (:func:`assets.is_splat_ply`), so a splat ``.ply`` still routes
#: to the splat path even though it detects as a plain 3D asset.
SPLAT_EXT = {".splat", ".ksplat", ".spz"}

#: Point-cloud container extensions. ``.copc.laz`` (Cloud Optimized Point Cloud)
#: ends in ``.laz`` and is told apart by its full name / header at import.
POINTCLOUD_EXT = {".las", ".laz"}

#: File extensions Blender can import, mapped to the importer family. The CAD/BIM
#: formats (ifc, dxf, step) need an external importer and are handled specially
#: in :mod:`.assets`, but they are asset URLs all the same.
MODEL_EXT = {
    ".glb": "gltf", ".gltf": "gltf",
    ".obj": "obj",
    ".fbx": "fbx",
    ".stl": "stl",
    ".ply": "ply",
    ".usd": "usd", ".usda": "usd", ".usdc": "usd", ".usdz": "usd",
    ".abc": "alembic",
    ".dae": "collada",
    ".x3d": "x3d", ".wrl": "x3d",
    ".svg": "svg",
    ".blend": "blend",
    # CAD / BIM
    ".ifc": "ifc", ".ifczip": "ifc", ".ifcxml": "ifc",
    ".dxf": "dxf",
    ".step": "step", ".stp": "step",
}

#: The CAD/BIM families, which need an external importer (ifcopenshell / Bonsai,
#: or a bundled add-on) rather than a core ``bpy.ops`` operator.
CAD_FAMILIES = frozenset(("ifc", "dxf", "step"))
IMAGE_EXT = {
    ".jpg", ".jpeg", ".png", ".webp", ".tif", ".tiff",
    ".exr", ".bmp", ".tga", ".hdr", ".jp2",
}

_XSD = "http://www.w3.org/2001/XMLSchema#"
NUMERIC_DT = {
    _XSD + t
    for t in (
        "integer", "decimal", "double", "float", "long", "int", "short", "byte",
        "nonNegativeInteger", "nonPositiveInteger", "negativeInteger",
        "positiveInteger", "unsignedLong", "unsignedInt", "unsignedShort",
        "unsignedByte",
    )
}
TEMPORAL_DT = {
    _XSD + t
    for t in ("date", "dateTime", "dateTimeStamp", "gYear", "gYearMonth", "time", "duration")
}
WKT_DT = (
    "http://www.opengis.net/ont/geosparql#wktLiteral",
    "https://w3id.org/rete/geo3#wktLiteral3D",
    "https://w3id.org/rete/geo3#box3dLiteral",
)

# Name evidence. Checked against the lower-cased variable name and, when the
# column came from a predicate, that predicate's local name. Order matters: the
# end-of-interval patterns are tested before the general temporal ones, or
# "endTime" would come back as a start.
_NAME_HINTS: List[Tuple[str, str]] = [
    (r"^(mesh|glb|gltf|model3d|asset|geometryfile|glbfile|animation)$", ASSET),
    (r"(meshnode|nodename|meshname)", MESH_NODE),
    (r"^(pmtiles|basemap|tiles|maptiles)$|(pmtiles)", MAP),
    (r"^(splat|splats|gaussian|gaussiansplat|3dgs|radiance)$", SPLAT),
    (r"^(pointcloud|points|lidar|copc|lasfile|laz)$|(pointcloud|lidar)", POINTS),
    (r"^(video|movie|clip|footage|reel)$", VIDEO),
    (r"^(wkt|geom|geometry|aswkt|aswkt3d|box|box3d|bbox|shape|location|coords?)$", GEOMETRY),
    (r"(thumbnail|depiction|image|photo|picture|iiif|cover|scan|poster)", IMAGE),
    (r"(colou?r|rgb|hex)$", COLOR),
    (r"^(label|name|title|prefl?abel|caption)$", LABEL),
    (r"^(class|type|category|kind|rdftype)$", CLASS),
    (r"^(end|until|death|dissolved|destroyed)|^to$", TIME_END),
    (r"^(start|begin|since|inception|birth|created|issued|from)", TIME),
    (r"(date|time|year|when|timestamp|frame|epoch)", TIME),
    # Bare conventional names for a time axis, exact-match only so that "ts" as
    # a time does not swallow a column called "tsunamis".
    (r"^(t|ts|sec|secs|seconds|millis|ms|elapsed|offset)$", TIME),
    (r"^(id|iri|uri|url|s|subject|entity|item|thing|node)$", ENTITY),
]

#: Variable names that mean "this number is a coordinate". Datasets that publish
#: positions as separate numeric columns (rather than a WKT literal) are common
#: enough — motion tracking, sensor logs, plain CSV-derived graphs — that the
#: axes are worth recognising by name.
AXIS_NAMES: Dict[str, int] = {
    "x": 0, "lon": 0, "long": 0, "longitude": 0, "easting": 0, "cx": 0,
    "y": 1, "lat": 1, "latitude": 1, "northing": 1, "cy": 1,
    "z": 2, "alt": 2, "altitude": 2, "elev": 2, "elevation": 2, "height": 2, "cz": 2,
}

_HEX_RE = re.compile(r"^#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$")
_RGB_RE = re.compile(r"^rgba?\(\s*[\d.]+[,\s]+[\d.]+[,\s]+[\d.]+", re.IGNORECASE)
_ISO_RE = re.compile(r"^-?\d{3,4}-\d{2}(-\d{2})?([T ]\d{2}:\d{2})?")
_YEAR_RE = re.compile(r"^-?\d{1,4}$")
_IIIF_RE = re.compile(r"/(full|square|pct:|\d+,\d+)/[^/]+/\d+/(default|native|color|gray)\.")

#: Predicate IRIs whose meaning we know for certain. The heuristics get most of
#: these right anyway; pinning them makes the published datasets exact — and
#: pins the cases the heuristics would get *wrong*, notably the datasets that
#: express time as bare decimal seconds, which otherwise read as plain numbers.
KNOWN_PREDICATES: Dict[str, str] = {
    # 3D assets
    "https://w3id.org/rete/anatomy#glbFile": ASSET,
    "https://3d.si.edu/prop/mesh": ASSET,
    "https://bioexplora.cat/prop/mesh": ASSET,
    "https://w3id.org/rete/dance#animation": ASSET,
    "https://w3id.org/rete/anatomy#meshNode": MESH_NODE,
    # CAD / BIM assets. glbModel is the shipped predicate; ifcModel/ifcFile are
    # pinned so a graph pointing straight at a raw .ifc imports without relying
    # on the URL suffix alone.
    "https://w3id.org/rete/cad#glbModel": ASSET,
    "https://w3id.org/rete/cad#ifcModel": ASSET,
    "https://w3id.org/rete/cad#ifcFile": ASSET,
    "https://w3id.org/rete/media#splat": SPLAT,
    "https://w3id.org/rete/media#gaussianSplat": SPLAT,
    "https://w3id.org/rete/media#pointCloud": POINTS,
    "https://w3id.org/rete/media#copc": POINTS,
    "https://w3id.org/rete/media#lidar": POINTS,
    "https://w3id.org/rete/cad#ifcClass": CLASS,
    "https://w3id.org/rete/cad#elevation": NUMBER,
    "https://w3id.org/rete/cad#netArea": NUMBER,
    "https://w3id.org/rete/cad#grossVolume": NUMBER,
    # geometry
    "https://w3id.org/rete/geo3#asWKT3D": GEOMETRY,
    "https://w3id.org/rete/geo3#box": GEOMETRY,
    "http://www.opengis.net/ont/geosparql#asWKT": GEOMETRY,
    "http://www.opengis.net/ont/geosparql#hasGeometry": GEOMETRY,
    "https://w3id.org/rete/geoadmin/prop/geomFine": GEOMETRY,
    "https://w3id.org/rete/geo3#x": NUMBER,
    "https://w3id.org/rete/geo3#y": NUMBER,
    "https://w3id.org/rete/geo3#z": NUMBER,
    "http://www.w3.org/2003/01/geo/wgs84_pos#lat": NUMBER,
    "http://www.w3.org/2003/01/geo/wgs84_pos#long": NUMBER,
    "http://rs.tdwg.org/dwc/terms/decimalLatitude": NUMBER,
    "http://rs.tdwg.org/dwc/terms/decimalLongitude": NUMBER,
    # labels and types
    "http://www.w3.org/2000/01/rdf-schema#label": LABEL,
    "http://schema.org/name": LABEL,
    "http://purl.org/dc/terms/title": LABEL,
    "http://www.w3.org/2004/02/skos/core#prefLabel": LABEL,
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#type": CLASS,
    "https://w3id.org/rete/anatomy#tissueType": CLASS,
    "https://w3id.org/rete/cad#ifcClass": CLASS,
    # images
    "http://xmlns.com/foaf/0.1/depiction": IMAGE,
    "http://schema.org/image": IMAGE,
    "https://schema.org/image": IMAGE,
    "http://schema.org/thumbnailUrl": IMAGE,
    "https://bioexplora.cat/prop/preview": IMAGE,
    "https://bioexplora.cat/prop/thumbnail": IMAGE,
    "https://scrollprize.org/vocab#thumbnail": IMAGE,
    "https://w3id.org/rete/lombardi/momaImage": IMAGE,
    # time — the decimal-seconds vocabularies must be pinned, since a bare
    # "4.000" is indistinguishable from any other number by value alone.
    "http://purl.org/dc/terms/date": TIME,
    "http://schema.org/startDate": TIME,
    "http://schema.org/endDate": TIME_END,
    "http://www.w3.org/2006/time#hasBeginning": TIME,
    "http://www.w3.org/2006/time#hasEnd": TIME_END,
    "https://w3id.org/rete/subtitles#start": TIME,
    "https://w3id.org/rete/subtitles#end": TIME_END,
    "https://w3id.org/rete/dance#startTime": TIME,
    "https://w3id.org/rete/dance#endTime": TIME_END,
    "https://w3id.org/rete/tracking#t": TIME,
    "https://w3id.org/rete/worldcup#minute": TIME,
    "http://www.w3.org/ns/prov#generatedAtTime": TIME,
    "http://rs.tdwg.org/dwc/terms/eventDate": TIME,
}


def url_extension(value: str) -> str:
    """Lower-cased file extension of a URL or path, ignoring query and fragment."""
    if not value:
        return ""
    path = urlparse(value).path if "://" in value else value
    return os.path.splitext(path)[1].lower()


def is_model_url(value: str) -> bool:
    return url_extension(value) in MODEL_EXT


def is_video_url(value: str) -> bool:
    return url_extension(value) in VIDEO_EXT


def is_map_url(value: str) -> bool:
    return url_extension(value) == ".pmtiles"


def is_splat_url(value: str) -> bool:
    """A splat-only container. ``.ply`` is not here — it is content-sniffed."""
    return url_extension(value) in SPLAT_EXT


def is_pointcloud_url(value: str) -> bool:
    """A LAS/LAZ/COPC point cloud (``.copc.laz`` ends in ``.laz``)."""
    return url_extension(value) in POINTCLOUD_EXT


def is_image_url(value: str) -> bool:
    if url_extension(value) in IMAGE_EXT:
        return True
    # IIIF Image API URLs carry no extension in some profiles.
    return bool(_IIIF_RE.search(value or ""))


def is_color(value: str) -> bool:
    v = (value or "").strip()
    return bool(_HEX_RE.match(v) or _RGB_RE.match(v))


def is_temporal(value: str, datatype: str = "") -> bool:
    if datatype in TEMPORAL_DT:
        return True
    v = (value or "").strip()
    return bool(_ISO_RE.match(v))


def is_number(value: str, datatype: str = "") -> bool:
    if datatype in NUMERIC_DT:
        return True
    try:
        float(value)
        return True
    except (TypeError, ValueError):
        return False


def classify_cell(cell) -> Optional[str]:
    """The role one value argues for, or ``None`` if it argues for nothing."""
    if cell is None:
        return None
    value, dt = cell.value, cell.datatype
    if cell.kind == "iri":
        if is_map_url(value):
            return MAP
        if is_pointcloud_url(value):
            return POINTS
        if is_splat_url(value):
            return SPLAT
        if is_model_url(value):
            return ASSET
        if is_video_url(value):
            return VIDEO
        if is_image_url(value):
            return IMAGE
        return None  # an IRI alone does not say whether it is entity or class
    if dt in WKT_DT or geometry.parse(value) is not None:
        return GEOMETRY
    if is_map_url(value):
        return MAP
    if is_pointcloud_url(value):
        return POINTS
    if is_splat_url(value):
        return SPLAT
    if is_model_url(value):
        return ASSET
    if is_video_url(value):
        return VIDEO
    if is_image_url(value):
        return IMAGE
    if is_color(value):
        return COLOR
    if dt in TEMPORAL_DT:
        return TIME
    if is_number(value, dt):
        return NUMBER
    if is_temporal(value, dt):
        return TIME
    return None


def name_role(name: str) -> Optional[str]:
    n = (name or "").lower().lstrip("?")
    n = re.sub(r"[^a-z0-9]", "", n)
    for pattern, role in _NAME_HINTS:
        if re.search(pattern, n):
            return role
    return None


def classify_column(
    name: str,
    cells: Sequence,
    *,
    predicate: str = "",
    sample: int = 60,
) -> str:
    """The role for a whole column, from a sample of its values.

    A role needs a majority of the non-empty sample to agree, so one stray
    number in a text column does not turn it numeric.
    """
    if predicate and predicate in KNOWN_PREDICATES:
        return KNOWN_PREDICATES[predicate]

    votes: Dict[str, int] = {}
    seen = 0
    iri_count = 0
    for cell in cells[:sample]:
        if cell is None:
            continue
        seen += 1
        if cell.kind == "iri":
            iri_count += 1
        role = classify_cell(cell)
        if role:
            votes[role] = votes.get(role, 0) + 1

    if seen:
        best = max(votes.items(), key=lambda kv: kv[1], default=None)
        if best and best[1] >= max(1, seen // 2):
            role = best[0]
            # Value evidence normally wins, with one exception: a column of bare
            # numbers named like a time really is a time. Several datasets
            # publish seconds as plain decimals, and read as numbers they would
            # never reach the timeline.
            if role == NUMBER:
                hinted = name_role(name)
                if hinted in (TIME, TIME_END):
                    return hinted
            return role
        if iri_count >= max(1, seen // 2):
            # All IRIs and nothing more specific: entity unless named like a type.
            hinted = name_role(name)
            return CLASS if hinted == CLASS else ENTITY

    hinted = name_role(name)
    if hinted:
        return hinted
    return TEXT if seen else IGNORE


_PREFIX_RE = re.compile(r"PREFIX\s+([A-Za-z][\w.\-]*)?\s*:\s*<([^>]*)>", re.IGNORECASE)
_IRI_VAR_RE = re.compile(r"<([^>\s]+)>\s*\?([A-Za-z_]\w*)")
_QNAME_VAR_RE = re.compile(r"(?<![<\w:/#])([A-Za-z][\w.\-]*)?:([\w.\-]+)\s+\?([A-Za-z_]\w*)")
_TYPE_VAR_RE = re.compile(r"(?<![\w?])a\s+\?([A-Za-z_]\w*)")

RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"


def predicates_from_query(sparql: str) -> Dict[str, str]:
    """Map result variables to the predicate that binds them, read off the query.

    A SELECT result carries no link between a column and the predicate it came
    from, so without this the known-predicate table could never fire. Scanning
    the query text recovers the link for the shapes that actually get written —
    ``?s <iri> ?var``, ``?s prefix:local ?var``, and ``?s a ?var`` — which is
    enough to pin the vocabularies whose values are ambiguous on their own,
    above all the ones publishing time as a bare decimal.
    """
    if not sparql:
        return {}
    prefixes = {(m.group(1) or ""): m.group(2) for m in _PREFIX_RE.finditer(sparql)}

    found: Dict[str, str] = {}
    for match in _IRI_VAR_RE.finditer(sparql):
        found.setdefault(match.group(2), match.group(1))
    for match in _QNAME_VAR_RE.finditer(sparql):
        base = prefixes.get(match.group(1) or "")
        if base:
            found.setdefault(match.group(3), base + match.group(2))
    for match in _TYPE_VAR_RE.finditer(sparql):
        found.setdefault(match.group(1), RDF_TYPE)
    return found


def classify_result(result, predicates: Optional[Dict[str, str]] = None) -> Dict[str, str]:
    """Detected role per variable for a whole result set.

    When no predicate map is supplied, one is recovered from the result's own
    query text.
    """
    if predicates is None:
        predicates = predicates_from_query(getattr(result, "query", "") or "")
    return {
        var: classify_column(var, result.column(var), predicate=predicates.get(var, ""))
        for var in result.vars
    }


class Binding:
    """Which column fills which role when building the scene.

    Resolved once per import from the detected roles plus any user override, so
    the builder never has to re-decide mid-run.
    """

    def __init__(self, roles: Dict[str, str], order: Sequence[str]):
        self.roles = dict(roles)
        self.order = list(order)

    def first(self, *wanted: str) -> Optional[str]:
        for var in self.order:
            if self.roles.get(var) in wanted:
                return var
        return None

    def all_of(self, *wanted: str) -> List[str]:
        return [v for v in self.order if self.roles.get(v) in wanted]

    @property
    def entity(self) -> Optional[str]:
        return self.first(ENTITY)

    @property
    def label(self) -> Optional[str]:
        return self.first(LABEL)

    @property
    def asset(self) -> Optional[str]:
        return self.first(ASSET)

    @property
    def mesh_node(self) -> Optional[str]:
        return self.first(MESH_NODE)

    @property
    def image(self) -> Optional[str]:
        return self.first(IMAGE)

    @property
    def video(self) -> Optional[str]:
        return self.first(VIDEO)

    @property
    def maps(self) -> List[str]:
        return self.all_of(MAP)

    @property
    def splat(self) -> Optional[str]:
        return self.first(SPLAT)

    @property
    def points(self) -> Optional[str]:
        return self.first(POINTS)

    @property
    def color(self) -> Optional[str]:
        return self.first(COLOR)

    @property
    def klass(self) -> Optional[str]:
        return self.first(CLASS)

    @property
    def time(self) -> Optional[str]:
        return self.first(TIME)

    @property
    def time_end(self) -> Optional[str]:
        return self.first(TIME_END)

    @property
    def numbers(self) -> List[str]:
        return self.all_of(NUMBER)

    @property
    def xyz(self) -> List[str]:
        """Numeric columns that name coordinate axes, ordered X, Y, Z.

        Returns ``[]`` unless at least two axes are present — one lone column
        called ``x`` is more likely a measurement than a position.
        """
        found: Dict[int, str] = {}
        for var in self.order:
            if self.roles.get(var) != NUMBER:
                continue
            axis = AXIS_NAMES.get(re.sub(r"[^a-z0-9]", "", var.lower().lstrip("?")))
            if axis is not None and axis not in found:
                found[axis] = var
        if len(found) < 2:
            return []
        return [found.get(i, "") for i in range(3)]

    @property
    def geometries(self) -> List[str]:
        """Geometry columns, positional ones before bounding boxes.

        A row often carries both a centroid and a box; the centroid places the
        object and the box sizes it, so the order matters.
        """
        geoms = self.all_of(GEOMETRY)
        return sorted(geoms, key=lambda v: 1 if "box" in v.lower() or "bbox" in v.lower() else 0)


def resolve(result, roles: Dict[str, str], overrides: Optional[Dict[str, str]] = None) -> Binding:
    """Merge detected roles with user overrides, then pick the entity column.

    If nothing was detected as the entity, the first IRI column becomes it —
    without an identity there is nothing to hang properties on.
    """
    merged = dict(roles)
    for var, role in (overrides or {}).items():
        if role and role != "AUTO":
            merged[var] = role

    if not any(r == ENTITY for r in merged.values()):
        # The fallback identity is the first plain IRI column — but not one that
        # already plays a content role (a map/asset/video/image URL is an IRI
        # too, and consuming it as the entity would drop the content).
        content = {MAP, ASSET, VIDEO, IMAGE, SPLAT, POINTS}
        for var in result.vars:
            if merged.get(var) in content:
                continue
            cells = [c for c in result.column(var) if c is not None]
            if cells and all(c.kind == "iri" for c in cells[:20]):
                merged[var] = ENTITY
                break
    return Binding(merged, result.vars)
