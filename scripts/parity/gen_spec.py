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


# Settings pages (ConnectionsSettings excluded): boxes come from the
# reference outlines (`artifacts/audit/ref/settings-*.outline.txt`), each
# padded so the ink lands inside. Switch states and select values follow
# the client settings, which differ between the captures and the port's
# defaults, so only geometry is judged there.
def box(name, x, y, w, h, pad=4, **k):
    return ink(name, x - pad, y - pad, x + w + pad, y + h + pad, **k)

def control(name, x, y, w, h):
    """A bordered control: its left and right edges at mid-height and its
    top and bottom 3px inside the left edge, where no text runs. Ink boxes
    miss a 1px border that sits within the ink threshold of its fill."""
    mid = y + h / 2
    return [
        hscan(f"{name} left edge", mid, x - 6, x + 6),
        hscan(f"{name} right edge", mid, x + w - 6, x + w + 6),
        vscan(f"{name} top/bottom", x + 3, y - 6, y + h + 6),
    ]

def settings_chrome(title_w):
    return [
        fill("modal fill (content)", 700, 760 - 30),
        box("nav search kbd", 244, 55, 20, 20),
        box("breadcrumb", 301, 55, 85 + title_w, 20, **T),
        box("close button", 1023, 51, 28, 28),
    ]

def scope_row():
    return [box("scope sentence", 327, 113, 459, 28)]

GROUP_EDGES = [hscan("group left edge", 400, 300, 325), hscan("group right edge", 400, 1015, 1040)]

scene("settings-general", settings_chrome(51), GROUP_EDGES, [
    *control("restore defaults", 855, 53, 164, 24),
    box("h2 Notifications", 327, 113, 99, 28, **T),
    vscan("notifications card top", 600, 140, 165),
    box("thread notifications title", 328, 164, 134, 20, **T),
    box("thread notifications description", 328, 188, 428, 36, **T),
    *control("thread notifications select", 788, 180, 224, 28),
    box("in-app title", 328, 249, 131, 20, **T),
    box("h2 Sidebar", 327, 470, 58, 28, **T),
    *control("project order select", 836, 595, 176, 28),
    box("working section title", 328, 655, 108, 20, **T),
])
scene("settings-appearance", settings_chrome(81), [
    box("h3 Color scheme", 327, 113, 105, 28, **T),
    box("system tile", 311, 153, 231, 180, pad=3),
    box("dark tile", 798, 153, 231, 180, pad=3),
    box("h3 Themes", 327, 365, 62, 24, **T),
    *control("create theme", 805, 365, 108, 24),
    *control("add theme", 921, 365, 92, 24),
    box("T3 Code light sphere", 416, 414, 68, 68, pad=0),
    box("T3 Code name", 324, 492, 57, 20, **T),
    *control("T3 Code duplicate", 629, 490, 24, 24),
    box("T3 Chat light sphere", 779, 414, 68, 68, pad=0),
    box("Grove light sphere", 416, 548, 68, 68, pad=0),
])
scene("settings-chat", settings_chrome(31), GROUP_EDGES, [
    box("h2 Composer", 327, 113, 80, 28, **T),
    box("send shortcut title", 328, 231, 93, 20, **T),
    *control("send shortcut select", 852, 238, 160, 28),
    box("advanced", 312, 286, 160, 40, **T),
    box("h2 Follow-ups", 327, 359, 85, 28, **T),
    *control("follow-up select", 852, 426, 160, 28),
    box("h2 Activity", 327, 515, 59, 28, **T),
    *control("tool calls select", 852, 582, 160, 28),
    box("h2 Screen reader", 327, 671, 106, 28, **T),
])
scene("settings-diffs", settings_chrome(136), GROUP_EDGES, [
    box("h2 Diffs", 327, 113, 36, 28, **T),
    *control("diff layout select", 852, 171, 160, 28),
    *control("file state select", 852, 305, 160, 28),
    *control("diff colors select", 852, 372, 160, 28),
    box("diff colors value", 862, 376, 124, 20, **T),
    box("h2 Pull requests", 327, 452, 100, 28, **T),
    box("open in T3 Code title", 328, 570, 205, 20, **T),
])
scene("settings-keybindings", settings_chrome(84), scope_row(), [
    box("h2 Keybindings", 327, 173, 95, 28, **T),
    *control("search field", 328, 224, 485, 34),
    box("search icon", 338, 234, 14, 14),
    *control("add keybinding", 821, 225, 151, 32),
    *control("keybindings.json", 980, 225, 32, 32),
    box("h2 Navigation", 327, 303, 83, 28, **T),
    box("row 1 title", 328, 354, 172, 20, **T),
    box("row 1 when", 328, 380, 150, 20, **T),
    box("row 1 keycaps", 967, 368, 44, 20, **TR),
    box("row 2 title", 328, 427, 141, 20, **T),
    box("row 2 when Always", 328, 453, 101, 20, **T),
    box("row 2 keycaps", 967, 441, 44, 20, **TR),
    box("row 3 keycaps", 967, 514, 44, 20, **TR),
])
scene("settings-voice", settings_chrome(37), scope_row(), [
    box("h2 Input", 327, 173, 40, 28, **T),
    *control("refresh microphones", 984, 231, 28, 28),
    box("microphone list", 328, 278, 684, 36, pad=1),
    *control("record button", 927, 348, 85, 28),
    *control("dictation select", 852, 415, 160, 28),
    box("h2 Transcription", 327, 495, 103, 28, **T),
    *control("transcribe with select", 852, 553, 160, 28),
    box("h2 Dictionary", 327, 767, 79, 13, pad=0, exclude="cut by the window edge"),
])
scene("settings-snap-shot", settings_chrome(72), GROUP_EDGES, [
    box("h2 SnapShots", 327, 113, 84, 28, **T),
    box("title", 328, 164, 72, 20, **T),
    box("description", 328, 188, 300, 18, **T),
    box("status", 328, 210, 200, 18, **T),
])
scene("settings-labs", settings_chrome(33), scope_row(), [
    box("h2 Beta", 327, 173, 35, 28, **T),
    box("boring sidebar title", 328, 224, 223, 20, **T),
    box("guest links title", 328, 309, 75, 20, **T),
    box("sharing & access link", 556, 352, 98, 15, **T),
    box("linked environments title", 328, 394, 139, 20, **T),
    box("simlock title", 328, 564, 194, 20, **T),
])
scene("settings-archived", settings_chrome(52), scope_row(), [
    box("h2 Archived threads", 327, 173, 132, 28, **T),
    box("empty title with icon", 328, 224, 158, 20, exclude="fixture: the parity world settings server is not connected, so the page shows the load error instead of the empty state"),
    box("empty description", 328, 250, 200, 18, exclude="fixture: the parity world settings server is not connected, so the page shows the load error instead of the empty state"),
    vscan("card top/bottom", 600, 200, 300, exclude="fixture: the parity world settings server is not connected, so the page shows the load error instead of the empty state"),
])
scene("settings-about", settings_chrome(40), [
    box("h2 App", 327, 113, 32, 28, **T),
    box("version title", 328, 164, 50, 20, **T),
    box("version copy", 386, 169, 36, 16, **T),
    *control("update track select", 852, 248, 160, 28),
    *control("iPhone and iPad", 767, 324, 146, 28),
    *control("Android", 921, 324, 91, 28),
    box("check for updates", 867, 172, 145, 28, exclude="the native app has no self-updater yet; see the settings report"),
    box("h2 Legal", 327, 471, 41, 28, exclude="sits 67px higher: the Welcome page row needs a primary environment this remote-only app does not have"),
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
