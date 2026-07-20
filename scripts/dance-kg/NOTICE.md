# NOTICE — dance-kg motion & animation work

**This is a non-commercial, highly experimental research prototype.**

The 3D motion and animation pipeline in `scripts/dance-kg/` (SMPL-X → glTF/animation,
image and mesh transformations) is built on assets and data that are licensed for
**non-commercial research use only**. It is exploratory work: heuristics (e.g. marker-based
hand-contact, leader-frame geometry) are approximations, not ground truth, and the outputs
are prototypes for research and demonstration — not a product.

## Do not use commercially

- **SMPL-X body model** (Max Planck Institute for Intelligent Systems) — licensed for
  research / non-commercial use. See <https://smpl-x.is.tue.mpg.de> and the license shipped
  with the download. Commercial licensing: smpl@max-planck-innovation.de.
- **CoMPAS3D** dataset (Rosie Lab, SFU) — **CC-BY-NC-4.0** (audio rights retained by owners).
  Non-commercial only. <https://huggingface.co/datasets/Rosie-Lab/compas3d>
- Some SMPL-X UV maps (Meshcapade, UV2023 / BEDLAM2.0) are **CC-BY-NC-4.0**.

## Gated assets are NOT redistributed

The SMPL-X model package lives under
`data/dance-kg/raw/compas3d/non-commertial-data/` (gitignored). These files are gated
downloads obtained under the SMPL-X license by the user; they are **not committed, not
uploaded to R2, and must not be redistributed**. Only the *code* that transforms them lives
in the repo.

## Reproducibility

All image / mesh / animation transformations run inside a **container** (Blender-based, see
`scripts/Dockerfile.blender`) — never on the host — so the environment is pinned and the
work is reproducible without polluting the host toolchain.
