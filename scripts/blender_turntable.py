"""Render a horizontal-rotation turntable of a .glb as a PNG frame sequence.

Run inside Blender (headless, with a virtual display for EEVEE):
  xvfb-run -a blender -b -noaudio --python scripts/blender_turntable.py -- \
      <model.glb> <out_frame_dir> [frames=36] [res=480]

Imports the glTF/GLB (Draco supported by Blender's bundled importer), parents all
meshes to an empty at the model's centre, frames a camera on it, lights it neutrally
on a soft light-grey backdrop, spins the empty 0->360 deg over `frames`, and writes
frame_001.png .. frame_NNN.png. A separate ffmpeg step makes the .webm and .gif.
"""
import bpy, sys, os, math, mathutils

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
glb = argv[0]
outdir = argv[1]
frames = int(argv[2]) if len(argv) > 2 else 36
res = int(argv[3]) if len(argv) > 3 else 480

# Empty scene, then make sure the glTF importer is on.
bpy.ops.wm.read_factory_settings(use_empty=True)
try:
    bpy.ops.preferences.addon_enable(module="io_scene_gltf2")
except Exception:
    pass

bpy.ops.import_scene.gltf(filepath=glb)
meshes = [o for o in bpy.context.scene.objects if o.type == "MESH"]
if not meshes:
    print("TURNTABLE: no mesh in", glb); sys.exit(2)

# Combined world-space bounding box.
mn = mathutils.Vector((1e18, 1e18, 1e18))
mx = mathutils.Vector((-1e18, -1e18, -1e18))
for o in meshes:
    for c in o.bound_box:
        w = o.matrix_world @ mathutils.Vector(c)
        mn = mathutils.Vector((min(mn.x, w.x), min(mn.y, w.y), min(mn.z, w.z)))
        mx = mathutils.Vector((max(mx.x, w.x), max(mx.y, w.y), max(mx.z, w.z)))
center = (mn + mx) / 2.0
radius = max((mx - mn).x, (mx - mn).y, (mx - mn).z) / 2.0 or 1.0

# Pivot empty at the centre; parent every mesh so they spin together.
bpy.ops.object.empty_add(location=center)
pivot = bpy.context.active_object
for o in meshes:
    o.parent = pivot
    o.matrix_parent_inverse = pivot.matrix_world.inverted()

# Camera, slightly above, looking at the centre.
cam_data = bpy.data.cameras.new("cam")
cam = bpy.data.objects.new("cam", cam_data)
bpy.context.scene.collection.objects.link(cam)
dist = radius * 3.0
cam.location = (center.x, center.y - dist, center.z + radius * 0.55)
cam.rotation_euler = (center - cam.location).to_track_quat("-Z", "Y").to_euler()
bpy.context.scene.camera = cam

# Soft neutral lighting + a light-grey world so specimens read on webm AND gif.
world = bpy.data.worlds.new("w"); bpy.context.scene.world = world
world.use_nodes = True
bg = world.node_tree.nodes["Background"]
bg.inputs[0].default_value = (0.92, 0.92, 0.93, 1.0)
bg.inputs[1].default_value = 1.0
key = bpy.data.lights.new("key", type="SUN"); key.energy = 3.2
ko = bpy.data.objects.new("key", key); bpy.context.scene.collection.objects.link(ko)
ko.rotation_euler = (math.radians(55), math.radians(15), math.radians(35))

scene = bpy.context.scene
scene.render.engine = "BLENDER_EEVEE"
scene.render.film_transparent = False
scene.render.resolution_x = scene.render.resolution_y = res
scene.render.image_settings.file_format = "PNG"
scene.frame_start = 1
scene.frame_end = frames

# Linear 0 -> 360 deg spin about Z over the frame range (+1 so the last frame
# isn't a duplicate of the first — seamless loop).
pivot.rotation_mode = "XYZ"
pivot.rotation_euler = (0, 0, 0); pivot.keyframe_insert("rotation_euler", frame=1)
pivot.rotation_euler = (0, 0, math.radians(360)); pivot.keyframe_insert("rotation_euler", frame=frames + 1)
for fc in pivot.animation_data.action.fcurves:
    for kp in fc.keyframe_points:
        kp.interpolation = "LINEAR"

os.makedirs(outdir, exist_ok=True)
for f in range(1, frames + 1):
    scene.frame_set(f)
    scene.render.filepath = os.path.join(outdir, "frame_%03d.png" % f)
    bpy.ops.render.render(write_still=True)
print("TURNTABLE: wrote %d frames to %s" % (frames, outdir))
