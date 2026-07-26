use quote::quote;
use syn::{Expr, FnArg, Ident, LitStr, Pat, ReturnType, Type};

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
    match (guard, signature_synthesized) {
        (Some(guard), true) => {
            syn::parse_quote! {
                {
                    if #guard {
                        #prepend
                        #original_block;
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
                    Ok(())
                }
            }
        }
        (None, false) => {
            syn::parse_quote! {
                {
                    #prepend
                    (|| #original_block)()?;
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
