//! GraphqlInput / GraphqlOutput derive macros.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, GenericArgument, PathArguments, Type};

pub fn expand_graphql_input(input: DeriveInput) -> syn::Result<TokenStream> {
    expand(
        input,
        quote! { distributed::graphql::GraphqlInputType },
        quote! { distributed::graphql::GraphqlInputType },
    )
}

pub fn expand_graphql_output(input: DeriveInput) -> syn::Result<TokenStream> {
    expand(
        input,
        quote! { distributed::graphql::GraphqlOutputType },
        quote! { distributed::graphql::GraphqlOutputType },
    )
}

fn expand(
    input: DeriveInput,
    trait_path: TokenStream,
    nested_trait: TokenStream,
) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            &input,
            "GraphqlInput/GraphqlOutput only support structs with named fields",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &input,
            "GraphqlInput/GraphqlOutput require named fields",
        ));
    };

    let mut field_tokens = Vec::new();
    for field in &fields.named {
        let field_name = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new_spanned(field, "field must be named"))?;
        let field_name_str = field_name.to_string();
        let (type_name, nullable, list, nested) =
            map_type(&field.ty, field, &nested_trait)?;
        let nested_tokens = match nested {
            Some(tokens) => quote! { Some(::std::boxed::Box::new(#tokens)) },
            None => quote! { None },
        };
        field_tokens.push(quote! {
            distributed::graphql::GraphqlTypeField {
                name: #field_name_str.to_string(),
                type_name: #type_name.to_string(),
                nullable: #nullable,
                list: #list,
                nested: #nested_tokens,
            }
        });
    }

    let type_name_str = name.to_string();
    Ok(quote! {
        impl #trait_path for #name {
            fn graphql_type() -> distributed::graphql::GraphqlTypeDef {
                distributed::graphql::GraphqlTypeDef::new(
                    #type_name_str,
                    vec![#(#field_tokens),*],
                ).with_type_id(::std::any::TypeId::of::<#name>())
            }
        }
    })
}

fn map_type(
    ty: &Type,
    span: &syn::Field,
    nested_trait: &TokenStream,
) -> syn::Result<(String, bool, bool, Option<TokenStream>)> {
    if let Some(inner) = extract_path_arg(ty, "Option") {
        let (name, _, list, nested) = map_type(inner, span, nested_trait)?;
        return Ok((name, true, list, nested));
    }
    if let Some(inner) = extract_path_arg(ty, "Vec") {
        let (name, nullable, _, nested) = map_type(inner, span, nested_trait)?;
        return Ok((name, nullable, true, nested));
    }

    let path = match ty {
        Type::Path(p) => p,
        _ => {
            return Err(syn::Error::new_spanned(
                span,
                "unsupported field type for GraphqlInput/GraphqlOutput",
            ));
        }
    };
    let last = path
        .path
        .segments
        .last()
        .ok_or_else(|| syn::Error::new_spanned(span, "empty type path"))?;
    let ident = last.ident.to_string();

    let scalar = match ident.as_str() {
        "String" | "str" => Some("String"),
        "bool" => Some("Boolean"),
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" | "isize" | "usize" => {
            Some("BigInt")
        }
        "f32" | "f64" => Some("Float"),
        "Value" => Some("JSON"),
        _ => None,
    };

    if let Some(s) = scalar {
        return Ok((s.to_string(), false, false, None));
    }

    let nested = quote! { <#ty as #nested_trait>::graphql_type() };
    Ok((ident, false, false, Some(nested)))
}

fn extract_path_arg<'a>(ty: &'a Type, wrapper: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let seg = path.path.segments.last()?;
    if seg.ident != wrapper {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}
