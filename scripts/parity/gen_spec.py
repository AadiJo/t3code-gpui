"""Builds the parity spec: the elements compare.py measures in each scene. Each area's scenes
live in their own module under specs/ (builders in specs/kit.py), imported in AREAS order, so
work on different areas touches different files. compare.py builds the spec itself;
`python3 gen_spec.py <out.json>` writes it out for inspection.
"""
import importlib
import json
import sys

AREAS = [
    "sidebar",
    "chat",
    "composer",
    "palette",
    "settings",
    "settings_server",
    "terminal",
    "right_panel",
    "pages",
    "thread_details",
    "notifications",
]
TOLERANCE = {"px": 0.5, "text_px": 1.0, "channel": 1}


def build():
    """The spec: every area's scenes in AREAS order, each scene's elements in its own order."""
    from specs import kit

    for area in AREAS:
        importlib.import_module(f"specs.{area}")
    return {"tolerance": TOLERANCE, "scenes": kit.scenes}


if __name__ == "__main__":
    spec = build()
    json.dump(spec, open(sys.argv[1], "w"), indent=1)
    print(sum(len(s["elements"]) for s in spec["scenes"].values()), "elements")
