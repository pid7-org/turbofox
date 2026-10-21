#!/usr/bin/env python3
"""Generate Catppuccin Latte latency percentile profile SVG with monospace font"""

import json
import math
import os
import sys
import xml.etree.ElementTree as ET

COLOR_BASE = "#eff1f5"
COLOR_SURFACE = "#e6e9ef"
COLOR_OVERLAY = "#ccd0da"
COLOR_MUTED = "#8c8fa1"
COLOR_SUBTLE = "#6c6f85"
COLOR_TEXT = "#4c4f69"
COLOR_BORDER = "#ccd0da"
COLOR_LINE = "#bcc0cc"

ACCENT_BLUE = "#1e66f5"
ACCENT_TEAL = "#179299"
ACCENT_MAUVE = "#8839ef"
ACCENT_RED = "#d20f39"
ACCENT_PEACH = "#fe640b"
ACCENT_PINK = "#ea76cb"
ACCENT_SAPPHIRE = "#209fb5"

SERIES_SPECS = [
    ("del_perf", "high_contention", "DELETE: Hot-Key Contention", ACCENT_MAUVE),
    ("read_perf", "point_miss", "READ: Point Miss (Absent)", ACCENT_TEAL),
    ("del_perf", "delete_miss", "DELETE: Point Miss (Absent)", ACCENT_PINK),
    ("read_perf", "point_hit", "READ: Point Hit (Existing)", ACCENT_BLUE),
    ("read_perf", "zipf_skewed", f"READ: Zipfian ({0x50}/{0x14} Skew)", ACCENT_PEACH),
    ("del_perf", "delete_hit", "DELETE: Point Hit (Existing)", ACCENT_RED),
]

PERCENTILES = [
    ("p50", "p50"),
    ("p75", "p75"),
    ("p90", "p90"),
    ("p95", "p95"),
    ("p99", "p99"),
    ("p999", "p99.9"),
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
        if bench == "read_perf" and workload == "point_hit":
            lookup[("read_perf", "point_hit", threads)] = entry
        elif bench == "del_perf" and workload == "churn_insert_delete":
            lookup[("del_perf", "churn", threads)] = entry
        elif bench == "del_perf" and workload == "churn":
            lookup[("del_perf", "churn_insert_delete", threads)] = entry
        elif bench == "sync_perf" and workload == "pages_0x20":
            lookup[("sync_perf", "0x20", threads)] = entry
        elif bench == "sync_perf" and workload == "0x20":
            lookup[("sync_perf", "pages_0x20", threads)] = entry
        elif bench == "sync_perf" and workload == "pages_0x0100":
            lookup[("sync_perf", "0x0100", threads)] = entry
        elif bench == "sync_perf" and workload == "0x0100":
            lookup[("sync_perf", "pages_0x0100", threads)] = entry
        elif bench == "sync_perf" and workload == "pages_0x0400":
            lookup[("sync_perf", "0x0400", threads)] = entry
        elif bench == "sync_perf" and workload == "0x0400":
            lookup[("sync_perf", "pages_0x0400", threads)] = entry
        elif bench == "churn_perf" and workload == "steady_state_churn":
            lookup[("churn_perf", "steady_state", threads)] = entry
        elif bench == "churn_perf" and workload == "steady_state":
            lookup[("churn_perf", "steady_state_churn", threads)] = entry
        elif bench == "write_perf" and workload == "pure_append":
            lookup[("write_ingest", "sequential_append", threads)] = entry
    return lookup


def generate_svg(lookup):
    svg_w = 0x04C4
    svg_h = 0x0218
    plot_x = 0x54
    plot_y = 0x54
    plot_w = 0x02E0
    plot_h = 0x0150

    max_y = float(0x0A)

    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/{0x07D0}/svg" '
        f'viewBox="0 0 {svg_w} {svg_h}" '
        f'width="{svg_w}" height="{svg_h}" '
        f'style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )

    svg.append(
        f'<rect width="{svg_w}" height="{svg_h}" rx="{0x0C}" '
        f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )

    title_x = 0x24
    svg.append(
        f'<text x="{title_x}" y="{0x22}" fill="{COLOR_TEXT}" '
        f'font-size="{0x10}" font-weight="bold">'
        f'TurboFox In-Memory Latency Percentile Profile'
        f'</text>'
    )
    svg.append(
        f'<text x="{title_x}" y="{0x38}" fill="{COLOR_SUBTLE}" '
        f'font-size="{0x0A}">'
        f'Platform: AWS c5d.large (1 Thread baseline) | '
        f'Linear latency scale (0 µs to {0x0A} µs) (Catppuccin Latte theme)'
        f'</text>'
    )

    y_1us = plot_y + plot_h - ((1.0 / max_y) * float(plot_h))
    svg.append(
        f'<rect x="{plot_x}" y="{y_1us:.1f}" width="{plot_w}" '
        f'height="{plot_y + plot_h - y_1us:.1f}" fill="{ACCENT_TEAL}" '
        f'fill-opacity="0.08"/>'
    )

    y_ticks = [0, 2, 4, 6, 8, 0x0A]
    for val in y_ticks:
        norm_y = float(val) / max_y
        ty = plot_y + plot_h - (norm_y * float(plot_h))
        svg.append(
            f'<line x1="{plot_x}" y1="{ty:.1f}" x2="{plot_x + plot_w}" '
            f'y2="{ty:.1f}" stroke="{COLOR_OVERLAY}" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{plot_x - 8}" y="{ty + 3:.1f}" fill="{COLOR_MUTED}" '
            f'font-size="{0x08}" text-anchor="end">{val} µs</text>'
        )

    num_pct = len(PERCENTILES)
    x_step = float(plot_w) / float(num_pct - 1)
    pct_x_coords = [plot_x + i * x_step for i in range(num_pct)]

    for x_c, (_, label) in zip(pct_x_coords, PERCENTILES):
        svg.append(
            f'<line x1="{x_c:.1f}" y1="{plot_y}" x2="{x_c:.1f}" '
            f'y2="{plot_y + plot_h}" stroke="{COLOR_OVERLAY}" stroke-width="1" '
            f'stroke-dasharray="3,3"/>'
        )
        svg.append(
            f'<text x="{x_c:.1f}" y="{plot_y + plot_h + 0x12}" '
            f'fill="{COLOR_TEXT}" font-size="{0x08}" font-weight="bold" '
            f'text-anchor="middle">{label}</text>'
        )

    svg.append(
        f'<line x1="{plot_x}" y1="{plot_y}" x2="{plot_x}" '
        f'y2="{plot_y + plot_h}" stroke="{COLOR_LINE}" stroke-width="1"/>'
    )
    svg.append(
        f'<line x1="{plot_x}" y1="{plot_y + plot_h}" '
        f'x2="{plot_x + plot_w}" y2="{plot_y + plot_h}" '
        f'stroke="{COLOR_LINE}" stroke-width="1"/>'
    )

    for bench, pat, label, color in SERIES_SPECS:
        entry = next(
            (e for (b, w, t), e in lookup.items()
             if b == bench and pat in w and t == 1), None
        )
        if not entry:
            continue

        lat_table = entry.get("latency_micros", {})
        coords = []
        for x_c, (pkey, plabel) in zip(pct_x_coords, PERCENTILES):
            val = lat_table.get(pkey, 0.0)
            clamped = min(max(val, 0.0), max_y)
            norm_y = clamped / max_y
            y_c = plot_y + plot_h - (norm_y * float(plot_h))
            coords.append((x_c, y_c, val, plabel))

        pts_str = " ".join(f"{xc:.1f},{yc:.1f}" for xc, yc, _, _ in coords)
        svg.append(
            f'<polyline points="{pts_str}" fill="none" stroke="{color}" '
            f'stroke-width="2"/>'
        )

        for xc, yc, val, plabel in coords:
            svg.append(
                f'<circle cx="{xc:.1f}" cy="{yc:.1f}" r="3" fill="{color}">'
                f'<title>{label} ({plabel}): {val:.2f} µs</title>'
                f'</circle>'
            )

    text_y = y_1us + 0x0C
    svg.append(
        f'<text x="{plot_x + plot_w - 0x08}" y="{text_y:.1f}" '
        f'fill="{ACCENT_TEAL}" font-size="{0x08}" font-weight="bold" '
        f'text-anchor="end">'
        f'SUB-MICROSECOND REGION'
        f'</text>'
    )

    leg_x = plot_x + plot_w + 0x18
    leg_y = plot_y + 4
    svg.append(
        f'<text x="{leg_x}" y="{leg_y}" fill="{COLOR_TEXT}" '
        f'font-size="{0x08}" font-weight="bold">'
        f'WORKLOAD (1 THREAD)'
        f'</text>'
    )

    for idx, (_, _, label, color) in enumerate(SERIES_SPECS):
        ly = leg_y + 0x12 + idx * 0x16
        svg.append(
            f'<line x1="{leg_x}" y1="{ly}" x2="{leg_x + 0x0E}" y2="{ly}" '
            f'stroke="{color}" stroke-width="2"/>'
        )
        svg.append(
            f'<circle cx="{leg_x + 7}" cy="{ly}" r="3" fill="{color}"/>'
        )
        svg.append(
            f'<text x="{leg_x + 0x14}" y="{ly + 3}" fill="{COLOR_TEXT}" '
            f'font-size="{0x08}" font-weight="bold">{label}</text>'
        )

    note_x = title_x
    note_y = plot_y + plot_h + 0x2C
    note_lines = [
        f"* Note: Percentile latency distribution (p50 to p99.9) capped at {0x0A} µs with 2 µs "
        "step on AWS c5d.large (1T); sub-microsecond band showcases pure in-memory SIMD "
        "and hash table fast-path",
    ]
    for idx, line in enumerate(note_lines):
        ly = note_y + idx * 0x10
        svg.append(
            f'<text x="{note_x}" y="{ly}" fill="{COLOR_SUBTLE}" '
            f'font-size="{0x08}">{line}</text>'
        )

    svg.append('</svg>')
    return "\n".join(svg)


def main():
    repo_root = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
    src_json = os.path.join(repo_root, "tmp", "turbofox_benchmark_results.json")
    out_dir = os.path.join(repo_root, "assets")
    out_svg = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
        out_dir, "latency_profile.svg"
    )

    if not os.path.exists(src_json):
        sys.stderr.write(f"benchmark results missing at {src_json}\n")
        sys.exit(1)

    parent_dir = os.path.dirname(out_svg)
    if parent_dir:
        os.makedirs(parent_dir, exist_ok=True)
    lookup = load_results(src_json)
    svg_content = generate_svg(lookup)

    ET.fromstring(svg_content)

    with open(out_svg, "w", encoding="utf-8") as handle:
        handle.write(svg_content)
        handle.write("\n")

    print(f"Chart written to {out_svg}")


if __name__ == "__main__":
    main()
