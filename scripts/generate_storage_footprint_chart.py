#!/usr/bin/env python3
"""Generate Rosé Pine Dawn storage footprint and tombstone SVG with monospace font"""

import json
import os
import sys
import xml.etree.ElementTree as ET

# Rosé Pine Dawn palette
COLOR_BASE = "#faf4ed"
COLOR_SURFACE = "#fffaf3"
COLOR_OVERLAY = "#f2e9e1"
COLOR_MUTED = "#9893a5"
COLOR_SUBTLE = "#797593"
COLOR_TEXT = "#575279"
COLOR_BORDER = "#dfdad9"
COLOR_LINE = "#cecacd"

# Accent colors
ACCENT_PINE = "#286983"
ACCENT_IRIS = "#907aa9"
ACCENT_LOVE = "#b4637a"
ACCENT_GOLD = "#ea9d34"
ACCENT_FOAM = "#56949f"
ACCENT_ROSE = "#d7827e"

WORKLOADS = [
    ("del_perf", "churn_insert_delete", f"CHURN: {0x32}/{0x32} Tombstones", ACCENT_LOVE),
    ("write_ingest", "sequential_append", "WAL: Ingest (Sequential Append)", ACCENT_PINE),
    ("read_perf", "point_hit", "READ: Point Hit (Existing)", ACCENT_FOAM),
    ("del_perf", "delete_hit", "DELETE: Point Hit (Existing)", ACCENT_GOLD),
    ("read_perf", "mixed_95_5", f"READ/WRITE: Mixed {0x5F}/5 OLTP", ACCENT_ROSE),
    ("del_perf", "high_contention", "DELETE: Hot-Key Contention", ACCENT_IRIS),
]

FONT_STACK = (
    "ui-monospace,'SF Mono',Menlo,Monaco,Consolas,'Liberation Mono',monospace"
)


def load_results(path):
    with open(path, "r", encoding="utf-8") as handle:
        data = json.load(handle)
    lookup = {}
    for entry in data.get("results", []):
        bench = entry["benchmark"]
        workload = entry["workload"]
        threads = entry["threads"]
        lookup[(bench, workload, threads)] = entry
        if bench == "write_perf" and workload == "pure_append":
            lookup[("write_ingest", "sequential_append", threads)] = entry
    return lookup


def generate_svg(lookup):
    svg_w = 0x04C4
    svg_h = 0x0218
    title_x = 0x24
    plot_x = 0x0158
    plot_y = 0x54
    plot_w = 0x0300
    plot_h = 0x0140

    max_mb = float(0x1E)
    row_h = float(plot_h) / float(len(WORKLOADS))
    bar_h = 0x10

    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/{0x07D0}/svg" '
        f'viewBox="0 0 {svg_w} {svg_h}" width="{svg_w}" height="{svg_h}" '
        f'style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )
    svg.append(
        f'<rect width="{svg_w}" height="{svg_h}" rx="{0x0C}" '
        f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )
    svg.append(
        f'<text x="{title_x}" y="{0x22}" fill="{COLOR_TEXT}" '
        f'font-size="{0x10}" font-weight="bold">'
        f'TurboFox On-Disk Storage Footprint &amp; Allocation Density'
        f'</text>'
    )
    svg.append(
        f'<text x="{title_x}" y="{0x38}" fill="{COLOR_SUBTLE}" font-size="{0x0A}">'
        f'Final database storage footprint after 0x10000 operations per workload '
        f'(Rosé Pine Dawn theme)'
        f'</text>'
    )

    # Gridlines
    ticks = [0, 5, 0x0A, 0x0F, 0x14, 0x19, 0x1E]
    for tick in ticks:
        x = plot_x + (float(tick) / max_mb) * plot_w
        svg.append(
            f'<line x1="{x:.1f}" y1="{plot_y}" x2="{x:.1f}" '
            f'y2="{plot_y + plot_h}" stroke="{COLOR_OVERLAY}" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{x:.1f}" y="{plot_y + plot_h + 0x12}" fill="{COLOR_MUTED}" '
            f'font-size="{0x08}" text-anchor="middle">{tick} MB</text>'
        )

    # Axis boundary lines
    svg.append(
        f'<line x1="{plot_x}" y1="{plot_y}" x2="{plot_x}" '
        f'y2="{plot_y + plot_h}" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<line x1="{plot_x}" y1="{plot_y + plot_h}" x2="{plot_x + plot_w}" '
        f'y2="{plot_y + plot_h}" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    # Bars
    for idx, (bench, workload, label, color) in enumerate(WORKLOADS):
        entry = lookup.get((bench, workload, 1))
        bytes_val = entry["db_size_bytes"] if entry else 0
        mb_val = float(bytes_val) / float(0x00100000)
        bw = max((mb_val / max_mb) * plot_w, 2.0)
        by = plot_y + float(idx) * row_h + (row_h - float(bar_h)) / 2.0

        svg.append(
            f'<text x="{plot_x - 0x0A}" y="{by + 0x0B:.1f}" fill="{COLOR_TEXT}" '
            f'font-size="{0x08}" font-weight="bold" text-anchor="end">{label}</text>'
        )
        svg.append(
            f'<rect x="{plot_x}" y="{by:.1f}" width="{bw:.1f}" '
            f'height="{bar_h}" rx="2" fill="{color}"/>'
        )
        svg.append(
            f'<text x="{plot_x + bw + 6:.1f}" y="{by + 0x0B:.1f}" fill="{color}" '
            f'font-size="{0x08}" font-weight="bold">'
            f'{mb_val:.2f} MB ({bytes_val:,} B)</text>'
        )

    # Footnote
    note_x = title_x
    note_y = plot_y + plot_h + 0x2C
    note_lines = [
        "* Note: Final storage size includes index slot arrays, DataBuf pages, and header metadata",
        f"  Steady-state churn ({0x32}/{0x32}) demonstrates bounded storage footprint via "
        "tombstone recycling",
    ]
    for idx, line in enumerate(note_lines):
        ly = note_y + idx * 0x10
        svg.append(
            f'<text x="{note_x}" y="{ly}" fill="{COLOR_SUBTLE}" '
            f'font-size="{0x08}">{line}</text>'
        )

    svg.append("</svg>")
    return "\n".join(svg)


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    json_path = os.path.join(root, "tmp", "turbofox_benchmark_results.json")
    out_path = os.path.join(root, "assets", "storage_footprint.svg")
    lookup = load_results(json_path)
    svg = generate_svg(lookup)

    # Validate XML before writing
    ET.fromstring(svg)

    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write(svg)
        handle.write("\n")
    print(f"Generated {out_path}")


if __name__ == "__main__":
    main()
