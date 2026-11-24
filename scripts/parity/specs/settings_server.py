"""The server settings pages, each measured relative to an anchor right of the scope row."""
from .kit import ink, hscan, T, TR, scene

# Server settings pages (fixture data; references are the audit's captures of the fork,
# `artifacts/audit/ref/settings-*.png`). The shell draws the scope row above these pages,
# so each page is measured relative to a control right of that row: the anchor.
LOC = "locator: a control right of the scope row, so the page is judged apart from the shell above it"
def server_page(name, anchor, *elements):
    a = {"anchor": anchor["name"]}
    scene(name, [{**anchor, "exclude": LOC}], [{**e, **a} for e in elements])

server_page("settings-providers", ink("refresh icon", 903, 100, 921, 215),
    ink("heading", 325, 165, 402, 206, **T),
    ink("add button", 405, 175, 428, 199),
    hscan("card left edge", 300, 300, 330),
    hscan("card right edge", 300, 1010, 1040),
    ink("row 1 mark", 328, 226, 348, 250),
    ink("row 1 title", 358, 226, 420, 250, **T),
    ink("row 1 switch", 978, 236, 1016, 260),
)
server_page("settings-source-control", ink("first switch", 978, 100, 1016, 300),
    ink("heading", 325, 165, 425, 206, **T),
    ink("row 1 title", 326, 220, 452, 246, **T),
    ink("row 2 title", 326, 305, 575, 331, **T),
    ink("row 2 switch", 978, 326, 1016, 352),
    hscan("merge method select", 424, 840, 1020),
)
server_page("settings-worktrees", ink("start from remote switch", 978, 285, 1016, 420),
    ink("heading", 325, 165, 395, 206, **T),
    ink("row 1 title", 326, 220, 447, 246, **T),
    hscan("location input", 263, 765, 1020),
    ink("advanced chevron", 326, 409, 344, 431),
    ink("advanced label", 344, 409, 400, 431, **T),
    ink("branches heading", 325, 465, 402, 506, **T),
)
server_page("settings-maintenance", ink("first switch", 978, 100, 1016, 300),
    ink("heading", 325, 165, 395, 206, **T),
    ink("row 1 title", 326, 220, 489, 246, **T),
    ink("row 2 switch", 978, 326, 1016, 352),
    ink("background heading", 325, 406, 483, 447, **T),
    hscan("profile select", 486, 840, 1020),
    ink("artifacts heading", 325, 629, 466, 670, **T),
    ink("artifacts off label", 948, 698, 974, 720, **TR),
)
server_page("settings-threads", ink("clone switch", 978, 262, 1016, 360),
    ink("heading", 325, 165, 430, 206, **T),
    ink("row 1 title", 326, 220, 468, 246, **T),
    hscan("starts-in input", 245, 715, 1020),
    ink("new threads heading", 325, 370, 426, 411, **T),
)
server_page("settings-projects", ink("row 1 chevron", 990, 100, 1016, 300),
    ink("intro", 309, 170, 800, 196, **T),
    hscan("card left edge", 236, 300, 330),
    hscan("card right edge", 236, 1010, 1040),
    ink("row 1 icon", 326, 226, 346, 246),
    ink("row 1 title", 354, 216, 420, 240, **T),
    ink("row 2 icon", 326, 287, 346, 307),
)
server_page("settings-browser", ink("first switch", 978, 100, 1016, 290),
    ink("heading", 325, 165, 436, 206, **T),
    ink("row 1 title", 326, 220, 481, 246, **T),
    ink("browsing heading", 325, 303, 402, 344, **T),
    hscan("add profile button", 383, 890, 1020),
    hscan("profile list edge", 433, 320, 340),
)
server_page("settings-devices", ink("first switch", 978, 100, 1016, 290),
    ink("heading", 325, 165, 390, 206, **T),
    ink("row 1 title", 326, 220, 413, 246, **T),
    ink("sources heading", 325, 370, 447, 411, **T),
    hscan("add source button", 392, 870, 1020),
    ink("local simulators", 326, 425, 440, 451, **T),
)
