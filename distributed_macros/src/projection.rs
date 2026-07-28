use std::collections::BTreeMap;

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{
    braced, bracketed, parenthesized, Expr, ExprCall, ExprField, ExprLit, ExprPath, ExprUnary,
    Ident, Lit, LitInt, LitStr, Member, Path, Result, Token, Type, UnOp,
};

use crate::shared::compact_tokens;

pub(crate) fn expand(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as ProjectionDeclaration);
    expand_declaration(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

struct ProjectionDeclaration {
    name: LitStr,
    version: LitInt,
    epoch: LitStr,
    partition: ProjectionPartitionSyntax,
    arms: Vec<ProjectionArm>,
}

enum ProjectionPartitionSyntax {
    Unit,
    Expression(Expr),
}

struct ProjectionArm {
    selector: EventSelector,
    operations: Vec<Operation>,
}

struct SelectorExpansion {
    selectors: Vec<(TokenStream, String)>,
    binding: Ident,
    body: Type,
    state_body: bool,
}

enum EventSelector {
    State {
        names: Vec<LitStr>,
        version: LitInt,
        binding: Ident,
        body: Type,
    },
    Event {
        event: Type,
        binding: Ident,
    },
    Deletion {
        names: Vec<LitStr>,
        version: LitInt,
        binding: Ident,
        identity: Type,
    },
}

struct Operation {
    kind: OperationKind,
    model: Path,
    key: Vec<FieldAssignment>,
    fields: Vec<FieldAssignment>,
    alias: Option<Ident>,
    related: Option<RelatedSource>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OperationKind {
    Insert,
    Upsert,
    Patch,
    UpsertPatch,
    Delete,
    Recreate,
    InsertRelated,
    UpsertRelated,
    DeleteRelated,
    StateUpsert,
}

struct RelatedSource {
    alias: Ident,
    relationship: Ident,
}

struct FieldAssignment {
    name: Ident,
    value: AssignmentValue,
}

enum AssignmentValue {
    Expression(Expr),
    Unset,
}

impl Parse for ProjectionDeclaration {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut name: Option<LitStr> = None;
        let mut version: Option<LitInt> = None;
        let mut epoch: Option<LitStr> = None;
        let mut partition = None;
        let mut arms = Vec::new();

        while !input.is_empty() {
            let keyword: Ident = input.parse()?;
            match keyword.to_string().as_str() {
                "name" => {
                    input.parse::<Token![:]>()?;
                    set_once(&mut name, input.parse()?, &keyword, "name")?;
                    parse_separator(input)?;
                }
                "version" => {
                    input.parse::<Token![:]>()?;
                    set_once(&mut version, input.parse()?, &keyword, "version")?;
                    parse_separator(input)?;
                }
                "epoch" => {
                    input.parse::<Token![:]>()?;
                    set_once(&mut epoch, input.parse()?, &keyword, "epoch")?;
                    parse_separator(input)?;
                }
                "partition" => {
                    input.parse::<Token![:]>()?;
                    let expression: Expr = input.parse()?;
                    if partition.is_some() {
                        return Err(syn::Error::new_spanned(
                            keyword,
                            "duplicate projection `partition`",
                        ));
                    }
                    partition = Some(
                        if matches!(
                            &expression,
                            Expr::Path(path) if path.path.is_ident("unit")
                        ) {
                            ProjectionPartitionSyntax::Unit
                        } else {
                            ProjectionPartitionSyntax::Expression(expression)
                        },
                    );
                    parse_separator(input)?;
                }
                "on" => arms.push(parse_arm(input)?),
                "eventual" | "direct" => {
                    return Err(syn::Error::new_spanned(
                        keyword,
                        "execution placement is not part of projection!; bind the descriptor with `.eventual()` or `.direct()`",
                    ));
                }
                "invalidate" | "invalidate_model" | "invalidate_relationship" => {
                    return Err(syn::Error::new_spanned(
                        keyword,
                        "invalidation is derived fallback metadata, not an authoritative projection operation",
                    ));
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        keyword,
                        "expected projection header (`name`, `version`, `epoch`, `partition`) or `on`",
                    ));
                }
            }
        }

        let name = name.ok_or_else(|| input.error("projection! requires `name: \"...\"`"))?;
        let version = version.ok_or_else(|| input.error("projection! requires `version: N`"))?;
        if version.base10_parse::<u64>()? == 0 {
            return Err(syn::Error::new_spanned(
                &version,
                "projection version must be greater than zero",
            ));
        }
        let epoch = epoch.ok_or_else(|| input.error("projection! requires `epoch: \"...\"`"))?;
        if epoch.value().is_empty() {
            return Err(syn::Error::new_spanned(
                &epoch,
                "projection epoch must not be empty",
            ));
        }
        let partition =
            partition.ok_or_else(|| input.error("projection! requires `partition: ...`"))?;
        if arms.is_empty() {
            return Err(input.error("projection! requires at least one `on` arm"));
        }
        Ok(Self {
            name,
            version,
            epoch,
            partition,
            arms,
        })
    }
}

fn set_once<T>(slot: &mut Option<T>, value: T, span: &Ident, field: &str) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(syn::Error::new_spanned(
            span,
            format!("duplicate projection `{field}`"),
        ));
    }
    Ok(())
}

fn parse_separator(input: ParseStream<'_>) -> Result<()> {
    if input.peek(Token![;]) {
        input.parse::<Token![;]>()?;
    } else if input.peek(Token![,]) {
        input.parse::<Token![,]>()?;
    } else {
        return Err(input.error("expected `;` or `,`"));
    }
    Ok(())
}

fn parse_arm(input: ParseStream<'_>) -> Result<ProjectionArm> {
    let selector = if input.peek(syn::token::Bracket) || input.peek(LitStr) {
        parse_state_selector(input)?
    } else {
        let event: Type = input.parse()?;
        let arguments;
        parenthesized!(arguments in input);
        let binding: Ident = arguments.parse()?;
        if !arguments.is_empty() {
            return Err(arguments.error("typed event selector accepts one body binding"));
        }
        EventSelector::Event { event, binding }
    };
    let operations;
    braced!(operations in input);
    let mut parsed = Vec::new();
    while !operations.is_empty() {
        parsed.push(parse_operation(&operations)?);
    }
    if parsed.is_empty() {
        return Err(operations.error("projection arm requires at least one operation"));
    }
    Ok(ProjectionArm {
        selector,
        operations: parsed,
    })
}

fn parse_state_selector(input: ParseStream<'_>) -> Result<EventSelector> {
    let mut names = Vec::new();
    if input.peek(syn::token::Bracket) {
        let values;
        bracketed!(values in input);
        while !values.is_empty() {
            names.push(values.parse::<LitStr>()?);
            if values.peek(Token![,]) {
                values.parse::<Token![,]>()?;
            } else if values.peek(Token![|]) {
                values.parse::<Token![|]>()?;
            } else if !values.is_empty() {
                return Err(values.error("separate state event names with `,` or `|`"));
            }
        }
    } else {
        names.push(input.parse::<LitStr>()?);
        while input.peek(Token![|]) {
            input.parse::<Token![|]>()?;
            names.push(input.parse::<LitStr>()?);
        }
    }
    if names.is_empty() {
        return Err(input.error("state selector requires at least one event name"));
    }
    let version_keyword: Ident = input.parse()?;
    if version_keyword != "version" {
        return Err(syn::Error::new_spanned(
            version_keyword,
            "state selector requires `version N`",
        ));
    }
    let version: LitInt = input.parse()?;
    if version.base10_parse::<u64>()? == 0 {
        return Err(syn::Error::new_spanned(
            &version,
            "domain-event version must be greater than zero",
        ));
    }
    let arguments;
    parenthesized!(arguments in input);
    let binding: Ident = arguments.parse()?;
    arguments.parse::<Token![:]>()?;
    let body: Type = arguments.parse()?;
    if !arguments.is_empty() {
        return Err(arguments.error("state selector accepts `binding: DomainStateType`"));
    }
    if binding == "deleted" {
        Ok(EventSelector::Deletion {
            names,
            version,
            binding,
            identity: body,
        })
    } else {
        Ok(EventSelector::State {
            names,
            version,
            binding,
            body,
        })
    }
}

fn parse_operation(input: ParseStream<'_>) -> Result<Operation> {
    let keyword: Ident = input.parse()?;
    let kind = match keyword.to_string().as_str() {
        "insert" => OperationKind::Insert,
        "upsert" => OperationKind::Upsert,
        "patch" => OperationKind::Patch,
        "upsert_patch" => OperationKind::UpsertPatch,
        "delete" => OperationKind::Delete,
        "recreate" => OperationKind::Recreate,
        "insert_related" => OperationKind::InsertRelated,
        "upsert_related" => OperationKind::UpsertRelated,
        "delete_related" => OperationKind::DeleteRelated,
        "link" | "unlink" => {
            return Err(syn::Error::new_spanned(
                keyword,
                "link/unlink are derived relationship consequences; author the related row mutation",
            ));
        }
        "invalidate" | "invalidate_model" | "invalidate_relationship" => {
            return Err(syn::Error::new_spanned(
                keyword,
                "invalidation is fallback policy, not an authoritative row operation",
            ));
        }
        _ => {
            return Err(syn::Error::new_spanned(
                keyword,
                "unknown projection operation; expected insert/upsert/patch/upsert_patch/delete/recreate or a related-row variant",
            ));
        }
    };

    let related = if matches!(
        kind,
        OperationKind::InsertRelated | OperationKind::UpsertRelated | OperationKind::DeleteRelated
    ) {
        let alias: Ident = input.parse()?;
        input.parse::<Token![.]>()?;
        let relationship: Ident = input.parse()?;
        input.parse::<Token![->]>()?;
        Some(RelatedSource {
            alias,
            relationship,
        })
    } else {
        None
    };
    let model: Path = input.parse()?;

    let from_state = if kind == OperationKind::Upsert && input.peek(Ident) {
        let fork = input.fork();
        fork.parse::<Ident>()? == "from"
    } else {
        false
    };
    if from_state {
        input.parse::<Ident>()?;
        let source: Ident = input.parse()?;
        if source != "state" {
            return Err(syn::Error::new_spanned(
                source,
                "upsert shorthand is exactly `upsert Model from state`",
            ));
        }
        let alias = parse_alias(input)?;
        input.parse::<Token![;]>()?;
        return Ok(Operation {
            kind: OperationKind::StateUpsert,
            model,
            key: Vec::new(),
            fields: Vec::new(),
            alias,
            related,
        });
    }

    let contents;
    braced!(contents in input);
    let mut key = None;
    let mut fields = None;
    while !contents.is_empty() {
        let section: Ident = contents.parse()?;
        let section_contents;
        braced!(section_contents in contents);
        let values = parse_field_assignments(&section_contents)?;
        match section.to_string().as_str() {
            "key" => {
                if key.replace(values).is_some() {
                    return Err(syn::Error::new_spanned(
                        section,
                        "duplicate projection operation section",
                    ));
                }
            }
            "set" => {
                if fields.replace(values).is_some() {
                    return Err(syn::Error::new_spanned(
                        section,
                        "duplicate projection operation section",
                    ));
                }
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    section,
                    "projection operation sections are `key` and `set`",
                ));
            }
        }
        if contents.peek(Token![,]) {
            contents.parse::<Token![,]>()?;
        }
    }
    let alias = parse_alias(input)?;
    input.parse::<Token![;]>()?;
    let key = key.ok_or_else(|| input.error("projection row operation requires `key { ... }`"))?;
    let fields = fields.unwrap_or_default();
    match kind {
        OperationKind::Delete | OperationKind::DeleteRelated if !fields.is_empty() => {
            return Err(input.error("delete operations cannot declare `set` fields"));
        }
        OperationKind::Patch | OperationKind::UpsertPatch if fields.is_empty() => {
            return Err(input.error("patch operations require non-empty `set` fields"));
        }
        _ => {}
    }
    Ok(Operation {
        kind,
        model,
        key,
        fields,
        alias,
        related,
    })
}

fn parse_alias(input: ParseStream<'_>) -> Result<Option<Ident>> {
    if input.peek(Token![as]) {
        input.parse::<Token![as]>()?;
        Ok(Some(input.parse()?))
    } else {
        Ok(None)
    }
}

fn parse_field_assignments(input: ParseStream<'_>) -> Result<Vec<FieldAssignment>> {
    let mut fields = Vec::new();
    while !input.is_empty() {
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let value = if input.peek(Ident) {
            let fork = input.fork();
            let ident: Ident = fork.parse()?;
            if ident == "unset" && (fork.is_empty() || fork.peek(Token![,])) {
                input.parse::<Ident>()?;
                AssignmentValue::Unset
            } else {
                AssignmentValue::Expression(input.parse()?)
            }
        } else {
            AssignmentValue::Expression(input.parse()?)
        };
        fields.push(FieldAssignment { name, value });
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        } else if !input.is_empty() {
            return Err(input.error("projection fields must be comma-separated"));
        }
    }
    Ok(fields)
}

fn expand_declaration(declaration: ProjectionDeclaration) -> Result<TokenStream> {
    let name = declaration.name;
    let version = declaration.version;
    let epoch = declaration.epoch;
    let first_selector = declaration
        .arms
        .first()
        .expect("parser requires at least one projection arm");
    let partition_selector = selector_expansions(&first_selector.selector)?;
    let partition_binding = partition_selector.binding;
    let partition_body = partition_selector.body;
    let all_body_types = declaration
        .arms
        .iter()
        .map(|arm| match &arm.selector {
            EventSelector::State { body, .. } => body.clone(),
            EventSelector::Event { event, .. } => event.clone(),
            EventSelector::Deletion { identity, .. } => identity.clone(),
        })
        .collect::<Vec<_>>();
    let has_deletion = declaration
        .arms
        .iter()
        .any(|arm| matches!(&arm.selector, EventSelector::Deletion { .. }));
    let partition = match declaration.partition {
        ProjectionPartitionSyntax::Unit => quote!(distributed::ProjectionPartition::Unit),
        ProjectionPartitionSyntax::Expression(expression) => {
            let (expression, body_field) =
                projection_expression(&expression, &partition_binding, &partition_body)?;
            if body_field.is_some() && has_deletion {
                return Err(syn::Error::new_spanned(
                    expression,
                    "a projection spanning deletion events must partition by a stable envelope field or literal",
                ));
            }
            let compatibility = body_field
                .map(|field| {
                    all_body_types
                        .iter()
                        .map(move |body| {
                    quote! {
                        if distributed::projection::lower::body_path::<#body>(#field)?
                            != __partition_expression
                        {
                            return Err(distributed::ProjectionProgramError::InvalidOperation {
                                operation: "projection partition".to_owned(),
                                reason: "partition body field has incompatible event-body metadata"
                                    .to_owned(),
                            });
                        }
                    }
                })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            quote! {{
                let __partition_expression = #expression;
                #(#compatibility)*
                distributed::ProjectionPartition::Expression(__partition_expression)
            }}
        }
    };
    let mut generated_arms = Vec::new();
    let mut selector_tokens = Vec::new();
    let mut lower_cases = Vec::new();
    let mut output_models = BTreeMap::<String, Path>::new();
    let mut output_relationships = BTreeMap::<String, Path>::new();
    let mut assertions = Vec::new();
    let mut direct_models = Vec::new();
    let mut all_direct = true;
    let mut expanded_arm_index = 0usize;

    for arm in declaration.arms {
        let SelectorExpansion {
            selectors,
            binding,
            body,
            state_body,
        } = selector_expansions(&arm.selector)?;
        if arm.operations.len() == 1
            && matches!(
                arm.operations[0].kind,
                OperationKind::StateUpsert | OperationKind::Upsert
            )
            && arm.operations[0].related.is_none()
        {
            direct_models.push(compact_tokens(&arm.operations[0].model));
        } else {
            all_direct = false;
        }
        for (selector, selector_identity) in selectors {
            let arm_id = LitStr::new(
                &format!("arm-{expanded_arm_index}-{selector_identity}"),
                Span::call_site(),
            );
            let mut aliases = BTreeMap::<String, (Path, Ident)>::new();
            let mut operation_statements = Vec::new();
            let mut operation_variables = Vec::new();
            for (operation_index, operation) in arm.operations.iter().enumerate() {
                let operation_id =
                    LitStr::new(&format!("operation-{operation_index}"), Span::call_site());
                let operation_variable = format_ident!("__projection_operation_{operation_index}");
                let model = &operation.model;
                output_models.insert(compact_tokens(model), model.clone());
                let kind = mutation_kind_tokens(operation.kind);

                let operation_expression = if operation.kind == OperationKind::StateUpsert {
                    if !state_body {
                        return Err(syn::Error::new_spanned(
                            model,
                            "`upsert Model from state` requires a state-body selector",
                        ));
                    }
                    assertions.push(quote! {
                        const _: () =
                            distributed::projection::lower::assert_state_upsert_compatible(
                                <#body as distributed::projection::lower::ProjectionBodyMetadata>::PROJECTION_FIELDS,
                                <#model as distributed::projection::lower::ProjectionReadModelMetadata>::PROJECTION_FIELDS,
                            );
                    });
                    quote! {
                        distributed::projection::lower::state_upsert_operation::<#body, #model>(
                            #operation_id,
                            #operation_index as u32,
                        )?
                    }
                } else {
                    let key =
                        authoring_fields(&operation.key, &binding, &body, model, &mut assertions)?;
                    let fields = authoring_fields(
                        &operation.fields,
                        &binding,
                        &body,
                        model,
                        &mut assertions,
                    )?;
                    if let Some(related) = &operation.related {
                        let Some((parent_model, parent_variable)) =
                            aliases.get(&related.alias.to_string())
                        else {
                            return Err(syn::Error::new_spanned(
                                &related.alias,
                                "related operation must reference a preceding `as alias` operation",
                            ));
                        };
                        let marker = relationship_marker_path(parent_model, &related.relationship)?;
                        output_relationships.insert(compact_tokens(&marker), marker.clone());
                        quote! {
                            distributed::projection::lower::related_operation::<#marker>(
                                #operation_id,
                                #operation_index as u32,
                                #kind,
                                &#parent_variable,
                                vec![#(#key),*],
                                vec![#(#fields),*],
                            )?
                        }
                    } else {
                        quote! {
                            distributed::projection::lower::model_operation::<#model>(
                                #operation_id,
                                #operation_index as u32,
                                #kind,
                                vec![#(#key),*],
                                vec![#(#fields),*],
                            )?
                        }
                    }
                };
                operation_statements.push(quote! {
                    let #operation_variable = #operation_expression;
                });
                operation_variables.push(operation_variable.clone());
                if let Some(alias) = &operation.alias {
                    if aliases
                        .insert(
                            alias.to_string(),
                            (model.clone(), operation_variable.clone()),
                        )
                        .is_some()
                    {
                        return Err(syn::Error::new_spanned(
                            alias,
                            "duplicate projection operation alias",
                        ));
                    }
                }

                let lower_call = if let Some(related) = &operation.related {
                    let Some((parent_model, _)) = aliases.get(&related.alias.to_string()) else {
                        return Err(syn::Error::new_spanned(
                            &related.alias,
                            "related operation must reference a preceding alias",
                        ));
                    };
                    let marker = relationship_marker_path(parent_model, &related.relationship)?;
                    let parent_index = arm
                        .operations
                        .iter()
                        .position(|candidate| {
                            candidate
                                .alias
                                .as_ref()
                                .is_some_and(|alias| alias == &related.alias)
                        })
                        .expect("validated alias has a source operation");
                    let parent_id =
                        LitStr::new(&format!("operation-{parent_index}"), Span::call_site());
                    quote! {
                        distributed::projection::lower::lower_related_mutation::<#marker>(
                            &mut __builder,
                            __resolved,
                            __mutation,
                            #parent_id,
                        )?;
                    }
                } else {
                    quote! {
                        distributed::projection::lower::lower_model_mutation::<#model>(
                            &mut __builder,
                            __mutation,
                        )?;
                    }
                };
                lower_cases.push(quote! {
                    if __mutation.provenance().arm_id() == #arm_id
                        && __mutation
                            .provenance()
                            .operation_ids()
                            .iter()
                            .any(|__id| __id == #operation_id)
                    {
                        #lower_call
                        continue;
                    }
                });
            }
            generated_arms.push(quote! {
                {
                    #(#operation_statements)*
                    distributed::ProjectionArm::try_new(
                        #arm_id,
                        #selector,
                        vec![#(#operation_variables),*],
                    )?
                }
            });
            selector_tokens.push(selector);
            expanded_arm_index += 1;
        }
    }

    if all_direct
        && direct_models
            .first()
            .is_some_and(|first| direct_models.iter().any(|model| model != first))
    {
        all_direct = false;
    }
    let eligibility = if all_direct {
        quote!(distributed::projection::lower::DirectEligible)
    } else {
        quote!(distributed::projection::lower::EventualOnly)
    };
    let output_models = output_models.values();
    let output_relationships = output_relationships.values();

    Ok(quote! {{
        #(#assertions)*

        struct __DistributedProjectionEventSet;

        impl distributed::ProjectionEventSet for __DistributedProjectionEventSet {
            fn projection_event_selectors(
            ) -> Result<
                Vec<distributed::ProjectionEventSelector>,
                distributed::ProjectionProgramError,
            > {
                Ok(vec![#(#selector_tokens),*])
            }
        }

        fn __distributed_projection_program(
        ) -> Result<distributed::ProjectionProgram, distributed::ProjectionProgramError> {
            distributed::projection::lower::projection_program(
                #name,
                #version,
                #partition,
                vec![#(#generated_arms),*],
            )
        }

        fn __distributed_projection_resolve(
            __occurrence: &distributed::DomainEventOccurrence,
        ) -> Result<distributed::ResolvedProjectionPlan, distributed::ProjectionProgramError> {
            distributed::projection::lower::resolve_typed::<__DistributedProjectionEventSet>(
                __distributed_projection_program()?,
                __occurrence,
            )
        }

        fn __distributed_projection_lower(
            __resolved: &distributed::ResolvedProjectionPlan,
        ) -> Result<
            distributed::projection::lower::LoweredProjectionPlan,
            distributed::projection::lower::ProjectionLoweringError,
        > {
            let mut __builder = distributed::ReadModelWritePlanBuilder::new();
            for __mutation in __resolved.mutations() {
                #(#lower_cases)*
                return Err(
                    distributed::projection::lower::ProjectionLoweringError::UnknownOperation {
                        arm: __mutation.provenance().arm_id().to_owned(),
                        operations: __mutation.provenance().operation_ids().to_vec(),
                    },
                );
            }
            distributed::projection::lower::finish_lowering(__builder, __resolved)
        }

        fn __distributed_projection_inventory(
        ) -> Result<
            distributed::projection::lower::ProjectionOutputInventory,
            distributed::projection::lower::ProjectionLoweringError,
        > {
            Ok(distributed::projection::lower::ProjectionOutputInventory::new(
                vec![
                    #(
                        distributed::projection::lower::ProjectionOutputModel::of::<#output_models>()?
                    ),*
                ],
                vec![
                    #(
                        distributed::projection::lower::output_relationship::<#output_relationships>()?
                    ),*
                ],
            ))
        }

        distributed::projection::lower::ProjectionDescriptor::<#eligibility>::__generated(
            #name,
            #version,
            #epoch,
            __distributed_projection_program,
            __distributed_projection_resolve,
            __distributed_projection_lower,
            __distributed_projection_inventory,
        )
    }})
}

fn selector_expansions(selector: &EventSelector) -> Result<SelectorExpansion> {
    match selector {
        EventSelector::State {
            names,
            version,
            binding,
            body,
        } => Ok(SelectorExpansion {
            selectors: names
                .iter()
                .map(|name| {
                    (
                        quote! {
                            distributed::projection::lower::state_selector::<#body>(
                                #name,
                                #version,
                            )?
                        },
                        name.value(),
                    )
                })
                .collect(),
            binding: binding.clone(),
            body: body.clone(),
            state_body: true,
        }),
        EventSelector::Event { event, binding } => Ok(SelectorExpansion {
            selectors: vec![(
                quote! {
                    distributed::projection::lower::event_selector::<#event>()?
                },
                compact_tokens(event),
            )],
            binding: binding.clone(),
            body: event.clone(),
            state_body: false,
        }),
        EventSelector::Deletion {
            names,
            version,
            binding,
            identity,
        } => Ok(SelectorExpansion {
            selectors: names
                .iter()
                .map(|name| {
                    (
                        quote! {
                            distributed::projection::lower::deletion_selector::<#identity>(
                                #name,
                                #version,
                            )?
                        },
                        name.value(),
                    )
                })
                .collect(),
            binding: binding.clone(),
            body: identity.clone(),
            state_body: false,
        }),
    }
}

fn authoring_fields(
    fields: &[FieldAssignment],
    binding: &Ident,
    body: &Type,
    model: &Path,
    assertions: &mut Vec<TokenStream>,
) -> Result<Vec<TokenStream>> {
    fields
        .iter()
        .map(|field| {
            let target = field.name.to_string();
            match &field.value {
                AssignmentValue::Unset => Ok(quote! {
                    distributed::projection::lower::ProjectionAuthoringField::unset(#target)
                }),
                AssignmentValue::Expression(expression) => {
                    let (tokens, body_field) = projection_expression(expression, binding, body)?;
                    if let Some(source) = body_field {
                        assertions.push(quote! {
                            const _: () =
                                distributed::projection::lower::assert_explicit_field_compatible(
                                    <#body as distributed::projection::lower::ProjectionBodyMetadata>::PROJECTION_FIELDS,
                                    #source,
                                    <#model as distributed::projection::lower::ProjectionReadModelMetadata>::PROJECTION_FIELDS,
                                    #target,
                                );
                        });
                    }
                    Ok(quote! {
                        distributed::projection::lower::ProjectionAuthoringField::set(
                            #target,
                            #tokens,
                        )
                    })
                }
            }
        })
        .collect()
}

fn projection_expression(
    expression: &Expr,
    binding: &Ident,
    body: &Type,
) -> Result<(TokenStream, Option<String>)> {
    if let Some(field) = direct_field(expression, binding) {
        return Ok((
            quote! {
                distributed::projection::lower::body_path::<#body>(#field)?
            },
            Some(field),
        ));
    }
    if let Expr::Field(field) = expression {
        if let Some(envelope) = envelope_field(field)? {
            return Ok((
                quote! {
                    distributed::ProjectionExpression::envelope(
                        distributed::ProjectionEnvelopeField::#envelope
                    )
                },
                None,
            ));
        }
    }
    match expression {
        Expr::Lit(ExprLit { lit, .. }) => literal_expression(lit),
        Expr::Unary(unary) => negative_literal_expression(unary),
        Expr::Call(call) => transform_expression(call, binding, body),
        Expr::Path(ExprPath { path, .. }) if path.is_ident("null") => Ok((
            quote! {
                distributed::ProjectionExpression::constant(
                    distributed::ProjectionValue::null()
                )
            },
            None,
        )),
        Expr::Path(ExprPath { path, .. }) if path.segments.len() >= 2 => {
            let variant = path
                .segments
                .last()
                .expect("path length was checked")
                .ident
                .to_string();
            let enum_type = path
                .segments
                .iter()
                .take(path.segments.len() - 1)
                .map(|segment| segment.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            Ok((
                quote! {
                    distributed::ProjectionExpression::enum_variant(
                        #enum_type,
                        #variant,
                    )?
                },
                None,
            ))
        }
        _ => Err(syn::Error::new_spanned(
            expression,
            "projection expressions are closed and deterministic: use a flat body field, stable envelope field, literal, `null`, or enum variant",
        )),
    }
}

fn literal_expression(literal: &Lit) -> Result<(TokenStream, Option<String>)> {
    let value = match literal {
        Lit::Str(value) => quote! {
            distributed::ProjectionValue::string(#value)
        },
        Lit::Bool(value) => quote! {
            distributed::ProjectionValue::boolean(#value)
        },
        Lit::Int(value) => {
            let suffix = value.suffix();
            if suffix == "i64" {
                quote! {
                    distributed::ProjectionValue::signed(#value)
                }
            } else if suffix == "u64" {
                quote! {
                    distributed::ProjectionValue::unsigned(#value)
                }
            } else {
                return Err(syn::Error::new_spanned(
                    value,
                    "projection integer literals require an explicit `i64` or `u64` suffix",
                ));
            }
        }
        Lit::Float(value) if value.suffix() == "f64" => quote! {
            distributed::ProjectionValue::try_float(#value)?
        },
        Lit::Float(value) => {
            return Err(syn::Error::new_spanned(
                value,
                "projection float literals require an explicit `f64` suffix",
            ));
        }
        _ => {
            return Err(syn::Error::new_spanned(
                literal,
                "unsupported projection literal",
            ));
        }
    };
    Ok((
        quote! {
            distributed::ProjectionExpression::constant(#value)
        },
        None,
    ))
}

fn negative_literal_expression(unary: &ExprUnary) -> Result<(TokenStream, Option<String>)> {
    if !matches!(unary.op, UnOp::Neg(_)) {
        return Err(syn::Error::new_spanned(
            unary,
            "projection expressions do not support unary operators",
        ));
    }
    let Expr::Lit(ExprLit { lit, .. }) = unary.expr.as_ref() else {
        return Err(syn::Error::new_spanned(
            unary,
            "projection negation is allowed only on typed numeric literals",
        ));
    };
    let value = match lit {
        Lit::Int(value) if value.suffix() == "i64" => quote! {
            distributed::ProjectionValue::signed(-#value)
        },
        Lit::Float(value) if value.suffix() == "f64" => quote! {
            distributed::ProjectionValue::try_float(-#value)?
        },
        _ => {
            return Err(syn::Error::new_spanned(
                unary,
                "negative projection literals require an `i64` or `f64` suffix",
            ));
        }
    };
    Ok((
        quote! {
            distributed::ProjectionExpression::constant(#value)
        },
        None,
    ))
}

fn transform_expression(
    call: &ExprCall,
    binding: &Ident,
    body: &Type,
) -> Result<(TokenStream, Option<String>)> {
    let Expr::Path(callee) = call.func.as_ref() else {
        return Err(syn::Error::new_spanned(
            &call.func,
            "projection transforms must use a closed transform name",
        ));
    };
    let transform = if callee.path.is_ident("string_concat") {
        quote!(distributed::ProjectionScalarTransform::StringConcat)
    } else if callee.path.is_ident("first_present") {
        quote!(distributed::ProjectionScalarTransform::FirstPresent)
    } else {
        return Err(syn::Error::new_spanned(
            &callee.path,
            "unknown projection transform; supported transforms are `string_concat` and `first_present`",
        ));
    };
    if call.args.is_empty() {
        return Err(syn::Error::new_spanned(
            call,
            "projection transforms require at least one argument",
        ));
    }
    let arguments = call
        .args
        .iter()
        .map(|argument| projection_expression(argument, binding, body).map(|result| result.0))
        .collect::<Result<Vec<_>>>()?;
    Ok((
        quote! {
            distributed::ProjectionExpression::transform(
                #transform,
                vec![#(#arguments),*],
            )?
        },
        None,
    ))
}

fn direct_field(expression: &Expr, binding: &Ident) -> Option<String> {
    let Expr::Field(ExprField { base, member, .. }) = expression else {
        return None;
    };
    let Expr::Path(ExprPath { path, .. }) = base.as_ref() else {
        return None;
    };
    if !path.is_ident(binding)
        && !path.is_ident("body")
        && !path.is_ident("state")
        && !path.is_ident("event")
    {
        return None;
    }
    match member {
        Member::Named(field) => Some(field.to_string()),
        Member::Unnamed(_) => None,
    }
}

fn envelope_field(field: &ExprField) -> Result<Option<Ident>> {
    let Expr::Path(ExprPath { path, .. }) = field.base.as_ref() else {
        return Ok(None);
    };
    if !path.is_ident("envelope") {
        return Ok(None);
    }
    let Member::Named(member) = &field.member else {
        return Err(syn::Error::new_spanned(
            &field.member,
            "projection envelope fields must be named",
        ));
    };
    let variant = match member.to_string().as_str() {
        "occurrence_version" => "OccurrenceVersion",
        "occurrence_id" => "OccurrenceId",
        "event_name" => "EventName",
        "event_version" => "EventVersion",
        "body_fingerprint" => "BodyFingerprint",
        "body_kind" => "BodyKind",
        "body_type_name" => "BodyTypeName",
        "body_version" => "BodyVersion",
        "body_schema" => "BodySchema",
        "body_codec" => "BodyCodec",
        "body_codec_version" => "BodyCodecVersion",
        "aggregate_type" => "AggregateType",
        "aggregate_id" => "AggregateId",
        "aggregate_sequence" => "AggregateSequence",
        "publication_ordinal" => "PublicationOrdinal",
        _ => {
            return Err(syn::Error::new_spanned(
                member,
                "unknown or nondeterministic projection envelope field",
            ));
        }
    };
    Ok(Some(Ident::new(variant, member.span())))
}

fn mutation_kind_tokens(kind: OperationKind) -> TokenStream {
    let variant = match kind {
        OperationKind::Insert => "Insert",
        OperationKind::Upsert | OperationKind::StateUpsert => "Upsert",
        OperationKind::Patch => "Patch",
        OperationKind::UpsertPatch => "UpsertPatch",
        OperationKind::Delete | OperationKind::DeleteRelated => "Delete",
        OperationKind::Recreate => "Recreate",
        OperationKind::InsertRelated => "InsertRelated",
        OperationKind::UpsertRelated => "UpsertRelated",
    };
    let variant = Ident::new(variant, Span::call_site());
    quote!(distributed::ProjectionMutationKind::#variant)
}

fn relationship_marker_path(model: &Path, relationship: &Ident) -> Result<Path> {
    let mut path = model.clone();
    let model_name = path
        .segments
        .pop()
        .ok_or_else(|| syn::Error::new_spanned(model, "relationship source model path is empty"))?
        .into_value()
        .ident;
    let marker = format_ident!(
        "__Distributed{}EffectRelationship_{}",
        model_name,
        relationship
    );
    path.segments.push(syn::PathSegment::from(marker));
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_state_upsert_and_delete() {
        let declaration: ProjectionDeclaration = syn::parse_quote! {
            name: "todos";
            version: 1;
            epoch: "todos-v1";
            partition: unit;
            on ["todo.created", "todo.completed"] version 1 (state: TodoState) {
                upsert Todos from state as todo;
            }
            on TodoPurged(event) {
                delete Todos { key { todo_id: event.todo_id } };
            }
        };
        let expanded = expand_declaration(declaration).expect("expand projection");
        let rendered = expanded.to_string();
        assert!(rendered.contains("state_upsert_operation"));
        assert!(rendered.contains("lower_model_mutation"));
        assert!(rendered.contains("EventualOnly"));
    }

    #[test]
    fn rejects_arbitrary_rust_expression() {
        let declaration: ProjectionDeclaration = syn::parse_quote! {
            name: "todos";
            version: 1;
            epoch: "todos-v1";
            partition: unit;
            on TodoChanged(event) {
                patch Todos {
                    key { todo_id: event.todo_id },
                    set { title: make_title(event.todo_id) }
                };
            }
        };
        let error = expand_declaration(declaration).unwrap_err();
        assert!(error.to_string().contains("unknown projection transform"));
    }
}
