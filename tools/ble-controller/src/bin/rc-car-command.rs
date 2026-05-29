use std::{
    env,
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};
use rc_ble_controller::{COMMANDS, ControlCommand, axis_value_to_percent, parse_control_command};
use serialport::{ClearBuffer, SerialPort};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    sync::mpsc,
    time::{interval, timeout},
};

const DEFAULT_SYSTEM_SOCKET: &str = "/run/rc-ble-controller/rc-ble-controller.sock";
const LEGACY_SYSTEM_SOCKET: &str = "/run/rc-ble-controller.sock";
const DEFAULT_TMP_SOCKET: &str = "/tmp/rc-ble-controller.sock";
const SERVICE_NAME: &str = "rc-ble-controller.service";
const SERVICE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const CHRONOS_DRIVE_MIN_DELTA: i8 = 5;
const CHRONOS_DRIVE_MIN_INTERVAL: Duration = Duration::from_millis(150);

#[tokio::main]
async fn main() -> Result<()> {
    let cli = match Cli::parse(env::args().skip(1)) {
        Ok(cli) => cli,
        Err(message) => {
            eprintln!("{message}\n");
            print_usage();
            bail!("invalid arguments");
        }
    };

    if cli.help {
        print_usage();
        return Ok(());
    }

    if cli.command.is_empty() || matches!(cli.command.as_str(), "commands" | "list") {
        print!("{COMMANDS}");
        return Ok(());
    }

    let socket_path = cli.socket_path.unwrap_or_else(detect_socket_path);
    if cli.command == "status" {
        run_status(&socket_path).await?;
        return Ok(());
    }
    if matches!(cli.command.as_str(), "restart" | "service restart") {
        run_restart()?;
        return Ok(());
    }
    if matches!(cli.command.as_str(), "logs" | "service logs") {
        run_logs()?;
        return Ok(());
    }
    if matches!(cli.command.as_str(), "install" | "service install") {
        run_install()?;
        return Ok(());
    }

    if let Some(config) = ChronosProbeConfig::parse(&cli.command).map_err(anyhow::Error::msg)? {
        run_chronos_probe(config)?;
        return Ok(());
    }

    if let Some(config) = ChronosCalibrateConfig::parse(&cli.command).map_err(anyhow::Error::msg)? {
        run_chronos_calibration(config)?;
        return Ok(());
    }

    if let Some(config) = DriveConfig::parse(&cli.command).map_err(anyhow::Error::msg)? {
        run_drive(&socket_path, config).await?;
        return Ok(());
    }

    parse_control_command(&cli.command).map_err(anyhow::Error::msg)?;

    let response = send_socket_command(&socket_path, &cli.command).await?;

    print!("{response}");
    if response.starts_with("ERR ") {
        bail!("command failed");
    }

    Ok(())
}

async fn run_status(socket_path: &Path) -> Result<()> {
    println!("Service unit: {}", service_unit_status());
    println!("Socket path: {}", socket_path.display());
    let socket_file = match socket_path.try_exists() {
        Ok(true) => "present".to_string(),
        Ok(false) => "missing".to_string(),
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            "permission denied; run `newgrp rc-car` or open a new terminal".to_string()
        }
        Err(error) => format!("unknown ({error})"),
    };
    println!("Socket file: {socket_file}");

    match send_socket_command(socket_path, "status").await {
        Ok(response) => {
            println!("Socket connection: ok");
            print!("{response}");
            Ok(())
        }
        Err(error) => {
            println!("Socket connection: failed");
            bail!("service is not reachable: {error:#}");
        }
    }
}

fn run_restart() -> Result<()> {
    let status = Command::new("systemctl")
        .args(["restart", SERVICE_NAME])
        .status()
        .context("restarting RcCar service")?;

    if !status.success() {
        bail!("service restart failed");
    }

    println!("Restarted {SERVICE_NAME}");
    println!("Check it with: rc-car-command status");
    Ok(())
}

fn run_logs() -> Result<()> {
    let status = Command::new("journalctl")
        .args(["-u", SERVICE_NAME, "-f"])
        .status()
        .context("opening RcCar service logs")?;

    if !status.success() {
        bail!("journalctl failed");
    }

    Ok(())
}

fn run_install() -> Result<()> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("install-service.sh");
    if !script.exists() {
        bail!(
            "install script not found at {}. Run ./install-service.sh from tools/ble-controller instead",
            script.display()
        );
    }

    let status = Command::new("bash")
        .arg(&script)
        .status()
        .with_context(|| format!("running {}", script.display()))?;

    if !status.success() {
        bail!("install failed");
    }

    Ok(())
}

async fn send_socket_command(socket_path: &Path, command: &str) -> Result<String> {
    let mut stream = match UnixStream::connect(socket_path).await {
        Ok(stream) => stream,
        Err(error) if error.kind() == ErrorKind::PermissionDenied => {
            bail!(
                "connecting to {}: permission denied. Run `newgrp rc-car` or open a new terminal so your shell has the rc-car group",
                socket_path.display()
            );
        }
        Err(error) => {
            return Err(error).with_context(|| format!("connecting to {}", socket_path.display()));
        }
    };

    stream
        .write_all(command.as_bytes())
        .await
        .context("sending command")?;
    stream.write_all(b"\n").await.context("sending newline")?;
    stream.shutdown().await.context("closing command write")?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .context("reading response")?;

    Ok(response)
}

struct SocketCommandStream {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl SocketCommandStream {
    async fn connect(socket_path: &Path) -> Result<Self> {
        let stream = match UnixStream::connect(socket_path).await {
            Ok(stream) => stream,
            Err(error) if error.kind() == ErrorKind::PermissionDenied => {
                bail!(
                    "connecting to {}: permission denied. Run `newgrp rc-car` or open a new terminal so your shell has the rc-car group",
                    socket_path.display()
                );
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("connecting to {}", socket_path.display()));
            }
        };
        let (reader, writer) = stream.into_split();

        Ok(Self {
            reader: BufReader::new(reader),
            writer,
        })
    }

    async fn send(&mut self, command: &ControlCommand) -> Result<String> {
        let line = command.socket_line();
        self.writer
            .write_all(line.as_bytes())
            .await
            .with_context(|| format!("sending {line}"))?;
        self.writer
            .write_all(b"\n")
            .await
            .context("sending newline")?;

        let mut response = String::new();
        timeout(
            SERVICE_RESPONSE_TIMEOUT,
            self.reader.read_line(&mut response),
        )
        .await
        .context("timed out waiting for service response")?
        .context("reading service response")?;

        if response.starts_with("ERR ") {
            bail!("{}", response.trim_end());
        }

        Ok(response)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DriveInput {
    Gamepad,
    Chronos,
}

struct DriveConfig {
    input: DriveInput,
    device: Option<String>,
    deadzone_percent: u8,
    rate_hz: u64,
    baud_rate: u32,
    invert_throttle: bool,
    invert_steering: bool,
}

impl DriveConfig {
    fn parse(command: &str) -> std::result::Result<Option<Self>, String> {
        if !command
            .split_whitespace()
            .next()
            .is_some_and(|word| word == "drive")
        {
            return Ok(None);
        }
        if parse_control_command(command).is_ok() {
            return Ok(None);
        }

        let mut args = command.split_whitespace().skip(1).peekable();
        let mut config = Self {
            input: DriveInput::Gamepad,
            device: None,
            deadzone_percent: 8,
            rate_hz: 20,
            baud_rate: 115_200,
            invert_throttle: false,
            invert_steering: false,
        };

        while let Some(arg) = args.next() {
            match arg {
                "--input" => config.input = parse_drive_input(next_word(&mut args, "--input")?)?,
                "gamepad" | "controller" => config.input = DriveInput::Gamepad,
                "chronos" => config.input = DriveInput::Chronos,
                "--device" => config.device = Some(next_word(&mut args, "--device")?.to_string()),
                "--deadzone" => {
                    config.deadzone_percent =
                        parse_u8_range(next_word(&mut args, "--deadzone")?, 0, 99, "--deadzone")?;
                }
                "--rate-hz" => {
                    config.rate_hz =
                        parse_u64_range(next_word(&mut args, "--rate-hz")?, 1, 60, "--rate-hz")?;
                }
                "--baud" | "--baud-rate" => {
                    config.baud_rate = next_word(&mut args, "--baud")?
                        .parse()
                        .map_err(|_| "invalid --baud value".to_string())?;
                }
                "--invert-throttle" => config.invert_throttle = true,
                "--invert-steering" => config.invert_steering = true,
                "--help" | "-h" => return Err(drive_usage().to_string()),
                _ => return Err(format!("unknown drive option: {arg}")),
            }
        }

        Ok(Some(config))
    }
}

fn parse_drive_input(input: &str) -> std::result::Result<DriveInput, String> {
    match input {
        "gamepad" | "controller" => Ok(DriveInput::Gamepad),
        "chronos" => Ok(DriveInput::Chronos),
        _ => Err(format!("unknown drive input: {input}")),
    }
}

fn drive_usage() -> &'static str {
    "Usage:
  rc-car-command drive [gamepad] [--device <name-or-id>] [--deadzone 0..99] [--rate-hz 1..60]
  rc-car-command drive chronos --device <path> [--baud 115200] [--invert-throttle] [--invert-steering]

Gamepad buttons:
  South: front LEDs toggle
  East: bottom LEDs on
  West: bottom LEDs off
  Start/Mode: neutral"
}

fn chronos_probe_usage() -> &'static str {
    "Usage:
  rc-car-command chronos-probe --device <path> [--baud 115200] [--limit <polls>]"
}

fn chronos_calibrate_usage() -> &'static str {
    "Usage:
  rc-car-command chronos-calibrate --device <path> [--baud 115200] [--deadzone 0..99] [--samples 1..50] [--invert-throttle] [--invert-steering]"
}

fn next_word<'a>(
    args: &mut impl Iterator<Item = &'a str>,
    option: &str,
) -> std::result::Result<&'a str, String> {
    args.next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn parse_u8_range(value: &str, min: u8, max: u8, option: &str) -> std::result::Result<u8, String> {
    let value = value
        .parse::<u8>()
        .map_err(|_| format!("invalid {option} value"))?;
    if value < min || value > max {
        Err(format!("{option} must be between {min} and {max}"))
    } else {
        Ok(value)
    }
}

fn parse_u64_range(
    value: &str,
    min: u64,
    max: u64,
    option: &str,
) -> std::result::Result<u64, String> {
    let value = value
        .parse::<u64>()
        .map_err(|_| format!("invalid {option} value"))?;
    if value < min || value > max {
        Err(format!("{option} must be between {min} and {max}"))
    } else {
        Ok(value)
    }
}

fn parse_usize_range(
    value: &str,
    min: usize,
    max: usize,
    option: &str,
) -> std::result::Result<usize, String> {
    let value = value
        .parse::<usize>()
        .map_err(|_| format!("invalid {option} value"))?;
    if value < min || value > max {
        Err(format!("{option} must be between {min} and {max}"))
    } else {
        Ok(value)
    }
}

async fn run_drive(socket_path: &Path, config: DriveConfig) -> Result<()> {
    match config.input {
        DriveInput::Gamepad => run_gamepad_drive(socket_path, config).await,
        DriveInput::Chronos => run_chronos_drive(socket_path, config).await,
    }
}

async fn run_gamepad_drive(socket_path: &Path, config: DriveConfig) -> Result<()> {
    let mut gilrs = Gilrs::new().map_err(|error| anyhow::anyhow!("opening gamepads: {error}"))?;
    let gamepad_id = select_gamepad(&gilrs, config.device.as_deref())?;
    let gamepad = gilrs.gamepad(gamepad_id);
    println!("Driving with gamepad: {} ({gamepad_id:?})", gamepad.name());
    println!("Press Ctrl-C to stop and send neutral.");

    let mut stream = SocketCommandStream::connect(socket_path).await?;
    let mut ticker = interval(Duration::from_millis(1_000 / config.rate_hz));
    let mut last_sent = None;
    let mut last_sent_at = Instant::now() - Duration::from_secs(1);

    loop {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.context("waiting for Ctrl-C")?;
                send_neutral(&mut stream).await?;
                println!("Stopped.");
                return Ok(());
            }
            _ = ticker.tick() => {
                while let Some(event) = gilrs.next_event() {
                    if event.id != gamepad_id {
                        continue;
                    }

                    match event.event {
                        EventType::Disconnected => {
                            send_neutral(&mut stream).await?;
                            bail!("gamepad disconnected");
                        }
                        EventType::ButtonPressed(button, _) => {
                            handle_gamepad_button(&mut stream, button).await?;
                        }
                        _ => {}
                    }
                }

                let gamepad = gilrs.gamepad(gamepad_id);
                let throttle = axis_value_to_percent(
                    -gamepad.value(Axis::LeftStickY),
                    config.deadzone_percent,
                    config.invert_throttle,
                );
                let steering = axis_value_to_percent(
                    gamepad.value(Axis::LeftStickX),
                    config.deadzone_percent,
                    config.invert_steering,
                );

                let command = ControlCommand::Drive { throttle, steering };
                let now = Instant::now();
                if last_sent != Some((throttle, steering))
                    || now.duration_since(last_sent_at) >= Duration::from_millis(500)
                {
                    stream.send(&command).await?;
                    last_sent = Some((throttle, steering));
                    last_sent_at = now;
                }
            }
        }
    }
}

fn select_gamepad(gilrs: &Gilrs, filter: Option<&str>) -> Result<GamepadId> {
    for (id, gamepad) in gilrs.gamepads() {
        if !gamepad.is_connected() {
            continue;
        }

        if let Some(filter) = filter {
            let id_text = format!("{id:?}");
            if !id_text.eq_ignore_ascii_case(filter)
                && !gamepad
                    .name()
                    .to_lowercase()
                    .contains(&filter.to_lowercase())
            {
                continue;
            }
        }

        return Ok(id);
    }

    if let Some(filter) = filter {
        bail!("no connected gamepad matched {filter}");
    }

    bail!("no connected gamepad found");
}

async fn handle_gamepad_button(stream: &mut SocketCommandStream, button: Button) -> Result<()> {
    let command = match button {
        Button::South => Some(ControlCommand::FrontLedToggle),
        Button::East => Some(ControlCommand::BottomLed(
            rc_ble_controller::BottomLedAction::On,
        )),
        Button::West => Some(ControlCommand::BottomLed(
            rc_ble_controller::BottomLedAction::Off,
        )),
        Button::Start | Button::Mode => Some(ControlCommand::Drive {
            throttle: 0,
            steering: 0,
        }),
        _ => None,
    };

    if let Some(command) = command {
        stream.send(&command).await?;
    }

    Ok(())
}

async fn run_chronos_drive(socket_path: &Path, config: DriveConfig) -> Result<()> {
    let device = config
        .device
        .as_deref()
        .context("drive chronos requires --device <path> until receiver autodiscovery is added")?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let device = device.to_string();
    let baud_rate = config.baud_rate;
    let deadzone_percent = config.deadzone_percent;
    let invert_throttle = config.invert_throttle;
    let invert_steering = config.invert_steering;
    let reader_device = device.clone();

    std::thread::spawn(move || {
        if let Err(error) = read_chronos_ap_events(
            &reader_device,
            baud_rate,
            deadzone_percent,
            invert_throttle,
            invert_steering,
            tx,
        ) {
            eprintln!("Chronos receiver failed: {error:#}");
        }
    });

    let mut stream = None;
    println!("Driving from Chronos receiver on {device} at {baud_rate} baud.");
    println!(
        "Select ACC with #, then hold the bottom-right Down button while driving; the RF icon appears only while held."
    );
    println!("Release the bottom-right button to send neutral after about one second.");

    loop {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.context("waiting for Ctrl-C")?;
                send_chronos_socket_command(socket_path, &mut stream, &ControlCommand::Drive { throttle: 0, steering: 0 }).await?;
                println!("Stopped.");
                return Ok(());
            }
            command = rx.recv() => {
                let Some(command) = command else {
                    send_chronos_socket_command(socket_path, &mut stream, &ControlCommand::Drive { throttle: 0, steering: 0 }).await?;
                    bail!("Chronos receiver disconnected");
                };
                send_chronos_socket_command(socket_path, &mut stream, &command).await?;
            }
        }
    }
}

async fn send_chronos_socket_command(
    socket_path: &Path,
    stream: &mut Option<SocketCommandStream>,
    command: &ControlCommand,
) -> Result<()> {
    if stream.is_none() {
        *stream = Some(SocketCommandStream::connect(socket_path).await?);
    }

    let send_result = stream
        .as_mut()
        .expect("stream was just connected")
        .send(command)
        .await;

    match send_result {
        Ok(_) => {}
        Err(error) if error.to_string().starts_with("ERR ") => return Err(error),
        Err(_) => {
            *stream = Some(SocketCommandStream::connect(socket_path).await?);
            stream
                .as_mut()
                .expect("stream was just reconnected")
                .send(command)
                .await?;
        }
    }

    if is_neutral_drive(command) {
        *stream = None;
    }

    Ok(())
}

fn is_neutral_drive(command: &ControlCommand) -> bool {
    matches!(
        command,
        ControlCommand::Drive {
            throttle: 0,
            steering: 0
        }
    )
}

fn read_chronos_ap_events(
    device: &str,
    baud_rate: u32,
    deadzone_percent: u8,
    invert_throttle: bool,
    invert_steering: bool,
    tx: mpsc::UnboundedSender<ControlCommand>,
) -> Result<()> {
    let mut ap = ChronosAp::open(device, baud_rate)?;
    ap.start()?;
    let mut last_drive = None;
    let mut last_drive_sent_at = None;
    let mut last_packet_at = None;
    let neutral_after = Duration::from_secs(1);

    loop {
        let event = ap.poll_event()?;
        if !matches!(event, ChronosEvent::NoData) {
            last_packet_at = Some(Instant::now());
        }

        match event {
            ChronosEvent::NoData => {
                if last_drive.is_some_and(|drive| drive != (0, 0))
                    && last_packet_at
                        .is_some_and(|last_packet| last_packet.elapsed() >= neutral_after)
                {
                    last_drive = Some((0, 0));
                    last_drive_sent_at = Some(Instant::now());
                    if tx
                        .send(ControlCommand::Drive {
                            throttle: 0,
                            steering: 0,
                        })
                        .is_err()
                    {
                        let _ = ap.stop();
                        return Ok(());
                    }
                }
            }
            ChronosEvent::Accel { x, y, .. } => {
                let (throttle, steering) = chronos_drive_from_accel(
                    x as f32,
                    y as f32,
                    deadzone_percent,
                    invert_throttle,
                    invert_steering,
                );
                let drive = (throttle, steering);
                if should_send_chronos_drive(last_drive, last_drive_sent_at, drive) {
                    last_drive = Some(drive);
                    last_drive_sent_at = Some(Instant::now());
                    if tx
                        .send(ControlCommand::Drive { throttle, steering })
                        .is_err()
                    {
                        let _ = ap.stop();
                        return Ok(());
                    }
                }
            }
            ChronosEvent::UpButton => {
                last_drive = Some((0, 0));
                last_drive_sent_at = Some(Instant::now());
                if tx
                    .send(ControlCommand::Drive {
                        throttle: 0,
                        steering: 0,
                    })
                    .is_err()
                {
                    let _ = ap.stop();
                    return Ok(());
                }
            }
            ChronosEvent::StarButton => {
                if tx.send(ControlCommand::FrontLedToggle).is_err() {
                    let _ = ap.stop();
                    return Ok(());
                }
            }
            ChronosEvent::HashButton => {
                if tx
                    .send(ControlCommand::BottomLed(
                        rc_ble_controller::BottomLedAction::On,
                    ))
                    .is_err()
                {
                    let _ = ap.stop();
                    return Ok(());
                }
            }
            ChronosEvent::Unknown(data) => eprintln!("unknown Chronos event: {}", hex_bytes(&data)),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

async fn send_neutral(stream: &mut SocketCommandStream) -> Result<()> {
    stream
        .send(&ControlCommand::Drive {
            throttle: 0,
            steering: 0,
        })
        .await?;
    Ok(())
}

fn chronos_drive_from_accel(
    x: f32,
    y: f32,
    deadzone_percent: u8,
    invert_throttle: bool,
    invert_steering: bool,
) -> (i8, i8) {
    let throttle = axis_value_to_percent(-(x / 128.0), deadzone_percent, invert_throttle);
    let steering = axis_value_to_percent(-(y / 128.0), deadzone_percent, invert_steering);
    (throttle, steering)
}

fn should_send_chronos_drive(
    last_drive: Option<(i8, i8)>,
    last_sent_at: Option<Instant>,
    drive: (i8, i8),
) -> bool {
    let Some(last_drive) = last_drive else {
        return true;
    };
    if last_drive == drive {
        return false;
    }
    if drive == (0, 0) || last_drive == (0, 0) {
        return true;
    }
    if last_sent_at.is_some_and(|sent_at| sent_at.elapsed() < CHRONOS_DRIVE_MIN_INTERVAL) {
        return false;
    }

    let throttle_delta = (drive.0 as i16 - last_drive.0 as i16).abs();
    let steering_delta = (drive.1 as i16 - last_drive.1 as i16).abs();
    throttle_delta >= CHRONOS_DRIVE_MIN_DELTA as i16
        || steering_delta >= CHRONOS_DRIVE_MIN_DELTA as i16
}

struct ChronosCalibrateConfig {
    device: String,
    baud_rate: u32,
    deadzone_percent: u8,
    invert_throttle: bool,
    invert_steering: bool,
    samples: usize,
}

impl ChronosCalibrateConfig {
    fn parse(command: &str) -> std::result::Result<Option<Self>, String> {
        if !command
            .split_whitespace()
            .next()
            .is_some_and(|word| matches!(word, "chronos-calibrate" | "chronos-map"))
        {
            return Ok(None);
        }

        let mut args = command.split_whitespace().skip(1);
        let mut device = None;
        let mut baud_rate = 115_200;
        let mut deadzone_percent = 8;
        let mut invert_throttle = false;
        let mut invert_steering = false;
        let mut samples = 5;

        while let Some(arg) = args.next() {
            match arg {
                "--device" => device = Some(next_word(&mut args, "--device")?.to_string()),
                "--baud" | "--baud-rate" => {
                    baud_rate = next_word(&mut args, "--baud")?
                        .parse()
                        .map_err(|_| "invalid --baud value".to_string())?;
                }
                "--deadzone" => {
                    deadzone_percent =
                        parse_u8_range(next_word(&mut args, "--deadzone")?, 0, 99, "--deadzone")?;
                }
                "--samples" => {
                    samples =
                        parse_usize_range(next_word(&mut args, "--samples")?, 1, 50, "--samples")?;
                }
                "--invert-throttle" => invert_throttle = true,
                "--invert-steering" => invert_steering = true,
                "--help" | "-h" => return Err(chronos_calibrate_usage().to_string()),
                _ if device.is_none() && !arg.starts_with("--") => device = Some(arg.to_string()),
                _ => return Err(format!("unknown chronos-calibrate option: {arg}")),
            }
        }

        Ok(Some(Self {
            device: device
                .ok_or_else(|| "chronos-calibrate requires --device <path>".to_string())?,
            baud_rate,
            deadzone_percent,
            invert_throttle,
            invert_steering,
            samples,
        }))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChronosCalibrationMove {
    Neutral,
    Forward,
    Backward,
    Left,
    Right,
}

impl ChronosCalibrationMove {
    const ALL: [Self; 5] = [
        Self::Neutral,
        Self::Forward,
        Self::Backward,
        Self::Left,
        Self::Right,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Forward => "forward",
            Self::Backward => "backward",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    const fn instructions(self) -> &'static str {
        match self {
            Self::Neutral => "hold the watch face-up and level",
            Self::Forward => {
                "tilt the watch toward the direction you want the car to drive forward"
            }
            Self::Backward => "tilt the watch toward the direction you want the car to reverse",
            Self::Left => "tilt the watch toward the direction you want the car to steer left",
            Self::Right => "tilt the watch toward the direction you want the car to steer right",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ChronosCalibrationReading {
    movement: ChronosCalibrationMove,
    x: f32,
    y: f32,
    z: f32,
    throttle: i8,
    steering: i8,
}

fn run_chronos_calibration(config: ChronosCalibrateConfig) -> Result<()> {
    println!(
        "Chronos calibration checks watch tilt mapping only; it does not send commands to the car."
    );
    println!("Select ACC with # on the watch before starting.");
    println!(
        "For each step, press Enter, then hold the bottom-right Down button until samples finish."
    );

    let mut ap = ChronosAp::open(&config.device, config.baud_rate)?;
    println!(
        "Polling Chronos AP on {} at {} baud.",
        config.device, config.baud_rate
    );
    print_chronos_status(&mut ap, "Status before start");
    ap.start()?;
    print_chronos_status(&mut ap, "Status after start ");

    run_with_chronos_stop(&mut ap, |ap| run_chronos_calibration_steps(ap, &config))
}

fn run_chronos_calibration_steps(
    ap: &mut ChronosAp,
    config: &ChronosCalibrateConfig,
) -> Result<()> {
    let mut readings = Vec::new();

    for movement in ChronosCalibrationMove::ALL {
        println!();
        println!("Step: {} - {}.", movement.label(), movement.instructions());
        wait_for_enter("Press Enter when ready to capture this position...")?;
        let reading = capture_chronos_calibration_reading(movement, ap, config)?;
        println!(
            "Captured {}: x={:.1} y={:.1} z={:.1} -> throttle={} steering={}",
            reading.movement.label(),
            reading.x,
            reading.y,
            reading.z,
            reading.throttle,
            reading.steering
        );
        readings.push(reading);
    }

    print_chronos_calibration_summary(&readings);
    Ok(())
}

fn wait_for_enter(prompt: &str) -> Result<()> {
    print!("{prompt}");
    io::stdout().flush().context("flushing prompt")?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("reading calibration confirmation")?;
    Ok(())
}

fn capture_chronos_calibration_reading(
    movement: ChronosCalibrationMove,
    ap: &mut ChronosAp,
    config: &ChronosCalibrateConfig,
) -> Result<ChronosCalibrationReading> {
    let mut count = 0usize;
    let mut x_sum = 0i32;
    let mut y_sum = 0i32;
    let mut z_sum = 0i32;
    let started_at = Instant::now();

    while count < config.samples && started_at.elapsed() < Duration::from_secs(10) {
        match ap.poll_event()? {
            ChronosEvent::Accel { x, y, z } => {
                count += 1;
                x_sum += x as i32;
                y_sum += y as i32;
                z_sum += z as i32;
                let (throttle, steering) = chronos_drive_from_accel(
                    x as f32,
                    y as f32,
                    config.deadzone_percent,
                    config.invert_throttle,
                    config.invert_steering,
                );
                println!(
                    "  sample {count}/{}: x={x} y={y} z={z} -> throttle={throttle} steering={steering}",
                    config.samples
                );
            }
            ChronosEvent::NoData => std::thread::sleep(Duration::from_millis(100)),
            event => {
                println!("  ignored {}", event.describe());
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    if count == 0 {
        bail!(
            "no accelerometer packets captured for {}. Keep ACC selected and hold the bottom-right Down button while sampling",
            movement.label()
        );
    }

    let x = x_sum as f32 / count as f32;
    let y = y_sum as f32 / count as f32;
    let z = z_sum as f32 / count as f32;
    let (throttle, steering) = chronos_drive_from_accel(
        x,
        y,
        config.deadzone_percent,
        config.invert_throttle,
        config.invert_steering,
    );

    Ok(ChronosCalibrationReading {
        movement,
        x,
        y,
        z,
        throttle,
        steering,
    })
}

fn print_chronos_calibration_summary(readings: &[ChronosCalibrationReading]) {
    const THRESHOLD: i8 = 15;

    println!();
    println!("Chronos mapping summary:");
    for reading in readings {
        let status = chronos_calibration_status(*reading, THRESHOLD);
        println!(
            "  {:8} -> throttle={:4} steering={:4} [{status}]",
            reading.movement.label(),
            reading.throttle,
            reading.steering
        );
    }

    if chronos_axis_looks_reversed(
        readings,
        ChronosCalibrationMove::Forward,
        ChronosCalibrationMove::Backward,
        |reading| reading.throttle,
        THRESHOLD,
    ) {
        println!("Suggestion: throttle looks reversed; use `--invert-throttle` for drive chronos.");
    }
    if chronos_axis_looks_reversed(
        readings,
        ChronosCalibrationMove::Right,
        ChronosCalibrationMove::Left,
        |reading| reading.steering,
        THRESHOLD,
    ) {
        println!("Suggestion: steering looks reversed; use `--invert-steering` for drive chronos.");
    }
    if let Some(neutral) = find_chronos_reading(readings, ChronosCalibrationMove::Neutral) {
        if neutral.throttle.abs() > THRESHOLD || neutral.steering.abs() > THRESHOLD {
            println!(
                "Neutral is outside the deadzone; hold the watch flatter or increase `--deadzone` before driving."
            );
        }
    }

    println!(
        "Expected driving: tilt forward = positive throttle, tilt backward = negative throttle, tilt left/right = steering."
    );
}

fn chronos_calibration_status(reading: ChronosCalibrationReading, threshold: i8) -> &'static str {
    match reading.movement {
        ChronosCalibrationMove::Neutral
            if reading.throttle.abs() <= threshold && reading.steering.abs() <= threshold =>
        {
            "ok"
        }
        ChronosCalibrationMove::Forward if reading.throttle >= threshold => "ok",
        ChronosCalibrationMove::Backward if reading.throttle <= -threshold => "ok",
        ChronosCalibrationMove::Left if reading.steering <= -threshold => "ok",
        ChronosCalibrationMove::Right if reading.steering >= threshold => "ok",
        _ => "check",
    }
}

fn chronos_axis_looks_reversed(
    readings: &[ChronosCalibrationReading],
    positive_move: ChronosCalibrationMove,
    negative_move: ChronosCalibrationMove,
    value: impl Fn(ChronosCalibrationReading) -> i8,
    threshold: i8,
) -> bool {
    let Some(positive) = find_chronos_reading(readings, positive_move) else {
        return false;
    };
    let Some(negative) = find_chronos_reading(readings, negative_move) else {
        return false;
    };

    value(positive) <= -threshold && value(negative) >= threshold
}

fn find_chronos_reading(
    readings: &[ChronosCalibrationReading],
    movement: ChronosCalibrationMove,
) -> Option<ChronosCalibrationReading> {
    readings
        .iter()
        .copied()
        .find(|reading| reading.movement == movement)
}

fn print_chronos_status(ap: &mut ChronosAp, label: &str) {
    match ap.status() {
        Ok(status) => println!("{label}: {}", hex_bytes(&status)),
        Err(error) => eprintln!("warning: {label} failed: {error:#}"),
    }
}

fn run_with_chronos_stop<T>(
    ap: &mut ChronosAp,
    run: impl FnOnce(&mut ChronosAp) -> Result<T>,
) -> Result<T> {
    let result = run(ap);
    let stop_result = ap.stop();

    match (result, stop_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(error)) => Err(error).context("stopping Chronos AP after run"),
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(stop_error)) => {
            eprintln!("warning: failed to stop Chronos AP after error: {stop_error:#}");
            Err(error)
        }
    }
}

struct ChronosProbeConfig {
    device: String,
    baud_rate: u32,
    limit: Option<usize>,
    raw: bool,
}

impl ChronosProbeConfig {
    fn parse(command: &str) -> std::result::Result<Option<Self>, String> {
        if !command
            .split_whitespace()
            .next()
            .is_some_and(|word| word == "chronos-probe")
        {
            return Ok(None);
        }

        let mut args = command.split_whitespace().skip(1);
        let mut device = None;
        let mut baud_rate = 115_200;
        let mut limit = None;
        let mut raw = false;

        while let Some(arg) = args.next() {
            match arg {
                "--device" => device = Some(next_word(&mut args, "--device")?.to_string()),
                "--baud" | "--baud-rate" => {
                    baud_rate = next_word(&mut args, "--baud")?
                        .parse()
                        .map_err(|_| "invalid --baud value".to_string())?;
                }
                "--limit" => {
                    limit = Some(
                        next_word(&mut args, "--limit")?
                            .parse()
                            .map_err(|_| "invalid --limit value".to_string())?,
                    );
                }
                "--raw" => raw = true,
                "--help" | "-h" => return Err(chronos_probe_usage().to_string()),
                _ if device.is_none() && !arg.starts_with("--") => device = Some(arg.to_string()),
                _ => return Err(format!("unknown chronos-probe option: {arg}")),
            }
        }

        Ok(Some(Self {
            device: device.ok_or_else(|| "chronos-probe requires --device <path>".to_string())?,
            baud_rate,
            limit,
            raw,
        }))
    }
}

fn run_chronos_probe(config: ChronosProbeConfig) -> Result<()> {
    if config.raw {
        return run_chronos_raw_probe(config);
    }

    let mut ap = ChronosAp::open(&config.device, config.baud_rate)?;
    println!(
        "Polling Chronos AP on {} at {} baud. Select ACC with #, then hold the bottom-right Down button; the RF icon appears only while held.",
        config.device, config.baud_rate
    );
    print_chronos_status(&mut ap, "Status before start");
    ap.start()?;
    print_chronos_status(&mut ap, "Status after start ");

    let limit = config.limit.unwrap_or(40);
    run_with_chronos_stop(&mut ap, |ap| {
        for index in 0..limit {
            let event = ap.poll_event()?;
            println!("{index:04}: {}", event.describe());
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    })
}

fn run_chronos_raw_probe(config: ChronosProbeConfig) -> Result<()> {
    let mut port = serialport::new(&config.device, config.baud_rate)
        .timeout(Duration::from_millis(500))
        .open()
        .with_context(|| format!("opening Chronos receiver {}", config.device))?;
    let mut buffer = [0u8; 64];
    let mut seen = 0usize;

    println!(
        "Reading raw Chronos receiver bytes from {} at {} baud. Press Ctrl-C to stop.",
        config.device, config.baud_rate
    );

    loop {
        match port.read(&mut buffer) {
            Ok(0) => {}
            Ok(read) => {
                seen += read;
                for byte in &buffer[..read] {
                    print!("{byte:02x} ");
                }
                println!();
                if config.limit.is_some_and(|limit| seen >= limit) {
                    return Ok(());
                }
            }
            Err(error) if error.kind() == ErrorKind::TimedOut => {}
            Err(error) => return Err(error).context("reading Chronos receiver"),
        }
    }
}

struct ChronosAp {
    port: Box<dyn SerialPort>,
}

impl ChronosAp {
    fn open(device: &str, baud_rate: u32) -> Result<Self> {
        let port = serialport::new(device, baud_rate)
            .timeout(Duration::from_millis(750))
            .open()
            .with_context(|| format!("opening Chronos access point {device}"))?;
        Ok(Self { port })
    }

    fn status(&mut self) -> Result<Vec<u8>> {
        self.send(0x00, &[0x00]).context("reading AP status")
    }

    fn start(&mut self) -> Result<()> {
        self.send(0x07, &[]).context("starting SimpliciTI AP")?;
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.send(0x09, &[]).context("stopping SimpliciTI AP")?;
        Ok(())
    }

    fn poll_event(&mut self) -> Result<ChronosEvent> {
        let payload = self
            .send(0x08, &[0x00, 0x00, 0x00, 0x00])
            .context("polling Chronos data")?;
        Ok(ChronosEvent::from_payload(&payload))
    }

    fn send(&mut self, opcode: u8, payload: &[u8]) -> Result<Vec<u8>> {
        let len = payload
            .len()
            .checked_add(3)
            .context("Chronos AP packet too long")?;
        if len > u8::MAX as usize {
            bail!("Chronos AP packet too long");
        }

        let mut packet = Vec::with_capacity(len);
        packet.extend_from_slice(&[0xff, opcode, len as u8]);
        packet.extend_from_slice(payload);

        let _ = self.port.clear(ClearBuffer::Input);
        self.port
            .write_all(&packet)
            .with_context(|| format!("sending AP opcode {opcode:#04x}"))?;
        self.port.flush().context("flushing AP command")?;
        std::thread::sleep(Duration::from_millis(15));

        let mut header = [0u8; 3];
        self.port
            .read_exact(&mut header)
            .with_context(|| format!("reading AP opcode {opcode:#04x} response header"))?;
        if header[0] != 0xff {
            bail!("invalid AP response start byte {:#04x}", header[0]);
        }
        if header[2] < 3 {
            bail!("invalid AP response length {}", header[2]);
        }

        let payload_len = header[2] as usize - 3;
        let mut response_payload = vec![0u8; payload_len];
        if payload_len > 0 {
            self.port
                .read_exact(&mut response_payload)
                .with_context(|| format!("reading AP opcode {opcode:#04x} response payload"))?;
        }

        Ok(response_payload)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
enum ChronosEvent {
    NoData,
    Accel { x: i8, y: i8, z: i8 },
    UpButton,
    StarButton,
    HashButton,
    Unknown(Vec<u8>),
}

impl ChronosEvent {
    fn from_payload(payload: &[u8]) -> Self {
        match payload {
            [0xff, ..] => Self::NoData,
            [0x01, x, y, z, ..] => Self::Accel {
                x: *x as i8,
                y: *y as i8,
                z: *z as i8,
            },
            [0x32, ..] => Self::UpButton,
            [0x12, ..] => Self::StarButton,
            [0x22, ..] => Self::HashButton,
            _ => Self::Unknown(payload.to_vec()),
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::NoData => "no data".to_string(),
            Self::Accel { x, y, z } => {
                let (throttle, steering) =
                    chronos_drive_from_accel(*x as f32, *y as f32, 8, false, false);
                format!("accel x={x} y={y} z={z} -> drive throttle={throttle} steering={steering}")
            }
            Self::UpButton => "button UP -> neutral".to_string(),
            Self::StarButton => "button STAR -> front LEDs toggle".to_string(),
            Self::HashButton => "button # -> bottom LEDs on".to_string(),
            Self::Unknown(data) => format!("unknown {}", hex_bytes(data)),
        }
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn service_unit_status() -> String {
    match Command::new("systemctl")
        .args(["is-active", SERVICE_NAME])
        .output()
    {
        Ok(output) if output.status.success() => "active".to_string(),
        Ok(output) => {
            let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if status.is_empty() {
                "inactive".to_string()
            } else {
                status
            }
        }
        Err(_) => "unknown (systemctl unavailable)".to_string(),
    }
}

#[derive(Default)]
struct Cli {
    socket_path: Option<PathBuf>,
    command: String,
    help: bool,
}

impl Cli {
    fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut args = args.peekable();
        let mut cli = Cli::default();
        let mut command_parts = Vec::new();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--socket" => {
                    let socket = args
                        .next()
                        .ok_or_else(|| "--socket requires a value".to_string())?;
                    cli.socket_path = Some(PathBuf::from(socket));
                }
                "--help" | "-h" => {
                    cli.help = true;
                    return Ok(cli);
                }
                _ if arg.starts_with("--") => return Err(format!("unknown option: {arg}")),
                _ => {
                    command_parts.push(arg);
                    command_parts.extend(args);
                    break;
                }
            }
        }

        cli.command = command_parts.join(" ");
        Ok(cli)
    }
}

fn detect_socket_path() -> PathBuf {
    if let Some(path) = env::var_os("RC_CAR_SOCKET").map(PathBuf::from) {
        return path;
    }

    let candidates = [
        Some(PathBuf::from(DEFAULT_SYSTEM_SOCKET)),
        Some(PathBuf::from(LEGACY_SYSTEM_SOCKET)),
        env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .map(|path| path.join("rc-ble-controller.sock")),
        Some(PathBuf::from(DEFAULT_TMP_SOCKET)),
    ];

    candidates
        .iter()
        .flatten()
        .find(|path| Path::new(path).exists())
        .cloned()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SYSTEM_SOCKET))
}

fn print_usage() {
    eprintln!(
        "Usage:
  rc-car-command <command>
  rc-car-command commands
  rc-car-command status
  rc-car-command restart
  rc-car-command logs
  rc-car-command --socket /tmp/rc-ble-controller.sock <command>

Examples:
  rc-car-command forward
  rc-car-command drive 40 -15
  rc-car-command drive gamepad
  rc-car-command chronos-probe --device /dev/ttyACM0 --limit 128
  rc-car-command chronos-calibrate --device /dev/ttyACM0
  rc-car-command stop
  rc-car-command servo-left
  rc-car-command bottom-on
  rc-car-command restart"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chronos_default_mapping_uses_x_for_throttle_and_y_for_steering() {
        let (throttle, steering) = chronos_drive_from_accel(-64.0, 0.0, 8, false, false);
        assert!(throttle > 0);
        assert_eq!(steering, 0);

        let (throttle, steering) = chronos_drive_from_accel(64.0, 0.0, 8, false, false);
        assert!(throttle < 0);
        assert_eq!(steering, 0);

        let (throttle, steering) = chronos_drive_from_accel(0.0, 64.0, 8, false, false);
        assert_eq!(throttle, 0);
        assert!(steering < 0);

        let (throttle, steering) = chronos_drive_from_accel(0.0, -64.0, 8, false, false);
        assert_eq!(throttle, 0);
        assert!(steering > 0);
    }

    #[test]
    fn chronos_mapping_respects_inversion_flags() {
        let normal = chronos_drive_from_accel(64.0, -64.0, 8, false, false);
        let inverted_throttle = chronos_drive_from_accel(64.0, -64.0, 8, true, false);
        let inverted_steering = chronos_drive_from_accel(64.0, -64.0, 8, false, true);

        assert_eq!(inverted_throttle.0, -normal.0);
        assert_eq!(inverted_throttle.1, normal.1);
        assert_eq!(inverted_steering.0, normal.0);
        assert_eq!(inverted_steering.1, -normal.1);
    }

    #[test]
    fn chronos_drive_updates_are_rate_and_delta_limited() {
        let recent = Some(Instant::now());
        let old = Some(Instant::now() - CHRONOS_DRIVE_MIN_INTERVAL - Duration::from_millis(1));

        assert!(should_send_chronos_drive(None, None, (20, 0)));
        assert!(!should_send_chronos_drive(Some((20, 0)), recent, (20, 0)));
        assert!(!should_send_chronos_drive(Some((20, 0)), old, (24, 0)));
        assert!(!should_send_chronos_drive(Some((20, 0)), recent, (30, 0)));
        assert!(should_send_chronos_drive(Some((20, 0)), old, (25, 0)));
        assert!(should_send_chronos_drive(Some((20, 0)), recent, (0, 0)));
        assert!(should_send_chronos_drive(Some((0, 0)), recent, (20, 0)));
    }

    #[test]
    fn chronos_calibration_flags_reversed_axes() {
        let readings = [
            ChronosCalibrationReading {
                movement: ChronosCalibrationMove::Forward,
                x: 0.0,
                y: 0.0,
                z: 0.0,
                throttle: -50,
                steering: 0,
            },
            ChronosCalibrationReading {
                movement: ChronosCalibrationMove::Backward,
                x: 0.0,
                y: 0.0,
                z: 0.0,
                throttle: 50,
                steering: 0,
            },
            ChronosCalibrationReading {
                movement: ChronosCalibrationMove::Right,
                x: 0.0,
                y: 0.0,
                z: 0.0,
                throttle: 0,
                steering: -50,
            },
            ChronosCalibrationReading {
                movement: ChronosCalibrationMove::Left,
                x: 0.0,
                y: 0.0,
                z: 0.0,
                throttle: 0,
                steering: 50,
            },
        ];

        assert!(chronos_axis_looks_reversed(
            &readings,
            ChronosCalibrationMove::Forward,
            ChronosCalibrationMove::Backward,
            |reading| reading.throttle,
            15
        ));
        assert!(chronos_axis_looks_reversed(
            &readings,
            ChronosCalibrationMove::Right,
            ChronosCalibrationMove::Left,
            |reading| reading.steering,
            15
        ));
    }
}
