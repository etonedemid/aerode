# Blender scene for aerode-claude's assistant: the coral critter in glossy Aero plastic.
# blender -b -P data/assistant/crab.py -- OUTDIR   renders idle0..5.png and work0..5.png (transparent, 256x256)
import bpy, math, sys

out = sys.argv[sys.argv.index("--") + 1] if "--" in sys.argv else "data/assistant/crab"
bpy.ops.wm.read_factory_settings(use_empty=True)
sc = bpy.context.scene

# the reference is drawn on a ~30 px grid (400 px image): x right, z up, in grid units
def block(name, x0, x1, z0, z1, depth, mat, bevel=0.18, y=0.0):
    bpy.ops.mesh.primitive_cube_add(location=((x0 + x1) / 2, y, (z0 + z1) / 2))
    o = bpy.context.object
    o.name = name
    o.scale = ((x1 - x0) / 2, depth / 2, (z1 - z0) / 2)
    bpy.ops.object.transform_apply(scale=True)
    m = o.modifiers.new("bevel", "BEVEL"); m.width = bevel; m.segments = 4
    bpy.ops.object.shade_smooth()
    o.data.materials.append(mat)
    return o

def plastic(name, rgb, rough=0.25, coat=1.0):
    m = bpy.data.materials.new(name); m.use_nodes = True
    b = m.node_tree.nodes["Principled BSDF"]
    b.inputs["Base Color"].default_value = (*rgb, 1)
    b.inputs["Roughness"].default_value = rough
    b.inputs["Coat Weight"].default_value = coat
    b.inputs["Coat Roughness"].default_value = 0.03
    return m

def srgb(h):
    c = [int(h[i:i + 2], 16) / 255 for i in (1, 3, 5)]
    return tuple(v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4 for v in c)

coral = plastic("coral", srgb("#f25f47"), 0.3)
eye = plastic("eye", (0.004, 0.004, 0.005), 0.05)

D = 3.2  # body depth
body = block("body", 2.3, 10.8, 2.25, 8.25, D, coral, 0.35)
arm_l = block("arm_l", 0.4, 2.6, 4.25, 6.25, D * 0.7, coral, 0.3)
arm_r = block("arm_r", 10.5, 12.7, 4.25, 6.25, D * 0.7, coral, 0.3)
eye_l = block("eye_l", 3.5, 4.6, 6.25, 7.3, 0.4, eye, 0.12, y=-D / 2 - 0.08)
eye_r = block("eye_r", 8.55, 9.65, 6.25, 7.3, 0.4, eye, 0.12, y=-D / 2 - 0.08)
legs = [block(f"leg{i}", x0, x0 + 1.05, 0.0, 2.6, D * 0.55, coral, 0.2) for i, x0 in enumerate((2.3, 4.4, 7.55, 9.75))]

# everything hangs off the body so a bob moves it all
for o in [arm_l, arm_r, eye_l, eye_r] + legs:
    o.parent = body
    o.matrix_parent_inverse = body.matrix_world.inverted()

# sky-blue Aero world: lights the clear coat with a soft blue gradient, not seen (transparent film)
w = bpy.data.worlds.new("sky"); sc.world = w; w.use_nodes = True
nt = w.node_tree
tc = nt.nodes.new("ShaderNodeTexCoord"); sep = nt.nodes.new("ShaderNodeSeparateXYZ"); ramp = nt.nodes.new("ShaderNodeValToRGB")
ramp.color_ramp.elements[0].color = (0.55, 0.62, 0.7, 1)
ramp.color_ramp.elements[1].color = (0.35, 0.65, 1.0, 1)
nt.links.new(tc.outputs["Generated"], sep.inputs[0]); nt.links.new(sep.outputs["Z"], ramp.inputs[0])
nt.links.new(ramp.outputs[0], nt.nodes["Background"].inputs[0])
nt.nodes["Background"].inputs[1].default_value = 0.5

def light(kind, loc, energy, size=4):
    l = bpy.data.lights.new(kind, "AREA"); l.energy = energy; l.size = size
    o = bpy.data.objects.new(kind, l); sc.collection.objects.link(o); o.location = loc
    o.rotation_euler = (loc - __import__("mathutils").Vector((6.5, 0, 4))).to_track_quat("Z", "Y").to_euler()
from mathutils import Vector
light("key", Vector((2, -12, 14)), 1500, 6)
light("rim", Vector((14, 8, 10)), 1000, 4)
light("fill", Vector((12, -10, 2)), 300, 8)

cam = bpy.data.objects.new("cam", bpy.data.cameras.new("cam")); sc.collection.objects.link(cam); sc.camera = cam
cam.location = (13, -26, 8)
cam.rotation_euler = (Vector((6.55, 0, 3.9)) - cam.location).to_track_quat("-Z", "Y").to_euler()
cam.data.lens = 62

sc.render.engine = "CYCLES"
sc.cycles.device = "CPU"
sc.cycles.samples = 96
sc.cycles.use_denoising = True
sc.render.film_transparent = True
sc.render.resolution_x = sc.render.resolution_y = 256
sc.render.image_settings.file_format = "PNG"
sc.render.image_settings.color_mode = "RGBA"
sc.view_settings.view_transform = "Standard"

base = {o.name: (o.location.copy(), o.rotation_euler.copy()) for o in bpy.data.objects if o.type == "MESH"}
def pose(bob=0.0, legs_up=(0, 0, 0, 0), arm=(0, 0), blink=False, lean=0.0):
    for n, (l, r) in base.items():
        o = bpy.data.objects[n]; o.location = l.copy(); o.rotation_euler = r.copy()
    body.location.z += bob
    body.rotation_euler.y = lean
    for leg, up in zip(legs, legs_up): leg.location.z += up
    arm_l.location.z += arm[0]; arm_r.location.z += arm[1]
    # a blink: the eyes sink into the body
    for e in (eye_l, eye_r): e.location.y += 0.5 if blink else 0.0

def render(name):
    sc.render.filepath = f"{out}/{name}.png"
    bpy.ops.render.render(write_still=True)

# idle: slow breathing, the arms drift with it; idle3 is the same pose blinking (aerode-claude shows it now and then)
idle = [
    dict(bob=0.00, arm=(0.00, 0.00)),
    dict(bob=0.06, arm=(0.05, 0.05)),
    dict(bob=0.10, arm=(0.08, 0.08)),
    dict(bob=0.10, arm=(0.08, 0.08), blink=True),
    dict(bob=0.06, arm=(0.05, 0.05)),
    dict(bob=0.02, arm=(0.01, 0.01)),
]
for i, f in enumerate(idle):
    pose(**f); render(f"idle{i}")
# working: a busy scuttle - alternate leg pairs step, arms tap like typing, a bob and a blink
s = 0.35
frames = [
    dict(bob=0.10, legs_up=(s, 0, s, 0), arm=(0.45, -0.15), lean=0.03),
    dict(bob=0.18, legs_up=(s * 0.5, 0, s * 0.5, 0), arm=(0.15, 0.15), lean=0.0),
    dict(bob=0.10, legs_up=(0, s, 0, s), arm=(-0.15, 0.45), lean=-0.03),
    dict(bob=0.18, legs_up=(0, s * 0.5, 0, s * 0.5), arm=(0.15, 0.15), lean=0.0),
    dict(bob=0.10, legs_up=(s, 0, s, 0), arm=(0.45, -0.15), lean=0.03),
    dict(bob=0.02, legs_up=(0, 0, 0, 0), arm=(0, 0), lean=0.0),
]
for i, f in enumerate(frames):
    pose(**f); render(f"work{i}")
