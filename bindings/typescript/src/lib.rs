#![deny(clippy::all)]

use napi_derive::napi;

/// Smoke-test function proving the napi-rs toolchain builds end-to-end.
#[napi]
pub fn hello() -> String {
    "z3rno bindings scaffold OK".to_string()
}
