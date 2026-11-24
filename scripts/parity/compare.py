"""Visual parity between the GPUI port and the fork's web app.

  record  <ref-dir> [scene-filter ...]    measure the reference PNGs once and write each
                                          scene's values to expected/<scene>.json under the
                                          mode (committed; the PNGs are not); filters
                                          re-record only the scenes whose name contains
                                          one, skipping those without a PNG here
  compare <port-dir> <out-dir> [ref-dir]  measure the port PNGs, write report.md and
                                          results.json, and side-by-side images when
                                          the reference PNGs are available

`PARITY_MODE=owner` records and judges the capture at the owner's settings (contrast 95,
glass 40) instead of the shared one (`default`).

The spec (gen_spec.py, one module per area under specs/) lists, per scene, the elements to measure (see measure.py) with a name,
optionally `blur: true` on a scan across backdrop-blurred glass (edge
positions judged, colours shown only), `anchor: "<element>"` (measure relative to where that element landed, also
horizontally with `anchor_x: true`,
for content below a documented difference that moves it),
and optionally `text: true` (glyph shapes differ by rasteriser, so a text run's
right edge is reported but not judged; `text: "right"` for right-aligned text
judges the right edge and reports the left) or `exclude: "<reason>"` (a fixture or a
documented GPUI limit; measured and shown, never judged).
"""
import json
import os
import sys
from pathlib import Path

from PIL import Image, ImageDraw

import gen_spec
import measure as m

HERE = Path(__file__).resolve().parent
SPEC = gen_spec.build()
EXPECTED_DIR = HERE / "expected"
MODE = os.environ.get("PARITY_MODE", "default")
PX = SPEC["tolerance"]["px"]
# GPUI places a line's baseline with the font's exact ascent and descent
# (gpui text_system/line.rs: padding_top = (line_height - ascent - descent) / 2);
# Chromium rounds both to whole pixels first. A text run therefore sits up to
# ~0.45px off, which device-pixel snapping can show as 1px. Text runs get this
# vertical tolerance; everything else keeps PX.
TEXT_PX = SPEC["tolerance"]["text_px"]
CHANNEL = SPEC["tolerance"]["channel"]


def channels(a, b):
    a, b = a.lstrip("#"), b.lstrip("#")
    return max(abs(int(a[i : i + 2], 16) - int(b[i : i + 2], 16)) for i in (0, 2, 4))


def load_expected():
    """Every scene's recorded values in MODE."""
    expected = {}
    for path in sorted(EXPECTED_DIR.glob("*.json")):
        modes = json.loads(path.read_text())
        if MODE in modes:
            expected[path.stem] = modes[MODE]
    return expected


def record(ref_dir, *only):
    """Measures the reference PNGs into each scene's file under MODE, keeping its other mode;
    with filters, re-records just the scenes whose name contains one (references live in
    several directories, captured at different times)."""
    ref_dir = Path(ref_dir)
    EXPECTED_DIR.mkdir(exist_ok=True)
    for scene, body in SPEC["scenes"].items():
        if only and not any(f in scene for f in only):
            continue
        png = ref_dir / f"{body.get('reference', scene)}.png"
        if only and not png.exists():
            print(f"skipped {scene}: no {png.name} in {ref_dir}")
            continue
        im = m.load(png)
        path = EXPECTED_DIR / f"{scene}.json"
        modes = json.loads(path.read_text()) if path.exists() else {}
        modes[MODE] = {e["name"]: m.measure(im, e) for e in body["elements"]}
        path.write_text(json.dumps(dict(sorted(modes.items())), indent=1) + "\n")
        print(f"wrote {path.relative_to(HERE)} ({MODE})")


def judge(element, ref, port):
    """Rows of (property, ref, port, delta, ok) for one element; ok None is informational."""
    rows = []
    kind = element["kind"]
    if kind == "fill":
        d = channels(ref["color"], port["color"])
        rows.append(("colour", ref["color"], port["color"], d, d <= CHANNEL))
    elif kind == "ink":
        if ref.get("empty") or port.get("empty"):
            ok = bool(ref.get("empty")) == bool(port.get("empty"))
            rows.append(("ink", "none" if ref.get("empty") else "present", "none" if port.get("empty") else "present", "", ok))
            return rows
        for side in ("top", "left", "bottom", "right"):
            d = port[side] - ref[side]
            # A text run's far edge depends on glyph advances; judge the edge it is anchored to.
            text = element.get("text")
            free = {True: "right", "right": "left"}.get(text)
            judged = side != free
            limit = TEXT_PX if text and side in ("top", "bottom") else PX
            rows.append((side, ref[side], port[side], d, abs(d) <= limit if judged else None))
        rh, ph = ref["bottom"] - ref["top"], port["bottom"] - port["top"]
        rows.append(("height", rh, ph, ph - rh, None))
        d = channels(ref["color"], port["color"])
        rows.append(("ink colour", ref["color"], port["color"], d, d <= CHANNEL))
        d = channels(ref["bg"], port["bg"])
        rows.append(("background", ref["bg"], port["bg"], d, d <= CHANNEL))
    else:
        # Over a backdrop-blurred surface the fork's colours carry the blurred
        # content behind the glass, which GPUI cannot sample: positions only.
        blur = element.get("blur", False)
        colour_ok = lambda d: None if blur else d <= CHANNEL
        d = channels(ref["start"], port["start"])
        rows.append(("start colour", ref["start"], port["start"], d, colour_ok(d)))
        re, pe = ref["edges"], port["edges"]
        if len(re) != len(pe):
            rows.append(("edges", len(re), len(pe), len(pe) - len(re), False))
            rows.append(("edge list", " ".join(f"{e['at']:g}{e['color']}" for e in re), " ".join(f"{e['at']:g}{e['color']}" for e in pe), "", None))
        else:
            for i, (a, b) in enumerate(zip(re, pe)):
                d = b["at"] - a["at"]
                rows.append((f"edge {i + 1} at", a["at"], b["at"], d, abs(d) <= PX))
                d = channels(a["color"], b["color"])
                rows.append((f"edge {i + 1} colour", a["color"], b["color"], d, colour_ok(d)))
    return rows


def fmt(v):
    return f"{v:g}" if isinstance(v, float) else str(v)


def draw_mark(draw, element, colour):
    at = [v * m.SCALE for v in element["at"]]
    kind = element["kind"]
    if kind == "fill":
        x, y = at
        draw.ellipse([x - 6, y - 6, x + 6, y + 6], outline=colour, width=3)
    elif kind == "ink":
        draw.rectangle(at, outline=colour, width=2)
    elif kind == "hscan":
        y, x0, x1 = at
        draw.line([x0, y, x1, y], fill=colour, width=2)
    else:
        x, y0, y1 = at
        draw.line([x, y0, x, y1], fill=colour, width=2)


def side_by_side(ref_png, port_png, marks, out):
    if not ref_png.exists():
        return
    a, b = Image.open(ref_png).convert("RGB"), Image.open(port_png).convert("RGB")
    for im in (a, b):
        draw = ImageDraw.Draw(im)
        for element, state in marks:
            colour = {"pass": (0, 200, 120), "FAIL": (255, 40, 40), "excluded": (120, 120, 255)}[state]
            draw_mark(draw, element, colour)
    canvas = Image.new("RGB", (a.width + b.width + 20, max(a.height, b.height) + 44), (255, 0, 255))
    canvas.paste(a, (0, 44))
    canvas.paste(b, (a.width + 20, 44))
    draw = ImageDraw.Draw(canvas)
    draw.rectangle([0, 0, canvas.width, 42], fill=(0, 0, 0))
    draw.text((10, 14), "REFERENCE (web fork)", fill=(255, 255, 255))
    draw.text((a.width + 30, 14), "PORT (gpui)   green pass, red fail, blue excluded", fill=(255, 255, 255))
    canvas.save(out)


def compare(port_dir, out_dir, ref_dir=None):
    port_dir, out_dir = Path(port_dir), Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    expected = load_expected()
    body_lines = []
    summary = []
    results = {}
    failing = []
    for scene, body in SPEC["scenes"].items():
        port_png = port_dir / f"{scene}.png"
        if not port_png.exists():
            summary.append((scene, 0, 0, 0, "(no port image)"))
            continue
        if scene not in expected:
            summary.append((scene, 0, 0, 0, f"(not recorded for {MODE})"))
            continue
        im = m.load(port_png)
        by_name = {e["name"]: e for e in body["elements"]}
        counts = {"pass": 0, "FAIL": 0, "excluded": 0}
        rows_md = []
        marks = []
        for element in body["elements"]:
            ref = expected[scene][element["name"]]
            dy = dx = 0.0
            if "anchor" in element:
                # Content below a known, documented difference: measure it where
                # the anchor actually landed, so spacing compares like for like.
                anchor = by_name[element["anchor"]]
                found = m.measure(im, anchor)
                if not found.get("empty"):
                    dy = found["top"] - expected[scene][anchor["name"]]["top"]
                    if element.get("anchor_x"):
                        dx = found["left"] - expected[scene][anchor["name"]]["left"]
            # Scene captures lay out without the sidebar or with the Linux topbar; `port_dx`/`port_dy` say how far.
            dx += element.get("port_dx", 0)
            dy += element.get("port_dy", 0)
            port = m.unmoved(m.measure(im, m.moved(element, dx, dy)), dx, dy)
            rows = judge(element, ref, port)
            verdicts = [ok for *_, ok in rows if ok is not None]
            state = "excluded" if element.get("exclude") else ("pass" if all(verdicts) else "FAIL")
            counts[state] += 1
            if state == "FAIL":
                bad = "; ".join(f"{prop} {fmt(r)} -> {fmt(p)}" for prop, r, p, _, ok in rows if ok is False)
                failing.append(f"- {scene} / {element['name']}: {bad}")
            marks.append((element, state))
            results.setdefault(scene, []).append({"element": element["name"], "state": state, "ref": ref, "port": port})
            for i, (prop, r, p, d, ok) in enumerate(rows):
                verdict = "excluded" if state == "excluded" else ("" if ok is None else ("ok" if ok else "**fail**"))
                note = element.get("exclude", element.get("note", "")) if i == 0 else ""
                if i == 0 and "anchor" in element:
                    note = (note + " " if note else "") + f"(relative to {element['anchor']}, port offset {dy:+g}px)"
                name = element["name"] if i == 0 else ""
                rows_md.append(f"| {name} | {prop} | {fmt(r)} | {fmt(p)} | {fmt(d)} | {verdict} | {note} |")
        summary.append((scene, counts["pass"], counts["FAIL"], counts["excluded"], ""))
        body_lines += [f"## {scene}", "", "| element | property | reference | port | delta | result | note |", "| --- | --- | --- | --- | --- | --- | --- |"]
        body_lines += rows_md + [""]
        if ref_dir:
            side_by_side(Path(ref_dir) / f"{body.get('reference', scene)}.png", port_png, marks, out_dir / f"{scene}.png")
    total = sum(f for _, _, f, _, _ in summary)
    head = [
        "# Visual parity report",
        "",
        f"Tolerance: {PX}px per edge, {CHANNEL}/255 per channel. Text runs judge the edge they are anchored to plus top and bottom at {TEXT_PX}px "
        "(GPUI does not round font ascent and descent the way Chromium does); their far edge is shown only.",
        "",
        "| scene | pass | fail | excluded |",
        "| --- | --- | --- | --- |",
    ]
    head += [f"| {s} {note} | {p} | {f} | {e} |" for s, p, f, e, note in summary]
    head += ["", f"**{total} failing elements.**", ""]
    if failing:
        head += ["### Failures", ""] + failing + [""]
    (out_dir / "report.md").write_text("\n".join(head + body_lines) + "\n")
    (out_dir / "results.json").write_text(json.dumps(results, indent=1) + "\n")
    print("\n".join(head))
    print(f"report: {out_dir / 'report.md'}")
    return total


if __name__ == "__main__":
    command, *args = sys.argv[1:] or ["help"]
    if command == "record":
        record(*args)
    elif command == "compare":
        sys.exit(1 if compare(*args) else 0)
    else:
        print(__doc__)
