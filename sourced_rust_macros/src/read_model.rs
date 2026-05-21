use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, LitStr};

pub fn derive_read_model(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_read_model(input) {
        Ok(expanded) => TokenStream::from(expanded),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_read_model(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;

    // Extract #[readmodel(collection = "...")] from struct-level attributes
    let collection = extract_collection(&input);

    // Extract the field marked with #[readmodel(id)] or default to "id"
    let id_field = extract_id_field(&input)?;

    Ok(quote! {
        impl sourced_rust::ReadModel for #name {
            const COLLECTION: &'static str = #collection;

            fn id(&self) -> &str {
                &self.#id_field
            }
        }
    })
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

fn extract_id_field(input: &DeriveInput) -> syn::Result<syn::Ident> {
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

    for field in &fields.named {
        for attr in &field.attrs {
            if attr.path().is_ident("readmodel") {
                let mut is_id = false;
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("id") {
                        is_id = true;
                    }
                    Ok(())
                })?;
                if is_id {
                    return field.ident.clone().ok_or_else(|| {
                        syn::Error::new_spanned(field, "ReadModel id field must be named")
                    });
                }
            }
        }
    }

    // Default: look for a field named "id"
    for field in &fields.named {
        if let Some(ident) = &field.ident {
            if ident == "id" {
                return Ok(ident.clone());
            }
        }
    }

    Err(syn::Error::new_spanned(
        input,
        "ReadModel derive requires a field named `id` or a field marked with #[readmodel(id)]",
    ))
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

        assert!(expanded.contains("impl sourced_rust :: ReadModel for CounterView"));
        assert!(expanded.contains("fn id"));
    }

    #[test]
    fn expand_read_model_rejects_missing_id_field() {
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
    fn snake_case_preserves_multi_char_lowercase_mapping() {
        assert_eq!(to_snake_case("İdView"), "i\u{307}d_view");
    }
}
