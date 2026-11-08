#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEFAULT_OUT="tmp/results/turbofox_benchmark_results.json"
TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/turbofox-target}"
BENCH_DIR="/tmp/turbofox_bench"

OUT_FILE="${DEFAULT_OUT}"
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
    --dir)
      BENCH_DIR="$2"
      shift 2
      ;;
    --out)
      OUT_FILE="$2"
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
export TMPDIR="${BENCH_DIR}"

# Collect system metadata
echo "[$(date +%T)] Collecting machine hardware, memory, and storage telemetry..." >&2
METADATA_JSON=$(python3 "${SCRIPT_DIR}/collect_machine_metadata.py" \
  --bench-dir "${BENCH_DIR}" --region "${REGION}" 2>/dev/null || echo "{}")

echo "[$(date +%T)] Compiling benchmarks in release mode with target-cpu=native..." >&2
RUSTFLAGS="${RUSTFLAGS:--C target-cpu=native}" CARGO_TARGET_DIR="${TARGET_DIR}" cargo bench --no-run --message-format=short
echo "[$(date +%T)] Benchmark compilation completed" >&2

DEPS_DIR="${TARGET_DIR}/release/deps"
SCALE_BIN=$(find "${DEPS_DIR}" -maxdepth 1 -name "scale_perf-*" -executable -type f | head -n 1)
KV_BIN=$(find "${DEPS_DIR}" -maxdepth 1 -name "kv_perf-*" -executable -type f ! -name "kv_perf_multi_tx-*" | head -n 1)
MULTI_BIN=$(find "${DEPS_DIR}" -maxdepth 1 -name "kv_perf_multi_tx-*" -executable -type f | head -n 1)

if [[ -z "${SCALE_BIN}" || -z "${KV_BIN}" || -z "${MULTI_BIN}" ]]; then
  echo "Error: Failed to locate all benchmark binaries in ${DEPS_DIR}" >&2
  exit 1
fi

TMP_RESULTS=$(mktemp)
echo "[" > "${TMP_RESULTS}"
FIRST_ENTRY=1

drop_page_cache() {
  sync
  if [[ $(id -u) -eq 0 ]]; then
    echo 3 > /proc/sys/vm/drop_caches 2>/dev/null || true
  elif command -v sudo >/dev/null 2>&1; then
    sudo -n sh -c "sync; echo 3 > /proc/sys/vm/drop_caches" 2>/dev/null || true
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
RUN_MULTI=0

IFS=',' read -ra BENCH_ID_ARRAY <<< "${SELECTED_BENCHES}"
for b_id in "${BENCH_ID_ARRAY[@]}"; do
  case "${b_id,,}" in
    all)
      RUN_SCALE=1
      RUN_KV=1
      RUN_MULTI=1
      ;;
    1|scale|scale_perf)
      RUN_SCALE=1
      ;;
    2|kv|kv_perf)
      RUN_KV=1
      ;;
    3|multi|kv_multi|kv_perf_multi_tx)
      RUN_MULTI=1
      ;;
    *)
      echo "Unknown benchmark identifier: '${b_id}'" >&2
      echo "Available IDs: 1 (scale), 2 (kv), 3 (multi), all" >&2
      exit 1
      ;;
  esac
done

# 1. Run Scale Benchmark across scaling intervals
if [[ ${RUN_SCALE} -eq 1 ]]; then
  taskset_single=""
  if command -v taskset >/dev/null 2>&1; then
    taskset_single="taskset -c 0"
  fi

  echo "[$(date +%T)] === [1/3] Running scale_perf across scaling intervals (core 0) ===" >&2
  drop_page_cache
  json_out=$(${taskset_single} "${SCALE_BIN}")
  append_result "${json_out}"
  echo "[$(date +%T)] Completed scale_perf" >&2
  drop_page_cache
  sleep 2
fi

# 2. Run Unified Core KV Benchmark (threads=1, set 80/20, get/del 60/40)
if [[ ${RUN_KV} -eq 1 ]]; then
  taskset_single=""
  if command -v taskset >/dev/null 2>&1; then
    taskset_single="taskset -c 0"
  fi

  echo "[$(date +%T)] === [2/3] Running kv_perf [set 80/20, get/del 60/40] (threads=1, core 0) ===" >&2
  drop_page_cache
  json_out=$(${taskset_single} "${KV_BIN}")
  append_result "${json_out}"
  echo "[$(date +%T)] Completed kv_perf" >&2
  drop_page_cache
  sleep 2
fi

# 3. Run Multi-threaded KV Benchmark (threads=2, concurrent transactions)
if [[ ${RUN_MULTI} -eq 1 ]]; then
  echo "[$(date +%T)] === [3/3] Running kv_perf_multi_tx (threads=2, physical cores 0-1) ===" >&2
  drop_page_cache
  json_out=$("${MULTI_BIN}")
  append_result "${json_out}"
  echo "[$(date +%T)] Completed kv_perf_multi_tx" >&2
  drop_page_cache
fi

echo "]" >> "${TMP_RESULTS}"

# Write final consolidated JSON artifact
cat <<EOF > "${OUT_FILE}"
{
  "metadata": ${METADATA_JSON},
  "results": $(cat "${TMP_RESULTS}")
}
EOF

rm -f "${TMP_RESULTS}"
echo "Benchmark session completed; artifact written to ${OUT_FILE}" >&2
