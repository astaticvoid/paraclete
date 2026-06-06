// SPDX-License-Identifier: GPL-3.0-or-later
//! Minimal CLAP host callbacks. All no-ops for P8.

use std::ffi::{c_char, c_void};

use clap_sys::host::clap_host;
use clap_sys::version::{clap_version, CLAP_VERSION_MAJOR, CLAP_VERSION_MINOR, CLAP_VERSION_REVISION};

static HOST_NAME:    &[u8] = b"Paraclete\0";
static HOST_VENDOR:  &[u8] = b"Paraclete Audio\0";
static HOST_URL:     &[u8] = b"\0";
static HOST_VERSION: &[u8] = b"0.1.0\0";

pub(crate) static HOST: clap_host = clap_host {
    clap_version: clap_version {
        major:    CLAP_VERSION_MAJOR,
        minor:    CLAP_VERSION_MINOR,
        revision: CLAP_VERSION_REVISION,
    },
    host_data:        std::ptr::null_mut(),
    name:             HOST_NAME.as_ptr()    as *const c_char,
    vendor:           HOST_VENDOR.as_ptr()  as *const c_char,
    url:              HOST_URL.as_ptr()     as *const c_char,
    version:          HOST_VERSION.as_ptr() as *const c_char,
    get_extension:    Some(host_get_extension),
    request_restart:  Some(host_request_restart),
    request_process:  Some(host_request_process),
    request_callback: Some(host_request_callback),
};

unsafe extern "C" fn host_get_extension(
    _host: *const clap_host,
    _id:   *const c_char,
) -> *const c_void {
    std::ptr::null()
}

unsafe extern "C" fn host_request_restart(_host: *const clap_host) {}
unsafe extern "C" fn host_request_process(_host: *const clap_host) {}
unsafe extern "C" fn host_request_callback(_host: *const clap_host) {}
