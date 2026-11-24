"""The terminal drawer, its context menu and the close confirmation."""
from .kit import fill, ink, hscan, vscan, T, BL, chrome, scene

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
