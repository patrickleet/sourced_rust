mod read_model;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    braced,
    parse::{Parse, ParseStream},
    parse_macro_input, Expr, FnArg, Ident, ItemFn, ItemImpl, LitStr, Pat, Token,
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
                quote! { self.#entity_field.digest_v(#event_name, #ver, &()); }
            } else if param_names.len() == 1 {
                let param = param_names[0];
                quote! { self.#entity_field.digest_v(#event_name, #ver, &(#param.clone(),)); }
            } else {
                quote! { self.#entity_field.digest_v(#event_name, #ver, &(#(#param_names.clone()),*)); }
            }
        }
        None => {
            if param_names.is_empty() {
                quote! { self.#entity_field.digest_empty(#event_name); }
            } else if param_names.len() == 1 {
                let param = param_names[0];
                quote! { self.#entity_field.digest(#event_name, &(#param.clone(),)); }
            } else {
                quote! { self.#entity_field.digest(#event_name, &(#(#param_names.clone()),*)); }
            }
        }
    }
}

/// Wrap a method body with an optional guard condition and prepended statements.
fn wrap_body_with_guard(
    guard: Option<&Expr>,
    prepend: proc_macro2::TokenStream,
    original_stmts: &[syn::Stmt],
) -> syn::Block {
    if let Some(guard) = guard {
        syn::parse_quote! {
            {
                if #guard {
                    #prepend
                    #(#original_stmts)*
                }
            }
        }
    } else {
        syn::parse_quote! {
            {
                #prepend
                #(#original_stmts)*
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
        quote! { self.#emitter_field.enqueue_with(#event_name, &(#param.clone(),)); }
    } else {
        quote! { self.#emitter_field.enqueue_with(#event_name, &(#(#param_names.clone()),*)); }
    };
    quote! {
        if !self.#entity_field.is_replaying() {
            #enqueue_expr
        }
    }
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
/// #[enqueue("OrderCreated")]
/// fn create(&mut self, id: String, items: Vec<Item>) {
///     // enqueue call auto-inserted, params serialized as JSON
///     // calls self.emitter.enqueue_with(...)
/// }
/// ```
///
/// With guard condition:
/// ```ignore
/// #[enqueue("StepCompleted", when = self.can_complete())]
/// fn complete_step(&mut self) {
///     self.completed = true;
/// }
/// ```
///
/// With custom emitter field name:
/// ```ignore
/// #[enqueue(my_emitter, "Created")]
/// fn create(&mut self, name: String) {
///     // uses self.my_emitter instead of self.emitter
/// }
/// ```
///
/// The macro supports:
/// - Default emitter field name: `emitter` (can be overridden by specifying field name first)
/// - `when = condition`: guard that wraps the entire method body
#[proc_macro_attribute]
pub fn enqueue(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with parse_enqueue_args);
    let mut func = parse_macro_input!(item as ItemFn);

    let emitter_field = &args.emitter_field;
    let event_name = &args.event_name;

    // Use function parameters - serialize as tuple to JSON
    let param_names: Vec<_> = func
        .sig
        .inputs
        .iter()
        .filter_map(|arg| {
            if let FnArg::Typed(pat_type) = arg {
                if let Pat::Ident(pat_ident) = &*pat_type.pat {
                    return Some(&pat_ident.ident);
                }
            }
            None
        })
        .collect();

    let entity_field = &args.entity_field;

    let enqueue_call = if param_names.is_empty() {
        quote! {
            if !self.#entity_field.is_replaying() {
                self.#emitter_field.enqueue(#event_name, "");
            }
        }
    } else if param_names.len() == 1 {
        // Single-element tuple needs trailing comma: (x,) not (x)
        let param = &param_names[0];
        quote! {
            if !self.#entity_field.is_replaying() {
                self.#emitter_field.enqueue_with(#event_name, &(#param.clone(),));
            }
        }
    } else {
        // Multi-element tuple
        quote! {
            if !self.#entity_field.is_replaying() {
                self.#emitter_field.enqueue_with(#event_name, &(#(#param_names.clone()),*));
            }
        }
    };

    // Build the new function body
    let original_stmts = &func.block.stmts;
    let new_body = if let Some(guard) = &args.guard {
        // Wrap everything in the guard condition
        syn::parse_quote! {
            {
                if #guard {
                    #enqueue_call
                    #(#original_stmts)*
                }
            }
        }
    } else {
        // No guard - just prepend enqueue
        syn::parse_quote! {
            {
                #enqueue_call
                #(#original_stmts)*
            }
        }
    };
    func.block = Box::new(new_body);

    TokenStream::from(quote! { #func })
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

    // Parse optional guard: `when = condition`
    if input.peek(Token![,]) {
        input.parse::<Token![,]>()?;

        if input.peek(syn::Ident) {
            let ident: syn::Ident = input.fork().parse()?;
            if ident == "when" {
                input.parse::<syn::Ident>()?; // consume "when"
                input.parse::<Token![=]>()?;
                guard = Some(input.parse()?);
            }
        }
    }

    Ok(EnqueueArgs {
        emitter_field,
        entity_field: format_ident!("entity"),
        event_name,
        guard,
    })
}

/// Attribute macro that automatically inserts a digest call at the beginning of a method.
///
/// # Usage
///
/// Basic usage with function parameters (automatically captured):
/// ```ignore
/// #[digest("Initialized")]
/// fn initialize(&mut self, id: String, user_id: String) {
///     // digest call auto-inserted, params serialized as tuple
/// }
/// ```
///
/// With guard condition:
/// ```ignore
/// #[digest("Completed", when = !self.completed)]
/// fn complete(&mut self) {
///     self.completed = true;
/// }
/// ```
///
/// With custom entity field name:
/// ```ignore
/// #[digest(my_entity, "Created")]
/// fn create(&mut self, name: String) {
///     // uses self.my_entity instead of self.entity
/// }
/// ```
///
/// The macro supports:
/// - Default entity field name: `entity` (can be overridden by specifying field name first)
/// - `when = condition`: guard that wraps the entire method body
#[proc_macro_attribute]
pub fn digest(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with parse_digest_args);
    let mut func = parse_macro_input!(item as ItemFn);

    let param_names = extract_param_names(&func.sig);
    let digest_call = generate_digest_call(
        &args.entity_field,
        &args.event_name,
        &param_names,
        args.version.as_ref(),
    );

    let original_stmts = &func.block.stmts;
    let new_body = wrap_body_with_guard(args.guard.as_ref(), digest_call, original_stmts);
    func.block = Box::new(new_body);

    TokenStream::from(quote! { #func })
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

        if input.peek(syn::Ident) {
            let ident: syn::Ident = input.fork().parse()?;
            if ident == "when" {
                input.parse::<syn::Ident>()?; // consume "when"
                input.parse::<Token![=]>()?;
                guard = Some(input.parse()?);
            } else if ident == "version" {
                input.parse::<syn::Ident>()?; // consume "version"
                input.parse::<Token![=]>()?;
                version = Some(input.parse()?);
            }
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
/// sourced_rust::aggregate!(Todo, entity {
///     "Initialized"(id, user_id, task) => initialize,
///     "Completed"() => complete(),
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
    let input = parse_macro_input!(input as AggregateInput);

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
                    self.#method_name();
                }
            }
        } else if call_args.is_empty() {
            // Event has payload but method takes no args
            quote! {
                #event_name => {
                    self.#method_name();
                }
            }
        } else if args.len() == 1 {
            // Single-element tuple needs trailing comma: (x,) not (x)
            let arg = &args[0];
            let call_arg = &call_args[0];
            quote! {
                #event_name => {
                    let (#arg,) = event.decode().map_err(|e| e.to_string())?;
                    self.#method_name(#call_arg);
                }
            }
        } else {
            // Multi-element tuple
            quote! {
                #event_name => {
                    let (#(#args),*) = event.decode().map_err(|e| e.to_string())?;
                    self.#method_name(#(#call_args),*);
                }
            }
        }
    });

    // Generate upcasters() method if upcasters are defined
    let upcasters_method = if input.upcasters.is_empty() {
        quote! {}
    } else {
        let upcaster_entries = input.upcasters.iter().map(|u| {
            let event_name = &u.event_name;
            let from_version = &u.from_version;
            let to_version = &u.to_version;
            let transform_fn = &u.transform_fn;
            quote! {
                sourced_rust::EventUpcaster {
                    event_type: #event_name,
                    from_version: #from_version,
                    to_version: #to_version,
                    transform: #transform_fn,
                }
            }
        });
        quote! {
            fn upcasters() -> &'static [sourced_rust::EventUpcaster] {
                static UPCASTERS: &[sourced_rust::EventUpcaster] = &[
                    #(#upcaster_entries),*
                ];
                UPCASTERS
            }
        }
    };

    let expanded = quote! {
        impl sourced_rust::Aggregate for #agg_name {
            type ReplayError = String;

            fn entity(&self) -> &sourced_rust::Entity {
                &self.#entity_field
            }

            fn entity_mut(&mut self) -> &mut sourced_rust::Entity {
                &mut self.#entity_field
            }

            fn replay_event(
                &mut self,
                event: &sourced_rust::EventRecord,
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

    TokenStream::from(expanded)
}

struct UpcasterDef {
    event_name: LitStr,
    from_version: syn::LitInt,
    to_version: syn::LitInt,
    transform_fn: syn::Path,
}

struct AggregateInput {
    agg_name: Ident,
    entity_field: Ident,
    events: Vec<EventDef>,
    upcasters: Vec<UpcasterDef>,
}

struct EventDef {
    event_name: LitStr,
    args: Vec<Ident>,
    method_name: Ident,
    method_args: Option<Vec<Ident>>, // None = use event args, Some([]) = no args, Some([x,y]) = specific args
}

impl Parse for AggregateInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let agg_name: Ident = input.parse()?;
        input.parse::<Token![,]>()?;
        let entity_field: Ident = input.parse()?;

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
                // Parse: ("EventName", from => to, transform_fn)
                let inner;
                syn::parenthesized!(inner in upcaster_content);

                let event_name: LitStr = inner.parse()?;
                inner.parse::<Token![,]>()?;
                let from_version: syn::LitInt = inner.parse()?;
                inner.parse::<Token![=>]>()?;
                let to_version: syn::LitInt = inner.parse()?;
                inner.parse::<Token![,]>()?;
                let transform_fn: syn::Path = inner.parse()?;

                upcasters.push(UpcasterDef {
                    event_name,
                    from_version,
                    to_version,
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
    enqueue: Option<Ident>, // Some(emitter_field) if enqueue enabled
    upcasters: Vec<UpcasterDef>,
}

fn parse_sourced_args(input: ParseStream) -> syn::Result<SourcedArgs> {
    let entity_field: Ident = input.parse()?;
    let mut enum_name = None;
    let mut enqueue = None;
    let mut upcasters = Vec::new();

    while input.peek(Token![,]) {
        input.parse::<Token![,]>()?;
        if input.peek(Ident) {
            let kw: Ident = input.fork().parse()?;
            if kw == "events" {
                input.parse::<Ident>()?;
                input.parse::<Token![=]>()?;
                enum_name = Some(input.parse::<LitStr>()?);
            } else if kw == "enqueue" {
                input.parse::<Ident>()?;
                // Optional custom emitter field: enqueue(my_emitter)
                if input.peek(syn::token::Paren) {
                    let inner;
                    syn::parenthesized!(inner in input);
                    enqueue = Some(inner.parse::<Ident>()?);
                } else {
                    enqueue = Some(format_ident!("emitter"));
                }
            } else if kw == "upcasters" {
                input.parse::<Ident>()?;
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
                    let transform: syn::Path = inner.parse()?;
                    upcasters.push(UpcasterDef {
                        event_name: ev_name,
                        from_version: from_ver,
                        to_version: to_ver,
                        transform_fn: transform,
                    });
                    if upcaster_content.peek(Token![,]) {
                        upcaster_content.parse::<Token![,]>()?;
                    }
                }
            }
        }
    }

    Ok(SourcedArgs {
        entity_field,
        enum_name,
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
        if input.peek(Ident) {
            let ident: Ident = input.fork().parse()?;
            if ident == "when" {
                input.parse::<Ident>()?;
                input.parse::<Token![=]>()?;
                guard = Some(input.parse()?);
            } else if ident == "version" {
                input.parse::<Ident>()?;
                input.parse::<Token![=]>()?;
                version = Some(input.parse()?);
            }
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
/// # Usage
///
/// ```ignore
/// #[sourced(entity)]
/// impl Todo {
///     #[event("Initialized")]
///     pub fn initialize(&mut self, id: String, user_id: String, task: String) {
///         self.entity.set_id(&id);
///         self.user_id = user_id;
///         self.task = task;
///     }
///
///     #[event("Completed", when = !self.completed)]
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
/// - `#[sourced(entity, upcasters(("EventName", 1 => 2, upcast_fn)))]` - upcasters
#[proc_macro_attribute]
pub fn sourced(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with parse_sourced_args);
    let mut impl_block = parse_macro_input!(item as ItemImpl);

    // Extract struct name from self type
    let struct_name = match &*impl_block.self_ty {
        syn::Type::Path(type_path) => {
            if let Some(segment) = type_path.path.segments.last() {
                segment.ident.clone()
            } else {
                return syn::Error::new_spanned(
                    &impl_block.self_ty,
                    "#[sourced] requires a named type",
                )
                .to_compile_error()
                .into();
            }
        }
        _ => {
            return syn::Error::new_spanned(
                &impl_block.self_ty,
                "#[sourced] requires a named type",
            )
            .to_compile_error()
            .into();
        }
    };

    // Collect event info and modify methods
    let mut event_methods: Vec<EventMethodInfo> = Vec::new();

    for item in &mut impl_block.items {
        if let syn::ImplItem::Fn(method) = item {
            match find_and_remove_event_attr(&mut method.attrs) {
                Ok(Some(event_attr)) => {
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

                    let original_stmts = &method.block.stmts;
                    let new_body = wrap_body_with_guard(
                        event_attr.guard.as_ref(),
                        prepend,
                        original_stmts,
                    );
                    method.block = new_body;

                    event_methods.push(EventMethodInfo {
                        event_name: event_attr.event_name,
                        method_name: method.sig.ident.clone(),
                        params,
                    });
                }
                Ok(None) => { /* not an event method, skip */ }
                Err(err) => return err.to_compile_error().into(),
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
        let variant_name = format_ident!("{}", e.event_name.value());
        if e.params.is_empty() {
            quote! { #variant_name }
        } else {
            let fields = e.params.iter().map(|(name, ty)| quote! { #name: #ty });
            quote! { #variant_name { #(#fields),* } }
        }
    });

    let enum_def = quote! {
        #[derive(Debug, Clone, PartialEq)]
        pub enum #enum_name {
            #(#enum_variants),*
        }
    };

    // Generate event_name() method on the enum
    let event_name_arms = event_methods.iter().map(|e| {
        let variant_name = format_ident!("{}", e.event_name.value());
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
        let variant_name = format_ident!("{}", e.event_name.value());
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
        impl TryFrom<&sourced_rust::EventRecord> for #enum_name {
            type Error = String;
            fn try_from(event: &sourced_rust::EventRecord) -> Result<Self, Self::Error> {
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
                    self.#method_name();
                }
            }
        } else if e.params.len() == 1 {
            let (name, _) = &e.params[0];
            quote! {
                #event_name_str => {
                    let (#name,) = event.decode().map_err(|e| e.to_string())?;
                    self.#method_name(#name);
                }
            }
        } else {
            let names: Vec<_> = e.params.iter().map(|(n, _)| n).collect();
            quote! {
                #event_name_str => {
                    let (#(#names),*) = event.decode().map_err(|e| e.to_string())?;
                    self.#method_name(#(#names),*);
                }
            }
        }
    });

    let upcasters_method = if args.upcasters.is_empty() {
        quote! {}
    } else {
        let upcaster_entries = args.upcasters.iter().map(|u| {
            let ev_name = &u.event_name;
            let from_v = &u.from_version;
            let to_v = &u.to_version;
            let transform = &u.transform_fn;
            quote! {
                sourced_rust::EventUpcaster {
                    event_type: #ev_name,
                    from_version: #from_v,
                    to_version: #to_v,
                    transform: #transform,
                }
            }
        });
        quote! {
            fn upcasters() -> &'static [sourced_rust::EventUpcaster] {
                static UPCASTERS: &[sourced_rust::EventUpcaster] = &[
                    #(#upcaster_entries),*
                ];
                UPCASTERS
            }
        }
    };

    let aggregate_impl = quote! {
        impl sourced_rust::Aggregate for #struct_name {
            type ReplayError = String;

            fn entity(&self) -> &sourced_rust::Entity {
                &self.#entity_field
            }

            fn entity_mut(&mut self) -> &mut sourced_rust::Entity {
                &mut self.#entity_field
            }

            fn replay_event(
                &mut self,
                event: &sourced_rust::EventRecord,
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
        #aggregate_impl
    };

    TokenStream::from(expanded)
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
/// #[readmodel(collection = "counter_views")]
/// struct CounterView {
///     #[readmodel(id)]
///     pub id: String,
///     pub name: String,
///     pub value: i32,
/// }
/// ```
///
/// - `#[readmodel(collection = "...")]` sets the collection name.
///   If omitted, defaults to snake_case struct name + "s".
/// - `#[readmodel(id)]` marks the field used as the unique identifier.
///   If omitted, defaults to a field named `id`.
#[proc_macro_derive(ReadModel, attributes(readmodel))]
pub fn derive_read_model(input: TokenStream) -> TokenStream {
    read_model::derive_read_model(input)
}
