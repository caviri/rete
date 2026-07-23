"""The time dimension: RDF temporal literals onto Blender's timeline.

Graphs date things in wildly different registers — a subtitle cue at 12.4
seconds, a football player's position at 40 ms intervals, a manuscript "c.
1180", a law commenced in 2024. All of them reduce to a number on one axis, and
that axis is then mapped linearly onto the scene's frame range.

Parsing is deliberately forgiving: partial ISO dates, negative (BCE) years, bare
years, ``xsd:duration`` and plain numbers all resolve, because refusing a date
means an object silently never appears.
"""

from __future__ import annotations

import math
import re
from typing import Dict, Iterable, List, Optional, Sequence, Tuple

import bpy

SECONDS_PER_DAY = 86400.0
DAYS_PER_YEAR = 365.2425

_DATE_RE = re.compile(
    r"^\s*(?P<sign>-)?(?P<year>\d{1,6})"
    r"(?:-(?P<month>\d{1,2}))?"
    r"(?:-(?P<day>\d{1,2}))?"
    r"(?:[T ](?P<hour>\d{1,2}):(?P<minute>\d{2})(?::(?P<second>\d{2}(?:\.\d+)?))?)?"
)
_TIME_RE = re.compile(r"^\s*(?P<hour>\d{1,2}):(?P<minute>\d{2})(?::(?P<second>\d{2}(?:\.\d+)?))?")
_DURATION_RE = re.compile(
    r"^\s*(?P<sign>-)?P(?:(?P<y>[\d.]+)Y)?(?:(?P<mo>[\d.]+)M)?(?:(?P<d>[\d.]+)D)?"
    r"(?:T(?:(?P<h>[\d.]+)H)?(?:(?P<mi>[\d.]+)M)?(?:(?P<s>[\d.]+)S)?)?\s*$"
)

_XSD = "http://www.w3.org/2001/XMLSchema#"

#: Datatypes that genuinely denote a date. Anything else typed — decimal,
#: integer, double — is a number on the axis, never a year.
_DATE_DATATYPES = frozenset(
    _XSD + t for t in ("date", "dateTime", "dateTimeStamp", "gYear", "gYearMonth")
)


def _days_from_civil(year: int, month: int, day: int) -> float:
    """Days since 1970-01-01 in the proleptic Gregorian calendar.

    Written out rather than delegated to ``datetime`` because historical graphs
    carry years outside ``datetime``'s 1..9999 range, and BCE dates are common
    in archaeological and epigraphic datasets.
    """
    y = year - (1 if month <= 2 else 0)
    era = (y if y >= 0 else y - 399) // 400
    yoe = y - era * 400
    mp = (month + 9) % 12
    doy = (153 * mp + 2) // 5 + day - 1
    doe = yoe * 365 + yoe // 4 - yoe // 100 + doy
    return float(era * 146097 + doe - 719468)


def to_seconds(value: str, datatype: str = "") -> Optional[float]:
    """A temporal literal as seconds on a single continuous axis.

    Dates become seconds since the Unix epoch; ``xsd:time`` becomes seconds
    within a day; durations become their length in seconds; bare numbers pass
    through unchanged (a frame index, a millisecond timestamp or a year all
    behave correctly once the whole column is normalised together).
    """
    if value is None:
        return None
    text = str(value).strip()
    if not text:
        return None

    if datatype == _XSD + "duration" or text.startswith(("P", "-P")):
        m = _DURATION_RE.match(text)
        if m:
            g = m.groupdict()
            total = (
                float(g["y"] or 0) * DAYS_PER_YEAR * SECONDS_PER_DAY
                + float(g["mo"] or 0) * (DAYS_PER_YEAR / 12.0) * SECONDS_PER_DAY
                + float(g["d"] or 0) * SECONDS_PER_DAY
                + float(g["h"] or 0) * 3600.0
                + float(g["mi"] or 0) * 60.0
                + float(g["s"] or 0)
            )
            return -total if g["sign"] else total

    # A clock time with no date: "14:30:05". Distinguished from a dateTime by
    # the absence of a leading date part.
    if datatype == _XSD + "time" or (":" in text and "-" not in text and "T" not in text):
        m = _TIME_RE.match(text)
        if m:
            g = m.groupdict()
            return float(g["hour"]) * 3600 + float(g["minute"]) * 60 + float(g["second"] or 0)

    # A date needs positive evidence, and the pattern must consume the whole
    # string. Without the second condition "0.5" parses as the year 0 — which
    # silently wrecks every dataset that publishes time as decimal seconds.
    m = _DATE_RE.match(text)
    remainder = text[m.end() :].strip() if m else "x"
    plausible_year = bool(m) and len(m.group("year")) == 4
    is_date = bool(m) and not remainder and (
        datatype in _DATE_DATATYPES
        or bool(m.group("month"))
        or (plausible_year and not datatype)
    )
    if is_date:
        g = m.groupdict()
        year = int(g["year"]) * (-1 if g["sign"] else 1)
        month = max(1, min(12, int(g["month"] or 1)))
        day = max(1, min(31, int(g["day"] or 1)))
        seconds = _days_from_civil(year, month, day) * SECONDS_PER_DAY
        seconds += float(g["hour"] or 0) * 3600 + float(g["minute"] or 0) * 60
        seconds += float(g["second"] or 0)
        return seconds

    try:
        return float(text)
    except ValueError:
        return None


def format_seconds(seconds: float, style: str = "auto") -> str:
    """Human-readable label for a point on the axis, for the UI readout."""
    if seconds is None or not math.isfinite(seconds):
        return "—"
    if style == "raw":
        return f"{seconds:g}"
    if style == "clock" or abs(seconds) < 3.15e7:
        sign = "-" if seconds < 0 else ""
        s = abs(seconds)
        h, rem = divmod(s, 3600)
        m, sec = divmod(rem, 60)
        return f"{sign}{int(h):02d}:{int(m):02d}:{sec:06.3f}"
    days = seconds / SECONDS_PER_DAY
    year = 1970 + days / DAYS_PER_YEAR
    return f"{year:.2f}".rstrip("0").rstrip(".")


class Mapper:
    """Maps the dataset's time axis onto the scene's frame range."""

    def __init__(self, values: Sequence[float], frame_start: int, frame_end: int):
        finite = [v for v in values if v is not None and math.isfinite(v)]
        self.low = min(finite) if finite else 0.0
        self.high = max(finite) if finite else 1.0
        if self.high - self.low < 1e-9:
            self.high = self.low + 1.0
        self.frame_start = frame_start
        self.frame_end = max(frame_end, frame_start + 1)

    def frame(self, seconds: Optional[float]) -> Optional[float]:
        if seconds is None or not math.isfinite(seconds):
            return None
        t = (seconds - self.low) / (self.high - self.low)
        return self.frame_start + t * (self.frame_end - self.frame_start)

    def seconds(self, frame: float) -> float:
        t = (frame - self.frame_start) / max(1e-9, self.frame_end - self.frame_start)
        return self.low + t * (self.high - self.low)

    @property
    def span(self) -> Tuple[float, float]:
        return (self.low, self.high)


# ------------------------------------------------------------------ keyframing


def iter_fcurves(action) -> Iterable:
    """Every F-curve in an action, across Blender's action layouts.

    Blender 4.4 moved actions to layers/slots and kept ``action.fcurves`` as a
    legacy view that is empty for slotted actions, so both paths are walked.
    """
    if action is None:
        return []
    curves = list(getattr(action, "fcurves", []) or [])
    if curves:
        return curves
    for layer in getattr(action, "layers", []) or []:
        for strip in getattr(layer, "strips", []) or []:
            for bag in getattr(strip, "channelbags", []) or []:
                curves.extend(bag.fcurves)
    return curves


def _constant(obj: "bpy.types.Object", data_paths: Sequence[str]) -> None:
    """Force step interpolation — visibility must switch, not fade."""
    anim = obj.animation_data
    if not anim or not anim.action:
        return
    wanted = set(data_paths)
    for curve in iter_fcurves(anim.action):
        if curve.data_path in wanted:
            for kp in curve.keyframe_points:
                kp.interpolation = "CONSTANT"


def key_visibility(
    obj: "bpy.types.Object",
    start: Optional[float],
    end: Optional[float],
    *,
    frame_start: int,
    frame_end: int,
) -> None:
    """Make an object exist only between ``start`` and ``end`` frames.

    Both the viewport and the render flags are keyed, so what you scrub is what
    you render.
    """
    paths = ("hide_viewport", "hide_render")
    first = int(round(start)) if start is not None else frame_start
    last = int(round(end)) if end is not None else None

    if first > frame_start:
        obj.hide_viewport = obj.hide_render = True
        for path in paths:
            obj.keyframe_insert(data_path=path, frame=frame_start)
    obj.hide_viewport = obj.hide_render = False
    for path in paths:
        obj.keyframe_insert(data_path=path, frame=max(frame_start, first))
    if last is not None and last < frame_end:
        obj.hide_viewport = obj.hide_render = True
        for path in paths:
            obj.keyframe_insert(data_path=path, frame=last)
        obj.hide_viewport = obj.hide_render = False
    _constant(obj, paths)


def key_grow(
    obj: "bpy.types.Object",
    start: Optional[float],
    *,
    frame_start: int,
    duration: int = 12,
) -> None:
    """Scale an object up from nothing as its moment arrives."""
    if start is None:
        return
    first = max(frame_start, int(round(start)))
    target = tuple(obj.scale)
    obj.scale = (0.0, 0.0, 0.0)
    obj.keyframe_insert(data_path="scale", frame=max(frame_start, first - duration))
    obj.scale = target
    obj.keyframe_insert(data_path="scale", frame=first)


def key_location(obj: "bpy.types.Object", frames_locations: Sequence[Tuple[float, Sequence[float]]]) -> None:
    """Keyframe a motion path — the time-series case (tracking, trajectories)."""
    for frame, location in sorted(frames_locations, key=lambda fl: fl[0]):
        obj.location = tuple(location)
        obj.keyframe_insert(data_path="location", frame=int(round(frame)))


def key_property(obj: "bpy.types.Object", key: str, frames_values: Sequence[Tuple[float, float]]) -> None:
    """Keyframe a custom property, so RDF values over time can drive anything."""
    path = f'["{key}"]'
    for frame, value in sorted(frames_values, key=lambda fv: fv[0]):
        obj[key] = float(value)
        try:
            obj.keyframe_insert(data_path=path, frame=int(round(frame)))
        except (TypeError, RuntimeError):
            return


def retime_action(obj: "bpy.types.Object", offset: float, scale: float = 1.0) -> None:
    """Shift (and optionally stretch) an imported animation onto the timeline.

    Assets that carry their own animation — a danced figure, a rigged model —
    arrive starting at frame 1; this places each one at its own moment.
    """
    anim = obj.animation_data
    if not anim or not anim.action:
        return
    for curve in iter_fcurves(anim.action):
        for kp in curve.keyframe_points:
            kp.co.x = kp.co.x * scale + offset
            kp.handle_left.x = kp.handle_left.x * scale + offset
            kp.handle_right.x = kp.handle_right.x * scale + offset
        curve.update()


def set_scene_range(scene: "bpy.types.Scene", frame_start: int, frame_end: int) -> None:
    scene.frame_start = int(frame_start)
    scene.frame_end = int(max(frame_end, frame_start + 1))


def collect(
    rows: Sequence[Dict],
    var: Optional[str],
) -> List[Optional[float]]:
    """The time axis for a result set, as seconds per row."""
    if not var:
        return [None] * len(rows)
    out: List[Optional[float]] = []
    for row in rows:
        cell = row.get(var)
        out.append(to_seconds(cell.value, cell.datatype) if cell is not None else None)
    return out
