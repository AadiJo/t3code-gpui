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
        ink("update button", 210, 740, 245, 772, exclude="Check for updates: the native shell has its own updater, so the port leaves it out"),
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

scene("terminal-menu", chrome(), [
    hscan("menu left/right edges", 700, 390, 540),
    vscan("menu top/bottom", 470, 615, 779),
    fill("menu fill", 515, 700),
    ink("menu add to chat (disabled)", 405, 634, 520, 657, **T),
    ink("menu copy (disabled)", 405, 662, 520, 685, **T),
    ink("menu paste", 405, 690, 520, 713, **T),
    ink("menu select all", 405, 718, 520, 741, **T),
    ink("menu jump to latest", 405, 746, 525, 769, **T),
])

scene("terminal-close-confirm", [
    hscan("dialog left/right edges", 400, 280, 820),
    vscan("dialog top/bottom", 550, 290, 490),
    fill("dialog body fill", 700, 390),
    fill("dialog footer fill", 450, 445),
    ink("dialog title", 315, 330, 640, 362, **T),
    ink("dialog description", 315, 365, 700, 392, **T),
    ink("cancel and confirm", 615, 420, 790, 462),
    fill("backdrop", 150, 600, **BL),
])

scene("right-panel-diff", chrome(), [
    hscan("panel left edge", 400, 600, 640),
    fill("panel fill", 900, 600),
    ink("panel header", 620, 52, 1100, 90),
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

# Owner scenes (interface 15, glass 40, contrast 95), rendered by parity-shot
# when PARITY_CLIENT_SETTINGS names the owner's settings file. "-wide" is the
# owner's 1710x1074 window; dx, dy move the 1100x780 boxes to it.
FIX_GIT = "fixture: the thread's git state, editor, machine and scripts are fixture content"
FIX_PROJECTS = "fixture: project set, paths and environment label"
FIX_KEYS = "Linux: shortcut labels read Ctrl+Shift+N where the Mac shows the glyphs"

def details(dx=0, dy=0):
    return [
        hscan("card left edge", 150 + dy, 815 + dx, 840 + dx, **BL),
        hscan("card right edge", 150 + dy, 1075 + dx, 1100 + dx, **BL),
        vscan("card top edge", 950 + dx, 50 + dy, 70 + dy, **BL),
        ink("workspace rows", 840 + dx, 66 + dy, 1080 + dx, 160 + dy, exclude=FIX_GIT),
        vscan("section rule", 1000 + dx, 160 + dy, 175 + dy),
        ink("branch icon", 840 + dx, 178 + dy, 864 + dx, 204 + dy),
        ink("branch label", 866 + dx, 178 + dy, 1040 + dx, 204 + dy, **T),
        ink("branch chevron", 1050 + dx, 178 + dy, 1080 + dx, 204 + dy),
        ink("git action row", 840 + dx, 208 + dy, 1040 + dx, 234 + dy, exclude=FIX_GIT),
        ink("changes label", 866 + dx, 238 + dy, 928 + dx, 264 + dy, **T),
        ink("changes counts", 960 + dx, 238 + dy, 1080 + dx, 264 + dy, exclude=FIX_GIT),
    ]

def palette(dx=0, dy=0):
    return [
        hscan("palette left edge", 300 + dy, 260 + dx, 300 + dx, **BL),
        hscan("palette right edge", 300 + dy, 812 + dx, 840 + dx, **BL),
        vscan("palette top edge", 550 + dx, 60 + dy, 100 + dy, **BL),
        ink("search icon", 292 + dx, 88 + dy, 316 + dx, 116 + dy),
        ink("search placeholder", 318 + dx, 88 + dy, 640 + dx, 116 + dy, **T),
        ink("section label", 286 + dx, 134 + dy, 400 + dx, 156 + dy, **T),
        fill("highlighted item", 600 + dx, 172 + dy),
        ink("item 1 icon", 292 + dx, 160 + dy, 316 + dx, 186 + dy),
        ink("item 1 title", 318 + dx, 160 + dy, 560 + dx, 186 + dy, **T),
        ink("item 1 shortcut", 700 + dx, 160 + dy, 810 + dx, 186 + dy, exclude=FIX_KEYS),
        ink("item 2 title", 318 + dx, 190 + dy, 560 + dx, 216 + dy, **T),
        ink("item 3 title", 318 + dx, 220 + dy, 560 + dx, 246 + dy, **T),
    ]

scene("details", details())
scene("details-wide", details(dx=610, dy=4.5), reference="details-wide")
scene("palette-root", palette())
scene("palette-root-wide", palette(dx=305, dy=29.4), reference="palette-root-wide")
scene("palette-query-sync", [
    ink("section label", 286, 134, 400, 156, **T),
    ink("thread icon", 292, 160, 316, 186),
    ink("thread title", 318, 160, 560, 182, **T),
    ink("thread subtitle", 318, 182, 640, 200, **T),
    ink("thread time", 720, 166, 810, 186, **TR, exclude="fixture: relative time"),
    vscan("footer rule", 600, 200, 240),
])
scene("palette-nested-project", [
    ink("back arrow", 292, 88, 316, 116),
    ink("search placeholder", 318, 88, 640, 116, **T),
    ink("section label", 286, 134, 400, 156, **T),
    ink("project 1 favicon", 292, 160, 316, 186),
    ink("project 1 title", 318, 160, 560, 182, **T),
    ink("project 1 location", 318, 182, 760, 200, exclude=FIX_PROJECTS),
    ink("no project row", 292, 214, 560, 240),
    ink("project 2 favicon", 292, 240, 316, 266),
])
INBOX = {"anchor": "inbox title", "anchor_x": True}
scene("inbox", [
    ink("inbox popover", 130, 500, 520, 745, exclude="its place follows the sidebar footer's bell, which the sidebar owns"),
    ink("inbox title", 150, 525, 260, 545, **T, exclude="locator for the rows below"),
    ink("mark all read", 400, 525, 500, 545, **TR, **INBOX),
    ink("tabs", 150, 552, 260, 574, **T, **INBOX),
    ink("item 1 icon", 155, 590, 178, 612, **INBOX),
    ink("item 1 title", 178, 588, 420, 606, **T, **INBOX),
    ink("item 1 subtitle", 178, 606, 420, 621, **T, **INBOX),
    ink("item 1 meta", 178, 621, 420, 636, **T, **INBOX),
    ink("item 1 read button", 460, 600, 495, 625, **INBOX),
    fill("inbox fill", 450, 700),
])

json.dump({"tolerance": {"px": 0.5, "text_px": 1.0, "channel": 1}, "scenes": scenes}, open(sys.argv[1], "w"), indent=1)
print(sum(len(s["elements"]) for s in scenes.values()), "elements")
