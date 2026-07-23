"""Images and videos from a graph, as scene objects.

Image URLs already become textured materials (see :mod:`.materials`). This adds
the two things a material cannot do:

* an **image plane** — a photo, a IIIF scan, a portrait — standing upright in 3D
  at the entity's position, sized to the picture's own aspect ratio;
* a **360° world** — an equirectangular panorama becomes the scene's world
  environment;

and it brings **video** in at all: a movie URL becomes an upright plane whose
texture plays, synced to the scene's frame range, so a graph of clips lays them
out in space and scrubbing the timeline plays them.

Videos load through Blender's own movie reader, so whatever containers the build
supports (mp4/webm/mov/…) work; nothing here decodes video itself.
"""

from __future__ import annotations

import os
from typing import Optional, Tuple

import bpy

from . import assets, materials


def _image_dimensions(image) -> Tuple[int, int]:
    w, h = image.size[0], image.size[1]
    return (w or 1, h or 1)


def _aspect(image) -> float:
    w, h = _image_dimensions(image)
    return w / h if h else 1.0


def _upright_plane(name: str, width: float, height: float, collection) -> "bpy.types.Object":
    """A plane standing on the XZ axes (normal toward -Y), bottom at the origin.

    Upright and ground-seated is how a photo or a video screen wants to sit in a
    scene — you walk up to it, rather than looking down on it.
    """
    hw = width / 2.0
    verts = [(-hw, 0.0, 0.0), (hw, 0.0, 0.0), (hw, 0.0, height), (-hw, 0.0, height)]
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(verts, [], [[0, 1, 2, 3]])
    mesh.uv_layers.new(name="UVMap")
    uvs = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
    for loop, uv in zip(mesh.uv_layers[0].data, uvs):
        loop.uv = uv
    mesh.update()
    obj = bpy.data.objects.new(name, mesh)
    if collection is not None:
        collection.objects.link(obj)
    return obj


def image_plane(
    url: str,
    name: str,
    collection,
    *,
    height: float = 1.0,
    max_pixels: int = 2048,
    emissive: bool = False,
) -> Optional["bpy.types.Object"]:
    """An upright plane textured with the image at ``url``, sized to its aspect."""
    image = assets.load_image(url, max_pixels=max_pixels)
    if image is None:
        return None
    width = height * _aspect(image)
    obj = _upright_plane(name, width, height, collection)
    mat = materials.textured(url, max_pixels=max_pixels)
    if mat is not None:
        if emissive:
            _make_emissive(mat)
        materials.assign(obj, mat)
    return obj


def set_world_panorama(url: str, *, max_pixels: int = 4096, strength: float = 1.0) -> bool:
    """Use an equirectangular image as the scene world's environment texture."""
    image = assets.load_image(url, max_pixels=max_pixels)
    if image is None:
        return False
    world = bpy.context.scene.world
    if world is None:
        world = bpy.data.worlds.new("rete world")
        bpy.context.scene.world = world
    world.use_nodes = True
    tree = world.node_tree
    tree.nodes.clear()
    out = tree.nodes.new("ShaderNodeOutputWorld")
    out.location = (300, 0)
    bg = tree.nodes.new("ShaderNodeBackground")
    bg.location = (100, 0)
    bg.inputs["Strength"].default_value = strength
    env = tree.nodes.new("ShaderNodeTexEnvironment")
    env.image = image
    env.location = (-200, 0)
    tex = tree.nodes.new("ShaderNodeTexCoord")
    tex.location = (-400, 0)
    tree.links.new(tex.outputs["Generated"], env.inputs["Vector"])
    tree.links.new(env.outputs["Color"], bg.inputs["Color"])
    tree.links.new(bg.outputs["Background"], out.inputs["Surface"])
    return True


# ----------------------------------------------------------------- video


def _make_emissive(mat: "bpy.types.Material") -> None:
    """Wire an image texture's colour into emission, so a screen glows."""
    if not mat.use_nodes:
        return
    tree = mat.node_tree
    bsdf = next((n for n in tree.nodes if n.type == "BSDF_PRINCIPLED"), None)
    tex = next((n for n in tree.nodes if n.type == "TEX_IMAGE"), None)
    if bsdf is None or tex is None:
        return
    for socket_name in ("Emission Color", "Emission"):
        socket = bsdf.inputs.get(socket_name)
        if socket is not None:
            tree.links.new(tex.outputs["Color"], socket)
            break
    strength = bsdf.inputs.get("Emission Strength")
    if strength is not None:
        strength.default_value = 1.0


def load_movie(url: str) -> Optional[Tuple["bpy.types.Image", int]]:
    """Load a video URL as a movie image datablock; return ``(image, frames)``."""
    try:
        path = assets.fetch(url)
    except IOError:
        return None
    try:
        image = bpy.data.images.load(path, check_existing=True)
    except RuntimeError:
        return None
    if image.source != "MOVIE":
        # Blender infers MOVIE from the extension; force it for odd containers.
        try:
            image.source = "MOVIE"
        except (TypeError, RuntimeError):
            return None
    frames = int(getattr(image, "frame_duration", 0)) or 1
    return image, frames


def video_material(url: str, *, frame_start: int = 1, emissive: bool = True) -> Optional["bpy.types.Material"]:
    """A material whose base colour is a video, played over the frame range."""
    loaded = load_movie(url)
    if loaded is None:
        return None
    image, frames = loaded
    name = f"rete:video:{os.path.basename(url)}"
    mat = bpy.data.materials.get(name)
    if mat is not None:
        return mat
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    tree = mat.node_tree
    bsdf = next((n for n in tree.nodes if n.type == "BSDF_PRINCIPLED"), None)
    tex = tree.nodes.new("ShaderNodeTexImage")
    tex.image = image
    tex.location = (-320, 260)
    # The image user drives playback: start at frame_start, run the clip's
    # length, loop, and refresh as the timeline moves.
    user = tex.image_user
    user.frame_duration = frames
    user.frame_start = frame_start
    user.use_auto_refresh = True
    user.use_cyclic = True
    if bsdf is not None:
        tree.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
        if emissive:
            for socket_name in ("Emission Color", "Emission"):
                socket = bsdf.inputs.get(socket_name)
                if socket is not None:
                    tree.links.new(tex.outputs["Color"], socket)
                    break
            strength = bsdf.inputs.get("Emission Strength")
            if strength is not None:
                strength.default_value = 1.0
    return mat


def video_plane(
    url: str,
    name: str,
    collection,
    *,
    height: float = 1.0,
    frame_start: int = 1,
) -> Optional[Tuple["bpy.types.Object", int]]:
    """An upright plane playing a video; returns ``(object, frame_count)``."""
    loaded = load_movie(url)
    if loaded is None:
        return None
    image, frames = loaded
    width = height * _aspect(image)
    obj = _upright_plane(name, width, height, collection)
    mat = video_material(url, frame_start=frame_start)
    if mat is not None:
        materials.assign(obj, mat)
    return obj, frames
