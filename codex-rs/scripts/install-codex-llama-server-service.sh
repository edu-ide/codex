#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVICE_NAME="codex-llama-server.service"
SERVICE_FILE="${SCRIPT_DIR}/systemd/${SERVICE_NAME}"
ENV_TEMPLATE="${SCRIPT_DIR}/systemd/${SERVICE_NAME%.service}.env"

SYSTEMD_DIR="/etc/systemd/system"
ENV_FILE="/etc/default/${SERVICE_NAME%.service}"
ENABLE_SERVICE=0
START_SERVICE=0
OVERWRITE=0
DRY_RUN=0

usage() {
  cat <<'USAGE'
Usage: install-codex-llama-server-service.sh [options]

Install the systemd service for the codex remote-control daemon.

Options:
  --systemd-dir PATH       Destination systemd directory (default: /etc/systemd/system)
  --env-file PATH          Destination env file path (default: /etc/default/codex-llama-server)
  --env-template PATH      Optional custom env template path to install
                          (default: systemd/codex-llama-server.env)
  --enable                 Enable service on boot
  --start                  Start/restart service after installation
  --overwrite              Overwrite existing unit/env files
  --dry-run                Print actions without applying
  -h, --help               Show this help
USAGE
}

run_cmd() {
  if (( DRY_RUN )); then
    echo "+ $*"
    return 0
  fi
  "$@"
}

while (("$#")); do
  case "$1" in
    --systemd-dir)
      SYSTEMD_DIR="${2:?missing systemd-dir path}"
      shift 2
      ;;
    --env-file)
      ENV_FILE="${2:?missing env file path}"
      shift 2
      ;;
    --env-template)
      ENV_TEMPLATE="${2:?missing env template path}"
      shift 2
      ;;
    --enable)
      ENABLE_SERVICE=1
      shift
      ;;
    --start)
      START_SERVICE=1
      shift
      ;;
    --overwrite)
      OVERWRITE=1
      shift
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if (( EUID != 0 )); then
  echo "This installer writes to system directories. Run as root or with sudo." >&2
  exit 1
fi

if [[ ! -f "$SERVICE_FILE" ]]; then
  echo "Missing service template: $SERVICE_FILE" >&2
  exit 1
fi

if [[ ! -f "$ENV_TEMPLATE" ]]; then
  echo "Missing env template: $ENV_TEMPLATE" >&2
  exit 1
fi

TARGET_UNIT="${SYSTEMD_DIR%/}/$SERVICE_NAME"

install_file() {
  local src="$1"
  local dst="$2"
  local label="$3"
  if [[ -e "$dst" && "$OVERWRITE" != 1 ]]; then
    if [[ "$label" == "environment file" ]]; then
      echo "${label} already exists and --overwrite is not set: $dst"
    else
      echo "${label} already exists and --overwrite is not set: $dst"
    fi
    return
  fi
  run_cmd install -Dm0644 "$src" "$dst"
}

install_file "$SERVICE_FILE" "$TARGET_UNIT" "service unit"
install_file "$ENV_TEMPLATE" "$ENV_FILE" "environment file"

run_cmd systemctl daemon-reload

if (( ENABLE_SERVICE )); then
  run_cmd systemctl enable "$SERVICE_NAME"
fi

if (( START_SERVICE )); then
  run_cmd systemctl restart "$SERVICE_NAME"
fi
