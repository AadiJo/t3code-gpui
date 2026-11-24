"""Pixel measurements shared by record and compare. Coordinates are logical px;
images are 2x, so one device pixel is 0.5 logical px.

Kinds (one per element in the spec, gen_spec.py):
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
    """The median colour of the 6x6 device pixels at a point (grain averages out)."""
    px = im.load()
    X, Y = int(x * SCALE), int(y * SCALE)
    samples = [px[X + dx, Y + dy] for dx in range(-3, 3) for dy in range(-3, 3)]
    return {"color": hexc(tuple(sorted(c[k] for c in samples)[len(samples) // 2] for k in range(3)))}


def ink(im, x0, y0, x1, y1, threshold=24):
    px = im.load()
    X0, Y0, X1, Y1 = (int(v * SCALE) for v in (x0, y0, x1, y1))
    X1, Y1 = min(X1, im.width), min(Y1, im.height)
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


def settled(values, i):
    """The median of up to 6 pixels from `i`: a run's colour without grain noise."""
    window = values[i : i + 6] or values[-1:]
    return tuple(sorted(c[k] for c in window)[len(window) // 2] for k in range(3))


def runs(values, start, threshold=6):
    """Edges along a scan: positions where a new stable colour begins."""
    edges = []
    current = values[0]
    first = settled(values, 0)
    i = 1
    while i < len(values):
        if dist(values[i], current) > threshold:
            # Skip anti-aliased transition pixels to the next stable colour.
            j = i
            while j + 1 < len(values) and dist(values[j + 1], values[j]) > threshold:
                j += 1
            current = values[j]
            edges.append({"at": (start + i) / SCALE, "color": hexc(settled(values, j))})
            i = j + 1
        else:
            i += 1
    return {"start": hexc(first), "edges": edges}


def hscan(im, y, x0, x1):
    px = im.load()
    Y = int(y * SCALE)
    return runs([px[x, Y] for x in range(int(x0 * SCALE), int(x1 * SCALE))], int(x0 * SCALE))


def vscan(im, x, y0, y1):
    px = im.load()
    X = int(x * SCALE)
    return {**runs([px[X, y] for y in range(int(y0 * SCALE), int(y1 * SCALE))], int(y0 * SCALE)), "vertical": True}


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


def shifted(element, dy):
    """The element with its vertical coordinates moved by `dy`."""
    if not dy:
        return element
    at = list(element["at"])
    kind = element["kind"]
    if kind == "fill":
        at[1] += dy
    elif kind == "ink":
        at[1] += dy
        at[3] += dy
    elif kind == "hscan":
        at[0] += dy
    else:
        at[1] += dy
        at[2] += dy
    return {**element, "at": at}


def unshift(result, dy):
    """A measurement taken at a shifted position, moved back by `dy`."""
    if not dy or result.get("empty"):
        return result
    out = dict(result)
    for key in ("top", "bottom"):
        if key in out:
            out[key] -= dy
    if "edges" in out and isinstance(out.get("edges"), list) and out["edges"] and "vertical" in out:
        out["edges"] = [{**e, "at": e["at"] - dy} for e in out["edges"]]
    return out


def moved(element, dx, dy):
    """The element moved by `dx` and `dy`, for a port image whose layout is offset."""
    element = shifted(element, dy)
    if not dx:
        return element
    at = list(element["at"])
    kind = element["kind"]
    if kind in ("fill", "vscan"):
        at[0] += dx
    elif kind == "ink":
        at[0] += dx
        at[2] += dx
    else:
        at[1] += dx
        at[2] += dx
    return {**element, "at": at}


def unmoved(result, dx, dy):
    """A measurement from `moved`, back in reference coordinates."""
    result = unshift(result, dy)
    if not dx or result.get("empty"):
        return result
    out = dict(result)
    for key in ("left", "right"):
        if key in out:
            out[key] -= dx
    if isinstance(out.get("edges"), list) and "vertical" not in out:
        out["edges"] = [{**e, "at": e["at"] - dx} for e in out["edges"]]
    return out
