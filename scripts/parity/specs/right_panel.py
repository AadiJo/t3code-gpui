"""The right panel's diff tab."""
from .kit import fill, ink, hscan, chrome, scene

scene("right-panel-diff", chrome(), [
    hscan("panel left edge", 400, 600, 640),
    fill("panel fill", 900, 600),
    ink("panel header", 620, 52, 1100, 90),
])
