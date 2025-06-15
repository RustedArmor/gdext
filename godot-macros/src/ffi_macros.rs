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
use std::env;

use std::sync::atomic::{AtomicU32, Ordering};

static FUNCTION_COUNTER: AtomicU32 = AtomicU32::new(0);

pub(super) fn wasm_declare_init_fn(input: TokenStream) -> ParseResult<TokenStream> {
    if !input.is_empty() {
        return bail!(input, "macro expects no arguments");
    }

    let crate_name = env::var("CARGO_PKG_NAME")
        .expect("CARGO_PKG_NAME env var not found. This macro must be run by Cargo.")
        .replace('-', "_"); // crate names may contain hyphens, but Rust identifiers must not.

    let index = FUNCTION_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Rust presently requires that statics with a custom `#[link_section]` must be a simple
    // list of bytes on the Wasm target (with no extra levels of indirection such as references).
    //
    // As such, instead we export a function with a known prefix to be used by the embedder.
    // This prefix is queried at load time, see godot-macros/src/gdextension.rs.
    let function_name = format_ident!("__godot_rust_registrant_{}_{}", crate_name, index);

    let code = quote! {
        #[cfg(target_family = "wasm")] // Strictly speaking not necessary, as this macro is only invoked for Wasm.
        #[no_mangle]
        extern "C" fn #function_name() {
            __init();
        }
    };

    Ok(code)
}
