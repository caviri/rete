"""Convert OBJ surface meshes to Draco-compressed GLB for web preview/rendering.

LOSSY, ON PURPOSE: Draco quantizes vertex positions to an integer grid (default
14 bits per axis over the bounding box), trading exact coordinates for ~15-25x
smaller files. Triangle connectivity is preserved losslessly. Keep the source
OBJ as the analytical ground truth; treat the GLB as a disposable render copy.

Reports the quantization grid step (max per-axis positional error) so the
precision cost is explicit.

Run (rete-blender image, Blender 3.6):
  blender -b -noaudio --python scripts/obj_to_draco_glb.py -- <quant_bits> <out_dir> <obj1> [<obj2> ...]
"""
import bpy, sys, os

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
quant = int(argv[0])
out_dir = argv[1]
objs = argv[2:]
os.makedirs(out_dir, exist_ok=True)

for obj_path in objs:
    name = os.path.splitext(os.path.basename(obj_path))[0]
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.wm.obj_import(filepath=obj_path)

    meshes = [o for o in bpy.context.scene.objects if o.type == "MESH"]
    nv = sum(len(o.data.vertices) for o in meshes)
    nf = sum(len(o.data.polygons) for o in meshes)

    # world-space bounding box across all imported meshes
    xs = []; ys = []; zs = []
    for o in meshes:
        for corner in o.bound_box:
            w = o.matrix_world @ __import__("mathutils").Vector(corner)
            xs.append(w.x); ys.append(w.y); zs.append(w.z)
    dims = (max(xs) - min(xs), max(ys) - min(ys), max(zs) - min(zs))
    max_dim = max(dims)
    grid_step = max_dim / (2 ** quant - 1)  # max per-axis quantization error

    src_mb = os.path.getsize(obj_path) / 1e6
    out = os.path.join(out_dir, name + ".glb")
    bpy.ops.object.select_all(action='SELECT')
    bpy.ops.export_scene.gltf(
        filepath=out, export_format='GLB', use_selection=True,
        export_draco_mesh_compression_enable=True,
        export_draco_mesh_compression_level=6,
        export_draco_position_quantization=quant,
        export_draco_normal_quantization=10,
        export_yup=False, export_apply=True,
        export_materials='NONE',
    )
    out_mb = os.path.getsize(out) / 1e6
    print(f"=== GLB {name}: {nv:,} verts / {nf:,} faces | "
          f"OBJ {src_mb:.1f} MB -> Draco GLB {out_mb:.1f} MB ({src_mb/out_mb:.1f}x) | "
          f"bbox {dims[0]:.1f}x{dims[1]:.1f}x{dims[2]:.1f} | "
          f"{quant}-bit quant grid = {grid_step*1000:.1f} nm max per-axis error", flush=True)
print("=== DRACO GLB DONE")
