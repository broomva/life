//! `lifed` — privileged daemon hosting the `KernelEngine`.
//!
//! Phase 2 scaffold. Real entrypoint logic lands in BRO-900.

#![deny(unsafe_code)]
// `config` and `error` define the types the real entrypoint (BRO-900) will
// consume — exercised only by `config::tests` today.  Suppress dead-code
// warnings until the entrypoint lands; BRO-900 removes this allow.
#![allow(dead_code)]

mod config;
mod error;

fn main() {
    eprintln!("lifed: scaffold — entrypoint wired in BRO-900");
    std::process::exit(0);
}
