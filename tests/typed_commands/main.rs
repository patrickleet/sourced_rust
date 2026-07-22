#![cfg(all(feature = "graphql", feature = "sqlite"))]
#![allow(dead_code)]

use std::any::TypeId;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_graphql::futures_util::StreamExt;
use async_graphql::Request;
use axum::body::Body;
use axum::http::Request as HttpRequest;
use distributed::graphql::{
    build_surface, graphql_router_with_service, read, surface_for_role, typed_command, Accepted,
    DistributedClientSurfaceExport, EffectInputFieldMarker, EffectModelFieldMarker, Fact,
    GraphqlCommands, GraphqlEngine, GraphqlInputType, GraphqlOutputType, GraphqlTypeDef,
    GraphqlTypeField, ModelPermissions, PreparedCommand, Projected, RoleGrant, SurfaceOptions,
    SurfaceProjector,
};
use distributed::microsvc::{CausalCommandContext, HandlerError, Routes, Service};
use distributed::microsvc::{Context, Session};
use distributed::{
    command_confirmations, command_effects, command_input_defaults, DistributedProjectManifest,
    GraphqlInput, GraphqlOutput, ReadModel, RelationalReadModel,
};
use serde::{Deserialize, Serialize};
use tower::util::ServiceExt;

static GRAPHQL_TYPED_GUARD_INVOKED: AtomicBool = AtomicBool::new(false);
static GRAPHQL_TYPED_HANDLER_INVOKED: AtomicBool = AtomicBool::new(false);

#[derive(Deserialize)]
struct InputA {
    id: String,
}

#[derive(Serialize)]
struct OutputA {
    id: String,
}

#[derive(Deserialize)]
struct InputB {
    id: String,
}

#[derive(Serialize)]
struct OutputB {
    id: String,
}

#[derive(Clone, Deserialize, GraphqlInput)]
struct PlanInput {
    id: String,
    title: String,
}

#[derive(Clone, Deserialize, GraphqlInput)]
struct RenamedDefaultInput {
    #[serde(rename = "todoId")]
    id: String,
}

#[derive(Serialize, GraphqlOutput)]
struct PlanOutput {
    id: String,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "plan_views", primary_key = ["id"])]
struct PlanView {
    id: String,
    title: String,
    count: i64,
    #[readmodel(text)]
    status: PlanStatus,
}

#[derive(Clone, Serialize, Deserialize)]
enum PlanStatus {
    Open,
    Closed,
}

#[derive(Clone, Deserialize, GraphqlInput)]
struct ForgedInput {
    id: String,
    title: String,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "forged_views", primary_key = ["id"])]
struct ForgedView {
    id: String,
    count: i64,
}

#[derive(Clone, Serialize, Deserialize, GraphqlInput)]
struct JsonDocument {
    label: String,
}

#[derive(Clone, Deserialize, GraphqlInput)]
struct JsonPatchInput {
    id: String,
    tags: Vec<String>,
    details: JsonDocument,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "json_views", primary_key = ["id"])]
struct JsonView {
    id: String,
    #[readmodel(jsonb)]
    tags: Vec<String>,
    #[readmodel(jsonb)]
    details: JsonDocument,
}

#[derive(Clone, Deserialize, GraphqlInput)]
struct BigIntKeyInput {
    key: i64,
    title: String,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "bigint_key_views", primary_key = ["key"])]
struct BigIntKeyView {
    id: String,
    key: i64,
    title: String,
}

#[derive(Clone, Deserialize, GraphqlInput)]
struct BigIntRelationshipInput {
    source_key: i64,
    target_id: String,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "bigint_relation_targets", primary_key = ["id"])]
struct BigIntRelationshipTarget {
    id: String,
    #[readmodel(
        foreign_key = "bigint_relation_sources.key",
        delegated_from = "BigIntRelationshipSource.key"
    )]
    source_key: i64,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "bigint_relation_sources", primary_key = ["key"])]
struct BigIntRelationshipSource {
    id: String,
    key: i64,
    #[readmodel(has_many = "BigIntRelationshipTarget", foreign_key = "source_key")]
    targets: Vec<BigIntRelationshipTarget>,
}

#[derive(Clone, Deserialize, GraphqlInput)]
struct NullableKeyInput {
    key: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "nullable_key_views", primary_key = ["key"])]
struct NullableKeyView {
    id: String,
    key: Option<String>,
    title: String,
}

#[derive(Clone, Deserialize, GraphqlInput)]
struct CompositeKeyInput {
    tenant_id: String,
    id: String,
    title: String,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(
    table = "composite_key_views",
    primary_key = ["tenant_id", "id"]
)]
struct CompositeKeyView {
    tenant_id: String,
    id: String,
    title: String,
}

#[derive(Clone, Deserialize, GraphqlInput)]
struct FloatEffectInput {
    id: String,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "float_effect_views", primary_key = ["id"])]
struct FloatEffectView {
    id: String,
    value: Option<f64>,
}

#[derive(Clone, Serialize, Deserialize)]
struct NestedJsonFloat {
    value: f64,
}

#[derive(Clone, Serialize, Deserialize)]
struct NestedJsonDocument {
    nested: NestedJsonFloat,
}

const NONFINITE_JSON_DOCUMENT: NestedJsonDocument = NestedJsonDocument {
    nested: NestedJsonFloat {
        value: f64::NEG_INFINITY,
    },
};

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "json_float_effect_views", primary_key = ["id"])]
struct JsonFloatEffectView {
    id: String,
    #[readmodel(jsonb)]
    value_f32: f32,
    #[readmodel(jsonb)]
    value_f64: f64,
    #[readmodel(jsonb)]
    document: serde_json::Value,
    #[readmodel(jsonb)]
    nested_document: NestedJsonDocument,
}

#[derive(Clone, Debug)]
struct BrokenText;

impl Serialize for BrokenText {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(serde::ser::Error::custom("broken constant serializer"))
    }
}

impl<'de> Deserialize<'de> for BrokenText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let _ = serde::de::IgnoredAny::deserialize(deserializer)?;
        Ok(Self)
    }
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "broken_constant_views", primary_key = ["id"])]
struct BrokenConstantView {
    id: String,
    #[readmodel(text)]
    value: BrokenText,
}

struct ForgedTitleMarker;

impl EffectInputFieldMarker for ForgedTitleMarker {
    type Input = ForgedInput;
    type Value = String;
    type NonNullValue = String;
    type Nullability = distributed::graphql::EffectRequired;
    type PathKind = distributed::graphql::EffectInputTerminalKind;
    type Wire = distributed::graphql::EffectWireString;
    type Nested = String;

    fn path() -> Vec<&'static str> {
        vec!["title"]
    }
}

struct ForgedCountMarker;

impl EffectModelFieldMarker for ForgedCountMarker {
    type Model = ForgedView;
    // Deliberately lies about the independently-derived SQL/GraphQL field.
    type Value = String;
    type Wire = distributed::graphql::EffectWireString;
    const FIELD: &'static str = "count";
}

struct ForgedDefaultMarker;

impl EffectInputFieldMarker for ForgedDefaultMarker {
    type Input = PlanInput;
    type Value = String;
    type NonNullValue = String;
    type Nullability = distributed::graphql::EffectRequired;
    type PathKind = distributed::graphql::EffectInputTerminalKind;
    type Wire = distributed::graphql::EffectWireString;
    type Nested = String;

    fn path() -> Vec<&'static str> {
        vec!["missing"]
    }
}

fn object_type<T: 'static>(name: &str) -> GraphqlTypeDef {
    GraphqlTypeDef::new(
        name,
        vec![GraphqlTypeField {
            name: "id".into(),
            type_name: "String".into(),
            nullable: false,
            list: false,
            item_nullable: false,
            nested: None,
        }],
    )
    .with_type_id(TypeId::of::<T>())
}

impl GraphqlInputType for InputA {
    fn graphql_type() -> GraphqlTypeDef {
        object_type::<Self>("CommandInput")
    }
}

impl GraphqlOutputType for OutputA {
    fn graphql_type() -> GraphqlTypeDef {
        object_type::<Self>("CommandOutput")
    }
}

impl GraphqlInputType for InputB {
    fn graphql_type() -> GraphqlTypeDef {
        object_type::<Self>("CommandInput")
    }
}

impl GraphqlOutputType for OutputB {
    fn graphql_type() -> GraphqlTypeDef {
        object_type::<Self>("CommandOutput")
    }
}

async fn handler_a(
    _context: &CausalCommandContext<'_>,
    input: InputA,
) -> Result<PreparedCommand<Accepted<OutputA>>, HandlerError> {
    Ok(PreparedCommand::prepare(OutputA { id: input.id }).unwrap())
}

async fn guarded_handler_a(
    _context: &CausalCommandContext<'_>,
    input: InputA,
) -> Result<PreparedCommand<Accepted<OutputA>>, HandlerError> {
    GRAPHQL_TYPED_HANDLER_INVOKED.store(true, Ordering::SeqCst);
    Ok(PreparedCommand::<Accepted<OutputA>>::prepare(OutputA { id: input.id }).unwrap())
}

async fn handler_b(
    _context: &CausalCommandContext<'_>,
    input: InputB,
) -> Result<PreparedCommand<Accepted<OutputB>>, HandlerError> {
    Ok(PreparedCommand::<Accepted<OutputB>>::prepare(OutputB { id: input.id }).unwrap())
}

async fn accepted_plan_handler(
    _context: &CausalCommandContext<'_>,
    input: PlanInput,
) -> Result<PreparedCommand<Accepted<PlanOutput>>, HandlerError> {
    Ok(PreparedCommand::<Accepted<PlanOutput>>::prepare(PlanOutput { id: input.id }).unwrap())
}

async fn renamed_default_handler(
    _context: &CausalCommandContext<'_>,
    input: RenamedDefaultInput,
) -> Result<PreparedCommand<Accepted<PlanOutput>>, HandlerError> {
    Ok(PreparedCommand::<Accepted<PlanOutput>>::prepare(PlanOutput { id: input.id }).unwrap())
}

async fn fact_plan_handler(
    _context: &CausalCommandContext<'_>,
    input: PlanInput,
) -> Result<PreparedCommand<Fact<PlanOutput>>, HandlerError> {
    Ok(PreparedCommand::<Fact<PlanOutput>>::prepare(PlanOutput { id: input.id }).unwrap())
}

async fn projected_plan_handler(
    _context: &CausalCommandContext<'_>,
    _input: PlanInput,
) -> Result<PreparedCommand<Projected<PlanOutput>>, HandlerError> {
    Err(HandlerError::Rejected(
        "projected preparation requires task 5's transactional projection proof".into(),
    ))
}

async fn forged_handler(
    _context: &CausalCommandContext<'_>,
    input: ForgedInput,
) -> Result<PreparedCommand<Accepted<PlanOutput>>, HandlerError> {
    Ok(PreparedCommand::<Accepted<PlanOutput>>::prepare(PlanOutput { id: input.id }).unwrap())
}

async fn json_patch_handler(
    _context: &CausalCommandContext<'_>,
    input: JsonPatchInput,
) -> Result<PreparedCommand<Accepted<PlanOutput>>, HandlerError> {
    Ok(PreparedCommand::<Accepted<PlanOutput>>::prepare(PlanOutput { id: input.id }).unwrap())
}

async fn bigint_key_handler(
    _context: &CausalCommandContext<'_>,
    input: BigIntKeyInput,
) -> Result<PreparedCommand<Accepted<PlanOutput>>, HandlerError> {
    Ok(
        PreparedCommand::<Accepted<PlanOutput>>::prepare(PlanOutput {
            id: input.key.to_string(),
        })
        .unwrap(),
    )
}

async fn nullable_key_handler(
    _context: &CausalCommandContext<'_>,
    input: NullableKeyInput,
) -> Result<PreparedCommand<Accepted<PlanOutput>>, HandlerError> {
    Ok(
        PreparedCommand::<Accepted<PlanOutput>>::prepare(PlanOutput {
            id: input.key.unwrap_or_default(),
        })
        .unwrap(),
    )
}

async fn bigint_relationship_handler(
    _context: &CausalCommandContext<'_>,
    input: BigIntRelationshipInput,
) -> Result<PreparedCommand<Accepted<PlanOutput>>, HandlerError> {
    Ok(
        PreparedCommand::<Accepted<PlanOutput>>::prepare(PlanOutput {
            id: input.target_id,
        })
        .unwrap(),
    )
}

async fn composite_key_handler(
    _context: &CausalCommandContext<'_>,
    input: CompositeKeyInput,
) -> Result<PreparedCommand<Accepted<PlanOutput>>, HandlerError> {
    Ok(PreparedCommand::<Accepted<PlanOutput>>::prepare(PlanOutput { id: input.id }).unwrap())
}

async fn float_effect_handler(
    _context: &CausalCommandContext<'_>,
    input: FloatEffectInput,
) -> Result<PreparedCommand<Accepted<PlanOutput>>, HandlerError> {
    Ok(PreparedCommand::<Accepted<PlanOutput>>::prepare(PlanOutput { id: input.id }).unwrap())
}

fn plan_projector() -> SurfaceProjector {
    SurfaceProjector::new("project_plan")
        .facts(["plan.changed"])
        .models(["PlanView"])
}

fn plan_confirmations() -> distributed::graphql::CompiledConfirmationPlan<PlanInput> {
    let projector = plan_projector();
    confirmations_for(&projector)
}

fn plan_input_defaults() -> distributed::graphql::CompiledInputDefaults<PlanInput> {
    command_input_defaults! {
        input: PlanInput;
        default input.id = uuid_v7();
    }
}

fn forged_input_defaults() -> distributed::graphql::CompiledInputDefaults<PlanInput> {
    distributed::graphql::__command_input_defaults::<PlanInput>([
        distributed::graphql::__input_default_uuid_v7::<PlanInput, ForgedDefaultMarker>(),
    ])
}

fn confirmations_for(
    projector: &SurfaceProjector,
) -> distributed::graphql::CompiledConfirmationPlan<PlanInput> {
    command_confirmations! {
        input: PlanInput;
        confirm projector -> PlanView {
            key { id: input.id },
            partition: input.id
        };
    }
}

fn plan_permissions(role: &str) -> ModelPermissions<PlanView> {
    ModelPermissions::new().grant(role, read().all_columns())
}

fn forged_effects() -> distributed::graphql::CompiledCommandEffects<ForgedInput> {
    let key: distributed::graphql::TypedEffectKey<ForgedView> = __DistributedForgedViewEffectKey {
        id: distributed::graphql::__effect_key_assignment::<
            __DistributedForgedViewEffectModelField_id,
            _,
        >(distributed::graphql::__effect_input::<
            ForgedInput,
            __DistributedForgedInputEffectInputField_id,
        >()),
    }
    .into();
    distributed::graphql::__command_effects::<ForgedInput>([
        distributed::graphql::__effect_patch::<ForgedView>(
            key,
            vec![distributed::graphql::__effect_assignment::<
                ForgedCountMarker,
                _,
            >(distributed::graphql::__effect_input::<
                ForgedInput,
                ForgedTitleMarker,
            >())],
        ),
    ])
}

fn forged_primary_key_assignment_effects() -> distributed::graphql::CompiledCommandEffects<PlanInput>
{
    let key: distributed::graphql::TypedEffectKey<PlanView> = __DistributedPlanViewEffectKey {
        id: distributed::graphql::__effect_key_assignment::<
            __DistributedPlanViewEffectModelField_id,
            _,
        >(distributed::graphql::__effect_input::<
            PlanInput,
            __DistributedPlanInputEffectInputField_id,
        >()),
    }
    .into();
    distributed::graphql::__command_effects::<PlanInput>([distributed::graphql::__effect_patch::<
        PlanView,
    >(
        key,
        vec![distributed::graphql::__effect_assignment::<
            __DistributedPlanViewEffectModelField_id,
            _,
        >(distributed::graphql::__effect_input::<
            PlanInput,
            __DistributedPlanInputEffectInputField_id,
        >())],
    )])
}

fn two_confirmation_plan(
    reverse: bool,
) -> distributed::graphql::CompiledConfirmationPlan<PlanInput> {
    let first = SurfaceProjector::new("project_a")
        .facts(["plan.changed"])
        .models(["PlanView"]);
    let second = SurfaceProjector::new("project_b")
        .facts(["plan.changed"])
        .models(["PlanView"]);
    if reverse {
        command_confirmations! {
            input: PlanInput;
            confirm second -> PlanView { key { id: input.id } };
            confirm first -> PlanView { key { id: input.id } };
        }
    } else {
        command_confirmations! {
            input: PlanInput;
            confirm first -> PlanView { key { id: input.id } };
            confirm second -> PlanView { key { id: input.id } };
        }
    }
}

fn service_a(service_id: &str) -> Service {
    Service::new().named(service_id).routes(
        Routes::new()
            .typed_command(typed_command::<InputA, Accepted<OutputA>>("todo.create"))
            .handle(handler_a),
    )
}

fn service_b(service_id: &str) -> Service {
    Service::new().named(service_id).routes(
        Routes::new()
            .typed_command(typed_command::<InputB, Accepted<OutputB>>("todo.create"))
            .handle(handler_b),
    )
}

fn guarded_service_a(service_id: &str) -> Service {
    Service::new().named(service_id).routes(
        Routes::new()
            .typed_command(typed_command::<InputA, Accepted<OutputA>>("todo.create"))
            .guarded(
                |_| {
                    GRAPHQL_TYPED_GUARD_INVOKED.store(true, Ordering::SeqCst);
                    true
                },
                guarded_handler_a,
            ),
    )
}

fn pool() -> sqlx::SqlitePool {
    sqlx::SqlitePool::connect_lazy("sqlite::memory:").unwrap()
}

#[tokio::test]
async fn service_id_mismatch_fails_in_both_builder_call_orders() {
    let service = service_a("todos");

    let before = GraphqlEngine::builder(pool())
        .service_id("wrong")
        .service(&service)
        .build()
        .err()
        .expect("service ID mismatch must fail");
    assert!(before
        .to_string()
        .contains("does not match executable service ID"));

    let after = GraphqlEngine::builder(pool())
        .service(&service)
        .service_id("wrong")
        .build()
        .err()
        .expect("service ID overwrite must fail");
    assert!(after
        .to_string()
        .contains("does not match bound executable service ID"));
}

#[tokio::test]
async fn bound_commands_cannot_be_replaced_even_with_an_empty_registry() {
    let service = service_a("todos");
    let error = GraphqlEngine::builder(pool())
        .service(&service)
        .commands(GraphqlCommands::new())
        .build()
        .err()
        .expect("bound mutation inventory must not be clearable");
    assert!(error
        .to_string()
        .contains("cannot replace commands derived"));
}

#[tokio::test]
async fn attachment_checks_exact_rust_types_after_structural_parity() {
    let engine_source = service_a("todos");
    let engine = GraphqlEngine::builder(pool())
        .service(&engine_source)
        .build()
        .unwrap();
    let executable = service_b("todos");

    let error = executable
        .try_with_graphql(engine)
        .err()
        .expect("lookalike Rust types must not bind");
    assert!(error.to_string().contains("TypeId mismatch"));
}

#[tokio::test]
async fn attachment_checks_full_structure_and_service_identity() {
    let engine_source = service_a("todos");
    let engine = GraphqlEngine::builder(pool())
        .service(&engine_source)
        .build()
        .unwrap();
    let structurally_different = Service::new().named("todos").routes(
        Routes::new()
            .typed_command(
                typed_command::<InputA, Accepted<OutputA>>("todo.create")
                    .field_name("createSomethingElse"),
            )
            .handle(handler_a),
    );
    let error = structurally_different
        .try_with_graphql(engine)
        .err()
        .expect("structural drift must not bind");
    assert!(error
        .to_string()
        .contains("structural fingerprint mismatch"));

    let engine_source = service_a("todos-a");
    let engine = GraphqlEngine::builder(pool())
        .service(&engine_source)
        .build()
        .unwrap();
    let wrong_service = service_a("todos-b");
    let error = wrong_service
        .try_with_graphql(engine)
        .err()
        .expect("service identity drift must not bind");
    assert!(error.to_string().contains("service ID mismatch"));
}

#[tokio::test]
async fn projector_topology_identity_drift_changes_service_binding_fingerprint() {
    let declared = SurfaceProjector::new("project_plan")
        .facts(["plan.changed"])
        .models(["PlanView"]);
    let engine_source = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(
                typed_command::<PlanInput, Accepted<PlanOutput>>("plan.create")
                    .confirmations(confirmations_for(&declared)),
            )
            .handle(accepted_plan_handler),
    );
    let engine = GraphqlEngine::builder(pool())
        .model::<PlanView>(plan_permissions("anonymous"))
        .service(&engine_source)
        .client_projectors([declared])
        .build()
        .unwrap();

    let drifted = SurfaceProjector::new("project_plan")
        .facts(["plan.renamed"])
        .models(["PlanView"]);
    let executable = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(
                typed_command::<PlanInput, Accepted<PlanOutput>>("plan.create")
                    .confirmations(confirmations_for(&drifted)),
            )
            .handle(accepted_plan_handler),
    );
    let error = executable
        .try_with_graphql(engine)
        .err()
        .expect("captured projector topology drift must change service identity");
    assert!(error
        .to_string()
        .contains("structural fingerprint mismatch"));
}

#[tokio::test]
async fn matched_typed_inventory_attaches_while_mutation_dispatch_remains_fail_closed() {
    let service = service_a("todos");
    let engine = GraphqlEngine::builder(pool())
        .service(&service)
        .build()
        .unwrap();
    let service = service
        .try_with_graphql(engine)
        .expect("validated inventory may serve reads before task 5");
    let engine = service.graphql_engine().unwrap();
    let query = engine
        .execute(&Session::new(), Request::new("{ __typename }"))
        .await;
    assert!(query.errors.is_empty(), "{query:?}");
    let mutation = engine
        .execute(
            &Session::new(),
            Request::new("mutation { todo_create(input: { id: \"todo-1\" }) { id } }"),
        )
        .await;
    assert_eq!(mutation.errors.len(), 1, "{mutation:?}");
    assert!(mutation.errors[0]
        .message
        .contains("durable command committer"));
}

#[tokio::test]
async fn every_graphql_dispatch_path_fences_before_typed_guards_and_handlers() {
    GRAPHQL_TYPED_GUARD_INVOKED.store(false, Ordering::SeqCst);
    GRAPHQL_TYPED_HANDLER_INVOKED.store(false, Ordering::SeqCst);
    let service = Arc::new(guarded_service_a("todos"));
    let engine = Arc::new(
        GraphqlEngine::builder(pool())
            .service(&service)
            .build()
            .unwrap(),
    );
    let mutation = "mutation { todo_create(input: { id: \"todo-1\" }) { id } }";

    let response = engine
        .execute(&Session::new(), Request::new(mutation))
        .await;
    assert_eq!(response.errors.len(), 1, "{response:?}");

    let streamed = engine
        .execute_stream(&Session::new(), Request::new(mutation))
        .next()
        .await
        .expect("mutation stream emits one fail-closed response");
    assert_eq!(streamed.errors.len(), 1, "{streamed:?}");

    let router = graphql_router_with_service(Arc::clone(&engine), Arc::clone(&service));
    let response = router
        .oneshot(
            HttpRequest::post("/graphql")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "query": mutation }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert!(!GRAPHQL_TYPED_GUARD_INVOKED.load(Ordering::SeqCst));
    assert!(!GRAPHQL_TYPED_HANDLER_INVOKED.load(Ordering::SeqCst));
}

#[tokio::test]
async fn router_construction_runs_full_binding_validation() {
    let engine_source = service_a("todos");
    let engine = Arc::new(
        GraphqlEngine::builder(pool())
            .service(&engine_source)
            .build()
            .unwrap(),
    );
    let executable = Arc::new(service_b("todos"));

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        graphql_router_with_service(engine, executable)
    }))
    .expect_err("router must not serve a mismatched typed inventory");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("unknown panic");
    assert!(
        message.contains("TypeId mismatch"),
        "unexpected panic: {message}"
    );
}

#[tokio::test]
async fn query_only_service_binding_executes_without_a_command_committer() {
    let service = Service::new().named("query-only");
    let engine = GraphqlEngine::builder(pool())
        .service(&service)
        .build()
        .unwrap();
    let response = engine
        .execute(&Session::new(), Request::new("{ __typename }"))
        .await;
    assert!(response.errors.is_empty(), "{response:?}");
    service
        .try_with_graphql(engine)
        .expect("query-only identity binding must attach without task 5");
}

#[test]
fn pool_free_typed_export_preserves_service_provenance_and_rejects_relabeling() {
    let service = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(typed_command::<PlanInput, Accepted<PlanOutput>>(
                "plan.create",
            ))
            .handle(accepted_plan_handler),
    );
    let catalog = build_surface(&[PlanView::schema().clone()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_service(&service)
        .unwrap();
    let selected = surface_for_role(
        &catalog,
        "anonymous",
        &std::collections::BTreeMap::from([("PlanView".into(), RoleGrant::all_columns())]),
    )
    .unwrap();
    let project = DistributedProjectManifest::new("plans").table_schema(PlanView::schema().clone());
    let manifest = DistributedClientSurfaceExport::from_project(&project, selected.clone())
        .unwrap()
        .manifest()
        .unwrap();
    assert_eq!(manifest.service_id, "plans");
    assert_eq!(manifest.commands[0].name, "plan.create");

    let relabeled =
        DistributedProjectManifest::new("other-plans").table_schema(PlanView::schema().clone());
    let error = DistributedClientSurfaceExport::from_project(&relabeled, selected).unwrap_err();
    assert!(error
        .to_string()
        .contains("does not match typed Surface provenance"));
}

#[test]
fn pool_free_service_command_inventory_is_exclusive_in_both_call_orders() {
    let service = service_a("todos");
    let empty = GraphqlCommands::new();

    let service_then_registry = build_surface(&[], &SurfaceOptions::sqlite())
        .unwrap()
        .with_service(&service)
        .unwrap()
        .with_commands(&empty)
        .unwrap_err();
    assert!(service_then_registry.contains("frozen after attachment from the executable Service"));

    let registry_then_service = build_surface(&[], &SurfaceOptions::sqlite())
        .unwrap()
        .with_commands(&empty)
        .unwrap()
        .with_service(&service)
        .unwrap_err();
    assert!(registry_then_service.contains("cannot replace an already attached command inventory"));
}

#[test]
fn pool_free_service_and_projector_topology_validate_in_both_call_orders() {
    let make_service = || {
        Service::new().named("plans").routes(
            Routes::new()
                .typed_command(
                    typed_command::<PlanInput, Accepted<PlanOutput>>("plan.create")
                        .confirmations(plan_confirmations()),
                )
                .handle(accepted_plan_handler),
        )
    };
    let tables = [PlanView::schema().clone()];

    build_surface(&tables, &SurfaceOptions::sqlite())
        .unwrap()
        .with_service(&make_service())
        .unwrap()
        .with_projectors([plan_projector()])
        .unwrap();
    build_surface(&tables, &SurfaceOptions::sqlite())
        .unwrap()
        .with_projectors([plan_projector()])
        .unwrap()
        .with_service(&make_service())
        .unwrap();

    let unknown = build_surface(&tables, &SurfaceOptions::sqlite())
        .unwrap()
        .with_service(&make_service())
        .unwrap()
        .with_projectors([SurfaceProjector::new("some_other_projector")
            .facts(["plan.changed"])
            .models(["PlanView"])])
        .unwrap_err();
    assert!(unknown.contains("expects unknown projector `project_plan`"));

    let wrong_model = build_surface(
        &[PlanView::schema().clone(), ForgedView::schema().clone()],
        &SurfaceOptions::sqlite(),
    )
    .unwrap()
    .with_projectors([SurfaceProjector::new("project_plan")
        .facts(["plan.changed"])
        .models(["ForgedView"])])
    .unwrap()
    .with_service(&make_service())
    .unwrap_err();
    assert!(wrong_model.contains("not in the projector topology"));

    let wrong_facts = build_surface(&tables, &SurfaceOptions::sqlite())
        .unwrap()
        .with_service(&make_service())
        .unwrap()
        .with_projectors([SurfaceProjector::new("project_plan")
            .facts(["some.other.fact"])
            .models(["PlanView"])])
        .unwrap_err();
    assert!(wrong_facts.contains("topology identity does not match"));

    let changed_model_set = build_surface(
        &[PlanView::schema().clone(), ForgedView::schema().clone()],
        &SurfaceOptions::sqlite(),
    )
    .unwrap()
    .with_service(&make_service())
    .unwrap()
    .with_projectors([SurfaceProjector::new("project_plan")
        .facts(["plan.changed"])
        .models(["PlanView", "ForgedView"])])
    .unwrap_err();
    assert!(changed_model_set.contains("topology identity does not match"));

    let captured_reordered = SurfaceProjector::new("project_plan")
        .facts(["plan.changed", "plan.created", "plan.changed"])
        .models(["ForgedView", "PlanView", "PlanView"]);
    let service = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(
                typed_command::<PlanInput, Accepted<PlanOutput>>("plan.create")
                    .confirmations(confirmations_for(&captured_reordered)),
            )
            .handle(accepted_plan_handler),
    );
    build_surface(
        &[PlanView::schema().clone(), ForgedView::schema().clone()],
        &SurfaceOptions::sqlite(),
    )
    .unwrap()
    .with_service(&service)
    .unwrap()
    .with_projectors([SurfaceProjector::new("project_plan")
        .facts(["plan.created", "plan.changed"])
        .models(["PlanView", "ForgedView"])])
    .expect("fact/model ordering and duplicates are not topology identity drift");
}

#[test]
fn pool_free_selection_rejects_omitted_confirmation_topology() {
    let service = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(
                typed_command::<PlanInput, Fact<PlanOutput>>("plan.fact")
                    .confirmations(plan_confirmations()),
            )
            .handle(fact_plan_handler),
    );
    let catalog = build_surface(&[PlanView::schema().clone()], &SurfaceOptions::sqlite())
        .unwrap()
        .with_service(&service)
        .unwrap();
    let error = surface_for_role(
        &catalog,
        "anonymous",
        &std::collections::BTreeMap::from([("PlanView".into(), RoleGrant::all_columns())]),
    )
    .unwrap_err();
    assert!(error.contains("expects unknown projector `project_plan`"));
}

#[tokio::test]
async fn execute_stream_cannot_dispatch_through_an_injected_legacy_service() {
    let typed = service_a("todos");
    let engine = GraphqlEngine::builder(pool())
        .service(&typed)
        .build()
        .unwrap();
    let invoked = Arc::new(AtomicBool::new(false));
    let handler_flag = Arc::clone(&invoked);
    let raw_service = Arc::new(
        Service::new()
            .named("unrelated")
            .routes(
                Routes::new()
                    .command("todo.create")
                    .handle(move |_: &Context<()>| {
                        let handler_flag = Arc::clone(&handler_flag);
                        async move {
                            handler_flag.store(true, Ordering::SeqCst);
                            Ok(serde_json::json!({ "id": "forged" }))
                        }
                    }),
            ),
    );
    let request = Request::new("mutation { todo_create(input: { id: \"todo-1\" }) { id } }")
        .data(raw_service);
    let response = engine
        .execute_stream(&Session::new(), request)
        .next()
        .await
        .expect("fail-closed stream emits one response");
    assert_eq!(response.errors.len(), 1, "{response:?}");
    assert!(!response.errors.is_empty(), "{response:?}");
    assert!(!invoked.load(Ordering::SeqCst));
}

#[tokio::test]
async fn manifest_populates_typed_consistency_and_revalidation_effects() {
    let service = service_a("todos");
    let engine = GraphqlEngine::builder(pool())
        .service(&service)
        .build()
        .unwrap();
    let manifest = engine.client_manifest_for_role("anonymous").unwrap();
    let command = manifest
        .commands
        .iter()
        .find(|command| command.name == "todo.create")
        .unwrap();

    assert_eq!(
        command.extensions.consistency.as_ref().unwrap().kind,
        "accepted"
    );
    let effects = command.extensions.effects.as_ref().unwrap();
    assert!(effects.operations.is_empty());
    assert_eq!(effects.fallback, "revalidate");
    assert!(!manifest.capabilities.causal_receipts);
    assert!(!manifest.capabilities.confirmed_persistence);
}

#[tokio::test]
async fn generated_input_default_is_reused_as_the_effect_key_and_fingerprinted() {
    let command_with_default = || {
        typed_command::<PlanInput, Accepted<PlanOutput>>("plan.create")
            .input_defaults(plan_input_defaults())
            .confirmations(plan_confirmations())
            .effects(command_effects! {
                input: PlanInput;
                upsert PlanView {
                    key { id: input.id },
                    set { title: input.title, count: 0 }
                };
            })
    };
    let service_with_default = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(command_with_default())
            .handle(accepted_plan_handler),
    );
    let engine = GraphqlEngine::builder(pool())
        .model::<PlanView>(plan_permissions("anonymous"))
        .service(&service_with_default)
        .client_projectors([plan_projector()])
        .build()
        .unwrap();
    let manifest = engine.client_manifest_for_role("anonymous").unwrap();
    let command = &manifest.commands[0];
    let defaults = command.extensions.input_defaults.as_ref().unwrap();
    assert_eq!(defaults.version, 1);
    assert_eq!(defaults.defaults.len(), 1);
    assert_eq!(defaults.defaults[0]["path"], serde_json::json!(["id"]));
    assert_eq!(defaults.defaults[0]["generator"], "uuid_v7");
    let effect = &command.extensions.effects.as_ref().unwrap().operations[0];
    assert_eq!(
        effect["key"]["fields"][0]["value"]["path"],
        serde_json::json!(["id"])
    );
    let confirmation = &command.extensions.confirmations.as_ref().unwrap().expected[0];
    assert_eq!(
        confirmation["key"]["fields"][0]["value"]["path"],
        serde_json::json!(["id"])
    );
    assert_eq!(confirmation["partition"]["path"], serde_json::json!(["id"]));

    let without_default = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(
                typed_command::<PlanInput, Accepted<PlanOutput>>("plan.create")
                    .confirmations(plan_confirmations())
                    .effects(command_effects! {
                        input: PlanInput;
                        upsert PlanView {
                            key { id: input.id },
                            set { title: input.title, count: 0 }
                        };
                    }),
            )
            .handle(accepted_plan_handler),
    );
    let manifest_without_default = GraphqlEngine::builder(pool())
        .model::<PlanView>(plan_permissions("anonymous"))
        .service(&without_default)
        .client_projectors([plan_projector()])
        .build()
        .unwrap()
        .client_manifest_for_role("anonymous")
        .unwrap();
    assert_ne!(
        manifest.schema_fingerprint,
        manifest_without_default.schema_fingerprint
    );
    let error = without_default
        .try_with_graphql(engine)
        .err()
        .expect("input-default drift must change the service binding fingerprint");
    assert!(error
        .to_string()
        .contains("structural fingerprint mismatch"));
}

#[tokio::test]
async fn forged_input_default_marker_is_revalidated_against_wire_shape() {
    let service = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(
                typed_command::<PlanInput, Accepted<PlanOutput>>("plan.create")
                    .input_defaults(forged_input_defaults()),
            )
            .handle(accepted_plan_handler),
    );
    let error = GraphqlEngine::builder(pool())
        .service(&service)
        .build()
        .err()
        .expect("forged default markers must not bypass Surface validation");
    assert!(error
        .to_string()
        .contains("references unknown field `missing`"));
}

#[tokio::test]
async fn generated_input_default_uses_the_canonical_renamed_wire_path() {
    let service = Service::new().named("todos").routes(
        Routes::new()
            .typed_command(
                typed_command::<RenamedDefaultInput, Accepted<PlanOutput>>("todo.create")
                    .input_defaults(command_input_defaults! {
                        input: RenamedDefaultInput;
                        default input.id = uuid_v7();
                    }),
            )
            .handle(renamed_default_handler),
    );
    let manifest = GraphqlEngine::builder(pool())
        .service(&service)
        .build()
        .unwrap()
        .client_manifest_for_role("anonymous")
        .unwrap();
    let defaults = manifest.commands[0]
        .extensions
        .input_defaults
        .as_ref()
        .unwrap();
    assert_eq!(defaults.defaults[0]["path"], serde_json::json!(["todoId"]));
}

#[tokio::test]
async fn json_container_leaves_reach_the_manifest() {
    let json_service = Service::new().named("json").routes(
        Routes::new()
            .typed_command(
                typed_command::<JsonPatchInput, Accepted<PlanOutput>>("json.patch").effects(
                    command_effects! {
                        input: JsonPatchInput;
                        patch JsonView {
                            key { id: input.id },
                            set { tags: input.tags, details: input.details }
                        };
                    },
                ),
            )
            .handle(json_patch_handler),
    );
    let manifest = GraphqlEngine::builder(pool())
        .model::<JsonView>(ModelPermissions::new().grant("anonymous", read().all_columns()))
        .service(&json_service)
        .build()
        .unwrap()
        .client_manifest_for_role("anonymous")
        .unwrap();
    let effects = manifest.commands[0].extensions.effects.as_ref().unwrap();
    assert_eq!(effects.operations.len(), 1);
    let effects_json = serde_json::to_string(&effects.operations).unwrap();
    assert!(effects_json.contains("tags"), "{effects_json}");
    assert!(effects_json.contains("details"), "{effects_json}");
}

#[tokio::test]
async fn embedded_primary_keys_reject_keyed_effects_while_composite_keys_remain_normalized() {
    let bigint_manifest = GraphqlEngine::builder(pool())
        .service_id("bigint-read")
        .model::<BigIntKeyView>(ModelPermissions::new().grant("anonymous", read().all_columns()))
        .build()
        .unwrap()
        .client_manifest_for_role("anonymous")
        .unwrap();
    assert_eq!(
        bigint_manifest.models[0].normalization,
        distributed::graphql::ModelNormalization::Embedded
    );
    let bigint_service = Service::new().named("bigint").routes(
        Routes::new()
            .typed_command(
                typed_command::<BigIntKeyInput, Accepted<PlanOutput>>("bigint.upsert").effects(
                    command_effects! {
                        input: BigIntKeyInput;
                        upsert BigIntKeyView {
                            key { key: input.key },
                            set { title: input.title }
                        };
                    },
                ),
            )
            .handle(bigint_key_handler),
    );
    let error = GraphqlEngine::builder(pool())
        .model::<BigIntKeyView>(ModelPermissions::new().grant("anonymous", read().all_columns()))
        .service(&bigint_service)
        .build()
        .err()
        .expect("BigInt identities must not accept keyed optimistic effects");
    assert!(error.to_string().contains("embedded model `BigIntKeyView`"));

    let relationship_service = Service::new().named("bigint-relationship").routes(
        Routes::new()
            .typed_command(
                typed_command::<BigIntRelationshipInput, Accepted<PlanOutput>>("bigint.link")
                    .effects(command_effects! {
                        input: BigIntRelationshipInput;
                        link BigIntRelationshipSource.targets -> BigIntRelationshipTarget {
                            source { key: input.source_key },
                            target { id: input.target_id }
                        };
                    }),
            )
            .handle(bigint_relationship_handler),
    );
    let error = GraphqlEngine::builder(pool())
        .model::<BigIntRelationshipSource>(
            ModelPermissions::new().grant("anonymous", read().all_columns()),
        )
        .model::<BigIntRelationshipTarget>(
            ModelPermissions::new().grant("anonymous", read().all_columns()),
        )
        .service(&relationship_service)
        .build()
        .err()
        .expect("relationship effects must reject embedded source identities");
    assert!(error
        .to_string()
        .contains("embedded model `BigIntRelationshipSource`"));

    let schema_error = NullableKeyView::schema()
        .validate()
        .expect_err("relational primary keys cannot be nullable");
    assert!(schema_error
        .to_string()
        .contains("primary-key column `key` must be non-null"));
    let nullable_service = Service::new().named("nullable").routes(
        Routes::new()
            .typed_command(
                typed_command::<NullableKeyInput, Accepted<PlanOutput>>("nullable.delete").effects(
                    command_effects! {
                        input: NullableKeyInput;
                        delete NullableKeyView { key { key: input.key } };
                    },
                ),
            )
            .handle(nullable_key_handler),
    );
    let error = GraphqlEngine::builder(pool())
        .model::<NullableKeyView>(ModelPermissions::new().grant("anonymous", read().all_columns()))
        .service(&nullable_service)
        .build()
        .err()
        .expect("nullable identities must be rejected before keyed optimistic effects");
    assert!(error
        .to_string()
        .contains("primary-key column `key` must be non-null"));

    let composite_service = Service::new().named("composite").routes(
        Routes::new()
            .typed_command(
                typed_command::<CompositeKeyInput, Accepted<PlanOutput>>("composite.patch")
                    .effects(command_effects! {
                        input: CompositeKeyInput;
                        patch CompositeKeyView {
                            key { tenant_id: input.tenant_id, id: input.id },
                            set { title: input.title }
                        };
                    }),
            )
            .handle(composite_key_handler),
    );
    let composite_manifest = GraphqlEngine::builder(pool())
        .model::<CompositeKeyView>(ModelPermissions::new().grant("anonymous", read().all_columns()))
        .service(&composite_service)
        .build()
        .unwrap()
        .client_manifest_for_role("anonymous")
        .unwrap();
    let distributed::graphql::ModelNormalization::Normalized { fields, .. } =
        &composite_manifest.models[0].normalization
    else {
        panic!("ordinary non-null composite identity must remain normalized");
    };
    assert_eq!(
        fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        ["tenant_id", "id"]
    );
    assert_eq!(
        composite_manifest.commands[0]
            .extensions
            .effects
            .as_ref()
            .unwrap()
            .operations
            .len(),
        1
    );
}

#[tokio::test]
async fn embedded_models_retain_global_invalidation_and_server_resolved_confirmations() {
    let projector = SurfaceProjector::new("project_bigint")
        .facts(["bigint.changed"])
        .models(["BigIntKeyView"]);
    let confirmations = command_confirmations! {
        input: BigIntKeyInput;
        confirm projector -> BigIntKeyView { key { key: input.key } };
    };
    let service = Service::new().named("bigint").routes(
        Routes::new()
            .typed_command(
                typed_command::<BigIntKeyInput, Accepted<PlanOutput>>("bigint.invalidate")
                    .confirmations(confirmations)
                    .effects(command_effects! {
                        input: BigIntKeyInput;
                        invalidate BigIntKeyView;
                    }),
            )
            .handle(bigint_key_handler),
    );
    let manifest = GraphqlEngine::builder(pool())
        .model::<BigIntKeyView>(ModelPermissions::new().grant("anonymous", read().all_columns()))
        .service(&service)
        .client_projectors([projector])
        .build()
        .unwrap()
        .client_manifest_for_role("anonymous")
        .unwrap();

    assert_eq!(
        manifest.models[0].normalization,
        distributed::graphql::ModelNormalization::Embedded
    );
    let command = &manifest.commands[0];
    assert_eq!(
        command.extensions.effects.as_ref().unwrap().operations[0]["kind"],
        "invalidate_model"
    );
    let confirmations = command.extensions.confirmations.as_ref().unwrap();
    assert_eq!(confirmations.kind, "finite");
    assert_eq!(confirmations.expected[0]["model"], "BigIntKeyView");
    assert_eq!(
        confirmations.expected[0]["key"]["fields"][0]["value"]["path"],
        serde_json::json!(["key"])
    );
}

#[tokio::test]
async fn upsert_and_patch_effects_cannot_assign_primary_key_fields() {
    let service = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(
                typed_command::<PlanInput, Accepted<PlanOutput>>("plan.bad_patch")
                    .effects(forged_primary_key_assignment_effects()),
            )
            .handle(accepted_plan_handler),
    );
    let error = GraphqlEngine::builder(pool())
        .model::<PlanView>(plan_permissions("anonymous"))
        .service(&service)
        .build()
        .err()
        .expect("forged primary-key assignments must fail Surface validation");
    assert!(error
        .to_string()
        .contains("cannot assign primary-key field"));
}

#[tokio::test]
async fn accepted_finite_confirmation_is_exported_and_marks_the_projector_causal() {
    let command = typed_command::<PlanInput, Accepted<PlanOutput>>("plan.create")
        .input_defaults(plan_input_defaults())
        .confirmations(plan_confirmations())
        .effects(command_effects! {
            input: PlanInput;
            upsert PlanView {
                key { id: input.id },
                set { title: input.title, count: 0 }
            };
        });
    let service = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(command)
            .handle(accepted_plan_handler),
    );
    let engine = GraphqlEngine::builder(pool())
        .model::<PlanView>(plan_permissions("anonymous"))
        .service(&service)
        .client_projectors([plan_projector()])
        .build()
        .unwrap();
    let manifest = engine.client_manifest_for_role("anonymous").unwrap();
    let command = manifest
        .commands
        .iter()
        .find(|command| command.name == "plan.create")
        .unwrap();
    let confirmations = command.extensions.confirmations.as_ref().unwrap();
    assert_eq!(confirmations.kind, "finite");
    assert_eq!(confirmations.expected.len(), 1);
    assert_eq!(confirmations.expected[0]["projector"], "project_plan");
    assert_eq!(confirmations.expected[0]["model"], "PlanView");
    assert!(confirmations.expected[0]
        .get("projector_topology")
        .is_none());
    assert!(confirmations.expected[0].get("facts").is_none());
    assert_eq!(confirmations.fallback, "revalidate");
    assert!(
        manifest
            .projectors
            .iter()
            .find(|projector| projector.name == "project_plan")
            .unwrap()
            .causal_confirmation
    );
}

#[tokio::test]
async fn text_backed_enum_constant_reaches_a_valid_client_manifest() {
    let command =
        typed_command::<PlanInput, Accepted<PlanOutput>>("plan.close").effects(command_effects! {
            input: PlanInput;
            patch PlanView {
                key { id: input.id },
                set { status: constant(PlanStatus::Closed) }
            };
        });
    let service = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(command)
            .handle(accepted_plan_handler),
    );
    let engine = GraphqlEngine::builder(pool())
        .model::<PlanView>(plan_permissions("anonymous"))
        .service(&service)
        .build()
        .unwrap();
    let manifest = engine.client_manifest_for_role("anonymous").unwrap();
    let operations = &manifest.commands[0]
        .extensions
        .effects
        .as_ref()
        .unwrap()
        .operations;
    assert_eq!(operations[0]["fields"][0]["field"], "status");
    assert_eq!(operations[0]["fields"][0]["value"]["value"], "Closed");
}

#[tokio::test]
async fn fallible_constant_serialization_returns_a_build_error_without_panicking() {
    let result = std::panic::catch_unwind(|| {
        let service = Service::new().named("broken").routes(
            Routes::new()
                .typed_command(
                    typed_command::<PlanInput, Accepted<PlanOutput>>("broken.patch").effects(
                        command_effects! {
                            input: PlanInput;
                            patch BrokenConstantView {
                                key { id: input.id },
                                set { value: constant(BrokenText) }
                            };
                        },
                    ),
                )
                .handle(accepted_plan_handler),
        );
        GraphqlEngine::builder(pool())
            .model::<BrokenConstantView>(
                ModelPermissions::new().grant("anonymous", read().all_columns()),
            )
            .service(&service)
            .build()
            .err()
            .expect("invalid constant serialization must be a configuration error")
            .to_string()
    });
    let error = result.expect("constant construction and registry build must not panic");
    assert!(error.contains("constant effect value failed to serialize"));
    assert!(error.contains("broken constant serializer"));
}

#[tokio::test]
async fn nonfinite_float_constant_is_rejected_but_explicit_null_is_portable() {
    let nonfinite = Service::new().named("floats").routes(
        Routes::new()
            .typed_command(
                typed_command::<FloatEffectInput, Accepted<PlanOutput>>("float.nan").effects(
                    command_effects! {
                        input: FloatEffectInput;
                        patch FloatEffectView {
                            key { id: input.id },
                            set { value: constant(f64::NAN) }
                        };
                    },
                ),
            )
            .handle(float_effect_handler),
    );
    let error = GraphqlEngine::builder(pool())
        .model::<FloatEffectView>(ModelPermissions::new().grant("anonymous", read().all_columns()))
        .service(&nonfinite)
        .build()
        .err()
        .expect("non-finite Float constants must not become implicit SQL null");
    assert!(error
        .to_string()
        .contains("non-finite f32/f64 constants cannot be represented"));

    let explicit_null = Service::new().named("floats").routes(
        Routes::new()
            .typed_command(
                typed_command::<FloatEffectInput, Accepted<PlanOutput>>("float.clear").effects(
                    command_effects! {
                        input: FloatEffectInput;
                        patch FloatEffectView {
                            key { id: input.id },
                            set { value: null() }
                        };
                    },
                ),
            )
            .handle(float_effect_handler),
    );
    let manifest = GraphqlEngine::builder(pool())
        .model::<FloatEffectView>(ModelPermissions::new().grant("anonymous", read().all_columns()))
        .service(&explicit_null)
        .build()
        .unwrap()
        .client_manifest_for_role("anonymous")
        .unwrap();
    assert_eq!(
        manifest.commands[0]
            .extensions
            .effects
            .as_ref()
            .unwrap()
            .operations[0]["fields"][0]["value"]["kind"],
        "null"
    );
}

#[tokio::test]
async fn json_backed_constants_reject_nonfinite_floats_but_preserve_json_null() {
    let nonfinite_f32 = Service::new().named("json-floats").routes(
        Routes::new()
            .typed_command(
                typed_command::<FloatEffectInput, Accepted<PlanOutput>>("json-float.infinity")
                    .effects(command_effects! {
                        input: FloatEffectInput;
                        patch JsonFloatEffectView {
                            key { id: input.id },
                            set { value_f32: constant(f32::INFINITY) }
                        };
                    }),
            )
            .handle(float_effect_handler),
    );
    let error = GraphqlEngine::builder(pool())
        .model::<JsonFloatEffectView>(
            ModelPermissions::new().grant("anonymous", read().all_columns()),
        )
        .service(&nonfinite_f32)
        .build()
        .err()
        .expect("non-finite f32 JSON constants must fail before serialization");
    assert!(error
        .to_string()
        .contains("non-finite f32/f64 constants cannot be represented"));

    let nonfinite_f64 = Service::new().named("json-floats").routes(
        Routes::new()
            .typed_command(
                typed_command::<FloatEffectInput, Accepted<PlanOutput>>("json-float.nan").effects(
                    command_effects! {
                        input: FloatEffectInput;
                        patch JsonFloatEffectView {
                            key { id: input.id },
                            set { value_f64: constant(f64::NAN) }
                        };
                    },
                ),
            )
            .handle(float_effect_handler),
    );
    let error = GraphqlEngine::builder(pool())
        .model::<JsonFloatEffectView>(
            ModelPermissions::new().grant("anonymous", read().all_columns()),
        )
        .service(&nonfinite_f64)
        .build()
        .err()
        .expect("non-finite f64 JSON constants must fail before serialization");
    assert!(error
        .to_string()
        .contains("non-finite f32/f64 constants cannot be represented"));

    let nested_nonfinite = Service::new().named("json-floats").routes(
        Routes::new()
            .typed_command(
                typed_command::<FloatEffectInput, Accepted<PlanOutput>>("json-float.nested")
                    .effects(command_effects! {
                        input: FloatEffectInput;
                        patch JsonFloatEffectView {
                            key { id: input.id },
                            set { nested_document: constant(NONFINITE_JSON_DOCUMENT) }
                        };
                    }),
            )
            .handle(float_effect_handler),
    );
    let error = GraphqlEngine::builder(pool())
        .model::<JsonFloatEffectView>(
            ModelPermissions::new().grant("anonymous", read().all_columns()),
        )
        .service(&nested_nonfinite)
        .build()
        .err()
        .expect("nested non-finite JSON constants must fail before serialization");
    assert!(error
        .to_string()
        .contains("non-finite f32/f64 constants cannot be represented"));

    let json_null = Service::new().named("json-floats").routes(
        Routes::new()
            .typed_command(
                typed_command::<FloatEffectInput, Accepted<PlanOutput>>("json.clear").effects(
                    command_effects! {
                        input: FloatEffectInput;
                        patch JsonFloatEffectView {
                            key { id: input.id },
                            set { document: constant(serde_json::Value::Null) }
                        };
                    },
                ),
            )
            .handle(float_effect_handler),
    );
    let manifest = GraphqlEngine::builder(pool())
        .model::<JsonFloatEffectView>(
            ModelPermissions::new().grant("anonymous", read().all_columns()),
        )
        .service(&json_null)
        .build()
        .unwrap()
        .client_manifest_for_role("anonymous")
        .unwrap();
    let value = &manifest.commands[0]
        .extensions
        .effects
        .as_ref()
        .unwrap()
        .operations[0]["fields"][0]["value"];
    assert_eq!(value["kind"], "constant");
    assert!(value["value"].is_null());
}

#[tokio::test]
async fn consistency_confirmation_matrix_fails_closed() {
    let missing_fact = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(typed_command::<PlanInput, Fact<PlanOutput>>("plan.fact"))
            .handle(fact_plan_handler),
    );
    let error = GraphqlEngine::builder(pool())
        .service(&missing_fact)
        .build()
        .err()
        .expect("fact without a finite plan must fail");
    assert!(error
        .to_string()
        .contains("must declare at least one expected projector confirmation"));

    let projected = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(
                typed_command::<PlanInput, Projected<PlanOutput>>("plan.projected")
                    .confirmations(plan_confirmations()),
            )
            .handle(projected_plan_handler),
    );
    let error = GraphqlEngine::builder(pool())
        .service(&projected)
        .build()
        .err()
        .expect("projected with async confirmation must fail");
    assert!(error
        .to_string()
        .contains("cannot declare asynchronous projector confirmations"));
}

#[tokio::test]
async fn role_redaction_erases_the_whole_confirmation_and_optimistic_plan() {
    let command = typed_command::<PlanInput, Accepted<PlanOutput>>("plan.create")
        .input_defaults(plan_input_defaults())
        .confirmations(plan_confirmations())
        .effects(command_effects! {
            input: PlanInput;
            patch PlanView {
                key { id: input.id },
                set { title: input.title }
            };
        });
    let service = Service::new().named("plans").routes(
        Routes::new()
            .typed_command(command)
            .handle(accepted_plan_handler),
    );
    let engine = GraphqlEngine::builder(pool())
        .model::<PlanView>(ModelPermissions::new().grant("user", read().columns(["title"])))
        .service(&service)
        .client_projectors([plan_projector()])
        .build()
        .unwrap();
    let manifest = engine.client_manifest_for_role("user").unwrap();
    let command = &manifest.commands[0];
    assert!(command.extensions.input_defaults.is_some());
    let confirmations = command.extensions.confirmations.as_ref().unwrap();
    assert_eq!(confirmations.kind, "unavailable");
    assert!(confirmations.expected.is_empty());
    assert_eq!(confirmations.fallback, "revalidate");
    let effects = command.extensions.effects.as_ref().unwrap();
    assert!(effects.operations.is_empty());
    assert_eq!(effects.fallback, "revalidate");
    assert!(manifest
        .projectors
        .iter()
        .all(|projector| !projector.causal_confirmation));
}

#[tokio::test]
async fn forged_name_valid_marker_types_are_rejected_from_wire_metadata() {
    let service = Service::new().named("forged").routes(
        Routes::new()
            .typed_command(
                typed_command::<ForgedInput, Accepted<PlanOutput>>("forged.patch")
                    .effects(forged_effects()),
            )
            .handle(forged_handler),
    );
    let error = GraphqlEngine::builder(pool())
        .model::<ForgedView>(ModelPermissions::new().grant("anonymous", read().all_columns()))
        .service(&service)
        .build()
        .err()
        .expect("wire String -> BigInt must fail despite forged marker Value types");
    assert!(error.to_string().contains("has GraphQL type `String`"));
    assert!(error.to_string().contains("requires `BigInt`"));
}

#[tokio::test]
async fn confirmation_set_order_does_not_change_manifest_fingerprint() {
    let build = |reverse| {
        let service = Service::new().named("plans").routes(
            Routes::new()
                .typed_command(
                    typed_command::<PlanInput, Accepted<PlanOutput>>("plan.create")
                        .confirmations(two_confirmation_plan(reverse)),
                )
                .handle(accepted_plan_handler),
        );
        GraphqlEngine::builder(pool())
            .model::<PlanView>(plan_permissions("anonymous"))
            .service(&service)
            .client_projectors([
                SurfaceProjector::new("project_a")
                    .facts(["plan.changed"])
                    .models(["PlanView"]),
                SurfaceProjector::new("project_b")
                    .facts(["plan.changed"])
                    .models(["PlanView"]),
            ])
            .build()
            .unwrap()
            .client_manifest_for_role("anonymous")
            .unwrap()
    };
    let first = build(false);
    let second = build(true);
    assert_eq!(first.schema_fingerprint, second.schema_fingerprint);
    assert_eq!(first.commands, second.commands);
}
