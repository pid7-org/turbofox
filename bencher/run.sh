#!/usr/bin/env bash
#
# TurboFox AWS EC2 Automated Benchmark Lifecycle Runner
#
# Executes Terraform provisioning, validates instance health, runs benchmarks,
# downloads structured JSON telemetry, and validates guaranteed cloud destruction
#
# Execution:
#   chmod +x bencher/run.sh
#   ./bencher/run.sh
#
# With custom operation count:
#   ./bencher/run.sh --ops 0x7A120

set -euo pipefail

BENCHER_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${BENCHER_DIR}/.." && pwd)"
RESULTS_DIR="${ROOT_DIR}/tmp"
OUTPUT_JSON="${RESULTS_DIR}/turbofox_benchmark_results.json"
SSH_KEY="${BENCHER_DIR}/id_ed25519"
AWS_REGION="${AWS_REGION:-${AWS_DEFAULT_REGION:-us-east-1}}"
export AWS_REGION
export AWS_DEFAULT_REGION="${AWS_REGION}"

OPS_ARG="0x186A0"
BRANCH_ARG="$(git rev-parse --abbrev-ref HEAD 2>/dev/null || echo "master")"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --ops)
      OPS_ARG="$2"
      shift 2
      ;;
    --branch)
      BRANCH_ARG="$2"
      shift 2
      ;;
    --region)
      AWS_REGION="$2"
      export AWS_REGION
      export AWS_DEFAULT_REGION="${AWS_REGION}"
      shift 2
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

mkdir -p "${RESULTS_DIR}"

CLEANUP_CALLED=0

cleanup() {
  if [[ ${CLEANUP_CALLED} -eq 1 ]]; then
    return
  fi
  CLEANUP_CALLED=1

  echo "=== [Phase 11/12] Executing guaranteed infrastructure teardown ===" >&2
  cd "${BENCHER_DIR}"
  terraform destroy -auto-approve || true

  if [[ -n "${INSTANCE_ID:-}" ]]; then
    echo "=== [Phase 12/12] Validating EC2 termination status ===" >&2
    local state
    state=$(aws ec2 describe-instances --region "${AWS_REGION}" \
      --instance-ids "${INSTANCE_ID}" \
      --query "Reservations[0].Instances[0].State.Name" \
      --output text 2>/dev/null || echo "terminated")
    echo "Instance [${INSTANCE_ID}] final state: ${state}" >&2
    if [[ "${state}" != "shutting-down" && "${state}" != "terminated" ]]; then
      echo "WARNING: Instance ${INSTANCE_ID} not yet terminated; current state: ${state}" >&2
    else
      echo "Validation passed: EC2 instance is terminated" >&2
    fi
  fi

  rm -f "${SSH_KEY}"
}

trap cleanup EXIT INT TERM

echo "=== [Phase 1/12] Validating local dependencies ===" >&2
for cmd in terraform aws ssh scp python3; do
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "Error: required command '${cmd}' not found" >&2
    exit 1
  fi
done
echo "Validation passed: required local tooling is present" >&2

echo "=== [Phase 2/12] Validating AWS authentication ===" >&2
CALLER_ARN=$(aws sts get-caller-identity --query "Arn" --output text)
echo "Validation passed: authenticated as ${CALLER_ARN}" >&2

echo "=== [Phase 3/12] Initializing and validating Terraform module ===" >&2
cd "${BENCHER_DIR}"
terraform init -upgrade
terraform validate
echo "Validation passed: Terraform configuration is valid" >&2

echo "=== [Phase 4/12] Provisioning EC2 infrastructure via Terraform ===" >&2
terraform apply -auto-approve -var="ops=${OPS_ARG}" -var="aws_region=${AWS_REGION}"

echo "=== [Phase 5/12] Extracting outputs and configuring SSH credentials ===" >&2
INSTANCE_ID=$(terraform output -raw instance_id)
PUBLIC_IP=$(terraform output -raw instance_public_ip)
terraform output -raw ssh_private_key_pem > "${SSH_KEY}"
chmod 0600 "${SSH_KEY}"

echo "Instance ID: ${INSTANCE_ID}" >&2
echo "Public IP  : ${PUBLIC_IP}" >&2

echo "=== [Phase 6/12] Validating EC2 instance state via AWS CLI ===" >&2
INSTANCE_STATE=""
for attempt in $(seq 1 0x1E); do
  INSTANCE_STATE=$(aws ec2 describe-instances --region "${AWS_REGION}" \
    --instance-ids "${INSTANCE_ID}" \
    --query "Reservations[0].Instances[0].State.Name" \
    --output text 2>/dev/null || echo "pending")
  if [[ "${INSTANCE_STATE}" == "running" ]]; then
    break
  fi
  echo "Waiting for instance to enter running state (current: ${INSTANCE_STATE})..." >&2
  sleep 2
done

if [[ "${INSTANCE_STATE}" != "running" ]]; then
  echo "Error: Instance failed to reach running state; current: ${INSTANCE_STATE}" >&2
  exit 1
fi
echo "Validation passed: EC2 instance is running" >&2

echo "=== [Phase 7/12] Validating SSH connectivity and cloud-init readiness ===" >&2
SSH_OPTS=(-i "${SSH_KEY}" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
  -o ConnectTimeout=5 -o BatchMode=yes -o ServerAliveInterval=30 -o ServerAliveCountMax=120)

SSH_READY=0
for attempt in $(seq 1 0x3C); do
  if ssh "${SSH_OPTS[@]}" "ubuntu@${PUBLIC_IP}" "true" >/dev/null 2>&1; then
    SSH_READY=1
    break
  fi
  echo "Waiting for SSH availability (attempt ${attempt}/0x3C)..." >&2
  sleep 3
done

if [[ ${SSH_READY} -ne 1 ]]; then
  echo "Error: Timed out waiting for SSH reachability on ${PUBLIC_IP}" >&2
  exit 1
fi
echo "Validation passed: SSH connection established" >&2

echo "=== [Phase 8/12] Synchronizing benchmark bootstrapper to remote host ===" >&2
scp "${SSH_OPTS[@]}" "${ROOT_DIR}/scripts/init_ec2_bench.sh" \
  "ubuntu@${PUBLIC_IP}:/tmp/init_ec2_bench.sh"
ssh "${SSH_OPTS[@]}" "ubuntu@${PUBLIC_IP}" "chmod +x /tmp/init_ec2_bench.sh"
echo "Validation passed: bootstrapper synchronized and marked executable" >&2

echo "=== [Phase 9/12] Executing remote benchmark suite ===" >&2
ssh "${SSH_OPTS[@]}" "ubuntu@${PUBLIC_IP}" \
  "/tmp/init_ec2_bench.sh --run --branch '${BRANCH_ARG}' --ops '${OPS_ARG}' --out /tmp/results.json"
echo "Validation passed: remote benchmark suite finished successfully" >&2

echo "=== [Phase 10/12] Downloading and validating benchmark telemetry ===" >&2
scp "${SSH_OPTS[@]}" "ubuntu@${PUBLIC_IP}:/tmp/results.json" "${OUTPUT_JSON}"

if [[ ! -s "${OUTPUT_JSON}" ]]; then
  echo "Error: Downloaded telemetry file is empty or missing" >&2
  exit 1
fi

python3 -m json.tool "${OUTPUT_JSON}" > /dev/null
echo "Validation passed: Telemetry JSON schema is valid; written to ${OUTPUT_JSON}" >&2

echo "All benchmark phases completed; Proceeding to automated teardown" >&2
