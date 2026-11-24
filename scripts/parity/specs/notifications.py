"""The notification inbox popover, at the shared and at the owner's settings."""
from .kit import fill, ink, T, TR, chrome, sidebar, scene

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
