#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVICE_NAME="ilhae-runtime-proxy.service"
SERVICE_FILE="${SCRIPT_DIR}/systemd/${SERVICE_NAME}"
ENV_TEMPLATE="${SCRIPT_DIR}/systemd/${SERVICE_NAME%.service}.env"
BINARY="${SCRIPT_DIR}/../target/debug/ilhae-runtime-proxy"
SYSTEMD_DIR="/etc/systemd/system"
ENV_FILE="/etc/default/${SERVICE_NAME%.service}"
BINARY_DESTINATION="/usr/local/bin/ilhae-runtime-proxy"
SERVICE_USER="${SUDO_USER:-${USER:-root}}"
ENABLE_SERVICE=0
START_SERVICE=0
OVERWRITE=0
OVERWRITE_ENV=0
DRY_RUN=0

usage() {
  cat <<'USAGE'
Usage: install-ilhae-runtime-proxy-service.sh [options]

Install the authenticated Ilhae runtime controller and streaming proxy.

Options:
  --binary PATH            Built ilhae-runtime-proxy binary
  --binary-destination PATH
                           Installed binary path (default: /usr/local/bin/ilhae-runtime-proxy)
  --systemd-dir PATH       Destination systemd directory (default: /etc/systemd/system)
  --env-file PATH          Destination env file (default: /etc/default/ilhae-runtime-proxy)
  --env-template PATH      Environment template to install
  --service-user USER      Account that owns the runtime and config
                           (default: invoking user)
  --enable                 Enable service on boot
  --start                  Start/restart service after installation
  --overwrite              Overwrite existing unit and binary
  --overwrite-env          Also overwrite an existing env file and its secret
  --dry-run                Print actions without applying them
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
    --binary)
      BINARY="${2:?missing binary path}"
      shift 2
      ;;
    --binary-destination)
      BINARY_DESTINATION="${2:?missing binary destination path}"
      shift 2
      ;;
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
    --service-user)
      SERVICE_USER="${2:?missing service user}"
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
    --overwrite-env)
      OVERWRITE_ENV=1
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

for required in "$SERVICE_FILE" "$ENV_TEMPLATE" "$BINARY"; do
  if [[ ! -f "$required" ]]; then
    echo "Missing required file: $required" >&2
    exit 1
  fi
done

SERVICE_ENTRY="$(getent passwd "$SERVICE_USER" || true)"
if [[ -z "$SERVICE_ENTRY" ]]; then
  echo "Unknown service user: $SERVICE_USER" >&2
  exit 1
fi
if [[ ! "$SERVICE_USER" =~ ^[a-zA-Z_][a-zA-Z0-9_-]*[$]?$ ]]; then
  echo "Unsupported service user name: $SERVICE_USER" >&2
  exit 1
fi
SERVICE_HOME="$(cut -d: -f6 <<<"$SERVICE_ENTRY")"
SERVICE_GROUP="$(id -gn "$SERVICE_USER")"
if [[ -z "$SERVICE_HOME" || ! -d "$SERVICE_HOME" ]]; then
  echo "Service home does not exist for $SERVICE_USER: $SERVICE_HOME" >&2
  exit 1
fi

escape_sed_replacement() {
  sed 's/[&|\\]/\\&/g' <<<"$1"
}

RENDERED_SERVICE="$(mktemp)"
RENDERED_ENV="$(mktemp)"
trap 'rm -f "$RENDERED_SERVICE" "$RENDERED_ENV"' EXIT
sed \
  -e "s|@ILHAE_RUNTIME_PROXY_USER@|$(escape_sed_replacement "$SERVICE_USER")|g" \
  -e "s|@ILHAE_RUNTIME_PROXY_GROUP@|$(escape_sed_replacement "$SERVICE_GROUP")|g" \
  -e "s|@ILHAE_RUNTIME_PROXY_HOME@|$(escape_sed_replacement "$SERVICE_HOME")|g" \
  -e "s|@ILHAE_RUNTIME_PROXY_BINARY@|$(escape_sed_replacement "$BINARY_DESTINATION")|g" \
  "$SERVICE_FILE" >"$RENDERED_SERVICE"
RUNTIME_PROXY_TOKEN="$(od -An -N32 -tx1 /dev/urandom | tr -d ' \n')"
if [[ ! "$RUNTIME_PROXY_TOKEN" =~ ^[[:xdigit:]]{64}$ ]]; then
  echo "Failed to generate a runtime proxy token" >&2
  exit 1
fi
sed \
  -e "s|@ILHAE_RUNTIME_PROXY_TOKEN@|$(escape_sed_replacement "$RUNTIME_PROXY_TOKEN")|g" \
  "$ENV_TEMPLATE" >"$RENDERED_ENV"

install_file() {
  local source="$1"
  local destination="$2"
  local mode="$3"
  if [[ -e "$destination" && "$OVERWRITE" != 1 ]]; then
    echo "Destination exists and --overwrite is not set: $destination"
    return
  fi
  run_cmd install -Dm"$mode" "$source" "$destination"
}

install_file "$BINARY" "$BINARY_DESTINATION" 0755
install_file "$RENDERED_SERVICE" "${SYSTEMD_DIR%/}/$SERVICE_NAME" 0644
if [[ -e "$ENV_FILE" && "$OVERWRITE_ENV" != 1 ]]; then
  echo "Preserving existing environment file: $ENV_FILE"
else
  run_cmd install -Dm0600 "$RENDERED_ENV" "$ENV_FILE"
fi
run_cmd systemctl daemon-reload

if (( ENABLE_SERVICE )); then
  run_cmd systemctl enable "$SERVICE_NAME"
fi
if (( START_SERVICE )); then
  run_cmd systemctl restart "$SERVICE_NAME"
fi
