//! Verifies that the generated proto types are visible at the expected
//! module paths. If this test compiles, codegen is wired correctly.

#[test]
fn life_v1_agent_module_present() {
    // Just check the module path resolves; the actual types are populated as
    // services land in A5, B13, B14.
    let _ = std::any::type_name::<life_runtime_proto::life::v1::Empty>();
}

#[test]
fn life_admin_v1_module_present() {
    let _ = std::any::type_name::<life_runtime_proto::life::admin::v1::Empty>();
}
