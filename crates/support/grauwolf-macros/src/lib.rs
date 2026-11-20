//! proc macros `#[tool]` for tool implementations from functions and `#[toolset]` for several
//! tools from an `impl` block (ADR-0003, F03.5).

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{
    FnArg, Ident, ImplItem, ItemFn, ItemImpl, LitStr, Pat, ReturnType, Token, Type,
    parse_macro_input,
};

/// Turns a function `fn name(a: A, …) -> Result<T, ToolError>` (also `async`) into a tool.
///
/// Generates the type `Name` (the tool) and `NameArgs` (the arguments with a JSON schema). The
/// description of the tool comes from the doc comments of the function, that of the arguments from
/// `#[arg("…")]`.
///
/// ```ignore
/// /// Adds two numbers.
/// #[tool]
/// async fn add(#[arg("First number")] a: f64, #[arg("Second number")] b: f64) -> Result<f64, ToolError> {
///     Ok(a + b)
/// }
/// let registry = ToolRegistry::builder().tool(Add).build();
/// ```
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(Span::call_site(), "#[tool] takes no arguments")
            .to_compile_error()
            .into();
    }
    let function = parse_macro_input!(item as ItemFn);
    expand(function, &Paths::new("::grauwolf_tools"))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// `#[tool]` for the facade: the generated code refers to `::grauwolf::tools`, so that users
/// only need `grauwolf` (re-exported as `grauwolf::tool`; ADR-0003 addendum).
#[doc(hidden)]
#[proc_macro_attribute]
pub fn facade_tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(Span::call_site(), "#[tool] takes no arguments")
            .to_compile_error()
            .into();
    }
    let function = parse_macro_input!(item as ItemFn);
    expand(function, &Paths::new("::grauwolf::tools"))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Paths under which the generated code finds the tool crate and its helper crates.
struct Paths {
    krate: proc_macro2::TokenStream,
    serde: String,
    schemars: String,
}

impl Paths {
    fn new(krate: &str) -> Self {
        Self {
            krate: krate.parse().expect("valid path"),
            serde: format!("{krate}::__private::serde"),
            schemars: format!("{krate}::__private::schemars"),
        }
    }
}

fn expand(mut function: ItemFn, paths: &Paths) -> syn::Result<proc_macro2::TokenStream> {
    let (krate, serde_path, schemars_path) = (&paths.krate, &paths.serde, &paths.schemars);
    let name = function.sig.ident.clone();
    let tool_type = Ident::new(&pascal_case(&name.to_string()), name.span());
    let args_type = format_ident!("{}Args", tool_type);
    let description = doc_comment(&function.attrs);
    let output = ok_type(&function.sig.output)?;

    let (fields, arg_names) = arguments(&mut function.sig.inputs, false)?;
    let call = if function.sig.asyncness.is_some() {
        quote! { #name(#(args.#arg_names),*).await }
    } else {
        quote! { #name(#(args.#arg_names),*) }
    };
    let name_text = name.to_string();
    let args_doc = format!("Arguments of the tool `{name_text}`.");
    let tool_doc = format!("Tool `{name_text}`, generated with `#[tool]`.");

    Ok(quote! {
        #function

        #[doc = #args_doc]
        #[derive(#krate::__private::serde::Deserialize, #krate::__private::schemars::JsonSchema)]
        #[serde(crate = #serde_path)]
        #[schemars(crate = #schemars_path)]
        pub struct #args_type {
            #(#fields),*
        }

        #[doc = #tool_doc]
        #[derive(Debug, Clone, Copy, Default)]
        pub struct #tool_type;

        impl #krate::Tool for #tool_type {
            type Args = #args_type;
            type Output = #output;

            fn name(&self) -> &str {
                #name_text
            }

            fn description(&self) -> &str {
                #description
            }

            fn call(
                &self,
                args: Self::Args,
            ) -> #krate::__private::BoxFuture<'_, ::core::result::Result<Self::Output, #krate::ToolError>> {
                ::std::boxed::Box::pin(async move { #call })
            }
        }
    })
}

/// Turns every method of an `impl` block marked with `#[tool]` into a tool; the tools share the
/// object via `Arc<Self>` (shared state). Other methods stay unchanged.
///
/// Generates per tool `<Type><Method>` (holds an `Arc<Type>`) and `<Type><Method>Args`, as well as
/// `fn tools(self: Arc<Self>) -> Vec<Arc<dyn DynTool>>`. Tool methods take `&self`.
///
/// ```ignore
/// #[toolset]
/// impl Shop {
///     /// Adds an item to the shopping cart.
///     #[tool]
///     fn add_to_cart(&self, #[arg("item")] item: String) -> Result<String, ToolError> { … }
/// }
/// let registry = ToolRegistry::default().extended(Arc::new(shop).tools());
/// ```
#[proc_macro_attribute]
pub fn toolset(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(Span::call_site(), "#[toolset] takes no arguments")
            .to_compile_error()
            .into();
    }
    let block = parse_macro_input!(item as ItemImpl);
    expand_toolset(block, &Paths::new("::grauwolf_tools"))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// `#[toolset]` for the facade (re-exported as `grauwolf::toolset`; ADR-0003 addendum).
#[doc(hidden)]
#[proc_macro_attribute]
pub fn facade_toolset(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(Span::call_site(), "#[toolset] takes no arguments")
            .to_compile_error()
            .into();
    }
    let block = parse_macro_input!(item as ItemImpl);
    expand_toolset(block, &Paths::new("::grauwolf::tools"))
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand_toolset(mut block: ItemImpl, paths: &Paths) -> syn::Result<proc_macro2::TokenStream> {
    let (krate, serde_path, schemars_path) = (&paths.krate, &paths.serde, &paths.schemars);
    if let Some((_, path, _)) = &block.trait_ {
        return Err(syn::Error::new_spanned(
            path,
            "#[toolset] belongs on an inherent `impl` block, not on a trait implementation",
        ));
    }
    if !block.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &block.generics,
            "#[toolset] does not support generic `impl` blocks",
        ));
    }
    let self_type = block.self_ty.clone();
    let Type::Path(path) = self_type.as_ref() else {
        return Err(syn::Error::new_spanned(
            &self_type,
            "#[toolset] expects a named type",
        ));
    };
    let type_name = path
        .path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
        .unwrap_or_default();

    let mut generated = Vec::new();
    let mut tool_types = Vec::new();
    for item in &mut block.items {
        let ImplItem::Fn(method) = item else {
            continue;
        };
        let marked = method
            .attrs
            .iter()
            .any(|attribute| attribute.path().is_ident("tool"));
        if !marked {
            continue;
        }
        method
            .attrs
            .retain(|attribute| !attribute.path().is_ident("tool"));
        match method.sig.inputs.first() {
            Some(FnArg::Receiver(receiver))
                if receiver.reference.is_some() && receiver.mutability.is_none() => {}
            _ => {
                return Err(syn::Error::new_spanned(
                    &method.sig,
                    "tool methods in #[toolset] take `&self` (shared, not `&mut self`)",
                ));
            }
        }
        let name = method.sig.ident.clone();
        let pascal = pascal_case(&name.to_string());
        let tool_type = format_ident!("{}{}", type_name, pascal, span = name.span());
        let args_type = format_ident!("{}Args", tool_type);
        let description = doc_comment(&method.attrs);
        let output = ok_type(&method.sig.output)?;
        let (fields, arg_names) = arguments(&mut method.sig.inputs, true)?;
        let call = if method.sig.asyncness.is_some() {
            quote! { self.0.#name(#(args.#arg_names),*).await }
        } else {
            quote! { self.0.#name(#(args.#arg_names),*) }
        };
        let name_text = name.to_string();
        let args_doc = format!("Arguments of the tool `{name_text}`.");
        let tool_doc = format!("Tool `{name_text}` of `{type_name}`, generated with `#[toolset]`.");
        generated.push(quote! {
            #[doc = #args_doc]
            #[derive(#krate::__private::serde::Deserialize, #krate::__private::schemars::JsonSchema)]
            #[serde(crate = #serde_path)]
            #[schemars(crate = #schemars_path)]
            pub struct #args_type {
                #(#fields),*
            }

            #[doc = #tool_doc]
            #[derive(Clone)]
            pub struct #tool_type(pub ::std::sync::Arc<#self_type>);

            impl #krate::Tool for #tool_type {
                type Args = #args_type;
                type Output = #output;

                fn name(&self) -> &str {
                    #name_text
                }

                fn description(&self) -> &str {
                    #description
                }

                fn call(
                    &self,
                    args: Self::Args,
                ) -> #krate::__private::BoxFuture<'_, ::core::result::Result<Self::Output, #krate::ToolError>> {
                    ::std::boxed::Box::pin(async move { #call })
                }
            }
        });
        tool_types.push(tool_type);
    }

    Ok(quote! {
        #block

        #(#generated)*

        impl #self_type {
            /// All tools of this object (`#[toolset]`), in the order of the methods.
            pub fn tools(
                self: ::std::sync::Arc<Self>,
            ) -> ::std::vec::Vec<::std::sync::Arc<dyn #krate::DynTool>> {
                ::std::vec![
                    #(::std::sync::Arc::new(#tool_types(::std::sync::Arc::clone(&self)))
                        as ::std::sync::Arc<dyn #krate::DynTool>),*
                ]
            }
        }
    })
}

/// Fields of the argument struct and names of the arguments; `#[arg("…")]` becomes the
/// description and is removed. With `method`, the receiver `&self` is skipped.
fn arguments(
    inputs: &mut Punctuated<FnArg, Token![,]>,
    method: bool,
) -> syn::Result<(Vec<proc_macro2::TokenStream>, Vec<Ident>)> {
    let mut fields = Vec::new();
    let mut arg_names = Vec::new();
    for input in inputs.iter_mut() {
        let typed = match input {
            FnArg::Receiver(_) if method => continue,
            FnArg::Receiver(_) => {
                return Err(syn::Error::new_spanned(
                    input,
                    "#[tool] functions have no `self` – for methods use #[toolset] on the `impl` block",
                ));
            }
            FnArg::Typed(typed) => typed,
        };
        let Pat::Ident(pattern) = typed.pat.as_ref() else {
            return Err(syn::Error::new_spanned(
                &typed.pat,
                "arguments need simple names",
            ));
        };
        let arg_name = pattern.ident.clone();
        let mut docs = Vec::new();
        let mut kept = Vec::new();
        for attribute in typed.attrs.drain(..) {
            if attribute.path().is_ident("arg") {
                let text: LitStr = attribute.parse_args()?;
                docs.push(quote! { #[doc = #text] });
            } else {
                kept.push(attribute);
            }
        }
        typed.attrs = kept;
        let ty = &typed.ty;
        fields.push(quote! { #(#docs)* pub #arg_name: #ty });
        arg_names.push(arg_name);
    }
    Ok((fields, arg_names))
}

/// Joined doc comments (`///`), without leading spaces.
fn doc_comment(attributes: &[syn::Attribute]) -> String {
    attributes
        .iter()
        .filter(|attribute| attribute.path().is_ident("doc"))
        .filter_map(|attribute| match &attribute.meta {
            syn::Meta::NameValue(meta) => match &meta.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(text),
                    ..
                }) => Some(text.value().trim().to_owned()),
                _ => None,
            },
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `T` from the return type `Result<T, …>`.
fn ok_type(output: &ReturnType) -> syn::Result<Type> {
    let error =
        || syn::Error::new_spanned(output, "#[tool] functions return `Result<T, ToolError>`");
    let ReturnType::Type(_, ty) = output else {
        return Err(error());
    };
    let Type::Path(path) = ty.as_ref() else {
        return Err(error());
    };
    let segment = path.path.segments.last().ok_or_else(error)?;
    if segment.ident != "Result" {
        return Err(error());
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(error());
    };
    match arguments.args.first() {
        Some(syn::GenericArgument::Type(ok)) => Ok(ok.clone()),
        _ => Err(error()),
    }
}

/// `snake_case` → `PascalCase`.
fn pascal_case(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect()
}
