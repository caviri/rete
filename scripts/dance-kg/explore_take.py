#!/usr/bin/env python3
"""Prototype extraction for one CoMPAS3D take: parse annotations, derive the
follower's trajectory in the leader's reference frame, inter-dancer distance,
and hand-contact events. Sanity-checks the derived hand contact against the
annotation text and writes a summary JSON + a diagnostic PNG.

This is exploration code, not the final KG builder. It deliberately depends only
on the raw npz (no gated SMPL-X body model): "hand" positions come from the eight
hand-height Vicon markers, "heading" from the SMPL-X pelvis global orientation.

Usage:  python explore_take.py <take_dir> [out_dir]
  take_dir e.g. data/dance-kg/raw/compas3d/Pair1/Pair1_song1_take1
"""
import sys, json, re
from pathlib import Path
import numpy as np
from scipy.spatial.transform import Rotation

# --- marker layout (Vicon "FrontWaist" 53-set, indices found empirically) -----
# The 8 hand markers sit at chest height (z~1.0) with the highest per-frame speed
# of any markers. Feet are 30-39 (z~0.05), head 48-52 (z~1.5). We treat all eight
# as one "hands" point cloud per dancer — enough to detect inter-dancer contact
# without resolving left/right. See README "Upstream data quirks" for provenance.
HAND_MARKERS = list(range(40, 48))
HAND_CONTACT_THRESH_M = 0.15   # min hand-to-hand distance below which we call contact
CONTACT_MIN_FRAMES = 3         # debounce: contact/release must persist this many frames


def load_dancer(npz_path):
    d = np.load(npz_path, allow_pickle=True)
    markers = np.array(d["markers_obs"].tolist(), dtype=float)   # (N,53,3)
    return {
        "gender": str(d["gender"]),
        "fps": float(d["mocap_frame_rate"]),
        "trans": np.asarray(d["trans"], float),                  # (N,3) pelvis
        "orient": np.asarray(d["poses"], float)[:, :3],          # (N,3) global axis-angle
        "markers": markers,
        "hands": markers[:, HAND_MARKERS, :],                    # (N,8,3)
        "n": markers.shape[0],
    }


def parse_annotations(txt_path):
    """CoMPAS3D annotation rows are tab-separated:
    section, roles, t_start_hms, t_start_s, t_end_hms, t_end_s, dur_hms, dur_s, description
    """
    rows = []
    for line in Path(txt_path).read_text(encoding="utf-8", errors="replace").splitlines():
        if not line.strip():
            continue
        parts = line.split("\t")
        if len(parts) < 9:
            continue
        try:
            t0, t1 = float(parts[3]), float(parts[5])
        except ValueError:
            continue
        rows.append({"t_start": t0, "t_end": t1, "desc": parts[8].strip(),
                     "section": parts[0].strip(), "roles": parts[1].strip()})
    return rows


def heading_frame(orient_aa, trans):
    """Return per-frame orthonormal floor frame (forward, left) for the leader.
    Forward = the pelvis body axis whose world projection is most horizontal
    (z-up world). Left = up x forward. All flattened to the floor plane."""
    R = Rotation.from_rotvec(orient_aa).as_matrix()      # (N,3,3) body->world
    up = np.array([0.0, 0.0, 1.0])
    # candidate body axes in world coords: columns of R
    axes = R.transpose(0, 2, 1)                          # (N,3,3): axes[:,k] = world dir of body axis k
    horiz = 1.0 - np.abs(axes @ up)                      # how horizontal each axis is
    pick = horiz.argmax(axis=1)                          # per-frame most-horizontal axis
    fwd = axes[np.arange(len(axes)), pick]               # (N,3)
    fwd[:, 2] = 0.0
    n = np.linalg.norm(fwd, axis=1, keepdims=True)
    fwd = np.divide(fwd, n, out=np.zeros_like(fwd), where=n > 1e-6)
    left = np.cross(np.tile(up, (len(fwd), 1)), fwd)     # (N,3)
    return fwd, left


def follower_in_leader_frame(leader, follower):
    fwd, left = heading_frame(leader["orient"], leader["trans"])
    rel = follower["trans"] - leader["trans"]            # (N,3) world
    f = np.einsum("ij,ij->i", rel, fwd)                  # forward component (ahead +)
    l = np.einsum("ij,ij->i", rel, left)                 # left component
    up = rel[:, 2]                                       # height diff
    return np.stack([f, l, up], axis=1)                  # (N,3) in leader frame


def min_hand_distance(leader, follower):
    """Per-frame minimum distance between any leader hand marker and any follower
    hand marker. (N,8,3) x (N,8,3) -> (N,)"""
    lh, fh = leader["hands"], follower["hands"]          # (N,8,3)
    diff = lh[:, :, None, :] - fh[:, None, :, :]         # (N,8,8,3)
    dist = np.linalg.norm(diff, axis=3)                  # (N,8,8)
    return dist.reshape(dist.shape[0], -1).min(axis=1)   # (N,)


def debounce(mask, min_len):
    """Remove runs of True/False shorter than min_len (simple morphological clean)."""
    out = mask.copy()
    n = len(mask)
    i = 0
    while i < n:
        j = i
        while j < n and out[j] == out[i]:
            j += 1
        if j - i < min_len and 0 < i:            # too-short run: flip to previous state
            out[i:j] = out[i - 1]
        i = j
    return out


def contact_intervals(contact_mask, fps):
    intervals = []
    n = len(contact_mask)
    i = 0
    while i < n:
        if contact_mask[i]:
            j = i
            while j < n and contact_mask[j]:
                j += 1
            intervals.append((i / fps, j / fps))
            i = j
        else:
            i += 1
    return intervals


def main():
    take_dir = Path(sys.argv[1])
    out_dir = Path(sys.argv[2]) if len(sys.argv) > 2 else take_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    stem = take_dir.name

    lead_path = take_dir / f"{stem}_leader.npz"
    if not lead_path.exists():                       # tolerate the Pair7 "_leaderi" typo
        alt = take_dir / f"{stem}_leaderi.npz"
        if alt.exists():
            lead_path = alt
    ld = load_dancer(lead_path)
    fd = load_dancer(take_dir / f"{stem}_follower.npz")

    n = min(ld["n"], fd["n"])
    for d in (ld, fd):
        for k in ("trans", "orient", "markers", "hands"):
            d[k] = d[k][:n]
    fps = ld["fps"]
    dur = n / fps

    ann = parse_annotations(take_dir / f"{stem}.txt") if (take_dir / f"{stem}.txt").exists() else []

    rel = follower_in_leader_frame(ld, fd)               # (n,3)
    dist = np.linalg.norm(fd["trans"][:, :2] - ld["trans"][:, :2], axis=1)  # floor-plane separation
    handmin = min_hand_distance(ld, fd)
    contact = debounce(handmin < HAND_CONTACT_THRESH_M, CONTACT_MIN_FRAMES)
    intervals = contact_intervals(contact, fps)

    # --- sanity check: do annotation rows mentioning hold/hand/throw overlap contact? ---
    hold_re = re.compile(r"hold|hand|throw|closed|open", re.I)
    checked = matched = 0
    for a in ann:
        if hold_re.search(a["desc"]):
            checked += 1
            i0, i1 = int(a["t_start"] * fps), int(min(a["t_end"] * fps, n))
            if i1 > i0 and contact[i0:i1].mean() > 0.3:
                matched += 1

    summary = {
        "take": stem,
        "frames": int(n), "fps": fps, "duration_s": round(dur, 2),
        "leader_gender": ld["gender"], "follower_gender": fd["gender"],
        "annotation_rows": len(ann),
        "floor_separation_m": {
            "min": round(float(dist.min()), 3), "mean": round(float(dist.mean()), 3),
            "max": round(float(dist.max()), 3)},
        "follower_in_leader_frame_m": {
            "forward": [round(float(rel[:, 0].min()), 2), round(float(rel[:, 0].max()), 2)],
            "left":    [round(float(rel[:, 1].min()), 2), round(float(rel[:, 1].max()), 2)]},
        "hand_min_distance_m": {
            "min": round(float(handmin.min()), 3), "median": round(float(np.median(handmin)), 3),
            "p90": round(float(np.percentile(handmin, 90)), 3)},
        "hand_contact": {
            "threshold_m": HAND_CONTACT_THRESH_M,
            "fraction_of_time_in_contact": round(float(contact.mean()), 3),
            "n_contact_intervals": len(intervals),
            "n_release_events": max(0, len(intervals) - 1)},
        "sanity_check_hold_annotations": {
            "rows_mentioning_hold_hand_throw": checked,
            "rows_overlapping_detected_contact": matched,
            "agreement": round(matched / checked, 3) if checked else None},
    }
    (out_dir / f"{stem}.explore.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    print(json.dumps(summary, indent=2))

    # --- diagnostic figure ---
    try:
        import matplotlib
        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
        t = np.arange(n) / fps
        fig, ax = plt.subplots(1, 3, figsize=(16, 5))

        ax[0].plot(ld["trans"][:, 0], ld["trans"][:, 1], lw=0.8, label="leader")
        ax[0].plot(fd["trans"][:, 0], fd["trans"][:, 1], lw=0.8, label="follower")
        ax[0].set_title("World floor plan (x,y)"); ax[0].set_aspect("equal"); ax[0].legend()
        ax[0].set_xlabel("x (m)"); ax[0].set_ylabel("y (m)")

        sc = ax[1].scatter(rel[:, 1], rel[:, 0], c=t, s=4, cmap="viridis")
        ax[1].scatter([0], [0], c="red", marker="*", s=200, label="leader")
        ax[1].set_title("Follower in leader frame\n(x=left, y=forward)")
        ax[1].set_aspect("equal"); ax[1].legend(); fig.colorbar(sc, ax=ax[1], label="t (s)")
        ax[1].set_xlabel("left (m)"); ax[1].set_ylabel("forward (m)")

        ax[2].plot(t, handmin, lw=0.6, color="gray", label="min hand dist")
        ax[2].axhline(HAND_CONTACT_THRESH_M, color="red", ls="--", lw=0.8, label="threshold")
        for i0, i1 in intervals:
            ax[2].axvspan(i0, i1, color="green", alpha=0.12)
        ax[2].set_title("Hand contact (green = in contact)")
        ax[2].set_xlabel("t (s)"); ax[2].set_ylabel("m"); ax[2].set_ylim(0, 1.0); ax[2].legend()

        fig.suptitle(f"{stem}  —  {ld['gender']} leader / {fd['gender']} follower, {dur:.0f}s @ {fps:.0f}fps")
        fig.tight_layout()
        fig.savefig(out_dir / f"{stem}.explore.png", dpi=110)
        print(f"wrote {out_dir / f'{stem}.explore.png'}")
    except Exception as e:
        print("plot skipped:", e)


if __name__ == "__main__":
    main()
