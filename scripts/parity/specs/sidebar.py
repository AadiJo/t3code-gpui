"""Window chrome, the sidebar and its rows (reference captures of the fork at 1100x780)."""
from .kit import fill, ink, hscan, vscan, T, TR, BL, chrome, sidebar, composer, scene

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

# The owner's configuration (interface 15px, repository grouping with an
# override, Working shelf), captured from the fork at 1100x780 with the owner's
# client settings; the port renders them with PARITY_CLIENT_SETTINGS.
OWNER_LINUX = "the headless shot runs on Linux, whose shortcut labels read Ctrl+1 where the owner's Mac reads \u23181"
scene("owner-sb-top", [
    fill("sidebar fill", 240, 40),
    ink("search placeholder", 38, 62, 110, 90, **T),
    ink("header actions", 150, 60, 250, 92),
    vscan("draft 1 top/bottom", 20, 95, 176),
    ink("draft 1 pen", 14, 104, 30, 122),
    ink("draft 1 project", 54, 106, 160, 124, **T),
    ink("draft 1 title", 16, 125, 240, 148, **T),
    vscan("card 1 top/bottom", 12, 250, 350),
    ink("card 1 project", 37, 274, 160, 295, **T),
    ink("card 1 time", 200, 274, 242, 295, **TR),
    ink("card 1 title", 16, 296, 240, 319, **T),
    ink("card 1 branch", 16, 316, 60, 336, **T),
    ink("card 1 provider", 222, 318, 245, 338),
    ink("row 2 status", 175, 351, 242, 372, **TR),
    ink("row 3 status", 175, 429, 242, 450, **TR),
], reference="sb-top")
scene("owner-sb-shelves-collapsed", [
    ink("last card title", 16, 591, 240, 614, **T),
    ink("working label", 16, 644, 70, 664, **T),
    ink("snoozed label", 16, 675, 75, 695, **T),
    ink("settled label", 16, 706, 65, 726, **T),
    ink("shelf counts", 205, 640, 225, 730),
    ink("shelf chevrons", 228, 640, 245, 730),
], reference="sb-shelves-collapsed")
scene("owner-sb-shelves-expanded", [
    ink("snoozed 2 title", 41, 498, 195, 522, **T),
    ink("snoozed 2 time", 200, 500, 242, 520, **TR),
    ink("settled label", 16, 533, 60, 553, **T),
    ink("settled 1 title", 41, 564, 200, 588, **T),
    ink("settled 1 time", 215, 566, 242, 586, **TR),
    vscan("hovered settled row", 10, 690, 738),
    ink("settled 5 title", 41, 703, 200, 727, **T),
], reference="sb-shelves-expanded")
scene("owner-sb-thread-menu", [
    hscan("menu left/right edges", 600, 120, 330, **BL),
    vscan("menu top/bottom", 220, 390, 780, **BL),
    ink("item 1 label", 161, 406, 320, 429, **T),
    ink("rename label", 161, 519, 320, 543, **T),
    ink("delete label", 161, 746, 250, 770, **T),
], reference="sb-thread-menu")
scene("owner-sb-draft-menu", [
    hscan("menu left/right edges", 260, 120, 280, **BL),
    vscan("menu top/bottom", 200, 205, 315, **BL),
    ink("copy label", 161, 219, 240, 242, **T),
    ink("discard label", 161, 280, 265, 304, **T),
], reference="sb-draft-menu")
scene("owner-sb-rename", [
    hscan("rename field edges", 540, 5, 250),
    vscan("rename field top/bottom", 100, 525, 556),
    ink("rename text", 14, 533, 200, 550, **T),
], reference="sb-rename")
scene("owner-sb-jump-hints", [
    vscan("hint 1 chip", 232, 270, 300),
    ink("hint 1 label", 200, 274, 242, 295, exclude=OWNER_LINUX),
], reference="sb-jump-hints")
scene("owner-sb-filter", [
    hscan("popup left/right edges", 200, 160, 450, **BL),
    vscan("popup top/bottom", 300, 85, 330, **BL),
    ink("search icon", 180, 105, 198, 123),
    ink("search placeholder", 200, 104, 330, 124, **T),
    fill("highlighted row fill", 300, 150),
    ink("all projects label", 203, 141, 280, 162, **T),
    ink("row 2 label", 203, 171, 310, 192, **T),
    ink("row 6 label", 203, 291, 240, 312, **T),
], reference="sb-filter")
scene("owner-sb-drag", [
    ink("pinned label", 16, 268, 58, 286, **T),
    ink("active label", 16, 292, 56, 310, **T),
], reference="sb-drag")
