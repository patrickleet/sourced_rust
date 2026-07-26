use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    parse::{ParseStream, Parser},
    Expr, FnArg, Ident, ItemImpl, LitStr, Token,
};

use crate::aggregate::{
    aggregate_impl_tokens, event_variant_ident, generate_upcaster_tokens, parse_upcaster_list,
    validate_aggregate_type_literal, UpcasterDef,
};
use crate::shared::{
    ensure_sourced_result_signature, extract_params_with_types, generate_digest_call,
    generate_enqueue_call, wrap_result_body_with_guard,
};

pub(crate) struct SourcedArgs {
    entity_field: Ident,
    enum_name: Option<LitStr>,
    aggregate_type: Option<LitStr>,
    enqueue: Option<Ident>, // Some(emitter_field) if enqueue enabled
    pub(crate) upcasters: Vec<UpcasterDef>,
}

pub(crate) fn parse_sourced_args(input: ParseStream) -> syn::Result<SourcedArgs> {
    if input.is_empty() {
        return Err(
            input.error("#[sourced] requires the entity field name, e.g. `#[sourced(entity)]`")
        );
    }
    let entity_field: Ident = input.parse()?;
    let mut enum_name = None;
    let mut aggregate_type = None;
    let mut enqueue = None;
    let mut upcasters = Vec::new();

    while input.peek(Token![,]) {
        input.parse::<Token![,]>()?;
        // Allow (and ignore) a trailing comma.
        if input.is_empty() {
            break;
        }
        let kw: Ident = input.parse()?;
        if kw == "events" {
            input.parse::<Token![=]>()?;
            enum_name = Some(input.parse::<LitStr>()?);
        } else if kw == "aggregate_type" {
            input.parse::<Token![=]>()?;
            let lit = input.parse::<LitStr>()?;
            validate_aggregate_type_literal(&lit)?;
            aggregate_type = Some(lit);
        } else if kw == "enqueue" {
            // Optional custom emitter field: enqueue(my_emitter)
            if input.peek(syn::token::Paren) {
                let inner;
                syn::parenthesized!(inner in input);
                enqueue = Some(inner.parse::<Ident>()?);
            } else {
                enqueue = Some(format_ident!("emitter"));
            }
        } else if kw == "upcasters" {
            let upcaster_content;
            syn::parenthesized!(upcaster_content in input);
            upcasters = parse_upcaster_list(&upcaster_content)?;
        } else {
            return Err(syn::Error::new_spanned(
                &kw,
                format!(
                    "unsupported key `{kw}` in #[sourced(...)]; expected `events`, `aggregate_type`, `enqueue`, or `upcasters`"
                ),
            ));
        }
    }

    Ok(SourcedArgs {
        entity_field,
        enum_name,
        aggregate_type,
        enqueue,
        upcasters,
    })
}

pub(crate) struct EventAttr {
    event_name: LitStr,
    guard: Option<Expr>,
    version: Option<syn::LitInt>,
}

pub(crate) fn parse_event_args(input: ParseStream) -> syn::Result<EventAttr> {
    let event_name: LitStr = input.parse()?;
    let mut guard = None;
    let mut version = None;

    while input.peek(Token![,]) {
        input.parse::<Token![,]>()?;
        // Allow (and ignore) a trailing comma.
        if input.is_empty() {
            break;
        }
        let ident: Ident = input.parse()?;
        if ident == "when" {
            input.parse::<Token![=]>()?;
            guard = Some(input.parse()?);
        } else if ident == "version" {
            input.parse::<Token![=]>()?;
            version = Some(input.parse()?);
        } else {
            return Err(syn::Error::new_spanned(
                &ident,
                format!("unsupported key `{ident}` in #[event(...)]; expected `when` or `version`"),
            ));
        }
    }

    Ok(EventAttr {
        event_name,
        guard,
        version,
    })
}

fn find_and_remove_event_attr(
    attrs: &mut Vec<syn::Attribute>,
) -> Result<Option<EventAttr>, syn::Error> {
    let idx = attrs.iter().position(|a| a.path().is_ident("event"));
    match idx {
        Some(idx) => {
            let attr = attrs.remove(idx);
            let event_attr = attr.parse_args_with(parse_event_args)?;
            Ok(Some(event_attr))
        }
        None => Ok(None),
    }
}

struct EventMethodInfo {
    event_name: LitStr,
    method_name: Ident,
    params: Vec<(Ident, syn::Type)>,
}

pub(crate) fn expand_sourced(attr: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
    let args = parse_sourced_args.parse2(attr)?;
    let mut impl_block = syn::parse2::<ItemImpl>(item)?;

    // Extract struct name from self type
    let struct_name = match &*impl_block.self_ty {
        syn::Type::Path(type_path) => match type_path.path.segments.last() {
            Some(segment) => segment.ident.clone(),
            None => {
                return Err(syn::Error::new_spanned(
                    &impl_block.self_ty,
                    "#[sourced] requires a named type",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &impl_block.self_ty,
                "#[sourced] requires a named type",
            ));
        }
    };

    // Collect event info and modify methods
    let mut event_methods: Vec<EventMethodInfo> = Vec::new();
    // Detect duplicate event names so the conflict points at the offending
    // attribute instead of surfacing as a confusing duplicate match arm later.
    let mut seen_events: std::collections::HashSet<String> = std::collections::HashSet::new();
    // Distinct event names can still derive the same enum variant because only
    // the last `.`-segment is PascalCased (`user.completed` and
    // `admin.completed` both become `Completed`). Track the derived idents so
    // the collision is reported here, naming both event strings, instead of as
    // a duplicate-variant error inside the generated enum.
    let mut seen_variants: std::collections::HashMap<String, LitStr> =
        std::collections::HashMap::new();

    for item in &mut impl_block.items {
        if let syn::ImplItem::Fn(method) = item {
            match find_and_remove_event_attr(&mut method.attrs) {
                Ok(Some(event_attr)) => {
                    // Event methods are replayed as `self.method(...)`, so they
                    // must take a `self` receiver. Reject free associated
                    // functions up front with a pointed message.
                    if !matches!(method.sig.inputs.first(), Some(FnArg::Receiver(_))) {
                        return Err(syn::Error::new_spanned(
                            &method.sig,
                            "#[event] methods must take a `&mut self` receiver",
                        ));
                    }

                    let event_key = event_attr.event_name.value();
                    if !seen_events.insert(event_key.clone()) {
                        return Err(syn::Error::new_spanned(
                            &event_attr.event_name,
                            format!("duplicate #[event] name `{event_key}` in this #[sourced] impl block"),
                        ));
                    }

                    let variant = event_variant_ident(&event_attr.event_name);
                    if let Some(prev) = seen_variants.get(&variant.to_string()) {
                        return Err(syn::Error::new_spanned(
                            &event_attr.event_name,
                            format!(
                                "#[event] names `{}` and `{event_key}` both derive the enum variant `{variant}`; rename one so the variant names are distinct",
                                prev.value()
                            ),
                        ));
                    }
                    seen_variants.insert(variant.to_string(), event_attr.event_name.clone());

                    let signature_synthesized =
                        ensure_sourced_result_signature(&mut method.sig, "event")?;

                    let params = extract_params_with_types(&method.sig, "event")?;
                    let param_name_refs: Vec<&Ident> =
                        params.iter().map(|(name, _)| name).collect();

                    // Build prepend: optional enqueue + digest
                    let enqueue_call = args.enqueue.as_ref().map(|emitter_field| {
                        generate_enqueue_call(
                            &args.entity_field,
                            emitter_field,
                            &event_attr.event_name,
                            &param_name_refs,
                        )
                    });
                    let digest_call = generate_digest_call(
                        &args.entity_field,
                        &event_attr.event_name,
                        &param_name_refs,
                        event_attr.version.as_ref(),
                    );
                    let prepend = quote! {
                        #enqueue_call
                        #digest_call
                    };

                    let new_body = wrap_result_body_with_guard(
                        event_attr.guard.as_ref(),
                        prepend,
                        &method.block,
                        signature_synthesized,
                    );
                    method.block = new_body;

                    event_methods.push(EventMethodInfo {
                        event_name: event_attr.event_name,
                        method_name: method.sig.ident.clone(),
                        params,
                    });
                }
                Ok(None) => { /* not an event method, skip */ }
                Err(err) => return Err(err),
            }
        }
    }

    // Determine enum name
    let enum_name = if let Some(ref custom) = args.enum_name {
        format_ident!("{}", custom.value())
    } else {
        format_ident!("{}Event", struct_name)
    };

    // Generate event enum
    let enum_variants = event_methods.iter().map(|e| {
        let variant_name = event_variant_ident(&e.event_name);
        if e.params.is_empty() {
            quote! { #variant_name }
        } else {
            let fields = e.params.iter().map(|(name, ty)| quote! { #name: #ty });
            quote! { #variant_name { #(#fields),* } }
        }
    });

    let enum_def = quote! {
        #[allow(clippy::enum_variant_names)]
        #[derive(Debug, Clone, PartialEq)]
        pub enum #enum_name {
            #(#enum_variants),*
        }
    };

    // Generate event_name() method on the enum
    let event_name_arms = event_methods.iter().map(|e| {
        let variant_name = event_variant_ident(&e.event_name);
        let name_str = &e.event_name;
        if e.params.is_empty() {
            quote! { #enum_name::#variant_name => #name_str }
        } else {
            quote! { #enum_name::#variant_name { .. } => #name_str }
        }
    });

    let event_name_impl = quote! {
        impl #enum_name {
            pub fn event_name(&self) -> &'static str {
                match self {
                    #(#event_name_arms),*
                }
            }
        }
    };

    // Generate TryFrom<&EventRecord>
    let try_from_arms = event_methods.iter().map(|e| {
        let variant_name = event_variant_ident(&e.event_name);
        let event_name_str = &e.event_name;
        if e.params.is_empty() {
            quote! {
                #event_name_str => Ok(#enum_name::#variant_name),
            }
        } else if e.params.len() == 1 {
            let (name, _) = &e.params[0];
            quote! {
                #event_name_str => {
                    let (#name,) = event.decode().map_err(|e| e.to_string())?;
                    Ok(#enum_name::#variant_name { #name })
                }
            }
        } else {
            let names: Vec<_> = e.params.iter().map(|(n, _)| n).collect();
            quote! {
                #event_name_str => {
                    let (#(#names),*) = event.decode().map_err(|e| e.to_string())?;
                    Ok(#enum_name::#variant_name { #(#names),* })
                }
            }
        }
    });

    let try_from_impl = quote! {
        impl TryFrom<&distributed::EventRecord> for #enum_name {
            type Error = String;
            fn try_from(event: &distributed::EventRecord) -> Result<Self, Self::Error> {
                match event.event_name.as_str() {
                    #(#try_from_arms)*
                    _ => Err(format!("Unknown event: {}", event.event_name)),
                }
            }
        }
    };

    // Generate impl Aggregate
    let entity_field = &args.entity_field;
    let replay_arms: Vec<_> = event_methods
        .iter()
        .map(|e| {
            let event_name_str = &e.event_name;
            let method_name = &e.method_name;
            if e.params.is_empty() {
                quote! {
                    #event_name_str => {
                        self.#method_name().map_err(|e| e.to_string())?;
                    }
                }
            } else if e.params.len() == 1 {
                let (name, _) = &e.params[0];
                quote! {
                    #event_name_str => {
                        let (#name,) = event.decode().map_err(|e| e.to_string())?;
                        self.#method_name(#name).map_err(|e| e.to_string())?;
                    }
                }
            } else {
                let names: Vec<_> = e.params.iter().map(|(n, _)| n).collect();
                quote! {
                    #event_name_str => {
                        let (#(#names),*) = event.decode().map_err(|e| e.to_string())?;
                        self.#method_name(#(#names),*).map_err(|e| e.to_string())?;
                    }
                }
            }
        })
        .collect();

    let (upcaster_wrappers, upcasters_method) =
        generate_upcaster_tokens(&struct_name, &args.upcasters);
    let aggregate_type_method = args.aggregate_type.as_ref().map(|aggregate_type| {
        quote! {
            fn aggregate_type() -> &'static str {
                #aggregate_type
            }
        }
    });

    // ReplayError stays `String` — see the rationale on `expand_aggregate`.
    let aggregate_impl = aggregate_impl_tokens(
        &struct_name,
        entity_field,
        &aggregate_type_method,
        &replay_arms,
        &upcasters_method,
    );

    let expanded = quote! {
        #impl_block
        #enum_def
        #event_name_impl
        #try_from_impl
        #upcaster_wrappers
        #aggregate_impl
    };

    Ok(expanded)
}

// ============================================================================
// #[derive(ReadModel)] derive macro
// ============================================================================
