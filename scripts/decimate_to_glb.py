"""Decimate OBJ meshes and export web-preview Draco-GLB (for playground 3D cells).

Blender handles multi-million-vertex meshes that the WASM (gltf-transform)
simplifier aborts on. Collapse-decimates to a target face ratio, then exports
Draco-compressed GLB (14-bit position quant). LOSSY preview only; keep the OBJ.

Run (rete-blender): blender -b -noaudio --python scripts/decimate_to_glb.py -- <ratio> <out_dir> <obj1> [...]
"""
import bpy, sys, os

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
ratio = float(argv[0]); out_dir = argv[1]; objs = argv[2:]
os.makedirs(out_dir, exist_ok=True)

for obj_path in objs:
    name = os.path.splitext(os.path.basename(obj_path))[0]
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.wm.obj_import(filepath=obj_path)
    meshes = [o for o in bpy.context.scene.objects if o.type == "MESH"]
    f0 = sum(len(o.data.polygons) for o in meshes)
    for o in meshes:
        m = o.modifiers.new("dec", "DECIMATE")
        m.decimate_type = "COLLAPSE"; m.ratio = ratio
        bpy.context.view_layer.objects.active = o
        bpy.ops.object.modifier_apply(modifier="dec")
    f1 = sum(len(o.data.polygons) for o in meshes)
    out = os.path.join(out_dir, name + ".glb")
    bpy.ops.object.select_all(action='SELECT')
    bpy.ops.export_scene.gltf(
        filepath=out, export_format='GLB', use_selection=True,
        export_draco_mesh_compression_enable=True,
        export_draco_mesh_compression_level=6,
        export_draco_position_quantization=14,
        export_yup=False, export_apply=True, export_materials='NONE')
    print(f"=== {name}: {f0:,} -> {f1:,} faces ({ratio:.0%}) -> "
          f"{os.path.getsize(out)/1e6:.1f} MB GLB", flush=True)
print("=== DECIMATE DONE")
