/*
 * Copyright (c) godot-rust; Bromeon and contributors.
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Macro implementations used by `godot-ffi` crate.

#![cfg(feature = "experimental-wasm")]

use crate::util::bail;
use crate::ParseResult;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use std::sync::atomic::{AtomicBool, Ordering};


static SRAND_INITIALIZED: AtomicBool = AtomicBool::new(false);

// Create sufficiently unique identifier without entire `uuid` (let alone `rand`) crate dependency.
struct TrivialRng;

impl TrivialRng {
    fn rand() -> u32 {
        if SRAND_INITIALIZED.compare_exchange(
            false,
            true,
            Ordering::Acquire,
            Ordering::Relaxed
        ).is_ok() {
            unsafe {
                let time = libc::time(std::ptr::null_mut());
                let pid = libc::getpid();
                let seed_64bit = time ^ (pid as i64);
                let upper_half = (seed_64bit >> 32) as u32;
                let lower_half = seed_64bit as u32;
                let seed = upper_half ^ lower_half;
                libc::srand(seed);
            }
        }
        unsafe { libc::rand() as u32 }
    }
}


pub(super) fn wasm_declare_init_fn(input: TokenStream) -> ParseResult<TokenStream> {
    if !input.is_empty() {
        return bail!(input, "macro expects no arguments");
    }

    let a = TrivialRng::rand();
    let b = TrivialRng::rand();

    // Rust presently requires that statics with a custom `#[link_section]` must be a simple
    // list of bytes on the Wasm target (with no extra levels of indirection such as references).
    //
    // As such, instead we export a function with a random name of known prefix to be used by the embedder.
    // This prefix is queried at load time, see godot-macros/src/gdextension.rs.
    let function_name = format_ident!("__godot_rust_registrant_{a}_{b}");

    let code = quote! {
        #[cfg(target_family = "wasm")] // Strictly speaking not necessary, as this macro is only invoked for Wasm.
        #[no_mangle]
        extern "C" fn #function_name() {
            __init();
        }
    };

    Ok(code)
}
