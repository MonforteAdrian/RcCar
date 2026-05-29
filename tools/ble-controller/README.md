# RcCar BLE controller

Linux host tools for controlling the RcCar over BLE.

The recommended setup is:

1. Run `rc-ble-controller` as a service. It keeps one BLE connection open to the car.
2. Use `rc-car-command` from any terminal or program to send commands.

## Quick install

From this directory:

```bash
./install-service.sh
```

That builds and installs:

- `/usr/local/bin/rc-ble-controller`
- `/usr/local/bin/rc-car-command`
- `/etc/systemd/system/rc-ble-controller.service`

The installer also creates the `rc-car` system user/group, adds the service user
to the `bluetooth` group, and adds the installing user to the `rc-car` group for
socket access. If `rc-car-command` gets a socket permission error immediately
after install, open a new login session so the new group membership is active.

Then use:

```bash
rc-car-command commands
rc-car-command status
rc-car-command forward
rc-car-command drive 40 -15
rc-car-command stop
rc-car-command servo-left
rc-car-command bottom-on
```

## Commands

```text
forward
backward
stop
left
right
servo-left
servo-left-front
servo-front
servo-right-front
servo-right
front-toggle
bottom-on
bottom-off
drive <throttle -100..100> <steering -100..100>
throttle <value -100..100>
steer <value -100..100>
drive gamepad
drive chronos --device <path>
chronos-probe --device <path>
status
restart
logs
install
```

Show commands any time:

```bash
rc-car-command commands
```

Check service and BLE connection state:

```bash
rc-car-command status
```

## Auto-stop safety

In service mode, `rc-ble-controller` automatically sends `motor stop` after 3
seconds without active drive/throttle commands. A manual `rc-car-command stop` or
`rc-car-command drive 0 0` also clears the pending auto-stop timer.

To change the timeout in a foreground development run:

```bash
cargo run -- --service --auto-stop-seconds 5
```

To disable it:

```bash
cargo run -- --service --auto-stop-seconds 0
```

## Analog driving

Analog commands use signed percentages:

- `throttle`: `-100` full reverse, `0` stop, `100` full forward.
- `steering`: `-100` turn left, `0` straight, `100` turn right.

`drive <throttle> <steering>` mixes throttle and steering into the wheel motors.
The separate `steer <value>` command moves the servo used by the ultrasonic
sensor.

The firmware applies a 15% analog motor deadband after throttle/steering mixing:
motor outputs at or below 15% are treated as stop to avoid low-power buzzing.

One-shot examples:

```bash
rc-car-command drive 35 -20
rc-car-command throttle 0
rc-car-command steer 15
```

## Gamepad drive mode

Use a USB or Bluetooth controller recognized by Linux:

```bash
rc-car-command drive gamepad
rc-car-command drive gamepad --deadzone 10 --rate-hz 20
rc-car-command drive gamepad --device DualSense
```

Default mapping:

- Left stick up/down controls throttle.
- Left stick left/right controls steering.
- South button toggles the front LEDs.
- East button turns the bottom LEDs on.
- West button turns the bottom LEDs off.
- Start/Mode sends neutral (`drive 0 0`).

If an axis is reversed for your controller, use `--invert-throttle` or
`--invert-steering`. Press Ctrl-C to exit; drive mode sends neutral before it
stops.

Gamepad access usually requires the user running `rc-car-command` to have
permission to read `/dev/input/event*`. Depending on the distribution, that may
mean adding the user to the `input` group or installing a udev rule. During early
testing, USB is often simpler than sharing one Bluetooth adapter between the car
BLE connection and the gamepad.

## Chronos receiver support

The eZ430-Chronos RF access point appears as a serial device such as
`/dev/ttyACM0` or a stable `/dev/serial/by-id/...eZ430-ChronosAP...` symlink.
On many Linux systems the device is owned by the `uucp` group, so use a fresh
login session after adding your user to that group.

Start by polling the access point and confirming watch packets are received:

```bash
rc-car-command chronos-probe --device /dev/ttyACM0 --limit 128
```

Use `#` on the watch to select `ACC` on the lower display, then hold the
bottom-right Down button while probing or driving. The RF icon appears only
while this button is held. The probe prints decoded accelerometer samples and
the drive command derived from them. In drive mode, releasing the bottom-right
button sends neutral after about one second without packets. Drive mode filters
tiny watch jitter and limits meaningful Chronos updates before forwarding them
to the BLE service.

Before driving, confirm the watch orientation and axis direction:

```bash
rc-car-command chronos-calibrate --device /dev/ttyACM0
```

The calibration command does not send anything to the car. It asks you to hold
the watch level, then tilt it forward, backward, left, and right while holding
the bottom-right Down button. The expected mapping is:

| Watch movement | Car command |
| --- | --- |
| Level | Neutral |
| Tilt forward | Positive throttle / forward |
| Tilt backward | Negative throttle / reverse |
| Tilt left | Negative steering / left |
| Tilt right | Positive steering / right |

Calibration should report `ok` for all five positions before driving. The
default Chronos mapping uses the calibrated orientation where forward/backward
tilt changes throttle and left/right tilt changes steering.

If calibration reports a reversed axis, pass `--invert-throttle` or
`--invert-steering` to `drive chronos`. If neutral is outside the deadzone, hold
the watch flatter or increase `--deadzone`.

Run it with:

```bash
rc-car-command drive chronos --device /dev/ttyACM0
```

The default Chronos AP baud rate is 115200. Add `--raw` to `chronos-probe` only
when debugging a non-standard receiver or firmware that does not use the
standard Chronos access-point protocol.

## Service management

Check whether the service is running:

```bash
rc-car-command status
```

Watch logs:

```bash
rc-car-command logs
```

Restart:

```bash
rc-car-command restart
```

Rebuild, reinstall, and restart the service from this checkout:

```bash
rc-car-command install
```

## Service security

The installed service runs as the dedicated `rc-car` user, not as root. Its
socket is created at `/run/rc-ble-controller/rc-ble-controller.sock` with mode
`0660`, so only the service account and users in the `rc-car` group can send
control commands.

## Test without installing

Terminal 1:

```bash
./run-service.sh
```

Terminal 2:

```bash
./command.sh commands
./command.sh forward
./command.sh stop
```

## Direct interactive mode

You can also run the controller directly without the service:

```bash
cargo run --
```

This opens an interactive prompt. Enter a number or a command, for example:

```text
rc> 2
rc> servo left
rc> bottom-led on
rc> quit
```

## Direct one-shot mode

```bash
cargo run -- motor forward
cargo run -- motor stop
cargo run -- servo left
```

If multiple `RcCar` devices are nearby, target the known address:

```bash
cargo run -- --addr EF:0C:F2:A2:D0:11 servo right-front
```

## How it works

- `rc-ble-controller` connects to the BLE peripheral named `RcCar`.
- In service mode, it listens on `/run/rc-ble-controller/rc-ble-controller.sock`.
- `rc-car-command` sends one-shot or streaming plain text commands to that socket.
- The service translates the text command into the correct BLE GATT write.
