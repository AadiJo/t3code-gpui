"""The command palette. The owner scenes (interface 15, glass 40, contrast 95) render when
PARITY_CLIENT_SETTINGS names the owner's settings file; "-wide" is the owner's 1710x1074
window, and dx, dy move the 1100x780 boxes to it."""
from .kit import fill, ink, hscan, vscan, T, TR, BL, scene

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

FIX_PROJECTS = "fixture: project set, paths and environment label"
FIX_KEYS = "Linux: shortcut labels read Ctrl+Shift+N where the Mac shows the glyphs"

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
