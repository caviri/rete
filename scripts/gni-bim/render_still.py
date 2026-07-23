"""Render a single hero still of one or more GLBs to a PNG (headless Blender/EEVEE).

  xvfb-run -a blender -b -noaudio --python scripts/gni-bim/render_still.py -- \
      <out.png> <width> <glb>[|r,g,b|alpha] [<glb>|r,g,b|alpha ...]

Each GLB gets a flat-ish Principled material of the given colour; alpha<1 makes it
a translucent overlay (BLEND). Camera is a front-right-above 3/4 on a dark backdrop.
"""
import bpy, sys, os, math, mathutils

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
OUT = argv[0]; W = int(argv[1]); specs = argv[2:]

bpy.ops.wm.read_factory_settings(use_empty=True)
try:
    bpy.ops.preferences.addon_enable(module="io_scene_gltf2")
except Exception:
    pass

allmeshes = []
for spec in specs:
    parts = spec.split("|")
    path = parts[0]
    col = tuple(float(x) for x in parts[1].split(",")) if len(parts) > 1 else (0.80, 0.72, 0.60)
    alpha = float(parts[2]) if len(parts) > 2 else 1.0
    before = set(bpy.context.scene.objects)
    bpy.ops.import_scene.gltf(filepath=path)
    new = [o for o in bpy.context.scene.objects if o not in before and o.type == "MESH"]
    mat = bpy.data.materials.new("m"); mat.use_nodes = True
    b = mat.node_tree.nodes.get("Principled BSDF")
    b.inputs["Base Color"].default_value = (col[0], col[1], col[2], 1.0)
    b.inputs["Roughness"].default_value = 0.72
    try: b.inputs["Metallic"].default_value = 0.0
    except Exception: pass
    if alpha < 1.0:
        b.inputs["Alpha"].default_value = alpha
        mat.blend_method = "BLEND"; mat.show_transparent_back = False
    for o in new:
        o.data.materials.clear(); o.data.materials.append(mat)
    allmeshes += new

# Our GLBs are already Z-up; Blender's glTF importer assumes Y-up and rotates +90°X,
# laying the building on its side. Undo that so the model stands upright.
Rx = mathutils.Matrix.Rotation(-math.pi / 2, 4, "X")
for o in allmeshes:
    o.matrix_world = Rx @ o.matrix_world
bpy.context.view_layer.update()

mn = mathutils.Vector((1e18,) * 3); mx = mathutils.Vector((-1e18,) * 3)
for o in allmeshes:
    for c in o.bound_box:
        w = o.matrix_world @ mathutils.Vector(c)
        mn = mathutils.Vector((min(mn.x, w.x), min(mn.y, w.y), min(mn.z, w.z)))
        mx = mathutils.Vector((max(mx.x, w.x), max(mx.y, w.y), max(mx.z, w.z)))
center = (mn + mx) / 2.0
size = (mx - mn); radius = max(size.x, size.y, size.z) / 2.0 or 1.0

cam_data = bpy.data.cameras.new("cam"); cam = bpy.data.objects.new("cam", cam_data)
bpy.context.scene.collection.objects.link(cam)
dz = float(os.environ.get("CAM_Z", "0.55"))
df = float(os.environ.get("DIST_FACTOR", "1.25"))
d = mathutils.Vector((0.85, -0.95, dz)); d.normalize()
cam.location = center + d * (size.length * df)     # frame by the diagonal so nothing is cropped
cam_data.lens = float(os.environ.get("LENS", "40"))
cam.rotation_euler = (center - cam.location).to_track_quat("-Z", "Y").to_euler()
bpy.context.scene.camera = cam

world = bpy.data.worlds.new("w"); bpy.context.scene.world = world; world.use_nodes = True
bg = world.node_tree.nodes["Background"]
bg.inputs[0].default_value = (0.045, 0.055, 0.078, 1.0); bg.inputs[1].default_value = 1.0
key = bpy.data.lights.new("key", type="SUN"); key.energy = 4.2
ko = bpy.data.objects.new("key", key); bpy.context.scene.collection.objects.link(ko)
ko.rotation_euler = (math.radians(52), math.radians(12), math.radians(38))
fill = bpy.data.lights.new("fill", type="SUN"); fill.energy = 1.5; fill.color = (0.7, 0.8, 1.0)
fo = bpy.data.objects.new("fill", fill); bpy.context.scene.collection.objects.link(fo)
fo.rotation_euler = (math.radians(62), 0, math.radians(-125))

scene = bpy.context.scene
scene.render.engine = "CYCLES"          # headless, no display needed (image has no xvfb)
scene.cycles.device = "CPU"
scene.cycles.samples = int(os.environ.get("CYCLES_SAMPLES", "40"))
try:
    scene.cycles.use_denoising = True
    scene.cycles.denoiser = "OPENIMAGEDENOISE"
except Exception:
    pass
scene.render.film_transparent = False
scene.render.resolution_x = W; scene.render.resolution_y = int(W * 0.60)
scene.render.image_settings.file_format = "PNG"
os.makedirs(os.path.dirname(OUT) or ".", exist_ok=True)
scene.render.filepath = OUT
bpy.ops.render.render(write_still=True)
print("STILL:", OUT)
