//! Compile-time guarantees for command_input_defaults!.

#[test]
fn command_input_defaults_compile_fail() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/command_input_defaults_compile_fail/*.rs");
}
