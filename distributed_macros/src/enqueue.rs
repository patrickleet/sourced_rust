use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse::Parser, Expr, Ident, ItemFn, LitStr, Token};

use crate::shared::{
    ensure_sourced_result_signature, extract_params_with_types, wrap_result_body_with_guard,
};

pub(crate) fn expand_enqueue(attr: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
    let args = parse_enqueue_args.parse2(attr)?;
    let mut func = syn::parse2::<ItemFn>(item)?;

    let signature_synthesized = ensure_sourced_result_signature(&mut func.sig, "enqueue")?;

    let emitter_field = &args.emitter_field;
    let event_name = &args.event_name;

    // Use function parameters - serialize as tuple to JSON
    let params = extract_params_with_types(&func.sig, "enqueue")?;
    let param_names: Vec<&Ident> = params.iter().map(|(name, _)| name).collect();

    let entity_field = &args.entity_field;

    let enqueue_call = if param_names.is_empty() {
        quote! {
            if !self.#entity_field.is_replaying() {
                self.#emitter_field.enqueue(#event_name, "");
            };
        }
    } else if param_names.len() == 1 {
        // Single-element tuple needs trailing comma: (x,) not (x)
        let param = &param_names[0];
        quote! {
            if !self.#entity_field.is_replaying() {
                self.#emitter_field.enqueue_with(#event_name, &(#param.clone(),))?;
            };
        }
    } else {
        // Multi-element tuple
        quote! {
            if !self.#entity_field.is_replaying() {
                self.#emitter_field.enqueue_with(#event_name, &(#(#param_names.clone()),*))?;
            };
        }
    };

    let new_body = wrap_result_body_with_guard(
        args.guard.as_ref(),
        enqueue_call,
        &func.block,
        signature_synthesized,
    );
    *func.block = new_body;

    Ok(quote! { #func })
}

pub(crate) struct EnqueueArgs {
    pub(crate) emitter_field: syn::Ident,
    pub(crate) entity_field: syn::Ident,
    event_name: LitStr,
    guard: Option<Expr>,
}

pub(crate) fn parse_enqueue_args(input: syn::parse::ParseStream) -> syn::Result<EnqueueArgs> {
    // Check if first token is an identifier (potential emitter field) or a string literal (event name)
    let (emitter_field, event_name) = if input.peek(LitStr) {
        // No emitter field specified, use default "emitter"
        let event_name: LitStr = input.parse()?;
        (format_ident!("emitter"), event_name)
    } else {
        // First token is an identifier - emitter field name, event name follows
        let first_ident: syn::Ident = input.parse()?;
        input.parse::<Token![,]>()?;
        let event_name: LitStr = input.parse()?;
        (first_ident, event_name)
    };

    let mut guard = None;
    // Defaults to the conventional `entity` field; overridable so a renamed
    // entity field still produces a correct `is_replaying()` guard.
    let mut entity_field = format_ident!("entity");

    // Parse optional keyword arguments: `when = condition`, `entity = field`
    while input.peek(Token![,]) {
        input.parse::<Token![,]>()?;
        // Allow (and ignore) a trailing comma.
        if input.is_empty() {
            break;
        }
        let ident: syn::Ident = input.parse()?;
        if ident == "when" {
            input.parse::<Token![=]>()?;
            guard = Some(input.parse()?);
        } else if ident == "entity" {
            input.parse::<Token![=]>()?;
            entity_field = input.parse()?;
        } else {
            return Err(syn::Error::new_spanned(
                &ident,
                format!(
                    "unsupported key `{ident}` in #[enqueue(...)]; expected `when` or `entity`"
                ),
            ));
        }
    }

    Ok(EnqueueArgs {
        emitter_field,
        entity_field,
        event_name,
        guard,
    })
}
