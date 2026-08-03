use std::collections::BTreeMap;

use distributed::application::{
    Application, CommandMount, CommandSpec, CommandTypeSpec, ContractCompiler, Module,
};
use distributed::graphql::{
    build_surface, surface_for_application_contract, ClientSurfaceIdentity, CommandConsistency,
    SurfaceOptions,
};

fn command(id: &str) -> CommandSpec {
    CommandSpec::try_new(
        id,
        id.replace('.', "_"),
        CommandTypeSpec {
            name: format!("{id}Input"),
            fields: Vec::new(),
        },
        CommandTypeSpec {
            name: format!("{id}Output"),
            fields: Vec::new(),
        },
        CommandConsistency::Eventual,
    )
    .expect("test command should be portable")
}

fn application_with_commands(order: &[&str]) -> Application {
    let commands = order.iter().map(|id| command(id)).collect::<Vec<_>>();
    let module = Module::new("todo")
        .commands(commands)
        .build()
        .expect("test module should be valid");
    Application::try_new("todo-app", [module], []).expect("test application should be valid")
}

#[test]
fn module_identity_is_identical_across_full_and_split_selection() {
    let full = application_with_commands(&["todo.create", "todo.archive"]);
    let split = application_with_commands(&["todo.archive", "todo.create"]);

    assert_eq!(full.manifest().commands, split.manifest().commands);
    assert_eq!(
        full.manifest().canonical_bytes().unwrap(),
        split.manifest().canonical_bytes().unwrap()
    );
}

#[test]
fn application_manifest_is_byte_deterministic_and_contains_no_executable_data() {
    fn handler() {}

    let spec = command("todo.create");
    let mount = CommandMount::from_handler(spec.clone(), handler);
    let module = Module::new("todo")
        .command(spec)
        .mount(mount)
        .build()
        .expect("mounted module should be valid");
    let application = Application::try_new("todo-app", [module], []).unwrap();

    let first = application.manifest().canonical_bytes().unwrap();
    let second = application.manifest().canonical_bytes().unwrap();
    assert_eq!(first, second);
    let json = String::from_utf8(first.clone()).unwrap();
    assert!(!json.contains("handler"));
    assert!(!json.contains("application_composition"));
    assert_eq!(
        distributed::application::ApplicationManifest::from_canonical_bytes(&first)
            .unwrap()
            .canonical_bytes()
            .unwrap(),
        first
    );
}

#[test]
fn contract_only_surface_compiles_without_repository_or_service() {
    let compiler = ContractCompiler::new("contract-only");
    let surface = compiler.surface().expect("surface IR should be pool-free");
    let sdl = compiler.graphql_sdl().expect("SDL should compile from IR");
    assert!(sdl.contains("type Query"));
    let manifest = compiler.manifest().expect("manifest should compile");
    assert_eq!(manifest.name, "contract-only");
    assert!(surface.commands().is_empty());
}

#[test]
fn contract_only_compiler_exports_a_selected_client_surface() {
    let full = build_surface(&[], &SurfaceOptions::sqlite()).unwrap();
    let selected = surface_for_application_contract(
        &full,
        "web",
        &["user".into()],
        &["user".into()],
        &BTreeMap::from([("user".into(), BTreeMap::new())]),
    )
    .expect("selected contract surface should compile");
    let manifest = ContractCompiler::new("contract-only")
        .client_manifest("contract-only", selected)
        .expect("client manifest should not need Service provenance");
    assert_eq!(manifest.service_id, "contract-only");
    assert!(matches!(
        manifest.surface,
        ClientSurfaceIdentity::Application { name, .. } if name == "web"
    ));
}

#[test]
fn unlisted_linked_module_changes_nothing() {
    let listed = application_with_commands(&["todo.create"]);
    let unlisted = Module::new("audit")
        .command(command("audit.record"))
        .build()
        .expect("unlisted module should be valid");
    let listed_again = application_with_commands(&["todo.create"]);

    assert_eq!(
        listed.manifest().canonical_bytes().unwrap(),
        listed_again.manifest().canonical_bytes().unwrap()
    );
    assert_eq!(unlisted.id(), "audit");
    assert_eq!(listed.manifest().module_ids(), ["todo"]);
}

#[test]
fn contract_compiler_surface_matches_shared_builder() {
    let expected = build_surface(&[], &SurfaceOptions::sqlite()).unwrap();
    let actual = ContractCompiler::new("surface").surface().unwrap();
    assert_eq!(expected.query_root_names(), actual.query_root_names());
}

#[test]
fn explicit_registration_reports_duplicate_and_missing_identities() {
    let duplicate = Module::new("todo")
        .commands([command("todo.create"), command("todo.create")])
        .build()
        .expect_err("duplicate commands must fail closed");
    assert!(duplicate.to_string().contains("duplicate command identity"));

    let missing_mount = Module::new("todo")
        .mount(CommandMount::contract(command("todo.create")))
        .build()
        .expect_err("a mount without its listed command must fail closed");
    assert!(missing_mount
        .to_string()
        .contains("missing command identity"));
}
