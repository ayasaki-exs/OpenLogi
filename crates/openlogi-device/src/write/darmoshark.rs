//! Darmoshark DPI and report-rate driver.
//!
//! The device is a standalone raw-HID node, so the HID++ write layer cannot
//! reach it. This driver owns the request/response exchange over
//! [`RawChannel`](crate::backend::RawChannel) and exposes the same semantic
//! values the DPI and report-rate panels already consume.
//!
//! Every public entry point opens the config channel, performs the handshake,
//! reads the full sensor block, changes only the requested field and writes
//! the complete block back. The protocol has no per-field write: `0xb5` always
//! carries DPI, rate, LOD, angle and sleep together.

use std::sync::LazyLock;
use std::time::Duration;

use openlogi_core::hid::{
    Dpi, DpiCapabilities, DpiInfo, HidppOperation, ReportRate, ReportRateInfo, WriteError,
};
use openlogi_device_registry::darmoshark::{
    DARMOSHARK_RECEIVER_PRODUCT_ID, DarmosharkDescriptor, find_darmoshark,
};

use crate::backend::{HidBackend, RawChannel};
use crate::channel::route::{DeviceRoute, find_raw_route_node};
use crate::darmoshark::{
    BaseInfo, Contract, Power, REPORT_LEN, REPORT_RATES, WorkMode, cmd_base_info, cmd_dpi_full,
    cmd_power, cmd_report_rate, cmd_save, parse_base_info, parse_power, ty,
};

const READ_TIMEOUT: Duration = Duration::from_millis(1500);
const WRITE_CYCLES: usize = 3;
const READS_PER_CYCLE: usize = 3;
const MIN_DPI: u16 = 50;
const MAX_DPI: u16 = 30_000;
const DPI_STEP: u16 = 50;

/// The device answers one report at a time; serialize every exchange so a GUI
/// slider and a background read cannot consume each other's replies.
static SESSION_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Whether `route` addresses a registered Darmoshark config channel.
#[must_use]
pub fn is_route(route: &DeviceRoute) -> bool {
    route_descriptor(route).is_some()
}

fn route_descriptor(route: &DeviceRoute) -> Option<&'static DarmosharkDescriptor> {
    let DeviceRoute::RawHid {
        vendor_id,
        product_id,
        usage_page,
        usage_id,
        ..
    } = route
    else {
        return None;
    };
    find_darmoshark(*vendor_id, *product_id, *usage_page, *usage_id)
}

fn work_mode(route: &DeviceRoute) -> Option<WorkMode> {
    let descriptor = route_descriptor(route)?;
    Some(if descriptor.product_id == DARMOSHARK_RECEIVER_PRODUCT_ID {
        WorkMode::Wireless
    } else {
        WorkMode::Usb
    })
}

struct Session {
    channel: Box<dyn RawChannel>,
    mode: WorkMode,
    contract: Contract,
}

impl Session {
    async fn open(backend: &dyn HidBackend, route: &DeviceRoute) -> Result<Self, WriteError> {
        let mode = work_mode(route).ok_or(WriteError::DeviceNotFound)?;
        let node = find_raw_route_node(backend, route)
            .await?
            .ok_or(WriteError::DeviceNotFound)?;
        let mut channel = backend.open_raw_channel(&node).await?;
        let handshake = exchange(
            &mut *channel,
            &cmd_power(mode),
            ty::GET_POWER,
            1,
            HidppOperation::ReadDpiCapabilities,
        )
        .await?;
        Ok(Self {
            channel,
            mode,
            contract: Contract::detect(&handshake),
        })
    }

    async fn request(
        &mut self,
        command: &[u8; REPORT_LEN],
        expect_type: u8,
        expect_sub: u8,
        operation: HidppOperation,
    ) -> Result<[u8; REPORT_LEN], WriteError> {
        exchange(
            &mut *self.channel,
            command,
            expect_type,
            expect_sub,
            operation,
        )
        .await
    }

    async fn read_power(&mut self, operation: HidppOperation) -> Result<Power, WriteError> {
        let response = self
            .request(&cmd_power(self.mode), ty::GET_POWER, 1, operation)
            .await?;
        Ok(parse_power(&response, self.contract))
    }

    async fn read_base_info(&mut self, operation: HidppOperation) -> Result<BaseInfo, WriteError> {
        let response = self
            .request(&cmd_base_info(self.mode), ty::SENSOR, 1, operation)
            .await?;
        Ok(parse_base_info(&response, self.contract))
    }

    async fn save(&mut self, operation: HidppOperation) -> Result<(), WriteError> {
        self.request(&cmd_save(self.mode), ty::SAVE, 1, operation)
            .await
            .map(|_response| ())
    }
}

async fn exchange(
    channel: &mut dyn RawChannel,
    command: &[u8; REPORT_LEN],
    expect_type: u8,
    expect_sub: u8,
    operation: HidppOperation,
) -> Result<[u8; REPORT_LEN], WriteError> {
    let mut last_error = WriteError::RequestTimedOut { operation };
    for _cycle in 0..WRITE_CYCLES {
        if let Err(error) = channel.write_output_report(command).await {
            last_error = error.into();
            continue;
        }
        for _read in 0..READS_PER_CYCLE {
            let mut buffer = [0_u8; REPORT_LEN];
            match channel.read_input_report(&mut buffer, READ_TIMEOUT).await {
                Ok(Some(REPORT_LEN)) => {
                    if (buffer[0] & 0x3f) == expect_type && buffer[3] == expect_sub {
                        return Ok(buffer);
                    }
                    last_error = WriteError::Hid(format!(
                        "unexpected Darmoshark response type {:#04x} sub {:#04x} (want {expect_type:#04x}/{expect_sub:#04x})",
                        buffer[0] & 0x3f,
                        buffer[3]
                    ));
                }
                Ok(Some(read)) => {
                    last_error = WriteError::Hid(format!(
                        "Darmoshark response was {read} bytes, expected {REPORT_LEN}"
                    ));
                }
                Ok(None) => {
                    last_error = WriteError::RequestTimedOut { operation };
                    break;
                }
                Err(error) => {
                    last_error = error.into();
                    break;
                }
            }
        }
    }
    Err(last_error)
}

fn level_count(base: &BaseInfo) -> Result<u8, WriteError> {
    // A `dms` base-info response carries five X/Y pairs; 6–10 levels use the
    // extended command this driver does not implement.
    if base.level_count == 0 || base.level_count > 5 {
        return Err(WriteError::EmptyDpiList);
    }
    Ok(base.level_count)
}

fn current_dpi(base: &BaseInfo) -> Result<Dpi, WriteError> {
    let count = level_count(base)?;
    let index = usize::from(base.current_dpi);
    if index >= usize::from(count) {
        return Err(WriteError::EmptyDpiList);
    }
    match base.dpi.get(index).copied().filter(|value| *value != 0) {
        Some(value) => Ok(Dpi::new(value)),
        None => Err(WriteError::EmptyDpiList),
    }
}

fn supported_dpi() -> Result<DpiCapabilities, WriteError> {
    DpiCapabilities::new((MIN_DPI..=MAX_DPI).step_by(usize::from(DPI_STEP)).collect())
}

fn dpi_info(base: &BaseInfo) -> Result<DpiInfo, WriteError> {
    Ok(DpiInfo {
        current: current_dpi(base)?,
        capabilities: supported_dpi()?,
    })
}

fn pairs_for_write(
    base: &BaseInfo,
    replacement: Option<Dpi>,
) -> Result<Vec<(u16, u16)>, WriteError> {
    let count = level_count(base)?;
    let index = usize::from(base.current_dpi);
    if index >= usize::from(count) {
        return Err(WriteError::EmptyDpiList);
    }
    let fallback = current_dpi(base)?.into_inner();
    let mut pairs = Vec::with_capacity(usize::from(count));
    for slot in 0..usize::from(count) {
        if slot == index {
            let value = replacement.map_or(fallback, Dpi::into_inner);
            pairs.push((value, value));
            continue;
        }
        let x = base
            .dpi
            .get(slot)
            .copied()
            .filter(|value| *value != 0)
            .unwrap_or(fallback);
        let y = base
            .dpi
            .get(slot + 5)
            .copied()
            .filter(|value| *value != 0)
            .unwrap_or(x);
        pairs.push((x, y));
    }
    Ok(pairs)
}

async fn write_dpi_block(
    session: &mut Session,
    base: &BaseInfo,
    dpi: Dpi,
    operation: HidppOperation,
) -> Result<(), WriteError> {
    let count = level_count(base)?;
    let pairs = pairs_for_write(base, Some(dpi))?;
    let command = cmd_dpi_full(
        session.mode,
        base.sensor,
        base.report_rate,
        count,
        base.current_dpi,
        &pairs,
        base.lod,
        base.angle,
        base.sleep_ms,
        base.delay,
    );
    session.request(&command, ty::SENSOR, 2, operation).await?;
    if let Err(error) = session.save(operation).await {
        return Err(WriteError::Hid(format!(
            "DPI write succeeded but save failed: {error}"
        )));
    }
    Ok(())
}

async fn write_dpi_attempt(
    session: &mut Session,
    base: &BaseInfo,
    dpi: Dpi,
) -> Result<Option<Dpi>, WriteError> {
    write_dpi_block(session, base, dpi, HidppOperation::WriteDpi).await?;
    match session.read_base_info(HidppOperation::WriteDpi).await {
        Ok(after) => Ok(Some(current_dpi(&after)?)),
        Err(error) => {
            tracing::debug!(%error, "Darmoshark DPI write acknowledged; read-back skipped");
            Ok(None)
        }
    }
}

/// Reads the current DPI and the values the driver exposes to the UI.
pub async fn read_dpi(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
) -> Result<DpiInfo, WriteError> {
    if !is_route(route) {
        return Err(WriteError::DeviceNotFound);
    }
    let _guard = SESSION_LOCK.lock().await;
    let mut session = Session::open(backend, route).await?;
    let base = session
        .read_base_info(HidppOperation::ReadDpiCapabilities)
        .await?;
    dpi_info(&base)
}

/// Sets the current sensor DPI, preserving every other field in the sensor block.
pub async fn set_dpi(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
    dpi: Dpi,
) -> Result<(), WriteError> {
    if !is_route(route) {
        return Err(WriteError::DeviceNotFound);
    }
    let supported = supported_dpi()?;
    if !supported.contains(dpi) {
        return Err(WriteError::DpiUnsupported { requested: dpi });
    }
    let _guard = SESSION_LOCK.lock().await;
    let mut session = Session::open(backend, route).await?;
    let base = session.read_base_info(HidppOperation::WriteDpi).await?;
    match write_dpi_attempt(&mut session, &base, dpi).await? {
        None => return Ok(()),
        Some(actual) if actual == dpi => return Ok(()),
        Some(actual) => {
            tracing::debug!(%dpi, %actual, "Darmoshark DPI read back differently; retrying once");
        }
    }
    let base = session.read_base_info(HidppOperation::WriteDpi).await?;
    match write_dpi_attempt(&mut session, &base, dpi).await? {
        None => Ok(()),
        Some(retried) if retried == dpi => Ok(()),
        Some(retried) => Err(WriteError::DpiNotApplied {
            requested: dpi,
            actual: retried,
        }),
    }
}

fn report_rate_info(base: &BaseInfo, power: Power) -> Result<ReportRateInfo, WriteError> {
    let max_index = usize::from(power.report_rate_max).min(REPORT_RATES.len() - 1);
    let supported = REPORT_RATES[..=max_index]
        .iter()
        .copied()
        .map(ReportRate::new)
        .collect();
    let index = usize::from(base.report_rate);
    let current = REPORT_RATES
        .get(index)
        .copied()
        .ok_or_else(|| WriteError::Hid(format!("device reported report-rate index {index}")))?;
    Ok(ReportRateInfo {
        current: ReportRate::new(current),
        supported,
    })
}

fn report_rate_index(rate: ReportRate) -> Option<u8> {
    REPORT_RATES
        .iter()
        .position(|hz| ReportRate::new(*hz) == rate)
        .and_then(|index| u8::try_from(index).ok())
}

/// Reads the current report rate and the rates this connection lists.
pub async fn read_report_rate(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
) -> Result<ReportRateInfo, WriteError> {
    if !is_route(route) {
        return Err(WriteError::DeviceNotFound);
    }
    let _guard = SESSION_LOCK.lock().await;
    let mut session = Session::open(backend, route).await?;
    let base = session
        .read_base_info(HidppOperation::ReadReportRate)
        .await?;
    let power = session.read_power(HidppOperation::ReadReportRate).await?;
    report_rate_info(&base, power)
}

/// Sets the report rate and returns what the device reports afterwards.
pub async fn set_report_rate(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
    rate: ReportRate,
) -> Result<ReportRateInfo, WriteError> {
    if !is_route(route) {
        return Err(WriteError::DeviceNotFound);
    }
    let _guard = SESSION_LOCK.lock().await;
    let mut session = Session::open(backend, route).await?;
    let base = session
        .read_base_info(HidppOperation::WriteReportRate)
        .await?;
    let power = session.read_power(HidppOperation::WriteReportRate).await?;
    let before = report_rate_info(&base, power)?;
    if !before.supports(rate) {
        return Err(WriteError::ReportRateUnsupported { requested: rate });
    }
    if before.current == rate {
        return Ok(before);
    }
    let index =
        report_rate_index(rate).ok_or(WriteError::ReportRateUnsupported { requested: rate })?;
    let command = cmd_report_rate(session.mode, index);
    session
        .request(&command, ty::SENSOR, 4, HidppOperation::WriteReportRate)
        .await?;
    if let Err(error) = session.save(HidppOperation::WriteReportRate).await {
        return Err(WriteError::Hid(format!(
            "report-rate write succeeded but save failed: {error}"
        )));
    }
    match session
        .read_base_info(HidppOperation::WriteReportRate)
        .await
    {
        Ok(after) => {
            let after = report_rate_info(&after, power)?;
            if after.current != rate {
                return Err(WriteError::ReportRateNotApplied {
                    requested: rate,
                    actual: after.current,
                });
            }
            Ok(after)
        }
        Err(error) => {
            tracing::debug!(%error, "Darmoshark report-rate write acknowledged; read-back skipped");
            Ok(ReportRateInfo {
                current: rate,
                supported: before.supported,
            })
        }
    }
}

#[cfg(test)]
mod tests;
