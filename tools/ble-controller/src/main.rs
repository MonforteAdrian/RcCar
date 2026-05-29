use std::{
    env, fs,
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use btleplug::{
    api::{Central, Manager as _, Peripheral as _, ScanFilter, WriteType},
    platform::{Adapter, Manager, Peripheral},
};
use rc_ble_controller::{
    AutoStopEffect, BleWrite, COMMAND_MENU, ControlCommand, parse_control_command,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    time::{Instant, sleep, timeout},
};
use uuid::Uuid;

const DEFAULT_AUTO_STOP_SECONDS: u64 = 3;
const DEVICE_NAME: &str = "RcCar";

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

    let manager = Manager::new().await.context("creating BLE manager")?;
    let adapters = manager.adapters().await.context("listing BLE adapters")?;
    let adapter = adapters
        .into_iter()
        .next()
        .context("no Bluetooth adapter found")?;

    let peripheral = find_rc_car(&adapter, cli.address.as_deref(), cli.scan_seconds).await?;

    if !peripheral
        .is_connected()
        .await
        .context("checking BLE connection state")?
    {
        peripheral.connect().await.context("connecting to RcCar")?;
    }

    peripheral
        .discover_services()
        .await
        .context("discovering RcCar GATT services")?;

    if cli.service {
        run_service(
            &peripheral,
            &cli.socket_path,
            Duration::from_secs(cli.auto_stop_seconds),
        )
        .await?;
    } else if let Some(command) = cli.command {
        write_command(&peripheral, &command).await?;
    } else {
        run_interactive(&peripheral).await?;
    }

    Ok(())
}

async fn write_command(peripheral: &Peripheral, command: &ControlCommand) -> Result<()> {
    send_control_command_to_ble(peripheral, command).await?;
    println!("sent {}", command.description());
    Ok(())
}

async fn send_control_command_to_ble(
    peripheral: &Peripheral,
    command: &ControlCommand,
) -> Result<()> {
    ensure_peripheral_connected(peripheral).await?;

    for write in command.ble_writes() {
        send_ble_write(peripheral, &write).await?;
    }

    Ok(())
}

async fn ensure_peripheral_connected(peripheral: &Peripheral) -> Result<()> {
    let mut reconnected = false;
    match peripheral.is_connected().await {
        Ok(true) => {}
        Ok(false) => {
            peripheral
                .connect()
                .await
                .context("reconnecting to RcCar")?;
            reconnected = true;
        }
        Err(error) => {
            eprintln!("warning: failed to check BLE connection state before write: {error:#}");
            peripheral
                .connect()
                .await
                .context("reconnecting to RcCar after connection-state error")?;
            reconnected = true;
        }
    }

    if reconnected {
        peripheral
            .discover_services()
            .await
            .context("discovering RcCar GATT services")?;
    }

    Ok(())
}

async fn send_ble_write(peripheral: &Peripheral, write: &BleWrite) -> Result<()> {
    let uuid =
        Uuid::parse_str(write.characteristic.uuid_str()).context("parsing characteristic UUID")?;
    let characteristic = peripheral
        .characteristics()
        .into_iter()
        .find(|characteristic| characteristic.uuid == uuid)
        .with_context(|| {
            format!(
                "characteristic {} not found",
                write.characteristic.uuid_str()
            )
        })?;

    peripheral
        .write(&characteristic, &write.payload, WriteType::WithResponse)
        .await
        .with_context(|| format!("writing {}", write.description))?;

    Ok(())
}

async fn run_service(
    peripheral: &Peripheral,
    socket_path: &Path,
    auto_stop_after: Duration,
) -> Result<()> {
    if socket_path.exists() {
        fs::remove_file(socket_path)
            .with_context(|| format!("removing existing socket {}", socket_path.display()))?;
    }

    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("binding socket {}", socket_path.display()))?;
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o660))
        .with_context(|| format!("setting socket permissions {}", socket_path.display()))?;

    println!("RcCar BLE service listening on {}", socket_path.display());
    println!("Socket permissions: 0660");
    println!("Send commands with: rc-car-command forward");
    if auto_stop_after.is_zero() {
        println!("Auto-stop is disabled");
    } else {
        println!(
            "Auto-stop after {} seconds without commands",
            auto_stop_after.as_secs()
        );
    }

    let mut auto_stop_deadline = None;
    loop {
        let stream = accept_socket_client(&listener, auto_stop_deadline).await?;
        let Some(stream) = stream else {
            if let Err(error) = send_auto_stop(peripheral).await {
                eprintln!("auto-stop failed: {error:#}");
            } else {
                println!("auto-stop: sent motor stop");
            }
            auto_stop_deadline = None;
            continue;
        };

        match handle_socket_client(peripheral, stream, auto_stop_after).await {
            Ok(effect) => {
                apply_auto_stop_effect(effect, auto_stop_after, &mut auto_stop_deadline);
            }
            Err(error) => eprintln!("socket client failed: {error:#}"),
        }
    }
}

async fn accept_socket_client(
    listener: &UnixListener,
    auto_stop_deadline: Option<Instant>,
) -> Result<Option<UnixStream>> {
    if let Some(deadline) = auto_stop_deadline {
        let now = Instant::now();
        if deadline <= now {
            return Ok(None);
        }

        match timeout(deadline - now, listener.accept()).await {
            Ok(Ok((stream, _addr))) => Ok(Some(stream)),
            Ok(Err(error)) => Err(error).context("accepting socket client"),
            Err(_) => Ok(None),
        }
    } else {
        let (stream, _addr) = listener.accept().await.context("accepting socket client")?;
        Ok(Some(stream))
    }
}

async fn send_auto_stop(peripheral: &Peripheral) -> Result<()> {
    let command = parse_control_command("stop").map_err(anyhow::Error::msg)?;
    send_control_command_to_ble(peripheral, &command).await
}

fn apply_auto_stop_effect(
    effect: AutoStopEffect,
    auto_stop_after: Duration,
    auto_stop_deadline: &mut Option<Instant>,
) {
    match effect {
        AutoStopEffect::Arm if !auto_stop_after.is_zero() => {
            *auto_stop_deadline = Some(Instant::now() + auto_stop_after);
        }
        AutoStopEffect::Disarm => {
            *auto_stop_deadline = None;
        }
        AutoStopEffect::Arm | AutoStopEffect::NoChange => {}
    }
}

async fn handle_socket_client(
    peripheral: &Peripheral,
    stream: UnixStream,
    auto_stop_after: Duration,
) -> Result<AutoStopEffect> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let mut final_effect = AutoStopEffect::NoChange;
    let mut client_auto_stop_deadline = None;

    loop {
        line.clear();
        match read_socket_line(&mut reader, &mut line, &mut client_auto_stop_deadline).await? {
            SocketReadEvent::AutoStop => {
                if let Err(error) = send_auto_stop(peripheral).await {
                    let response = format!("ERR auto-stop failed: {error:#}\n");
                    writer
                        .write_all(response.as_bytes())
                        .await
                        .context("writing socket auto-stop error")?;
                } else {
                    writer
                        .write_all(b"OK auto-stop\n")
                        .await
                        .context("writing socket auto-stop")?;
                    final_effect = AutoStopEffect::Disarm;
                }
                continue;
            }
            SocketReadEvent::Line(0) => {
                writer.shutdown().await.context("closing socket client")?;
                return Ok(final_effect);
            }
            SocketReadEvent::Line(_) => {}
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line == "status" {
            let ble_status = match peripheral.is_connected().await {
                Ok(true) => "connected",
                Ok(false) => "disconnected",
                Err(_) => "unknown",
            };
            let auto_stop_status = if auto_stop_after.is_zero() {
                "disabled".to_string()
            } else {
                format!("{}s", auto_stop_after.as_secs())
            };
            let response =
                format!("OK service running\nBLE: {ble_status}\nAuto-stop: {auto_stop_status}\n");
            writer
                .write_all(response.as_bytes())
                .await
                .context("writing socket status")?;
            continue;
        }

        if matches!(line, "h" | "help" | "menu" | "commands" | "list") {
            writer
                .write_all(COMMAND_MENU.as_bytes())
                .await
                .context("writing socket help")?;
            continue;
        }

        match parse_control_command(line) {
            Ok(command) => match send_control_command_to_ble(peripheral, &command).await {
                Ok(()) => {
                    let response = format!("OK sent {}\n", command.description());
                    writer
                        .write_all(response.as_bytes())
                        .await
                        .context("writing socket success")?;
                    let effect = command.auto_stop_effect();
                    apply_auto_stop_effect(effect, auto_stop_after, &mut client_auto_stop_deadline);
                    final_effect = final_effect.merge(effect);
                }
                Err(error) => {
                    let response = format!("ERR {error:#}\n");
                    writer
                        .write_all(response.as_bytes())
                        .await
                        .context("writing socket BLE error")?;
                }
            },
            Err(message) => {
                let response = format!("ERR {message}\n");
                writer
                    .write_all(response.as_bytes())
                    .await
                    .context("writing socket parse error")?;
            }
        }
    }
}

enum SocketReadEvent {
    Line(usize),
    AutoStop,
}

async fn read_socket_line<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut String,
    auto_stop_deadline: &mut Option<Instant>,
) -> Result<SocketReadEvent> {
    if let Some(deadline) = *auto_stop_deadline {
        let now = Instant::now();
        if deadline <= now {
            *auto_stop_deadline = None;
            return Ok(SocketReadEvent::AutoStop);
        }

        match timeout(deadline - now, reader.read_line(line)).await {
            Ok(Ok(read)) => Ok(SocketReadEvent::Line(read)),
            Ok(Err(error)) => Err(error).context("reading socket command"),
            Err(_) => {
                *auto_stop_deadline = None;
                Ok(SocketReadEvent::AutoStop)
            }
        }
    } else {
        let read = timeout(Duration::from_secs(5), reader.read_line(line))
            .await
            .context("socket client timed out waiting for a command")?
            .context("reading socket command")?;
        Ok(SocketReadEvent::Line(read))
    }
}

async fn run_interactive(peripheral: &Peripheral) -> Result<()> {
    println!(
        "Connected to RcCar. Type a command number or command text. Type 'help' for options, 'quit' to exit."
    );
    print_interactive_menu();

    let stdin = io::stdin();
    loop {
        print!("rc> ");
        io::stdout().flush().context("flushing prompt")?;

        let mut line = String::new();
        if stdin.read_line(&mut line).context("reading command")? == 0 {
            break;
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        match line {
            "q" | "quit" | "exit" => break,
            "h" | "help" | "menu" => {
                print_interactive_menu();
                continue;
            }
            _ => {}
        }

        match parse_control_command(line) {
            Ok(command) => {
                if let Err(error) = write_command(peripheral, &command).await {
                    eprintln!("write failed: {error:#}");
                } else {
                    print_interactive_menu();
                }
            }
            Err(message) => eprintln!("{message}. Type 'help' to list commands."),
        }
    }

    Ok(())
}

async fn find_rc_car(
    adapter: &Adapter,
    address: Option<&str>,
    scan_seconds: u64,
) -> Result<Peripheral> {
    adapter
        .start_scan(ScanFilter::default())
        .await
        .context("starting BLE scan")?;

    let deadline = Instant::now() + Duration::from_secs(scan_seconds);
    while Instant::now() < deadline {
        for peripheral in adapter
            .peripherals()
            .await
            .context("listing scanned BLE devices")?
        {
            if matches_rc_car(&peripheral, address).await? {
                let _ = adapter.stop_scan().await;
                return Ok(peripheral);
            }
        }

        sleep(Duration::from_millis(250)).await;
    }

    let _ = adapter.stop_scan().await;

    if let Some(address) = address {
        bail!("RcCar with address {address} was not found");
    }

    bail!("RcCar was not found by advertised name");
}

async fn matches_rc_car(peripheral: &Peripheral, address: Option<&str>) -> Result<bool> {
    let Some(properties) = peripheral
        .properties()
        .await
        .context("reading BLE device properties")?
    else {
        return Ok(false);
    };

    if let Some(address) = address {
        return Ok(properties.address.to_string().eq_ignore_ascii_case(address));
    }

    Ok(properties.local_name.as_deref() == Some(DEVICE_NAME))
}

#[derive(Default)]
struct Cli {
    address: Option<String>,
    scan_seconds: u64,
    service: bool,
    socket_path: PathBuf,
    auto_stop_seconds: u64,
    command: Option<ControlCommand>,
    help: bool,
}

impl Cli {
    fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut args = args.peekable();
        let mut cli = Cli {
            scan_seconds: 5,
            socket_path: default_socket_path(),
            auto_stop_seconds: DEFAULT_AUTO_STOP_SECONDS,
            ..Default::default()
        };

        while let Some(arg) = args.peek() {
            if !arg.starts_with("--") {
                break;
            }

            let arg = args.next().expect("peeked argument must exist");
            match arg.as_str() {
                "--addr" | "--address" => {
                    cli.address = Some(next_arg(&mut args, "--addr")?);
                }
                "--scan-seconds" => {
                    let seconds = next_arg(&mut args, "--scan-seconds")?;
                    cli.scan_seconds = seconds
                        .parse()
                        .map_err(|_| format!("invalid --scan-seconds value: {seconds}"))?;
                }
                "--service" => {
                    cli.service = true;
                }
                "--socket" => {
                    cli.socket_path = PathBuf::from(next_arg(&mut args, "--socket")?);
                }
                "--auto-stop-seconds" => {
                    let seconds = next_arg(&mut args, "--auto-stop-seconds")?;
                    cli.auto_stop_seconds = seconds
                        .parse()
                        .map_err(|_| format!("invalid --auto-stop-seconds value: {seconds}"))?;
                }
                "--help" | "-h" => {
                    cli.help = true;
                    return Ok(cli);
                }
                _ => return Err(format!("unknown option: {arg}")),
            }
        }

        let command = args.collect::<Vec<_>>();
        if !command.is_empty() {
            cli.command = Some(parse_control_command(&command.join(" "))?);
        }

        if cli.service && cli.command.is_some() {
            return Err("--service cannot be combined with a one-shot command".to_string());
        }

        Ok(cli)
    }
}

fn default_socket_path() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("rc-ble-controller.sock")
}

fn next_arg(
    args: &mut impl Iterator<Item = String>,
    option: &str,
) -> std::result::Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{option} requires a value"))
}

fn print_usage() {
    eprintln!(
        "Usage:
  cargo run -- [--addr EF:0C:F2:A2:D0:11]
  cargo run -- [--addr EF:0C:F2:A2:D0:11] <command>
  cargo run -- --service [--socket /tmp/rc-ble-controller.sock]

Options:
  --addr <mac>             Connect to this BLE address instead of the first RcCar by name
  --scan-seconds <seconds> Scan timeout, default 5
  --service                Run as a Linux Unix-socket service
  --socket <path>          Service socket path
  --auto-stop-seconds <n>  Send motor stop after n seconds without commands, default 3; 0 disables

{}",
        rc_ble_controller::COMMANDS
    );
}

fn print_interactive_menu() {
    println!("{COMMAND_MENU}");
}
