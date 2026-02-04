use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    braced,
    parse::{Parse, ParseStream},
    parse_macro_input, Expr, FnArg, Ident, ItemFn, LitStr, Pat, Token,
};

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
/// With `id` shorthand (expands to `self.entity.id().to_string()`):
/// ```ignore
/// #[digest("Completed", id, when = !self.completed)]
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
/// - `id` shorthand: expands to `self.<entity_field>.id().to_string()`
/// - `when = condition`: guard that wraps the entire method body
#[proc_macro_attribute]
pub fn digest(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with parse_digest_args);
    let mut func = parse_macro_input!(item as ItemFn);

    let entity_field = &args.entity_field;
    let event_name = &args.event_name;

    // Process custom args, expanding `id` shorthand
    let expanded_args: Vec<proc_macro2::TokenStream> = args
        .custom_args
        .iter()
        .map(|arg| {
            // Check if it's the `id` shorthand
            if let Expr::Path(path) = arg {
                if path.path.is_ident("id") {
                    return quote! { self.#entity_field.id().to_string() };
                }
            }
            quote! { #arg }
        })
        .collect();

    // Build the digest call based on whether custom expressions were provided
    let digest_call = if args.custom_args.is_empty() {
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

        if param_names.is_empty() {
            quote! {
                self.#entity_field.digest_empty(#event_name);
            }
        } else if param_names.len() == 1 {
            // Single-element tuple needs trailing comma: (x,) not (x)
            let param = &param_names[0];
            quote! {
                self.#entity_field.digest(#event_name, &(#param.clone(),));
            }
        } else {
            // Multi-element tuple
            quote! {
                self.#entity_field.digest(#event_name, &(#(#param_names.clone()),*));
            }
        }
    } else {
        // Use custom expressions
        let temp_names: Vec<_> = (0..expanded_args.len())
            .map(|i| format_ident!("__digest_arg_{}", i))
            .collect();

        if temp_names.len() == 1 {
            let temp = &temp_names[0];
            let expanded = &expanded_args[0];
            quote! {
                let #temp = #expanded;
                self.#entity_field.digest(#event_name, &(#temp,));
            }
        } else {
            quote! {
                #(let #temp_names = #expanded_args;)*
                self.#entity_field.digest(#event_name, &(#(#temp_names),*));
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
    custom_args: Vec<Expr>,
    guard: Option<Expr>,
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

    let mut custom_args = Vec::new();
    let mut guard = None;

    // Parse optional custom expressions and/or guard
    while input.peek(Token![,]) {
        input.parse::<Token![,]>()?;

        // Check for `when = expr`
        if input.peek(syn::Ident) {
            let ident: syn::Ident = input.fork().parse()?;
            if ident == "when" {
                input.parse::<syn::Ident>()?; // consume "when"
                input.parse::<Token![=]>()?;
                guard = Some(input.parse()?);
                continue;
            }
        }

        // Otherwise it's a custom arg expression
        let expr: Expr = input.parse()?;
        custom_args.push(expr);
    }

    Ok(DigestArgs {
        entity_field,
        event_name,
        custom_args,
        guard,
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
///     "Completed"(id) => complete(),  // () means method takes no args
/// });
/// ```
///
/// This generates the `Aggregate` trait impl that handles replaying events.
/// Events are stored as serialized payloads and deserialized on replay.
///
/// Note: Use `=> method()` (with parens) when the method takes no arguments,
/// even if the event has args. Use `=> method` (no parens) to pass all event
/// args to the method.
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
        }
    };

    TokenStream::from(expanded)
}

struct AggregateInput {
    agg_name: Ident,
    entity_field: Ident,
    events: Vec<EventDef>,
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

        Ok(AggregateInput {
            agg_name,
            entity_field,
            events,
        })
    }
}
