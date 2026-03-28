//! Manual FFI bindings for the `esp_hid` component.
//!
//! The `esp_hid` component is built by ESP-IDF (`libesp_hid.a` is linked) but
//! `bindgen` does not process its headers, so we declare the required types and
//! functions by hand.
#![allow(
    non_camel_case_types,
    dead_code,
    reason = "FFI types matching C naming conventions"
)]

use std::ffi::c_char;

use esp_idf_svc::sys::{esp_err_t, esp_event_handler_t};

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

pub type esp_hid_transport_t = u32;
pub const ESP_HID_TRANSPORT_BLE: esp_hid_transport_t = 1;

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

pub const ESP_HIDD_START_EVENT: i32 = 0;
pub const ESP_HIDD_CONNECT_EVENT: i32 = 1;
pub const ESP_HIDD_PROTOCOL_MODE_EVENT: i32 = 2;
pub const ESP_HIDD_CONTROL_EVENT: i32 = 3;
pub const ESP_HIDD_OUTPUT_EVENT: i32 = 4;
pub const ESP_HIDD_FEATURE_EVENT: i32 = 5;
pub const ESP_HIDD_DISCONNECT_EVENT: i32 = 6;
pub const ESP_HIDD_STOP_EVENT: i32 = 7;

// ---------------------------------------------------------------------------
// Opaque device handle
// ---------------------------------------------------------------------------

/// Opaque HID device — only ever used behind a pointer.
#[repr(C)]
pub struct esp_hidd_dev_s {
    _opaque: [u8; 0],
}

// SAFETY: The ESP-IDF HID device is protected by FreeRTOS critical sections
// internally. We only share the pointer across the main thread and callbacks
// running on the same Bluedroid task, using atomic operations.
unsafe impl Send for esp_hidd_dev_s {}
unsafe impl Sync for esp_hidd_dev_s {}

// ---------------------------------------------------------------------------
// Config structs
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct esp_hid_raw_report_map_t {
    pub data: *const u8,
    pub len: u16,
}

#[repr(C)]
pub struct esp_hid_device_config_t {
    pub vendor_id: u16,
    pub product_id: u16,
    pub version: u16,
    pub device_name: *const c_char,
    pub manufacturer_name: *const c_char,
    pub serial_number: *const c_char,
    pub report_maps: *mut esp_hid_raw_report_map_t,
    pub report_maps_len: u8,
}

// ---------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------

unsafe extern "C" {
    /// Initialise a HID device on the given transport.
    pub fn esp_hidd_dev_init(
        config: *const esp_hid_device_config_t,
        transport: esp_hid_transport_t,
        callback: esp_event_handler_t,
        dev: *mut *mut esp_hidd_dev_s,
    ) -> esp_err_t;

    /// Send an INPUT report to the connected host.
    pub fn esp_hidd_dev_input_set(
        dev: *mut esp_hidd_dev_s,
        map_index: usize,
        report_id: usize,
        data: *mut u8,
        length: usize,
    ) -> esp_err_t;

    /// Set (pre-load) a FEATURE report value for the connected host.
    pub fn esp_hidd_dev_feature_set(
        dev: *mut esp_hidd_dev_s,
        map_index: usize,
        report_id: usize,
        data: *mut u8,
        length: usize,
    ) -> esp_err_t;
}
