use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, Ident, LitStr, Token, Type, Visibility};

struct ApplicationInput {
    visibility: Visibility,
    name: Ident,
    id: LitStr,
    modules: Vec<Expr>,
    surfaces: Vec<Expr>,
    capabilities: Vec<Expr>,
    extensions: Vec<Expr>,
}

impl Parse for ApplicationInput {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let visibility: Visibility = input.parse()?;
        let name: Ident = input.parse()?;
        if input.peek(Token![:]) {
            input.parse::<Token![:]>()?;
            let _: Type = input.parse()?;
        }
        if input.peek(Token![=]) {
            input.parse::<Token![=]>()?;
        }
        let content;
        syn::braced!(content in input);
        let mut output = Self {
            visibility,
            name,
            id: LitStr::new("", proc_macro2::Span::call_site()),
            modules: Vec::new(),
            surfaces: Vec::new(),
            capabilities: Vec::new(),
            extensions: Vec::new(),
        };
        while !content.is_empty() {
            let field: Ident = content.parse()?;
            content.parse::<Token![:]>()?;
            match field.to_string().as_str() {
                "id" => output.id = content.parse()?,
                "modules" => output.modules = parse_expr_array(&content)?,
                "surfaces" => output.surfaces = parse_expr_array(&content)?,
                "capabilities" | "required_capabilities" => {
                    output.capabilities = parse_expr_array(&content)?
                }
                "extensions" => output.extensions = parse_expr_array(&content)?,
                other => {
                    return Err(syn::Error::new(
                        field.span(),
                        format!("unknown application field `{other}`"),
                    ))
                }
            }
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }
        if output.id.value().is_empty() {
            output.id = LitStr::new(
                &output.name.to_string().to_ascii_lowercase(),
                output.name.span(),
            );
        }
        Ok(output)
    }
}

fn parse_expr_array(input: ParseStream<'_>) -> syn::Result<Vec<Expr>> {
    let content;
    syn::bracketed!(content in input);
    Ok(Punctuated::<Expr, Token![,]>::parse_terminated(&content)?
        .into_iter()
        .collect())
}

pub fn expand(input: proc_macro2::TokenStream) -> syn::Result<TokenStream> {
    let framework = crate::shared::framework_path()?;
    let input = syn::parse2::<ApplicationInput>(input)?;
    let ApplicationInput {
        visibility,
        name,
        id,
        modules,
        surfaces,
        capabilities,
        extensions,
    } = input;
    let accessor = format_ident!("{}", name.to_string().to_lowercase());
    let modules = modules.iter().map(|value| quote! { (&*#value).clone() });
    let surfaces = surfaces.iter().map(|value| quote! { (&*#value).clone() });
    let capabilities = capabilities
        .iter()
        .map(|value| quote! { #value })
        .collect::<Vec<_>>();
    let extensions = extensions
        .iter()
        .map(|value| quote! { (&*#value).clone() })
        .collect::<Vec<_>>();
    let capability_builder = if capabilities.is_empty() {
        quote! {}
    } else {
        quote! { .required_capabilities([#(#capabilities),*]) }
    };
    Ok(quote! {
        #visibility static #name: ::std::sync::LazyLock<#framework::application::Application> =
            ::std::sync::LazyLock::new(|| {
                #framework::application::Application::new(#id)
                    .modules([#(#modules),*])
                    .surfaces([#(#surfaces),*])
                    .extensions([#(#extensions),*])
                    #capability_builder
                    .build()
                    .unwrap_or_else(|error| panic!("invalid generated application: {error}"))
            });

        #visibility fn #accessor() -> &'static #framework::application::Application {
            &#name
        }
    })
}
