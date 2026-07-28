#[test]
fn projection_compile_fail() {
    let tests = trybuild::TestCases::new();
    tests.compile_fail("tests/projection_compile_fail/*.rs");
}
