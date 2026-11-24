"""The thread details card at the owner's settings (see palette.py for the owner scenes)."""
from .kit import ink, hscan, vscan, T, BL, scene

FIX_GIT = "fixture: the thread's git state, editor, machine and scripts are fixture content"

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

scene("details", details())
scene("details-wide", details(dx=610, dy=4.5), reference="details-wide")
