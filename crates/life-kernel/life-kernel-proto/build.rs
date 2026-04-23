fn main() {
    println!("cargo:rerun-if-changed=proto/kernel.proto");
    // Codegen wired in Commit 3 of BRO-857.
}
