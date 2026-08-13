//! Compile-time diagnostics specific to GraphqlInput / GraphqlOutput.

#[test]
fn graphql_compile_fail() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/graphql_compile_fail/*.rs");
}
