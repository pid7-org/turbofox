#!/usr/bin/env python3
"""Generate Rosé Pine Dawn concurrency tail latency (p99) SVG with monospace font"""

import json
import os
import sys
import xml.etree.ElementTree as ET

# Workloads profiled: (bench, workload, display_label)
MEM_WORKLOADS = [
    ("read_perf", "point_hit", "READ: Point Hit (Existing)"),
    ("read_perf", "point_miss", "READ: Point Miss (Absent)"),
    ("read_perf", "zipf_skewed", f"READ: Zipfian ({0x50}/{0x14} Skew)"),
    ("read_perf", "mixed_95_5", f"READ/WRITE: Mixed {0x5F}/5 OLTP"),
    ("del_perf", "delete_hit", "DELETE: Point Hit (Existing)"),
    ("del_perf", "delete_miss", "DELETE: Point Miss (Absent)"),
    ("del_perf", "high_contention", "DELETE: Hot-Key Contention"),
    ("del_perf", "churn_insert_delete", f"CHURN: {0x32}/{0x32} Put/Del"),
]

DISK_WORKLOADS = [
    ("write_ingest", "sequential_append", "WAL: Ingest (NVMe fsync)"),
]

THREADS = [1, 2, 4]

# Rosé Pine Dawn palette
COLOR_BASE = "#faf4ed"
COLOR_SURFACE = "#fffaf3"
COLOR_OVERLAY = "#f2e9e1"
COLOR_MUTED = "#9893a5"
COLOR_SUBTLE = "#797593"
COLOR_TEXT = "#575279"
COLOR_BORDER = "#dfdad9"
COLOR_LINE = "#cecacd"

# Accent colors for threads (Pine, Iris, Love)
THREAD_COLORS = ["#286983", "#907aa9", "#b4637a"]

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
        thr = entry["threads"]
        lookup[(bench, workload, thr)] = entry
        if bench == "write_perf" and workload == "pure_append":
            lookup[("write_ingest", "sequential_append", thr)] = entry
    return lookup


def generate_svg(lookup):
    svg_w = 0x04C4
    svg_h = 0x0218

    p1_x = 0x0118
    p1_y = 0x54
    p1_w = 0x01A4
    p1_h = 0x0158

    p2_x = 0x033E
    p2_y = p1_y
    p2_w = 0x0140
    p2_h = p1_h

    svg = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/{0x07D0}/svg" '
        f'viewBox="0 0 {svg_w} {svg_h}" width="{svg_w}" height="{svg_h}" '
        f'style="background:{COLOR_BASE}; font-family:{FONT_STACK};">'
    )

    # Base card container
    svg.append(
        f'<rect width="{svg_w}" height="{svg_h}" rx="{0x0C}" '
        f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )

    # Header section
    title_x = 0x24
    svg.append(
        f'<text x="{title_x}" y="{0x22}" fill="{COLOR_TEXT}" '
        f'font-size="{0x10}" font-weight="bold">'
        f'TurboFox Concurrency Tail Latency Profile (p99)'
        f'</text>'
    )
    svg.append(
        f'<text x="{title_x}" y="{0x38}" fill="{COLOR_SUBTLE}" '
        f'font-size="{0x0A}">'
        f'Tail latency scaling across 1, 2, and 4 concurrent threads '
        f'(Rosé Pine Dawn theme)'
        f'</text>'
    )

    # Legend in header
    leg_x = svg_w - 0x010E
    leg_y = 0x22
    for t_idx, thr in enumerate(THREADS):
        lx = leg_x + t_idx * 0x36
        svg.append(
            f'<rect x="{lx}" y="{leg_y - 8}" width="{0x0A}" height="{0x0A}" '
            f'rx="2" fill="{THREAD_COLORS[t_idx]}"/>'
        )
        svg.append(
            f'<text x="{lx + 0x0E}" y="{leg_y}" fill="{COLOR_TEXT}" '
            f'font-size="{0x09}" font-weight="bold">{thr}T</text>'
        )

    # Panel 1: In-Memory Workloads header & grid (0 to 0x16 µs)
    svg.append(
        f'<text x="{p1_x}" y="{p1_y - 0x0A}" fill="{COLOR_TEXT}" '
        f'font-size="{0x0A}" font-weight="bold">'
        f'In-Memory Workloads (Microseconds µs)'
        f'</text>'
    )

    max_p1 = float(0x16)
    ticks_p1 = [0, 5, 0x0A, 0x0F, 0x14]
    for tick in ticks_p1:
        x = p1_x + (float(tick) / max_p1) * p1_w
        svg.append(
            f'<line x1="{x:.1f}" y1="{p1_y}" x2="{x:.1f}" '
            f'y2="{p1_y + p1_h}" stroke="{COLOR_OVERLAY}" stroke-width="1"/>'
        )
        svg.append(
            f'<text x="{x:.1f}" y="{p1_y + p1_h + 0x12}" fill="{COLOR_MUTED}" '
            f'font-size="{0x08}" text-anchor="middle">{tick} µs</text>'
        )

    # Panel 1 Bars & Labels
    row_h1 = p1_h / float(len(MEM_WORKLOADS))
    bar_h = 0x07
    for r_idx, (bench, workload, label) in enumerate(MEM_WORKLOADS):
        y_top = p1_y + float(r_idx) * row_h1
        svg.append(
            f'<text x="{p1_x - 0x0A}" y="{y_top + row_h1 / 2.0 + 3:.1f}" '
            f'fill="{COLOR_TEXT}" font-size="{0x08}" font-weight="bold" '
            f'text-anchor="end">{label}</text>'
        )
        for t_idx, thr in enumerate(THREADS):
            entry = lookup.get((bench, workload, thr))
            val = entry["latency_micros"]["p99"] if entry else 0.0
            bw = max((val / max_p1) * p1_w, 2.0)
            by = y_top + 2 + t_idx * (bar_h + 2)
            c = THREAD_COLORS[t_idx]
            svg.append(
                f'<rect x="{p1_x}" y="{by:.1f}" width="{bw:.1f}" '
                f'height="{bar_h}" rx="2" fill="{c}"/>'
            )
            svg.append(
                f'<text x="{p1_x + bw + 4:.1f}" y="{by + 6:.1f}" fill="{c}" '
                f'font-size="{0x07}" font-weight="bold">{val:.2f}</text>'
            )

    # Panel 2: Disk WAL Persistence header & grid (0 to 0x0A ms)
    svg.append(
        f'<text x="{p2_x}" y="{p2_y - 0x0A}" fill="{COLOR_TEXT}" '
        f'font-size="{0x0A}" font-weight="bold">'
        f'Disk NVMe Persistence (Milliseconds ms)'
        f'</text>'
    )

    max_p2 = float(0x2710)
    ticks_p2 = [0, float(0x09C4), float(0x1388), float(0x1D4C), float(0x2710)]
    for tick in ticks_p2:
        x = p2_x + (tick / max_p2) * p2_w
        svg.append(
            f'<line x1="{x:.1f}" y1="{p2_y}" x2="{x:.1f}" '
            f'y2="{p2_y + p2_h}" stroke="{COLOR_OVERLAY}" stroke-width="1"/>'
        )
        ms_lbl = (
            f"{tick / float(0x03E8):.1f} ms"
            if tick > 0
            else "0 ms"
        )
        svg.append(
            f'<text x="{x:.1f}" y="{p2_y + p2_h + 0x12}" fill="{COLOR_MUTED}" '
            f'font-size="{0x08}" text-anchor="middle">{ms_lbl}</text>'
        )

    # Panel 2 Bars & Labels
    y_top2 = p2_y + 0x28
    svg.append(
        f'<text x="{p2_x}" y="{y_top2 - 8}" fill="{COLOR_SUBTLE}" '
        f'font-size="{0x08}" font-weight="bold">WAL: Ingest (NVMe fsync)</text>'
    )
    for t_idx, thr in enumerate(THREADS):
        entry = lookup.get(("write_ingest", "sequential_append", thr))
        val = entry["latency_micros"]["p99"] if entry else 0.0
        bw = max((val / max_p2) * p2_w, 2.0)
        by = y_top2 + t_idx * 0x16
        c = THREAD_COLORS[t_idx]
        svg.append(
            f'<rect x="{p2_x}" y="{by:.1f}" width="{bw:.1f}" '
            f'height="{0x0C}" rx="2" fill="{c}"/>'
        )
        svg.append(
            f'<text x="{p2_x + bw + 4:.1f}" y="{by + 9:.1f}" fill="{c}" '
            f'font-size="{0x08}" font-weight="bold">'
            f'{val / float(0x03E8):.2f} ms'
            f'</text>'
        )

    # Minimal left-aligned side notes without container box
    note_x = title_x
    note_y = p1_y + p1_h + 0x30
    note_lines = [
        "* Note: p99 tail latency across 1T, 2T, and 4T worker threads; "
        "Point Hit/Miss tests uniform lookup vs key-not-found rejection",
        f"  Zipfian ({0x50}/{0x14}) evaluates cache skew; Mixed {0x5F}/5 "
        f"simulates OLTP; Churn {0x32}/{0x32} stresses tombstone cleanup",
        "  WAL Ingest evaluates parallel append logging durability and NVMe "
        "fsync flush queueing under thread contention",
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
    out_path = os.path.join(root, "assets", "concurrency_tail_latency.svg")
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
