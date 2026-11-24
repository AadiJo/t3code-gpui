"""The element builders and scene registry every area module uses. Coordinates are logical
px in the 1100x780 reference captures; see compare.py for what each kind and flag judges."""

def fill(name, x, y, **k): return {"name": name, "kind": "fill", "at": [x, y], **k}
def ink(name, x0, y0, x1, y1, **k): return {"name": name, "kind": "ink", "at": [x0, y0, x1, y1], **k}
def hscan(name, y, x0, x1, **k): return {"name": name, "kind": "hscan", "at": [y, x0, x1], **k}
def vscan(name, x, y0, y1, **k): return {"name": name, "kind": "vscan", "at": [x, y0, y1], **k}
T = {"text": True}
TR = {"text": "right"}
BL = {"blur": True}
FIX_MODEL = "fixture: the composer catalog is the Claude fixture (thread_route_view.rs), not Replay"

scenes = {}
def scene(name, *groups, reference=None):
    body = {"elements": [e for g in groups for e in g]}
    if reference: body["reference"] = reference
    scenes[name] = body

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
