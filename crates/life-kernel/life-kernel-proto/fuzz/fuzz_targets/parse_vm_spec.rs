#![no_main]

use libfuzzer_sys::fuzz_target;
use prost::Message;

fuzz_target!(|data: &[u8]| {
    // Exercise the proto parser against arbitrary bytes. If decode succeeds
    // the resulting VmSpec is well-formed by definition; if it fails we get
    // a recoverable error. A panic here is a bug.
    let _ = life_kernel_proto::pb::VmSpec::decode(data);
});
