# RcCar

Rust firmware and Linux control tools for an RC car built around the BBC micro:bit v2 / nRF52833.

The firmware uses Embassy and Nordic SoftDevice S140 to expose a BLE peripheral named `RcCar`. BLE writes are routed into the same command channels as the existing firmware tasks, so the car can be controlled from Linux without removing the existing embedded control paths.

## Repository layout

| Path | Purpose |
|---|---|
| `src/` | Embedded firmware for the nRF52833 |
| `memory.x` | Linker memory layout for S140 SoftDevice + app |
| `docs/bluetooth-development-plan.md` | BLE implementation notes, UUIDs, and manual `bluetoothctl` workflow |
| `tools/ble-controller/` | Linux Rust controller, service, and `rc-car-command` helper |

## Hardware and firmware target

- Board/chip: BBC micro:bit v2 / nRF52833
- Rust target: `thumbv7em-none-eabihf`
- Runner: `probe-rs run --chip nRF52833_xxAA`
- BLE stack: `nrf-softdevice` with Nordic S140
- Advertised BLE name: `RcCar`

The app is linked after S140:

```ld
FLASH : ORIGIN = 0x00000000 + 156K, LENGTH = 512K - 156K
RAM   : ORIGIN = 0x200024a0, LENGTH = 0x1db60
```

## Build and flash firmware

Check the firmware:

```bash
cargo check --release
```

Flash the app:

```bash
cargo run --release
```

After a full chip erase, flash S140 before flashing the app:

```bash
probe-rs erase --chip nRF52833_xxAA --allow-erase-all
probe-rs download --chip nRF52833_xxAA --verify --binary-format hex /path/to/s140_nrf52_7.3.0_softdevice.hex
cargo run --release
```

## BLE GATT API

Custom service:

```text
a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e500
```

| Characteristic | UUID | Commands |
|---|---|---|
| Motor | `a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e501` | `0x00` stop, `0x01` forward, `0x02` backward, `0x03` left, `0x04` right |
| Servo | `a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e502` | `0x00` right, `0x01` right-front, `0x02` front, `0x03` left-front, `0x04` left |
| Front LEDs | `a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e503` | `0x01` toggle |
| Bottom LEDs | `a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e504` | `0x00` off, `0x01` dim green on |

Each custom characteristic also exposes a `0x2901` Characteristic User Description descriptor with a readable label.

## Linux controller

The easiest control path is the Linux service and command helper in `tools/ble-controller`.

Install and start the service:

```bash
cd tools/ble-controller
./install-service.sh
```

Then control the car from any terminal or program:

```bash
rc-car-command commands
rc-car-command status
rc-car-command forward
rc-car-command stop
rc-car-command servo-left
rc-car-command bottom-on
```

The service keeps one BLE connection open and automatically sends `motor stop` after 3 seconds without commands.
It runs as the dedicated `rc-car` system user and exposes a `0660` Unix socket for members of the `rc-car` group.

See `tools/ble-controller/README.md` for service management, foreground testing, and direct interactive mode.

## Manual Linux BLE testing

If the GATT table changed, refresh BlueZ first:

```bash
bluetoothctl
disconnect EF:0C:F2:A2:D0:11
remove EF:0C:F2:A2:D0:11
scan on
connect EF:0C:F2:A2:D0:11
menu gatt
list-attributes
```

Then select a characteristic and write a byte:

```text
select-attribute a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e502
write 0x02
```

More manual BLE examples are in `docs/bluetooth-development-plan.md`.

## Current known gap

BLE, the Linux service, servo control, LEDs, and I2C motor commands are wired. If motors still do not move, the next investigation is hardware-side: motor battery/power switch, driver enable/sleep, wiring, or motor-board channel/value mapping.
