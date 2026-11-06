use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_lite::stream;
use hidpp::async_trait;
use openlogi_core::device::{BatteryLevel, BatteryStatus};
use openlogi_core::hid::{Dpi, ReportRate, WriteError};

use crate::backend::{
    BackendError, HidBackend, HotplugStream, NodeId, NodeInfo, RawChannel, RawWriter,
};
use crate::channel::route::DeviceRoute;
use crate::darmoshark::{REPORT_LEN, decode_report_rate, encode_report_rate, level_mask};

use super::{is_route, read_dpi, read_report_rate, read_status, set_dpi, set_report_rate};

const MOCK_ID: &str = "mock-darmoshark";
const MOCK_ROUTE_IDENTITY: &str = "id:mock-darmoshark";

#[derive(Clone)]
struct MockState {
    rate: u8,
    max_rate: u8,
    dpi: [u16; 10],
    level_count: u8,
    current: u8,
    lod: u8,
    angle: i8,
    sleep_ms: u16,
    delay: u8,
    sensor: u8,
    battery_percent: u8,
    firmware: [u8; 3],
}

impl Default for MockState {
    fn default() -> Self {
        Self {
            rate: 1,
            max_rate: 3,
            // Five X/Y pairs: X at 8 + 4*i, Y at 10 + 4*i.
            dpi: [400, 1000, 1200, 1600, 2000, 400, 1000, 1600, 2000, 255],
            level_count: 5,
            current: 1,
            lod: 1,
            angle: 0,
            sleep_ms: 60,
            delay: 4,
            sensor: 0x20,
            battery_percent: 81,
            firmware: [0, 8, 1],
        }
    }
}

struct MockBackend {
    state: Arc<Mutex<MockState>>,
}

impl MockBackend {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MockState::default())),
        }
    }

    fn route() -> DeviceRoute {
        DeviceRoute::RawHid {
            vendor_id: 0x1915,
            product_id: 0x0723,
            usage_page: 0xff0a,
            usage_id: 0x0001,
            identity: MOCK_ROUTE_IDENTITY.to_owned(),
        }
    }

    fn node() -> NodeInfo {
        NodeInfo {
            id: NodeId::from(MOCK_ID.to_owned()),
            vendor_id: 0x1915,
            product_id: 0x0723,
            usage_page: 0xff0a,
            usage_id: 0x0001,
            name: "Mock Darmoshark".to_owned(),
            manufacturer: Some("Darmoshark".to_owned()),
            serial_number: None,
        }
    }
}

#[async_trait]
impl HidBackend for MockBackend {
    async fn enumerate(&self) -> Result<Vec<NodeInfo>, BackendError> {
        Ok(vec![Self::node()])
    }

    async fn enumerate_hidpp(&self) -> Result<Vec<NodeInfo>, BackendError> {
        Ok(Vec::new())
    }

    async fn open_hidpp(
        &self,
        _node: &NodeInfo,
    ) -> Result<Option<Arc<hidpp::channel::HidppChannel>>, BackendError> {
        Ok(None)
    }

    async fn open_raw_writer(&self, _node: &NodeInfo) -> Result<Box<dyn RawWriter>, BackendError> {
        Err(BackendError::Backend("mock has no writer".to_owned()))
    }

    async fn open_raw_channel(
        &self,
        _node: &NodeInfo,
    ) -> Result<Box<dyn RawChannel>, BackendError> {
        Ok(Box::new(MockRawChannel {
            state: Arc::clone(&self.state),
            responses: VecDeque::new(),
        }))
    }

    fn watch(&self) -> Result<HotplugStream, BackendError> {
        Ok(Box::new(stream::empty()))
    }
}

struct MockRawChannel {
    state: Arc<Mutex<MockState>>,
    responses: VecDeque<[u8; REPORT_LEN]>,
}

impl MockRawChannel {
    fn state(&self) -> MockState {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn push(&mut self, response: [u8; REPORT_LEN]) {
        self.responses.push_back(response);
    }

    fn ack(&mut self, ty: u8, sub: u8) {
        let mut response = [0_u8; REPORT_LEN];
        response[0] = ty;
        response[3] = sub;
        self.push(response);
    }

    fn push_power(&mut self) {
        let state = self.state();
        let mut response = [0_u8; REPORT_LEN];
        response[0] = 1;
        response[3] = 1;
        response[5] = 1;
        response[6] = state.battery_percent;
        response[7] = 0;
        response[10] = state.max_rate;
        self.push(response);
    }

    fn push_version(&mut self) {
        let state = self.state();
        let [major, minor, patch] = state.firmware;
        let mut response = [0_u8; REPORT_LEN];
        response[0] = 0;
        response[3] = 0;
        response[8] = (major << 4) | minor;
        response[9] = patch;
        self.push(response);
    }

    fn push_base_info(&mut self) {
        let state = self.state();
        let mut response = [0_u8; REPORT_LEN];
        response[0] = 4;
        response[3] = 1;
        response[4] = state.sensor;
        response[5] = encode_report_rate(state.rate);
        response[6] = level_mask(state.level_count);
        response[7] = state.current;
        for slot in 0..5 {
            let base = 8 + 4 * slot;
            let x = state.dpi[slot].to_le_bytes();
            let y = state.dpi[slot + 5].to_le_bytes();
            response[base] = x[0];
            response[base + 1] = x[1];
            response[base + 2] = y[0];
            response[base + 3] = y[1];
        }
        response[43] = state.lod;
        response[44] = state.angle.cast_unsigned();
        let sleep = state.sleep_ms.to_le_bytes();
        response[52] = sleep[0];
        response[53] = sleep[1];
        response[54] = state.delay;
        self.push(response);
    }

    fn apply_dpi_full(&mut self, request: &[u8]) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.rate = decode_report_rate(request[5]);
        state.level_count = u8::try_from(request[6].count_ones()).unwrap_or(0);
        state.current = request[7];
        for slot in 0..5 {
            let base = 8 + 4 * slot;
            state.dpi[slot] = u16::from_le_bytes([request[base], request[base + 1]]);
            state.dpi[slot + 5] = u16::from_le_bytes([request[base + 2], request[base + 3]]);
        }
        state.lod = request[43];
        state.angle = request[44].cast_signed();
        state.sleep_ms = u16::from_le_bytes([request[50], request[51]]);
        state.delay = request[54];
    }
}

#[async_trait]
impl RawChannel for MockRawChannel {
    async fn write_output_report(&mut self, report: &[u8]) -> Result<(), BackendError> {
        let ty = report[0] & 0x3f;
        let sub = report[3];
        match (ty, sub) {
            (0, 0) => self.push_version(),
            (1, 1) => self.push_power(),
            (4, 1) => self.push_base_info(),
            (4, 2) => {
                self.apply_dpi_full(report);
                self.ack(4, 2);
            }
            (4, 4) => {
                self.state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .rate = report[4].saturating_sub(1);
                self.ack(4, 4);
            }
            (10, 1) => self.ack(10, 1),
            _ => self.ack(ty, sub),
        }
        Ok(())
    }

    async fn read_input_report(
        &mut self,
        buffer: &mut [u8],
        _timeout: Duration,
    ) -> Result<Option<usize>, BackendError> {
        let Some(response) = self.responses.pop_front() else {
            return Ok(None);
        };
        let length = buffer.len().min(response.len());
        buffer[..length].copy_from_slice(&response[..length]);
        Ok(Some(response.len()))
    }
}

fn expect_state(backend: &MockBackend) -> MockState {
    backend
        .state
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

#[tokio::test]
async fn reads_the_active_dpi_and_a_linear_supported_range() {
    let backend = MockBackend::new();
    let info = read_dpi(&backend, &MockBackend::route())
        .await
        .expect("mock answers");

    assert_eq!(info.current, Dpi::new(1000));
    assert!(info.capabilities.contains(Dpi::new(400)));
    assert!(info.capabilities.contains(Dpi::new(800)));
    assert_eq!(info.capabilities.values().len(), 600);
}

#[tokio::test]
async fn reads_battery_and_firmware_without_touching_the_sensor_block() {
    let backend = MockBackend::new();
    let status = read_status(&backend, &MockBackend::route())
        .await
        .expect("mock answers");

    let battery = status.battery.expect("battery reading");
    assert_eq!(battery.percentage, Some(81));
    assert_eq!(battery.level, BatteryLevel::Good);
    assert_eq!(battery.status, BatteryStatus::Unknown);
    assert_eq!(status.firmware.as_deref(), Some("0.8.1"));
    assert_eq!(expect_state(&backend).rate, 1, "status must not write");
}

#[tokio::test]
async fn writes_the_active_slot_and_reads_it_back() {
    let backend = MockBackend::new();
    set_dpi(&backend, &MockBackend::route(), Dpi::new(800))
        .await
        .expect("mock accepts DPI");

    let state = expect_state(&backend);
    assert_eq!(state.dpi[1], 800);
    assert_eq!(state.dpi[6], 800);
    let info = read_dpi(&backend, &MockBackend::route())
        .await
        .expect("mock answers");
    assert_eq!(info.current, Dpi::new(800));
}

#[tokio::test]
async fn rejects_a_dpi_the_ui_could_never_pick() {
    let backend = MockBackend::new();
    let error = set_dpi(&backend, &MockBackend::route(), Dpi::new(123))
        .await
        .expect_err("123 is not a supported step");
    assert!(matches!(
        error,
        WriteError::DpiUnsupported { requested } if requested == Dpi::new(123)
    ));
}

#[tokio::test]
async fn reads_the_report_rate_and_the_connection_list() {
    let backend = MockBackend::new();
    let info = read_report_rate(&backend, &MockBackend::route())
        .await
        .expect("mock answers");

    assert_eq!(info.current, ReportRate::new(500));
    assert_eq!(
        info.supported,
        vec![
            ReportRate::new(125),
            ReportRate::new(500),
            ReportRate::new(1000),
            ReportRate::new(2000),
        ]
    );
}

#[tokio::test]
async fn writes_a_listed_report_rate_and_reads_it_back() {
    let backend = MockBackend::new();
    let info = set_report_rate(&backend, &MockBackend::route(), ReportRate::new(1000))
        .await
        .expect("mock accepts 1000 Hz");

    assert_eq!(info.current, ReportRate::new(1000));
    assert_eq!(expect_state(&backend).rate, 2);
}

#[tokio::test]
async fn rejects_a_report_rate_the_connection_does_not_list() {
    let backend = MockBackend::new();
    let error = set_report_rate(&backend, &MockBackend::route(), ReportRate::new(4000))
        .await
        .expect_err("the receiver caps at 2000 Hz");
    assert!(matches!(
        error,
        WriteError::ReportRateUnsupported { requested } if requested == ReportRate::new(4000)
    ));
}

#[tokio::test]
async fn a_non_raw_route_is_not_claimed() {
    let route = DeviceRoute::Direct {
        vendor_id: 0x1915,
        product_id: 0x0723,
    };
    assert!(!is_route(&route));
    assert!(matches!(
        read_dpi(&MockBackend::new(), &route).await,
        Err(WriteError::DeviceNotFound)
    ));
}
