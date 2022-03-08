// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License..

#![no_std]
#![cfg_attr(target_vendor = "teaclave", feature(rustc_private))]
#![feature(const_extern_fn)]
#![allow(clippy::missing_safety_doc)]
#![allow(unused_macros)]
#![allow(non_camel_case_types)]

extern crate alloc;

#[cfg(target_os = "linux")]
extern crate sgx_ffi;
extern crate sgx_oc;
#[cfg(target_os = "linux")]
extern crate sgx_sync;
#[cfg(target_os = "linux")]
extern crate sgx_tlibc_sys;
#[cfg(target_os = "linux")]
extern crate sgx_trts;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
extern crate sgx_types;

#[macro_use]
mod macros;

cfg_if! {
    if #[cfg(target_os = "linux")] {
        mod linux;
        pub use linux::*;
    } else if #[cfg(target_os = "android")]  {
        mod android;
        pub use android::*;
    } else {

    }
}
