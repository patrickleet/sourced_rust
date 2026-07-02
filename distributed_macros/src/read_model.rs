use proc_macro::TokenStream;
use quote::{quote, ToTokens};
use syn::{
    punctuated::Punctuated, Attribute, Data, DeriveInput, Expr, ExprArray, ExprLit, Field, Fields,
    GenericArgument, Lit, LitStr, Meta, PathArguments, Token, Type,
};

pub fn derive_read_model(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_read_model(input) {
        Ok(expanded) => TokenStream::from(expanded),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_read_model(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let struct_attrs = StructAttrs::from_input(&input)?;
    let fields = named_fields(&input)?;
    let field_attrs = fields
        .named
        .iter()
        .map(FieldAttrs::from_field)
        .collect::<syn::Result<Vec<_>>>()?;
    let relational =
        struct_attrs.is_relational() || field_attrs.iter().any(FieldAttrs::is_relational);

    let id_field = find_id_field(&fields.named, &field_attrs)?;
    let collection = struct_attrs
        .collection
        .clone()
        .or_else(|| struct_attrs.table.clone())
        .unwrap_or_else(|| format!("{}s", to_snake_case(&name.to_string())));

    let read_model_impl = if let Some(id_field) = &id_field {
        Some(quote! {
            impl distributed::ReadModel for #name {
                const COLLECTION: &'static str = #collection;

                fn id(&self) -> &str {
                    &self.#id_field
                }
            }
        })
    } else if relational {
        None
    } else {
        return Err(syn::Error::new_spanned(
            input,
            "ReadModel derive requires a field named `id` or a field marked with #[readmodel(id)]",
        ));
    };

    let relational_impl = if relational {
        Some(expand_relational_read_model(
            name,
            &struct_attrs,
            &fields.named,
            &field_attrs,
            id_field.as_ref(),
        )?)
    } else {
        None
    };

    Ok(quote! {
        #read_model_impl
        #relational_impl
    })
}

fn named_fields(input: &DeriveInput) -> syn::Result<&FieldsNamed> {
    let Data::Struct(data_struct) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "ReadModel derive can only be used on structs with named fields",
        ));
    };

    let Fields::Named(fields) = &data_struct.fields else {
        return Err(syn::Error::new_spanned(
            &data_struct.fields,
            "ReadModel derive requires named fields",
        ));
    };

    Ok(fields)
}

type FieldsNamed = syn::FieldsNamed;

fn expand_relational_read_model(
    name: &syn::Ident,
    struct_attrs: &StructAttrs,
    fields: &Punctuated<Field, Token![,]>,
    field_attrs: &[FieldAttrs],
    id_field: Option<&syn::Ident>,
) -> syn::Result<proc_macro2::TokenStream> {
    let model_name = name.to_string();
    let table_name = struct_attrs
        .table
        .clone()
        .or_else(|| struct_attrs.collection.clone())
        .unwrap_or_else(|| format!("{}s", to_snake_case(&model_name)));

    let primary_key_fields =
        relational_primary_key_fields(struct_attrs, fields, field_attrs, id_field);
    let primary_key_columns = primary_key_fields
        .iter()
        .map(|column| quote! { #column.to_string() })
        .collect::<Vec<_>>();

    let mut column_defs = Vec::new();
    let mut row_inserts = Vec::new();
    let mut row_fields = Vec::new();
    let mut key_inserts = Vec::new();
    let mut foreign_keys = Vec::new();
    let mut indexes = Vec::new();
    let mut relationships = Vec::new();
    let mut hydrate_include_arms = Vec::new();
    let mut include_rows_arms = Vec::new();
    let mut include_schema_arms = Vec::new();

    for (field, attrs) in fields.iter().zip(field_attrs) {
        let ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(field, "ReadModel fields must be named"))?;
        let field_name = ident.to_string();

        if let Some(relationship) = attrs.relationship_tokens(&field_name)? {
            relationships.push(relationship);
            let (hydrate_arm, include_rows_arm, include_schema_arm) =
                attrs.relationship_include_tokens(field, &field_name)?;
            hydrate_include_arms.push(hydrate_arm);
            include_rows_arms.push(include_rows_arm);
            include_schema_arms.push(include_schema_arm);
            row_fields.push(quote! { #ident: ::core::default::Default::default() });
            continue;
        }

        if attrs.skip_query {
            row_fields.push(quote! { #ident: ::core::default::Default::default() });
            continue;
        }

        let column_name = attrs.column.clone().unwrap_or_else(|| field_name.clone());
        let primary_key = primary_key_fields
            .iter()
            .any(|pk| pk == &field_name || pk == &column_name);
        let nullable = attrs.nullable || option_inner_type(&field.ty).is_some();
        let column_type = column_type_tokens(&field.ty, attrs.jsonb);
        let default_tokens = option_string_tokens(attrs.default.as_deref());
        let foreign_key_value = attrs.foreign_key.as_ref().map(foreign_key_tokens);
        let foreign_key = foreign_key_value
            .as_ref()
            .map(|foreign_key| quote! { Some(#foreign_key) })
            .unwrap_or_else(|| quote! { None });
        if let Some(foreign_key) = &foreign_key_value {
            foreign_keys.push(foreign_key.clone());
        }
        let delegated_from = option_string_tokens(attrs.delegated_from.as_deref());
        let has_default = attrs.has_default;
        let jsonb = attrs.jsonb;

        column_defs.push(quote! {
            distributed::ColumnDef {
                field_name: #field_name.to_string(),
                column_name: #column_name.to_string(),
                column_type: #column_type,
                nullable: #nullable,
                has_default: #has_default,
                default: #default_tokens,
                primary_key: #primary_key,
                foreign_key: #foreign_key,
                delegated_from: #delegated_from,
                jsonb: #jsonb,
                skipped: false,
            }
        });

        if let Some(value) = bytes_row_value_tokens(&field.ty, quote! { self.#ident }) {
            row_inserts.push(quote! {
                row.insert(#column_name, #value);
            });
        } else {
            row_inserts.push(quote! {
                row.insert_serde(#column_name, &self.#ident)?;
            });
        }
        row_fields.push(quote! {
            #ident: row.get_serde(#column_name)?
        });

        if primary_key {
            if let Some(value) = bytes_row_value_tokens(&field.ty, quote! { self.#ident }) {
                key_inserts.push(quote! {
                    key.values.insert(#column_name.to_string(), #value);
                });
            } else {
                key_inserts.push(quote! {
                    key.values.insert(
                        #column_name.to_string(),
                        distributed::RowValue::from_serde(&self.#ident)?,
                    );
                });
            }
        }

        if attrs.indexed || attrs.unique {
            let index_columns = vec![column_name.clone()];
            let index_name = attrs
                .index_name
                .clone()
                .unwrap_or_else(|| default_index_name(&table_name, &index_columns, attrs.unique));
            let unique = attrs.unique;
            indexes.push(index_def_tokens(index_name, index_columns, unique));
        }
    }

    for index in &struct_attrs.indexes {
        let index_columns = index
            .columns
            .iter()
            .map(|column| resolve_column_reference(column, fields, field_attrs))
            .collect::<Vec<_>>();
        let index_name = index
            .name
            .clone()
            .unwrap_or_else(|| default_index_name(&table_name, &index_columns, index.unique));
        indexes.push(index_def_tokens(index_name, index_columns, index.unique));
    }

    Ok(quote! {
        impl distributed::RelationalReadModel for #name {
            fn schema() -> &'static distributed::ReadModelSchema {
                static SCHEMA: ::std::sync::LazyLock<distributed::ReadModelSchema> =
                    ::std::sync::LazyLock::new(|| distributed::ReadModelSchema {
                        model_name: #model_name.to_string(),
                        table_name: #table_name.to_string(),
                        columns: vec![#(#column_defs),*],
                        primary_key: distributed::PrimaryKey {
                            columns: vec![#(#primary_key_columns),*],
                        },
                        version_column: Some(distributed::DEFAULT_READ_MODEL_VERSION_COLUMN.to_string()),
                        foreign_keys: vec![#(#foreign_keys),*],
                        indexes: vec![#(#indexes),*],
                        relationships: vec![#(#relationships),*],
                    });
                &SCHEMA
            }

            fn primary_key(&self) -> Result<distributed::RowKey, distributed::ReadModelError> {
                let mut key = distributed::RowKey::default();
                #(#key_inserts)*
                Ok(key)
            }

            fn to_row(&self) -> Result<distributed::RowValues, distributed::ReadModelError> {
                let mut row = distributed::RowValues::new();
                #(#row_inserts)*
                Ok(row)
            }

            fn from_row(row: distributed::RowValues) -> Result<Self, distributed::ReadModelError> {
                Ok(Self {
                    #(#row_fields),*
                })
            }
        }

        impl distributed::RelationalReadModelIncludes for #name {
            fn hydrate_include(
                &mut self,
                include: &str,
                rows: Vec<distributed::RowValues>,
            ) -> Result<(), distributed::ReadModelError> {
                match include {
                    #(#hydrate_include_arms,)*
                    _ => Err(distributed::ReadModelError::Metadata(format!(
                        "read model `{}` has no hydratable relationship `{}`",
                        #model_name,
                        include
                    ))),
                }
            }

            fn include_rows(
                &self,
                include: &str,
            ) -> Result<Vec<distributed::RowValues>, distributed::ReadModelError> {
                match include {
                    #(#include_rows_arms,)*
                    _ => Err(distributed::ReadModelError::Metadata(format!(
                        "read model `{}` has no tracked relationship `{}`",
                        #model_name,
                        include
                    ))),
                }
            }

            fn include_target_schema(
                include: &str,
            ) -> Result<&'static distributed::ReadModelSchema, distributed::ReadModelError> {
                match include {
                    #(#include_schema_arms,)*
                    _ => Err(distributed::ReadModelError::Metadata(format!(
                        "read model `{}` has no tracked relationship `{}`",
                        #model_name,
                        include
                    ))),
                }
            }
        }
    })
}

fn relational_primary_key_fields(
    struct_attrs: &StructAttrs,
    fields: &Punctuated<Field, Token![,]>,
    field_attrs: &[FieldAttrs],
    id_field: Option<&syn::Ident>,
) -> Vec<String> {
    if !struct_attrs.primary_key.is_empty() {
        return struct_attrs
            .primary_key
            .iter()
            .map(|key| resolve_column_reference(key, fields, field_attrs))
            .collect();
    }

    id_field
        .map(|id| {
            let id_name = id.to_string();
            resolve_column_reference(&id_name, fields, field_attrs)
        })
        .into_iter()
        .collect()
}

fn resolve_column_reference(
    reference: &str,
    fields: &Punctuated<Field, Token![,]>,
    field_attrs: &[FieldAttrs],
) -> String {
    fields
        .iter()
        .zip(field_attrs)
        .find_map(|(field, attrs)| {
            let field_name = field.ident.as_ref()?.to_string();
            if field_name == reference {
                Some(attrs.column.clone().unwrap_or(field_name))
            } else {
                None
            }
        })
        .unwrap_or_else(|| reference.to_string())
}

fn default_index_name(table_name: &str, columns: &[String], unique: bool) -> String {
    let prefix = if unique { "uq" } else { "idx" };
    format!("{prefix}_{table_name}_{}", columns.join("_"))
}

fn index_def_tokens(
    index_name: String,
    index_columns: Vec<String>,
    unique: bool,
) -> proc_macro2::TokenStream {
    let columns = index_columns
        .iter()
        .map(|column| quote! { #column.to_string() })
        .collect::<Vec<_>>();

    quote! {
        distributed::IndexDef {
            name: Some(#index_name.to_string()),
            columns: vec![#(#columns),*],
            unique: #unique,
        }
    }
}

#[derive(Default)]
struct StructAttrs {
    collection: Option<String>,
    table: Option<String>,
    primary_key: Vec<String>,
    indexes: Vec<IndexAttr>,
}

struct IndexAttr {
    name: Option<String>,
    columns: Vec<String>,
    unique: bool,
}

impl StructAttrs {
    fn from_input(input: &DeriveInput) -> syn::Result<Self> {
        let mut attrs = Self::default();
        for attr in &input.attrs {
            if attr.path().is_ident("collection") {
                attrs.collection = Some(parse_direct_string_attr(attr, "collection")?);
                continue;
            }

            if attr.path().is_ident("table") {
                attrs.table = Some(parse_direct_string_attr(attr, "table")?);
                continue;
            }

            if attr.path().is_ident("index") {
                attrs
                    .indexes
                    .push(parse_direct_index_attr(attr, "index", false)?);
                continue;
            }

            if attr.path().is_ident("unique") {
                attrs
                    .indexes
                    .push(parse_direct_index_attr(attr, "unique", true)?);
                continue;
            }

            if !attr.path().is_ident("readmodel") {
                continue;
            }

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("collection") {
                    attrs.collection = Some(meta.value()?.parse::<LitStr>()?.value());
                } else if meta.path.is_ident("table") {
                    attrs.table = Some(meta.value()?.parse::<LitStr>()?.value());
                } else if meta.path.is_ident("primary_key") {
                    let expr = meta.value()?.parse::<Expr>()?;
                    attrs.primary_key = parse_string_list(expr)?;
                } else {
                    return Err(meta.error("unknown readmodel struct attribute"));
                }
                Ok(())
            })?;
        }
        Ok(attrs)
    }

    fn is_relational(&self) -> bool {
        self.table.is_some() || !self.primary_key.is_empty() || !self.indexes.is_empty()
    }
}

#[derive(Default)]
struct FieldAttrs {
    id: bool,
    column: Option<String>,
    indexed: bool,
    index_name: Option<String>,
    unique: bool,
    jsonb: bool,
    skip_query: bool,
    nullable: bool,
    has_default: bool,
    default: Option<String>,
    foreign_key: Option<ForeignKeyParts>,
    delegated_from: Option<String>,
    relationship: Option<RelationshipAttr>,
}

impl FieldAttrs {
    fn from_field(field: &Field) -> syn::Result<Self> {
        let mut attrs = Self::default();
        let mut pending_foreign_key: Option<String> = None;
        let mut pending_through: Option<String> = None;
        for attr in &field.attrs {
            if attr.path().is_ident("id") {
                attrs.id = true;
                if let Some(column) = parse_optional_direct_string_attr(attr)? {
                    attrs.column = Some(column);
                }
                continue;
            }

            if attr.path().is_ident("column") {
                attrs.column = Some(parse_direct_string_attr(attr, "column")?);
                continue;
            }

            if attr.path().is_ident("index") {
                attrs.indexed = true;
                if let Some(index_name) = parse_optional_direct_string_attr(attr)? {
                    attrs.index_name = Some(index_name);
                }
                continue;
            }

            if attr.path().is_ident("unique") {
                attrs.unique = true;
                attrs.indexed = true;
                if let Some(index_name) = parse_optional_direct_string_attr(attr)? {
                    attrs.index_name = Some(index_name);
                }
                continue;
            }

            if !attr.path().is_ident("readmodel") {
                continue;
            }

            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("id") {
                    attrs.id = true;
                } else if meta.path.is_ident("column") {
                    attrs.column = Some(meta.value()?.parse::<LitStr>()?.value());
                } else if meta.path.is_ident("index") {
                    attrs.indexed = true;
                    if meta.input.peek(Token![=]) {
                        attrs.index_name = Some(meta.value()?.parse::<LitStr>()?.value());
                    }
                } else if meta.path.is_ident("unique") {
                    attrs.unique = true;
                    attrs.indexed = true;
                } else if meta.path.is_ident("jsonb") {
                    attrs.jsonb = true;
                } else if meta.path.is_ident("skip_query")
                    || meta.path.is_ident("skip")
                    || meta.path.is_ident("private")
                {
                    attrs.skip_query = true;
                } else if meta.path.is_ident("nullable") {
                    attrs.nullable = true;
                } else if meta.path.is_ident("default") {
                    attrs.has_default = true;
                    if meta.input.peek(Token![=]) {
                        attrs.default = Some(meta.value()?.parse::<LitStr>()?.value());
                    }
                } else if meta.path.is_ident("foreign_key") {
                    let value = meta.value()?.parse::<LitStr>()?.value();
                    if attrs.relationship.is_some() {
                        let relationship = relationship_mut(&mut attrs, "foreign_key")?;
                        if relationship.foreign_key.is_some() {
                            return Err(
                                meta.error("relationship foreign_key declared more than once")
                            );
                        }
                        relationship.foreign_key = Some(value);
                    } else if pending_foreign_key.is_some() {
                        return Err(meta.error("relationship foreign_key declared more than once"));
                    } else {
                        pending_foreign_key = Some(value);
                    }
                } else if meta.path.is_ident("delegated_from") {
                    attrs.delegated_from = Some(meta.value()?.parse::<LitStr>()?.value());
                } else if meta.path.is_ident("has_many") {
                    let target = meta.value()?.parse::<LitStr>()?.value();
                    attrs.relationship = Some(RelationshipAttr {
                        kind: RelationshipKindAttr::HasMany,
                        target_model: target,
                        foreign_key: None,
                        through: None,
                    });
                } else if meta.path.is_ident("belongs_to") {
                    let target = meta.value()?.parse::<LitStr>()?.value();
                    attrs.relationship = Some(RelationshipAttr {
                        kind: RelationshipKindAttr::BelongsTo,
                        target_model: target,
                        foreign_key: None,
                        through: None,
                    });
                } else if meta.path.is_ident("many_to_many") {
                    let target = meta.value()?.parse::<LitStr>()?.value();
                    attrs.relationship = Some(RelationshipAttr {
                        kind: RelationshipKindAttr::ManyToMany,
                        target_model: target,
                        foreign_key: None,
                        through: None,
                    });
                } else if meta.path.is_ident("through") {
                    let through = meta.value()?.parse::<LitStr>()?.value();
                    if attrs.relationship.is_some() {
                        let relationship = relationship_mut(&mut attrs, "through")?;
                        if relationship.through.is_some() {
                            return Err(meta.error("relationship through declared more than once"));
                        }
                        relationship.through = Some(through);
                    } else if pending_through.is_some() {
                        return Err(meta.error("relationship through declared more than once"));
                    } else {
                        pending_through = Some(through);
                    }
                } else {
                    return Err(meta.error("unknown readmodel field attribute"));
                }
                Ok(())
            })?;
        }

        if let Some(value) = pending_foreign_key {
            if let Some(relationship) = attrs.relationship.as_mut() {
                if relationship.foreign_key.is_some() {
                    return Err(syn::Error::new_spanned(
                        field,
                        "relationship foreign_key declared more than once",
                    ));
                }
                relationship.foreign_key = Some(value);
            } else {
                attrs.foreign_key = Some(parse_foreign_key(&value)?);
            }
        }

        if let Some(through) = pending_through {
            if let Some(relationship) = attrs.relationship.as_mut() {
                if relationship.through.is_some() {
                    return Err(syn::Error::new_spanned(
                        field,
                        "relationship through declared more than once",
                    ));
                }
                relationship.through = Some(through);
            } else {
                return Err(syn::Error::new_spanned(
                    field,
                    "`through` must be declared with a relationship attribute",
                ));
            }
        }

        Ok(attrs)
    }

    fn is_relational(&self) -> bool {
        self.column.is_some()
            || self.indexed
            || self.unique
            || self.jsonb
            || self.skip_query
            || self.nullable
            || self.has_default
            || self.foreign_key.is_some()
            || self.delegated_from.is_some()
            || self.relationship.is_some()
    }

    fn relationship_tokens(
        &self,
        field_name: &str,
    ) -> syn::Result<Option<proc_macro2::TokenStream>> {
        let Some(relationship) = &self.relationship else {
            return Ok(None);
        };
        let Some(foreign_key) = relationship.foreign_key.as_deref() else {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("relationship `{field_name}` must declare `foreign_key = \"...\"`"),
            ));
        };
        let target_model = &relationship.target_model;
        let through = option_string_tokens(relationship.through.as_deref());
        let kind = match relationship.kind {
            RelationshipKindAttr::HasMany => quote! { distributed::RelationshipKind::HasMany },
            RelationshipKindAttr::BelongsTo => quote! { distributed::RelationshipKind::BelongsTo },
            RelationshipKindAttr::ManyToMany => {
                quote! { distributed::RelationshipKind::ManyToMany }
            }
        };
        Ok(Some(quote! {
            distributed::RelationshipDef {
                field_name: #field_name.to_string(),
                kind: #kind,
                target_model: #target_model.to_string(),
                foreign_key: Some(#foreign_key.to_string()),
                through: #through,
            }
        }))
    }

    fn relationship_include_tokens(
        &self,
        field: &Field,
        field_name: &str,
    ) -> syn::Result<(
        proc_macro2::TokenStream,
        proc_macro2::TokenStream,
        proc_macro2::TokenStream,
    )> {
        let relationship = self.relationship.as_ref().ok_or_else(|| {
            syn::Error::new_spanned(field, "field is not a read-model relationship")
        })?;
        let ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(field, "ReadModel fields must be named"))?;

        match relationship.kind {
            RelationshipKindAttr::HasMany | RelationshipKindAttr::ManyToMany => {
                let inner = vec_inner_type(&field.ty).ok_or_else(|| {
                    syn::Error::new_spanned(
                        field,
                        format!("relationship `{field_name}` must be shaped as `Vec<T>`"),
                    )
                })?;
                validate_relationship_target_type(
                    field,
                    inner,
                    &relationship.target_model,
                    field_name,
                )?;
                let hydrate = quote! {
                    #field_name => {
                        self.#ident = rows
                            .into_iter()
                            .map(<#inner as distributed::RelationalReadModel>::from_row)
                            .collect::<Result<Vec<_>, distributed::ReadModelError>>()?;
                        Ok(())
                    }
                };
                let include_rows = quote! {
                    #field_name => self
                        .#ident
                        .iter()
                        .map(distributed::RelationalReadModel::to_row)
                        .collect::<Result<Vec<_>, distributed::ReadModelError>>()
                };
                let include_schema = quote! {
                    #field_name => Ok(<#inner as distributed::RelationalReadModel>::schema())
                };
                Ok((hydrate, include_rows, include_schema))
            }
            RelationshipKindAttr::BelongsTo => {
                let inner = option_inner_type(&field.ty).ok_or_else(|| {
                    syn::Error::new_spanned(
                        field,
                        format!(
                            "belongs_to relationship `{field_name}` must be shaped as `Option<T>`"
                        ),
                    )
                })?;
                validate_relationship_target_type(
                    field,
                    inner,
                    &relationship.target_model,
                    field_name,
                )?;
                let hydrate = quote! {
                    #field_name => {
                        let mut rows = rows.into_iter();
                        self.#ident = match rows.next() {
                            Some(row) => Some(<#inner as distributed::RelationalReadModel>::from_row(row)?),
                            None => None,
                        };
                        if rows.next().is_some() {
                            return Err(distributed::ReadModelError::Metadata(format!(
                                "belongs_to relationship `{}` returned more than one row",
                                #field_name
                            )));
                        }
                        Ok(())
                    }
                };
                let include_rows = quote! {
                    #field_name => {
                        let mut rows = Vec::new();
                        if let Some(value) = &self.#ident {
                            rows.push(distributed::RelationalReadModel::to_row(value)?);
                        }
                        Ok(rows)
                    }
                };
                let include_schema = quote! {
                    #field_name => Ok(<#inner as distributed::RelationalReadModel>::schema())
                };
                Ok((hydrate, include_rows, include_schema))
            }
        }
    }
}

fn relationship_mut<'a>(
    attrs: &'a mut FieldAttrs,
    name: &str,
) -> syn::Result<&'a mut RelationshipAttr> {
    attrs.relationship.as_mut().ok_or_else(|| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("`{name}` must be declared after a relationship attribute"),
        )
    })
}

#[derive(Clone)]
struct ForeignKeyParts {
    table: String,
    column: String,
}

#[derive(Clone)]
struct RelationshipAttr {
    kind: RelationshipKindAttr,
    target_model: String,
    foreign_key: Option<String>,
    through: Option<String>,
}

#[derive(Clone, Copy)]
enum RelationshipKindAttr {
    HasMany,
    BelongsTo,
    ManyToMany,
}

fn find_id_field(
    fields: &Punctuated<Field, Token![,]>,
    field_attrs: &[FieldAttrs],
) -> syn::Result<Option<syn::Ident>> {
    let mut explicit_id: Option<syn::Ident> = None;
    for (field, attrs) in fields.iter().zip(field_attrs) {
        if attrs.id {
            let ident = field.ident.clone().ok_or_else(|| {
                syn::Error::new_spanned(field, "ReadModel id field must be named")
            })?;
            if let Some(previous) = &explicit_id {
                return Err(syn::Error::new_spanned(
                    field,
                    format!(
                        "Multiple #[readmodel(id)] fields found: `{}` and `{}`",
                        previous, ident
                    ),
                ));
            }
            explicit_id = Some(ident);
        }
    }

    if explicit_id.is_some() {
        return Ok(explicit_id);
    }

    Ok(fields
        .iter()
        .filter_map(|field| field.ident.clone())
        .find(|ident| ident == "id"))
}

fn parse_string_list(expr: Expr) -> syn::Result<Vec<String>> {
    match expr {
        Expr::Array(ExprArray { elems, .. }) => elems.into_iter().map(parse_string_expr).collect(),
        expr => parse_string_expr(expr).map(|value| vec![value]),
    }
}

fn parse_string_expr(expr: Expr) -> syn::Result<String> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(value),
            ..
        }) => Ok(value.value()),
        other => Err(syn::Error::new_spanned(
            other,
            "expected string literal in readmodel attribute",
        )),
    }
}

fn parse_direct_string_attr(attr: &Attribute, attr_name: &str) -> syn::Result<String> {
    parse_optional_direct_string_attr(attr)?.ok_or_else(|| {
        syn::Error::new_spanned(attr, format!("#[{attr_name}] requires a string literal"))
    })
}

fn parse_optional_direct_string_attr(attr: &Attribute) -> syn::Result<Option<String>> {
    match &attr.meta {
        Meta::List(list) => Ok(Some(list.parse_args::<LitStr>()?.value())),
        Meta::NameValue(name_value) => parse_string_expr(name_value.value.clone()).map(Some),
        Meta::Path(_) => Ok(None),
    }
}

fn parse_direct_index_attr(
    attr: &Attribute,
    attr_name: &str,
    unique: bool,
) -> syn::Result<IndexAttr> {
    let mut name = None;
    let mut columns = None;

    match &attr.meta {
        Meta::List(_) => {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    name = Some(meta.value()?.parse::<LitStr>()?.value());
                } else if meta.path.is_ident("columns") {
                    let expr = meta.value()?.parse::<Expr>()?;
                    columns = Some(parse_string_list(expr)?);
                } else {
                    return Err(meta.error(format!("unknown {attr_name} attribute")));
                }
                Ok(())
            })?;
        }
        Meta::NameValue(name_value) => {
            columns = Some(parse_string_list(name_value.value.clone())?);
        }
        Meta::Path(_) => {}
    }

    let columns = columns.ok_or_else(|| {
        syn::Error::new_spanned(attr, format!("#[{attr_name}] requires columns = [\"...\"]"))
    })?;
    if columns.is_empty() {
        return Err(syn::Error::new_spanned(
            attr,
            format!("#[{attr_name}] requires at least one column"),
        ));
    }

    Ok(IndexAttr {
        name,
        columns,
        unique,
    })
}

fn parse_foreign_key(value: &str) -> syn::Result<ForeignKeyParts> {
    let Some((table, column)) = value.split_once('.') else {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "foreign_key must use `table.column` syntax",
        ));
    };
    Ok(ForeignKeyParts {
        table: table.to_string(),
        column: column.to_string(),
    })
}

fn foreign_key_tokens(foreign_key: &ForeignKeyParts) -> proc_macro2::TokenStream {
    let table = &foreign_key.table;
    let column = &foreign_key.column;
    quote! {
        distributed::ForeignKey {
            table: #table.to_string(),
            column: #column.to_string(),
        }
    }
}

fn option_string_tokens(value: Option<&str>) -> proc_macro2::TokenStream {
    match value {
        Some(value) => quote! { Some(#value.to_string()) },
        None => quote! { None },
    }
}

fn column_type_tokens(ty: &Type, jsonb: bool) -> proc_macro2::TokenStream {
    if jsonb {
        return quote! { distributed::ColumnType::Json };
    }

    let ty = option_inner_type(ty).unwrap_or(ty);
    if let Some(last) = last_type_segment(ty) {
        let ident = last.ident.to_string();
        return match ident.as_str() {
            "String" | "str" => quote! { distributed::ColumnType::Text },
            "bool" => quote! { distributed::ColumnType::Boolean },
            "i8" | "i16" | "i32" | "i64" | "isize" => {
                quote! { distributed::ColumnType::Integer }
            }
            "u8" | "u16" | "u32" | "u64" | "usize" => {
                quote! { distributed::ColumnType::UnsignedInteger }
            }
            "f32" | "f64" => quote! { distributed::ColumnType::Float },
            "Vec" => {
                if vec_inner_is_u8(last) {
                    quote! { distributed::ColumnType::Bytes }
                } else {
                    quote! { distributed::ColumnType::Json }
                }
            }
            "HashMap" | "BTreeMap" | "Value" => quote! { distributed::ColumnType::Json },
            _ => {
                let type_name = ty.to_token_stream().to_string();
                quote! { distributed::ColumnType::Unsupported(#type_name.to_string()) }
            }
        };
    }

    let type_name = ty.to_token_stream().to_string();
    quote! { distributed::ColumnType::Unsupported(#type_name.to_string()) }
}

fn bytes_row_value_tokens(
    ty: &Type,
    value: proc_macro2::TokenStream,
) -> Option<proc_macro2::TokenStream> {
    let option_inner = option_inner_type(ty);
    let ty = option_inner.unwrap_or(ty);
    let segment = last_type_segment(ty)?;
    if segment.ident != "Vec" || !vec_inner_is_u8(segment) {
        return None;
    }

    if option_inner.is_some() {
        Some(quote! {
            match &#value {
                Some(value) => distributed::RowValue::Bytes(value.clone()),
                None => distributed::RowValue::Null,
            }
        })
    } else {
        Some(quote! {
            distributed::RowValue::Bytes(#value.clone())
        })
    }
}

fn option_inner_type(ty: &Type) -> Option<&Type> {
    let segment = last_type_segment(ty)?;
    if segment.ident != "Option" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        GenericArgument::Type(ty) => Some(ty),
        _ => None,
    })
}

fn vec_inner_type(ty: &Type) -> Option<&Type> {
    let segment = last_type_segment(ty)?;
    if segment.ident != "Vec" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        GenericArgument::Type(ty) => Some(ty),
        _ => None,
    })
}

fn validate_relationship_target_type(
    field: &Field,
    ty: &Type,
    target_model: &str,
    field_name: &str,
) -> syn::Result<()> {
    let Some(segment) = last_type_segment(ty) else {
        return Err(syn::Error::new_spanned(
            field,
            format!("relationship `{field_name}` target type must be a named read model"),
        ));
    };
    if segment.ident != target_model {
        return Err(syn::Error::new_spanned(
            field,
            format!(
                "relationship `{field_name}` targets `{target_model}` but the field stores `{}`",
                segment.ident
            ),
        ));
    }
    Ok(())
}

fn last_type_segment(ty: &Type) -> Option<&syn::PathSegment> {
    match ty {
        Type::Path(path) => path.path.segments.last(),
        Type::Reference(reference) => last_type_segment(&reference.elem),
        _ => None,
    }
}

fn vec_inner_is_u8(segment: &syn::PathSegment) -> bool {
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return false;
    };
    args.args.iter().any(|arg| match arg {
        GenericArgument::Type(Type::Path(path)) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "u8"),
        _ => false,
    })
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.extend(ch.to_lowercase());
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_read_model_accepts_named_id_field() {
        let input: DeriveInput = syn::parse_quote! {
            struct CounterView {
                id: String,
                value: i32,
            }
        };

        let expanded = expand_read_model(input).unwrap().to_string();

        assert!(expanded.contains("impl distributed :: ReadModel for CounterView"));
        assert!(expanded.contains("fn id"));
    }

    #[test]
    fn expand_read_model_accepts_explicit_id_attribute() {
        let input: DeriveInput = syn::parse_quote! {
            struct CounterView {
                id: String,
                #[readmodel(id)]
                counter_id: String,
                value: i32,
            }
        };

        let expanded = expand_read_model(input).unwrap().to_string();

        assert!(expanded.contains("& self . counter_id"));
        assert!(!expanded.contains("& self . id"));
    }

    #[test]
    fn expand_read_model_accepts_direct_collection_attribute() {
        let input: DeriveInput = syn::parse_quote! {
            #[collection("counter_views")]
            struct CounterView {
                #[id]
                counter_id: String,
                value: i32,
            }
        };

        let expanded = expand_read_model(input).unwrap().to_string();

        assert!(expanded.contains("const COLLECTION : & 'static str = \"counter_views\""));
        assert!(expanded.contains("& self . counter_id"));
    }

    #[test]
    fn expand_read_model_accepts_direct_table_and_id_column_attributes() {
        let input: DeriveInput = syn::parse_quote! {
            #[table = "counter_views"]
            struct CounterView {
                #[id("counter_id")]
                id: String,
                value: i32,
            }
        };

        let expanded = expand_read_model(input).unwrap().to_string();

        assert!(expanded.contains("table_name : \"counter_views\""));
        assert!(expanded.contains("column_name : \"counter_id\""));
    }

    #[test]
    fn expand_read_model_accepts_direct_column_attribute() {
        let input: DeriveInput = syn::parse_quote! {
            #[table("counter_views")]
            struct CounterView {
                #[id]
                id: String,
                #[column("counter_value")]
                value: i32,
            }
        };

        let expanded = expand_read_model(input).unwrap().to_string();

        assert!(expanded.contains("column_name : \"counter_value\""));
    }

    #[test]
    fn expand_read_model_accepts_direct_index_attribute() {
        let input: DeriveInput = syn::parse_quote! {
            #[table("counter_views")]
            struct CounterView {
                #[id]
                id: String,
                #[index("idx_counter_views_value")]
                value: i32,
            }
        };

        let expanded = expand_read_model(input).unwrap().to_string();

        assert!(expanded.contains("name : Some (\"idx_counter_views_value\""));
        assert!(expanded.contains("columns : vec ! [\"value\""));
        assert!(expanded.contains("unique : false"));
    }

    #[test]
    fn expand_read_model_accepts_direct_unique_attribute() {
        let input: DeriveInput = syn::parse_quote! {
            #[table("counter_views")]
            struct CounterView {
                #[id]
                id: String,
                #[unique("uq_counter_views_slug")]
                slug: String,
            }
        };

        let expanded = expand_read_model(input).unwrap().to_string();

        assert!(expanded.contains("name : Some (\"uq_counter_views_slug\""));
        assert!(expanded.contains("columns : vec ! [\"slug\""));
        assert!(expanded.contains("unique : true"));
    }

    #[test]
    fn expand_read_model_accepts_struct_compound_index_attribute() {
        let input: DeriveInput = syn::parse_quote! {
            #[table("account_summaries")]
            #[index(name = "idx_account_summaries_owner_created", columns = ["owner", "created_at"])]
            struct AccountSummary {
                #[id("account_id")]
                id: String,
                owner: String,
                #[column("created_at_utc")]
                created_at: String,
            }
        };

        let expanded = expand_read_model(input).unwrap().to_string();

        assert!(expanded.contains("name : Some (\"idx_account_summaries_owner_created\""));
        assert!(expanded.contains("columns : vec ! [\"owner\" . to_string () , \"created_at_utc\""));
        assert!(expanded.contains("unique : false"));
    }

    #[test]
    fn expand_read_model_accepts_struct_compound_unique_attribute() {
        let input: DeriveInput = syn::parse_quote! {
            #[table("accounts")]
            #[unique(columns = ["tenant_id", "slug"])]
            struct AccountSummary {
                #[id("account_id")]
                id: String,
                tenant_id: String,
                slug: String,
            }
        };

        let expanded = expand_read_model(input).unwrap().to_string();

        assert!(expanded.contains("name : Some (\"uq_accounts_tenant_id_slug\""));
        assert!(expanded.contains("columns : vec ! [\"tenant_id\""));
        assert!(expanded.contains("\"slug\""));
        assert!(expanded.contains("unique : true"));
    }

    #[test]
    fn expand_read_model_rejects_struct_index_without_columns() {
        let input: DeriveInput = syn::parse_quote! {
            #[table("counter_views")]
            #[index]
            struct CounterView {
                #[id]
                id: String,
                value: i32,
            }
        };

        let err = expand_read_model(input).expect_err("struct index needs columns");

        assert!(
            err.to_string().contains("#[index] requires columns"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn expand_read_model_rejects_direct_attributes_without_values() {
        let input: DeriveInput = syn::parse_quote! {
            #[collection]
            struct CounterView {
                id: String,
                value: i32,
            }
        };

        let err = expand_read_model(input).expect_err("direct collection needs a value");

        assert!(
            err.to_string()
                .contains("#[collection] requires a string literal"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn expand_read_model_rejects_missing_id_field_for_document_models() {
        let input: DeriveInput = syn::parse_quote! {
            struct CounterView {
                value: i32,
            }
        };

        let err = expand_read_model(input).expect_err("missing id field should return an error");

        assert!(
            err.to_string().contains("field named `id`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn expand_read_model_allows_composite_relational_models_without_string_id() {
        let input: DeriveInput = syn::parse_quote! {
            #[readmodel(table = "player_weapons", primary_key = ["player_id", "weapon_id"])]
            struct PlayerWeapon {
                #[readmodel(foreign_key = "players.player_id", delegated_from = "Player.player_id")]
                player_id: String,
                weapon_id: String,
            }
        };

        let expanded = expand_read_model(input).unwrap().to_string();

        assert!(expanded.contains("impl distributed :: RelationalReadModel for PlayerWeapon"));
        assert!(!expanded.contains("impl distributed :: ReadModel for PlayerWeapon"));
    }

    #[test]
    fn expand_read_model_rejects_multiple_explicit_id_attributes() {
        let input: DeriveInput = syn::parse_quote! {
            struct CounterView {
                #[readmodel(id)]
                counter_id: String,
                #[readmodel(id)]
                tenant_id: String,
                value: i32,
            }
        };

        let err = expand_read_model(input).expect_err("multiple ids should return an error");

        assert!(
            err.to_string()
                .contains("Multiple #[readmodel(id)] fields found"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn expand_read_model_rejects_unknown_struct_attributes() {
        let input: DeriveInput = syn::parse_quote! {
            #[readmodel(tabel = "counter_views")]
            struct CounterView {
                id: String,
                value: i32,
            }
        };

        let err = expand_read_model(input).expect_err("unknown struct attribute should fail");

        assert!(
            err.to_string()
                .contains("unknown readmodel struct attribute"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn expand_read_model_rejects_unknown_field_attributes() {
        let input: DeriveInput = syn::parse_quote! {
            struct CounterView {
                #[readmodel(ide)]
                id: String,
                value: i32,
            }
        };

        let err = expand_read_model(input).expect_err("unknown field attribute should fail");

        assert!(
            err.to_string()
                .contains("unknown readmodel field attribute"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn expand_read_model_accepts_relationship_metadata_before_relationship_kind() {
        let input: DeriveInput = syn::parse_quote! {
            #[readmodel(table = "players")]
            struct Player {
                #[readmodel(id, column = "player_id")]
                id: String,
                #[readmodel(foreign_key = "player_id", has_many = "PlayerWeapon")]
                weapons: Vec<PlayerWeapon>,
            }
        };

        let expanded = expand_read_model(input).unwrap().to_string();

        assert!(expanded.contains("RelationshipKind :: HasMany"));
        assert!(expanded.contains("foreign_key : Some (\"player_id\""));
    }

    #[test]
    fn expand_read_model_accepts_through_before_many_to_many() {
        let input: DeriveInput = syn::parse_quote! {
            #[readmodel(table = "players")]
            struct Player {
                #[readmodel(id, column = "player_id")]
                id: String,
                #[readmodel(through = "player_weapon_links", foreign_key = "player_id", many_to_many = "Weapon")]
                weapons: Vec<Weapon>,
            }
        };

        let expanded = expand_read_model(input).unwrap().to_string();

        assert!(expanded.contains("RelationshipKind :: ManyToMany"));
        assert!(expanded.contains("through : Some (\"player_weapon_links\""));
        assert!(expanded.contains("foreign_key : Some (\"player_id\""));
    }

    #[test]
    fn expand_read_model_rejects_tuple_structs() {
        let input: DeriveInput = syn::parse_quote! {
            struct CounterView(String);
        };

        let err = expand_read_model(input).expect_err("tuple struct should return an error");

        assert!(
            err.to_string().contains("requires named fields"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn expand_read_model_rejects_relationships_without_foreign_key() {
        let input: DeriveInput = syn::parse_quote! {
            #[readmodel(table = "players")]
            struct Player {
                #[readmodel(id)]
                player_id: String,
                #[readmodel(has_many = "PlayerWeapon")]
                weapons: Vec<PlayerWeapon>,
            }
        };

        let err = expand_read_model(input).expect_err("missing relationship key should fail");

        assert!(
            err.to_string().contains("foreign_key"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn expand_read_model_rejects_duplicate_relationship_foreign_keys() {
        let input: DeriveInput = syn::parse_quote! {
            #[readmodel(table = "players")]
            struct Player {
                #[readmodel(id)]
                player_id: String,
                #[readmodel(has_many = "PlayerWeapon", foreign_key = "player_id", foreign_key = "owner_id")]
                weapons: Vec<PlayerWeapon>,
            }
        };

        let err = expand_read_model(input).expect_err("duplicate relationship foreign key");

        assert!(
            err.to_string()
                .contains("foreign_key declared more than once"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn expand_read_model_rejects_duplicate_pending_relationship_through_attrs() {
        let input: DeriveInput = syn::parse_quote! {
            #[readmodel(table = "players")]
            struct Player {
                #[readmodel(id)]
                player_id: String,
                #[readmodel(through = "player_weapon_links", through = "weapon_players", many_to_many = "Weapon")]
                weapons: Vec<Weapon>,
            }
        };

        let err = expand_read_model(input).expect_err("duplicate pending relationship through");

        assert!(
            err.to_string().contains("through declared more than once"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn snake_case_preserves_multi_char_lowercase_mapping() {
        assert_eq!(to_snake_case("İdView"), "i\u{307}d_view");
    }
}
