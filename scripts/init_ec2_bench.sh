#!/usr/bin/env bash
#
# TurboFox AWS EC2 Instance Provisioner & Benchmark Initializer
#
# Execution:
#   chmod +x scripts/init_ec2_bench.sh
#   ./scripts/init_ec2_bench.sh
#
# Auto-run benchmarks after host setup:
#   ./scripts/init_ec2_bench.sh --run --ops 0x186A0 --out results.json

set -euo pipefail

AUTO_RUN=0
REPO_URL="https://github.com/pid7-org/turbofox.git"
BRANCH="master"
BENCH_DIR="/mnt/nvme0n1"
PASSTHROUGH_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run)
      AUTO_RUN=1
      shift 1
      ;;
    --repo)
      REPO_URL="$2"
      shift 2
      ;;
    --branch)
      BRANCH="$2"
      shift 2
      ;;
    --bench-dir)
      BENCH_DIR="$2"
      shift 2
      ;;
    *)
      PASSTHROUGH_ARGS+=("$1")
      shift 1
      ;;
  esac
done

SUDO=""
if [[ $(id -u) -ne 0 ]]; then
  if command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
  else
    echo "Error: elevated privileges required; install sudo or run as root" >&2
    exit 1
  fi
fi

echo "=== [1/5] Provisioning OS dependencies ===" >&2
if command -v apt-get >/dev/null 2>&1; then
  $SUDO apt-get update -y
  $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y \
    build-essential git curl xfsprogs util-linux python3 linux-tools-common
elif command -v dnf >/dev/null 2>&1; then
  $SUDO dnf update -y
  $SUDO dnf install -y gcc make git curl xfsprogs util-linux python3 kernel-tools
fi

echo "=== [2/5] Tuning CPU scaling governor and memory subsystem ===" >&2
if command -v cpupower >/dev/null 2>&1; then
  $SUDO cpupower frequency-set -g performance >/dev/null 2>&1 || true
fi

for gov in /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor; do
  if [[ -f "${gov}" ]]; then
    echo "performance" | $SUDO tee "${gov}" >/dev/null 2>&1 || true
  fi
done

for thp in /sys/kernel/mm/transparent_hugepage/enabled /sys/kernel/mm/transparent_hugepage/defrag; do
  if [[ -f "${thp}" ]]; then
    echo "madvise" | $SUDO tee "${thp}" >/dev/null 2>&1 || true
  fi
done

echo "=== [3/5] Initializing local NVMe instance storage ===" >&2
if ! mountpoint -q "${BENCH_DIR}"; then
  NVME_DEV=""
  # Prioritize dedicated AWS EC2 NVMe instance store if identified by udev
  for disk_link in /dev/disk/by-id/nvme-Amazon_EC2_NVMe_Instance_Storage*; do
    if [[ -b "${disk_link}" ]]; then
      real_dev=$(readlink -f "${disk_link}")
      if ! lsblk -rn -o MOUNTPOINT "${real_dev}" 2>/dev/null | grep -q "/"; then
        NVME_DEV="${real_dev}"
        break
      fi
    fi
  done

  # Fallback: scan unmounted NVMe disks that have no active partition mounts
  if [[ -z "${NVME_DEV}" ]]; then
    for dev in /dev/nvme*n1; do
      if [[ -b "${dev}" ]]; then
        mounts=$(lsblk -rn -o MOUNTPOINT "${dev}" 2>/dev/null | grep -v "^$" || true)
        if [[ -z "${mounts}" ]] && ! grep -q "${dev}" /proc/mounts 2>/dev/null; then
          NVME_DEV="${dev}"
          break
        fi
      fi
    done
  fi

  if [[ -n "${NVME_DEV}" ]]; then
    echo "Formatting instance store ${NVME_DEV} with XFS (4K sectors)..." >&2
    $SUDO mkfs.xfs -f -s size=4096 "${NVME_DEV}"
    $SUDO mkdir -p "${BENCH_DIR}"
    $SUDO mount -o noatime,nodiratime,logbufs=8,logbsize=256k,allocsize=64M "${NVME_DEV}" "${BENCH_DIR}"
    $SUDO chmod 0777 "${BENCH_DIR}"
  else
    echo "No unmounted NVMe block device detected; falling back to /tmp/turbofox_bench" >&2
    BENCH_DIR="/tmp/turbofox_bench"
    mkdir -p "${BENCH_DIR}"
  fi
fi

echo "=== [4/5] Configuring Rust toolchain ===" >&2
if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- \
    -y --default-toolchain stable --profile minimal
fi

# shellcheck source=/dev/null
source "${HOME}/.cargo/env" 2>/dev/null || true
export PATH="${HOME}/.cargo/bin:${PATH}"
rustup default stable
rustup update stable

echo "=== [5/5] Synchronizing repository and runner ===" >&2
WORK_DIR="${PWD}"
HAS_CARGO=0
if [[ -f "${WORK_DIR}/Cargo.toml" ]] && grep -q 'name = "turbofox"' "${WORK_DIR}/Cargo.toml"; then
  HAS_CARGO=1
fi

if [[ ${HAS_CARGO} -eq 0 ]]; then
  TARGET_CLONE="${HOME}/turbofox"
  if [[ ! -f "${TARGET_CLONE}/Cargo.toml" ]]; then
    rm -rf "${TARGET_CLONE}"
    git clone --depth 1 -b "${BRANCH}" "${REPO_URL}" "${TARGET_CLONE}"
  fi
  WORK_DIR="${TARGET_CLONE}"
fi

RUNNER=""
if [[ -f "${WORK_DIR}/scripts/run_benchmarks.sh" ]]; then
  RUNNER="${WORK_DIR}/scripts/run_benchmarks.sh"
elif [[ -f "${WORK_DIR}/scripts/run_ec2_benches.sh" ]]; then
  RUNNER="${WORK_DIR}/scripts/run_ec2_benches.sh"
else
  echo "Error: benchmark runner script not found in ${WORK_DIR}/scripts" >&2
  exit 1
fi
chmod +x "${RUNNER}"

echo "Host setup completed successfully" >&2
echo "Repository: ${WORK_DIR}" >&2
echo "Benchmark directory: ${BENCH_DIR}" >&2

if [[ ${AUTO_RUN} -eq 1 ]]; then
  echo "Launching benchmark runner..." >&2
  cd "${WORK_DIR}"
  "${RUNNER}" --dir "${BENCH_DIR}" "${PASSTHROUGH_ARGS[@]}"
fi
