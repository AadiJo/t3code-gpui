"""Pages captured from their registered scenes (`SCENE_VIEWS` in parity_shot.rs)."""
from .kit import fill, ink, hscan, vscan, T, scene

scene("usage-limits-empty", [
    hscan("dialog left edge", 400, 36, 50),
    vscan("dialog top edge", 500, 30, 50),
    fill("dialog fill", 500, 400),
    ink("breadcrumb Usage", 61, 54, 106, 76, **T),
    ink("breadcrumb environments", 133, 53, 246, 77, **T),
    ink("environments chevron", 247, 56, 262, 73),
    ink("breadcrumb providers", 290, 54, 377, 76, **T),
    hscan("metric group edges", 74.5, 780, 990),
    ink("Limits pressed", 918, 52, 981, 77, **T),
    ink("refresh", 991, 50, 1019, 79),
    ink("close", 1023, 50, 1051, 79),
    vscan("header rule", 500, 80, 96),
    ink("empty message", 65, 113, 1035, 133, **T),
], reference="usage")

LINUX_TOPBAR = "Linux topbar is 40px, the Mac's 52px (theme_tokens.rs); the scene has no sidebar"
# The fork's list reserves a 6px scrollbar gutter (classic scrollbars in the capture browser); overlay scrollbars on macOS reserve none.
SIDEBAR = {"port_dx": -262, "port_dy": -12, "note": LINUX_TOPBAR + "; 6px scrollbar gutter in the capture"}
RIGHT = {"port_dx": 6, "port_dy": -12, "note": LINUX_TOPBAR + "; 6px scrollbar gutter in the capture"}
scene("pull-requests-toolbar", [
    hscan("search box left edge", 93, 280, 300, **SIDEBAR),
    vscan("search box top/bottom", 500, 70, 116, **SIDEBAR),
    ink("search icon", 294, 83, 314, 103, **SIDEBAR),
    ink("search placeholder", 318, 83, 600, 103, **T, **SIDEBAR),
    ink("sort button", 685, 77, 756, 109, **RIGHT),
    ink("views button", 858, 77, 960, 109, **RIGHT),
    ink("provider filter", 968, 77, 1030, 109, **RIGHT),
    ink("refresh button", 1038, 77, 1070, 109, **RIGHT),
], reference="pull-requests")
scene("issues-toolbar", [
    hscan("search box left edge", 93, 280, 300, **SIDEBAR),
    vscan("search box top/bottom", 500, 70, 116, **SIDEBAR),
    ink("search icon", 294, 83, 314, 103, **SIDEBAR),
    ink("search placeholder", 318, 83, 600, 103, **T, **SIDEBAR),
    ink("sort button", 755, 77, 826, 109, **RIGHT),
    ink("filters button", 834, 77, 920, 109, **RIGHT),
    ink("views button", 928, 77, 1030, 109, **RIGHT),
    ink("refresh button", 1038, 77, 1070, 109, **RIGHT),
], reference="issues")
