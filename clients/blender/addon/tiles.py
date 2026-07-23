"""PMTiles in Blender: a range-readable map archive becomes scene geometry.

A `.pmtiles` file is a whole tiled map in one immutable, HTTP-range-readable
file — the same idea as ``.rete``, for maps. This module reads one directly
(header, directory tree, a single tile) fetching only the byte ranges a build
touches, decodes the vector tiles (Mapbox Vector Tiles — protobuf), and turns
the features into Blender meshes: roads and boundaries as lines, land and water
as filled (optionally extruded) polygons, one object per layer.

Everything here is pure Python and standard-library only — a minimal varint and
protobuf reader, the PMTiles v3 directory format, and the Web Mercator inverse —
so nothing new is bundled into the extension. ``bpy`` is imported lazily inside
:func:`build_map`, leaving the reader and decoder testable on their own.
"""

from __future__ import annotations

import gzip
import math
import struct
import urllib.request
from typing import Dict, Iterable, List, Optional, Sequence, Tuple

USER_AGENT = "rete-blender (+https://github.com/caviri/rete)"

# PMTiles tile-type codes (header byte 99).
TYPE_MVT, TYPE_PNG, TYPE_JPEG, TYPE_WEBP, TYPE_AVIF = 1, 2, 3, 4, 5
_TYPE_NAME = {1: "mvt", 2: "png", 3: "jpeg", 4: "webp", 5: "avif"}
_RASTER_EXT = {TYPE_PNG: ".png", TYPE_JPEG: ".jpg", TYPE_WEBP: ".webp", TYPE_AVIF: ".avif"}

# Internal/tile compression codes (header bytes 97/98).
_COMP_NONE, _COMP_GZIP = 1, 2


# --------------------------------------------------------------------- varint


def _read_varint(buf: bytes, pos: int) -> Tuple[int, int]:
    """Decode one LEB128 unsigned varint; return ``(value, next_pos)``."""
    result = 0
    shift = 0
    while True:
        byte = buf[pos]
        pos += 1
        result |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return result, pos
        shift += 7


def _zigzag(value: int) -> int:
    return (value >> 1) ^ -(value & 1)


# --------------------------------------------------------- byte sources (range)


class _FileSource:
    """A local ``.pmtiles`` read by seek — no download, no whole-file load."""

    def __init__(self, path: str):
        self._path = path
        import os

        self.size = os.path.getsize(path)

    def read(self, offset: int, length: int) -> bytes:
        with open(self._path, "rb") as fh:
            fh.seek(offset)
            return fh.read(length)


class _RangeSource:
    """A remote ``.pmtiles`` read over HTTP range requests.

    Only the header, the directory pages, and the tiles a build touches are
    fetched — a continent's worth of boundaries answers in a few hundred KB.
    """

    def __init__(self, url: str):
        self._url = url
        self.size = 1 << 62  # unknown; range reads never need the total

    def read(self, offset: int, length: int) -> bytes:
        request = urllib.request.Request(
            self._url,
            headers={"User-Agent": USER_AGENT, "Range": f"bytes={offset}-{offset + length - 1}"},
        )
        with urllib.request.urlopen(request, timeout=60) as response:
            data = response.read()
        if len(data) < length and response.status not in (200, 206):
            raise IOError(f"short range read from {self._url}")
        return data


def _source(url_or_path: str):
    if url_or_path.startswith(("http://", "https://")):
        return _RangeSource(url_or_path)
    path = url_or_path[7:] if url_or_path.startswith("file://") else url_or_path
    return _FileSource(path)


# ------------------------------------------------------------- PMTiles reader


class DirEntry:
    __slots__ = ("tile_id", "offset", "length", "run_length")

    def __init__(self, tile_id: int, offset: int, length: int, run_length: int):
        self.tile_id = tile_id
        self.offset = offset
        self.length = length
        self.run_length = run_length


class PMTiles:
    """A PMTiles v3 archive opened for reading over a byte source."""

    def __init__(self, url_or_path: str):
        self.source = _source(url_or_path)
        self._parse_header()
        self._root = self._read_directory(self.root_offset, self.root_length)

    def _parse_header(self) -> None:
        head = self.source.read(0, 127)
        if head[:7] != b"PMTiles":
            raise IOError("not a PMTiles file (bad magic)")
        if head[7] != 3:
            raise IOError(f"unsupported PMTiles version {head[7]} (need v3)")
        v = struct.unpack("<11Q", head[8:96])
        (self.root_offset, self.root_length, self.meta_offset, self.meta_length,
         self.leaf_offset, self.leaf_length, self.tile_offset, self.tile_length,
         self.num_addressed, self.num_entries, self.num_contents) = v
        self.internal_compression = head[97]
        self.tile_compression = head[98]
        self.tile_type = head[99]
        self.min_zoom = head[100]
        self.max_zoom = head[101]

    def _decompress_internal(self, data: bytes) -> bytes:
        if self.internal_compression == _COMP_GZIP:
            return gzip.decompress(data)
        return data

    def _read_directory(self, offset: int, length: int) -> List[DirEntry]:
        buf = self._decompress_internal(self.source.read(offset, length))
        pos = 0
        n, pos = _read_varint(buf, pos)
        entries = [DirEntry(0, 0, 0, 0) for _ in range(n)]

        last_id = 0
        for e in entries:
            delta, pos = _read_varint(buf, pos)
            last_id += delta
            e.tile_id = last_id
        for e in entries:
            e.run_length, pos = _read_varint(buf, pos)
        for e in entries:
            e.length, pos = _read_varint(buf, pos)
        for i, e in enumerate(entries):
            raw, pos = _read_varint(buf, pos)
            if raw == 0 and i > 0:
                prev = entries[i - 1]
                e.offset = prev.offset + prev.length
            else:
                e.offset = raw - 1
        return entries

    def metadata(self) -> Dict[str, object]:
        import json

        raw = self.source.read(self.meta_offset, self.meta_length)
        try:
            return json.loads(self._decompress_internal(raw))
        except Exception:
            return {}

    def is_vector(self) -> bool:
        return self.tile_type == TYPE_MVT

    def type_name(self) -> str:
        return _TYPE_NAME.get(self.tile_type, str(self.tile_type))

    def raster_suffix(self) -> str:
        return _RASTER_EXT.get(self.tile_type, ".bin")

    def _find(self, entries: List[DirEntry], tile_id: int) -> Optional[DirEntry]:
        """Largest entry with ``tile_id <= target`` (binary search)."""
        lo, hi = 0, len(entries) - 1
        found = None
        while lo <= hi:
            mid = (lo + hi) // 2
            if entries[mid].tile_id <= tile_id:
                found = entries[mid]
                lo = mid + 1
            else:
                hi = mid - 1
        return found

    def tile(self, z: int, x: int, y: int) -> Optional[bytes]:
        """The decompressed bytes of one tile, or ``None`` if it is absent."""
        tile_id = zxy_to_tileid(z, x, y)
        entries = self._root
        for _ in range(4):  # root + up to a few leaf levels
            entry = self._find(entries, tile_id)
            if entry is None:
                return None
            if entry.run_length == 0:
                # A leaf-directory pointer: descend.
                entries = self._read_directory(self.leaf_offset + entry.offset, entry.length)
                continue
            if tile_id >= entry.tile_id + entry.run_length:
                return None
            raw = self.source.read(self.tile_offset + entry.offset, entry.length)
            if self.tile_compression == _COMP_GZIP:
                try:
                    return gzip.decompress(raw)
                except OSError:
                    return raw
            return raw
        return None


# ------------------------------------------------------- Hilbert tile ids


def zxy_to_tileid(z: int, x: int, y: int) -> int:
    """The PMTiles Hilbert-curve tile id for ``z/x/y``."""
    acc = 0
    for t in range(z):
        acc += (1 << t) * (1 << t)
    n = 1 << z
    rx = ry = 0
    d = 0
    tx, ty = x, y
    s = n >> 1
    while s > 0:
        rx = 1 if (tx & s) > 0 else 0
        ry = 1 if (ty & s) > 0 else 0
        d += s * s * ((3 * rx) ^ ry)
        if ry == 0:
            if rx == 1:
                tx = s - 1 - tx
                ty = s - 1 - ty
            tx, ty = ty, tx
        s >>= 1
    return acc + d


# --------------------------------------------------------- Web Mercator


def lonlat_to_tile(lon: float, lat: float, z: int) -> Tuple[int, int]:
    n = 1 << z
    x = int((lon + 180.0) / 360.0 * n)
    lat_r = math.radians(max(-85.05112878, min(85.05112878, lat)))
    y = int((1.0 - math.asinh(math.tan(lat_r)) / math.pi) / 2.0 * n)
    return (min(n - 1, max(0, x)), min(n - 1, max(0, y)))


def tile_pixel_to_lonlat(tx: int, ty: int, px: float, py: float, z: int, extent: int) -> Tuple[float, float]:
    """A tile-local coordinate (0..extent) to geographic lon/lat."""
    n = 1 << z
    wx = (tx + px / extent) / n
    wy = (ty + py / extent) / n
    lon = wx * 360.0 - 180.0
    lat = math.degrees(math.atan(math.sinh(math.pi * (1.0 - 2.0 * wy))))
    return (lon, lat)


def tiles_for_bbox(bbox: Tuple[float, float, float, float], z: int, cap: int = 256) -> List[Tuple[int, int, int]]:
    """Every ``z/x/y`` covering a ``(min_lon, min_lat, max_lon, max_lat)`` box."""
    min_lon, min_lat, max_lon, max_lat = bbox
    x0, y1 = lonlat_to_tile(min_lon, min_lat, z)
    x1, y0 = lonlat_to_tile(max_lon, max_lat, z)
    x0, x1 = sorted((x0, x1))
    y0, y1 = sorted((y0, y1))
    out = []
    for ty in range(y0, y1 + 1):
        for tx in range(x0, x1 + 1):
            out.append((z, tx, ty))
            if len(out) >= cap:
                return out
    return out


# ---------------------------------------------------------- MVT decoding

# Geometry types (MVT spec).
_GEOM_POINT, _GEOM_LINE, _GEOM_POLYGON = 1, 2, 3


class Feature:
    __slots__ = ("layer", "geom_type", "rings", "props")

    def __init__(self, layer: str, geom_type: int, rings: List[List[Tuple[float, float]]], props: Dict):
        self.layer = layer
        self.geom_type = geom_type
        self.rings = rings  # each ring: list of (lon, lat)
        self.props = props


def _read_field(buf: bytes, pos: int) -> Tuple[int, int, int]:
    """Read a protobuf tag; return ``(field_number, wire_type, next_pos)``."""
    tag, pos = _read_varint(buf, pos)
    return tag >> 3, tag & 0x7, pos


def _skip(buf: bytes, pos: int, wire_type: int) -> int:
    if wire_type == 0:
        _, pos = _read_varint(buf, pos)
    elif wire_type == 2:
        length, pos = _read_varint(buf, pos)
        pos += length
    elif wire_type == 5:
        pos += 4
    elif wire_type == 1:
        pos += 8
    return pos


def _decode_value(buf: bytes) -> object:
    pos = 0
    while pos < len(buf):
        field, wire, pos = _read_field(buf, pos)
        if field == 1 and wire == 2:  # string
            length, pos = _read_varint(buf, pos)
            return buf[pos:pos + length].decode("utf-8", "replace")
        if field == 2 and wire == 5:  # float
            return struct.unpack("<f", buf[pos:pos + 4])[0]
        if field == 3 and wire == 1:  # double
            return struct.unpack("<d", buf[pos:pos + 8])[0]
        if field in (4, 5) and wire == 0:  # int64 / uint64
            v, pos = _read_varint(buf, pos)
            return v
        if field == 6 and wire == 0:  # sint64
            v, pos = _read_varint(buf, pos)
            return _zigzag(v)
        if field == 7 and wire == 0:  # bool
            v, pos = _read_varint(buf, pos)
            return bool(v)
        pos = _skip(buf, pos, wire)
    return None


def _decode_geometry(commands: Sequence[int], tx: int, ty: int, z: int, extent: int) -> List[List[Tuple[float, float]]]:
    """MVT geometry commands to rings of ``(lon, lat)``."""
    rings: List[List[Tuple[float, float]]] = []
    current: List[Tuple[float, float]] = []
    cx = cy = 0
    i = 0
    n = len(commands)
    while i < n:
        cmd_int = commands[i]
        i += 1
        cmd = cmd_int & 0x7
        count = cmd_int >> 3
        if cmd == 1:  # MoveTo
            for _ in range(count):
                cx += _zigzag(commands[i]); cy += _zigzag(commands[i + 1]); i += 2
                if current:
                    rings.append(current)
                current = [tile_pixel_to_lonlat(tx, ty, cx, cy, z, extent)]
        elif cmd == 2:  # LineTo
            for _ in range(count):
                cx += _zigzag(commands[i]); cy += _zigzag(commands[i + 1]); i += 2
                current.append(tile_pixel_to_lonlat(tx, ty, cx, cy, z, extent))
        elif cmd == 7:  # ClosePath
            if current:
                current.append(current[0])
    if current:
        rings.append(current)
    return rings


def _decode_layer(buf: bytes, tx: int, ty: int, z: int) -> Iterable[Feature]:
    name = ""
    extent = 4096
    keys: List[str] = []
    values: List[object] = []
    feature_blobs: List[bytes] = []

    pos = 0
    while pos < len(buf):
        field, wire, pos = _read_field(buf, pos)
        if field == 1 and wire == 2:
            length, pos = _read_varint(buf, pos)
            name = buf[pos:pos + length].decode("utf-8", "replace"); pos += length
        elif field == 2 and wire == 2:
            length, pos = _read_varint(buf, pos)
            feature_blobs.append(buf[pos:pos + length]); pos += length
        elif field == 3 and wire == 2:
            length, pos = _read_varint(buf, pos)
            keys.append(buf[pos:pos + length].decode("utf-8", "replace")); pos += length
        elif field == 4 and wire == 2:
            length, pos = _read_varint(buf, pos)
            values.append(_decode_value(buf[pos:pos + length])); pos += length
        elif field == 5 and wire == 0:
            extent, pos = _read_varint(buf, pos)
        else:
            pos = _skip(buf, pos, wire)

    for blob in feature_blobs:
        feat = _decode_feature(blob, name, keys, values, tx, ty, z, extent)
        if feat is not None:
            yield feat


def _decode_feature(buf, name, keys, values, tx, ty, z, extent) -> Optional[Feature]:
    geom_type = 0
    tags: List[int] = []
    geometry: List[int] = []
    pos = 0
    while pos < len(buf):
        field, wire, pos = _read_field(buf, pos)
        if field == 2 and wire == 2:  # packed tags
            length, pos = _read_varint(buf, pos)
            end = pos + length
            while pos < end:
                v, pos = _read_varint(buf, pos)
                tags.append(v)
        elif field == 3 and wire == 0:
            geom_type, pos = _read_varint(buf, pos)
        elif field == 4 and wire == 2:  # packed geometry
            length, pos = _read_varint(buf, pos)
            end = pos + length
            while pos < end:
                v, pos = _read_varint(buf, pos)
                geometry.append(v)
        else:
            pos = _skip(buf, pos, wire)

    if not geometry:
        return None
    rings = _decode_geometry(geometry, tx, ty, z, extent)
    if not rings:
        return None
    props = {}
    for i in range(0, len(tags) - 1, 2):
        ki, vi = tags[i], tags[i + 1]
        if ki < len(keys) and vi < len(values):
            props[keys[ki]] = values[vi]
    return Feature(name, geom_type, rings, props)


def decode_tile(data: bytes, tx: int, ty: int, z: int) -> List[Feature]:
    """Decode one MVT tile's bytes into features (geometry in lon/lat)."""
    features: List[Feature] = []
    pos = 0
    while pos < len(data):
        field, wire, pos = _read_field(data, pos)
        if field == 3 and wire == 2:  # a Layer
            length, pos = _read_varint(data, pos)
            features.extend(_decode_layer(data[pos:pos + length], tx, ty, z))
            pos += length
        else:
            pos = _skip(data, pos, wire)
    return features


# --------------------------------------------------------- building a scene

WORLD_BBOX = (-180.0, -85.05112878, 180.0, 85.05112878)


def pick_zoom(archive: "PMTiles", bbox: Tuple[float, float, float, float], budget: int = 40) -> int:
    """The highest zoom whose tile count over ``bbox`` stays within ``budget``."""
    best = archive.min_zoom
    for z in range(archive.min_zoom, archive.max_zoom + 1):
        if len(tiles_for_bbox(bbox, z, cap=budget + 1)) <= budget:
            best = z
        else:
            break
    return best


def build_map(
    url: str,
    *,
    placement,
    bbox: Optional[Tuple[float, float, float, float]] = None,
    zoom: int = -1,
    max_tiles: int = 40,
    extrude: float = 0.0,
    max_verts: int = 2_000_000,
    collection=None,
    name: str = "map",
):
    """Read a ``.pmtiles`` map and build one Blender mesh per vector layer.

    ``placement`` is the geographic :class:`~.geometry.Placement` that lon/lat
    coordinates are projected through, so the map lands in the same frame as any
    other geographic layer in the scene. Returns ``(objects, note)``.
    """
    import bpy

    archive = PMTiles(url)
    box = bbox or WORLD_BBOX
    if not archive.is_vector():
        return _build_raster_map(archive, bpy, placement=placement, bbox=box,
                                 zoom=zoom, max_tiles=max_tiles, collection=collection, name=name)

    z = zoom if zoom >= 0 else pick_zoom(archive, box, budget=max_tiles)
    z = max(archive.min_zoom, min(archive.max_zoom, z))
    coords = tiles_for_bbox(box, z, cap=max_tiles)

    # layer -> {"verts": [...], "edges": [...], "faces": [...]}
    layers: Dict[str, Dict[str, list]] = {}
    total_verts = 0
    truncated = False
    for (tz, tx, ty) in coords:
        data = archive.tile(tz, tx, ty)
        if not data:
            continue
        for feat in decode_tile(data, tx, ty, tz):
            if total_verts >= max_verts:
                truncated = True
                break
            bucket = layers.setdefault(feat.layer, {"verts": [], "edges": [], "faces": []})
            total_verts += _add_feature(bucket, feat, placement)
        if truncated:
            break

    objects = []
    for layer_name, bucket in layers.items():
        if not bucket["verts"]:
            continue
        obj = _mesh_object(bpy, f"{name}:{layer_name}", bucket, collection, extrude=extrude)
        if obj is not None:
            obj["rete:mapLayer"] = layer_name
            obj["rete:mapSource"] = url
            objects.append(obj)

    note = ""
    if truncated:
        note = f"map truncated at {max_verts:,} vertices — lower the zoom or narrow the extent"
    elif not objects:
        note = f"no features in {len(coords)} tile(s) at zoom {z} over the chosen extent"
    return objects, note


def _add_feature(bucket: Dict[str, list], feat: "Feature", placement) -> int:
    """Append one feature's geometry to a layer bucket; return verts added."""
    added = 0
    for ring in feat.rings:
        pts = [placement.apply((lon, lat, 0.0)) for lon, lat in ring]
        if feat.geom_type == _GEOM_POLYGON and len(pts) >= 4:
            if pts[0] == pts[-1]:
                pts = pts[:-1]
            if len(pts) < 3:
                continue
            base = len(bucket["verts"])
            bucket["verts"].extend(pts)
            bucket["faces"].append(list(range(base, base + len(pts))))
            added += len(pts)
        elif feat.geom_type == _GEOM_LINE and len(pts) >= 2:
            base = len(bucket["verts"])
            bucket["verts"].extend(pts)
            bucket["edges"].extend((base + i, base + i + 1) for i in range(len(pts) - 1))
            added += len(pts)
        elif feat.geom_type == _GEOM_POINT:
            bucket["verts"].extend(pts)
            added += len(pts)
    return added


def _mesh_object(bpy, name, bucket, collection, *, extrude=0.0):
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(bucket["verts"], bucket["edges"], bucket["faces"])
    mesh.update()
    obj = bpy.data.objects.new(name, mesh)
    if collection is not None:
        collection.objects.link(obj)
    if extrude and bucket["faces"]:
        _extrude_faces(mesh, extrude)
    return obj


def _extrude_faces(mesh, height: float) -> None:
    import bmesh

    bm = bmesh.new()
    bm.from_mesh(mesh)
    faces = list(bm.faces)
    if faces:
        result = bmesh.ops.extrude_face_region(bm, geom=faces)
        moved = [v for v in result["geom"] if isinstance(v, bmesh.types.BMVert)]
        bmesh.ops.translate(bm, verts=moved, vec=(0.0, 0.0, height))
    bm.to_mesh(mesh)
    bm.free()


def _build_raster_map(archive, bpy, *, placement, bbox, zoom, max_tiles, collection, name):
    """Raster PMTiles: one textured plane per tile, at its geographic quad."""
    import os
    import tempfile

    from . import materials

    z = zoom if zoom >= 0 else pick_zoom(archive, bbox, budget=max_tiles)
    z = max(archive.min_zoom, min(archive.max_zoom, z))
    coords = tiles_for_bbox(bbox, z, cap=max_tiles)
    suffix = archive.raster_suffix()
    directory = tempfile.mkdtemp(prefix="rete-tiles-")

    objects = []
    for (tz, tx, ty) in coords:
        data = archive.tile(tz, tx, ty)
        if not data:
            continue
        path = os.path.join(directory, f"{tz}_{tx}_{ty}{suffix}")
        with open(path, "wb") as fh:
            fh.write(data)
        try:
            image = bpy.data.images.load(path, check_existing=True)
        except RuntimeError:
            continue
        # The tile's geographic quad -> a flat plane in world space.
        c00 = placement.apply((*tile_pixel_to_lonlat(tx, ty, 0, 0, tz, 1), 0.0)[:2] + (0.0,))
        corners = [
            placement.apply((*tile_pixel_to_lonlat(tx, ty, cx, cy, tz, 1), 0.0)[:2] + (0.0,))
            for cx, cy in ((0, 1), (1, 1), (1, 0), (0, 0))
        ]
        mesh = bpy.data.meshes.new(f"{name}:{tz}/{tx}/{ty}")
        mesh.from_pydata(corners, [], [[0, 1, 2, 3]])
        mesh.uv_layers.new(name="UVMap")
        for loop in mesh.uv_layers[0].data:
            pass  # default UVs cover the quad; good enough for a map tile
        mesh.update()
        obj = bpy.data.objects.new(mesh.name, mesh)
        if collection is not None:
            collection.objects.link(obj)
        mat = materials.textured(path)
        if mat is not None:
            materials.assign(obj, mat)
        obj["rete:mapSource"] = archive.__class__.__name__
        objects.append(obj)
    note = "" if objects else "no raster tiles over the chosen extent"
    return objects, note
