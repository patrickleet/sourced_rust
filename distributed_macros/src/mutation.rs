//! `mutation!` macro: event-independent read-model mutation programs.
//!
//! Supports classic sugar and GraphQL-looking syntax-only documents (not public
//! GraphQL schema fields):
//!
//! ```ignore
//! mutation! {
//!     mutation SaveTodo {
//!         upsert_todos(object: $input.todo)
//!     }
//! }
//! ```
//!
//! And file loading via [`mutation_file`]:
//!
//! ```ignore
//! mutation_file!("src/mutations/save_todo.mutation.graphql")
//! ```

use std::path::PathBuf;

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{braced, parenthesized, Ident, LitInt, LitStr, Path, Result, Token, Type};

pub(crate) fn expand(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = syn::parse_macro_input!(input as MutationDeclaration);
    expand_declaration(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Load a `.mutation.graphql` document relative to `CARGO_MANIFEST_DIR`.
pub(crate) fn expand_file(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let path_lit = syn::parse_macro_input!(input as LitStr);
    match load_graphql_file(&path_lit) {
        Ok(decl) => expand_declaration(decl)
            .unwrap_or_else(syn::Error::into_compile_error)
            .into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn load_graphql_file(path_lit: &LitStr) -> Result<MutationDeclaration> {
    let relative = path_lit.value();
    let manifest = std::env::var("CARGO_MANIFEST_DIR").map_err(|_| {
        syn::Error::new(
            path_lit.span(),
            "CARGO_MANIFEST_DIR is required to load mutation.graphql files",
        )
    })?;
    let full = PathBuf::from(manifest).join(&relative);
    let source = std::fs::read_to_string(&full).map_err(|error| {
        syn::Error::new(
            path_lit.span(),
            format!(
                "failed to read mutation file `{}` (resolved `{}`): {error}",
                relative,
                full.display()
            ),
        )
    })?;
    parse_graphql_document(&source, path_lit.span())
}

struct MutationDeclaration {
    name: Option<LitStr>,
    version: Option<LitInt>,
    input_ty: Option<Type>,
    operations: Vec<MutationOpSyntax>,
}

type GraphqlFieldBinding = (Ident, Vec<Ident>);

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
        keys: Vec<GraphqlFieldBinding>,
    },
    /// `update Model by_pk { key..., _set: { field: expr, ... } };`
    UpdateByPk {
        model: Path,
        keys: Vec<GraphqlFieldBinding>,
        sets: Vec<GraphqlFieldBinding>,
    },
    /// `insert Model one { object: input.root };`
    InsertOne {
        model: Path,
        object_root: Vec<Ident>,
    },
}

impl Parse for MutationDeclaration {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        // GraphQL-looking form: `mutation Name { upsert_todos(...) }`
        if input.peek(Ident) {
            let keyword: Ident = input.fork().parse()?;
            if keyword == "mutation" {
                return parse_graphql_token_document(input);
            }
        }

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
                "expected GraphQL-looking `mutation Name { … }`, classic mutation \
                 operations (`upsert`, `insert`, `update`, `delete`), or \
                 `name`/`version`/`input` metadata",
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

/// Parse GraphQL-looking tokens inside `mutation! { mutation Name { ops } }`.
fn parse_graphql_token_document(input: ParseStream<'_>) -> Result<MutationDeclaration> {
    let mutation_kw: Ident = input.parse()?;
    if mutation_kw != "mutation" {
        return Err(syn::Error::new(mutation_kw.span(), "expected `mutation`"));
    }
    let name_ident: Ident = input.parse()?;
    let mut version: Option<u64> = None;
    if input.peek(Token![@]) {
        input.parse::<Token![@]>()?;
        let attr: Ident = input.parse()?;
        if attr != "version" {
            return Err(syn::Error::new(
                attr.span(),
                "only `@version(n)` is supported on GraphQL-looking mutations",
            ));
        }
        let version_content;
        parenthesized!(version_content in input);
        let lit: LitInt = version_content.parse()?;
        version = Some(lit.base10_parse()?);
    }
    let body;
    braced!(body in input);
    let mut operations = Vec::new();
    while !body.is_empty() {
        operations.push(parse_graphql_field_operation(&body)?);
        // Optional trailing commas between field ops.
        if body.peek(Token![,]) {
            body.parse::<Token![,]>()?;
        }
    }
    if operations.is_empty() {
        return Err(syn::Error::new(
            name_ident.span(),
            "GraphQL-looking mutation body requires at least one operation",
        ));
    }
    // Keep the GraphQL operation name as the program identity so projection
    // bindings (`mutation: SaveTodo`) match `mutation SaveTodo { … }`.
    let name = LitStr::new(&name_ident.to_string(), name_ident.span());
    let version_lit = version.map(|v| LitInt::new(&v.to_string(), name_ident.span()));
    Ok(MutationDeclaration {
        name: Some(name),
        version: version_lit,
        input_ty: None,
        operations,
    })
}

/// Parse a raw `.mutation.graphql` text document.
fn parse_graphql_document(source: &str, span: proc_macro2::Span) -> Result<MutationDeclaration> {
    // Strip line comments (`# …`).
    let mut cleaned = String::new();
    for line in source.lines() {
        let trimmed = line.split('#').next().unwrap_or("").trim_end();
        if !trimmed.is_empty() {
            cleaned.push_str(trimmed);
            cleaned.push('\n');
        }
    }
    let tokens: TokenStream = cleaned.parse().map_err(|error| {
        syn::Error::new(
            span,
            format!("invalid GraphQL-looking mutation tokens: {error}"),
        )
    })?;
    syn::parse2::<MutationDeclaration>(tokens).map_err(|error| {
        syn::Error::new(
            span,
            format!("failed to parse mutation.graphql document: {error}"),
        )
    })
}

/// `upsert_todos(object: $input.todo)` / `delete_todos_by_pk(todo_id: $input.todo_id)`.
/// The suffix is the read-model table name (snake_case), same as the query field.
fn parse_graphql_field_operation(input: ParseStream<'_>) -> Result<MutationOpSyntax> {
    let field: Ident = input.parse()?;
    let field_name = field.to_string();
    let args;
    parenthesized!(args in input);

    if let Some(model) = field_name.strip_prefix("upsert_") {
        let model = model_path_from_name(model, field.span())?;
        let object_root = parse_graphql_object_arg(&args)?;
        return Ok(MutationOpSyntax::SugarUpsert {
            model,
            input_root: object_root,
        });
    }
    if let Some(model) = field_name
        .strip_prefix("insert_")
        .map(|rest| rest.strip_suffix("_one").unwrap_or(rest))
    {
        let model = model_path_from_name(model, field.span())?;
        let object_root = parse_graphql_object_arg(&args)?;
        return Ok(MutationOpSyntax::InsertOne { model, object_root });
    }
    if let Some(model) = field_name
        .strip_prefix("delete_")
        .and_then(|rest| rest.strip_suffix("_by_pk"))
    {
        let model = model_path_from_name(model, field.span())?;
        let keys = parse_graphql_key_args(&args)?;
        return Ok(MutationOpSyntax::DeleteByPk { model, keys });
    }
    if let Some(model) = field_name
        .strip_prefix("update_")
        .and_then(|rest| rest.strip_suffix("_by_pk"))
        .or_else(|| {
            field_name
                .strip_prefix("patch_")
                .and_then(|rest| rest.strip_suffix("_by_pk"))
        })
    {
        let model = model_path_from_name(model, field.span())?;
        let (keys, sets) = parse_graphql_update_args(&args)?;
        return Ok(MutationOpSyntax::UpdateByPk { model, keys, sets });
    }

    Err(syn::Error::new(
        field.span(),
        format!(
            "unsupported GraphQL-looking mutation field `{field_name}`; \
             expected upsert_todos, insert_todos[_one], delete_todos_by_pk, \
             or update_todos_by_pk (snake_case table name, same as the query field)"
        ),
    ))
}

fn model_path_from_name(name: &str, span: proc_macro2::Span) -> Result<Path> {
    if name.is_empty() {
        return Err(syn::Error::new(
            span,
            "model name missing from mutation field",
        ));
    }
    let pascal = snake_to_pascal(name);
    syn::parse_str::<Ident>(&pascal)
        .map(Path::from)
        .map_err(|_| {
            syn::Error::new(
                span,
                format!("`{name}` is not a valid read-model name (expected snake_case table name)"),
            )
        })
}

/// `chat_messages` → `ChatMessages`. A PascalCase suffix stays PascalCase.
fn snake_to_pascal(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn parse_graphql_object_arg(input: ParseStream<'_>) -> Result<Vec<Ident>> {
    let mut object_root = None;
    while !input.is_empty() {
        let key: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        if key != "object" {
            return Err(syn::Error::new(
                key.span(),
                "expected `object: $input…` argument",
            ));
        }
        object_root = Some(parse_graphql_input_path(input)?);
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
    }
    object_root.ok_or_else(|| input.error("upsert/insert requires `object: $input…`"))
}

fn parse_graphql_key_args(input: ParseStream<'_>) -> Result<Vec<GraphqlFieldBinding>> {
    let mut keys = Vec::new();
    while !input.is_empty() {
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let path = parse_graphql_input_path(input)?;
        keys.push((name, path));
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
    }
    if keys.is_empty() {
        return Err(input.error("delete_by_pk requires at least one key argument"));
    }
    Ok(keys)
}

fn parse_graphql_update_args(
    input: ParseStream<'_>,
) -> Result<(Vec<GraphqlFieldBinding>, Vec<GraphqlFieldBinding>)> {
    let mut keys = Vec::new();
    let mut sets = Vec::new();
    while !input.is_empty() {
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        if name == "_set" {
            let set_content;
            braced!(set_content in input);
            while !set_content.is_empty() {
                let field: Ident = set_content.parse()?;
                set_content.parse::<Token![:]>()?;
                let path = parse_graphql_input_path(&set_content)?;
                sets.push((field, path));
                if set_content.peek(Token![,]) {
                    set_content.parse::<Token![,]>()?;
                }
            }
        } else {
            let path = parse_graphql_input_path(input)?;
            keys.push((name, path));
        }
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
    }
    if keys.is_empty() || sets.is_empty() {
        return Err(input.error("update_by_pk requires key fields and `_set: { field: $input… }`"));
    }
    Ok((keys, sets))
}

/// `$input.todo`, `input.todo`, or `$todo_id` / bare paths.
fn parse_graphql_input_path(input: ParseStream<'_>) -> Result<Vec<Ident>> {
    if input.peek(Token![$]) {
        input.parse::<Token![$]>()?;
    }
    parse_input_path(input)
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

#[cfg(test)]
mod tests {
    use super::snake_to_pascal;

    #[test]
    fn snake_table_names_become_rust_type_names() {
        assert_eq!(snake_to_pascal("todos"), "Todos");
        assert_eq!(snake_to_pascal("chat_messages"), "ChatMessages");
        assert_eq!(snake_to_pascal("blob_games"), "BlobGames");
        assert_eq!(snake_to_pascal("auth_users"), "AuthUsers");
    }

    #[test]
    fn pascal_suffixes_stay_pascal() {
        assert_eq!(snake_to_pascal("Todos"), "Todos");
        assert_eq!(snake_to_pascal("ChatMessages"), "ChatMessages");
    }
}
