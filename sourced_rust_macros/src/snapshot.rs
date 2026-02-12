use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, LitStr};

pub fn derive_snapshot(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let snapshot_name = format_ident!("{}Snapshot", name);

    // Parse struct-level #[snapshot(...)] attributes
    let (entity_field_name, custom_id) = parse_struct_attrs(&input);
    let entity_field = format_ident!("{}", entity_field_name);

    // Collect eligible fields (exclude entity field and #[serde(skip)] fields)
    let fields = match &input.data {
        Data::Struct(data_struct) => match &data_struct.fields {
            Fields::Named(fields) => &fields.named,
            _ => panic!("Snapshot derive only supports structs with named fields"),
        },
        _ => panic!("Snapshot derive only supports structs"),
    };

    let mut snapshot_fields = Vec::new();
    for field in fields {
        let ident = field.ident.as_ref().unwrap();

        // Skip the entity field
        if *ident == entity_field {
            continue;
        }

        // Skip fields with #[serde(skip)]
        if has_serde_skip(&field.attrs) {
            continue;
        }

        snapshot_fields.push((ident.clone(), field.ty.clone()));
    }

    // Determine if custom_id matches an existing field
    let has_custom_id = custom_id.is_some();
    let id_field_name = custom_id
        .map(|s| format_ident!("{}", s))
        .unwrap_or_else(|| format_ident!("id"));

    // Check if id_field_name already exists in snapshot_fields
    let id_already_in_fields = snapshot_fields.iter().any(|(n, _)| *n == id_field_name);

    // Build the snapshot struct fields
    let struct_field_defs: Vec<_> = if has_custom_id || id_already_in_fields {
        // Custom ID key or "id" already present - just use existing fields
        snapshot_fields
            .iter()
            .map(|(n, ty)| quote! { pub #n: #ty })
            .collect()
    } else {
        // Default case: prepend `pub id: String`
        let mut defs = vec![quote! { pub id: String }];
        defs.extend(
            snapshot_fields
                .iter()
                .map(|(n, ty)| quote! { pub #n: #ty }),
        );
        defs
    };

    // Build snapshot() field assignments
    let field_assignments: Vec<_> = if has_custom_id || id_already_in_fields {
        // All fields just clone
        snapshot_fields
            .iter()
            .map(|(n, _)| quote! { #n: self.#n.clone() })
            .collect()
    } else {
        // Default: id from entity + clone all fields
        let mut assignments = vec![quote! { id: self.#entity_field.id().to_string() }];
        assignments.extend(
            snapshot_fields
                .iter()
                .map(|(n, _)| quote! { #n: self.#n.clone() }),
        );
        assignments
    };

    // Build restore_from_snapshot field assignments
    let restore_assignments: Vec<_> = snapshot_fields
        .iter()
        .map(|(n, _)| quote! { self.#n = snapshot.#n; })
        .collect();

    let expanded = quote! {
        #[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
        pub struct #snapshot_name {
            #(#struct_field_defs),*
        }

        impl #name {
            pub fn snapshot(&self) -> #snapshot_name {
                #snapshot_name {
                    #(#field_assignments),*
                }
            }
        }

        impl sourced_rust::Snapshottable for #name {
            type Snapshot = #snapshot_name;

            fn create_snapshot(&self) -> #snapshot_name {
                self.snapshot()
            }

            fn restore_from_snapshot(&mut self, snapshot: #snapshot_name) {
                let __id_value = snapshot.#id_field_name.clone();
                #(#restore_assignments)*
                self.#entity_field.set_id(&__id_value);
            }
        }
    };

    TokenStream::from(expanded)
}

fn parse_struct_attrs(input: &DeriveInput) -> (String, Option<String>) {
    let mut entity_field = "entity".to_string();
    let mut custom_id: Option<String> = None;

    for attr in &input.attrs {
        if !attr.path().is_ident("snapshot") {
            continue;
        }

        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("entity") {
                let value: LitStr = meta.value()?.parse()?;
                entity_field = value.value();
            } else if meta.path.is_ident("id") {
                let value: LitStr = meta.value()?.parse()?;
                custom_id = Some(value.value());
            }
            Ok(())
        });
    }

    (entity_field, custom_id)
}

fn has_serde_skip(attrs: &[syn::Attribute]) -> bool {
    for attr in attrs {
        if !attr.path().is_ident("serde") {
            continue;
        }

        let mut found_skip = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                found_skip = true;
            }
            Ok(())
        });

        if found_skip {
            return true;
        }
    }
    false
}
