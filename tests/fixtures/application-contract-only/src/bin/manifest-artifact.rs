fn main() {
    print!(
        "{}",
        String::from_utf8(application_contract_only::manifest_bytes())
            .expect("manifest is UTF-8 JSON")
    );
}
