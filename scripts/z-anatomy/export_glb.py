"""Export one Draco-compressed GLB per Z-Anatomy system for the web viewer.

Keeps only real organ meshes (drops the 8-vert '.j' label leader-lines, the
'Cross Section X/Y/Z' clipping planes, and all empties). glTF preserves object
names, so the viewer can address / isolate an individual structure by node name.

Run (rete-blender image):
  blender -b -noaudio --python export_glb.py -- <out_dir> <fbx1> [<fbx2> ...]
"""
import bpy, sys, os, re

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
out_dir = argv[0]
fbx_files = argv[1:]
os.makedirs(out_dir, exist_ok=True)

for fbx in fbx_files:
    system = re.sub(r"\d+$", "", os.path.splitext(os.path.basename(fbx))[0]).strip()
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.fbx(filepath=fbx)

    # the real organ meshes we keep (drop 8-vert '.j' label lines + cross-sections)
    keep = [o for o in bpy.context.scene.objects
            if o.type == "MESH" and len(o.data.vertices) >= 12
            and not o.name.endswith(".j") and not o.name.startswith("Cross Section")]

    # CRITICAL: bake each kept mesh's WORLD transform and unparent it BEFORE deleting
    # the empties. Many meshes are positioned by a parent empty (a '.g' group); if we
    # delete that parent first, the mesh collapses to its parent-relative local
    # transform (this is why MuscularSystem came out at 1/3 scale and misaligned).
    # CLEAR_KEEP_TRANSFORM writes the world matrix into the object so it survives.
    bpy.ops.object.select_all(action='DESELECT')
    for o in keep:
        o.select_set(True)
    bpy.context.view_layer.objects.active = keep[0]
    bpy.ops.object.parent_clear(type='CLEAR_KEEP_TRANSFORM')

    # now delete everything that is not a kept mesh (empties, label-lines, sections)
    keepset = set(keep)
    to_del = [o for o in list(bpy.context.scene.objects) if o not in keepset]
    bpy.ops.object.select_all(action='DESELECT')
    for o in to_del:
        o.select_set(True)
    bpy.ops.object.delete()

    kept = [o for o in bpy.context.scene.objects if o.type == "MESH"]
    out = os.path.join(out_dir, system.replace(" ", "_") + ".glb")
    bpy.ops.object.select_all(action='SELECT')
    bpy.ops.export_scene.gltf(
        filepath=out, export_format='GLB', use_selection=True,
        export_draco_mesh_compression_enable=True,
        export_draco_mesh_compression_level=6,
        export_yup=False, export_apply=True,   # keep Blender Z-up so the GLB frame
                                               # matches the JSON (mm, Z-up) exactly
        export_materials='NONE',
    )
    sz = os.path.getsize(out) / 1e6
    print(f"=== GLB {system}: {len(kept)} meshes -> {out} ({sz:.1f} MB)")
print("=== GLB EXPORT DONE")
