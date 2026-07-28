use quote::{quote, ToTokens};
use sha2::{Digest, Sha256};
use syn::{Attribute, Expr, FnArg, Ident, LitStr, Pat, ReturnType, Type};

// Shared helpers
// ============================================================================

/// Extract parameter names and types from a method signature (excludes `self`).
///
/// Every parameter must be a plain identifier: its name is recorded in the
/// event payload and used to call the method again on replay. A pattern like
/// `(a, b): (u8, u8)` or `_: String` has no single name, so silently skipping
/// it would drop the parameter from the payload and make the generated replay
/// arm call the method with too few arguments — an arity error pointing at
/// generated code, far from the cause. Reject it here with a spanned error.
pub(crate) fn extract_params_with_types(
    sig: &syn::Signature,
    attr_name: &str,
) -> syn::Result<Vec<(Ident, syn::Type)>> {
    sig.inputs
        .iter()
        .filter_map(|arg| match arg {
            FnArg::Typed(pat_type) => Some(pat_type),
            FnArg::Receiver(_) => None,
        })
        .map(|pat_type| match &*pat_type.pat {
            Pat::Ident(pat_ident) => Ok((pat_ident.ident.clone(), (*pat_type.ty).clone())),
            other => Err(syn::Error::new_spanned(
                other,
                format!(
                    "unsupported parameter pattern in #[{attr_name}] method — use a plain identifier"
                ),
            )),
        })
        .collect()
}

fn returns_result(sig: &syn::Signature) -> bool {
    match &sig.output {
        ReturnType::Default => false,
        ReturnType::Type(_, ty) => match ty.as_ref() {
            Type::Path(path) => path.path.segments.last().is_some_and(|segment| {
                segment.ident == "Result" || segment.ident == "SourcedResult"
            }),
            _ => false,
        },
    }
}

pub(crate) fn ensure_sourced_result_signature(
    sig: &mut syn::Signature,
    attr_name: &str,
) -> Result<bool, syn::Error> {
    match &sig.output {
        ReturnType::Default => {
            sig.output = syn::parse_quote!(-> distributed::SourcedResult<()>);
            Ok(true)
        }
        ReturnType::Type(_, _) if returns_result(sig) => Ok(false),
        ReturnType::Type(_, ty) => Err(syn::Error::new_spanned(
            ty,
            format!(
                "#[{}] methods must return Result<(), E>, SourcedResult, or omit the return type",
                attr_name
            ),
        )),
    }
}

/// Generate a digest call token stream.
pub(crate) fn generate_digest_call(
    entity_field: &Ident,
    event_name: &LitStr,
    param_names: &[&Ident],
    version: Option<&syn::LitInt>,
) -> proc_macro2::TokenStream {
    match version {
        Some(ver) => {
            if param_names.is_empty() {
                quote! { self.#entity_field.digest_v(#event_name, #ver, &())?; }
            } else if param_names.len() == 1 {
                let param = param_names[0];
                quote! { self.#entity_field.digest_v(#event_name, #ver, &(#param.clone(),))?; }
            } else {
                quote! { self.#entity_field.digest_v(#event_name, #ver, &(#(#param_names.clone()),*))?; }
            }
        }
        None => {
            if param_names.is_empty() {
                quote! { self.#entity_field.digest_empty(#event_name)?; }
            } else if param_names.len() == 1 {
                let param = param_names[0];
                quote! { self.#entity_field.digest(#event_name, &(#param.clone(),))?; }
            } else {
                quote! { self.#entity_field.digest(#event_name, &(#(#param_names.clone()),*))?; }
            }
        }
    }
}

/// Wrap a `Result<(), E>` command method with an optional guard and fallible prelude.
pub(crate) fn wrap_result_body_with_guard(
    guard: Option<&Expr>,
    prepend: proc_macro2::TokenStream,
    original_block: &syn::Block,
    signature_synthesized: bool,
) -> syn::Block {
    wrap_result_body_with_guard_and_postlude(
        guard,
        prepend,
        original_block,
        proc_macro2::TokenStream::new(),
        signature_synthesized,
    )
}

/// Wrap a sourced method with an optional guard and successful postlude.
pub(crate) fn wrap_result_body_with_guard_and_postlude(
    guard: Option<&Expr>,
    prepend: proc_macro2::TokenStream,
    original_block: &syn::Block,
    append: proc_macro2::TokenStream,
    signature_synthesized: bool,
) -> syn::Block {
    match (guard, signature_synthesized) {
        (Some(guard), true) => {
            syn::parse_quote! {
                {
                    if #guard {
                        #prepend
                        #original_block;
                        #append
                    }
                    Ok(())
                }
            }
        }
        (Some(guard), false) => {
            syn::parse_quote! {
                {
                    if #guard {
                        #prepend
                        (|| #original_block)()?;
                        #append
                    }
                    Ok(())
                }
            }
        }
        (None, true) => {
            syn::parse_quote! {
                {
                    #prepend
                    #original_block;
                    #append
                    Ok(())
                }
            }
        }
        (None, false) => {
            syn::parse_quote! {
                {
                    #prepend
                    (|| #original_block)()?;
                    #append
                    Ok(())
                }
            }
        }
    }
}

/// Generate an enqueue call token stream (for use within `#[sourced]`).
pub(crate) fn generate_enqueue_call(
    entity_field: &Ident,
    emitter_field: &Ident,
    event_name: &LitStr,
    param_names: &[&Ident],
) -> proc_macro2::TokenStream {
    let enqueue_expr = if param_names.is_empty() {
        quote! { self.#emitter_field.enqueue(#event_name, ""); }
    } else if param_names.len() == 1 {
        let param = param_names[0];
        quote! { self.#emitter_field.enqueue_with(#event_name, &(#param.clone(),))?; }
    } else {
        quote! { self.#emitter_field.enqueue_with(#event_name, &(#(#param_names.clone()),*))?; }
    };
    quote! {
        if !self.#entity_field.is_replaying() {
            #enqueue_expr
        };
    }
}

/// Build a deterministic schema string for one named JSON object.
pub(crate) fn canonical_object_schema(
    role: &str,
    type_name: &str,
    version: u64,
    container_attrs: &[Attribute],
    fields: impl IntoIterator<Item = (String, Type, Vec<Attribute>)>,
) -> String {
    let serde_container = canonical_serde_attrs(container_attrs);
    let fields = fields
        .into_iter()
        .map(|(name, ty, attrs)| {
            let ty = compact_tokens(&ty);
            let serde = canonical_serde_attrs(&attrs);
            format!(
                "{}:{}|{}:{}|{}:{}",
                name.len(),
                name,
                ty.len(),
                ty,
                serde.len(),
                serde
            )
        })
        .collect::<Vec<_>>();

    format!(
        "distributed.schema/v1|role={}:{}|type={}:{}|version={version}|serde={}:{}|fields={}:{}",
        role.len(),
        role,
        type_name.len(),
        type_name,
        serde_container.len(),
        serde_container,
        fields.len(),
        fields.join("|")
    )
}

fn canonical_serde_attrs(attrs: &[Attribute]) -> String {
    attrs
        .iter()
        .filter(|attr| attr.path().is_ident("serde"))
        .map(compact_tokens)
        .collect::<Vec<_>>()
        .join(",")
}

fn compact_tokens(tokens: &impl ToTokens) -> String {
    tokens
        .to_token_stream()
        .to_string()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

/// Return a lowercase SHA-256 descriptor fingerprint.
pub(crate) fn schema_fingerprint(schema: &str) -> String {
    let digest = Sha256::digest(schema.as_bytes());
    format!("sha256:{digest:x}")
}

pub(crate) fn validate_domain_event_name_literal(name: &LitStr) -> syn::Result<()> {
    const MAX_MESSAGE_NAME_BYTES: usize = 256;

    let value = name.value();
    if value.trim().is_empty() {
        return Err(syn::Error::new(
            name.span(),
            "domain-event name must not be empty",
        ));
    }
    if value.len() > MAX_MESSAGE_NAME_BYTES {
        return Err(syn::Error::new(
            name.span(),
            format!(
                "domain-event name is {} bytes, exceeding the maximum of {MAX_MESSAGE_NAME_BYTES}",
                value.len()
            ),
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(syn::Error::new(
            name.span(),
            "domain-event name must not contain control characters",
        ));
    }
    if let Some(wildcard) = value
        .chars()
        .find(|character| matches!(character, '*' | '#' | '>'))
    {
        return Err(syn::Error::new(
            name.span(),
            format!(
                "domain-event name contains broker wildcard `{wildcard}`; `*`, `#`, and `>` are reserved routing operators"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod schema_tests {
    use super::{schema_fingerprint, validate_domain_event_name_literal};
    use syn::LitStr;

    #[test]
    fn schema_fingerprint_matches_sha256_reference_vector() {
        assert_eq!(
            schema_fingerprint("abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn domain_event_name_validation_matches_runtime_boundaries() {
        let boundary = LitStr::new(&"a".repeat(256), proc_macro2::Span::call_site());
        let over = LitStr::new(&"a".repeat(257), proc_macro2::Span::call_site());

        assert!(validate_domain_event_name_literal(&boundary).is_ok());
        assert!(validate_domain_event_name_literal(&over).is_err());
    }
}
