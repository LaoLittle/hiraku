use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Attribute, DeriveInput, Expr, FnArg, ImplItem, ImplItemFn, Item, ItemEnum, ItemImpl, ItemMod,
    Lit, Meta, Pat, ReturnType, Type,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
    token::Comma,
};

#[proc_macro_derive(HksHandle, attributes(hks))]
pub fn derive_hks_handle(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_hks_handle(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand_hks_handle(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let syn::Data::Struct(item) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "HksHandle requires a tuple struct",
        ));
    };
    let syn::Fields::Unnamed(fields) = &item.fields else {
        return Err(syn::Error::new_spanned(
            &item.fields,
            "HksHandle requires a tuple struct",
        ));
    };
    if fields.unnamed.len() != 1 {
        return Err(syn::Error::new_spanned(
            &item.fields,
            "HksHandle requires one u64 field",
        ));
    }
    let Type::Path(field_type) = &fields.unnamed[0].ty else {
        return Err(syn::Error::new_spanned(
            &fields.unnamed[0].ty,
            "HksHandle field must be u64",
        ));
    };
    if !field_type.path.is_ident("u64") {
        return Err(syn::Error::new_spanned(
            &fields.unnamed[0].ty,
            "HksHandle field must be u64",
        ));
    }
    let mut public_name = name.to_string();
    let mut handle_type = None::<Expr>;
    for attribute in &input.attrs {
        if !attribute.path().is_ident("hks") {
            continue;
        }
        for meta in attribute.parse_args_with(Punctuated::<Meta, Comma>::parse_terminated)? {
            let Meta::NameValue(value) = meta else {
                return Err(syn::Error::new_spanned(meta, "invalid HksHandle option"));
            };
            if value.path.is_ident("name") {
                let Expr::Lit(expression) = value.value else {
                    return Err(syn::Error::new_spanned(value, "name requires a string"));
                };
                let Lit::Str(value) = expression.lit else {
                    return Err(syn::Error::new_spanned(
                        expression,
                        "name requires a string",
                    ));
                };
                public_name = value.value();
            } else if value.path.is_ident("handle_type") {
                handle_type = Some(value.value);
            } else {
                return Err(syn::Error::new_spanned(
                    value.path,
                    "unknown HksHandle option",
                ));
            }
        }
    }
    let handle_type = handle_type.ok_or_else(|| {
        syn::Error::new_spanned(input, "HksHandle requires #[hks(handle_type = ...)]")
    })?;
    Ok(quote! {
        impl ::hiraku_script::native::FromHksValue for #name {
            fn from_hks_value(
                value: &::hiraku_script::Value,
            ) -> Result<Self, ::hiraku_script::native::NativeError> {
                match value {
                    ::hiraku_script::Value::Handle { type_id, id }
                        if *type_id == (#handle_type) as u32 => Ok(Self(*id)),
                    _ => Err(::hiraku_script::native::NativeError::TypeMismatch(#public_name)),
                }
            }
        }

        impl ::hiraku_script::native::IntoHksValue for #name {
            fn into_hks_value(self) -> ::hiraku_script::Value {
                ::hiraku_script::Value::Handle {
                    type_id: (#handle_type) as u32,
                    id: self.0,
                }
            }
        }

        impl ::hiraku_script::native::HksScriptType for #name {
            fn hks_script_type<C>(
                registry: &mut ::hiraku_script::native::NativeRegistry<C>,
            ) -> ::hiraku_script::ScriptType {
                ::hiraku_script::ScriptType::Named(registry.define_type(#public_name))
            }
        }
    })
}

#[proc_macro_attribute]
pub fn hks_module(attribute: TokenStream, item: TokenStream) -> TokenStream {
    let namespace = if attribute.is_empty() {
        None
    } else {
        Some(parse_macro_input!(attribute as syn::LitStr).value())
    };
    let mut module = parse_macro_input!(item as ItemMod);
    match expand_module(&mut module, namespace.as_deref()) {
        Ok(registration) => {
            let (_, items) = module
                .content
                .as_mut()
                .expect("hks_module validates inline modules");
            items.push(syn::parse_quote!(#registration));
            quote!(#module).into()
        }
        Err(error) => error.into_compile_error().into(),
    }
}

#[derive(Default)]
struct FunctionOptions {
    name: Option<String>,
    receiver: Option<String>,
    result: Option<String>,
    selector: Option<String>,
    operator: Option<String>,
    raw: bool,
}

fn expand_module(
    module: &mut ItemMod,
    namespace: Option<&str>,
) -> syn::Result<proc_macro2::TokenStream> {
    let Some((_, items)) = module.content.as_mut() else {
        return Err(syn::Error::new_spanned(
            module,
            "#[hks_module] requires an inline module",
        ));
    };
    let mut context_type = None::<Type>;
    let mut registrations = Vec::new();
    for item in items.iter_mut() {
        let Item::Fn(function) = item else { continue };
        let Some(attribute_index) = function
            .attrs
            .iter()
            .position(|attribute| attribute.path().is_ident("hks"))
        else {
            continue;
        };
        let attribute = function.attrs.remove(attribute_index);
        let options = parse_function_options(&attribute)?;
        let context = function_context_type(&function.sig)?;
        if let Some(expected) = &context_type {
            if quote!(#expected).to_string() != quote!(#context).to_string() {
                return Err(syn::Error::new_spanned(
                    &function.sig,
                    "all functions in an HKS module must use the same context type",
                ));
            }
        } else {
            context_type = Some(context.clone());
        }
        registrations.push(register_module_function(function, &options, namespace)?);
    }
    let context = context_type.ok_or_else(|| {
        syn::Error::new_spanned(module, "#[hks_module] contains no #[hks] functions")
    })?;
    Ok(quote! {
        pub(super) fn register_hks(
            registry: &mut ::hiraku_script::native::NativeRegistry<#context>,
        ) -> Result<(), ::hiraku_script::native::RegistrationError> {
            #( #registrations )*
            Ok(())
        }
    })
}

fn parse_function_options(attribute: &Attribute) -> syn::Result<FunctionOptions> {
    if matches!(attribute.meta, Meta::Path(_)) {
        return Ok(FunctionOptions::default());
    }
    let metas = attribute.parse_args_with(Punctuated::<Meta, Comma>::parse_terminated)?;
    let mut options = FunctionOptions::default();
    for meta in metas {
        match meta {
            Meta::Path(path) if path.is_ident("raw") => options.raw = true,
            Meta::Path(path) if path.is_ident("receiver") => options.receiver = Some(String::new()),
            Meta::NameValue(value) => {
                let Some(key) = value.path.get_ident().map(ToString::to_string) else {
                    return Err(syn::Error::new_spanned(value.path, "invalid HKS option"));
                };
                let Expr::Lit(expression) = value.value else {
                    return Err(syn::Error::new_spanned(
                        value,
                        "HKS options require strings",
                    ));
                };
                let Lit::Str(value) = expression.lit else {
                    return Err(syn::Error::new_spanned(
                        expression,
                        "HKS options require strings",
                    ));
                };
                match key.as_str() {
                    "name" => options.name = Some(value.value()),
                    "receiver" => options.receiver = Some(value.value()),
                    "result" => options.result = Some(value.value()),
                    "selector" => options.selector = Some(value.value()),
                    "operator" => options.operator = Some(value.value()),
                    _ => return Err(syn::Error::new_spanned(value, "unknown HKS option")),
                }
            }
            meta => return Err(syn::Error::new_spanned(meta, "invalid HKS function option")),
        }
    }
    if options.operator.is_some() && options.selector.is_some() {
        return Err(syn::Error::new_spanned(
            attribute,
            "an HKS function cannot be both an operator and selector",
        ));
    }
    Ok(options)
}

fn function_context_type(signature: &syn::Signature) -> syn::Result<Type> {
    let Some(FnArg::Typed(argument)) = signature.inputs.first() else {
        return Err(syn::Error::new_spanned(
            signature,
            "HKS native functions require `&mut Context` as their first parameter",
        ));
    };
    let Type::Reference(reference) = argument.ty.as_ref() else {
        return Err(syn::Error::new_spanned(
            &argument.ty,
            "expected `&mut Context`",
        ));
    };
    if reference.mutability.is_none() {
        return Err(syn::Error::new_spanned(
            &argument.ty,
            "HKS context must be mutable",
        ));
    }
    Ok(reference.elem.as_ref().clone())
}

fn register_module_function(
    function: &syn::ItemFn,
    options: &FunctionOptions,
    namespace: Option<&str>,
) -> syn::Result<proc_macro2::TokenStream> {
    let rust_name = &function.sig.ident;
    let rust_name_string = rust_name.to_string();
    let public_name = options.name.clone().unwrap_or_else(|| {
        camel_case(
            rust_name_string
                .strip_prefix("native_")
                .unwrap_or(&rust_name_string),
        )
    });
    if options.raw {
        return Ok(if let Some(operator) = &options.operator {
            quote!(registry.register_operator_raw_fn(#operator, #rust_name)?;)
        } else if let Some(selector) = options.selector.as_deref().or_else(|| {
            if options.receiver.is_none() {
                namespace
            } else {
                None
            }
        }) {
            quote!(registry.register_selector_raw_fn(#selector, #public_name, #rust_name)?;)
        } else {
            quote!(registry.register_raw_fn(#public_name, #rust_name)?;)
        });
    }

    let script_arguments = function.sig.inputs.iter().skip(1).collect::<Vec<_>>();
    let receiver_count = usize::from(options.receiver.is_some());
    if script_arguments.len() < receiver_count {
        return Err(syn::Error::new_spanned(
            &function.sig,
            "missing HKS receiver parameter",
        ));
    }
    let parameters = script_arguments
        .iter()
        .skip(receiver_count)
        .map(|argument| match argument {
            FnArg::Typed(argument) => module_script_type(&argument.ty, None),
            FnArg::Receiver(receiver) => {
                Err(syn::Error::new_spanned(receiver, "self is unsupported"))
            }
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let receiver = if let Some(name) = &options.receiver {
        let receiver = if name.is_empty() {
            let FnArg::Typed(argument) = script_arguments[0] else {
                unreachable!();
            };
            module_script_type(&argument.ty, None)?
        } else {
            named_script_type(name)
        };
        quote!(Some(#receiver))
    } else {
        quote!(None)
    };
    let result_type = return_ok_type(&function.sig.output)?;
    let result = module_script_type(result_type, options.result.as_deref())?;
    let registration = if let Some(selector) = options.selector.as_deref().or_else(|| {
        if options.receiver.is_none() {
            namespace
        } else {
            None
        }
    }) {
        quote!(registry.register_selector_fn(#selector, #public_name, #rust_name)?)
    } else {
        quote!(registry.register_fn(#public_name, #rust_name)?)
    };
    Ok(quote! {
        let builtin = #registration;
        let signature = ::hiraku_script::FunctionSignature {
            receiver: #receiver,
            parameters: vec![ #( #parameters ),* ],
            variadic: None,
            result: #result,
        };
        registry.set_signature(builtin, signature)?;
    })
}

fn return_ok_type(output: &ReturnType) -> syn::Result<&Type> {
    let ReturnType::Type(_, ty) = output else {
        return Err(syn::Error::new_spanned(
            output,
            "HKS functions must return Result<T, NativeError>",
        ));
    };
    let Type::Path(path) = ty.as_ref() else {
        return Err(syn::Error::new_spanned(ty, "unsupported HKS return type"));
    };
    let segment = path
        .path
        .segments
        .last()
        .expect("return path has a segment");
    if segment.ident != "Result" {
        return Err(syn::Error::new_spanned(
            ty,
            "HKS functions must return Result<T, NativeError>",
        ));
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Err(syn::Error::new_spanned(
            ty,
            "Result requires type arguments",
        ));
    };
    match arguments.args.first() {
        Some(syn::GenericArgument::Type(ty)) => Ok(ty),
        _ => Err(syn::Error::new_spanned(ty, "Result requires an ok type")),
    }
}

fn module_script_type(
    ty: &Type,
    override_name: Option<&str>,
) -> syn::Result<proc_macro2::TokenStream> {
    if let Some(name) = override_name {
        return Ok(named_script_type(name));
    }
    Ok(quote!(
        <#ty as ::hiraku_script::native::HksScriptType>::hks_script_type(registry)
    ))
}

fn named_script_type(name: &str) -> proc_macro2::TokenStream {
    quote!(::hiraku_script::ScriptType::Named(registry.define_type(#name)))
}

fn camel_case(name: &str) -> String {
    let mut result = String::new();
    let mut uppercase = false;
    for character in name.chars() {
        if character == '_' {
            uppercase = true;
        } else if uppercase {
            result.extend(character.to_uppercase());
            uppercase = false;
        } else {
            result.push(character);
        }
    }
    result
}

struct NativeTypeDefinition {
    item: ItemEnum,
    implementation: ItemImpl,
}

impl Parse for NativeTypeDefinition {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let item = input.parse()?;
        let implementation = input.parse()?;
        if !input.is_empty() {
            return Err(input.error("hks_define! accepts one enum followed by one impl block"));
        }
        Ok(Self {
            item,
            implementation,
        })
    }
}

#[proc_macro]
pub fn hks_define(input: TokenStream) -> TokenStream {
    let NativeTypeDefinition {
        item,
        mut implementation,
    } = parse_macro_input!(input as NativeTypeDefinition);
    match expand(item, &mut implementation) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.into_compile_error().into(),
    }
}

fn expand(item: ItemEnum, implementation: &mut ItemImpl) -> syn::Result<proc_macro2::TokenStream> {
    let type_name = &item.ident;
    let Type::Path(impl_type) = implementation.self_ty.as_ref() else {
        return Err(syn::Error::new_spanned(
            &implementation.self_ty,
            "HKS impl target must be a named type",
        ));
    };
    if impl_type.path.segments.last().map(|segment| &segment.ident) != Some(type_name) {
        return Err(syn::Error::new_spanned(
            &implementation.self_ty,
            "impl target must match the enum declared by hks_define!",
        ));
    }

    let encode_arms = item
        .variants
        .iter()
        .map(|variant| encode_variant(type_name, variant))
        .collect::<syn::Result<Vec<_>>>()?;
    let decode_arms = item
        .variants
        .iter()
        .map(|variant| decode_variant(type_name, variant))
        .collect::<syn::Result<Vec<_>>>()?;

    let mut registrations = Vec::new();
    for impl_item in &mut implementation.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        let getter = take_marker(&mut method.attrs, "getter");
        registrations.push(register_method(type_name, method, getter)?);
    }

    Ok(quote! {
        #item
        #implementation

        impl ::hiraku_script::native::HksNativeType for #type_name {
            const HKS_TYPE_NAME: &'static str = stringify!(#type_name);

            fn encode_hks_payload(self) -> ::hiraku_script::Value {
                match self {
                    #( #encode_arms ),*
                }
            }

            fn decode_hks_payload(
                value: &::hiraku_script::Value,
            ) -> Result<Self, ::hiraku_script::native::NativeError> {
                let ::hiraku_script::Value::Tuple(fields) = value else {
                    return Err(::hiraku_script::native::NativeError::TypeMismatch(
                        stringify!(#type_name),
                    ));
                };
                let Some(::hiraku_script::Value::Symbol(variant)) = fields.first() else {
                    return Err(::hiraku_script::native::NativeError::TypeMismatch(
                        stringify!(#type_name),
                    ));
                };
                match variant.as_str() {
                    #( #decode_arms ),*,
                    _ => Err(::hiraku_script::native::NativeError::message(format!(
                        "unknown {} variant `{variant}`",
                        stringify!(#type_name),
                    ))),
                }
            }
        }

        impl ::hiraku_script::native::FromHksValue for #type_name {
            fn from_hks_value(
                value: &::hiraku_script::Value,
            ) -> Result<Self, ::hiraku_script::native::NativeError> {
                let ::hiraku_script::Value::Typed { value, .. } = value else {
                    return Err(::hiraku_script::native::NativeError::TypeMismatch(
                        stringify!(#type_name),
                    ));
                };
                <Self as ::hiraku_script::native::HksNativeType>::decode_hks_payload(value)
            }
        }

        impl ::hiraku_script::native::HksScriptType for #type_name {
            fn hks_script_type<C>(
                registry: &mut ::hiraku_script::native::NativeRegistry<C>,
            ) -> ::hiraku_script::ScriptType {
                ::hiraku_script::ScriptType::Named(
                    registry.define_type(stringify!(#type_name)),
                )
            }
        }

        impl #type_name {
            pub fn register_hks<C: 'static>(
                registry: &mut ::hiraku_script::native::NativeRegistry<C>,
            ) -> Result<::hiraku_script::SymbolId, ::hiraku_script::native::RegistrationError> {
                let type_id = registry.define_type(stringify!(#type_name));
                #( #registrations )*
                Ok(type_id)
            }
        }
    })
}

fn take_marker(attributes: &mut Vec<Attribute>, name: &str) -> bool {
    let found = attributes
        .iter()
        .any(|attribute| attribute.path().is_ident(name));
    attributes.retain(|attribute| !attribute.path().is_ident(name));
    found
}

fn register_method(
    type_name: &syn::Ident,
    method: &ImplItemFn,
    getter: bool,
) -> syn::Result<proc_macro2::TokenStream> {
    if method.sig.receiver().is_some() {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "the first hks_define! version supports static methods only",
        ));
    }
    let ReturnType::Type(_, result) = &method.sig.output else {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "registered methods must return the declared HKS type",
        ));
    };
    let fallible = registered_return_is_fallible(type_name, result)?;

    let method_name = &method.sig.ident;
    let public_name = method_name.to_string();
    let mut parameter_types = Vec::new();
    let mut conversions = Vec::new();
    let mut arguments = Vec::new();
    for (index, argument) in method.sig.inputs.iter().enumerate() {
        let FnArg::Typed(argument) = argument else {
            unreachable!();
        };
        let Pat::Ident(pattern) = argument.pat.as_ref() else {
            return Err(syn::Error::new_spanned(
                &argument.pat,
                "HKS parameters must be identifiers",
            ));
        };
        let name = &pattern.ident;
        let value = format_ident!("value_{index}");
        let ty = &argument.ty;
        parameter_types.push(script_type(ty)?);
        conversions.push(quote! {
            let #value = call.arguments.get(#index).ok_or(
                ::hiraku_script::native::NativeError::Arity {
                    expected: expected_arity,
                    actual: call.arguments.len(),
                },
            )?;
            let #name = <#ty as ::hiraku_script::native::FromHksValue>::from_hks_value(
                &#value.value,
            )?;
        });
        arguments.push(name.clone());
    }
    if getter && !parameter_types.is_empty() {
        return Err(syn::Error::new_spanned(
            &method.sig,
            "#[getter] methods cannot have parameters",
        ));
    }
    let expected_arity = parameter_types.len();
    let kind = if getter {
        quote!(::hiraku_script::StaticMemberKind::Getter)
    } else {
        quote!(::hiraku_script::StaticMemberKind::Method)
    };

    let invoke = if fallible {
        quote!(let result = #type_name::#method_name( #( #arguments ),* )?;)
    } else {
        quote!(let result = #type_name::#method_name( #( #arguments ),* );)
    };

    Ok(quote! {
        registry.register_static_raw_fn(
            type_id,
            #public_name,
            ::hiraku_script::FunctionSignature {
                receiver: None,
                parameters: vec![ #( #parameter_types ),* ],
                variadic: None,
                result: ::hiraku_script::ScriptType::Named(type_id),
            },
            #kind,
            move |_context, call| {
                let expected_arity = #expected_arity;
                if call.arguments.len() != expected_arity {
                    return Err(::hiraku_script::native::NativeError::Arity {
                        expected: expected_arity,
                        actual: call.arguments.len(),
                    });
                }
                #( #conversions )*
                #invoke
                Ok(::hiraku_script::native::HksNativeType::into_hks_typed(
                    result,
                    type_id,
                ))
            },
        )?;
    })
}

fn registered_return_is_fallible(type_name: &syn::Ident, ty: &Type) -> syn::Result<bool> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new_spanned(ty, "unsupported return type"));
    };
    let segment = path.path.segments.last().expect("type path has a segment");
    if segment.ident == *type_name || segment.ident == "Self" {
        return Ok(false);
    }
    if segment.ident == "Result"
        && let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments
        && let Some(syn::GenericArgument::Type(Type::Path(ok_type))) = arguments.args.first()
        && ok_type
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == *type_name || segment.ident == "Self")
    {
        return Ok(true);
    }
    Err(syn::Error::new_spanned(
        ty,
        "registered methods must return the declared HKS type or Result<that type, NativeError>",
    ))
}

fn script_type(ty: &Type) -> syn::Result<proc_macro2::TokenStream> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new_spanned(
            ty,
            "unsupported HKS parameter type",
        ));
    };
    let Some(name) = path
        .path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
    else {
        return Err(syn::Error::new_spanned(
            ty,
            "unsupported HKS parameter type",
        ));
    };
    match name.as_str() {
        "f32" | "f64" => Ok(quote!(::hiraku_script::ScriptType::Float)),
        "u8" | "u16" | "u32" | "i8" | "i16" | "i32" => Ok(quote!(::hiraku_script::ScriptType::Int)),
        "String" => Ok(quote!(::hiraku_script::ScriptType::String)),
        "bool" => Ok(quote!(::hiraku_script::ScriptType::Bool)),
        _ => Err(syn::Error::new_spanned(
            ty,
            "the first hks_define! version supports numeric, String and bool parameters",
        )),
    }
}

fn encode_variant(
    type_name: &syn::Ident,
    variant: &syn::Variant,
) -> syn::Result<proc_macro2::TokenStream> {
    let variant_name = &variant.ident;
    let public_name = variant_name.to_string();
    match &variant.fields {
        syn::Fields::Unit => Ok(quote! {
            #type_name::#variant_name => ::hiraku_script::Value::Tuple(vec![
                ::hiraku_script::Value::Symbol(#public_name.to_string()),
            ])
        }),
        syn::Fields::Unnamed(fields) => {
            let bindings = (0..fields.unnamed.len())
                .map(|index| format_ident!("field_{index}"))
                .collect::<Vec<_>>();
            Ok(quote! {
                #type_name::#variant_name( #( #bindings ),* ) =>
                    ::hiraku_script::Value::Tuple(vec![
                        ::hiraku_script::Value::Symbol(#public_name.to_string()),
                        #( ::hiraku_script::native::IntoHksValue::into_hks_value(#bindings) ),*
                    ])
            })
        }
        syn::Fields::Named(_) => Err(syn::Error::new_spanned(
            &variant.fields,
            "the first hks_define! version supports unit and tuple enum variants",
        )),
    }
}

fn decode_variant(
    type_name: &syn::Ident,
    variant: &syn::Variant,
) -> syn::Result<proc_macro2::TokenStream> {
    let variant_name = &variant.ident;
    let public_name = variant_name.to_string();
    match &variant.fields {
        syn::Fields::Unit => Ok(quote! {
            #public_name if fields.len() == 1 => Ok(#type_name::#variant_name)
        }),
        syn::Fields::Unnamed(fields) => {
            let conversions = fields
                .unnamed
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    let index = index + 1;
                    let ty = &field.ty;
                    quote! {
                        <#ty as ::hiraku_script::native::FromHksValue>::from_hks_value(
                            fields.get(#index).ok_or(
                                ::hiraku_script::native::NativeError::TypeMismatch(
                                    stringify!(#type_name),
                                ),
                            )?,
                        )?
                    }
                })
                .collect::<Vec<_>>();
            let length = fields.unnamed.len() + 1;
            Ok(quote! {
                #public_name if fields.len() == #length =>
                    Ok(#type_name::#variant_name( #( #conversions ),* ))
            })
        }
        syn::Fields::Named(_) => Err(syn::Error::new_spanned(
            &variant.fields,
            "the first hks_define! version supports unit and tuple enum variants",
        )),
    }
}
