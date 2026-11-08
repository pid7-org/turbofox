#!/usr/bin/env python3


import argparse
import datetime
import json
import os
import platform
import re
import sys


def read_sysfs(path: str, default: str = "") -> str:
    try:
        with open(path, "r", encoding="utf-8") as handle:
            return handle.read().strip()
    except Exception:
        return default


def get_cpu_info() -> dict:
    model = "unknown"
    flags = set()

    try:
        with open("/proc/cpuinfo", "r", encoding="utf-8") as handle:
            for line in handle:
                line = line.strip()
                if not line:
                    continue
                if ":" in line:
                    key, val = [part.strip() for part in line.split(":", 1)]
                    if key in ("model name", "Model Name", "Hardware"):
                        if model == "unknown":
                            model = val
                    elif key in ("flags", "Features"):
                        flags.update(val.split())
    except Exception:
        pass

    if model == "unknown":
        model = platform.processor() or "unknown"

    target_simd = [
        # x86 vector & crypto extensions
        "sse", "sse2", "sse3", "ssse3", "sse4_1", "sse4_2",
        "avx", "avx2", "fma", "avx512f", "avx512dq", "avx512cd",
        "avx512bw", "avx512vl", "avx512vnni", "aes", "sha_ni",
        # aarch64 vector & crypto extensions
        "neon", "asimd", "sve", "sve2", "pmull", "sha1", "sha2",
        "crc32", "atomics", "fphp", "asimdhp"
    ]
    detected_simd = sorted([ext for ext in target_simd if ext in flags])

    # Topology detection via sysfs
    total_vcpus = os.cpu_count() or 1
    physical_cores = set()
    sockets = set()

    cpu_dir = "/sys/devices/system/cpu"
    if os.path.exists(cpu_dir):
        for entry in os.listdir(cpu_dir):
            if entry.startswith("cpu") and entry[3:].isdigit():
                topo = os.path.join(cpu_dir, entry, "topology")
                pkg_id = read_sysfs(os.path.join(topo, "physical_package_id"))
                core_id = read_sysfs(os.path.join(topo, "core_id"))
                if pkg_id:
                    sockets.add(pkg_id)
                if pkg_id and core_id:
                    physical_cores.add(f"{pkg_id}:{core_id}")
                elif core_id:
                    physical_cores.add(core_id)

    num_sockets = max(len(sockets), 1)
    num_cores = max(len(physical_cores), 1)
    threads_per_core = max(total_vcpus // num_cores, 1)

    governor = read_sysfs(
        "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor",
        default="unknown"
    )

    return {
        "model": model,
        "total_vcpus": total_vcpus,
        "physical_cores": num_cores,
        "sockets": num_sockets,
        "threads_per_core": threads_per_core,
        "scaling_governor": governor,
        "simd_extensions": detected_simd
    }


def get_memory_info() -> dict:
    mem = {}
    try:
        with open("/proc/meminfo", "r", encoding="utf-8") as handle:
            for line in handle:
                parts = line.split(":")
                if len(parts) == 2:
                    k = parts[0].strip()
                    v = parts[1].strip().split()[0]
                    if v.isdigit():
                        mem[k] = int(v) * 1024
    except Exception:
        pass

    thp_raw = read_sysfs("/sys/kernel/mm/transparent_hugepage/enabled")
    thp_match = re.search(r"\[(.*?)\]", thp_raw)
    thp_mode = thp_match.group(1) if thp_match else (thp_raw or "unknown")

    swappiness_raw = read_sysfs("/proc/sys/vm/swappiness")
    swappiness = int(swappiness_raw) if swappiness_raw.isdigit() else None

    return {
        "total_bytes": mem.get("MemTotal", 0),
        "available_bytes": mem.get("MemAvailable", 0),
        "hugepage_size_bytes": mem.get("Hugepagesize", 0),
        "transparent_hugepage": thp_mode,
        "swappiness": swappiness
    }


def get_storage_info(target_dir: str) -> dict:
    abs_path = os.path.abspath(target_dir)
    st = os.statvfs(abs_path)
    total_bytes = st.f_blocks * st.f_frsize
    avail_bytes = st.f_bavail * st.f_frsize

    dev = "unknown"
    fs_type = "unknown"
    mount_opts = "unknown"
    rotational = None

    try:
        with open("/proc/mounts", "r", encoding="utf-8") as handle:
            best_mount = ""
            for line in handle:
                parts = line.split()
                if len(parts) >= 4:
                    m_dev, m_point, m_type, m_opts = parts[0], parts[1], parts[2], parts[3]
                    if abs_path.startswith(m_point) and len(m_point) > len(best_mount):
                        best_mount = m_point
                        dev, fs_type, mount_opts = m_dev, m_type, m_opts
    except Exception:
        pass

    if dev.startswith("/dev/"):
        dev_name = os.path.basename(dev)
        parent_disk = re.sub(r"p?\d+$", "", dev_name)
        rot_val = read_sysfs(f"/sys/block/{parent_disk}/queue/rotational")
        if rot_val == "0":
            rotational = False
        elif rot_val == "1":
            rotational = True

    return {
        "benchmark_path": abs_path,
        "device": dev,
        "filesystem": fs_type,
        "mount_options": mount_opts,
        "is_rotational": rotational,
        "total_bytes": total_bytes,
        "available_bytes": avail_bytes
    }


def collect_metadata(bench_dir: str, region: str) -> dict:
    instance_type = read_sysfs(
        "/sys/devices/virtual/dmi/id/product_name",
        default="unknown"
    )

    # Dynamic kernel page size query (supports 4K, 16K, 64K across x86 and aarch64)
    kernel_page_size = os.sysconf("SC_PAGE_SIZE")

    return {
        "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "region": region,
        "instance_type": instance_type,
        "os": {
            "kernel": platform.release(),
            "arch": platform.machine(),
            "page_size_bytes": kernel_page_size
        },
        "cpu": get_cpu_info(),
        "memory": get_memory_info(),
        "storage": get_storage_info(bench_dir)
    }


def main():
    parser = argparse.ArgumentParser(description="Collect machine metadata for TurboFox benchmarks")
    parser.add_argument("--bench-dir", default="/tmp", help="Target benchmark directory path")
    parser.add_argument("--region", default="unknown", help="AWS cloud deployment region")
    args = parser.parse_args()

    meta = collect_metadata(args.bench_dir, args.region)
    print(json.dumps(meta, indent=2))


if __name__ == "__main__":
    main()
