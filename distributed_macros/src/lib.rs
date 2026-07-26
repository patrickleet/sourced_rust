mod aggregate;
mod command_effects;
mod digest;
mod enqueue;
mod graphql_types;
mod read_model;
mod shared;
mod snapshot;
mod sourced;

use proc_macro::TokenStream;
use syn::DeriveInput;

/// Attribute macro that automatically queues a local event for emission.
#[proc_macro_attribute]
pub fn enqueue(attr: TokenStream, item: TokenStream) -> TokenStream {
    enqueue::expand_enqueue(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Attribute macro that automatically inserts a digest call at the beginning of a method.
#[proc_macro_attribute]
pub fn digest(attr: TokenStream, item: TokenStream) -> TokenStream {
    digest::expand_digest(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Generates the Aggregate trait impl with replay logic.
#[proc_macro]
pub fn aggregate(input: TokenStream) -> TokenStream {
    aggregate::expand_aggregate(input.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Attribute macro that generates a typed event enum, `TryFrom<&EventRecord>`,
/// and `impl Aggregate` from annotated methods in an impl block.
#[proc_macro_attribute]
pub fn sourced(attr: TokenStream, item: TokenStream) -> TokenStream {
    sourced::expand_sourced(attr.into(), item.into())
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

/// Derive macro for the `ReadModel` trait.
#[proc_macro_derive(
    ReadModel,
    attributes(readmodel, collection, table, column, id, index, unique)
)]
pub fn derive_read_model(input: TokenStream) -> TokenStream {
    read_model::derive_read_model(input)
}

/// Derive macro that generates a snapshot struct, `fn snapshot()`, and
/// `impl Snapshottable` for an aggregate.
#[proc_macro_derive(Snapshot, attributes(snapshot))]
pub fn derive_snapshot(input: TokenStream) -> TokenStream {
    snapshot::derive_snapshot(input)
}

/// Compile a portable, type-checked optimistic command-effect declaration.
#[proc_macro]
pub fn command_effects(input: TokenStream) -> TokenStream {
    command_effects::expand(input)
}

/// Compile declaration-owned generators for canonical command input fields.
#[proc_macro]
pub fn command_input_defaults(input: TokenStream) -> TokenStream {
    command_effects::expand_input_defaults(input)
}

/// Compile a finite, typed projector confirmation plan for one command input.
#[proc_macro]
pub fn command_confirmations(input: TokenStream) -> TokenStream {
    command_effects::expand_confirmations(input)
}

/// Derive `GraphqlInputType` for command mutation input structs.
#[proc_macro_derive(GraphqlInput, attributes(serde))]
pub fn derive_graphql_input(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match graphql_types::expand_graphql_input(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Derive `GraphqlOutputType` for command mutation output structs.
#[proc_macro_derive(GraphqlOutput, attributes(serde))]
pub fn derive_graphql_output(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match graphql_types::expand_graphql_output(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[cfg(test)]
mod entry_tests;
