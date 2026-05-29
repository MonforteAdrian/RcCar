# Bluetooth development plan

This plan keeps Bluetooth work staged so we can make a stack decision before changing the firmware too much.

## Current state

- The car firmware targets the BBC micro:bit v2 / nRF52833 with Embassy.
- The current control path is IR remote input -> Embassy channels -> motor and LED tasks.
- `Cargo.toml` does not include a BLE stack yet.
- `memory.x` currently uses the whole nRF52833 flash/RAM; a SoftDevice-based BLE stack needs a reserved flash/RAM region.
- The servo task currently runs a demo sweep instead of receiving commands.
- Before BLE work, fix the baseline embedded imports so the firmware checks cleanly: `defmt::info`, `defmt_rtt`, and `panic_probe`.

## BLE stack options

| Option | Pros | Cons | When to choose |
|---|---|---|---|
| `nrf-softdevice` + S140 | Best documented for nRF52833 + Embassy, upstream micro:bit v2 example path, mature GATT server macros | Requires flashing Nordic S140 separately, reserves about 156 KiB flash and 31 KiB RAM, needs interrupt/critical-section care | Recommended first path |
| `trouble` + `nrf-sdc` | More Rust BLE host code, avoids full SoftDevice app offset model | Larger dependency migration, newer stack, likely more integration time | Choose if avoiding full SoftDevice is a priority |
| External BLE module over UART/I2C | Keeps nRF firmware simpler | Extra hardware and wiring, not using built-in micro:bit BLE | Only if built-in BLE becomes blocked |

## Recommended first path

Use `nrf-softdevice` + S140 first. It is the shortest path to a working BLE GATT peripheral on the nRF52833 and fits the existing Embassy task model.

Do not remove the IR remote path. BLE should become another command source that writes into the same channels.

## First implementation options

### Option A: Minimal MVP

One custom GATT service with one write-only `command` characteristic.

Command bytes:

| Byte | Action |
|---:|---|
| `0x00` | Stop motors |
| `0x01` | Forward |
| `0x02` | Backward |
| `0x03` | Left |
| `0x04` | Right |
| `0x05` | Toggle front LEDs |

This is fastest to test with nRF Connect, but it mixes unrelated actions into one characteristic.

### Option B: Structured first slice

One custom GATT service with separate write-only characteristics:

| Characteristic | Values |
|---|---|
| `motor` | `0x00` stop, `0x01` forward, `0x02` backward, `0x03` left, `0x04` right |
| `servo` | `0x00` right, `0x01` right-front, `0x02` front, `0x03` left-front, `0x04` left |
| `big_led` | `0x01` toggle |

This is cleaner and still small. It is the preferred first implementation if we include servo work.

### Option C: Full controller profile

Separate characteristics for motors, servo, front LEDs, bottom LEDs, and status notifications.

This is the best long-term API, but it should wait until basic BLE advertising, connection, and command writes are proven.

## Implementation phases

### Phase 1: Baseline cleanup

Goal: make the current firmware buildable before BLE changes.

- Import `defmt::info` where `info!` is used.
- Add `use defmt_rtt as _;`.
- Add `use panic_probe as _;`.
- Run `cargo check --release`.

### Phase 2: BLE stack wiring

Goal: link and boot with SoftDevice.

- Add `nrf-softdevice` with features:
  - `nrf52833`
  - `s140`
  - `ble-peripheral`
  - `ble-gatt-server`
  - `critical-section-impl`
  - `defmt`
- Add `nrf-softdevice-s140`.
- Remove `critical-section-single-core` from `cortex-m`.
- Update `memory.x`:

```ld
MEMORY
{
  FLASH : ORIGIN = 0x00000000 + 156K, LENGTH = 512K - 156K
  RAM   : ORIGIN = 0x200024a0, LENGTH = 0x1db60
}
```

- Set Embassy interrupt priorities before `embassy_nrf::init`:

```rust
config.gpiote_interrupt_priority = embassy_nrf::interrupt::Priority::P2;
config.time_interrupt_priority = embassy_nrf::interrupt::Priority::P2;
```

### Phase 3: BLE task

Goal: advertise as `RcCar` and accept one BLE central connection.

- Add a `bluetooth` module.
- Create `enable_softdevice()`.
- Spawn a `softdevice_task(sd)` that runs `sd.run().await`.
- Spawn a BLE task that:
  - builds the GATT server,
  - advertises,
  - waits for a connection,
  - handles GATT write events,
  - resumes advertising after disconnect.

### Phase 4: Command routing

Goal: BLE writes control the existing car tasks.

- Map motor writes into `MOTORS_CHANNEL.try_send(...)`.
- Map servo writes into `SERVO_CHANNEL.try_send(...)`.
- Map front LED writes into `BIG_LEDS_CHANNEL.try_send(...)`.
- Change the servo task to wait for `ServoCommand` instead of running the demo sweep.
- On BLE disconnect, send `MotorCommand::Stop` and return the servo to the front position.

### Phase 5: Controller/testing

Goal: verify behavior before building a custom app.

- Flash S140 once with `probe-rs`.
- Flash the app with `cargo run --release`.
- Use nRF Connect on a phone to connect to `RcCar`.
- Write `01` then `00` to verify forward and stop.
- Later, use Python `bleak` or Web Bluetooth for a real controller UI.

BLE UUIDs:

| Characteristic | UUID |
|---|---|
| Service | `a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e500` |
| Motor | `a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e501` |
| Servo | `a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e502` |
| Front LEDs | `a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e503` |
| Bottom LEDs | `a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e504` |

The custom characteristics include Bluetooth `0x2901` Characteristic User Description
descriptors so clients can read labels:

| Characteristic | User description |
|---|---|
| Motor | `Motor` |
| Servo | `Servo` |
| Front LEDs | `Front LEDs` |
| Bottom LEDs | `Bottom LEDs` |

### Linux `bluetoothctl` workflow

Use this flow after flashing new firmware. If the GATT table changed, remove the
cached device first so BlueZ discovers the latest services and descriptors.

```bash
bluetoothctl
disconnect EF:0C:F2:A2:D0:11
remove EF:0C:F2:A2:D0:11
scan on
connect EF:0C:F2:A2:D0:11
menu gatt
list-attributes
```

Write commands by selecting the characteristic UUID and then writing one byte.

Motor:

```text
select-attribute a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e501
write 0x00   # stop
write 0x01   # forward
write 0x02   # backward
write 0x03   # left
write 0x04   # right
```

Servo:

```text
select-attribute a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e502
write 0x00   # right
write 0x01   # right-front
write 0x02   # front
write 0x03   # left-front
write 0x04   # left
```

Front LEDs:

```text
select-attribute a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e503
write 0x01   # toggle
```

Bottom LEDs:

```text
select-attribute a1a20000-b1b2-c1c2-d1d2-e1e2e3e4e504
write 0x00   # off
write 0x01   # dim green on
```

To read the human-friendly label for a custom characteristic, select its
`00002901-0000-1000-8000-00805f9b34fb` descriptor path from `list-attributes`
and run:

```text
read
```

### Rust BLE controller

The repository includes Linux host tools at `tools/ble-controller`.

Easy install:

```bash
cd tools/ble-controller
./install-service.sh
```

After that, use simple commands:

```bash
rc-car-command forward
rc-car-command stop
rc-car-command commands
```

Full usage, service management, and development commands are documented in
`tools/ble-controller/README.md`.

## Risk checklist

| Risk | Mitigation |
|---|---|
| App overwrites SoftDevice | Update `memory.x` before flashing the BLE app |
| SoftDevice interrupt conflict | Set Embassy GPIOTE/time priorities to `P2` |
| Critical-section conflict | Remove `critical-section-single-core`; use SoftDevice critical-section implementation |
| Missing S140 on chip | Flash S140 separately after chip erase |
| Servo behavior changes | Decide explicitly whether servo becomes command-driven in the first slice |
| BLE disconnect while motors run | Send `MotorCommand::Stop` on disconnect |
| No controller app yet | Start with nRF Connect raw writes |

## Suggested decision

Start with **Option B without bottom LEDs**:

1. `nrf-softdevice` + S140.
2. Separate `motor` and `big_led` characteristics.
3. Add `servo` only if we are ready to replace the demo sweep with command-driven behavior.
4. Leave bottom LEDs for a later pass because that task is not fully wired yet.
