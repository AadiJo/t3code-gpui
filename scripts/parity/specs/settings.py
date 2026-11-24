"""The settings modal: Connections and the client settings pages."""
from .kit import fill, ink, hscan, vscan, T, TR, BL, sidebar, scene

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
