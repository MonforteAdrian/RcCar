#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

socket_path="${RC_CAR_SOCKET:-/tmp/rc-ble-controller.sock}"

echo "Starting RcCar BLE service on $socket_path"
echo "In another terminal, run: ./command.sh forward"
cargo run -- --service --socket "$socket_path"
