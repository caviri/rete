"""Point clouds from a graph — LAS, LAZ, and cloud-native COPC.

COPC (Cloud Optimized Point Cloud) is the modern, efficient, cloud-native point
format: a single ``.copc.laz`` file whose points are organised into a clustered
octree, so a viewer can HTTP-range-fetch just the nodes it needs at just the
level of detail it needs — the point-cloud analogue of PMTiles, and the same
idea as ``.rete`` itself. A billion-point scan on a static host answers a
bounded, level-limited query in a few megabytes.

Reading LAZ/COPC needs ``laspy`` with the ``lazrs`` backend — pip-installable, a
small Rust wheel, but not bundled (like ifcopenshell for IFC). With it:

* a **COPC** URL is read through laspy's ``CopcReader``, which streams octree
  nodes over HTTP range and lets us cap detail by octree level — so a remote
  cloud never downloads more than the requested level of detail;
* plain **LAS/LAZ** are read whole and decimated to a point budget.

Without laspy, uncompressed **LAS** still works through a small pure-Python
parser here; LAZ/COPC degrade with a clear "install laspy[lazrs]" message.

Point clouds are ordinary Blender meshes (one vertex per point, colour as a
point attribute), so — unlike splats — the builder places them directly; this
module only reads and builds, recentring on the cloud's centroid so it lands at
the placement point.
"""

from __future__ import annotations

import struct
from typing import List, Optional, Sequence, Tuple

LAS_HINT = (
    "Reading LAZ/COPC needs laspy with the lazrs backend: "
    "`<blender-python> -m pip install \"laspy[lazrs]\"`. Uncompressed .las works "
    "without it."
)

#: Default decimation budget — how many points a build keeps.
MAX_POINTS = 500_000

RGBA = Tuple[float, float, float, float]


def is_copc_name(url: str) -> bool:
    return url.split("#", 1)[0].lower().endswith(".copc.laz")


# --------------------------------------------------------------- laspy path


def _laspy():
    try:
        import laspy  # noqa: PLC0415

        return laspy
    except Exception:
        return None


def _colors_from(points, count_hint: int) -> Optional[List[RGBA]]:
    """RGBA per point from a laspy point record, or ``None`` if uncoloured."""
    dims = set(getattr(points, "point_format", None).dimension_names) if hasattr(points, "point_format") else set()
    if not {"red", "green", "blue"} <= set(dims):
        return None
    red = points.red
    green = points.green
    blue = points.blue
    hi = max(int(red.max()), int(green.max()), int(blue.max())) if len(red) else 0
    scale = 255.0 if hi <= 255 else 65535.0
    return [
        (red[i] / scale, green[i] / scale, blue[i] / scale, 1.0) for i in range(len(red))
    ]


def _decimate(n: int, limit: int) -> int:
    return max(1, n // limit) if limit and n > limit else 1


def _open_copc(laspy, path: str, url: str, is_remote: bool):
    """A ``CopcReader`` over a local file or a remote HTTP-range URL.

    ``CopcReader.open`` wires up HTTP range streaming for an ``http(s)`` URL;
    a local file is opened and handed in directly.
    """
    if is_remote:
        return laspy.CopcReader.open(url)
    # The reader keeps reading from this handle during queries, so it is left
    # open deliberately (closed when the build's process ends).
    return laspy.CopcReader(open(path, "rb"))


def _read_copc(reader, limit: int):
    """Level-limited COPC read: accumulate octree levels until the budget is hit.

    Only the nodes for the levels touched are fetched, so a remote COPC transfers
    a fraction of the file. Returns ``(positions, colors, note)``.
    """
    positions: List[Tuple[float, float, float]] = []
    colors: List[RGBA] = []
    coloured = None
    depth = 0
    while len(positions) < limit and depth < 24:
        try:
            pts = reader.level_query(depth)
        except Exception:
            break
        if pts is None or len(pts.x) == 0:
            if depth == 0:
                depth += 1
                continue
            break
        xs, ys, zs = pts.x, pts.y, pts.z
        cols = _colors_from(pts, len(xs))
        if coloured is None:
            coloured = cols is not None
        for i in range(len(xs)):
            positions.append((float(xs[i]), float(ys[i]), float(zs[i])))
            if cols is not None:
                colors.append(cols[i])
        depth += 1
    note = f"COPC level-of-detail read: {len(positions):,} points to octree depth {depth - 1}"
    return positions, (colors if coloured else None), note


def _read_las_laspy(laspy, path: str, limit: int):
    las = laspy.read(path)
    n = len(las.x)
    step = _decimate(n, limit)
    xs, ys, zs = las.x, las.y, las.z
    cols = _colors_from(las, n)
    positions = [(float(xs[i]), float(ys[i]), float(zs[i])) for i in range(0, n, step)]
    colors = [cols[i] for i in range(0, n, step)] if cols is not None else None
    note = f"{len(positions):,} of {n:,} points" + ("" if step == 1 else f" (1 in {step})")
    return positions, colors, note


# ----------------------------------------------------- pure-Python LAS

# Point Data Record Format -> byte offset of the red channel, if the format
# carries RGB. Others have no colour.
_RGB_OFFSET = {2: 20, 3: 28, 5: 28, 7: 30, 8: 30, 10: 30}


def parse_las(path: str, limit: int = MAX_POINTS):
    """Uncompressed LAS -> ``(positions, colors)``; the no-laspy fallback.

    Reads the public header for the point layout and scales, then strides through
    the fixed-size records. LAZ is compressed and not handled here.
    """
    with open(path, "rb") as fh:
        header = fh.read(375)  # enough for the 1.4 public header block
        if header[:4] != b"LASF":
            raise IOError("not a LAS file")
        point_offset = struct.unpack_from("<I", header, 96)[0]
        fmt = header[104] & 0x3F
        record_len = struct.unpack_from("<H", header, 105)[0]
        legacy_count = struct.unpack_from("<I", header, 107)[0]
        scale = struct.unpack_from("<3d", header, 131)
        offset = struct.unpack_from("<3d", header, 155)
        count = legacy_count
        if count == 0 and len(header) >= 255:  # LAS 1.4 extended count
            count = struct.unpack_from("<Q", header, 247)[0]
        if fmt & 0x80 or record_len == 0:
            raise IOError("compressed (LAZ) — " + LAS_HINT)

        rgb_off = _RGB_OFFSET.get(fmt)
        step = _decimate(count, limit)
        fh.seek(point_offset)
        data = fh.read(record_len * count)

    positions: List[Tuple[float, float, float]] = []
    colors: List[RGBA] = []
    hi = 0
    raw_rgb: List[Tuple[int, int, int]] = []
    for i in range(0, count, step):
        base = i * record_len
        rec = data[base:base + record_len]
        if len(rec) < 12:
            break
        xi, yi, zi = struct.unpack_from("<iii", rec, 0)
        positions.append((xi * scale[0] + offset[0], yi * scale[1] + offset[1], zi * scale[2] + offset[2]))
        if rgb_off is not None and len(rec) >= rgb_off + 6:
            r, g, b = struct.unpack_from("<HHH", rec, rgb_off)
            raw_rgb.append((r, g, b))
            hi = max(hi, r, g, b)
    if raw_rgb:
        div = 255.0 if hi <= 255 else 65535.0
        colors = [(r / div, g / div, b / div, 1.0) for r, g, b in raw_rgb]
    return positions, (colors if colors else None)


# ------------------------------------------------------------- building


def build_point_cloud_mesh(name: str, positions, colors):
    """A Blender mesh: one vertex per point, colour as a point attribute.

    Recentred on its own centroid so it lands at the placement point (this is a
    plain mesh, so moving the vertices is fine). Returns an unlinked object.
    """
    import bpy

    if not positions:
        return None
    n = len(positions)
    cx = sum(p[0] for p in positions) / n
    cy = sum(p[1] for p in positions) / n
    cz = sum(p[2] for p in positions) / n
    verts = [(p[0] - cx, p[1] - cy, p[2] - cz) for p in positions]

    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(verts, [], [])
    mesh.update()
    if colors:
        attr = mesh.attributes.new(name="point_color", type="FLOAT_COLOR", domain="POINT")
        flat: List[float] = []
        for c in colors:
            flat.extend(c)
        attr.data.foreach_set("color", flat)
    obj = bpy.data.objects.new(name, mesh)
    obj["rete:pointCloud"] = True
    obj["rete:pointCount"] = n
    return obj


def import_points(url: str, path: str, *, is_remote: bool, limit: int = MAX_POINTS):
    """Read a point cloud into a Blender object. Returns ``(objects, note)``.

    ``path`` is a local file (already fetched) for LAS/LAZ; for a **remote COPC**
    the URL is streamed directly so only the requested detail is transferred.
    Raises ``IOError`` only when nothing can be read (LAZ/COPC without laspy).
    """
    laspy = _laspy()
    is_copc = is_copc_name(url)

    if laspy is not None and is_copc:
        try:
            reader = _open_copc(laspy, path, url, is_remote)
            positions, colors, note = _read_copc(reader, limit)
        except Exception as exc:
            raise IOError(f"COPC read failed for {url}: {exc}") from exc
    elif laspy is not None:
        positions, colors, note = _read_las_laspy(laspy, path, limit)
    else:
        # No laspy: only uncompressed LAS is readable here.
        ext = path.lower()
        if ext.endswith(".las"):
            positions, colors = parse_las(path, limit)
            note = f"{len(positions):,} points (built-in LAS reader) — install laspy[lazrs] for LAZ/COPC"
        else:
            raise IOError(f"{url}: {LAS_HINT}")

    label = _name_for(url)
    obj = build_point_cloud_mesh(label, positions, colors)
    if obj is None:
        raise IOError(f"no points read from {url}")
    obj["rete:pointSource"] = url
    return [obj], note


def _name_for(url: str) -> str:
    base = url.split("#", 1)[0].rsplit("/", 1)[-1]
    for suffix in (".copc.laz", ".laz", ".las"):
        if base.lower().endswith(suffix):
            return base[: -len(suffix)] or "points"
    return base or "points"
