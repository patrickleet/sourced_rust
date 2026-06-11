mod read_model;
mod snapshot;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    braced,
    parse::{Parse, ParseStream, Parser},
    Expr, FnArg, Ident, ItemFn, ItemImpl, LitStr, Pat, ReturnType, Token, Type,
};

// ============================================================================
// Shared helpers
// ============================================================================

/// Extract parameter names from a method signature (excludes `self`).
fn extract_param_names(sig: &syn::Signature) -> Vec<&Ident> {
    sig.inputs
        .iter()
        .filter_map(|arg| {
            if let FnArg::Typed(pat_type) = arg {
                if let Pat::Ident(pat_ident) = &*pat_type.pat {
                    return Some(&pat_ident.ident);
                }
            }
            None
        })
        .collect()
}

/// Extract parameter names and types from a method signature (excludes `self`).
fn extract_params_with_types(sig: &syn::Signature) -> Vec<(Ident, syn::Type)> {
    sig.inputs
        .iter()
        .filter_map(|arg| {
            if let FnArg::Typed(pat_type) = arg {
                if let Pat::Ident(pat_ident) = &*pat_type.pat {
                    return Some((pat_ident.ident.clone(), (*pat_type.ty).clone()));
                }
            }
            None
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

fn ensure_sourced_result_signature(
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
fn generate_digest_call(
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
fn wrap_result_body_with_guard(
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
fn generate_enqueue_call(
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

fn upcaster_wrapper_prefix(owner: &Ident) -> String {
    owner
        .to_string()
        .trim_start_matches("r#")
        .to_ascii_lowercase()
}

fn event_variant_ident(event_name: &LitStr) -> Ident {
    let value = event_name.value();
    let segment = value
        .rsplit('.')
        .find(|part| !part.is_empty())
        .unwrap_or(&value);
    let mut ident = String::new();
    let mut capitalize_next = true;

    for ch in segment.chars() {
        if ch.is_ascii_alphanumeric() {
            if ident.is_empty() && ch.is_ascii_digit() {
                ident.push_str("Event");
            }

            if capitalize_next {
                ident.push(ch.to_ascii_uppercase());
                capitalize_next = false;
            } else {
                ident.push(ch);
            }
        } else {
            capitalize_next = true;
        }
    }

    if ident.is_empty() {
        ident.push_str("Event");
    }

    format_ident!("{}", ident)
}

fn generate_upcaster_tokens(
    owner: &Ident,
    upcasters: &[UpcasterDef],
) -> (proc_macro2::TokenStream, proc_macro2::TokenStream) {
    if upcasters.is_empty() {
        return (quote! {}, quote! {});
    }

    let prefix = upcaster_wrapper_prefix(owner);
    let wrapper_names: Vec<_> = upcasters
        .iter()
        .enumerate()
        .map(|(idx, _)| format_ident!("__sourced_upcast_{}_{}", prefix, idx))
        .collect();

    let wrapper_defs = upcasters
        .iter()
        .zip(wrapper_names.iter())
        .map(|(u, wrapper)| {
            let source_type = &u.source_type;
            let target_type = &u.target_type;
            let to_version = &u.to_version;
            let transform_fn = &u.transform_fn;
            quote! {
                fn #wrapper(
                    event: &distributed::EventRecord,
                ) -> Result<Vec<u8>, distributed::UpcastError> {
                    distributed::upcast_payload::<#source_type, #target_type>(
                        event,
                        #to_version,
                        #transform_fn,
                    )
                }
            }
        });

    let upcaster_entries = upcasters
        .iter()
        .zip(wrapper_names.iter())
        .map(|(u, wrapper)| {
            let event_name = &u.event_name;
            let from_version = &u.from_version;
            let to_version = &u.to_version;
            quote! {
                distributed::EventUpcaster {
                    event_type: #event_name,
                    from_version: #from_version,
                    to_version: #to_version,
                    transform: #owner::#wrapper,
                }
            }
        });

    let upcasters_method = quote! {
        fn upcasters() -> &'static [distributed::EventUpcaster] {
            static UPCASTERS: &[distributed::EventUpcaster] = &[
                #(#upcaster_entries),*
            ];
            UPCASTERS
        }
    };

    (
        quote! {
            impl #owner {
                #(#wrapper_defs)*
            }
        },
        upcasters_method,
    )
}

// ============================================================================
// #[enqueue] attribute macro
// ============================================================================

/// Attribute macro that automatically queues a local event for emission.
///
/// Similar to `#[digest]`, this macro captures function parameters and
/// serializes them as the event data (using JSON). Events are queued
/// during method execution and can be emitted after a successful commit.
///
/// **Requires the `emitter` feature to be enabled.**
///
/// # Usage
///
/// Basic usage with function parameters (automatically captured):
/// ```ignore
/// #[enqueue("order.initialized")]
/// fn create(&mut self, id: String, items: Vec<Item>) {
///     // enqueue call auto-inserted, params serialized as JSON
///     // calls self.emitter.enqueue_with(...)
/// }
/// ```
///
/// With guard condition:
/// ```ignore
/// #[enqueue("step.completed", when = self.can_complete())]
/// fn complete_step(&mut self) {
///     self.completed = true;
/// }
/// ```
///
/// With custom emitter field name:
/// ```ignore
/// #[enqueue(my_emitter, "todo.initialized")]
/// fn create(&mut self, name: String) {
///     // uses self.my_emitter instead of self.emitter
/// }
/// ```
///
/// With a renamed entity field (used for the replay guard):
/// ```ignore
/// #[enqueue("order.initialized", entity = state)]
/// fn create(&mut self, id: String) {
///     // uses self.state.is_replaying() instead of self.entity.is_replaying()
/// }
/// ```
///
/// The macro supports:
/// - Default emitter field name: `emitter` (can be overridden by specifying field name first)
/// - `when = condition`: guard that wraps the entire method body
/// - `entity = field`: entity field used for the `is_replaying()` guard (default: `entity`)
/// - Methods may omit the return type; the macro expands them to `distributed::SourcedResult<()>`
#[proc_macro_attribute]
pub fn enqueue(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_enqueue(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

fn expand_enqueue(attr: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
    let args = parse_enqueue_args.parse2(attr)?;
    let mut func = syn::parse2::<ItemFn>(item)?;

    let signature_synthesized = ensure_sourced_result_signature(&mut func.sig, "enqueue")?;

    let emitter_field = &args.emitter_field;
    let event_name = &args.event_name;

    // Use function parameters - serialize as tuple to JSON
    let param_names = extract_param_names(&func.sig);

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

struct EnqueueArgs {
    emitter_field: syn::Ident,
    entity_field: syn::Ident,
    event_name: LitStr,
    guard: Option<Expr>,
}

fn parse_enqueue_args(input: syn::parse::ParseStream) -> syn::Result<EnqueueArgs> {
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

/// Attribute macro that automatically inserts a digest call at the beginning of a method.
///
/// If the annotated method omits a return type, it expands to
/// `distributed::SourcedResult<()>`. Methods may also explicitly return
/// `Result<(), E>` where `E` can be constructed from
/// `distributed::EventRecordError`; explicit `Result` methods should return
/// `Ok(())` from the original body.
///
/// The generated digest call runs before the original method body. This means
/// serialization errors are returned before the method mutates state. If the
/// method can reject a command for domain validation reasons, prefer calling
/// `self.entity.digest(...)?` explicitly after validation and before mutation.
///
/// # Usage
///
/// Basic usage with function parameters (automatically captured):
/// ```ignore
/// #[digest("initialized")]
/// fn initialize(&mut self, id: String, user_id: String) {
///     // digest call auto-inserted, params serialized as tuple
/// }
/// ```
///
/// With guard condition:
/// ```ignore
/// #[digest("completed", when = !self.completed)]
/// fn complete(&mut self) -> Result<(), distributed::EventRecordError> {
///     self.completed = true;
///     Ok(())
/// }
/// ```
/// The guard wraps both the digest call and the method body, so a false guard
/// records no event and skips the body.
///
/// With validation inside the command body, use explicit digesting:
/// ```ignore
/// fn rename(&mut self, title: String) -> Result<(), TodoError> {
///     if title.trim().is_empty() {
///         return Err(TodoError::EmptyTitle);
///     }
///
///     self.entity.digest("renamed", &(title.clone(),))?;
///     self.title = title;
///     Ok(())
/// }
/// ```
///
/// With custom entity field name:
/// ```ignore
/// #[digest(my_entity, "initialized")]
/// fn create(&mut self, name: String) -> Result<(), distributed::EventRecordError> {
///     // uses self.my_entity instead of self.entity
///     Ok(())
/// }
/// ```
///
/// The macro supports:
/// - Default entity field name: `entity` (can be overridden by specifying field name first)
/// - `when = condition`: guard that wraps the entire method body
#[proc_macro_attribute]
pub fn digest(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_digest(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

fn expand_digest(attr: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
    let args = parse_digest_args.parse2(attr)?;
    let mut func = syn::parse2::<ItemFn>(item)?;

    let signature_synthesized = ensure_sourced_result_signature(&mut func.sig, "digest")?;

    let param_names = extract_param_names(&func.sig);
    let digest_call = generate_digest_call(
        &args.entity_field,
        &args.event_name,
        &param_names,
        args.version.as_ref(),
    );

    let new_body = wrap_result_body_with_guard(
        args.guard.as_ref(),
        digest_call,
        &func.block,
        signature_synthesized,
    );
    *func.block = new_body;

    Ok(quote! { #func })
}

struct DigestArgs {
    entity_field: syn::Ident,
    event_name: LitStr,
    guard: Option<Expr>,
    version: Option<syn::LitInt>,
}

fn parse_digest_args(input: syn::parse::ParseStream) -> syn::Result<DigestArgs> {
    // Check if first token is an identifier (potential entity field) or a string literal (event name)
    let (entity_field, event_name) = if input.peek(LitStr) {
        // No entity field specified, use default "entity"
        let event_name: LitStr = input.parse()?;
        (format_ident!("entity"), event_name)
    } else {
        // First token is an identifier - could be entity field or event name follows
        let first_ident: syn::Ident = input.parse()?;
        input.parse::<Token![,]>()?;
        let event_name: LitStr = input.parse()?;
        (first_ident, event_name)
    };

    let mut guard = None;
    let mut version = None;

    // Parse optional keyword arguments: `when = condition`, `version = N`
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
        } else if ident == "version" {
            input.parse::<Token![=]>()?;
            version = Some(input.parse()?);
        } else {
            return Err(syn::Error::new_spanned(
                &ident,
                format!(
                    "unsupported key `{ident}` in #[digest(...)]; expected `when` or `version`"
                ),
            ));
        }
    }

    Ok(DigestArgs {
        entity_field,
        event_name,
        guard,
        version,
    })
}

// ============================================================================
// aggregate! proc-macro
// ============================================================================

/// Generates the Aggregate trait impl with replay logic.
///
/// # Usage
///
/// ```ignore
/// distributed::aggregate!(Todo, entity {
///     "initialized"(id, user_id, task) => initialize,
///     "completed"() => complete(),
/// });
/// ```
///
/// This generates the `Aggregate` trait impl that handles replaying events.
/// Events are stored as serialized payloads and deserialized on replay.
///
/// Note: Use `=> method()` (with parens) when the method takes no arguments.
/// Use `=> method` (no parens) to pass all event args to the method.
#[proc_macro]
pub fn aggregate(input: TokenStream) -> TokenStream {
    expand_aggregate(input.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

fn expand_aggregate(input: TokenStream2) -> syn::Result<TokenStream2> {
    let input = syn::parse2::<AggregateInput>(input)?;

    let agg_name = &input.agg_name;
    let entity_field = &input.entity_field;

    // Generate replay match arms - deserialize and call method directly
    let replay_arms = input.events.iter().map(|evt| {
        let event_name = &evt.event_name;
        let method_name = &evt.method_name;
        let args = &evt.args;

        // Determine what args to pass to the method
        let call_args: Vec<_> = match &evt.method_args {
            Some(method_args) => method_args.clone(),
            None => args.clone(),
        };

        if args.is_empty() {
            // No payload
            quote! {
                #event_name => {
                    self.#method_name().map_err(|e| e.to_string())?;
                }
            }
        } else if call_args.is_empty() {
            // Event has payload but method takes no args
            quote! {
                #event_name => {
                    self.#method_name().map_err(|e| e.to_string())?;
                }
            }
        } else if args.len() == 1 {
            // Single-element tuple needs trailing comma: (x,) not (x)
            let arg = &args[0];
            let call_arg = &call_args[0];
            quote! {
                #event_name => {
                    let (#arg,) = event.decode().map_err(|e| e.to_string())?;
                    self.#method_name(#call_arg).map_err(|e| e.to_string())?;
                }
            }
        } else {
            // Multi-element tuple
            quote! {
                #event_name => {
                    let (#(#args),*) = event.decode().map_err(|e| e.to_string())?;
                    self.#method_name(#(#call_args),*).map_err(|e| e.to_string())?;
                }
            }
        }
    });

    let (upcaster_wrappers, upcasters_method) =
        generate_upcaster_tokens(agg_name, &input.upcasters);
    let aggregate_type_method = input.aggregate_type.as_ref().map(|aggregate_type| {
        quote! {
            fn aggregate_type() -> &'static str {
                #aggregate_type
            }
        }
    });

    // ReplayError stays `String`: replay errors are flattened from
    // heterogeneous sources (per-event `decode()` errors, user method errors of
    // arbitrary `E`, and unknown-event messages) via `e.to_string()`. A typed
    // error would have to be generic over each method's error type or erase
    // them anyway, so `String` is the smaller, honest representation here.
    let expanded = quote! {
        #upcaster_wrappers

        impl distributed::Aggregate for #agg_name {
            type ReplayError = String;

            #aggregate_type_method

            fn entity(&self) -> &distributed::Entity {
                &self.#entity_field
            }

            fn entity_mut(&mut self) -> &mut distributed::Entity {
                &mut self.#entity_field
            }

            fn replay_event(
                &mut self,
                event: &distributed::EventRecord,
            ) -> Result<(), Self::ReplayError> {
                match event.event_name.as_str() {
                    #(#replay_arms)*
                    _ => return Err(format!("Unknown event: {}", event.event_name)),
                }
                Ok(())
            }

            #upcasters_method
        }
    };

    Ok(expanded)
}

struct UpcasterDef {
    event_name: LitStr,
    from_version: syn::LitInt,
    to_version: syn::LitInt,
    source_type: syn::Type,
    target_type: syn::Type,
    transform_fn: syn::Path,
}

struct AggregateInput {
    agg_name: Ident,
    entity_field: Ident,
    aggregate_type: Option<LitStr>,
    events: Vec<EventDef>,
    upcasters: Vec<UpcasterDef>,
}

struct EventDef {
    event_name: LitStr,
    args: Vec<Ident>,
    method_name: Ident,
    method_args: Option<Vec<Ident>>, // None = use event args, Some([]) = no args, Some([x,y]) = specific args
}

fn validate_aggregate_type_literal(lit: &LitStr) -> syn::Result<()> {
    let value = lit.value();
    if value.trim().is_empty() {
        return Err(syn::Error::new_spanned(
            lit,
            "`aggregate_type` must not be empty or whitespace",
        ));
    }
    if value.contains('\u{1f}') {
        return Err(syn::Error::new_spanned(
            lit,
            "`aggregate_type` must not contain the reserved stream delimiter (U+001F)",
        ));
    }
    Ok(())
}

impl Parse for AggregateInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let agg_name: Ident = input.parse()?;
        input.parse::<Token![,]>()?;
        let entity_field: Ident = input.parse()?;
        let mut aggregate_type = None;

        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            let kw: Ident = input.parse()?;
            if kw != "aggregate_type" {
                return Err(syn::Error::new(kw.span(), "expected `aggregate_type`"));
            }
            input.parse::<Token![=]>()?;
            let lit = input.parse::<LitStr>()?;
            validate_aggregate_type_literal(&lit)?;
            aggregate_type = Some(lit);
        }

        let content;
        braced!(content in input);

        let mut events = Vec::new();
        while !content.is_empty() {
            let event_name: LitStr = content.parse()?;

            // Parse (arg1, arg2, ...)
            let args_content;
            syn::parenthesized!(args_content in content);
            let args: syn::punctuated::Punctuated<Ident, Token![,]> =
                args_content.parse_terminated(Ident::parse, Token![,])?;
            let args: Vec<Ident> = args.into_iter().collect();

            content.parse::<Token![=>]>()?;
            let method_name: Ident = content.parse()?;

            // Check for optional method args: method() or method(a, b)
            let method_args = if content.peek(syn::token::Paren) {
                let method_args_content;
                syn::parenthesized!(method_args_content in content);
                let method_args: syn::punctuated::Punctuated<Ident, Token![,]> =
                    method_args_content.parse_terminated(Ident::parse, Token![,])?;
                Some(method_args.into_iter().collect())
            } else {
                None // Use event args
            };

            events.push(EventDef {
                event_name,
                args,
                method_name,
                method_args,
            });

            // Optional trailing comma
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }

        // Parse optional `upcasters [...]` block
        let mut upcasters = Vec::new();
        if input.peek(syn::Ident) {
            let kw: Ident = input.parse()?;
            if kw != "upcasters" {
                return Err(syn::Error::new(kw.span(), "expected `upcasters`"));
            }

            let upcaster_content;
            syn::bracketed!(upcaster_content in input);

            while !upcaster_content.is_empty() {
                // Parse: ("initialized", from => to, SourceType => TargetType, transform_fn)
                let inner;
                syn::parenthesized!(inner in upcaster_content);

                let event_name: LitStr = inner.parse()?;
                inner.parse::<Token![,]>()?;
                let from_version: syn::LitInt = inner.parse()?;
                inner.parse::<Token![=>]>()?;
                let to_version: syn::LitInt = inner.parse()?;
                inner.parse::<Token![,]>()?;
                let source_type: syn::Type = inner.parse()?;
                inner.parse::<Token![=>]>()?;
                let target_type: syn::Type = inner.parse()?;
                inner.parse::<Token![,]>()?;
                let transform_fn: syn::Path = inner.parse()?;

                upcasters.push(UpcasterDef {
                    event_name,
                    from_version,
                    to_version,
                    source_type,
                    target_type,
                    transform_fn,
                });

                // Optional trailing comma between upcaster entries
                if upcaster_content.peek(Token![,]) {
                    upcaster_content.parse::<Token![,]>()?;
                }
            }
        }

        Ok(AggregateInput {
            agg_name,
            entity_field,
            aggregate_type,
            events,
            upcasters,
        })
    }
}

// ============================================================================
// #[sourced] attribute macro
// ============================================================================

struct SourcedArgs {
    entity_field: Ident,
    enum_name: Option<LitStr>,
    aggregate_type: Option<LitStr>,
    enqueue: Option<Ident>, // Some(emitter_field) if enqueue enabled
    upcasters: Vec<UpcasterDef>,
}

fn parse_sourced_args(input: ParseStream) -> syn::Result<SourcedArgs> {
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
            while !upcaster_content.is_empty() {
                let inner;
                syn::parenthesized!(inner in upcaster_content);
                let ev_name: LitStr = inner.parse()?;
                inner.parse::<Token![,]>()?;
                let from_ver: syn::LitInt = inner.parse()?;
                inner.parse::<Token![=>]>()?;
                let to_ver: syn::LitInt = inner.parse()?;
                inner.parse::<Token![,]>()?;
                let source_type: syn::Type = inner.parse()?;
                inner.parse::<Token![=>]>()?;
                let target_type: syn::Type = inner.parse()?;
                inner.parse::<Token![,]>()?;
                let transform: syn::Path = inner.parse()?;
                upcasters.push(UpcasterDef {
                    event_name: ev_name,
                    from_version: from_ver,
                    to_version: to_ver,
                    source_type,
                    target_type,
                    transform_fn: transform,
                });
                if upcaster_content.peek(Token![,]) {
                    upcaster_content.parse::<Token![,]>()?;
                }
            }
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

struct EventAttr {
    event_name: LitStr,
    guard: Option<Expr>,
    version: Option<syn::LitInt>,
}

fn parse_event_args(input: ParseStream) -> syn::Result<EventAttr> {
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

/// Attribute macro that generates a typed event enum, `TryFrom<&EventRecord>`,
/// and `impl Aggregate` from annotated methods in an impl block.
///
/// Each `#[event(...)]` method is rewritten with the same recording order as
/// `#[digest]`: event recording runs before the original method body, and a
/// `when = ...` guard wraps both event recording and the body. Use explicit
/// `self.entity.digest(...)?` calls for commands that need to validate before
/// deciding whether to record an event.
///
/// # Usage
///
/// ```ignore
/// #[sourced(entity)]
/// impl Todo {
///     #[event("initialized")]
///     pub fn initialize(&mut self, id: String, user_id: String, task: String) {
///         self.entity.set_id(&id);
///         self.user_id = user_id;
///         self.task = task;
///     }
///
///     #[event("completed", when = !self.completed)]
///     pub fn complete(&mut self) {
///         self.completed = true;
///     }
/// }
/// // Generates: TodoEvent enum, TryFrom<&EventRecord>, impl Aggregate
/// ```
///
/// Options:
/// - `#[sourced(entity)]` - entity field name
/// - `#[sourced(entity, events = "CustomName")]` - custom enum name
/// - `#[sourced(entity, aggregate_type = "todos")]` - stable stream type name
/// - `#[sourced(entity, upcasters(("initialized", 1 => 2, OldPayload => NewPayload, upcast_fn)))]` - upcasters
#[proc_macro_attribute]
pub fn sourced(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_sourced(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

fn expand_sourced(attr: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
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
    let mut seen_events: std::collections::HashMap<String, proc_macro2::Span> =
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
                    if let Some(_prev) =
                        seen_events.insert(event_key.clone(), event_attr.event_name.span())
                    {
                        return Err(syn::Error::new_spanned(
                            &event_attr.event_name,
                            format!("duplicate #[event] name `{event_key}` in this #[sourced] impl block"),
                        ));
                    }

                    let signature_synthesized =
                        ensure_sourced_result_signature(&mut method.sig, "event")?;

                    let params = extract_params_with_types(&method.sig);
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
    let replay_arms = event_methods.iter().map(|e| {
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
    });

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
    let aggregate_impl = quote! {
        impl distributed::Aggregate for #struct_name {
            type ReplayError = String;

            #aggregate_type_method

            fn entity(&self) -> &distributed::Entity {
                &self.#entity_field
            }

            fn entity_mut(&mut self) -> &mut distributed::Entity {
                &mut self.#entity_field
            }

            fn replay_event(
                &mut self,
                event: &distributed::EventRecord,
            ) -> Result<(), Self::ReplayError> {
                match event.event_name.as_str() {
                    #(#replay_arms)*
                    _ => return Err(format!("Unknown event: {}", event.event_name)),
                }
                Ok(())
            }

            #upcasters_method
        }
    };

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

/// Derive macro for the `ReadModel` trait.
///
/// # Usage
///
/// ```ignore
/// #[derive(Clone, Serialize, Deserialize, ReadModel)]
/// #[collection("counter_views")]
/// struct CounterView {
///     #[id]
///     pub id: String,
///     pub name: String,
///     pub value: i32,
/// }
/// ```
///
/// - `#[readmodel(collection = "...")]` or `#[collection("...")]` sets the
///   collection name.
///   If omitted, defaults to snake_case struct name + "s".
/// - `#[readmodel(table = "...")]` or `#[table("...")]` opts into relational
///   table metadata.
/// - `#[readmodel(column = "...")]` or `#[column("...")]` sets a relational
///   column name.
/// - `#[readmodel(id)]`, `#[id]`, or `#[id("column_name")]` marks the field
///   used as the unique identifier.
///   If omitted, defaults to a field named `id`.
/// - `#[readmodel(index)]`, `#[index]`, or `#[index("index_name")]` declares a
///   secondary index on a field.
/// - `#[readmodel(unique)]`, `#[unique]`, or `#[unique("index_name")]`
///   declares a unique secondary index on a field.
/// - `#[index(columns = ["field_a", "field_b"])]` or
///   `#[index(name = "...", columns = ["field_a", "field_b"])]` declares a
///   compound secondary index on a struct.
/// - `#[unique(columns = ["field_a", "field_b"])]` or
///   `#[unique(name = "...", columns = ["field_a", "field_b"])]` declares a
///   compound unique index on a struct.
#[proc_macro_derive(
    ReadModel,
    attributes(readmodel, collection, table, column, id, index, unique)
)]
pub fn derive_read_model(input: TokenStream) -> TokenStream {
    read_model::derive_read_model(input)
}

// ============================================================================
// #[derive(Snapshot)] derive macro
// ============================================================================

/// Derive macro that generates a snapshot struct, `fn snapshot()`, and
/// `impl Snapshottable` for an aggregate.
///
/// # Usage
///
/// ```ignore
/// #[derive(Default, Snapshot)]
/// struct Todo {
///     pub entity: Entity,
///     user_id: String,
///     task: String,
///     completed: bool,
/// }
/// ```
///
/// Generates `TodoSnapshot` with `id: String` + all non-entity fields,
/// a `fn snapshot()` method, and `impl Snapshottable`.
///
/// Options:
/// - `#[snapshot(id = "sku")]` — use a struct field as the ID key instead of synthesizing `id`
/// - `#[snapshot(entity = "my_entity")]` — override the entity field name (default: `entity`)
/// - `#[snapshot(version = N)]` — set `Snapshottable::SNAPSHOT_VERSION` (default: 1).
///   Bump this whenever the snapshot layout changes incompatibly; on load a
///   stored snapshot whose version differs is treated as a cache miss and the
///   aggregate is rebuilt by replay rather than mis-decoded.
/// - Fields with `#[serde(skip)]` are automatically excluded
#[proc_macro_derive(Snapshot, attributes(snapshot))]
pub fn derive_snapshot(input: TokenStream) -> TokenStream {
    snapshot::derive_snapshot(input)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;
    use syn::parse::Parser;

    // ---- digest ----------------------------------------------------------

    #[test]
    fn expand_digest_inserts_digest_call() {
        let attr = quote! { "initialized" };
        let item = quote! {
            fn initialize(&mut self, id: String) {
                self.id = id;
            }
        };
        let out = expand_digest(attr, item).unwrap().to_string();
        assert!(out.contains("digest"), "unexpected output: {out}");
        assert!(out.contains("\"initialized\""), "unexpected output: {out}");
    }

    #[test]
    fn parse_digest_args_rejects_unknown_key() {
        let attr = quote! { "x", versoin = 2 };
        let err = parse_digest_args
            .parse2(attr)
            .err()
            .expect("unknown key should error");
        let msg = err.to_string();
        assert!(msg.contains("unsupported key `versoin`"), "got: {msg}");
        assert!(msg.contains("version"), "got: {msg}");
    }

    #[test]
    fn parse_digest_args_accepts_version_and_when() {
        let attr = quote! { entity, "renamed", when = true, version = 2 };
        let args = parse_digest_args.parse2(attr).unwrap();
        assert_eq!(args.entity_field, "entity");
        assert!(args.guard.is_some());
        assert!(args.version.is_some());
    }

    // ---- enqueue ---------------------------------------------------------

    #[test]
    fn parse_enqueue_args_defaults_entity_field() {
        let attr = quote! { "order.initialized" };
        let args = parse_enqueue_args.parse2(attr).unwrap();
        assert_eq!(args.emitter_field, "emitter");
        assert_eq!(args.entity_field, "entity");
    }

    #[test]
    fn parse_enqueue_args_overrides_entity_field() {
        let attr = quote! { "order.initialized", entity = state };
        let args = parse_enqueue_args.parse2(attr).unwrap();
        assert_eq!(args.entity_field, "state");
    }

    #[test]
    fn expand_enqueue_uses_renamed_entity_field_in_guard() {
        let attr = quote! { "order.initialized", entity = state };
        let item = quote! {
            fn create(&mut self, id: String) {
                self.id = id;
            }
        };
        let out = expand_enqueue(attr, item).unwrap().to_string();
        assert!(
            out.contains("self . state . is_replaying"),
            "expected renamed entity guard, got: {out}"
        );
    }

    #[test]
    fn parse_enqueue_args_rejects_unknown_key() {
        let attr = quote! { "x", wen = true };
        let err = parse_enqueue_args
            .parse2(attr)
            .err()
            .expect("unknown key should error");
        assert!(
            err.to_string().contains("unsupported key `wen`"),
            "got: {err}"
        );
    }

    // ---- event (parse) ---------------------------------------------------

    #[test]
    fn parse_event_args_rejects_unknown_key() {
        let attr = quote! { "completed", wen = true };
        let err = parse_event_args
            .parse2(attr)
            .err()
            .expect("unknown key should error");
        assert!(
            err.to_string().contains("unsupported key `wen`"),
            "got: {err}"
        );
    }

    // ---- sourced ---------------------------------------------------------

    #[test]
    fn parse_sourced_args_requires_entity_field() {
        let attr = quote! {};
        let err = parse_sourced_args
            .parse2(attr)
            .err()
            .expect("missing entity should error");
        assert!(
            err.to_string().contains("requires the entity field name"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_sourced_args_rejects_unknown_key() {
        let attr = quote! { entity, evnts = "Foo" };
        let err = parse_sourced_args
            .parse2(attr)
            .err()
            .expect("unknown key should error");
        assert!(
            err.to_string().contains("unsupported key `evnts`"),
            "got: {err}"
        );
    }

    #[test]
    fn expand_sourced_generates_enum_and_aggregate() {
        let attr = quote! { entity };
        let item = quote! {
            impl Todo {
                #[event("initialized")]
                pub fn initialize(&mut self, id: String) {
                    self.id = id;
                }
            }
        };
        let out = expand_sourced(attr, item).unwrap().to_string();
        assert!(out.contains("enum TodoEvent"), "got: {out}");
        assert!(
            out.contains("impl distributed :: Aggregate for Todo"),
            "got: {out}"
        );
    }

    #[test]
    fn expand_sourced_rejects_duplicate_event_names() {
        let attr = quote! { entity };
        let item = quote! {
            impl Todo {
                #[event("done")]
                pub fn complete(&mut self) {}
                #[event("done")]
                pub fn finish(&mut self) {}
            }
        };
        let err = expand_sourced(attr, item).expect_err("duplicate should error");
        assert!(
            err.to_string().contains("duplicate #[event] name `done`"),
            "got: {err}"
        );
    }

    #[test]
    fn expand_sourced_rejects_event_without_receiver() {
        let attr = quote! { entity };
        let item = quote! {
            impl Todo {
                #[event("initialized")]
                pub fn initialize(id: String) {}
            }
        };
        let err = expand_sourced(attr, item).expect_err("missing receiver should error");
        assert!(
            err.to_string().contains("must take a `&mut self` receiver"),
            "got: {err}"
        );
    }

    // ---- aggregate -------------------------------------------------------

    #[test]
    fn expand_aggregate_generates_impl() {
        let input = quote! {
            Todo, entity {
                "initialized"(id) => initialize,
            }
        };
        let out = expand_aggregate(input).unwrap().to_string();
        assert!(
            out.contains("impl distributed :: Aggregate for Todo"),
            "got: {out}"
        );
        assert!(out.contains("replay_event"), "got: {out}");
    }
}
