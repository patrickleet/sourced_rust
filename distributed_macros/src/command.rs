use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, FnArg, Ident, ItemFn, LitStr, PathArguments, ReturnType, Token, Type};

#[derive(Default)]
struct CommandArgs {
    id: Option<LitStr>,
    field_name: Option<LitStr>,
    input: Option<Type>,
    outcome: Option<Type>,
    roles: Vec<LitStr>,
    emits: Vec<Type>,
    applies: Option<Expr>,
    defaults: Option<Expr>,
    generated_defaults: Vec<(Ident, Ident)>,
}

impl Parse for CommandArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Self::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            match key.to_string().as_str() {
                "roles" => {
                    let content;
                    syn::parenthesized!(content in input);
                    let values = Punctuated::<syn::Expr, Token![,]>::parse_terminated(&content)?;
                    for value in values {
                        match value {
                            syn::Expr::Path(path) if path.path.segments.len() == 1 => {
                                args.roles.push(LitStr::new(
                                    &path.path.segments[0].ident.to_string(),
                                    path.path.segments[0].ident.span(),
                                ));
                            }
                            syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Str(value),
                                ..
                            }) => args.roles.push(value),
                            other => {
                                return Err(syn::Error::new_spanned(
                                    other,
                                    "command roles must be identifiers or string literals",
                                ))
                            }
                        }
                    }
                }
                "emits" => {
                    let content;
                    syn::parenthesized!(content in input);
                    args.emits = Punctuated::<Type, Token![,]>::parse_terminated(&content)?
                        .into_iter()
                        .collect();
                }
                "applies" => {
                    let content;
                    syn::parenthesized!(content in input);
                    args.applies = Some(content.parse()?);
                }
                "defaults" | "input_defaults" => {
                    input.parse::<Token![=]>()?;
                    args.defaults = Some(input.parse()?);
                }
                "default" => {
                    let content;
                    syn::parenthesized!(content in input);
                    let target: Ident = content.parse()?;
                    content.parse::<Token![=]>()?;
                    let generator: Ident = content.parse()?;
                    if !content.is_empty() {
                        let arguments;
                        syn::parenthesized!(arguments in content);
                        if !arguments.is_empty() {
                            return Err(
                                arguments.error("input-default generators take no arguments")
                            );
                        }
                    }
                    if !matches!(generator.to_string().as_str(), "uuid_v7" | "ulid") {
                        return Err(syn::Error::new(
                            generator.span(),
                            "unknown input-default generator; expected uuid_v7() or ulid()",
                        ));
                    }
                    args.generated_defaults.push((target, generator));
                }
                "id" | "field" | "field_name" => {
                    input.parse::<Token![=]>()?;
                    let value: LitStr = input.parse()?;
                    if key == "id" {
                        args.id = Some(value);
                    } else {
                        args.field_name = Some(value);
                    }
                }
                "input" | "outcome" => {
                    input.parse::<Token![=]>()?;
                    let value: Type = input.parse()?;
                    if key == "input" {
                        args.input = Some(value);
                    } else {
                        args.outcome = Some(value);
                    }
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown command option `{other}`"),
                    ))
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}

pub fn expand(
    attr: proc_macro2::TokenStream,
    item: proc_macro2::TokenStream,
) -> syn::Result<TokenStream> {
    let args = syn::parse2::<CommandArgs>(attr)?;
    let function = syn::parse2::<ItemFn>(item)?;
    let id = args.id.ok_or_else(|| {
        syn::Error::new(
            function.sig.ident.span(),
            "command declaration requires `id = \"...\"`",
        )
    })?;
    if !function.sig.asyncness.is_some() {
        return Err(syn::Error::new_spanned(
            &function.sig.fn_token,
            "typed command handlers must be async",
        ));
    }
    let typed_args = function
        .sig
        .inputs
        .iter()
        .filter_map(|argument| match argument {
            FnArg::Typed(argument) => Some(argument),
            FnArg::Receiver(_) => None,
        })
        .collect::<Vec<_>>();
    if typed_args.len() != 2 {
        return Err(syn::Error::new_spanned(
            &function.sig.inputs,
            "typed command handlers must take context and input parameters",
        ));
    }
    let context_ty = &typed_args[0].ty;
    let context_type = quote!(#context_ty).to_string();
    if !context_type.contains("CausalCommandContext") {
        return Err(syn::Error::new_spanned(
            &typed_args[0].ty,
            "first typed command parameter must be CausalCommandContext",
        ));
    }
    let inferred_input = (*typed_args[1].ty).clone();
    let input = args.input.unwrap_or(inferred_input);
    let outcome = args
        .outcome
        .or_else(|| infer_prepared_outcome(&function.sig.output))
        .ok_or_else(|| {
            syn::Error::new_spanned(
                &function.sig.output,
                "command outcome is not inferable; provide `outcome = PreparedCommand<Outcome>`",
            )
        })?;
    let field_name = args.field_name.unwrap_or_else(|| {
        LitStr::new(
            &id.value()
                .chars()
                .map(|character| {
                    if matches!(character, '.' | '-') {
                        '_'
                    } else {
                        character
                    }
                })
                .collect::<String>(),
            id.span(),
        )
    });
    let function_name = &function.sig.ident;
    let command_name = format_ident!("{}_command", function_name);
    let spec_name = format_ident!("{}_spec", function_name);
    let command_static = format_ident!("{}_COMMAND", function_name.to_string().to_uppercase());
    let spec_static = format_ident!("{}_SPEC", function_name.to_string().to_uppercase());
    let mount_name = format_ident!("{}_mount", function_name);
    let mount_static = format_ident!("{}_MOUNT", function_name.to_string().to_uppercase());
    let accessor_name = format_ident!("{}_application_command", function_name);
    let visibility = &function.vis;
    let roles = args.roles;
    let emits = args.emits;
    let applies = args.applies;
    let defaults = args.defaults;
    let generated_defaults = args.generated_defaults;
    let mut builder = quote! {
        ::distributed::graphql::typed_command::<#input, #outcome>(#id)
            .field_name(#field_name)
    };
    if !roles.is_empty() {
        builder.extend(quote! { .roles([#(#roles),*]) });
    }
    if !emits.is_empty() {
        builder.extend(quote! { .emits(::distributed::events!(#(#emits),*)) });
    }
    if let Some(applies) = applies {
        builder.extend(quote! { .applies(#applies) });
    }
    if let Some(defaults) = defaults {
        builder.extend(quote! { .input_defaults(#defaults) });
    } else if !generated_defaults.is_empty() {
        let defaults = generated_defaults.iter().map(|(field, generator)| {
            quote! { default input.#field = #generator(); }
        });
        builder.extend(quote! {
            .input_defaults(::distributed::command_input_defaults! {
                input: #input;
                #(#defaults)*
            })
        });
    }
    Ok(quote! {
        #[allow(unexpected_cfgs)]
        #[cfg(feature = "application-runtime")]
        #function

        #visibility fn #command_name() -> ::distributed::graphql::TypedCommand<#input, #outcome> {
            #builder
        }

        #visibility static #command_static: ::std::sync::LazyLock<
            ::distributed::graphql::TypedCommand<#input, #outcome>
        > = ::std::sync::LazyLock::new(#command_name);

        #visibility fn #spec_name() -> ::distributed::application::ApplicationResult<
            ::distributed::application::CommandSpec
        > {
            (#command_static).spec()
        }

        #visibility static #spec_static: ::std::sync::LazyLock<
            ::distributed::application::CommandSpec
        > = ::std::sync::LazyLock::new(||
            #spec_name().unwrap_or_else(|error| panic!("invalid generated command spec: {error}"))
        );

        #visibility fn #accessor_name() -> &'static ::distributed::application::CommandSpec {
            &#spec_static
        }

        #[allow(unexpected_cfgs)]
        #[cfg(feature = "application-runtime")]
        #visibility fn #mount_name() -> ::distributed::application::CommandMount {
            ::distributed::application::CommandMount::from_handler(
                (#spec_static).clone(),
                #function_name,
            )
        }

        #[allow(unexpected_cfgs)]
        #[cfg(feature = "application-runtime")]
        #visibility static #mount_static: ::std::sync::LazyLock<
            ::distributed::application::CommandMount
        > = ::std::sync::LazyLock::new(#mount_name);
    })
}

fn infer_prepared_outcome(output: &ReturnType) -> Option<Type> {
    let ReturnType::Type(_, output) = output else {
        return None;
    };
    let Type::Path(path) = output.as_ref() else {
        return None;
    };
    let result = path.path.segments.last()?;
    if result.ident != "Result" {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &result.arguments else {
        return None;
    };
    let first = arguments.args.first()?;
    let syn::GenericArgument::Type(Type::Path(prepared)) = first else {
        return None;
    };
    let prepared = prepared.path.segments.last()?;
    if prepared.ident != "PreparedCommand" {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &prepared.arguments else {
        return None;
    };
    match arguments.args.first()? {
        syn::GenericArgument::Type(output) => Some(output.clone()),
        _ => None,
    }
}
