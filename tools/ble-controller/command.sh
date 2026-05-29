#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

socket_path="${RC_CAR_SOCKET:-/tmp/rc-ble-controller.sock}"

cargo run --bin rc-car-command -- --socket "$socket_path" "$@"
