//! `mutation!` macro: event-independent read-model mutation programs.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{braced, Ident, LitInt, LitStr, Path, Result, Token, Type};

pub(crate) fn expand(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as MutationDeclaration);
    expand_declaration(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

struct MutationDeclaration {
    name: Option<LitStr>,
    version: Option<LitInt>,
    input_ty: Option<Type>,
    operations: Vec<MutationOpSyntax>,
}

enum MutationOpSyntax {
    /// `upsert Model from input.root;`
    SugarUpsert { model: Path, input_root: Vec<Ident> },
    /// `upsert Model one { object: input.root, conflict: primary_key, update: all_input_fields };`
    ExplicitUpsert {
        model: Path,
        object_root: Vec<Ident>,
        #[allow(dead_code)]
        conflict_primary_key: bool,
        #[allow(dead_code)]
        update_all: bool,
    },
    /// `delete Model by_pk { field: input.path, ... };`
    DeleteByPk {
        model: Path,
        keys: Vec<(Ident, Vec<Ident>)>,
    },
    /// `update Model by_pk { key..., _set: { field: expr, ... } };`
    UpdateByPk {
        model: Path,
        keys: Vec<(Ident, Vec<Ident>)>,
        sets: Vec<(Ident, Vec<Ident>)>,
    },
    /// `insert Model one { object: input.root };`
    InsertOne {
        model: Path,
        object_root: Vec<Ident>,
    },
}

impl Parse for MutationDeclaration {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut name = None;
        let mut version = None;
        let mut input_ty = None;
        let mut operations = Vec::new();

        while !input.is_empty() {
            if input.peek(Ident) {
                let keyword: Ident = input.fork().parse()?;
                match keyword.to_string().as_str() {
                    "name" => {
                        let _: Ident = input.parse()?;
                        input.parse::<Token![:]>()?;
                        name = Some(input.parse()?);
                        parse_separator(input)?;
                        continue;
                    }
                    "version" => {
                        let _: Ident = input.parse()?;
                        input.parse::<Token![:]>()?;
                        version = Some(input.parse()?);
                        parse_separator(input)?;
                        continue;
                    }
                    "input" => {
                        let _: Ident = input.parse()?;
                        input.parse::<Token![:]>()?;
                        input_ty = Some(input.parse()?);
                        parse_separator(input)?;
                        continue;
                    }
                    "upsert" | "delete" | "update" | "insert" | "patch" => {
                        operations.push(parse_operation(input)?);
                        continue;
                    }
                    _ => {}
                }
            }
            return Err(input.error(
                "expected mutation operation (`upsert`, `insert`, `update`, `delete`) \
                 or `name`/`version`/`input` metadata",
            ));
        }

        if operations.is_empty() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "mutation! requires at least one operation",
            ));
        }

        Ok(Self {
            name,
            version,
            input_ty,
            operations,
        })
    }
}

fn parse_separator(input: ParseStream<'_>) -> Result<()> {
    if input.peek(Token![;]) {
        input.parse::<Token![;]>()?;
    } else if input.peek(Token![,]) {
        input.parse::<Token![,]>()?;
    }
    Ok(())
}

fn parse_operation(input: ParseStream<'_>) -> Result<MutationOpSyntax> {
    let keyword: Ident = input.parse()?;
    match keyword.to_string().as_str() {
        "upsert" => parse_upsert(input),
        "insert" => parse_insert(input),
        "delete" => parse_delete(input),
        "update" | "patch" => parse_update(input),
        other => Err(syn::Error::new(
            keyword.span(),
            format!("unsupported mutation operation `{other}`"),
        )),
    }
}

fn parse_upsert(input: ParseStream<'_>) -> Result<MutationOpSyntax> {
    let model: Path = input.parse()?;
    if input.peek(Ident) {
        let mode: Ident = input.fork().parse()?;
        if mode == "from" {
            let _: Ident = input.parse()?;
            let input_root = parse_input_path(input)?;
            parse_separator(input)?;
            return Ok(MutationOpSyntax::SugarUpsert { model, input_root });
        }
    }
    let mode: Ident = input.parse()?;
    if mode != "one" {
        return Err(syn::Error::new(
            mode.span(),
            "expected `from` sugar or `one { ... }` explicit upsert",
        ));
    }
    let content;
    braced!(content in input);
    let mut object_root = None;
    let mut conflict_primary_key = false;
    let mut update_all = false;
    while !content.is_empty() {
        let field: Ident = content.parse()?;
        content.parse::<Token![:]>()?;
        match field.to_string().as_str() {
            "object" => {
                object_root = Some(parse_input_path(&content)?);
            }
            "conflict" => {
                let target: Ident = content.parse()?;
                if target != "primary_key" {
                    return Err(syn::Error::new(
                        target.span(),
                        "only `conflict: primary_key` is supported in v1",
                    ));
                }
                conflict_primary_key = true;
            }
            "update" => {
                let mode: Ident = content.parse()?;
                if mode != "all_input_fields" {
                    return Err(syn::Error::new(
                        mode.span(),
                        "only `update: all_input_fields` is supported in v1 sugar",
                    ));
                }
                update_all = true;
            }
            other => {
                return Err(syn::Error::new(
                    field.span(),
                    format!("unknown upsert field `{other}`"),
                ));
            }
        }
        parse_separator(&content)?;
    }
    let object_root = object_root.ok_or_else(|| {
        syn::Error::new(mode.span(), "explicit upsert requires `object: input...`")
    })?;
    parse_separator(input)?;
    Ok(MutationOpSyntax::ExplicitUpsert {
        model,
        object_root,
        conflict_primary_key,
        update_all,
    })
}

fn parse_insert(input: ParseStream<'_>) -> Result<MutationOpSyntax> {
    let model: Path = input.parse()?;
    let mode: Ident = input.parse()?;
    if mode != "one" {
        return Err(syn::Error::new(
            mode.span(),
            "expected `insert Model one { ... }`",
        ));
    }
    let content;
    braced!(content in input);
    let mut object_root = None;
    while !content.is_empty() {
        let field: Ident = content.parse()?;
        content.parse::<Token![:]>()?;
        if field == "object" {
            object_root = Some(parse_input_path(&content)?);
        } else {
            return Err(syn::Error::new(field.span(), "expected `object`"));
        }
        parse_separator(&content)?;
    }
    let object_root = object_root
        .ok_or_else(|| syn::Error::new(mode.span(), "insert requires `object: input...`"))?;
    parse_separator(input)?;
    Ok(MutationOpSyntax::InsertOne { model, object_root })
}

fn parse_delete(input: ParseStream<'_>) -> Result<MutationOpSyntax> {
    let model: Path = input.parse()?;
    let mode: Ident = input.parse()?;
    if mode != "by_pk" {
        return Err(syn::Error::new(
            mode.span(),
            "expected `delete Model by_pk { ... }`",
        ));
    }
    let content;
    braced!(content in input);
    let mut keys = Vec::new();
    while !content.is_empty() {
        let name: Ident = content.parse()?;
        content.parse::<Token![:]>()?;
        let path = parse_input_path(&content)?;
        keys.push((name, path));
        parse_separator(&content)?;
    }
    if keys.is_empty() {
        return Err(syn::Error::new(
            mode.span(),
            "delete by_pk requires key fields",
        ));
    }
    parse_separator(input)?;
    Ok(MutationOpSyntax::DeleteByPk { model, keys })
}

fn parse_update(input: ParseStream<'_>) -> Result<MutationOpSyntax> {
    let model: Path = input.parse()?;
    let mode: Ident = input.parse()?;
    if mode != "by_pk" {
        return Err(syn::Error::new(
            mode.span(),
            "expected `update Model by_pk { ... }`",
        ));
    }
    let content;
    braced!(content in input);
    let mut keys = Vec::new();
    let mut sets = Vec::new();
    while !content.is_empty() {
        let name: Ident = content.parse()?;
        content.parse::<Token![:]>()?;
        if name == "_set" {
            let set_content;
            braced!(set_content in content);
            while !set_content.is_empty() {
                let field: Ident = set_content.parse()?;
                set_content.parse::<Token![:]>()?;
                let path = parse_input_path(&set_content)?;
                sets.push((field, path));
                parse_separator(&set_content)?;
            }
        } else {
            let path = parse_input_path(&content)?;
            keys.push((name, path));
        }
        parse_separator(&content)?;
    }
    if keys.is_empty() || sets.is_empty() {
        return Err(syn::Error::new(
            mode.span(),
            "update by_pk requires key fields and `_set { ... }`",
        ));
    }
    parse_separator(input)?;
    Ok(MutationOpSyntax::UpdateByPk { model, keys, sets })
}

fn parse_input_path(input: ParseStream<'_>) -> Result<Vec<Ident>> {
    // Accept `input.foo.bar` or bare `foo.bar` paths.
    let first: Ident = input.parse()?;
    let mut path = vec![first];
    while input.peek(Token![.]) {
        input.parse::<Token![.]>()?;
        path.push(input.parse()?);
    }
    // Drop leading `input` segment; mutation IR paths are relative to the input object.
    if path.first().is_some_and(|segment| segment == "input") {
        path.remove(0);
    }
    if path.is_empty() {
        return Err(input.error("input path must select at least one field"));
    }
    Ok(path)
}

fn expand_declaration(decl: MutationDeclaration) -> Result<TokenStream> {
    let name = decl
        .name
        .map(|lit| lit.value())
        .unwrap_or_else(|| "mutation".to_owned());
    let version = decl
        .version
        .map(|lit| lit.base10_parse::<u64>())
        .transpose()?
        .unwrap_or(1);
    let input_ty = decl.input_ty.unwrap_or_else(|| syn::parse_quote!(()));

    let mut op_tokens = Vec::new();
    for (index, operation) in decl.operations.iter().enumerate() {
        op_tokens.push(expand_operation(index as u32, operation)?);
    }

    let name_lit = LitStr::new(&name, proc_macro2::Span::call_site());
    Ok(quote! {
        {
            let __operations = vec![
                #(#op_tokens),*
            ];
            let __program = ::distributed::MutationProgram::try_new(
                #name_lit,
                #version,
                __operations,
            ).unwrap_or_else(|error| ::std::panic!("invalid mutation program: {error}"));
            ::distributed::Mutation::<#input_ty>::from_program(__program)
        }
    })
}

fn expand_operation(index: u32, operation: &MutationOpSyntax) -> Result<TokenStream> {
    match operation {
        MutationOpSyntax::SugarUpsert { model, input_root }
        | MutationOpSyntax::ExplicitUpsert {
            model,
            object_root: input_root,
            ..
        } => {
            // Expand to runtime helper that uses ReadModel schema for key/fields.
            let root_lits = input_root
                .iter()
                .map(|ident| LitStr::new(&ident.to_string(), ident.span()))
                .collect::<Vec<_>>();
            let op_id = format!("upsert-{index}");
            let op_id_lit = LitStr::new(&op_id, proc_macro2::Span::call_site());
            Ok(quote! {
                {
                    use ::distributed::{
                        MutationAssignment, MutationConflictTarget, MutationField, MutationKeyField,
                        MutationKind, MutationOperation, MutationExpression, ProjectionTarget,
                        ProjectionValueType, RelationalReadModel,
                    };
                    let __schema = <#model as RelationalReadModel>::schema();
                    let __target = ProjectionTarget::try_new(
                        __schema.model_name.clone(),
                        __schema.table_name.clone(),
                    ).expect("model target");
                    let __root: &[&str] = &[#(#root_lits),*];
                    let mut __key = ::std::vec::Vec::new();
                    for (ordinal, column) in __schema.primary_key.columns.iter().enumerate() {
                        let mut path = __root.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
                        path.push(column.clone());
                        __key.push(MutationKeyField::try_new(
                            ordinal as u32,
                            column.clone(),
                            MutationExpression::input_path(ProjectionValueType::String, path)
                                .expect("key path"),
                        ).expect("key field"));
                    }
                    let mut __fields = ::std::vec::Vec::new();
                    let mut __ordinal = 0u32;
                    for column in __schema.columns.iter().filter(|column| !column.skipped) {
                        let mut path = __root.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
                        path.push(column.field_name.clone());
                        __fields.push(MutationField::try_new(
                            __ordinal,
                            column.field_name.clone(),
                            MutationAssignment::set(
                                MutationExpression::input_path(
                                    ProjectionValueType::String,
                                    path,
                                ).expect("field path"),
                            ),
                        ).expect("field"));
                        __ordinal += 1;
                    }
                    MutationOperation::try_new(
                        #op_id_lit,
                        #index,
                        MutationKind::Upsert,
                        __target,
                        __key,
                        __fields,
                        Some(MutationConflictTarget::PrimaryKey),
                        ::std::vec::Vec::new(),
                        ::std::vec::Vec::new(),
                        None,
                    ).expect("upsert operation")
                }
            })
        }
        MutationOpSyntax::InsertOne { model, object_root } => {
            let root_lits = object_root
                .iter()
                .map(|ident| LitStr::new(&ident.to_string(), ident.span()))
                .collect::<Vec<_>>();
            let op_id = format!("insert-{index}");
            let op_id_lit = LitStr::new(&op_id, proc_macro2::Span::call_site());
            Ok(quote! {
                {
                    use ::distributed::{
                        MutationAssignment, MutationField, MutationKeyField, MutationKind,
                        MutationOperation, MutationExpression, ProjectionTarget,
                        ProjectionValueType, RelationalReadModel,
                    };
                    let __schema = <#model as RelationalReadModel>::schema();
                    let __target = ProjectionTarget::try_new(
                        __schema.model_name.clone(),
                        __schema.table_name.clone(),
                    ).expect("model target");
                    let __root: &[&str] = &[#(#root_lits),*];
                    let mut __key = ::std::vec::Vec::new();
                    for (ordinal, column) in __schema.primary_key.columns.iter().enumerate() {
                        let mut path = __root.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
                        path.push(column.clone());
                        __key.push(MutationKeyField::try_new(
                            ordinal as u32,
                            column.clone(),
                            MutationExpression::input_path(ProjectionValueType::String, path)
                                .expect("key path"),
                        ).expect("key field"));
                    }
                    let mut __fields = ::std::vec::Vec::new();
                    let mut __ordinal = 0u32;
                    for column in __schema.columns.iter().filter(|column| !column.skipped) {
                        let mut path = __root.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
                        path.push(column.field_name.clone());
                        __fields.push(MutationField::try_new(
                            __ordinal,
                            column.field_name.clone(),
                            MutationAssignment::set(
                                MutationExpression::input_path(
                                    ProjectionValueType::String,
                                    path,
                                ).expect("field path"),
                            ),
                        ).expect("field"));
                        __ordinal += 1;
                    }
                    MutationOperation::try_new(
                        #op_id_lit,
                        #index,
                        MutationKind::Insert,
                        __target,
                        __key,
                        __fields,
                        None,
                        ::std::vec::Vec::new(),
                        ::std::vec::Vec::new(),
                        None,
                    ).expect("insert operation")
                }
            })
        }
        MutationOpSyntax::DeleteByPk { model, keys } => {
            let op_id = format!("delete-{index}");
            let op_id_lit = LitStr::new(&op_id, proc_macro2::Span::call_site());
            let key_tokens = keys.iter().enumerate().map(|(ordinal, (name, path))| {
                let name_lit = LitStr::new(&name.to_string(), name.span());
                let path_lits = path
                    .iter()
                    .map(|ident| LitStr::new(&ident.to_string(), ident.span()))
                    .collect::<Vec<_>>();
                let ordinal = ordinal as u32;
                quote! {
                    MutationKeyField::try_new(
                        #ordinal,
                        #name_lit,
                        MutationExpression::input_path(
                            ProjectionValueType::String,
                            vec![#(#path_lits.to_owned()),*],
                        ).expect("key path"),
                    ).expect("key field")
                }
            });
            Ok(quote! {
                {
                    use ::distributed::{
                        MutationKeyField, MutationKind, MutationOperation, MutationExpression,
                        ProjectionTarget, ProjectionValueType, RelationalReadModel,
                    };
                    let __schema = <#model as RelationalReadModel>::schema();
                    let __target = ProjectionTarget::try_new(
                        __schema.model_name.clone(),
                        __schema.table_name.clone(),
                    ).expect("model target");
                    MutationOperation::try_new(
                        #op_id_lit,
                        #index,
                        MutationKind::Delete,
                        __target,
                        vec![#(#key_tokens),*],
                        ::std::vec::Vec::new(),
                        None,
                        ::std::vec::Vec::new(),
                        ::std::vec::Vec::new(),
                        None,
                    ).expect("delete operation")
                }
            })
        }
        MutationOpSyntax::UpdateByPk { model, keys, sets } => {
            let op_id = format!("patch-{index}");
            let op_id_lit = LitStr::new(&op_id, proc_macro2::Span::call_site());
            let key_tokens = keys.iter().enumerate().map(|(ordinal, (name, path))| {
                let name_lit = LitStr::new(&name.to_string(), name.span());
                let path_lits = path
                    .iter()
                    .map(|ident| LitStr::new(&ident.to_string(), ident.span()))
                    .collect::<Vec<_>>();
                let ordinal = ordinal as u32;
                quote! {
                    MutationKeyField::try_new(
                        #ordinal,
                        #name_lit,
                        MutationExpression::input_path(
                            ProjectionValueType::String,
                            vec![#(#path_lits.to_owned()),*],
                        ).expect("key path"),
                    ).expect("key field")
                }
            });
            let field_tokens = sets.iter().enumerate().map(|(ordinal, (name, path))| {
                let name_lit = LitStr::new(&name.to_string(), name.span());
                let path_lits = path
                    .iter()
                    .map(|ident| LitStr::new(&ident.to_string(), ident.span()))
                    .collect::<Vec<_>>();
                let ordinal = ordinal as u32;
                quote! {
                    MutationField::try_new(
                        #ordinal,
                        #name_lit,
                        MutationAssignment::set(
                            MutationExpression::input_path(
                                ProjectionValueType::String,
                                vec![#(#path_lits.to_owned()),*],
                            ).expect("field path"),
                        ),
                    ).expect("field")
                }
            });
            Ok(quote! {
                {
                    use ::distributed::{
                        MutationAssignment, MutationField, MutationKeyField, MutationKind,
                        MutationOperation, MutationExpression, ProjectionTarget,
                        ProjectionValueType, RelationalReadModel,
                    };
                    let __schema = <#model as RelationalReadModel>::schema();
                    let __target = ProjectionTarget::try_new(
                        __schema.model_name.clone(),
                        __schema.table_name.clone(),
                    ).expect("model target");
                    MutationOperation::try_new(
                        #op_id_lit,
                        #index,
                        MutationKind::Patch,
                        __target,
                        vec![#(#key_tokens),*],
                        vec![#(#field_tokens),*],
                        None,
                        ::std::vec::Vec::new(),
                        ::std::vec::Vec::new(),
                        None,
                    ).expect("patch operation")
                }
            })
        }
    }
}
