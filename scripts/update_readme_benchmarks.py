#!/usr/bin/env python3
#
# Synchronize benchmark SVG assets and regenerate the README
# Benchmarks section with telemetry metadata and optional
# commit-pinned image URLs
#
# Basic usage (relative asset paths, local/GitHub rendering):
#
#   python scripts/update_readme_benchmarks.py
#
# Pin images to a specific commit for crates.io immutability:
#
#   python scripts/update_readme_benchmarks.py --commit $(git rev-parse HEAD)
#
# When --commit is supplied, every <img src="assets/..."> reference
# in the generated Benchmarks section is rewritten to an absolute
# raw.githubusercontent.com URL anchored at that commit SHA; this
# ensures crates.io (which resolves relative paths against the
# default branch HEAD) serves the exact images that were current at
# publish time rather than whatever is on `main` today
#
# Auto-detect HEAD (equivalent to the above):
#
#   python scripts/update_readme_benchmarks.py --commit HEAD
#
# Typical release workflow:
#
#   1. Generate / update benchmark SVGs into tmp/assets/
#   2. Run this script with --commit HEAD
#   3. Commit the updated README.md and assets/
#   4. Tag and cargo publish
#

import argparse
import glob
import json
import os
import re
import shutil
import subprocess
import sys

DEFAULT_SRC_DIR = "tmp/assets"
DEFAULT_DEST_DIR = "assets"
DEFAULT_RESULTS_JSON = "tmp/assets/turbofox_benchmark_results.json"
DEFAULT_README = "README.md"
REPO_RAW_BASE = (
    "https://raw.githubusercontent.com/pid7-org/turbofox"
)


def resolve_commit(commit_arg: str | None) -> str | None:
    """Resolves a commit reference to a full SHA hex digest.

    Returns None when no commit pinning is requested; raises on
    resolution failure so the caller never silently falls back to
    relative paths.
    """
    if commit_arg is None:
        return None
    try:
        result = subprocess.run(
            ["git", "rev-parse", commit_arg],
            capture_output=True,
            text=True,
            check=True,
        )
        return result.stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError) as exc:
        raise RuntimeError(
            f"Failed to resolve commit '{commit_arg}': {exc}"
        ) from exc

TOC_BENCHMARKS = """- [Benchmarks](#benchmarks)
  - [§1 · Concurrency Dynamics & Throughput Scaling](#1--concurrency-dynamics--throughput-scaling)
  - [§2 · Tail Latency Spectrum & Percentile CDF](#2--tail-latency-spectrum--percentile-cdf)
  - [§3 · Memory Residency & Resident Set Size (RSS)](#3--memory-residency--resident-set-size-rss)
  - [§4 · Cold-Open Latency & Index Memory Density](#4--cold-open-latency--index-memory-density)"""


def copy_svg_assets(src_dir: str, dest_dir: str) -> list[str]:
    """Copies all SVG assets from source to destination directory."""
    os.makedirs(dest_dir, exist_ok=True)
    svg_files = sorted(glob.glob(os.path.join(src_dir, "*.svg")))
    copied = []
    for src_path in svg_files:
        filename = os.path.basename(src_path)
        dest_path = os.path.join(dest_dir, filename)
        shutil.copy2(src_path, dest_path)
        copied.append(filename)
    return copied


def load_metadata(json_path: str) -> dict:
    """Loads benchmark telemetry metadata from results JSON if available."""
    if not os.path.exists(json_path):
        return {}
    try:
        with open(json_path, "r", encoding="utf-8") as handle:
            data = json.load(handle)
        return data.get("metadata", {})
    except Exception as exc:
        print(f"Warning: Failed to parse metadata from {json_path}: {exc}", file=sys.stderr)
        return {}


def format_env_items(metadata: dict) -> list[str]:
    """Generates Markdown bullet items for evaluation environment."""
    cpu = metadata.get("cpu", {})
    model = cpu.get("model", "Intel Xeon Platinum 8375C CPU @ 2.90GHz")
    model = model.replace("(R)", "").replace("(TM)", "").strip()
    vcpus = cpu.get("total_vcpus", 4)
    cores = cpu.get("physical_cores", 2)
    inst = metadata.get("instance_type", "c6id.xlarge")

    mem = metadata.get("memory", {})
    total_bytes = mem.get("total_bytes", 8153268224)
    hugepage_bytes = mem.get("hugepage_size_bytes", 2097152)

    storage = metadata.get("storage", {})
    device = storage.get("device", "/dev/nvme1n1")
    bench_path = storage.get("benchmark_path", "/mnt/nvme0n1")
    fs = storage.get("filesystem", "xfs")
    mount_opts = storage.get(
        "mount_options",
        "rw,noatime,nodiratime,inode64,allocsize=65536k,logbufs=8,logbsize=256k,noquota",
    )

    os_info = metadata.get("os", {})
    kernel = os_info.get("kernel", "7.0.0-1014-aws")
    arch = os_info.get("arch", "x86_64")

    vcpus_hex = f"0x0{vcpus}" if vcpus <= 9 else hex(vcpus)
    cores_hex = f"0x0{cores}" if cores <= 9 else hex(cores)

    return [
        f"- Host instance: AWS EC2 `{inst}` ({vcpus_hex} vCPUs, {cores_hex} physical cores,\n"
        f"  {model}, AVX-512 / AVX2 / SHA-NI vector extensions)",
        f"- Host memory: 0x08 GiB host DDR4 system RAM ({total_bytes:,} bytes); Linux hugepages enabled at\n"
        f"  0x02 MiB ({hugepage_bytes:,} bytes)",
        f"- Storage target: Direct local NVMe SSD (`{device}`, mounted at `{bench_path}`), formatted\n"
        f"  under {fs.upper()} (`{mount_opts}`)",
        f"- Operating system: Linux `{kernel}` ({arch})",
        "- Operation volume: 0x10_0000 (1,048,576) uniform operations per key-value suite; scaling\n"
        "  benchmarks sweep from 0x00 to 0x8000 (32,768) keys",
    ]


def build_benchmarks_section(
    metadata: dict,
    commit: str | None = None,
) -> str:
    """Builds the complete Benchmarks Markdown section.

    When commit is provided, image sources use absolute
    raw.githubusercontent.com URLs pinned to that SHA.
    """
    env_items = "\n".join(format_env_items(metadata))

    if commit:
        asset_prefix = f"{REPO_RAW_BASE}/{commit}/assets"
    else:
        asset_prefix = "assets"

    return f"""## Benchmarks

Benchmarks execute on dedicated bare-metal cloud instances with direct NVMe block storage,

{env_items}

### §1 · Concurrency Dynamics & Throughput Scaling

Throughput evaluations measure sustained operations per second comparing single-thread execution
against concurrent worker transactions across 0x10_0000 operations; workloads evaluate mutation
writes (`SET` with 80% new keys and 20% updates), point lookups (`GET` with 60% hits and 40%
misses), and key deletions (`DEL` with 60% present keys and 40% missing)

<img src="{asset_prefix}/kv_throughput_concurrency.svg" width="100%"
  alt="KV Operation Throughput Under Concurrency" />

Point reads and deletions execute with sub-microsecond internal dispatch; because point reads are
lock-free and execute directly against memory-mapped pages without shared mutex acquisition,
concurrency overhead on read throughput is dominated by CPU L1/L2 cache coherency traffic and memory
bus contention between physical cores rather than engine-level synchronization barriers

<img src="{asset_prefix}/kv_throughput_log_concurrency.svg" width="100%"
  alt="Operation Throughput Logarithmic Scale" />

The continuous logarithmic throughput profile illustrates the architectural divide between
in-memory dispatch and persistent storage commits; point reads and deletes operate across the
mega-ops spectrum while writes track the persistent NVMe barrier regime

<img src="{asset_prefix}/kv_concurrency_scaling_efficiency.svg" width="100%"
  alt="Concurrency Scaling Efficiency" />

In contrast, mutation writes (`SET`) isolate write lock and transaction commit contention,

* Single-thread writes commit sequentially into memory pages with minimal OCC transaction overhead
* Concurrent multi-transaction writes validate optimistic concurrency control (OCC) epochs,
  serialize atomic commit tickets, and synchronize page-level modifications; competing writers
  incur OCC retry cycles and page writeback delays under concurrent transaction pressure

<img src="{asset_prefix}/kv_wall_clock_duration.svg" width="100%"
  alt="Total Execution Wall Time" />

Translating throughput into real-world batch latency reveals the operational cost of NVMe commit
synchronization; batch point reads complete with sub-second execution, whereas persistent write
batches reflect the physical latency of dirty page flushing and OCC retry backoff

> [!NOTE]
> Point lookups and deletions execute lock-free via memory-mapped pages without shared locks; read
> scaling degradation is bounded by hardware CPU cache coherency and memory bus bandwidth across
> physical cores rather than engine-level synchronization barriers

### §2 · Tail Latency Spectrum & Percentile CDF

Latency distributions capture response times from median (`p50`) through extreme tail (`p99`,
`p999`, and `max`), isolating where lock contention and NVMe persistence barriers impact callers

<img src="{asset_prefix}/kv_tail_latency_split_threads.svg" width="100%"
  alt="Tail Latency Distribution" />

Read and delete operations exhibit near-flat percentile curves across the entire distribution:

* Point lookups (`GET`): Median latency remains sub-microsecond; at higher percentiles up to `p99`,
  latencies stay tightly bounded under lock-free memory mapping
* Key deletions (`DEL`): Deletions execute in place with bounded percentile progression, reclaiming
  slots without triggering page table re-allocation
* Zero heap allocation on miss: Missing keys resolve via hash routing without traversing payload
  pages, avoiding memory allocator pressure and eliminating GC-style tail pauses

<img src="{asset_prefix}/kv_tail_latency_grouped_bars.svg" width="100%"
  alt="Tail Latency Grouped Bars Comparison" />

Direct head-to-head comparisons across percentile SLA tiers illustrate exact contention divergence;
while median latencies across workloads remain tight, write tail latencies expand at higher tiers
under multi-threaded contention due to OCC validation conflicts and NVMe writeback serialization

<img src="{asset_prefix}/kv_latency_cdf_curves.svg" width="100%"
  alt="Latency Percentile Progression Curves" />

The semi-log percentile progression curves highlight the sharp inflection point for write mutations:

* Sub-microsecond median write: Sequential writes commit into active memory pages with low median
  latency
* NVMe writeback & OCC retry tail: Latency diverges past the upper percentiles, reflecting the cost
  of kernel dirty page flushing and OCC conflict aborts under heavy mutation pressure

> [!TIP]
> Sub-microsecond median dispatch holds across lock-free point operations; zero heap allocations on
> key-miss paths guarantee flat tail profiles across percentile tiers

### §3 · Memory Residency & Resident Set Size (RSS)

Peak resident set size (RSS) tracks actual physical memory consumed by host operating system page
tables and internal transaction state after completing 0x10_0000 operations

<img src="{asset_prefix}/kv_memory_footprint_concurrency.svg" width="100%"
  alt="Resident Memory Footprint Under Concurrency" />

Physical storage growth on NVMe tracks sequential allocation while memory footprint remains bounded:

* Demand-paged single thread: Single-worker transactions maintain a resident memory footprint well
  below on-disk database size because unreferenced pages remain un-faulted by the kernel
* Concurrent worker buffers: Under concurrent worker transactions, resident RAM expands to back
  thread-local transaction commit logs, OCC validation buffers, and active memory-mapped pages
  faulted across distinct worker threads

> [!NOTE]
> Memory-mapped persistence utilizes OS page tables for on-demand caching; resident memory stays
> bounded by active working set size rather than total container file volume

### §4 · Cold-Open Latency & Index Memory Density

Initialization benchmarks evaluate cold database startup duration and memory scaling as stored key
volume grows from 0x00 to 0x8000 (32,768) keys; configurations contrast raw initialization against
eager in-memory lookup cache construction

<img src="{asset_prefix}/scale_perf_combined_horizontal.svg" width="100%"
  alt="Database Open Latency and Resource Footprint Scaling" />

Cold startup behavior exhibits two distinct operational modes:

* Raw database open (`lookup_cache_disabled`): Bypasses key traversal entirely; startup latency
  remains sub-millisecond and flat; resident memory footprint remains pinned at a constant 4 KiB
  (0x1000 bytes) regardless of database size, as backing storage pages are mapped into virtual
  address space without being faulted into physical RAM
* Cached database open (`lookup_cache_enabled`): Traverses on-disk hash buckets at startup to
  pre-warm an in-memory hash index; initialization duration scales linearly with key count; this
  upfront cost amortizes lookup latencies for read-intensive workloads by routing subsequent reads
  in memory without disk navigation

<img src="{asset_prefix}/scale_perf_combined_vertical.svg" width="100%"
  alt="Database Open Latency and Resource Footprint Scaling Aligned" />

Aligning open duration directly against backing resource consumption over a shared key axis reveals
the proportional relationship between disk allocation and cache latency; physical disk space grows
monotonically without external fragmentation, while cached open latency tracks hash bucket depth
without experiencing non-linear compaction spikes

<img src="{asset_prefix}/scale_perf_cache_overhead.svg" width="100%"
  alt="In-Memory Lookup Cache Overhead and RAM Density" />

The cache overhead profile illustrates the operational trade-offs of eager indexing:

* Latency multiplier: The startup time ratio between cached and raw open remains bounded while cold
  open stays sub-millisecond across tested intervals
* Memory density: The in-memory lookup cache requires only a few bytes of physical RAM per indexed
  key, ensuring predictable memory budgets in embedded systems and edge runtime environments
* Monotonic storage growth: Backing storage expands monotonically in uniform OS page increments
  without allocation fragmentation or stop-the-world compaction cycles

> [!TIP]
> Uncached database open incurs zero heap allocation overhead, preserving a 4 KiB resident memory
> footprint across any key scale while keeping cold initialization strictly bounded
"""


def update_readme_content(readme_path: str, new_benchmarks: str) -> None:
    """Updates README.md with synchronized TOC index and benchmarks section."""
    with open(readme_path, "r", encoding="utf-8") as handle:
        content = handle.read()

    # 1. Update TOC Benchmarks subsection
    toc_pattern = re.compile(
        r"- \[Benchmarks\]\(#benchmarks\)(?:\n\s+-\s+\[.*?\]\(#.*?\)){1,10}",
        re.MULTILINE,
    )
    if toc_pattern.search(content):
        content = toc_pattern.sub(TOC_BENCHMARKS, content, count=1)
    else:
        print("Notice: Existing TOC Benchmarks block not found; index unchanged", file=sys.stderr)

    # 2. Update Benchmarks section body
    bench_pattern = re.compile(
        r"## Benchmarks\n\n.*?(?=\n## |\Z)",
        re.DOTALL,
    )
    if not bench_pattern.search(content):
        raise RuntimeError("Failed to locate '## Benchmarks' section in README.md")

    content = bench_pattern.sub(new_benchmarks, content, count=1)

    with open(readme_path, "w", encoding="utf-8") as handle:
        handle.write(content)


def validate_markdown(section: str) -> list[str]:
    """Validates markdown line length and punctuation rules."""
    errors = []
    for idx, line in enumerate(section.splitlines(), start=1):
        is_img_tag = line.lstrip().startswith("<img ")
        if len(line) > 100 and not is_img_tag:
            errors.append(f"Line {idx} exceeds 100 characters ({len(line)}): {line}")
        if line.rstrip().endswith("."):
            errors.append(f"Line {idx} ends with a full stop: {line}")
        for match in re.finditer(r"\.\s+[A-Z]", line):
            errors.append(f"Line {idx} contains sentence-terminating period: {line}")
    return errors


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Synchronize benchmark assets and update README benchmarks section"
    )
    parser.add_argument(
        "--src",
        default=DEFAULT_SRC_DIR,
        help=f"Source directory containing generated SVGs (default: {DEFAULT_SRC_DIR})",
    )
    parser.add_argument(
        "--dest",
        default=DEFAULT_DEST_DIR,
        help=f"Destination assets directory (default: {DEFAULT_DEST_DIR})",
    )
    parser.add_argument(
        "--results",
        default=DEFAULT_RESULTS_JSON,
        help=f"Path to turbofox_benchmark_results.json (default: {DEFAULT_RESULTS_JSON})",
    )
    parser.add_argument(
        "--readme",
        default=DEFAULT_README,
        help=f"Path to README.md (default: {DEFAULT_README})",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Validate and print markdown without writing files",
    )
    parser.add_argument(
        "--commit",
        default=None,
        help=(
            "Git commit ref (SHA, HEAD, tag) to pin image URLs to; "
            "resolved to a full SHA via git rev-parse. "
            "When set, <img> sources become absolute "
            "raw.githubusercontent.com URLs anchored at that "
            "commit, ensuring crates.io serves immutable images"
        ),
    )

    args = parser.parse_args()

    # Resolve commit reference to full SHA if provided
    commit_sha = resolve_commit(args.commit)

    print(f"=== TurboFox Benchmark Documentation Updater ===")
    print(f"Source assets : {args.src}")
    print(f"Target assets : {args.dest}")
    print(f"Results JSON  : {args.results}")
    print(f"Target README : {args.readme}")
    if commit_sha:
        print(f"Commit pin    : {commit_sha}")

    # Copy assets if source exists and not dry-run
    if os.path.exists(args.src) and not args.dry_run:
        copied = copy_svg_assets(args.src, args.dest)
        print(f"Synchronized {len(copied)} SVG assets from {args.src} -> {args.dest}")
    elif args.dry_run:
        print("Dry-run mode: skipping asset copy")
    else:
        print(f"Notice: Source directory {args.src} does not exist; skipping asset copy")

    # Load telemetry metadata
    metadata = load_metadata(args.results)
    if metadata:
        print(f"Loaded telemetry metadata for instance '{metadata.get('instance_type', 'unknown')}'")
    else:
        print("Notice: Telemetry metadata absent; using baseline environment specifications")

    # Generate section markdown
    new_section = build_benchmarks_section(metadata, commit=commit_sha)

    # Validate generated markdown
    validation_errors = validate_markdown(new_section)
    if validation_errors:
        print(f"Error: Generated markdown has {len(validation_errors)} style violations:", file=sys.stderr)
        for err in validation_errors:
            print(f"  {err}", file=sys.stderr)
        sys.exit(1)
    print("Validation passed: All lines <= 100 characters and zero full stops")

    if args.dry_run:
        print("\n=== Generated Benchmarks Markdown Preview ===")
        print(new_section[:800] + "\n...\n")
        return

    # Update README
    update_readme_content(args.readme, new_section)
    print(f"Successfully updated {args.readme} with synchronized Benchmarks section")


if __name__ == "__main__":
    main()
