#!/usr/bin/env bash

set -euo pipefail

DEFAULT_OPS="0x186A0" # 100,000 ops
DEFAULT_THREADS="1,4"
DEFAULT_SCALES="1000,10000,100000"
DEFAULT_OUT="tmp/results/turbofox_benchmark_results.json"
TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/turbofox-target}"
BENCH_DIR="/tmp/turbofox_bench"

OPS="${DEFAULT_OPS}"
THREADS_LIST="${DEFAULT_THREADS}"
SCALES_LIST="${DEFAULT_SCALES}"
OUT_FILE="${DEFAULT_OUT}"

WARMUP="0x00"
REGION="${AWS_REGION:-${AWS_DEFAULT_REGION:-unknown}}"
SELECTED_BENCHES="all"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --benches|--bench)
      if [[ "${SELECTED_BENCHES}" == "all" ]]; then
        SELECTED_BENCHES="$2"
      else
        SELECTED_BENCHES="${SELECTED_BENCHES},$2"
      fi
      shift 2
      ;;
    --ops)
      OPS="$2"
      shift 2
      ;;
    --threads)
      THREADS_LIST="$2"
      shift 2
      ;;
    --scales)
      SCALES_LIST="$2"
      shift 2
      ;;
    --dir)
      BENCH_DIR="$2"
      shift 2
      ;;
    --out)
      OUT_FILE="$2"
      shift 2
      ;;
    --warmup)
      WARMUP="$2"
      shift 2
      ;;
    --region)
      REGION="$2"
      shift 2
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 1
      ;;
  esac
done

mkdir -p "${BENCH_DIR}"
mkdir -p "$(dirname "${OUT_FILE}")"

# Collect system metadata
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
KERNEL=$(uname -r)
ARCH=$(uname -m)
CPU_MODEL=$(lscpu | grep "Model name" | sed 's/.*: *//' | tr -d '\n' || echo "Unknown")
TOTAL_CPUS=$(nproc || echo "1")
INSTANCE_TYPE="unknown"
if [[ -f "/sys/devices/virtual/dmi/id/product_name" ]]; then
  INSTANCE_TYPE=$(cat /sys/devices/virtual/dmi/id/product_name | tr -d '\n')
fi

echo "Compiling benchmarks in release mode with target-cpu=native..." >&2
RUSTFLAGS="${RUSTFLAGS:--C target-cpu=native}" CARGO_TARGET_DIR="${TARGET_DIR}" cargo bench --no-run

DEPS_DIR="${TARGET_DIR}/release/deps"
SCALE_BIN=$(find "${DEPS_DIR}" -maxdepth 1 -name "scale_perf-*" -executable -type f | head -n 1)
KV_BIN=$(find "${DEPS_DIR}" -maxdepth 1 -name "kv_perf-*" -executable -type f | head -n 1)

if [[ -z "${SCALE_BIN}" || -z "${KV_BIN}" ]]; then
  echo "Error: Failed to locate benchmark binaries in ${DEPS_DIR}" >&2
  exit 1
fi

TMP_RESULTS=$(mktemp)
echo "[" > "${TMP_RESULTS}"
FIRST_ENTRY=1

drop_page_cache() {
  sync
  if [[ $(id -u) -eq 0 ]]; then
    echo 3 > /proc/sys/vm/drop_caches 2>/dev/null || true
  fi
}

append_result() {
  local json="$1"
  if [[ ${FIRST_ENTRY} -eq 0 ]]; then
    echo "," >> "${TMP_RESULTS}"
  fi
  echo "${json}" >> "${TMP_RESULTS}"
  FIRST_ENTRY=0
}

RUN_SCALE=0
RUN_KV=0

IFS=',' read -ra BENCH_ID_ARRAY <<< "${SELECTED_BENCHES}"
for b_id in "${BENCH_ID_ARRAY[@]}"; do
  case "${b_id,,}" in
    all)
      RUN_SCALE=1
      RUN_KV=1
      ;;
    1|scale|scale_perf)
      RUN_SCALE=1
      ;;
    2|kv|kv_perf)
      RUN_KV=1
      ;;
    *)
      echo "Unknown benchmark identifier: '${b_id}'" >&2
      echo "Available IDs: 1 (scale), 2 (kv), all" >&2
      exit 1
      ;;
  esac
done

# Export ops for benchmark binaries
export TURBOFOX_BENCH_OPS="${OPS}"

# 1. Run Scale Benchmark across scaling intervals
if [[ ${RUN_SCALE} -eq 1 ]]; then
  taskset_single=""
  if command -v taskset >/dev/null 2>&1; then
    taskset_single="taskset -c 0"
  fi

  echo "Running scale_perf across scaling intervals..." >&2
  drop_page_cache
  json_out=$(${taskset_single} "${SCALE_BIN}")
  append_result "${json_out}"
fi

# 2. Run Unified Core KV Benchmark (threads=1,2, N=256K, set/get/del 60/40)
if [[ ${RUN_KV} -eq 1 ]]; then
  taskset_cmd=""
  if command -v taskset >/dev/null 2>&1; then
    physical_cpus=$(lscpu -p=CORE,CPU 2>/dev/null | grep -v '^#' | \
      sort -u -t, -k1,1 | cut -d, -f2 | head -n 2 | paste -sd, -)
    if [[ $(echo "${physical_cpus}" | tr -cd ',' | wc -c) -ge 1 ]]; then
      taskset_cmd="taskset -c ${physical_cpus}"
    else
      taskset_cmd="taskset -c 0-1"
    fi
  fi

  echo "Running kv_perf [set/get/del 60/40] (threads=1,2, N=256K)..." >&2
  drop_page_cache
  json_out=$(${taskset_cmd} "${KV_BIN}")
  append_result "${json_out}"
fi

echo "]" >> "${TMP_RESULTS}"

# Write final consolidated JSON artifact
cat <<EOF > "${OUT_FILE}"
{
  "metadata": {
    "timestamp_utc": "${TIMESTAMP}",
    "region": "${REGION}",
    "instance_type": "${INSTANCE_TYPE}",
    "cpu_model": "${CPU_MODEL}",
    "kernel": "${KERNEL}",
    "arch": "${ARCH}",
    "total_cpus": ${TOTAL_CPUS},
    "storage_path": "${BENCH_DIR}",
    "ops_requested": "${OPS}"
  },
  "results": $(cat "${TMP_RESULTS}")
}
EOF

rm -f "${TMP_RESULTS}"
echo "Benchmark session completed; artifact written to ${OUT_FILE}" >&2
