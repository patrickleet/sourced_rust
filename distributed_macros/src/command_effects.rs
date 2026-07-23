use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{braced, parenthesized, Expr, Ident, Lit, Path, Result, Token};

mod keyword {
    syn::custom_keyword!(input);
    syn::custom_keyword!(upsert);
    syn::custom_keyword!(patch);
    syn::custom_keyword!(delete);
    syn::custom_keyword!(link);
    syn::custom_keyword!(unlink);
    syn::custom_keyword!(invalidate);
    syn::custom_keyword!(key);
    syn::custom_keyword!(set);
    syn::custom_keyword!(source);
    syn::custom_keyword!(target);
    syn::custom_keyword!(confirm);
    syn::custom_keyword!(partition);
    syn::custom_keyword!(default);
}

pub fn expand(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    match syn::parse::<CommandEffects>(input) {
        Ok(effects) => effects.expand().into(),
        Err(error) => error.to_compile_error().into(),
    }
}

pub fn expand_confirmations(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    match syn::parse::<CommandConfirmations>(input) {
        Ok(confirmations) => confirmations.expand().into(),
        Err(error) => error.to_compile_error().into(),
    }
}

pub fn expand_input_defaults(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    match syn::parse::<CommandInputDefaults>(input) {
        Ok(defaults) => defaults.expand().into(),
        Err(error) => error.to_compile_error().into(),
    }
}

struct CommandInputDefaults {
    input: Path,
    defaults: Vec<InputDefault>,
}

impl Parse for CommandInputDefaults {
    fn parse(stream: ParseStream<'_>) -> Result<Self> {
        stream.parse::<keyword::input>()?;
        stream.parse::<Token![:]>()?;
        let input = stream.parse()?;
        stream.parse::<Token![;]>()?;

        let mut defaults = Vec::new();
        let mut fields = std::collections::BTreeSet::new();
        while !stream.is_empty() {
            stream.parse::<keyword::default>()?;
            let default = InputDefault::parse(stream)?;
            if !fields.insert(default.field.to_string()) {
                return Err(syn::Error::new(
                    default.field.span(),
                    format!("duplicate generated input default `{}`", default.field),
                ));
            }
            defaults.push(default);
            stream.parse::<Token![;]>()?;
        }
        if defaults.is_empty() {
            return Err(stream.error("command_input_defaults! requires at least one default"));
        }
        Ok(Self { input, defaults })
    }
}

impl CommandInputDefaults {
    fn expand(self) -> TokenStream {
        let input = self.input;
        let defaults = self
            .defaults
            .into_iter()
            .map(|default| default.expand(&input));
        quote! {
            distributed::graphql::__command_input_defaults::<#input>(
                vec![#(#defaults),*]
            )
        }
    }
}

enum InputDefaultGenerator {
    UuidV7,
    Ulid,
}

struct InputDefault {
    field: Ident,
    generator: InputDefaultGenerator,
}

impl InputDefault {
    fn parse(stream: ParseStream<'_>) -> Result<Self> {
        let input_ident: Ident = stream.parse()?;
        if input_ident != "input" {
            return Err(syn::Error::new(
                input_ident.span(),
                "generated defaults must target `input.field`",
            ));
        }
        stream.parse::<Token![.]>()?;
        let field = stream.parse()?;
        stream.parse::<Token![=]>()?;
        let generator: Ident = stream.parse()?;
        let arguments;
        parenthesized!(arguments in stream);
        if !arguments.is_empty() {
            return Err(arguments.error("input-default generators take no arguments"));
        }
        let generator = match generator.to_string().as_str() {
            "uuid_v7" => InputDefaultGenerator::UuidV7,
            "ulid" => InputDefaultGenerator::Ulid,
            _ => {
                return Err(syn::Error::new(
                    generator.span(),
                    "unknown input-default generator; expected uuid_v7() or ulid()",
                ));
            }
        };
        Ok(Self { field, generator })
    }

    fn expand(self, input: &Path) -> TokenStream {
        let marker_name = format_ident!(
            "__Distributed{}EffectInputField_{}",
            input.segments.last().unwrap().ident,
            self.field,
            span = self.field.span()
        );
        let marker = marker_path(input, marker_name);
        match self.generator {
            InputDefaultGenerator::UuidV7 => quote! {
                distributed::graphql::__input_default_uuid_v7::<#input, #marker>()
            },
            InputDefaultGenerator::Ulid => quote! {
                distributed::graphql::__input_default_ulid::<#input, #marker>()
            },
        }
    }
}

struct CommandEffects {
    input: Path,
    operations: Vec<Operation>,
}

impl Parse for CommandEffects {
    fn parse(stream: ParseStream<'_>) -> Result<Self> {
        stream.parse::<keyword::input>()?;
        stream.parse::<Token![:]>()?;
        let input = stream.parse()?;
        stream.parse::<Token![;]>()?;

        let mut operations = Vec::new();
        while !stream.is_empty() {
            operations.push(Operation::parse(stream)?);
            stream.parse::<Token![;]>()?;
        }
        if operations.is_empty() {
            return Err(stream.error(
                "command_effects! requires at least one operation; omit .effects(...) to use explicit revalidation",
            ));
        }
        Ok(Self { input, operations })
    }
}

impl CommandEffects {
    fn expand(self) -> TokenStream {
        let input = self.input;
        let operations = self
            .operations
            .into_iter()
            .map(|operation| operation.expand(&input));
        quote! {
            distributed::graphql::__command_effects::<#input>(vec![#(#operations),*])
        }
    }
}

struct CommandConfirmations {
    input: Path,
    confirmations: Vec<ProjectionConfirmation>,
}

impl Parse for CommandConfirmations {
    fn parse(stream: ParseStream<'_>) -> Result<Self> {
        stream.parse::<keyword::input>()?;
        stream.parse::<Token![:]>()?;
        let input = stream.parse()?;
        stream.parse::<Token![;]>()?;

        let mut confirmations = Vec::new();
        while !stream.is_empty() {
            stream.parse::<keyword::confirm>()?;
            confirmations.push(ProjectionConfirmation::parse(stream)?);
            stream.parse::<Token![;]>()?;
        }
        if confirmations.is_empty() {
            return Err(stream
                .error("command_confirmations! requires at least one finite projector target"));
        }
        Ok(Self {
            input,
            confirmations,
        })
    }
}

impl CommandConfirmations {
    fn expand(self) -> TokenStream {
        let input = self.input;
        let confirmations = self
            .confirmations
            .into_iter()
            .map(|confirmation| confirmation.expand(&input));
        quote! {
            distributed::graphql::__command_confirmations::<#input>(
                vec![#(#confirmations),*]
            )
        }
    }
}

struct ProjectionConfirmation {
    projector: Path,
    model: Path,
    key: FieldMap,
    partition: Option<ValueExpression>,
}

impl ProjectionConfirmation {
    fn parse(stream: ParseStream<'_>) -> Result<Self> {
        let projector = stream.parse()?;
        stream.parse::<Token![->]>()?;
        let model = stream.parse()?;
        let content;
        braced!(content in stream);
        content.parse::<keyword::key>()?;
        let key = FieldMap::parse_braced(&content)?;
        let partition: Option<ValueExpression> = if content.is_empty() {
            None
        } else {
            content.parse::<Token![,]>()?;
            content.parse::<keyword::partition>()?;
            content.parse::<Token![:]>()?;
            Some(content.parse()?)
        };
        if !content.is_empty() {
            return Err(content.error("unexpected tokens after projector confirmation"));
        }
        Ok(Self {
            projector,
            model,
            key,
            partition,
        })
    }

    fn expand(self, input: &Path) -> TokenStream {
        let projector = self.projector;
        let model = self.model;
        let key = self.key.expand_key(input, &model);
        let confirmation = quote! {
            (#projector).__distributed_confirmation::<#input, #model>(#key)
        };
        match self.partition {
            Some(partition) => {
                let partition = partition.expand(input);
                quote! { (#confirmation).partition(#partition) }
            }
            None => confirmation,
        }
    }
}

enum Operation {
    Upsert(ModelWrite),
    Patch(ModelWrite),
    Delete(ModelDelete),
    Link(RelationshipWrite),
    Unlink(RelationshipWrite),
    InvalidateModel(Path),
    InvalidateRelationship(RelationshipInvalidate),
}

impl Operation {
    fn parse(stream: ParseStream<'_>) -> Result<Self> {
        if stream.peek(keyword::upsert) {
            stream.parse::<keyword::upsert>()?;
            return Ok(Self::Upsert(ModelWrite::parse(stream)?));
        }
        if stream.peek(keyword::patch) {
            stream.parse::<keyword::patch>()?;
            return Ok(Self::Patch(ModelWrite::parse(stream)?));
        }
        if stream.peek(keyword::delete) {
            stream.parse::<keyword::delete>()?;
            return Ok(Self::Delete(ModelDelete::parse(stream)?));
        }
        if stream.peek(keyword::link) {
            stream.parse::<keyword::link>()?;
            return Ok(Self::Link(RelationshipWrite::parse(stream)?));
        }
        if stream.peek(keyword::unlink) {
            stream.parse::<keyword::unlink>()?;
            return Ok(Self::Unlink(RelationshipWrite::parse(stream)?));
        }
        if stream.peek(keyword::invalidate) {
            stream.parse::<keyword::invalidate>()?;
            let model: Path = stream.parse()?;
            if stream.peek(Token![.]) {
                stream.parse::<Token![.]>()?;
                let relationship = stream.parse()?;
                let content;
                braced!(content in stream);
                content.parse::<keyword::source>()?;
                let source = FieldMap::parse_braced(&content)?;
                if !content.is_empty() {
                    return Err(content.error("unexpected tokens after invalidate source key"));
                }
                return Ok(Self::InvalidateRelationship(RelationshipInvalidate {
                    model,
                    relationship,
                    source,
                }));
            }
            return Ok(Self::InvalidateModel(model));
        }
        Err(stream.error("expected upsert, patch, delete, link, unlink, or invalidate operation"))
    }

    fn expand(self, input: &Path) -> TokenStream {
        match self {
            Self::Upsert(write) => write.expand(input, WriteKind::Upsert),
            Self::Patch(write) => write.expand(input, WriteKind::Patch),
            Self::Delete(delete) => delete.expand(input),
            Self::Link(write) => write.expand(input, RelationshipKind::Link),
            Self::Unlink(write) => write.expand(input, RelationshipKind::Unlink),
            Self::InvalidateModel(model) => quote! {
                distributed::graphql::__effect_invalidate_model::<#model>()
            },
            Self::InvalidateRelationship(invalidate) => invalidate.expand(input),
        }
    }
}

struct ModelWrite {
    model: Path,
    key: FieldMap,
    fields: FieldMap,
}

impl ModelWrite {
    fn parse(stream: ParseStream<'_>) -> Result<Self> {
        let model = stream.parse()?;
        let content;
        braced!(content in stream);
        content.parse::<keyword::key>()?;
        let key = FieldMap::parse_braced(&content)?;
        content.parse::<Token![,]>()?;
        content.parse::<keyword::set>()?;
        let fields = FieldMap::parse_braced(&content)?;
        for (field, _) in &fields.0 {
            if key.0.iter().any(|(key_field, _)| key_field == field) {
                return Err(syn::Error::new(
                    field.span(),
                    format!(
                        "effect set cannot assign key field `{field}`; upsert/patch identity materializes from `key` and rekeying is unsupported"
                    ),
                ));
            }
        }
        if !content.is_empty() {
            return Err(content.error("unexpected tokens after model effect set"));
        }
        Ok(Self { model, key, fields })
    }

    fn expand(self, input: &Path, kind: WriteKind) -> TokenStream {
        let model = self.model;
        let key = self.key.expand_key(input, &model);
        let fields = self.fields.expand_fields(input, &model);
        match kind {
            WriteKind::Upsert => quote! {
                distributed::graphql::__effect_upsert::<#model>(#key, vec![#(#fields),*])
            },
            WriteKind::Patch => quote! {
                distributed::graphql::__effect_patch::<#model>(#key, vec![#(#fields),*])
            },
        }
    }
}

enum WriteKind {
    Upsert,
    Patch,
}

struct ModelDelete {
    model: Path,
    key: FieldMap,
}

impl ModelDelete {
    fn parse(stream: ParseStream<'_>) -> Result<Self> {
        let model = stream.parse()?;
        let content;
        braced!(content in stream);
        content.parse::<keyword::key>()?;
        let key = FieldMap::parse_braced(&content)?;
        if !content.is_empty() {
            return Err(content.error("unexpected tokens after delete key"));
        }
        Ok(Self { model, key })
    }

    fn expand(self, input: &Path) -> TokenStream {
        let model = self.model;
        let key = self.key.expand_key(input, &model);
        quote! { distributed::graphql::__effect_delete::<#model>(#key) }
    }
}

struct RelationshipWrite {
    source_model: Path,
    relationship: Ident,
    target_model: Path,
    source: FieldMap,
    target: FieldMap,
}

impl RelationshipWrite {
    fn parse(stream: ParseStream<'_>) -> Result<Self> {
        let source_model = stream.parse()?;
        stream.parse::<Token![.]>()?;
        let relationship = stream.parse()?;
        stream.parse::<Token![->]>()?;
        let target_model = stream.parse()?;
        let content;
        braced!(content in stream);
        content.parse::<keyword::source>()?;
        let source = FieldMap::parse_braced(&content)?;
        content.parse::<Token![,]>()?;
        content.parse::<keyword::target>()?;
        let target = FieldMap::parse_braced(&content)?;
        if !content.is_empty() {
            return Err(content.error("unexpected tokens after relationship effect keys"));
        }
        Ok(Self {
            source_model,
            relationship,
            target_model,
            source,
            target,
        })
    }

    fn expand(self, input: &Path, kind: RelationshipKind) -> TokenStream {
        let source_model = self.source_model;
        let target_model = self.target_model;
        let relationship_method = format_ident!(
            "__Distributed{}EffectRelationship_{}",
            source_model.segments.last().unwrap().ident,
            self.relationship,
            span = self.relationship.span()
        );
        let relationship_marker = marker_path(&source_model, relationship_method);
        let relationship = quote! {
            distributed::graphql::__effect_relationship::<#relationship_marker>()
        };
        let source = self.source.expand_key(input, &source_model);
        let target = self.target.expand_key(input, &target_model);
        match kind {
            RelationshipKind::Link => quote! {
                distributed::graphql::__effect_link::<#source_model, #target_model>(
                    #relationship,
                    #source,
                    #target,
                )
            },
            RelationshipKind::Unlink => quote! {
                distributed::graphql::__effect_unlink::<#source_model, #target_model>(
                    #relationship,
                    #source,
                    #target,
                )
            },
        }
    }
}

enum RelationshipKind {
    Link,
    Unlink,
}

struct RelationshipInvalidate {
    model: Path,
    relationship: Ident,
    source: FieldMap,
}

impl RelationshipInvalidate {
    fn expand(self, input: &Path) -> TokenStream {
        let model = self.model;
        let relationship_method = format_ident!(
            "__Distributed{}EffectRelationship_{}",
            model.segments.last().unwrap().ident,
            self.relationship,
            span = self.relationship.span()
        );
        let relationship_marker = marker_path(&model, relationship_method);
        let source = self.source.expand_key(input, &model);
        quote! {
            distributed::graphql::__effect_invalidate_relationship(
                distributed::graphql::__effect_relationship::<#relationship_marker>(),
                #source,
            )
        }
    }
}

struct FieldMap(Vec<(Ident, ValueExpression)>);

impl FieldMap {
    fn parse_braced(stream: ParseStream<'_>) -> Result<Self> {
        let content;
        braced!(content in stream);
        let mut fields = Vec::new();
        let mut names = std::collections::BTreeSet::new();
        while !content.is_empty() {
            let field: Ident = content.parse()?;
            if !names.insert(field.to_string()) {
                return Err(syn::Error::new(
                    field.span(),
                    format!("duplicate command-effect field `{field}`"),
                ));
            }
            content.parse::<Token![:]>()?;
            let value = content.parse()?;
            fields.push((field, value));
            if content.is_empty() {
                break;
            }
            content.parse::<Token![,]>()?;
        }
        if fields.is_empty() {
            return Err(content.error("effect key/set must contain at least one field"));
        }
        Ok(Self(fields))
    }

    fn expand_key(self, input: &Path, model: &Path) -> TokenStream {
        let mut key_type = model.clone();
        let last = key_type
            .segments
            .last_mut()
            .expect("syn paths always contain at least one segment");
        last.ident = format_ident!(
            "__Distributed{}EffectKey",
            last.ident,
            span = last.ident.span()
        );
        let fields = self.0.into_iter().map(|(field, expression)| {
            let marker_name = format_ident!(
                "__Distributed{}EffectModelField_{}",
                model.segments.last().unwrap().ident,
                field,
                span = field.span()
            );
            let marker = marker_path(model, marker_name);
            let value = expression.expand(input);
            quote! {
                #field: distributed::graphql::__effect_key_assignment::<#marker, _>(#value)
            }
        });
        quote! {
            {
                let key: distributed::graphql::TypedEffectKey<#model> =
                    #key_type { #(#fields),* }.into();
                key
            }
        }
    }

    fn expand_fields(self, input: &Path, model: &Path) -> Vec<TokenStream> {
        self.0
            .into_iter()
            .map(|(field, expression)| {
                let method = format_ident!(
                    "__Distributed{}EffectModelField_{}",
                    model.segments.last().unwrap().ident,
                    field,
                    span = field.span()
                );
                let marker = marker_path(model, method);
                let value = expression.expand(input);
                quote! {
                    distributed::graphql::__effect_assignment::<#marker, _>(#value)
                }
            })
            .collect()
    }
}

enum ValueExpression {
    Input(Vec<InputPathSegment>),
    Trusted(syn::LitStr),
    Null,
    Constant(Expr),
    Literal(Lit),
}

struct InputPathSegment {
    field: Ident,
    /// Required on every non-leaf segment so the macro can name the next
    /// derive-generated marker without runtime reflection.
    nested_type: Option<Path>,
}

impl Parse for ValueExpression {
    fn parse(stream: ParseStream<'_>) -> Result<Self> {
        if stream.peek(Lit) {
            return Ok(Self::Literal(stream.parse()?));
        }
        let function_or_input: Ident = stream.parse()?;
        if function_or_input == "input" {
            return Ok(Self::Input(parse_input_path(stream)?));
        }

        let arguments;
        parenthesized!(arguments in stream);
        match function_or_input.to_string().as_str() {
            "uuid_v7" => {
                if !arguments.is_empty() {
                    return Err(arguments.error("uuid_v7() takes no arguments"));
                }
                Err(syn::Error::new(
                    function_or_input.span(),
                    "uuid_v7() cannot be used as an anonymous effect value; declare `default input.field = uuid_v7()` with command_input_defaults! and reference `input.field`",
                ))
            }
            "ulid" => {
                if !arguments.is_empty() {
                    return Err(arguments.error("ulid() takes no arguments"));
                }
                Err(syn::Error::new(
                    function_or_input.span(),
                    "ulid() cannot be used as an anonymous effect value; declare `default input.field = ulid()` with command_input_defaults! and reference `input.field`",
                ))
            }
            "null" => {
                if !arguments.is_empty() {
                    return Err(arguments.error("null() takes no arguments"));
                }
                Ok(Self::Null)
            }
            "constant" => {
                let value: Expr = arguments.parse()?;
                if !arguments.is_empty() {
                    return Err(arguments.error("constant() accepts exactly one Rust expression"));
                }
                validate_constant_expression(&value)?;
                Ok(Self::Constant(value))
            }
            "trusted" => {
                let name: syn::LitStr = arguments.parse()?;
                if !arguments.is_empty() {
                    return Err(arguments.error("trusted() accepts exactly one string literal"));
                }
                let value = name.value();
                if value.is_empty()
                    || value.len() > 128
                    || value.trim() != value
                    || value.chars().any(char::is_control)
                {
                    return Err(syn::Error::new(
                        name.span(),
                        "trusted() preset name must be 1..=128 bytes, have no surrounding whitespace, and contain no control characters",
                    ));
                }
                Ok(Self::Trusted(name))
            }
            other => Err(syn::Error::new(
                function_or_input.span(),
                format!(
                    "unknown command-effect expression `{other}`; use input.field, a literal, constant(value), or null()"
                ),
            )),
        }
    }
}

impl ValueExpression {
    fn expand(self, input: &Path) -> TokenStream {
        match self {
            Self::Input(segments) => {
                let first = &segments[0].field;
                let marker_name = format_ident!(
                    "__Distributed{}EffectInputField_{}",
                    input.segments.last().unwrap().ident,
                    first,
                    span = first.span()
                );
                let root_marker = marker_path(input, marker_name);
                let mut marker = quote! { #root_marker };
                for pair in segments.windows(2) {
                    let nested_type = pair[0]
                        .nested_type
                        .as_ref()
                        .expect("parser requires a type on non-leaf input segments");
                    let field = &pair[1].field;
                    let marker_name = format_ident!(
                        "__Distributed{}EffectInputField_{}",
                        nested_type.segments.last().unwrap().ident,
                        field,
                        span = field.span()
                    );
                    let nested_marker = marker_path(nested_type, marker_name);
                    marker = quote! {
                        distributed::graphql::EffectInputPath<#marker, #nested_marker>
                    };
                }
                quote! { distributed::graphql::__effect_input::<#input, #marker>() }
            }
            Self::Trusted(name) => quote! {
                distributed::graphql::__effect_trusted(#name)
            },
            Self::Null => quote! {
                distributed::graphql::__effect_null()
            },
            Self::Constant(Expr::Lit(value)) if matches!(value.lit, Lit::Str(_)) => {
                let Lit::Str(value) = value.lit else { unreachable!() };
                quote! {
                    distributed::graphql::__effect_constant(::std::string::String::from(#value))
                }
            }
            Self::Constant(value @ Expr::Path(_)) => quote! {
                distributed::graphql::__effect_constant(const { #value })
            },
            Self::Constant(value) => quote! {
                distributed::graphql::__effect_constant(#value)
            },
            Self::Literal(Lit::Str(value)) => quote! {
                distributed::graphql::__effect_constant(::std::string::String::from(#value))
            },
            Self::Literal(Lit::Bool(value)) => quote! {
                distributed::graphql::__effect_constant(#value)
            },
            Self::Literal(Lit::Int(value)) => quote! {
                distributed::graphql::__effect_constant(#value)
            },
            Self::Literal(Lit::Float(value)) => quote! {
                distributed::graphql::__effect_constant(#value)
            },
            Self::Literal(other) => syn::Error::new_spanned(
                other,
                "only string, boolean, integer, and float literals are supported in command effects",
            )
            .to_compile_error(),
        }
    }
}

fn parse_input_path(stream: ParseStream<'_>) -> Result<Vec<InputPathSegment>> {
    stream.parse::<Token![.]>()?;
    let mut segments = Vec::new();
    loop {
        let field: Ident = stream.parse()?;
        let nested_type = if stream.peek(Token![<]) {
            stream.parse::<Token![<]>()?;
            let nested_type = stream.parse()?;
            stream.parse::<Token![>]>()?;
            Some(nested_type)
        } else {
            None
        };
        let continues = stream.peek(Token![.]);
        if continues && nested_type.is_none() {
            return Err(syn::Error::new(
                field.span(),
                "nested input paths require the nested Rust type after each parent field, for example `input.profile<ProfileInput>.display_name`",
            ));
        }
        if !continues && nested_type.is_some() {
            return Err(syn::Error::new(
                field.span(),
                "a nested input type annotation must be followed by another field segment",
            ));
        }
        segments.push(InputPathSegment { field, nested_type });
        if !continues {
            break;
        }
        stream.parse::<Token![.]>()?;
    }
    Ok(segments)
}

fn validate_constant_expression(expression: &Expr) -> Result<()> {
    match expression {
        Expr::Path(path)
            if path.qself.is_none()
                && path
                    .path
                    .segments
                    .iter()
                    .all(|segment| segment.arguments.is_empty()) =>
        {
            Ok(())
        }
        Expr::Lit(literal)
            if matches!(
                literal.lit,
                Lit::Str(_) | Lit::Bool(_) | Lit::Int(_) | Lit::Float(_)
            ) =>
        {
            Ok(())
        }
        Expr::Unary(unary)
            if matches!(unary.op, syn::UnOp::Neg(_))
                && matches!(
                    unary.expr.as_ref(),
                    Expr::Lit(literal)
                        if matches!(literal.lit, Lit::Int(_) | Lit::Float(_))
                ) =>
        {
            Ok(())
        }
        _ => Err(syn::Error::new_spanned(
            expression,
            "constant(...) accepts only a primitive literal (including a negative numeric literal) or a const/enum path; calls, macros, blocks, and other operators are not deterministic command-effect IR",
        )),
    }
}

fn marker_path(base: &Path, marker: Ident) -> Path {
    let mut path = base.clone();
    path.segments
        .last_mut()
        .expect("syn paths always contain at least one segment")
        .ident = marker;
    path
}
