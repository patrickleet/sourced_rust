mod read_model;

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    braced,
    parse::{Parse, ParseStream},
    parse_macro_input, Expr, FnArg, Ident, ItemFn, LitStr, Pat, Token,
};

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

    let entity_field = &args.entity_field;
    let event_name = &args.event_name;

    // Use function parameters - serialize as tuple
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

    let digest_call = match &args.version {
        Some(ver) => {
            // Versioned digest: use digest_v
            if param_names.is_empty() {
                quote! {
                    self.#entity_field.digest_v(#event_name, #ver, &());
                }
            } else if param_names.len() == 1 {
                let param = &param_names[0];
                quote! {
                    self.#entity_field.digest_v(#event_name, #ver, &(#param.clone(),));
                }
            } else {
                quote! {
                    self.#entity_field.digest_v(#event_name, #ver, &(#(#param_names.clone()),*));
                }
            }
        }
        None => {
            // Default: use digest (version 1)
            if param_names.is_empty() {
                quote! {
                    self.#entity_field.digest_empty(#event_name);
                }
            } else if param_names.len() == 1 {
                let param = &param_names[0];
                quote! {
                    self.#entity_field.digest(#event_name, &(#param.clone(),));
                }
            } else {
                quote! {
                    self.#entity_field.digest(#event_name, &(#(#param_names.clone()),*));
                }
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
                    #digest_call
                    #(#original_stmts)*
                }
            }
        }
    } else {
        // No guard - just prepend digest
        syn::parse_quote! {
            {
                #digest_call
                #(#original_stmts)*
            }
        }
    };
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
