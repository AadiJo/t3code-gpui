"""The composer: empty, focused, multiline, the model picker, mentions and markdown."""
from .kit import fill, ink, hscan, vscan, T, BL, chrome, composer, scene

scene("composer-empty", chrome(), composer())
scene("composer-focused", chrome(), composer())
scene("composer-multiline", chrome(), [
    hscan("composer left edge", 700, 290, 312, **BL),
    hscan("composer right edge", 700, 1040, 1070, **BL),
    vscan("composer card top/bottom", 800, 560, 779, **BL),
    fill("composer fill", 420, 650),
    ink("composer text", 310, 570, 900, 700, **T),
])
scene("composer-model-picker", chrome(), [
    hscan("picker left edge", 600, 440, 480, **BL),
    hscan("picker right edge", 600, 990, 1035, **BL),
    vscan("picker top edge", 700, 330, 370, **BL),
    vscan("picker bottom edge", 700, 690, 735, **BL),
    fill("picker rail fill", 482, 640),
    fill("picker list fill", 700, 640),
    fill("picker effort fill", 920, 600),
    fill("picker footer fill", 600, 702),
    vscan("search underline", 600, 375, 395),
    vscan("option pitch", 520, 398, 610, exclude="option rows hold fixture content (Claude catalog, not Replay)"),
])

scene("composer-mention", chrome(), [
    vscan("mention menu top", 700, 460, 500, **BL),
    vscan("mention rows", 340, 495, 650, exclude="rows hold fixture files"),
    fill("mention menu fill", 1000, 600),
])

# Captured by the audit: "Use `inline code` and **bold** here" then a TypeScript block.
scene("composer-markdown", chrome(), [
    hscan("code block left edge", 640, 300, 340, **BL),
    hscan("code block right edge", 640, 1020, 1060, **BL),
    vscan("code block top", 600, 600, 640, **BL),
    fill("code block fill", 700, 660),
    ink("language icon", 330, 630, 352, 652),
    ink("language label", 348, 628, 425, 650, **T),
    ink("code line 1", 330, 652, 500, 672, **T),
    ink("inline code chip", 345, 588, 440, 610),
    ink("prose line", 318, 582, 500, 612, **T),
])
