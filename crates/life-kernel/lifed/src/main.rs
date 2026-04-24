//! `lifed` — privileged daemon hosting the `KernelEngine`.
//!
//! Phase 2 scaffold. Real entrypoint logic lands in BRO-900.

#![deny(unsafe_code)]

mod error;

pub use error::{LifedError, LifedResult};

fn main() {
    eprintln!("lifed: scaffold — entrypoint wired in BRO-900");
    std::process::exit(0);
}
