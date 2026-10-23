"""Measure captures against the reference renders; requires Pillow."""
import hashlib
import json
from pathlib import Path

from PIL import Image, ImageChops

root = Path(__file__).resolve().parents[2] / "docs/web-ui-mockup/screenshots"
report = {
    "note": "Content is derived from native Hub state. Differences include honest live metadata and filters; see README.md.",
    "reference_comparisons": [],
    "captures": [],
}
for name, file in [
    ("Main", "board-1440x900.png"),
    ("Plan", "plans-1440x900.png"),
    ("Phone", "board-390x844.png"),
]:
    app = Image.open(root / file).convert("RGB")
    reference = Image.open(f"/tmp/forkdeck-reference-{name}.png").convert("RGB")
    if app.size != reference.size:
        raise ValueError(f"Capture size differs from reference: {file}")
    difference = ImageChops.difference(app, reference)
    count = sum(any(pixel) for pixel in difference.getdata())
    total = app.width * app.height
    report["reference_comparisons"].append({
        "capture": file,
        "reference": f"{name}.dc.html",
        "different_pixels": count,
        "total_pixels": total,
        "different_percent": round(100 * count / total, 3),
    })
for file in sorted(root.glob("*.png")):
    with Image.open(file) as capture:
        report["captures"].append({
            "path": file.name,
            "width": capture.width,
            "height": capture.height,
            "sha256": hashlib.sha256(file.read_bytes()).hexdigest(),
        })
(root / "comparison.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report["reference_comparisons"], indent=2))
