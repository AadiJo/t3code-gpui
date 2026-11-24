"""Writes spec.json: the elements compare.py measures in each parity scene.
Coordinates are logical px in the 1100x780 reference captures.
Usage: python3 gen_spec.py spec.json && python3 compare.py record <reference-png-dir>
"""
import json, sys

def fill(name, x, y, **k): return {"name": name, "kind": "fill", "at": [x, y], **k}
def ink(name, x0, y0, x1, y1, **k): return {"name": name, "kind": "ink", "at": [x0, y0, x1, y1], **k}
def hscan(name, y, x0, x1, **k): return {"name": name, "kind": "hscan", "at": [y, x0, x1], **k}
def vscan(name, x, y0, y1, **k): return {"name": name, "kind": "vscan", "at": [x, y0, y1], **k}
T = {"text": True}
TR = {"text": "right"}
BL = {"blur": True}
FIX_MODEL = "fixture: the composer catalog is the Claude fixture (thread_route_view.rs), not Replay"

def chrome():
    return [
        fill("titlebar fill", 600, 30),
        fill("sidebar fill (top)", 240, 40),
        hscan("sidebar edge", 400, 250, 262),
        ink("sidebar toggle", 84, 14, 112, 40),
        ink("right toggles", 995, 14, 1090, 40),
        ink("breadcrumb monogram", 272, 16, 293, 37),
        ink("breadcrumb project", 294, 14, 350, 40, **T),
        ink("breadcrumb slash", 350, 14, 366, 40, **T),
        ink("breadcrumb title", 368, 14, 520, 40, **T),
        ink("dev badge", 2, 15, 34, 37, exclude="dev builds only; never on the owner's build"),
    ]

def sidebar(active=True):
    e = [
        ink("search icon", 12, 62, 36, 90),
        ink("search placeholder", 38, 62, 110, 90, **T),
        ink("header actions", 150, 60, 245, 92),
        hscan("row 1 card edges", 140, 0, 256),
        vscan("row 1 card top/bottom", 12, 95, 192),
        ink("row 1 monogram", 16, 110, 36, 130),
        ink("row 1 project", 38, 108, 150, 130, **T),
        ink("row 1 time", 190, 108, 242, 130, **TR),
        ink("row 1 title", 16, 133, 240, 157, **T),
        ink("row 1 branch", 16, 156, 70, 176, **T),
        ink("row 1 provider", 218, 152, 245, 178),
        ink("row 2 status", 168, 190, 242, 214, **TR),
        ink("row 2 title", 16, 216, 240, 240, **T),
        ink("row 3 status", 168, 273, 242, 297, **TR),
        ink("settled label", 14, 700, 80, 724, **T),
        ink("settled count", 200, 700, 245, 724),
        ink("footer icons", 8, 735, 200, 772),
        ink("update button", 210, 740, 245, 772, exclude="reference build has no desktop updater bridge, so the web draws it disabled at 60%; the owner's app draws it enabled"),
    ]
    return e

def composer(y=648.5, fixture=True):
    return [
        hscan("composer left edge", 700, 290, 312, **BL),
        hscan("composer right edge", 700, 1040, 1070, **BL),
        vscan("composer card top/bottom", 800, 630, 779, **BL),
        fill("composer fill", 420, 700),
        ink("composer placeholder", 310, 662, 760, 690, **T),
        ink("attach icon", 310, 718, 352, 750),
        ink("send icon", 1050 - 45, 718, 1048, 750),
        ink("model control", 820, 718, 1005, 750, exclude=FIX_MODEL),
    ]

scenes = {}
def scene(name, *groups, reference=None):
    body = {"elements": [e for g in groups for e in g]}
    if reference: body["reference"] = reference
    scenes[name] = body

bottom_timeline = [
    vscan("card top/bottom", 700, 435, 560),
    hscan("card sides", 488, 300, 1060),
    ink("card title", 320, 450, 420, 475, **T),
    ink("card +7 -0", 420, 450, 470, 475),
    ink("card buttons", 830, 448, 1040, 478),
    ink("folder row", 316, 488, 400, 512),
    ink("folder stats", 960, 488, 1040, 512),
    ink("blockquote", 280, 375, 900, 405),
    ink("link line", 280, 408, 900, 432, **T),
    ink("assistant meta row", 280, 528, 480, 550, note="copy, fork and revert actions with the time"),
]

scene("window-thread", chrome(), sidebar(), composer(), bottom_timeline)
scene("sidebar-selected", chrome(), sidebar(), composer())
scene("sidebar-hover", chrome(), sidebar(), [
    fill("hovered row fill", 120, 190),
    ink("row 2 hover actions", 150, 190, 245, 214),
])
scene("sidebar-context-menu", chrome(), [
    hscan("menu left edge", 300, 120, 145, **BL),
    vscan("menu top edge", 200, 215, 245, **BL),
    fill("menu fill", 300, 290),
    ink("item 1 icon", 136, 233, 158, 255),
    ink("item 1 label", 160, 233, 300, 255, **T),
    vscan("menu row pitch", 136.5, 225, 300),
])

scene("sidebar-inbox", chrome(), [
    ink("inbox popover", 270, 440, 1000, 780),
    fill("inbox fill", 500, 590),
    ink("inbox title", 300, 462, 450, 490, **T),
    ink("mark all read", 700, 462, 870, 490, **T),
    ink("tabs", 300, 490, 450, 515, **T),
    ink("item 1 icon", 278, 520, 300, 545),
    ink("item 1 title", 316, 520, 760, 543, **T),
    ink("item 1 subtitle", 316, 543, 760, 562, **T),
    ink("item 1 meta", 316, 562, 760, 582, **T),
    fill("bell active fill", 160, 755),
])
A = {"anchor": "worked-for anchor"}
ANCHOR = ink("worked-for anchor", 360, 110, 440, 200, exclude="locator: finds the turn, whose offset follows the Agent pill")
scene("timeline-top", chrome(), [
    ink("first prompt", 300, 75, 1060, 112, exclude="the fork folds a message from another agent into an Agent pill; the port has no agent attribution yet"),
    ANCHOR,
    ink("worked for", 300, 130, 500, 155, **T, **A),
    vscan("turn rule", 700, 152, 175, **A),
    ink("h2 Done", 300, 170, 400, 196, **T, **A),
    ink("paragraph 1", 300, 205, 1060, 231, **T, **A),
    fill("inline code chip", 369, 216, **A),
    ink("bullet 1", 300, 240, 900, 262, **T, **A),
    ink("bullet 3", 300, 293, 900, 315, **T, **A),
    ink("ordered 1", 300, 326, 900, 350, **T, **A),
    ink("ordered 2 file chip", 300, 354, 900, 378, **T, **A),
    ink("ordered 3", 300, 381, 900, 405, **T, **A),
    vscan("code block top", 1000, 405, 440, **A),
    ink("code header badge", 315, 420, 345, 445, **A),
    ink("code header buttons", 960, 420, 1045, 445, **A),
    ink("code line 1", 320, 458, 900, 478, **T, **A),
    ink("code line 2", 320, 478, 900, 496, **T, **A),
    fill("code block fill", 1000, 520, **A),
], composer())
scene("timeline-tools-expanded", chrome(), [
    ANCHOR,
    ink("worked for", 300, 130, 500, 155, **T, **A),
    ink("worked time", 900, 130, 1050, 155, **TR, **A),
    ink("thought row", 300, 172, 1060, 196, **T, **A),
    ink("tasks row", 300, 200, 1060, 224, **T, **A),
    ink("read row", 300, 228, 1060, 252, **T, **A),
    ink("interim reply", 300, 262, 1060, 290, **T, **A),
    ink("changed 1 file row", 300, 298, 1060, 326, **T, **A),
    ink("h2 Done", 300, 334, 400, 360, **T, **A),
])
scene("timeline-bottom", chrome(), composer())
U = {"anchor": "bubble anchor"}
scene("timeline-user-message", chrome(), [
    ink("bubble anchor", 1040, 300, 1056, 500, exclude="locator: the bubble's right end; scroll position is not styling"),
    ink("user bubble", 540, 385, 1060, 445, **U),
    fill("bubble fill", 1000, 412, **U),
    ink("bubble text", 560, 400, 1040, 425, **T, **U),
    ink("message meta", 860, 445, 1050, 470, **U),
], composer())
scene("composer-empty", chrome(), composer())
scene("composer-focused", chrome(), composer())
scene("composer-multiline", chrome(), [
    hscan("composer left edge", 700, 290, 312, **BL),
    hscan("composer right edge", 700, 1040, 1070, **BL),
    vscan("composer card top/bottom", 800, 560, 779, **BL),
    fill("composer fill", 420, 650),
    ink("composer text", 310, 570, 900, 700, **T),
])
scene("composer-model-picker", chrome(), [
    hscan("picker left edge", 600, 440, 480, **BL),
    hscan("picker right edge", 600, 990, 1035, **BL),
    vscan("picker top edge", 700, 330, 370, **BL),
    vscan("picker bottom edge", 700, 690, 735, **BL),
    fill("picker rail fill", 482, 640),
    fill("picker list fill", 700, 640),
    fill("picker effort fill", 920, 600),
    fill("picker footer fill", 600, 702),
    vscan("search underline", 600, 375, 395),
    vscan("option pitch", 520, 398, 610, exclude="option rows hold fixture content (Claude catalog, not Replay)"),
])

scene("composer-mention", chrome(), [
    vscan("mention menu top", 700, 460, 500, **BL),
    vscan("mention rows", 340, 495, 650, exclude="rows hold fixture files"),
    fill("mention menu fill", 1000, 600),
])

scene("command-palette", [
    hscan("palette left edge", 300, 240, 290, **BL),
    hscan("palette right edge", 300, 815, 860, **BL),
    vscan("palette top edge", 550, 60, 100, **BL),
    vscan("palette bottom edge", 550, 470, 520, exclude="height follows the command list, which is registry content"),
    fill("palette fill", 800, 420),
    fill("highlighted item", 600, 178),
    ink("search placeholder", 270, 80, 600, 115, **T),
    ink("section label", 270, 135, 400, 160, **T),
    vscan("item pitch", 266, 158, 200),
    fill("backdrop over sidebar", 230, 300),
])

scene("settings-environments", [
    hscan("modal left edge", 400, 20, 60, **BL),
    hscan("modal right edge", 400, 1040, 1080, **BL),
    vscan("modal top edge", 600, 20, 60, **BL),
    vscan("modal bottom edge", 600, 720, 760, **BL),
    hscan("nav divider", 600, 270, 292),
    fill("nav fill", 150, 700),
    fill("content fill", 700, 650),
    ink("breadcrumb", 290, 45, 500, 75, **T),
    fill("backdrop", 1080, 400),
])

scene("terminal-drawer", chrome(), [
    vscan("drawer top border", 900, 485, 520),
    ink("terminal line 1", 258, 503, 700, 522, **T),
    ink("terminal line 2", 258, 522, 700, 540, **T),
    ink("terminal line 3", 258, 540, 700, 558, **T),
    ink("terminal toolbar", 1000, 505, 1095, 535),
    fill("terminal fill", 900, 650),
])

scene("right-panel-diff", chrome(), [
    hscan("panel left edge", 400, 600, 640),
    fill("panel fill", 900, 600),
    ink("panel header", 620, 52, 1100, 90),
])

# Captured by the audit (artifacts/audit/ref): the title's "Thread actions" menu.
scene("thread-actions-menu", chrome(), [
    hscan("menu left edge", 120, 360, 380, **BL),
    vscan("menu top edge", 450, 30, 60, **BL),
    vscan("menu bottom edge", 450, 405, 440, **BL),
    ink("item 1 icon", 380, 66, 400, 86),
    ink("item 1 label", 405, 64, 520, 88, **T),
    ink("snooze chevron", 545, 166, 570, 186),
    ink("delete label", 405, 402, 470, 426, **T),
])

# Captured by the audit: "Use `inline code` and **bold** here" then a TypeScript block.
scene("composer-markdown", chrome(), [
    hscan("code block left edge", 640, 300, 340, **BL),
    hscan("code block right edge", 640, 1020, 1060, **BL),
    vscan("code block top", 600, 600, 640, **BL),
    fill("code block fill", 700, 660),
    ink("language icon", 330, 630, 352, 652),
    ink("language label", 348, 628, 425, 650, **T),
    ink("code line 1", 330, 652, 500, 672, **T),
    ink("inline code chip", 345, 588, 440, 610),
    ink("prose line", 318, 582, 500, 612, **T),
])

scene("window-thread-light", [
    fill("titlebar fill", 600, 30),
    fill("sidebar fill", 240, 40),
    hscan("sidebar edge", 400, 250, 262),
    hscan("composer card sides", 700, 290, 1070),
    vscan("composer card top/bottom", 1046, 630, 779, **BL),
    fill("composer fill", 420, 700),
    ink("composer placeholder", 310, 662, 760, 690, **T),
    ink("row 1 title", 16, 133, 240, 157, **T),
    hscan("row 1 card edges", 140, 0, 256),
])

json.dump({"tolerance": {"px": 0.5, "text_px": 1.0, "channel": 1}, "scenes": scenes}, open(sys.argv[1], "w"), indent=1)
print(sum(len(s["elements"]) for s in scenes.values()), "elements")
