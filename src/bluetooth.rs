use crate::{
    big_led::{BIG_LEDS_CHANNEL, BigLedCommand},
    bottom_led::{BOTTOM_LEDS_CHANNEL, BottomLedCommand},
    motor::{MOTORS_CHANNEL, MotorCommand},
    servo::{SERVO_CHANNEL, ServoCommand, ServoDirection},
};
use defmt::{debug, info, unwrap};
use nrf_softdevice::{
    Softdevice,
    ble::{
        SecurityMode, Uuid,
        advertisement_builder::{
            Flag, LegacyAdvertisementBuilder, LegacyAdvertisementPayload, ServiceList,
        },
        gatt_server::{
            self, CharacteristicHandles, RegisterError, Service,
            builder::ServiceBuilder,
            characteristic::{Attribute, AttributeMetadata, Metadata, Properties, UserDescription},
        },
        peripheral,
    },
    raw,
};

const RC_SERVICE_UUID: [u8; 16] = 0xa1a20000_b1b2_c1c2_d1d2_e1e2e3e4e500_u128.to_le_bytes();
const MOTOR_UUID: [u8; 16] = 0xa1a20000_b1b2_c1c2_d1d2_e1e2e3e4e501_u128.to_le_bytes();
const SERVO_UUID: [u8; 16] = 0xa1a20000_b1b2_c1c2_d1d2_e1e2e3e4e502_u128.to_le_bytes();
const BIG_LED_UUID: [u8; 16] = 0xa1a20000_b1b2_c1c2_d1d2_e1e2e3e4e503_u128.to_le_bytes();
const BOTTOM_LED_UUID: [u8; 16] = 0xa1a20000_b1b2_c1c2_d1d2_e1e2e3e4e504_u128.to_le_bytes();
const ANALOG_OPCODE: u8 = 0x10;
const DRIVE_OPCODE: u8 = 0x11;
const ANALOG_MIN: i8 = -100;
const ANALOG_MAX: i8 = 100;

pub(crate) struct RcCarService {
    motor_value_handle: u16,
    servo_value_handle: u16,
    big_led_value_handle: u16,
    bottom_led_value_handle: u16,
}

pub(crate) enum RcCarServiceEvent {
    Motor(MotorCommand),
    Servo(ServoCommand),
    BigLed(u8),
    BottomLed(u8),
}

impl RcCarService {
    fn new(sd: &mut Softdevice) -> Result<Self, RegisterError> {
        let mut service_builder = ServiceBuilder::new(sd, Uuid::new_128(&RC_SERVICE_UUID))?;
        let motor = add_write_characteristic(&mut service_builder, MOTOR_UUID, b"Motor")?;
        let servo = add_write_characteristic(&mut service_builder, SERVO_UUID, b"Servo")?;
        let big_led = add_write_characteristic(&mut service_builder, BIG_LED_UUID, b"Front LEDs")?;
        let bottom_led =
            add_write_characteristic(&mut service_builder, BOTTOM_LED_UUID, b"Bottom LEDs")?;

        let _service_handle = service_builder.build();

        Ok(Self {
            motor_value_handle: motor.value_handle,
            servo_value_handle: servo.value_handle,
            big_led_value_handle: big_led.value_handle,
            bottom_led_value_handle: bottom_led.value_handle,
        })
    }
}

impl Service for RcCarService {
    type Event = RcCarServiceEvent;

    fn on_write(&self, handle: u16, data: &[u8]) -> Option<Self::Event> {
        if handle == self.motor_value_handle {
            parse_motor_write(data).map(RcCarServiceEvent::Motor)
        } else if handle == self.servo_value_handle {
            parse_servo_write(data).map(RcCarServiceEvent::Servo)
        } else if handle == self.big_led_value_handle {
            let command = data.first().copied()?;
            Some(RcCarServiceEvent::BigLed(command))
        } else if handle == self.bottom_led_value_handle {
            let command = data.first().copied()?;
            Some(RcCarServiceEvent::BottomLed(command))
        } else {
            None
        }
    }
}

fn add_write_characteristic(
    service_builder: &mut ServiceBuilder<'_>,
    uuid: [u8; 16],
    description: &'static [u8],
) -> Result<CharacteristicHandles, RegisterError> {
    let value = [0u8];
    let attr = Attribute::new(value).variable_len(3);
    let mut metadata = Metadata::new(Properties::new().write());
    metadata.user_description = Some(UserDescription {
        metadata: Some(AttributeMetadata {
            read: SecurityMode::Open,
            write: SecurityMode::NoAccess,
            ..Default::default()
        }),
        value: description,
        max_len: description.len() as u16,
    });

    Ok(service_builder
        .add_characteristic(Uuid::new_128(&uuid), attr, metadata)?
        .build())
}

#[nrf_softdevice::gatt_server]
pub(crate) struct Server {
    rc: RcCarService,
}

static ADV_DATA: LegacyAdvertisementPayload = LegacyAdvertisementBuilder::new()
    .flags(&[Flag::GeneralDiscovery, Flag::LE_Only])
    .full_name("RcCar")
    .build();

static SCAN_DATA: LegacyAdvertisementPayload = LegacyAdvertisementBuilder::new()
    .services_128(ServiceList::Complete, &[RC_SERVICE_UUID])
    .build();

pub(crate) fn enable_softdevice() -> (&'static mut Softdevice, Server) {
    let config = nrf_softdevice::Config {
        clock: Some(raw::nrf_clock_lf_cfg_t {
            source: raw::NRF_CLOCK_LF_SRC_RC as u8,
            rc_ctiv: 16,
            rc_temp_ctiv: 2,
            accuracy: raw::NRF_CLOCK_LF_ACCURACY_500_PPM as u8,
        }),
        conn_gap: Some(raw::ble_gap_conn_cfg_t {
            conn_count: 1,
            event_length: 24,
        }),
        conn_gatt: Some(raw::ble_gatt_conn_cfg_t { att_mtu: 64 }),
        gatts_attr_tab_size: Some(raw::ble_gatts_cfg_attr_tab_size_t {
            attr_tab_size: raw::BLE_GATTS_ATTR_TAB_SIZE_DEFAULT,
        }),
        gap_role_count: Some(raw::ble_gap_cfg_role_count_t {
            adv_set_count: 1,
            periph_role_count: 1,
            central_role_count: 0,
            central_sec_count: 0,
            _bitfield_1: raw::ble_gap_cfg_role_count_t::new_bitfield_1(0),
        }),
        gap_device_name: Some(raw::ble_gap_cfg_device_name_t {
            p_value: b"RcCar" as *const u8 as _,
            current_len: 5,
            max_len: 5,
            write_perm: SecurityMode::NoAccess.into_raw(),
            _bitfield_1: raw::ble_gap_cfg_device_name_t::new_bitfield_1(
                raw::BLE_GATTS_VLOC_STACK as u8,
            ),
        }),
        ..Default::default()
    };

    let sd = Softdevice::enable(&config);
    let server = unwrap!(Server::new(sd));
    (sd, server)
}

#[embassy_executor::task]
pub(crate) async fn softdevice_task(sd: &'static Softdevice) -> ! {
    sd.run().await
}

#[embassy_executor::task]
pub(crate) async fn bluetooth_task(sd: &'static Softdevice, server: Server) -> ! {
    info!("BLE advertising as RcCar");

    loop {
        let adv = peripheral::ConnectableAdvertisement::ScannableUndirected {
            adv_data: &ADV_DATA,
            scan_data: &SCAN_DATA,
        };
        let conn = unwrap!(
            peripheral::advertise_connectable(sd, adv, &peripheral::Config::default()).await
        );
        info!("BLE client connected");

        let _ = gatt_server::run(&conn, &server, |event| match event {
            ServerEvent::Rc(event) => match event {
                RcCarServiceEvent::Motor(command) => handle_motor_command(command),
                RcCarServiceEvent::Servo(command) => handle_servo_command(command),
                RcCarServiceEvent::BigLed(command) => handle_big_led_command(command),
                RcCarServiceEvent::BottomLed(command) => handle_bottom_led_command(command),
            },
        })
        .await;

        MOTORS_CHANNEL.send(MotorCommand::Stop).await;
        SERVO_CHANNEL.send(ServoCommand::SetPosition(0)).await;
        info!("BLE client disconnected");
    }
}

fn parse_motor_write(data: &[u8]) -> Option<MotorCommand> {
    match data {
        [0x00] => Some(MotorCommand::Stop),
        [0x01] => Some(MotorCommand::Forward),
        [0x02] => Some(MotorCommand::Backward),
        [0x03] => Some(MotorCommand::Left),
        [0x04] => Some(MotorCommand::Right),
        [ANALOG_OPCODE, value] => decode_percent(*value).map(MotorCommand::Throttle),
        [DRIVE_OPCODE, throttle, steering] => Some(MotorCommand::Drive {
            throttle: decode_percent(*throttle)?,
            steering: decode_percent(*steering)?,
        }),
        _ => None,
    }
}

fn parse_servo_write(data: &[u8]) -> Option<ServoCommand> {
    match data {
        [0x00] => Some(ServoCommand::SetDirection(ServoDirection::Right)),
        [0x01] => Some(ServoCommand::SetDirection(ServoDirection::RightFront)),
        [0x02] => Some(ServoCommand::SetDirection(ServoDirection::Front)),
        [0x03] => Some(ServoCommand::SetDirection(ServoDirection::LeftFront)),
        [0x04] => Some(ServoCommand::SetDirection(ServoDirection::Left)),
        [ANALOG_OPCODE, value] => decode_percent(*value).map(ServoCommand::SetPosition),
        _ => None,
    }
}

fn decode_percent(value: u8) -> Option<i8> {
    let value = value as i8;
    if value < ANALOG_MIN || value > ANALOG_MAX {
        None
    } else {
        Some(value)
    }
}

fn handle_motor_command(command: MotorCommand) {
    match MOTORS_CHANNEL.try_send(command) {
        Ok(()) => debug!("BLE motor command"),
        Err(_) => debug!("BLE motor channel full"),
    }
}

fn handle_big_led_command(command: u8) {
    if command == 0x01 {
        match BIG_LEDS_CHANNEL.try_send(BigLedCommand::Toggle) {
            Ok(()) => debug!("BLE big LED toggle"),
            Err(_) => debug!("BLE big LED channel full"),
        }
    } else {
        debug!("Unknown BLE big LED command {}", command);
    }
}

fn handle_servo_command(command: ServoCommand) {
    match SERVO_CHANNEL.try_send(command) {
        Ok(()) => debug!("BLE servo command"),
        Err(_) => debug!("BLE servo channel full"),
    }
}

fn handle_bottom_led_command(command: u8) {
    let led_command = match command {
        0x00 => Some(BottomLedCommand::AllOff),
        0x01 => Some(BottomLedCommand::AllOn),
        _ => None,
    };

    if let Some(led_command) = led_command {
        match BOTTOM_LEDS_CHANNEL.try_send(led_command) {
            Ok(()) => debug!("BLE bottom LED command {}", command),
            Err(_) => debug!("BLE bottom LED channel full"),
        }
    } else {
        debug!("Unknown BLE bottom LED command {}", command);
    }
}
