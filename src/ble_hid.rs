//! BLE HID device: GAP advertising, HOGP setup, and report sending.
//!
//! Uses raw `esp-idf-sys` FFI for all Bluetooth operations. The ESP-IDF
//! unified HID API (`esp_hidd_dev_init`) handles GATT service creation
//! and report characteristic setup internally.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU16, Ordering};

use esp_idf_svc::sys::*;
use log::{error, info, warn};

use crate::esp_hid_ffi::{self, esp_hidd_dev_s};
use crate::{feature_reports, hid_descriptor};

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

static HID_DEV: AtomicPtr<esp_hidd_dev_s> = AtomicPtr::new(std::ptr::null_mut());
static CONNECTED: AtomicBool = AtomicBool::new(false);
static SCAN_TIME: AtomicU16 = AtomicU16::new(0);

const DEVICE_NAME: &[u8] = b"ESP32 UC PTP\0";

/// HID service UUID — Bluetooth SIG base UUID with 16-bit 0x1812 embedded.
#[rustfmt::skip]
static HID_SERVICE_UUID128: [u8; 16] = [
    0xfb, 0x34, 0x9b, 0x5f, 0x80, 0x00, 0x00, 0x80,
    0x00, 0x10, 0x00, 0x00, 0x12, 0x18, 0x00, 0x00,
];

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns `true` when a BLE host is connected.
pub fn is_connected() -> bool {
    CONNECTED.load(Ordering::Acquire)
}

/// One-shot init: register GAP callback, configure security and advertising,
/// then create the HID device. Advertising starts automatically on the
/// `START` event from `esp_hidd_dev_init`.
///
/// # Safety contract (caller)
/// Bluetooth controller and Bluedroid must already be initialised and enabled.
pub fn init() -> Result<(), EspError> {
    unsafe {
        esp!(esp_ble_gap_register_callback(Some(gap_event_handler)))?;
        set_security_params()?;
        esp!(esp_ble_gap_set_device_name(DEVICE_NAME.as_ptr().cast()))?;
        configure_adv_data()?;

        // The esp_hid component defines esp_hidd_gatts_event_handler but does
        // not register it — callers must do so before esp_hidd_dev_init.
        esp!(esp_ble_gatts_register_callback(Some(
            esp_hid_ffi::esp_hidd_gatts_event_handler
        )))?;

        init_hid_device()?;
    }
    Ok(())
}

/// Send a single-finger touch input report (report ID 0x05).
///
/// Pass `finger_down = false` to lift the finger (sends zero contact count).
pub fn send_touch_report(
    x: u16,
    y: u16,
    contact_id: u32,
    finger_down: bool,
) -> Result<(), EspError> {
    let dev = HID_DEV.load(Ordering::Acquire);
    if dev.is_null() {
        return Err(EspError::from_infallible::<ESP_ERR_INVALID_STATE>());
    }

    let mut report = [0u8; hid_descriptor::TOUCH_REPORT_SIZE];

    if finger_down {
        // Contact 0: confidence=1 | tipSwitch=1 = 0x03
        report[0] = 0x03;
        report[1..5].copy_from_slice(&contact_id.to_le_bytes());
        report[5..7].copy_from_slice(&x.to_le_bytes());
        report[7..9].copy_from_slice(&y.to_le_bytes());
        // Contacts 1-4 stay zeroed (not active).
        report[47] = 1; // contactCount
    }

    let st = SCAN_TIME.fetch_add(50, Ordering::Relaxed);
    report[45..47].copy_from_slice(&st.to_le_bytes());

    unsafe {
        esp!(esp_hid_ffi::esp_hidd_dev_input_set(
            dev,
            0,
            hid_descriptor::REPORTID_MULTITOUCH as _,
            report.as_mut_ptr(),
            report.len(),
        ))
    }
}

// ---------------------------------------------------------------------------
// GAP setup
// ---------------------------------------------------------------------------

unsafe fn set_security_params() -> Result<(), EspError> {
    macro_rules! set_param {
        ($tag:expr, $val:expr) => {{
            let mut v = $val;
            esp!(esp_ble_gap_set_security_param(
                $tag,
                &mut v as *mut _ as *mut c_void,
                std::mem::size_of_val(&v) as u8,
            ))
        }};
    }

    // "Just Works" pairing — no PIN, no numeric comparison.
    set_param!(
        esp_ble_sm_param_t_ESP_BLE_SM_AUTHEN_REQ_MODE,
        ESP_LE_AUTH_BOND as u8
    )?;
    set_param!(
        esp_ble_sm_param_t_ESP_BLE_SM_IOCAP_MODE,
        ESP_IO_CAP_NONE as u8
    )?;
    set_param!(esp_ble_sm_param_t_ESP_BLE_SM_MAX_KEY_SIZE, 16u8)?;
    set_param!(
        esp_ble_sm_param_t_ESP_BLE_SM_SET_INIT_KEY,
        (ESP_BLE_ENC_KEY_MASK | ESP_BLE_ID_KEY_MASK) as u8
    )?;
    set_param!(
        esp_ble_sm_param_t_ESP_BLE_SM_SET_RSP_KEY,
        (ESP_BLE_ENC_KEY_MASK | ESP_BLE_ID_KEY_MASK) as u8
    )?;
    Ok(())
}

unsafe fn configure_adv_data() -> Result<(), EspError> {
    let mut adv = esp_ble_adv_data_t {
        set_scan_rsp: false,
        include_name: true,
        include_txpower: true,
        min_interval: 0x0006, // 7.5 ms
        max_interval: 0x0010, // 20 ms
        appearance: 0x03C0,   // Generic HID
        manufacturer_len: 0,
        p_manufacturer_data: std::ptr::null_mut(),
        service_data_len: 0,
        p_service_data: std::ptr::null_mut(),
        service_uuid_len: HID_SERVICE_UUID128.len() as u16,
        p_service_uuid: HID_SERVICE_UUID128.as_ptr().cast_mut(),
        flag: 0x06, // General Discoverable | BR/EDR Not Supported
    };
    esp!(esp_ble_gap_config_adv_data(&mut adv))?;

    // Scan response echoes the name and TX power so scanners see them.
    let mut scan_rsp = esp_ble_adv_data_t {
        set_scan_rsp: true,
        include_name: true,
        include_txpower: true,
        ..adv
    };
    scan_rsp.service_uuid_len = 0;
    scan_rsp.p_service_uuid = std::ptr::null_mut();
    scan_rsp.flag = 0;
    esp!(esp_ble_gap_config_adv_data(&mut scan_rsp))?;

    Ok(())
}

fn start_advertising() -> Result<(), EspError> {
    let mut params = esp_ble_adv_params_t {
        adv_int_min: 0x20,
        adv_int_max: 0x30,
        adv_type: esp_ble_adv_type_t_ADV_TYPE_IND,
        own_addr_type: esp_ble_addr_type_t_BLE_ADDR_TYPE_PUBLIC,
        peer_addr: [0; 6],
        peer_addr_type: esp_ble_addr_type_t_BLE_ADDR_TYPE_PUBLIC,
        channel_map: esp_ble_adv_channel_t_ADV_CHNL_ALL,
        adv_filter_policy: esp_ble_adv_filter_t_ADV_FILTER_ALLOW_SCAN_ANY_CON_ANY,
    };
    unsafe { esp!(esp_ble_gap_start_advertising(&mut params)) }
}

// ---------------------------------------------------------------------------
// HID device setup
// ---------------------------------------------------------------------------

unsafe fn init_hid_device() -> Result<(), EspError> {
    let mut report_map = esp_hid_ffi::esp_hid_raw_report_map_t {
        data: hid_descriptor::PTP_REPORT_DESCRIPTOR.as_ptr(),
        len: hid_descriptor::PTP_REPORT_DESCRIPTOR.len() as u16,
    };

    let config = esp_hid_ffi::esp_hid_device_config_t {
        vendor_id: 0x1234,
        product_id: 0x5678,
        version: 0x0100,
        device_name: DEVICE_NAME.as_ptr().cast(),
        manufacturer_name: c"esp32-universal-control".as_ptr(),
        serial_number: c"00000001".as_ptr(),
        report_maps: &mut report_map,
        report_maps_len: 1,
    };

    let mut dev: *mut esp_hidd_dev_s = std::ptr::null_mut();
    esp!(esp_hid_ffi::esp_hidd_dev_init(
        &config,
        esp_hid_ffi::ESP_HID_TRANSPORT_BLE,
        Some(hidd_event_handler),
        &mut dev,
    ))?;

    HID_DEV.store(dev, Ordering::Release);
    info!("HID device initialised");
    Ok(())
}

/// Pre-load feature report values so the GATT layer auto-responds to reads.
unsafe fn preload_feature_reports() {
    let dev = HID_DEV.load(Ordering::Acquire);
    if dev.is_null() {
        return;
    }

    let caps = feature_reports::DEVICE_CAPS;
    let rc = esp_hid_ffi::esp_hidd_dev_feature_set(
        dev,
        0,
        hid_descriptor::REPORTID_DEVICE_CAPS as _,
        caps.as_ptr().cast_mut(),
        caps.len(),
    );
    if rc != ESP_OK {
        warn!("Failed to set DEVICE_CAPS feature report: {rc}");
    }

    let cert = feature_reports::PTPHQA_BLOB;
    let rc = esp_hid_ffi::esp_hidd_dev_feature_set(
        dev,
        0,
        hid_descriptor::REPORTID_PTPHQA as _,
        cert.as_ptr().cast_mut(),
        cert.len(),
    );
    if rc != ESP_OK {
        warn!("Failed to set PTPHQA feature report: {rc}");
    }
}

/// Log details of an incoming feature report (SET_REPORT from host).
///
/// # Safety
/// `event_data` must point to a valid `esp_hidd_event_data_t` whose active
/// variant is `feature`.
unsafe fn log_feature_event(event_data: *mut c_void) {
    if event_data.is_null() {
        info!("HIDD: feature report (no data)");
        return;
    }
    let feat = &*(event_data as *const esp_hid_ffi::esp_hidd_feature_event_data_t);
    let id = feat.report_id;
    let len = feat.length as usize;

    if !feat.data.is_null() && len > 0 {
        let bytes = std::slice::from_raw_parts(feat.data, len);
        info!("HIDD: feature SET report_id=0x{id:02x} data={bytes:02x?}");

        // Report ID 0x04 = Input Mode.  Value 3 = Windows PTP collection.
        if id == hid_descriptor::REPORTID_REPORTMODE as u16 && bytes.first() == Some(&0x03) {
            info!("*** Windows set Input Mode = 3 (PTP) — precision touchpad confirmed ***");
        }
    } else {
        info!("HIDD: feature GET report_id=0x{id:02x} len={len}");
    }
}

// ---------------------------------------------------------------------------
// Callbacks
// ---------------------------------------------------------------------------

unsafe extern "C" fn gap_event_handler(
    event: esp_gap_ble_cb_event_t,
    param: *mut esp_ble_gap_cb_param_t,
) {
    #[allow(non_upper_case_globals, reason = "matching C enum constants")]
    match event {
        esp_gap_ble_cb_event_t_ESP_GAP_BLE_ADV_DATA_SET_COMPLETE_EVT => {
            info!("GAP: advertising data set");
        }
        esp_gap_ble_cb_event_t_ESP_GAP_BLE_ADV_START_COMPLETE_EVT => {
            info!("GAP: advertising started");
        }
        esp_gap_ble_cb_event_t_ESP_GAP_BLE_SEC_REQ_EVT => {
            info!("GAP: security request — accepting");
            let p = &*param;
            esp_ble_gap_security_rsp(p.ble_security.ble_req.bd_addr.as_ptr().cast_mut(), true);
        }
        esp_gap_ble_cb_event_t_ESP_GAP_BLE_AUTH_CMPL_EVT => {
            info!("GAP: authentication complete");
        }
        _ => {
            info!("GAP: event {event}");
        }
    }
}

unsafe extern "C" fn hidd_event_handler(
    _handler_args: *mut c_void,
    _base: esp_event_base_t,
    id: i32,
    event_data: *mut c_void,
) {
    match id {
        esp_hid_ffi::ESP_HIDD_START_EVENT => {
            info!("HIDD: start — beginning advertising");
            if let Err(e) = start_advertising() {
                error!("Failed to start advertising: {e}");
            }
        }
        esp_hid_ffi::ESP_HIDD_CONNECT_EVENT => {
            info!("HIDD: connected — waiting for host enumeration");
            preload_feature_reports();
        }
        esp_hid_ffi::ESP_HIDD_FEATURE_EVENT => {
            log_feature_event(event_data);
            CONNECTED.store(true, Ordering::Release);
        }
        esp_hid_ffi::ESP_HIDD_DISCONNECT_EVENT => {
            info!("HIDD: disconnected — restarting advertising");
            CONNECTED.store(false, Ordering::Release);
            if let Err(e) = start_advertising() {
                error!("Failed to restart advertising: {e}");
            }
        }
        esp_hid_ffi::ESP_HIDD_OUTPUT_EVENT => {
            info!("HIDD: output report received");
        }
        esp_hid_ffi::ESP_HIDD_STOP_EVENT => {
            info!("HIDD: stopped");
        }
        other => {
            info!("HIDD: event {other}");
        }
    }
}
