//! Compile-time guarantees for typed command declarations and effect IR.

#[test]
fn command_effects_compile_fail() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/command_effects_compile_fail/*.rs");
}
