#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

target="${TARGET:-x86_64-unknown-linux-gnu}"
service_user="rc-car"
service_group="rc-car"
bluetooth_group="bluetooth"
install_user="${SUDO_USER:-${USER:-}}"

echo "Building rc-ble-controller and rc-car-command..."
cargo build --release --target "$target"

echo "Creating service account and control group..."
if ! getent group "$service_group" >/dev/null; then
  sudo groupadd --system "$service_group"
fi

if ! id -u "$service_user" >/dev/null 2>&1; then
  nologin_shell="/usr/sbin/nologin"
  if [[ ! -x "$nologin_shell" ]]; then
    nologin_shell="/bin/false"
  fi
  sudo useradd --system --no-create-home --gid "$service_group" --home-dir /nonexistent --shell "$nologin_shell" "$service_user"
fi

if ! getent group "$bluetooth_group" >/dev/null; then
  sudo groupadd --system "$bluetooth_group"
fi
sudo usermod -a -G "$bluetooth_group" "$service_user"

added_install_user=""
if [[ -n "$install_user" && "$install_user" != "root" ]] && id "$install_user" >/dev/null 2>&1; then
  sudo usermod -a -G "$service_group" "$install_user"
  added_install_user="$install_user"
fi

echo "Installing binaries to /usr/local/bin..."
sudo install -m 755 "target/$target/release/rc-ble-controller" /usr/local/bin/rc-ble-controller
sudo install -m 755 "target/$target/release/rc-car-command" /usr/local/bin/rc-car-command

echo "Installing and restarting systemd service..."
sudo install -m 644 systemd/rc-ble-controller.service /etc/systemd/system/rc-ble-controller.service
sudo rm -f /run/rc-ble-controller.sock
sudo systemctl daemon-reload
sudo systemctl enable rc-ble-controller.service >/dev/null
sudo systemctl restart rc-ble-controller.service

echo
echo "RcCar service is installed."
echo "Service user:  $service_user"
echo "Control group: $service_group"
if [[ -n "$added_install_user" ]]; then
  echo "Added $added_install_user to $service_group; open a new login session if socket access is denied."
fi
echo "Show commands: rc-car-command commands"
echo "Try command:   rc-car-command forward"
echo "Stop command:  rc-car-command stop"
echo "Service logs:  journalctl -u rc-ble-controller.service -f"
