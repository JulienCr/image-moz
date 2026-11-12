"""Regenerates the 3D fixtures. Run:
blender -b --factory-startup --python tests/fixtures/3d/make_fixtures.py
"""
import os
import bpy
from mathutils import Vector

OUT = os.path.dirname(os.path.abspath(__file__))


def path(name):
    return os.path.join(OUT, name)


def reset():
    bpy.ops.wm.read_factory_settings(use_empty=True)


def add_camera():
    cam_data = bpy.data.cameras.new("Camera")
    cam = bpy.data.objects.new("Camera", cam_data)
    bpy.context.scene.collection.objects.link(cam)
    cam.location = (4, -4, 3)
    direction = Vector((0, 0, 0)) - cam.location
    cam.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()
    return cam


def add_cube(textured=False):
    bpy.ops.mesh.primitive_cube_add(size=2, location=(0, 0, 0))
    cube = bpy.context.active_object
    mat = bpy.data.materials.new("Mat")
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes["Principled BSDF"]
    if textured:
        img = bpy.data.images.new("green", 2, 2, alpha=True)
        img.pixels = [0.0, 1.0, 0.0, 1.0] * 4
        img.pack()
        tex = mat.node_tree.nodes.new("ShaderNodeTexImage")
        tex.image = img
        tex.interpolation = "Closest"
        bsdf.inputs["Base Color"].default_value = (1, 1, 1, 1)
        mat.node_tree.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
    else:
        bsdf.inputs["Base Color"].default_value = (1, 0, 0, 1)
    mat.diffuse_color = (1, 0, 0, 1) if not textured else (1, 1, 1, 1)
    cube.data.materials.append(mat)
    return cube


def export_glb(name, cameras, fmt="GLB"):
    bpy.ops.export_scene.gltf(
        filepath=path(name),
        export_format=fmt,
        export_cameras=cameras,
        export_image_format="AUTO",
    )


reset()
add_cube()
add_camera()
export_glb("cube.glb", True)
export_glb("cube.gltf", True, "GLTF_SEPARATE")
export_glb("cube_nocam.glb", False)
bpy.ops.wm.obj_export(
    filepath=path("cube.obj"),
    forward_axis="NEGATIVE_Z",
    up_axis="Y",
    export_materials=True,
)

reset()
add_cube(textured=True)
export_glb("textured.glb", False)

reset()
add_camera()
export_glb("empty.glb", True)
