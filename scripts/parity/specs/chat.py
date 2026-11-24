"""The conversation: timeline turns, the user bubble and the header's thread actions menu."""
from .kit import fill, ink, hscan, vscan, T, TR, BL, chrome, composer, scene

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
