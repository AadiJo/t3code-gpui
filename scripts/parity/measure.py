"""Pixel measurements shared by record and compare. Coordinates are logical px;
images are 2x, so one device pixel is 0.5 logical px.

Kinds (one per element in spec.json):
  fill  [x, y]                 colour at a point (median of the 2x2 device pixels)
  ink   [x0, y0, x1, y1]       bounding box of pixels that differ from the box's
                               background (its most common colour) by more than
                               `threshold`, plus the ink colour (the pixel furthest
                               from the background)
  hscan [y, x0, x1]            colour runs along a row: each edge where the colour
  vscan [x, y0, y1]            changes, collapsing anti-aliased steps
"""
from collections import Counter

from PIL import Image

SCALE = 2


def load(path):
    return Image.open(path).convert("RGB")


def hexc(c):
    return "#%02x%02x%02x" % tuple(c)


def dist(a, b):
    return max(abs(a[i] - b[i]) for i in range(3))


def fill(im, x, y):
    px = im.load()
    X, Y = int(x * SCALE), int(y * SCALE)
    samples = sorted(px[X + dx, Y + dy] for dx in (0, 1) for dy in (0, 1))
    return {"color": hexc(samples[1])}


def ink(im, x0, y0, x1, y1, threshold=24):
    px = im.load()
    X0, Y0, X1, Y1 = (int(v * SCALE) for v in (x0, y0, x1, y1))
    bg = Counter(px[x, y] for x in range(X0, X1) for y in range(Y0, Y1)).most_common(1)[0][0]
    top = left = None
    bottom = right = None
    best, best_d = bg, 0
    for y in range(Y0, Y1):
        for x in range(X0, X1):
            d = dist(px[x, y], bg)
            if d > threshold:
                top = y if top is None else top
                bottom = y
                left = x if left is None or x < left else left
                right = x if right is None or x > right else right
                if d > best_d:
                    best, best_d = px[x, y], d
    if top is None:
        return {"empty": True, "bg": hexc(bg)}
    return {
        "top": top / SCALE,
        "left": left / SCALE,
        "bottom": (bottom + 1) / SCALE,
        "right": (right + 1) / SCALE,
        "color": hexc(best),
        "bg": hexc(bg),
    }


def runs(values, start, threshold=6):
    """Edges along a scan: positions where a new stable colour begins."""
    edges = []
    current = values[0]
    i = 1
    while i < len(values):
        if dist(values[i], current) > threshold:
            # Skip anti-aliased transition pixels to the next stable colour.
            j = i
            while j + 1 < len(values) and dist(values[j + 1], values[j]) > threshold:
                j += 1
            current = values[j]
            edges.append({"at": (start + i) / SCALE, "color": hexc(current)})
            i = j + 1
        else:
            i += 1
    return {"start": hexc(values[0]), "edges": edges}


def hscan(im, y, x0, x1):
    px = im.load()
    Y = int(y * SCALE)
    return runs([px[x, Y] for x in range(int(x0 * SCALE), int(x1 * SCALE))], int(x0 * SCALE))


def vscan(im, x, y0, y1):
    px = im.load()
    X = int(x * SCALE)
    return runs([px[X, y] for y in range(int(y0 * SCALE), int(y1 * SCALE))], int(y0 * SCALE))


def measure(im, element):
    kind = element["kind"]
    args = element["at"]
    if kind == "fill":
        return fill(im, *args)
    if kind == "ink":
        return ink(im, *args, element.get("threshold", 24))
    if kind == "hscan":
        return hscan(im, *args)
    if kind == "vscan":
        return vscan(im, *args)
    raise ValueError(f"unknown kind {kind}")
