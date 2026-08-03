/// The runtime sibling uses the exact contract-only declaration and exposes
/// the service registration result for an integration boundary test.
pub fn registered_mount_identity() -> (String, String) {
    let service = application_contract_only::runtime_service();
    let mount = service
        .registered_command_mounts()
        .first()
        .expect("generated runtime declaration must register one mount");
    (mount.spec().id.clone(), mount.spec().fingerprint.clone())
}

#[cfg(test)]
mod tests {
    #[test]
    fn same_declaration_registers_a_callable_runtime_mount_identity() {
        let (id, fingerprint) = super::registered_mount_identity();
        assert_eq!(id, "todo.create");
        assert!(fingerprint.starts_with("sha256:"));
    }
}
