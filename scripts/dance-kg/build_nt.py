#!/usr/bin/env python3
"""Convert CoMPAS3D raw takes into the dance-kg ABox as N-Triples.

Emits: pairs + dancers (with skill level), songs, performances (72), and — for the
35 annotated takes — one Segment per annotation row, typed by the move(s) parsed from
its description, with hold, turn flags, timing, and the derived motion metrics
(follower-in-leader-frame offset, couple separation, hand-contact fraction).

Derived metrics reuse the geometry validated in explore_take.py. Takes whose npz are
missing/broken (Pair5_song1_take2) still get their annotation triples, minus motion.

Usage:  python build_nt.py <raw_compas3d_dir> <out.nt>
"""
import sys, re, hashlib
from pathlib import Path
import numpy as np
from scipy.spatial.transform import Rotation

BASE = "https://w3id.org/rete/dance/id/"
D = "https://w3id.org/rete/dance#"
SCHEMA = "https://schema.org/"
RDF = "http://www.w3.org/1999/02/22-rdf-syntax-ns#"
RDFS = "http://www.w3.org/2000/01/rdf-schema#"
DCT = "http://purl.org/dc/terms/"
XSD = "http://www.w3.org/2001/XMLSchema#"

HAND_MARKERS = list(range(40, 48))
CONTACT_THRESH = 0.15

# --- source metadata from the CoMPAS3D dataset card -------------------------------
PAIR_SKILL = {1: "Beginner", 2: "Intermediate", 3: "Beginner", 4: "Intermediate",
              5: "Professional", 6: "Intermediate", 7: "Professional", 8: "Beginner",
              9: "Professional"}
SONGS = {1: ("Tito Rojas", "Lo que te queda", 90),
         2: ("Louie Ramirez, Ray de La Paz", "Lluvia", 105),
         3: ("Leoni Torres", "Idilio", 95),
         4: ("Johnny Ventura", "Dilema", 93)}

# --- description -> move classes / holds / turn flags -----------------------------
MOVE_RULES = [  # (regex, class local-name)   order matters: specific before generic
    (r"double hand throw", "DoubleHandThrow"),
    (r"right hand throw", "RightHandThrow"),
    (r"left hand throw", "LeftHandThrow"),
    (r"hand throw|throw", "HandThrow"),
    (r"side basic", "SideBasicStep"),
    (r"guapea", "Guapea"),
    (r"basic step|basic\b", "BasicStep"),
    (r"\bxbl\b|cross body", "CrossBodyLead"),
    (r"enchufla", "Enchufla"),
    (r"dile que no", "DileQueNo"),
    (r"change of direction|swap position", "ChangeOfDirection"),
    (r"copa", "Copa"),
    (r"sombrero", "Sombrero"),
    (r"natural top", "NaturalTop"),
    (r"open break", "OpenBreak"),
    (r"walk(s|ing)? around", "WalksAround"),
    (r"outside turn", "RightTurn"),
    (r"inside turn", "LeftTurn"),
    (r"right turn", "RightTurn"),
    (r"left turn", "LeftTurn"),
    (r"lady styling", "LadyStyling"),
    (r"man styling", "ManStyling"),
    (r"arm ?lock", "ArmLock"),
    (r"\bcheck\b", "Check"),
    (r"\bdrop\b|\bdip\b", "Drop"),
    # footwork / shines
    (r"suzy ?q", "SuzyQ"),
    (r"mambo", "Mambo"),
    (r"\bkicks?\b", "Kicks"),
    (r"sliding|slide", "Sliding"),
    (r"\bpoint\b", "Point"),
    (r"standing", "Standing"),
    (r"steps?\b", "Steps"),
    # body / arm styling accents
    (r"hip movement", "HipMovement"),
    (r"body roll", "BodyRoll"),
    (r"body shake", "BodyShake"),
    (r"\bswing\b", "Swing"),
    (r"\bcomb\b", "Comb"),
    (r"\blasso\b", "Lasso"),
    (r"drawing circle", "DrawingCircle"),
    # catch-alls last
    (r"indescribable", "Indescribable"),
    (r"markers? swap", "MarkersSwapIssue"),
    (r"\bwalk\b", "Walk"),
]
ERROR_RULES = [
    (r"off ?beat", "OffBeat"),
    (r"mixed signal", "MixedSignals"),
    (r"misstep|misplaced|wrong step", "Misstep"),
    (r"misinterpret", "MisinterpretedSignal"),
    (r"fail", "FailedMove"),
]
HOLD_RULES = [
    (r"crossed hold|cross hold", "crossedHold"),
    (r"closed hold|closed hand", "closedHold"),
    (r"open hold", "openHold"),
    (r"shadow", "shadowHold"),
    (r"double hand", "doubleHold"),
    (r"no hold", "noHold"),
]


def esc(s):
    return s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", " ").replace("\r", " ").replace("\t", " ")


class NT:
    def __init__(self, path):
        self.f = open(path, "w", encoding="utf-8", newline="\n")  # LF only (NT parser)
        self.n = 0

    def uri(self, s, p, o):
        self.f.write(f"<{s}> <{p}> <{o}> .\n"); self.n += 1

    def lit(self, s, p, o, dt=None, lang=None):
        if dt:
            self.f.write(f'<{s}> <{p}> "{esc(str(o))}"^^<{dt}> .\n')
        elif lang:
            self.f.write(f'<{s}> <{p}> "{esc(str(o))}"@{lang} .\n')
        else:
            self.f.write(f'<{s}> <{p}> "{esc(str(o))}" .\n')
        self.n += 1

    def close(self):
        self.f.close()


def load_markers_trans_orient(npz):
    d = np.load(npz, allow_pickle=True)
    markers = np.array(d["markers_obs"].tolist(), dtype=float)
    return (markers, np.asarray(d["trans"], float), np.asarray(d["poses"], float)[:, :3],
            float(d["mocap_frame_rate"]))


def derived_series(lead_npz, fol_npz):
    lm, lt, lo, fps = load_markers_trans_orient(lead_npz)
    fm, ft, fo, _ = load_markers_trans_orient(fol_npz)
    n = min(len(lt), len(ft))
    lm, lt, lo, fm, ft = lm[:n], lt[:n], lo[:n], fm[:n], ft[:n]
    # leader heading frame
    R = Rotation.from_rotvec(lo).as_matrix()
    axes = R.transpose(0, 2, 1)
    up = np.array([0.0, 0.0, 1.0])
    pick = (1.0 - np.abs(axes @ up)).argmax(1)
    fwd = axes[np.arange(n), pick].copy(); fwd[:, 2] = 0
    nn = np.linalg.norm(fwd, axis=1, keepdims=True)
    fwd = np.divide(fwd, nn, out=np.zeros_like(fwd), where=nn > 1e-6)
    left = np.cross(np.tile(up, (n, 1)), fwd)
    rel = ft - lt
    f_fwd = np.einsum("ij,ij->i", rel, fwd)
    f_left = np.einsum("ij,ij->i", rel, left)
    sep = np.linalg.norm((ft - lt)[:, :2], axis=1)
    lh, fh = lm[:, HAND_MARKERS, :], fm[:, HAND_MARKERS, :]
    handmin = np.linalg.norm(lh[:, :, None, :] - fh[:, None, :, :], axis=3).reshape(n, -1).min(1)
    return {"n": n, "fps": fps, "fwd": f_fwd, "left": f_left, "sep": sep, "hand": handmin,
            "lt": lt, "ft": ft}


def parse_desc(desc, is_error):
    dl = desc.lower()
    moves = []
    seen = set()
    rules = ERROR_RULES if is_error else MOVE_RULES
    for pat, cls in rules:
        if re.search(pat, dl) and cls not in seen:
            moves.append(cls); seen.add(cls)
    holds = [h for pat, h in HOLD_RULES if re.search(pat, dl)]
    turn_leader = bool(re.search(r"turn for the leader|turn for the man|leader.{0,12}turn", dl))
    turn_follower = bool(re.search(r"turn for the follower|turn for the lady|follower.{0,12}turn|lady.{0,12}turn", dl))
    return moves, holds, turn_leader, turn_follower


SECTION_CLASS = {"Together": "PairedSegment", "Separate_Leader": "LeaderSegment",
                 "Separate_Follower": "FollowerSegment", "Errors": "ErrorSegment"}


def main():
    root = Path(sys.argv[1])
    out = NT(sys.argv[2])

    # --- skill levels & holds are defined in the TBox; here we emit instance data ---
    # pairs, dancers
    for pn in range(1, 10):
        pair = f"{BASE}pair{pn}"
        out.uri(pair, RDF + "type", D + "Pair")
        out.lit(pair, RDFS + "label", f"Pair {pn}", lang="en")
        skill = PAIR_SKILL[pn]
        for role, rl in (("leader", "Leader"), ("follower", "Follower")):
            dancer = f"{BASE}pair{pn}-{role}"
            out.uri(dancer, RDF + "type", D + "Dancer")
            out.uri(pair, D + "hasMember", dancer)
            out.uri(dancer, D + "hasSkillLevel", D + skill)
            out.lit(dancer, RDFS + "label", f"Pair {pn} {rl}", lang="en")

    # songs
    for sn, (artist, title, bpm) in SONGS.items():
        song = f"{BASE}song{sn}"
        out.uri(song, RDF + "type", D + "Song")
        out.lit(song, SCHEMA + "name", title, lang="en")
        out.lit(song, DCT + "creator", artist)
        out.lit(song, D + "tempoBPM", bpm, dt=XSD + "decimal")
        out.lit(song, RDFS + "label", f"{title} - {artist}", lang="en")

    # performances (all 72) + segments (annotated takes)
    take_dirs = sorted(root.glob("Pair*/*"))
    n_perf = n_seg = n_derived = 0
    for td in take_dirs:
        m = re.match(r"Pair(\d+)_song(\d+)_take(\d+)", td.name)
        if not m:
            continue
        pn, sn, tk = map(int, m.groups())
        perf = f"{BASE}pair{pn}-song{sn}-take{tk}"
        out.uri(perf, RDF + "type", D + "Performance")
        out.uri(perf, D + "performedBy", f"{BASE}pair{pn}")
        out.uri(perf, D + "hasLeader", f"{BASE}pair{pn}-leader")
        out.uri(perf, D + "hasFollower", f"{BASE}pair{pn}-follower")
        out.uri(perf, D + "toSong", f"{BASE}song{sn}")
        out.lit(perf, RDFS + "label", f"Pair {pn}, song {sn}, take {tk}", lang="en")
        n_perf += 1

        stem = td.name
        lead = td / f"{stem}_leader.npz"
        if not lead.exists():
            alt = td / f"{stem}_leaderi.npz"
            lead = alt if alt.exists() else lead
        fol = td / f"{stem}_follower.npz"
        ser = None
        if lead.exists() and fol.exists():
            try:
                ser = derived_series(lead, fol)
                out.lit(perf, D + "frameRate", ser["fps"], dt=XSD + "decimal")
                out.lit(perf, D + "frameCount", ser["n"], dt=XSD + "integer")
            except Exception as e:
                print(f"  ! motion load failed for {stem}: {e}", file=sys.stderr)

        txt = td / f"{stem}.txt"
        if not txt.exists():
            continue
        for i, line in enumerate(txt.read_text(encoding="utf-8", errors="replace").splitlines()):
            p = line.split("\t")
            if len(p) < 9:
                continue
            try:
                t0, t1 = float(p[3]), float(p[5])
            except ValueError:
                continue
            section = p[0].strip()
            desc = p[8].strip()
            if not desc:
                continue
            seg = f"{perf.replace(BASE, BASE)}-seg{i:03d}"
            out.uri(seg, RDF + "type", D + SECTION_CLASS.get(section, "Segment"))
            out.uri(perf, D + "hasSegment", seg)
            out.lit(seg, D + "section", section)
            out.lit(seg, D + "startTime", round(t0, 3), dt=XSD + "decimal")
            out.lit(seg, D + "endTime", round(t1, 3), dt=XSD + "decimal")
            out.lit(seg, D + "duration", round(t1 - t0, 3), dt=XSD + "decimal")
            out.lit(seg, RDFS + "label", desc, lang="en")
            is_error = section == "Errors"
            moves, holds, tl, tf = parse_desc(desc, is_error)
            for cls in moves:
                out.uri(seg, RDF + "type", D + cls)   # multi-typing for reasoning
            for h in holds:
                out.uri(seg, D + "hasHold", D + h)
            if tl:
                out.lit(seg, D + "turnByLeader", "true", dt=XSD + "boolean")
            if tf:
                out.lit(seg, D + "turnByFollower", "true", dt=XSD + "boolean")
            n_seg += 1
            # derived motion metrics for this interval
            if ser is not None:
                i0, i1 = int(t0 * ser["fps"]), min(int(t1 * ser["fps"]), ser["n"])
                if i1 > i0:
                    out.lit(seg, D + "meanCoupleSeparation", round(float(ser["sep"][i0:i1].mean()), 3), dt=XSD + "decimal")
                    out.lit(seg, D + "followerMeanForward", round(float(ser["fwd"][i0:i1].mean()), 3), dt=XSD + "decimal")
                    out.lit(seg, D + "followerMeanLeft", round(float(ser["left"][i0:i1].mean()), 3), dt=XSD + "decimal")
                    hm = ser["hand"][i0:i1]
                    out.lit(seg, D + "meanHandDistance", round(float(hm.mean()), 3), dt=XSD + "decimal")
                    out.lit(seg, D + "handContactFraction", round(float((hm < CONTACT_THRESH).mean()), 3), dt=XSD + "decimal")
                    n_derived += 1

    out.close()
    print(f"wrote {out.n} triples: {n_perf} performances, {n_seg} segments "
          f"({n_derived} with derived motion)")


if __name__ == "__main__":
    main()
