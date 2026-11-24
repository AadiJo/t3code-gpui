"""The `mac` parity mode: port renders judged against the fork rendered on the owner's Mac.

Both sides are drawn on the same Mac with the same fonts (SF Pro, Monaco, PT Mono), so text is
judged too: per element the glyph ink box (left, top, width, height), the baseline and the ink
colour, at 0.5px and 1/255. Boxes with a visible fill or border are judged by their edges and
fill colour.

  PARITY_MODE=mac python3 compare.py record <ref-dir> [view ...]
  PARITY_MODE=mac python3 compare.py compare <port-dir> <out-dir> [ref-dir]

`record` reads each `<view>.png` with its element JSON (boxes, computed styles, text) from the
Mac capture and writes `expected/mac/<view>.json`: the elements it judges and the values
measured from the reference image. Only numbers, boxes, font strings and fixture labels are
written. Any element whose text could name the machine (absolute paths, the user, host names,
`PARITY_SCRUB=word,word`) is dropped, and in the terminal views every text element is dropped
and only geometry kept, since a terminal prompt shows the user and host. Scroll pages
(`<view>.scroll-<n>.png`) have no element JSON; they get side-by-sides but are not judged.

`compare` finds each reference text in the port image (best ink match within 24px of where the
reference has it), measures it there, and writes `report.md` (per scene pass and fail, every
failing element with both values, grouped by cause and area), `results.json` and side-by-sides.
"""
import getpass
import json
import os
import re
import socket
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw

HERE = Path(__file__).resolve().parent
EXPECTED = HERE / "expected" / "mac"
SCALE = 2
PX = 0.5  # logical px
CHANNEL = 1  # per channel, of 255
INK = 24  # a pixel is ink when a channel differs from the background by more than this
SEARCH = 48  # device px searched around the reference position (24 logical px)
MONO = ("Monaco", "PT Mono", "monospace", "ui-monospace", "Menlo")


def load(path):
    return np.asarray(Image.open(path).convert("RGB"), dtype=np.int16)


def hexc(c):
    return "#%02x%02x%02x" % tuple(int(v) for v in c)


def unhex(h):
    h = h.lstrip("#")
    return [int(h[i : i + 2], 16) for i in (0, 2, 4)]


def channels(a, b):
    return max(abs(x - y) for x, y in zip(unhex(a), unhex(b)))


def mode_colour(region):
    flat = region.reshape(-1, 3)
    packed = (flat[:, 0].astype(np.int64) << 16) | (flat[:, 1].astype(np.int64) << 8) | flat[:, 2]
    values, counts = np.unique(packed, return_counts=True)
    v = values[counts.argmax()]
    return np.array([(v >> 16) & 255, (v >> 8) & 255, v & 255], dtype=np.int16)


def clip(im, x0, y0, x1, y1):
    h, w = im.shape[:2]
    return max(0, int(x0)), max(0, int(y0)), min(w, int(x1)), min(h, int(y1))


def ink_mask(im, x0, y0, x1, y1, bg=None):
    x0, y0, x1, y1 = clip(im, x0, y0, x1, y1)
    region = im[y0:y1, x0:x1]
    if region.size == 0:
        return None, None, (x0, y0)
    bg = mode_colour(region) if bg is None else bg
    mask = np.abs(region - bg).max(axis=2) > INK
    return mask, bg, (x0, y0)


def measure_ink(im, x0, y0, x1, y1, bg=None):
    """Ink box, baseline and colours of the text in a device-px region, in logical px."""
    mask, bg, (ox, oy) = ink_mask(im, x0, y0, x1, y1, bg)
    if mask is None or not mask.any():
        return {"empty": True, "bg": hexc(bg) if bg is not None else "#000000"}
    rows, cols = np.nonzero(mask)
    top, bottom, left, right = rows.min(), rows.max() + 1, cols.min(), cols.max() + 1
    counts = mask.sum(axis=1)
    dense = np.nonzero(counts >= 0.35 * counts.max())[0]
    baseline = dense.max() + 1
    region = im[oy : oy + mask.shape[0], ox : ox + mask.shape[1]]
    diff = np.abs(region - bg).max(axis=2)
    iy, ix = np.unravel_index(diff.argmax(), diff.shape)
    return {
        "left": (ox + left) / SCALE,
        "top": (oy + top) / SCALE,
        "width": (right - left) / SCALE,
        "height": (bottom - top) / SCALE,
        "baseline": (oy + baseline) / SCALE,
        "color": hexc(region[iy, ix]),
        "bg": hexc(bg),
    }


def find_offset(ref_mask, port_im, x0, y0, bg):
    """The (dx, dy) in device px where the port's ink best matches the reference's ink mask
    placed at (x0, y0); None when nothing near looks like it."""
    h, w = ref_mask.shape
    mask, _, (ox, oy) = ink_mask(port_im, x0 - SEARCH, y0 - SEARCH, x0 + w + SEARCH, y0 + h + SEARCH, bg)
    if mask is None or mask.shape[0] < h or mask.shape[1] < w:
        return None
    # Coarse search at half resolution, then refine around the best coarse offset.
    def score(dx, dy):
        sx, sy = x0 + dx - ox, y0 + dy - oy
        if sx < 0 or sy < 0 or sx + w > mask.shape[1] or sy + h > mask.shape[0]:
            return None
        return int(np.count_nonzero(mask[sy : sy + h, sx : sx + w] != ref_mask))

    best = None
    for dy in range(-SEARCH, SEARCH + 1, 3):
        for dx in range(-SEARCH, SEARCH + 1, 3):
            s = score(dx, dy)
            if s is not None and (best is None or (s, abs(dx) + abs(dy)) < (best[0], abs(best[1]) + abs(best[2]))):
                best = (s, dx, dy)
    if best is None:
        return None
    _, bx, by = best
    for dy in range(by - 3, by + 4):
        for dx in range(bx - 3, bx + 4):
            s = score(dx, dy)
            if s is not None and (s, abs(dx) + abs(dy)) < (best[0], abs(best[1]) + abs(best[2])):
                best = (s, dx, dy)
    if best[0] > 0.7 * ref_mask.sum() + 4:
        return None
    return best[1], best[2]


def edge_scan(im, x0, y0, x1, y1, like=None):
    """A colour step along a horizontal or vertical device-px segment: (position in logical px,
    colour before, colour after), or None. Without `like` the strongest step; with `like` (a
    reference edge) the step whose colours are closest to it, nearest its position on ties."""
    x0, y0, x1, y1 = clip(im, x0, y0, x1, y1)
    if x0 >= x1 or y0 >= y1:
        return None
    horizontal = y1 - y0 <= 1
    line = im[y0, x0:x1] if horizontal else im[y0:y1, x0]
    if len(line) < 3:
        return None
    start = x0 if horizontal else y0
    steps = np.abs(np.diff(line, axis=0)).max(axis=1)

    def edge(i):
        return {"at": (start + i + 1) / SCALE, "before": hexc(line[max(0, i - 1)]), "after": hexc(line[min(len(line) - 1, i + 2)])}

    if like is None:
        i = int(steps.argmax())
        return edge(i) if steps[i] > 6 else None
    candidates = [int(i) for i in np.nonzero(steps > 6)[0]]
    if not candidates:
        return None

    def cost(i):
        e = edge(i)
        return (channels(e["before"], like["before"]) + channels(e["after"], like["after"]), abs(e["at"] - like["at"]))

    best = min(candidates, key=cost)
    return edge(best) if cost(best)[0] <= 48 else None


# ---------------------------------------------------------------- element selection


def scrub_words():
    words = {getpass.getuser().lower(), socket.gethostname().split(".")[0].lower()}
    words |= {w.strip().lower() for w in os.environ.get("PARITY_SCRUB", "").split(",") if w.strip()}
    return {w for w in words if len(w) >= 3}


SENSITIVE = re.compile(r"(^|\s)(/|~/)[^\s]+/|[A-Za-z]:\\\\|\.t3/|@[A-Za-z0-9-]+:|WSL", re.I)


def sensitive(text, words):
    low = text.lower()
    return bool(SENSITIVE.search(text)) or any(w in low for w in words)


def parse_alpha(colour):
    if colour.startswith("rgba("):
        return float(colour.rstrip(")").split(",")[-1])
    m = re.search(r"/\s*([\d.]+)\)", colour)
    if m:
        return float(m.group(1))
    return 0.0 if colour in ("transparent", "") else 1.0


def border_width(border):
    widths = [float(v) for v in re.findall(r"([\d.]+)px", border)[:4]]
    return max(widths) if widths else 0.0


OVERLAYS = ("role=dialog", "role=alertdialog", "role=menu", "data-slot=popover-popup", "data-slot=menu-popup", "role=listbox", "data-command-palette")
# Overlays that dim and blur the whole window behind them.
MODAL = ("role=dialog", "role=alertdialog", "data-command-palette")


def overlay_boxes(elements):
    """(x, y, w, h, selector, DOM index, modal) of every popup, dialog and menu."""
    return [
        (e["x"], e["y"], e["w"], e["h"], e["el"], i, any(k in e["el"] for k in MODAL) and "popover-popup" not in e["el"] and e["w"] >= 300 and e["h"] >= 120)
        for i, e in enumerate(elements)
        if any(k in e["el"] for k in OVERLAYS) and e["w"] > 40 and e["h"] > 20
    ]


def hidden(i, e, overlays):
    """Whether an overlay drawn after element `i` covers it: a modal blurs everything behind
    it; a menu or popover hides what is under its box."""
    cx, cy = e["x"] + e["w"] / 2, e["y"] + e["h"] / 2
    for x, y, w, h, _, at, modal in overlays:
        if at > i and (modal or (x <= cx <= x + w and y <= cy <= y + h)):
            return True
    return False


def area_of(e, overlays, sidebar_right):
    cx, cy = e["x"] + e["w"] / 2, e["y"] + e["h"] / 2
    for x, y, w, h, el, _, _ in overlays:
        if x <= cx <= x + w and y <= cy <= y + h:
            kind = next(k for k in OVERLAYS if k in el).split("=")[-1]
            return f"overlay:{kind}"
    if cx < sidebar_right:
        return "sidebar"
    if cy < 52:
        return "titlebar"
    return "main"


def merge_code_runs(texts):
    """Syntax-highlighted code is one span per token; judge each line of tokens as one element."""
    merged, out = [], []
    for e in sorted(texts, key=lambda e: (round(e["y"] + e["h"] / 2), e["x"])):
        mono = e["el"].startswith("span") and any(m in e["font"] for m in MONO)
        last = merged[-1] if merged else None
        if (
            mono
            and last is not None
            and last["font"] == e["font"]
            and abs((last["y"] + last["h"] / 2) - (e["y"] + e["h"] / 2)) <= 1
            and 0 <= e["x"] - (last["x"] + last["w"]) <= 12
        ):
            last["text"] = last["text"] + " " + e["text"]
            last["w"] = e["x"] + e["w"] - last["x"]
            continue
        if mono:
            merged.append(dict(e))
        else:
            out.append(e)
    return out + merged


def select(view, data, words):
    """The judged elements of one view: visible leaf texts (innermost per text and place) and
    boxes with a visible fill or border."""
    vw, vh = data["viewport"]
    every = data["elements"]
    overlays = overlay_boxes(every)
    elements = [
        e
        for i, e in enumerate(every)
        if e["w"] > 0 and e["h"] > 0 and e["x"] < vw and e["y"] < vh and e["x"] + e["w"] > 0 and e["y"] + e["h"] > 0 and not hidden(i, e, overlays)
    ]
    sidebar = next((e for e in elements if e["el"].startswith("div[data-slot=sidebar]")), None)
    sidebar_right = sidebar["x"] + sidebar["w"] if sidebar and sidebar["w"] > 40 else 0
    terminal = view.startswith("terminal-")
    texts, boxes, dropped = {}, [], 0
    for e in elements:
        text = (e.get("text") or "").strip()
        if text and "\n" not in text and len(text) <= 160:
            if e["x"] < 0 or e["y"] < 0 or e["x"] + e["w"] > vw or e["y"] + e["h"] > vh:
                continue
            if sensitive(text, words) or (terminal and (any(m in e["font"] for m in MONO) or "xterm" in e["el"])):
                dropped += 1
                continue
            key = (text, round(e["x"]), round(e["y"] + e["h"] / 2))
            if key not in texts or e["w"] * e["h"] < texts[key]["w"] * texts[key]["h"]:
                texts[key] = e
        if parse_alpha(e.get("bg", "")) >= 0.5 or border_width(e.get("border", "")) >= 1:
            if 8 <= e["w"] < vw and 8 <= e["h"] < vh and e["x"] >= 0 and e["y"] >= 0 and e["x"] + e["w"] <= vw and e["y"] + e["h"] <= vh:
                boxes.append(e)
    texts = merge_code_runs(list(texts.values()))
    picked, seen = [], defaultdict(int)

    def name(kind, e, label):
        base = f"{kind} {e['el'].split('[')[0]} {label}".strip()
        seen[base] += 1
        return base if seen[base] == 1 else f"{base} #{seen[base]}"

    for e in sorted(texts, key=lambda e: (e["y"], e["x"])):
        picked.append({"kind": "text", "name": name("text", e, repr(e["text"].strip()[:48])), "area": area_of(e, overlays, sidebar_right), "font": e["font"], "box": [e["x"], e["y"], e["w"], e["h"]]})
    for e in sorted(boxes, key=lambda e: (e["y"], e["x"])):
        picked.append({"kind": "box", "name": name("box", e, f"{round(e['w'])}x{round(e['h'])}"), "area": area_of(e, overlays, sidebar_right), "box": [e["x"], e["y"], e["w"], e["h"]]})
    return picked, dropped


# ---------------------------------------------------------------- measuring


def text_region(box):
    x, y, w, h = (v * SCALE for v in box)
    return x - 2, y - 1, x + w + 2, y + h + 1


def measure_text_ref(im, el):
    return measure_ink(im, *text_region(el["box"]))


def measure_text_port(im, el, ref):
    """The reference text found in the port image: its ink measured where it best matches."""
    x0, y0, x1, y1 = text_region(el["box"])
    if ref.get("empty"):
        return measure_ink(im, x0, y0, x1, y1)
    bg = np.array(unhex(ref["bg"]), dtype=np.int16)
    rl, rt = int(round(ref["left"] * SCALE)), int(round(ref["top"] * SCALE))
    rw, rh = int(round(ref["width"] * SCALE)), int(round(ref["height"] * SCALE))
    ref_im = REF_IMAGE.get("im")
    ref_mask = None
    if ref_im is not None:
        ref_mask, _, _ = ink_mask(ref_im, rl, rt, rl + rw, rt + rh, bg)
    offset = find_offset(ref_mask, im, rl, rt, bg) if ref_mask is not None and ref_mask.any() else None
    if offset is None:
        found = measure_ink(im, x0, y0, x1, y1, bg)
        found["matched"] = False
        return found
    dx, dy = offset
    found = measure_ink(im, x0 + dx, y0 + dy, x1 + dx, y1 + dy, bg)
    # Ink that reaches the region's edge runs past it (a wider text): measure the extent with
    # room, keeping the colour from the reference-sized region.
    if not found.get("empty") and (found["left"] * SCALE <= x0 + dx + 1 or (found["left"] + found["width"]) * SCALE >= x1 + dx - 1):
        pad = max(8, int(0.12 * (x1 - x0)))
        wide = measure_ink(im, x0 + dx - 6, y0 + dy, x1 + dx + pad, y1 + dy, bg)
        if not wide.get("empty"):
            found = {**wide, "color": found["color"]}
    found["matched"] = True
    return found


def measure_box(im, el, ref=None):
    x, y, w, h = el["box"]
    X0, Y0, X1, Y1 = x * SCALE, y * SCALE, (x + w) * SCALE, (y + h) * SCALE
    span = 8 if ref is None else SEARCH
    my, mx = int((Y0 + Y1) / 2), int((X0 + X1) / 2)
    result = {}
    narrow = {
        "left": (X0 - 8, my, X0 + 8, my + 1),
        "right": (X1 - 8, my, X1 + 8, my + 1),
        "top": (mx, Y0 - 8, mx + 1, Y0 + 8),
        "bottom": (mx, Y1 - 8, mx + 1, Y1 + 8),
    }
    for side, seg in (
        ("left", (X0 - span, my, X0 + span, my + 1)),
        ("right", (X1 - span, my, X1 + span, my + 1)),
        ("top", (mx, Y0 - span, mx + 1, Y0 + span)),
        ("bottom", (mx, Y1 - span, mx + 1, Y1 + span)),
    ):
        if ref is not None and side not in ref:
            continue
        found = edge_scan(im, *seg, like=ref[side] if ref is not None else None)
        if ref is not None:
            # Prefer the edge where the reference recorded it (same window as `record`).
            near = edge_scan(im, *narrow[side])
            if near and channels(near["before"], ref[side]["before"]) + channels(near["after"], ref[side]["after"]) <= 6:
                found = near
        if found:
            result[side] = found
    inner = im[int(Y0) + 4 : int(Y1) - 4, int(X0) + 4 : int(X1) - 4]
    if inner.size:
        result["fill"] = hexc(mode_colour(inner))
    return result


REF_IMAGE = {}

# ---------------------------------------------------------------- record


def record(ref_dir, *only):
    ref_dir = Path(ref_dir)
    EXPECTED.mkdir(parents=True, exist_ok=True)
    words = scrub_words()
    for path in sorted(ref_dir.glob("*.json")):
        view = path.stem
        if view.startswith("_") or (only and view not in only) or not (ref_dir / f"{view}.png").exists():
            continue
        data = json.loads(path.read_text())
        im = load(ref_dir / f"{view}.png")
        picked, dropped = select(view, data, words)
        out = []
        for el in picked:
            ref = measure_text_ref(im, el) if el["kind"] == "text" else measure_box(im, el)
            if el["kind"] == "text" and ref.get("empty"):
                continue
            if el["kind"] == "box" and not any(k in ref for k in ("left", "right", "top", "bottom")):
                continue
            out.append({**el, "ref": ref})
        doc = {"view": view, "viewport": data["viewport"], "scale": data["scale"], "dropped": dropped, "elements": out}
        (EXPECTED / f"{view}.json").write_text(json.dumps(doc, separators=(",", ":"), ensure_ascii=False) + "\n")
        print(f"wrote expected/mac/{view}.json ({len(out)} elements, {dropped} dropped)")


# ---------------------------------------------------------------- compare


def judge_text(ref, port):
    rows = []
    if port.get("empty"):
        cause = "not found near its place" if port.get("matched") is False else "missing in port"
        return [("ink", "present", "none", "", False)], cause
    for prop in ("left", "top", "width", "height", "baseline"):
        d = round(port[prop] - ref[prop], 2)
        rows.append((prop, ref[prop], port[prop], d, abs(d) <= PX))
    d = channels(ref["color"], port["color"])
    rows.append(("ink colour", ref["color"], port["color"], d, d <= CHANNEL))
    bad = {r[0] for r in rows if r[4] is False}
    if not port.get("matched"):
        cause = "not found near its place"
    elif "width" in bad:
        cause = "text width"
    elif bad & {"height", "baseline"} and not bad & {"left", "top"}:
        cause = "glyph height or baseline"
    elif bad & {"left", "top"}:
        cause = "text position"
    elif "ink colour" in bad:
        cause = "text colour"
    else:
        cause = None
    return rows, cause


def judge_box(ref, port):
    rows = []
    for side in ("left", "right", "top", "bottom"):
        if side not in ref:
            continue
        if side not in port:
            rows.append((f"{side} edge", ref[side]["at"], "none", "", False))
            continue
        d = round(port[side]["at"] - ref[side]["at"], 2)
        rows.append((f"{side} edge", ref[side]["at"], port[side]["at"], d, abs(d) <= PX))
    if "fill" in ref and "fill" in port:
        d = channels(ref["fill"], port["fill"])
        rows.append(("fill", ref["fill"], port["fill"], d, d <= CHANNEL))
    bad = [r[0] for r in rows if r[4] is False]
    cause = None
    if any("edge" in b for b in bad):
        cause = "box edge"
    elif bad:
        cause = "fill colour"
    return rows, cause


def font_key(font):
    m = re.match(r"(\d+) ([\d.]+)px", font or "")
    family = "mono" if any(f in (font or "") for f in MONO) else "sans"
    return f"{m.group(1)} {m.group(2)}px {family}" if m else family


def fmt(v):
    return f"{v:g}" if isinstance(v, float) else str(v)


def side_by_side(ref_png, port_png, marks, out):
    a = Image.open(ref_png).convert("RGB") if ref_png and ref_png.exists() else None
    b = Image.open(port_png).convert("RGB")
    images = [im for im in (a, b) if im is not None]
    for im in images:
        draw = ImageDraw.Draw(im)
        for box, ok in marks:
            x, y, w, h = (v * SCALE for v in box)
            draw.rectangle([x, y, x + w, y + h], outline=(0, 200, 120) if ok else (255, 40, 40), width=2)
    width = sum(im.width for im in images) + 20 * (len(images) - 1)
    canvas = Image.new("RGB", (width, max(im.height for im in images) + 44), (255, 0, 255))
    x = 0
    draw = ImageDraw.Draw(canvas)
    draw.rectangle([0, 0, canvas.width, 42], fill=(0, 0, 0))
    for label, im in zip(["REFERENCE (fork on the Mac)", "PORT (gpui on the Mac)"] if a else ["PORT"], images):
        canvas.paste(im, (x, 44))
        draw.text((x + 10, 14), label + "   green pass, red fail", fill=(255, 255, 255))
        x += im.width + 20
    canvas.save(out)


def compare(port_dir, out_dir, ref_dir=None):
    port_dir, out_dir = Path(port_dir), Path(out_dir)
    (out_dir / "side").mkdir(parents=True, exist_ok=True)
    summary, groups, results = [], defaultdict(list), {}
    for path in sorted(EXPECTED.glob("*.json")):
        doc = json.loads(path.read_text())
        view = doc["view"]
        port_png = port_dir / f"{view}.png"
        if not port_png.exists():
            summary.append((view, 0, 0, len(doc["elements"]), "no port image"))
            continue
        im = load(port_png)
        ref_png = Path(ref_dir) / f"{view}.png" if ref_dir else None
        REF_IMAGE["im"] = load(ref_png) if ref_png and ref_png.exists() else None
        passed = failed = 0
        marks, rows_out = [], []
        for el in doc["elements"]:
            ref = el["ref"]
            if el["kind"] == "text":
                if REF_IMAGE["im"] is None:
                    port = measure_ink(im, *text_region(el["box"]))
                    port["matched"] = True
                else:
                    port = measure_text_port(im, el, ref)
                rows, cause = judge_text(ref, port)
            else:
                port = measure_box(im, el, ref)
                rows, cause = judge_box(ref, port)
            ok = cause is None
            passed += ok
            failed += not ok
            marks.append((el["box"], ok))
            rows_out.append({"element": el["name"], "area": el["area"], "state": "pass" if ok else "FAIL", "cause": cause, "rows": [[r[0], r[1], r[2], r[3], r[4]] for r in rows]})
            if not ok:
                bad = "; ".join(f"{p} {fmt(r)} -> {fmt(q)}" for p, r, q, _, good in rows if good is False)
                key = (cause, el["area"].split(":")[0] if el["area"].startswith("overlay") else el["area"], font_key(el.get("font")) if el["kind"] == "text" else "")
                groups[key].append(f"- {view} / {el['name']}: {bad}")
        results[view] = rows_out
        summary.append((view, passed, failed, 0, ""))
        side_by_side(ref_png, port_png, marks, out_dir / "side" / f"{view}.png")
    for png in sorted(port_dir.glob("*.scroll-*.png")):
        ref_png = Path(ref_dir) / png.name if ref_dir else None
        side_by_side(ref_png, png, [], out_dir / "side" / png.name)
    total_pass = sum(s[1] for s in summary)
    total_fail = sum(s[2] for s in summary)
    lines = [
        "# Mac parity report",
        "",
        f"Mode `mac`: port renders against the fork rendered on the owner's Mac. Tolerance {PX}px per position and size, {CHANNEL}/255 per channel.",
        "Text: glyph ink box (left, top, width, height), baseline and ink colour, found within 24px of the reference position.",
        "Boxes: edges at mid-side and the fill.",
        "",
        f"**{total_pass} elements pass, {total_fail} fail, in {sum(1 for s in summary if not s[4])} scenes.**",
        "",
        "| scene | pass | fail | note |",
        "| --- | --- | --- | --- |",
    ]
    lines += [f"| {v} | {p} | {f} | {note} |" for v, p, f, _, note in summary]
    lines += ["", "## Failures by cause", "", "Grouped by cause, area and text style. Each line: scene / element: property reference -> port (logical px, colours as hex).", ""]
    by_cause = defaultdict(int)
    for (cause, area, font), items in groups.items():
        by_cause[cause] += len(items)
    lines += ["| cause | elements |", "| --- | --- |"] + [f"| {c} | {n} |" for c, n in sorted(by_cause.items(), key=lambda kv: -kv[1])] + [""]
    for (cause, area, font), items in sorted(groups.items(), key=lambda kv: (kv[0][0], kv[0][1], -len(kv[1]))):
        lines += [f"### {cause} · {area}{' · ' + font if font else ''} ({len(items)})", ""] + items + [""]
    (out_dir / "report.md").write_text("\n".join(lines) + "\n")
    (out_dir / "results.json").write_text(json.dumps(results, indent=1, default=float) + "\n")
    print("\n".join(lines[:12 + len(summary)]))
    print(f"report: {out_dir / 'report.md'}")
    return total_fail


if __name__ == "__main__":
    command, *args = sys.argv[1:] or ["help"]
    if command == "record":
        record(*args)
    elif command == "compare":
        sys.exit(1 if compare(*args) else 0)
    else:
        print(__doc__)
