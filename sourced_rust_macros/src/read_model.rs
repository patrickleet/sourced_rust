use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, LitStr};

pub fn derive_read_model(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    // Extract #[readmodel(collection = "...")] from struct-level attributes
    let collection = extract_collection(&input);

    // Extract the field marked with #[readmodel(id)] or default to "id"
    let id_field = extract_id_field(&input);

    let expanded = quote! {
        impl sourced_rust::ReadModel for #name {
            const COLLECTION: &'static str = #collection;

            fn id(&self) -> &str {
                &self.#id_field
            }
        }
    };

    TokenStream::from(expanded)
}

fn extract_collection(input: &DeriveInput) -> String {
    for attr in &input.attrs {
        if !attr.path().is_ident("readmodel") {
            continue;
        }

        let mut collection = None;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("collection") {
                let value: LitStr = meta.value()?.parse()?;
                collection = Some(value.value());
            }
            Ok(())
        });

        if let Some(c) = collection {
            return c;
        }
    }

    // Default: snake_case struct name + "s"
    let name = input.ident.to_string();
    format!("{}s", to_snake_case(&name))
}

fn extract_id_field(input: &DeriveInput) -> syn::Ident {
    if let Data::Struct(data_struct) = &input.data {
        if let Fields::Named(fields) = &data_struct.fields {
            for field in &fields.named {
                for attr in &field.attrs {
                    if attr.path().is_ident("readmodel") {
                        let mut is_id = false;
                        let _ = attr.parse_nested_meta(|meta| {
                            if meta.path.is_ident("id") {
                                is_id = true;
                            }
                            Ok(())
                        });
                        if is_id {
                            return field.ident.clone().unwrap();
                        }
                    }
                }
            }

            // Default: look for a field named "id"
            for field in &fields.named {
                if let Some(ident) = &field.ident {
                    if ident == "id" {
                        return ident.clone();
                    }
                }
            }
        }
    }

    panic!("ReadModel derive: no field marked with #[readmodel(id)] and no field named `id`");
}

fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(ch.to_lowercase().next().unwrap());
        } else {
            result.push(ch);
        }
    }
    result
}
