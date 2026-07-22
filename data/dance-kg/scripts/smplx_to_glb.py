#!/usr/bin/env python3
"""CoMPAS3D take (two SMPL-X dancers) -> one animated, skinned .glb.

NON-COMMERCIAL, HIGHLY EXPERIMENTAL research prototype. See data/dance-kg/scripts/NOTICE.md.
Uses the SMPL-X body model (Max Planck, non-commercial license) + CoMPAS3D (CC-BY-NC).
Run ONLY inside the container (data/dance-kg/scripts/Dockerfile.anim), never on the host.

We build a glTF skin (armature + linear-blend-skinning weights) and drive it with the
per-frame joint rotations from the take's `poses`; the GPU/model-viewer does the actual
skin deformation, so the file stays small (rest mesh once + rotation keyframes). Pose
blendshapes are omitted (minor joint-area artifacts) — a proof, not a render.

Usage:
  python smplx_to_glb.py <take_dir> <out.glb> [--models DIR] [--max-seconds S] [--fps N]
"""
import sys, os, struct, argparse
import numpy as np
from scipy.spatial.transform import Rotation

# SMPL-X pose vector (165) = 55 joints x 3 axis-angle, in kintree order.
N_JOINTS = 55
GENDER_FILE = {"male": "SMPLX_MALE.npz", "female": "SMPLX_FEMALE.npz",
               "neutral": "SMPLX_NEUTRAL.npz"}
# glTF component types
UBYTE, UINT, FLOAT = 5121, 5125, 5126


def load_model(models_dir, gender):
    f = GENDER_FILE.get(str(gender).lower(), "SMPLX_NEUTRAL.npz")
    d = np.load(os.path.join(models_dir, f), allow_pickle=True)
    return {
        "v_template": d["v_template"].astype(np.float64),          # (V,3)
        "shapedirs": d["shapedirs"].astype(np.float64),            # (V,3,400)
        "J_regressor": d["J_regressor"].astype(np.float64),        # (55,V)
        "weights": d["weights"].astype(np.float64),               # (V,55)
        "kintree": d["kintree_table"].astype(np.int64),           # (2,55)
        "faces": d["f"].astype(np.uint32),                        # (F,3)
    }


def dancer_rig(model, betas):
    nb = min(300, betas.shape[0], model["shapedirs"].shape[2])
    V = model["v_template"] + np.einsum("vij,j->vi", model["shapedirs"][:, :, :nb], betas[:nb])
    J = model["J_regressor"] @ V                                   # (55,3) rest joints
    parents = model["kintree"][0].copy(); parents[0] = -1
    return V, J, parents


def vertex_normals(V, F):
    n = np.zeros_like(V)
    tris = V[F]
    fn = np.cross(tris[:, 1] - tris[:, 0], tris[:, 2] - tris[:, 0])
    for k in range(3):
        np.add.at(n, F[:, k], fn)
    ln = np.linalg.norm(n, axis=1, keepdims=True)
    return n / np.where(ln > 1e-9, ln, 1.0)


_BOX_F = [(0, 1, 3), (0, 3, 2), (4, 6, 7), (4, 7, 5), (0, 4, 5), (0, 5, 1),
          (2, 3, 7), (2, 7, 6), (0, 2, 6), (0, 6, 4), (1, 5, 7), (1, 7, 3)]
_SIGNS = np.array([[x, y, z] for x in (-1, 1) for y in (-1, 1) for z in (-1, 1)], float)


def skeleton_geometry(J, parents):
    """A procedural stick-figure: a small cube at each joint (weighted to that joint)
    and a bone box for each parent->child link (weighted to the PARENT joint, so both
    ends land exactly on the two joints under LBS). Contains NO SMPL-X mesh, weights,
    topology or blendshapes — only the joint rest locations (like a BVH skeleton), so
    it is safe to redistribute (pure CoMPAS3D motion). Returns V, faces, per-vertex
    joint assignment (rigid weight 1)."""
    verts, faces, assign = [], [], []

    def add_box(corners, jidx):
        base = len(verts)
        verts.extend(corners.tolist()); assign.extend([jidx] * 8)
        for a, b, c in _BOX_F:
            faces.append((base + a, base + b, base + c))

    for j in range(len(J)):
        add_box(J[j] + _SIGNS * 0.018, j)                    # joint marker cube
    for j in range(len(J)):
        p = parents[j]
        if p < 0:
            continue
        A, B = J[p], J[j]
        d = B - A; L = np.linalg.norm(d)
        if L < 1e-4:
            continue
        dirn = d / L
        u = np.cross(dirn, [0.0, 1.0, 0.0])
        if np.linalg.norm(u) < 1e-3:
            u = np.cross(dirn, [1.0, 0.0, 0.0])
        u /= np.linalg.norm(u); v = np.cross(dirn, u)
        r = 0.011
        corners = np.array([A + (d if end else 0) + su * u * r + sv * v * r
                            for end in (0, 1) for su in (-1, 1) for sv in (-1, 1)])
        add_box(corners, p)                                  # bone -> weighted to parent
    return np.array(verts, float), np.array(faces, np.uint32), np.array(assign, np.uint8)


class Glb:
    """Minimal single-buffer glTF assembler."""
    def __init__(self):
        self.blob = bytearray()
        self.bufferViews, self.accessors, self.nodes = [], [], []
        self.meshes, self.skins, self.animations = [], [], []
        self.anim_samplers, self.anim_channels = [], []   # one shared clip for all dancers
        self.scene_nodes = []

    def _pad(self):
        while len(self.blob) % 4:
            self.blob.append(0)

    def add(self, arr, ctype, type_str, minmax=False, target=None):
        self._pad()
        off = len(self.blob)
        data = arr.tobytes()
        self.blob += data
        bv = {"buffer": 0, "byteOffset": off, "byteLength": len(data)}
        if target:
            bv["target"] = target
        self.bufferViews.append(bv)
        acc = {"bufferView": len(self.bufferViews) - 1, "componentType": ctype,
               "count": int(arr.shape[0]), "type": type_str}
        if minmax:
            acc["min"] = np.atleast_1d(arr.min(axis=0)).tolist()
            acc["max"] = np.atleast_1d(arr.max(axis=0)).tolist()
        self.accessors.append(acc)
        return len(self.accessors) - 1


def add_dancer(g, V, faces, j0_arr, w0_arr, J, parents, poses, trans, fps, name, color, rconv, center):
    n = poses.shape[0]
    times = (np.arange(n) / fps).astype(np.float32)

    # --- mesh accessors (geometry + rigid/soft skin weights precomputed by caller) ---
    pos = g.add(V.astype(np.float32), FLOAT, "VEC3", minmax=True, target=34962)
    nrm = g.add(vertex_normals(V, faces).astype(np.float32), FLOAT, "VEC3", target=34962)
    j0 = g.add(j0_arr.astype(np.uint8), UBYTE, "VEC4", target=34962)
    wt0 = g.add(w0_arr.astype(np.float32), FLOAT, "VEC4", target=34962)
    idx = g.add(faces.reshape(-1).astype(np.uint32), UINT, "SCALAR", target=34963)

    # --- joint nodes (skeleton) ---
    base = len(g.nodes)
    for j in range(N_JOINTS):
        p = parents[j]
        local = (J[j] - J[p]) if p >= 0 else J[j]
        g.nodes.append({"name": f"{name}_j{j}", "translation": local.tolist(),
                        "rotation": [0, 0, 0, 1], "children": []})
    for j in range(N_JOINTS):
        p = parents[j]
        if p >= 0:
            g.nodes[base + p]["children"].append(base + j)
    joint_idx = list(range(base, base + N_JOINTS))

    # inverse bind matrices = translate(-J[j]) (rest global has identity rotation),
    # column-major 4x4.
    ibm = np.zeros((N_JOINTS, 16), np.float32)
    for j in range(N_JOINTS):
        M = np.eye(4); M[:3, 3] = -J[j]
        ibm[j] = M.T.reshape(-1)
    ibm_acc = g.add(ibm, FLOAT, "MAT4")
    g.skins.append({"joints": joint_idx, "inverseBindMatrices": ibm_acc,
                    "skeleton": base})

    # --- skinned mesh node ---
    g.meshes.append({"name": name, "primitives": [{
        "attributes": {"POSITION": pos, "NORMAL": nrm, "JOINTS_0": j0, "WEIGHTS_0": wt0},
        "indices": idx, "material": len(g.__dict__.setdefault("materials", []))}]})
    g.materials.append({"name": name + "_mat", "doubleSided": True, "pbrMetallicRoughness": {
        "baseColorFactor": color, "metallicFactor": 0.0, "roughnessFactor": 0.85}})
    mesh_node = len(g.nodes)
    g.nodes.append({"name": name + "_mesh", "mesh": len(g.meshes) - 1,
                    "skin": len(g.skins) - 1})

    # --- animation: per-joint rotation + root translation ---
    tin = g.add(times, FLOAT, "SCALAR", minmax=True)
    aa = poses.reshape(n, N_JOINTS, 3)

    def push(output_acc, node, path):   # append to the single shared clip
        g.anim_samplers.append({"input": tin, "output": output_acc, "interpolation": "LINEAR"})
        g.anim_channels.append({"sampler": len(g.anim_samplers) - 1,
                                "target": {"node": node, "path": path}})

    for j in range(N_JOINTS):
        rot = Rotation.from_rotvec(aa[:, j, :])
        if j == 0:
            rot = rconv * rot          # fold Z-up->Y-up into the root's world rotation
        push(g.add(rot.as_quat().astype(np.float32), FLOAT, "VEC4"), base + j, "rotation")
    # root translation: pelvis in mocap world -> Y-up -> recentered on the couple
    root_tr = (rconv.apply(J[0][None, :] + trans[:n]) - center[None, :]).astype(np.float32)
    push(g.add(root_tr, FLOAT, "VEC3"), base, "translation")

    return [base, mesh_node]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("take_dir")
    ap.add_argument("out")
    ap.add_argument("--models", default="data/dance-kg/raw/compas3d/non-commertial-data")
    ap.add_argument("--max-seconds", type=float, default=20.0)
    ap.add_argument("--fps", type=float, default=30.0)
    ap.add_argument("--skeleton", action="store_true",
                    help="export a license-clean stick-figure (no SMPL-X mesh) instead of the skinned body")
    a = ap.parse_args()

    stem = os.path.basename(a.take_dir.rstrip("/\\"))
    g = Glb()
    g.materials = []
    colors = {"leader": [0.20, 0.45, 0.85, 1.0], "follower": [0.90, 0.35, 0.45, 1.0]}
    rconv = Rotation.from_euler("x", -90, degrees=True)   # SMPL-X Z-up -> glTF Y-up

    # First pass: load + rig both dancers, and find the couple's floor centre so the
    # export sits around the origin (model-viewer frames it, and the two stay aligned).
    dancers, pel = [], []
    for role in ("leader", "follower"):
        npz = os.path.join(a.take_dir, f"{stem}_{role}.npz")
        if not os.path.exists(npz):
            alt = os.path.join(a.take_dir, f"{stem}_{role}i.npz")   # Pair7 typo
            npz = alt if os.path.exists(alt) else npz
        d = np.load(npz, allow_pickle=True)
        cap = int(a.max_seconds * float(d["mocap_frame_rate"]))
        poses = np.asarray(d["poses"], np.float64)[:cap]
        trans = np.asarray(d["trans"], np.float64)[:cap]
        model = load_model(a.models, str(d["gender"]))
        V, J, parents = dancer_rig(model, np.asarray(d["betas"], np.float64))
        pel.append(rconv.apply(J[0][None, :] + trans))
        dancers.append((role, model, V, J, parents, poses, trans, float(d["mocap_frame_rate"]), str(d["gender"])))
    allpel = np.concatenate(pel, axis=0)
    center = np.array([allpel[:, 0].mean(), 0.0, allpel[:, 2].mean()])   # recentre X,Z; keep Y (floor)

    dancer_roots = []
    for role, model, V, J, parents, poses, trans, fps, gender in dancers:
        if a.skeleton:
            Vg, faces, jassign = skeleton_geometry(J, parents)   # no SMPL-X mesh in output
            j0 = np.zeros((len(Vg), 4), np.uint8); j0[:, 0] = jassign
            w0 = np.zeros((len(Vg), 4), np.float32); w0[:, 0] = 1.0
        else:
            Vg, faces = V, model["faces"]
            order = np.argsort(-model["weights"], axis=1)[:, :4]
            w4 = np.take_along_axis(model["weights"], order, axis=1)
            w4 = w4 / np.clip(w4.sum(axis=1, keepdims=True), 1e-9, None)
            j0, w0 = order.astype(np.uint8), w4.astype(np.float32)
        roots = add_dancer(g, Vg, faces, j0, w0, J, parents,
                           poses, trans, fps, role, colors[role], rconv, center)
        dancer_roots += roots
        print(f"  {role}: {poses.shape[0]} frames, gender={gender}, verts={len(Vg)}")

    gltf = {
        "asset": {"version": "2.0", "generator": "dance-kg smplx_to_glb (non-commercial, experimental)",
                  "copyright": "SMPL-X (c) MPI, non-commercial; CoMPAS3D CC-BY-NC — see NOTICE.md"},
        "scene": 0, "scenes": [{"nodes": dancer_roots}],
        "nodes": g.nodes, "meshes": g.meshes, "materials": g.materials,
        "skins": g.skins,
        "animations": [{"name": "dance", "samplers": g.anim_samplers, "channels": g.anim_channels}],
        "accessors": g.accessors, "bufferViews": g.bufferViews,
        "buffers": [{"byteLength": len(g.blob)}],
    }

    # write .glb (header + JSON chunk + BIN chunk)
    import json
    jbytes = json.dumps(gltf, separators=(",", ":")).encode("utf-8")
    while len(jbytes) % 4:
        jbytes += b" "
    bbytes = bytes(g.blob)
    while len(bbytes) % 4:
        bbytes += b"\x00"
    total = 12 + 8 + len(jbytes) + 8 + len(bbytes)
    with open(a.out, "wb") as fh:
        fh.write(struct.pack("<III", 0x46546C67, 2, total))
        fh.write(struct.pack("<II", len(jbytes), 0x4E4F534A)); fh.write(jbytes)
        fh.write(struct.pack("<II", len(bbytes), 0x004E4942)); fh.write(bbytes)
    print(f"wrote {a.out}: {total/1e6:.1f} MB, {len(g.nodes)} nodes, "
          f"1 clip / {len(g.anim_channels)} channels")


if __name__ == "__main__":
    main()
