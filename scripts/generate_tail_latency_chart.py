#!/usr/bin/env python3
"""Generate Catppuccin Latte concurrency tail latency (p99) SVG with monospace font"""

import json
import os
import sys
import xml.etree.ElementTree as ET

MEM_WORKLOADS = [
    ("read_perf", "point_hit", "READ: Point Hit (Existing)"),
    ("read_perf", "point_miss", "READ: Point Miss (Absent)"),
    ("read_perf", "zipf_skewed", f"READ: Zipfian ({0x50}/{0x14} Skew)"),
    ("del_perf", "delete_hit", "DELETE: Point Hit (Existing)"),
    ("del_perf", "delete_miss", "DELETE: Point Miss (Absent)"),
    ("del_perf", "high_contention", "DELETE: Hot-Key Contention"),
    ("del_perf", "churn_insert_delete", f"CHURN: {0x32}/{0x32} Put/Del"),
]

DISK_WORKLOADS = [
    ("write_ingest", "sequential_append", "WAL: Ingest (NVMe fsync)"),
]

THREADS = [1, 4]

COLOR_BASE = "#eff1f5"
COLOR_SURFACE = "#e6e9ef"
COLOR_OVERLAY = "#ccd0da"
COLOR_MUTED = "#8c8fa1"
COLOR_SUBTLE = "#6c6f85"
COLOR_TEXT = "#4c4f69"
COLOR_BORDER = "#ccd0da"
COLOR_LINE = "#bcc0cc"

THREAD_COLORS = ["#1e66f5", "#8839ef"]

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

    svg.append(
        f'<rect width="{svg_w}" height="{svg_h}" rx="{0x0C}" '
        f'fill="{COLOR_BASE}" stroke="{COLOR_BORDER}" stroke-width="1"/>'
    )

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
        f'Tail latency scaling across 1 and 4 concurrent threads (Catppuccin Latte theme)'
        f'</text>'
    )

    leg_x = svg_w - 0x90
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

    row_h1 = p1_h / float(len(MEM_WORKLOADS))
    bar_h = 0x09
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
            by = y_top + 4 + t_idx * (bar_h + 3)
            c = THREAD_COLORS[t_idx]
            svg.append(
                f'<rect x="{p1_x}" y="{by:.1f}" width="{bw:.1f}" '
                f'height="{bar_h}" rx="2" fill="{c}"/>'
            )
            svg.append(
                f'<text x="{p1_x + bw + 4:.1f}" y="{by + 7:.1f}" fill="{c}" '
                f'font-size="{0x07}" font-weight="bold">{val:.2f}</text>'
            )

    svg.append(
        f'<text x="{p2_x}" y="{p2_y - 0x0A}" fill="{COLOR_TEXT}" '
        f'font-size="{0x0A}" font-weight="bold">'
        f'Disk NVMe Persistence (Milliseconds ms)'
        f'</text>'
    )

    max_p2 = float(0x4E20)
    ticks_p2 = [0, float(0x1388), float(0x2710), float(0x3A98), float(0x4E20)]
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

    note_x = title_x
    note_y = p1_y + p1_h + 0x30
    note_lines = [
        "* Note: p99 tail latency across 1T and 4T worker threads; "
        "Point Hit/Miss tests uniform lookup vs key-not-found rejection; "
        f"Zipfian ({0x50}/{0x14}) evaluates cache skew; Churn {0x32}/{0x32} stresses "
        "tombstone cleanup",
        "  WAL Ingest evaluates parallel append logging durability and NVMe fsync "
        "flush queueing under thread contention",
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

    ET.fromstring(svg)

    with open(out_path, "w", encoding="utf-8") as handle:
        handle.write(svg)
        handle.write("\n")
    print(f"Generated {out_path}")


if __name__ == "__main__":
    main()
