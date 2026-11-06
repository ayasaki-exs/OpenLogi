//! Darmoshark (MOJI) vendor HID protocol codec — pure data, no I/O.
//!
//! The Darmoshark launcher talks to its mice through a 64-byte non-numbered
//! input/output report pair on the vendor collection `0xff0a:0x0001`. Every
//! command is a 64-byte output report:
//!
//! ```text
//! [0]      operation type (0..63); bit 6 (0x40) = 2.4 GHz work mode
//! [1]      packet index / sub-index
//! [2]      command opcode
//! [3]      sub-command
//! [4..62]  payload
//! [63]     checksum (the whole report sums to 0xa1 mod 256)
//! ```
//!
//! Responses are 64-byte input reports. The device echoes the operation type
//! (including the work-mode bit), so callers match `resp[0] & 0x3f` and
//! `resp[3]`.
//!
//! This module owns the wire encoding only. Opening the interface, retries and
//! timeouts live in the (not yet written) transport driver; identity matching
//! lives in [`openlogi_device_registry::darmoshark`].

/// Length of every Darmoshark configuration report.
pub const REPORT_LEN: usize = 64;

/// Checksum target: the whole 64-byte report sums to this value mod 256.
pub const CHECKSUM_TARGET: u8 = 0xa1;

/// Bit 6 of `bytes[0]` marks a command sent through the 2.4 GHz receiver.
pub const WIRELESS_FLAG: u8 = 0x40;

/// Report-rate presets in hertz, indexed 0..=5.
pub const REPORT_RATES: [u16; 6] = [125, 500, 1000, 2000, 4000, 8000];

/// Operation types carried in `bytes[0]` (masked with `0x3f`).
pub mod ty {
    /// Firmware version read.
    pub const GET_VERSION: u8 = 0;
    /// Battery / power state read.
    pub const GET_POWER: u8 = 1;
    /// Profile switch.
    pub const SWITCH_PROFILE: u8 = 2;
    /// Button configuration.
    pub const BUTTON: u8 = 3;
    /// Sensor block: base info, DPI, report rate, lift-off distance.
    pub const SENSOR: u8 = 4;
    /// Lighting.
    pub const LIGHT: u8 = 5;
    /// Macro configuration (observed, not implemented).
    pub const MACRO: u8 = 7;
    /// Recovery / factory reset.
    pub const RECOVERY: u8 = 9;
    /// Persist settings to flash.
    pub const SAVE: u8 = 10;
}

/// Command opcodes carried in `bytes[2]`.
pub mod op {
    /// Base information: version, power, base config, light, save.
    pub const BASE_INFO: u8 = 129;
    /// Button configuration and profile switch.
    pub const BUTTON_CFG: u8 = 130;
    /// Macro configuration (observed, not implemented).
    pub const MACRO: u8 = 131;
    /// Sensor settings: current DPI, LOD, motion flags.
    pub const SENSOR_SET: u8 = 132;
    /// Debounce / sleep / button response time (observed, not implemented).
    pub const DEBOUNCE: u8 = 134;
    /// Bind a macro to a button (observed, not implemented).
    pub const MACRO_BIND: u8 = 136;
    /// Firmware update request (observed, not implemented).
    pub const FIRMWARE: u8 = 139;
    /// Lighting write.
    pub const LIGHT_SET: u8 = 145;
    /// Full DPI / report-rate / LOD write.
    pub const DPI_FULL_SET: u8 = 181;
}

/// Work mode: wired USB or 2.4 GHz through the receiver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkMode {
    /// Wired USB.
    Usb,
    /// 2.4 GHz receiver.
    Wireless,
}

impl WorkMode {
    /// The bit this mode adds to `bytes[0]`.
    #[must_use]
    pub const fn bit(self) -> u8 {
        match self {
            Self::Usb => 0,
            Self::Wireless => WIRELESS_FLAG,
        }
    }
}

/// Protocol contract detected from the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contract {
    /// Legacy `dms` layout.
    Dms,
    /// Newer `dms_v2` layout.
    DmsV2,
}

impl Contract {
    /// Detects the contract from a `[1, 0, 129, 1]` response.
    #[must_use]
    pub const fn detect(resp: &[u8; REPORT_LEN]) -> Self {
        if resp[6] == 0x15 && resp[7] == 0x19 {
            Self::DmsV2
        } else {
            Self::Dms
        }
    }
}

/// A 64-byte command report under construction.
#[derive(Debug, Clone)]
pub struct Report {
    buf: [u8; REPORT_LEN],
}

impl Default for Report {
    fn default() -> Self {
        Self {
            buf: [0; REPORT_LEN],
        }
    }
}

impl Report {
    /// Starts a report with `bytes[0] = ty`, `bytes[2] = cmd`, `bytes[3] = sub`.
    #[must_use]
    pub const fn new(ty: u8, sub: u8, cmd: u8) -> Self {
        let mut report = Self {
            buf: [0; REPORT_LEN],
        };
        report.buf[0] = ty & 0x3f;
        report.buf[2] = cmd;
        report.buf[3] = sub;
        report
    }

    /// Sets one payload byte.
    ///
    /// # Panics
    ///
    /// Panics when `index` is not inside the report. All builders in this
    /// module use fixed, in-range offsets.
    #[must_use]
    pub fn byte(mut self, index: usize, value: u8) -> Self {
        self.buf[index] = value;
        self
    }

    /// Writes a little-endian `u16` at `index`.
    ///
    /// # Panics
    ///
    /// Panics when `index + 1` is not inside the report. All builders in this
    /// module use fixed, in-range offsets.
    #[must_use]
    pub fn u16_le(mut self, index: usize, value: u16) -> Self {
        let bytes = value.to_le_bytes();
        self.buf[index] = bytes[0];
        self.buf[index + 1] = bytes[1];
        self
    }

    /// Copies `data` at `index`.
    ///
    /// # Panics
    ///
    /// Panics when `data` does not fit at `index`. Callers cap `data` at the
    /// button payload width before calling this.
    #[must_use]
    pub fn bytes(mut self, index: usize, data: &[u8]) -> Self {
        self.buf[index..index + data.len()].copy_from_slice(data);
        self
    }

    /// Applies the work-mode bit and the checksum, returning the wire bytes.
    #[must_use]
    pub fn finish(mut self, mode: WorkMode) -> [u8; REPORT_LEN] {
        self.buf[0] |= mode.bit();
        self.checksum();
        self.buf
    }

    fn checksum(&mut self) {
        let sum = self.buf[..REPORT_LEN - 1]
            .iter()
            .fold(0_u32, |acc, &byte| acc + u32::from(byte));
        let low = u8::try_from(sum & 0xff).unwrap_or(0);
        self.buf[REPORT_LEN - 1] = CHECKSUM_TARGET.wrapping_sub(low);
    }
}

/// Sensor/motion feature bits from the base-info response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SensorFlags(u8);

impl SensorFlags {
    /// Builds the flag set from its wire byte.
    #[must_use]
    pub const fn from_byte(value: u8) -> Self {
        Self(value)
    }

    /// Returns the wire byte.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        self.0
    }

    /// Bit 0: line-mode sensor.
    #[must_use]
    pub const fn line(self) -> bool {
        self.0 & 0x01 != 0
    }

    /// Bit 1: high-performance sensor.
    #[must_use]
    pub const fn sensor(self) -> bool {
        self.0 & 0x02 != 0
    }

    /// Bit 4: wave mode.
    #[must_use]
    pub const fn wave(self) -> bool {
        self.0 & 0x10 != 0
    }

    /// Bit 5: motion sync.
    #[must_use]
    pub const fn motion(self) -> bool {
        self.0 & 0x20 != 0
    }

    /// Bit 6: esports mode.
    #[must_use]
    pub const fn esports(self) -> bool {
        self.0 & 0x40 != 0
    }

    /// Bit 7: scroll mode.
    #[must_use]
    pub const fn scroll(self) -> bool {
        self.0 & 0x80 != 0
    }
}

/// Firmware versions decoded from the version response.
///
/// Each triple is `[major, minor, patch]`, formatted the way the launcher does
/// (`hi >> 4`, `hi & 0x0f`, `lo & 0x0f`). `mcu2` is `[0, 0, 0]` when the
/// device reports none.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Version {
    /// Main firmware version.
    pub firmware: [u8; 3],
    /// Secondary MCU version.
    pub mcu2: [u8; 3],
}

/// Battery / power state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Power {
    /// Charging state byte, as reported.
    pub state: u8,
    /// Battery percentage, 0..=100.
    pub percent: u8,
    /// Active onboard profile.
    pub profile: u8,
    /// Highest report-rate index the current connection supports.
    pub report_rate_max: u8,
}

/// Full base configuration read back from the device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaseInfo {
    /// DPI pairs: slots 0..5 hold X, slots 5..10 hold the matching Y. A
    /// `dms` base-info response carries five such pairs (ten `u16` values)
    /// starting at byte 8. For `dms_v2`, only slots 0..5 are populated; the
    /// Y-axis candidate overlaps the report-rate byte and is not modelled.
    pub dpi: [u16; 10],
    /// Number of enabled DPI levels.
    pub level_count: u8,
    /// Current report-rate index into [`REPORT_RATES`].
    pub report_rate: u8,
    /// Current DPI-level index.
    pub current_dpi: u8,
    /// Lift-off distance index (0 low, 1 medium, 2 high).
    pub lod: u8,
    /// Snap angle in degrees, signed.
    pub angle: i8,
    /// Sensor feature bits.
    pub sensor: SensorFlags,
    /// Sleep timeout in milliseconds.
    pub sleep_ms: u16,
    /// Button response delay.
    pub delay: u8,
}

/// Lighting state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Light {
    /// Mode: 1 static, 2 breathing, 3 color-cycle.
    pub mode: u8,
    /// Brightness.
    pub brightness: u8,
    /// Animation speed.
    pub speed: u8,
    /// Red, green and blue components.
    pub rgb: [u8; 3],
}

/// Encodes a report-rate index the way the device stores it (`index + 1`).
#[must_use]
pub const fn encode_report_rate(index: u8) -> u8 {
    index.saturating_add(1)
}

/// Decodes a stored report-rate value back to an index into [`REPORT_RATES`].
#[must_use]
pub const fn decode_report_rate(stored: u8) -> u8 {
    stored.saturating_sub(1)
}

/// Bit mask of enabled DPI levels for a level count of 1..=5.
#[must_use]
pub const fn level_mask(levels: u8) -> u8 {
    match levels {
        1 => 0b0_0001,
        2 => 0b0_0011,
        3 => 0b0_0111,
        4 => 0b0_1111,
        _ => 0b1_1111,
    }
}

fn version_triple(hi: u8, lo: u8) -> [u8; 3] {
    [hi >> 4, hi & 0x0f, lo & 0x0f]
}

/// Parses the firmware-version response.
#[must_use]
pub fn parse_version(resp: &[u8; REPORT_LEN]) -> Version {
    Version {
        firmware: version_triple(resp[8], resp[9]),
        mcu2: version_triple(resp[20], resp[21]),
    }
}

/// Parses the power response for `contract`.
#[must_use]
pub fn parse_power(resp: &[u8; REPORT_LEN], contract: Contract) -> Power {
    match contract {
        Contract::Dms => Power {
            state: resp[5],
            percent: resp[6],
            profile: resp[7],
            report_rate_max: resp[10],
        },
        Contract::DmsV2 => Power {
            state: resp[10],
            percent: resp[11],
            profile: resp[12],
            report_rate_max: resp[5],
        },
    }
}

/// Parses the base-info response for `contract`.
///
/// The `dms_v2` interpretation of `current_dpi` comes from the Python
/// reference implementation. Its DPI Y axis is not modelled: the candidate
/// Y range (`resp[17..19]` for slot 0) overlaps the report-rate byte at
/// `resp[18]`, so the two cannot both be true. Its LOD, angle, sleep and delay
/// offsets are also unknown and stay zero instead of being guessed.
#[must_use]
pub fn parse_base_info(resp: &[u8; REPORT_LEN], contract: Contract) -> BaseInfo {
    match contract {
        Contract::Dms => {
            let mut info = BaseInfo::default();
            // Five X/Y pairs, each four little-endian bytes: X at 8 + 4*i,
            // Y at 10 + 4*i. The launcher groups these ten u16 values into
            // five DPI pairs before editing them.
            for slot in 0..5 {
                let base = 8 + 4 * slot;
                info.dpi[slot] = u16::from_le_bytes([resp[base], resp[base + 1]]);
                info.dpi[slot + 5] = u16::from_le_bytes([resp[base + 2], resp[base + 3]]);
            }
            info.level_count = u8::try_from(resp[6].count_ones()).unwrap_or(0);
            info.report_rate = decode_report_rate(resp[5]);
            info.current_dpi = resp[7];
            info.lod = resp[43];
            info.angle = resp[44].cast_signed();
            info.sensor = SensorFlags::from_byte(resp[4]);
            info.sleep_ms = u16::from_le_bytes([resp[52], resp[53]]);
            info.delay = resp[54];
            info
        }
        Contract::DmsV2 => {
            let mut info = BaseInfo::default();
            for slot in 0..5 {
                let base = 15 + 4 * slot;
                info.dpi[slot] = u16::from_le_bytes([resp[base], resp[base + 1]]);
            }
            info.level_count = 5;
            info.report_rate = decode_report_rate(resp[18]);
            info.current_dpi = resp[13];
            info.sensor = SensorFlags::from_byte(resp[4]);
            info
        }
    }
}

/// Parses the lighting response.
#[must_use]
pub fn parse_light(resp: &[u8; REPORT_LEN]) -> Light {
    let mode = resp[5];
    let (red, green, blue) = match mode {
        1 => (11, 12, 13),
        2 => (14, 15, 16),
        _ => (17, 18, 19),
    };
    Light {
        mode,
        brightness: resp[7],
        speed: resp[9],
        rgb: [resp[red], resp[green], resp[blue]],
    }
}

/// `[0, 0, 129, 0]` — firmware version.
#[must_use]
pub fn cmd_version(mode: WorkMode) -> [u8; REPORT_LEN] {
    Report::new(ty::GET_VERSION, 0, op::BASE_INFO).finish(mode)
}

/// `[1, 0, 129, 1]` — power / handshake.
#[must_use]
pub fn cmd_power(mode: WorkMode) -> [u8; REPORT_LEN] {
    Report::new(ty::GET_POWER, 1, op::BASE_INFO).finish(mode)
}

/// `[4, 0, 129, 1]` — full base configuration.
#[must_use]
pub fn cmd_base_info(mode: WorkMode) -> [u8; REPORT_LEN] {
    Report::new(ty::SENSOR, 1, op::BASE_INFO).finish(mode)
}

/// `[5, 0, 129, 1]` — lighting state.
#[must_use]
pub fn cmd_light_get(mode: WorkMode) -> [u8; REPORT_LEN] {
    Report::new(ty::LIGHT, 1, op::BASE_INFO).finish(mode)
}

/// `[3, 0, 130, 3, button]` — button configuration read.
#[must_use]
pub fn cmd_button_get(mode: WorkMode, button: u8) -> [u8; REPORT_LEN] {
    Report::new(ty::BUTTON, 3, op::BUTTON_CFG)
        .byte(4, button)
        .finish(mode)
}

/// `[3, 0, 128 | len + 1, 4, button, data..]` — button configuration write.
///
/// # Panics
///
/// Panics when `data` is longer than 58 bytes, the space between byte 5 and
/// the checksum.
#[must_use]
pub fn cmd_button_set(mode: WorkMode, button: u8, data: &[u8]) -> [u8; REPORT_LEN] {
    let payload_len = u8::try_from(data.len()).unwrap_or(u8::MAX);
    Report::new(ty::BUTTON, 4, 0x80 | payload_len.saturating_add(1))
        .byte(4, button)
        .bytes(5, data)
        .finish(mode)
}

/// `[4, 0, 181, 2, ...]` — full DPI / report-rate / LOD write.
///
/// The device applies the whole sensor block at once, so callers read the
/// current values first and put back every field they are not changing.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "the wire block has this many independent fields; a params struct would only rename them"
)]
pub fn cmd_dpi_full(
    mode: WorkMode,
    sensor: SensorFlags,
    report_rate: u8,
    level_count: u8,
    current_dpi: u8,
    dpi: &[(u16, u16)],
    lod: u8,
    angle: i8,
    sleep_ms: u16,
    delay: u8,
) -> [u8; REPORT_LEN] {
    let mut report = Report::new(ty::SENSOR, 2, op::DPI_FULL_SET)
        .byte(4, sensor.to_byte())
        .byte(5, encode_report_rate(report_rate))
        .byte(6, level_mask(level_count))
        .byte(7, current_dpi);
    for (index, &(x, y)) in dpi.iter().take(10).enumerate() {
        let base = 8 + 4 * index;
        report = report.u16_le(base, x).u16_le(base + 2, y);
    }
    report
        .byte(43, lod)
        .byte(44, angle.cast_unsigned())
        .u16_le(50, sleep_ms)
        .u16_le(52, sleep_ms)
        .byte(54, delay)
        .finish(mode)
}

/// `[4, 0, 132, 4, level]` — set the current DPI level.
#[must_use]
pub fn cmd_dpi_current(mode: WorkMode, level: u8) -> [u8; REPORT_LEN] {
    Report::new(ty::SENSOR, 4, op::SENSOR_SET)
        .byte(
            4,
            if level > 0 {
                level.saturating_add(1)
            } else {
                0
            },
        )
        .finish(mode)
}

/// `[4, 0, 132, 4, rate]` — set the report-rate index.
///
/// The launcher uses the same type/opcode/sub-command as [`cmd_dpi_current`];
/// the payload is the zero-based rate index stored as `index + 1` (0 for the
/// default 125 Hz).
#[must_use]
pub fn cmd_report_rate(mode: WorkMode, index: u8) -> [u8; REPORT_LEN] {
    Report::new(ty::SENSOR, 4, op::SENSOR_SET)
        .byte(
            4,
            if index > 0 {
                index.saturating_add(1)
            } else {
                0
            },
        )
        .finish(mode)
}

/// `[4, 0, 132, 10, flags, lod, angle]` — LOD / motion flags / angle.
#[must_use]
pub fn cmd_lod(mode: WorkMode, sensor: SensorFlags, lod: u8, angle: i8) -> [u8; REPORT_LEN] {
    Report::new(ty::SENSOR, 10, op::SENSOR_SET)
        .byte(4, sensor.to_byte())
        .byte(5, lod)
        .byte(6, angle.cast_unsigned())
        .finish(mode)
}

/// `[5, 0, 145, 2, ...]` — lighting write.
#[must_use]
pub fn cmd_light_set(
    mode: WorkMode,
    light_mode: u8,
    brightness: u8,
    speed: u8,
    rgb: [u8; 3],
) -> [u8; REPORT_LEN] {
    Report::new(ty::LIGHT, 2, op::LIGHT_SET)
        .byte(4, 1)
        .byte(5, light_mode)
        .byte(6, 7)
        .byte(7, brightness)
        .byte(8, 1)
        .byte(9, speed)
        .byte(10, 1)
        .bytes(11, &rgb)
        .bytes(14, &rgb)
        .bytes(17, &rgb)
        .finish(mode)
}

/// `[2, 0, 130, 1, profile]` — switch profile.
#[must_use]
pub fn cmd_profile(mode: WorkMode, profile: u8) -> [u8; REPORT_LEN] {
    Report::new(ty::SWITCH_PROFILE, 1, op::BUTTON_CFG)
        .byte(4, profile)
        .finish(mode)
}

/// `[10, 0, 129, 1]` — persist settings to flash.
#[must_use]
pub fn cmd_save(mode: WorkMode) -> [u8; REPORT_LEN] {
    Report::new(ty::SAVE, 1, op::BASE_INFO).finish(mode)
}

/// `[9, 0, 129|130, tag, value]` — recovery / factory reset.
///
/// The opcode choice and the factory-reset tag (`255`) are reverse-engineered
/// from the launcher; this command is not exercised by OpenLogi yet.
#[must_use]
pub fn cmd_recovery(mode: WorkMode, tag: u8, value: u8) -> [u8; REPORT_LEN] {
    Report::new(
        ty::RECOVERY,
        tag,
        if value != 0 {
            op::BUTTON_CFG
        } else {
            op::BASE_INFO
        },
    )
    .byte(4, value)
    .finish(mode)
}

#[cfg(test)]
mod tests {
    use super::{
        CHECKSUM_TARGET, Contract, Light, Power, REPORT_LEN, Report, SensorFlags, Version,
        WorkMode, cmd_dpi_full, cmd_light_set, cmd_power, cmd_report_rate, cmd_save,
        decode_report_rate, encode_report_rate, level_mask, parse_base_info, parse_light,
        parse_power, parse_version, ty,
    };

    fn sum(report: &[u8; REPORT_LEN]) -> u32 {
        report.iter().map(|&byte| u32::from(byte)).sum()
    }

    #[test]
    fn handshake_checksum_is_the_one_the_launcher_sends() {
        let report = cmd_power(WorkMode::Usb);
        assert_eq!(sum(&report) % 256, u32::from(CHECKSUM_TARGET));
        assert_eq!(&report[..4], &[1, 0, 129, 1]);
        assert_eq!(report[63], 30);
    }

    #[test]
    fn wireless_mode_sets_bit_six() {
        let report = cmd_power(WorkMode::Wireless);
        assert_eq!(report[0], 1 | 0x40);
        assert_eq!(sum(&report) % 256, u32::from(CHECKSUM_TARGET));
    }

    #[test]
    fn save_is_the_expected_fixed_report() {
        let report = cmd_save(WorkMode::Usb);
        assert_eq!(&report[..4], &[10, 0, 129, 1]);
        assert_eq!(sum(&report) % 256, u32::from(CHECKSUM_TARGET));
    }

    #[test]
    fn static_red_lighting_write_puts_rgb_in_all_three_slots() {
        let report = cmd_light_set(WorkMode::Usb, 1, 100, 3, [255, 0, 0]);
        assert_eq!(&report[..5], &[5, 0, 145, 2, 1]);
        assert_eq!(report[5], 1);
        assert_eq!(report[7], 100);
        assert_eq!(report[9], 3);
        assert_eq!(&report[11..20], &[255, 0, 0, 255, 0, 0, 255, 0, 0]);
        assert_eq!(sum(&report) % 256, u32::from(CHECKSUM_TARGET));
    }

    #[test]
    fn dpi_full_writes_every_slot_and_the_sensor_block() {
        let report = cmd_dpi_full(
            WorkMode::Usb,
            SensorFlags::from_byte(0x02),
            2,
            5,
            0,
            &[
                (400, 400),
                (800, 800),
                (1600, 1600),
                (3200, 3200),
                (4800, 4800),
            ],
            1,
            0,
            300,
            4,
        );
        assert_eq!(&report[..4], &[4, 0, 181, 2]);
        assert_eq!(report[4], 0x02);
        assert_eq!(report[5], 3);
        assert_eq!(report[6], 0b1_1111);
        assert_eq!(report[7], 0);
        assert_eq!(&report[8..12], &[0x90, 0x01, 0x90, 0x01]);
        assert_eq!(report[43], 1);
        assert_eq!(report[44], 0);
        assert_eq!(&report[50..54], &[0x2c, 0x01, 0x2c, 0x01]);
        assert_eq!(report[54], 4);
        assert_eq!(sum(&report) % 256, u32::from(CHECKSUM_TARGET));
    }

    #[test]
    fn report_rate_write_uses_the_short_form() {
        let report = cmd_report_rate(WorkMode::Usb, 2);
        assert_eq!(&report[..5], &[4, 0, 132, 4, 3]);
        assert_eq!(sum(&report) % 256, u32::from(CHECKSUM_TARGET));
        let default = cmd_report_rate(WorkMode::Usb, 0);
        assert_eq!(&default[..5], &[4, 0, 132, 4, 0]);
    }

    #[test]
    fn report_rate_roundtrips_and_decodes_the_default_zero() {
        for index in 0..6 {
            assert_eq!(decode_report_rate(encode_report_rate(index)), index);
        }
        assert_eq!(decode_report_rate(0), 0);
        assert_eq!(decode_report_rate(1), 0);
    }

    #[test]
    fn level_masks_match_the_enabled_gears() {
        assert_eq!(level_mask(1), 1);
        assert_eq!(level_mask(2), 3);
        assert_eq!(level_mask(3), 7);
        assert_eq!(level_mask(4), 15);
        assert_eq!(level_mask(5), 31);
    }

    #[test]
    fn version_nibbles_decode() {
        let mut resp = [0_u8; REPORT_LEN];
        resp[8] = 0x12;
        resp[9] = 0x03;
        resp[20] = 0x01;
        resp[21] = 0x02;
        assert_eq!(
            parse_version(&resp),
            Version {
                firmware: [1, 2, 3],
                mcu2: [0, 1, 2],
            }
        );
    }

    #[test]
    fn power_layouts_differ_between_contracts() {
        let mut resp = [0_u8; REPORT_LEN];
        resp[5] = 1;
        resp[6] = 42;
        resp[7] = 2;
        resp[10] = 3;
        assert_eq!(
            parse_power(&resp, Contract::Dms),
            Power {
                state: 1,
                percent: 42,
                profile: 2,
                report_rate_max: 3,
            }
        );
        assert_eq!(
            parse_power(&resp, Contract::DmsV2),
            Power {
                state: 3,
                percent: 0,
                profile: 0,
                report_rate_max: 1,
            }
        );
    }

    #[test]
    fn base_info_dms_decodes_offsets() {
        let mut resp = [0_u8; REPORT_LEN];
        resp[4] = 0x02;
        resp[5] = 3;
        resp[6] = 0b1_1111;
        resp[7] = 1;
        resp[8..12].copy_from_slice(&[0x90, 0x01, 0x2c, 0x01]); // X0=400, Y0=300
        resp[12..16].copy_from_slice(&[0x20, 0x03, 0x20, 0x03]); // X1=800, Y1=800
        resp[43] = 2;
        resp[44] = 0xe2;
        resp[52] = 0x2c;
        resp[53] = 0x01;
        resp[54] = 4;
        let info = parse_base_info(&resp, Contract::Dms);
        assert_eq!(info.dpi[0], 400);
        assert_eq!(info.dpi[5], 300);
        assert_eq!(info.dpi[1], 800);
        assert_eq!(info.dpi[6], 800);
        assert_eq!(info.level_count, 5);
        assert_eq!(info.report_rate, 2);
        assert_eq!(info.current_dpi, 1);
        assert_eq!(info.lod, 2);
        assert_eq!(info.angle, -30);
        assert_eq!(info.sensor, SensorFlags::from_byte(0x02));
        assert_eq!(info.sleep_ms, 300);
        assert_eq!(info.delay, 4);
    }

    #[test]
    fn base_info_v2_reads_x_and_report_rate_without_overlap() {
        let mut resp = [0_u8; REPORT_LEN];
        resp[13] = 2;
        resp[15..17].copy_from_slice(&[0x90, 0x01]);
        resp[18] = 3;
        let info = parse_base_info(&resp, Contract::DmsV2);
        assert_eq!(info.dpi[0], 400);
        assert_eq!(info.dpi[5], 0, "the conflicting Y axis is not modelled");
        assert_eq!(info.level_count, 5);
        assert_eq!(info.report_rate, 2);
        assert_eq!(info.current_dpi, 2);
    }

    #[test]
    fn lighting_rgb_offsets_follow_the_mode() {
        let mut resp = [0_u8; REPORT_LEN];
        resp[5] = 2;
        resp[7] = 80;
        resp[9] = 4;
        resp[14] = 1;
        resp[15] = 2;
        resp[16] = 3;
        assert_eq!(
            parse_light(&resp),
            Light {
                mode: 2,
                brightness: 80,
                speed: 4,
                rgb: [1, 2, 3],
            }
        );
    }

    #[test]
    fn a_report_is_64_bytes_and_keeps_the_type_mask() {
        let report = Report::new(ty::LIGHT, 1, 129).finish(WorkMode::Usb);
        assert_eq!(report.len(), REPORT_LEN);
        assert_eq!(report[0], 5);
    }
}
