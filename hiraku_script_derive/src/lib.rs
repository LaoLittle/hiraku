use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Attribute, FnArg, ImplItem, ImplItemFn, ItemEnum, ItemImpl, Pat, ReturnType, Type,
    parse::{Parse, ParseStream},
    parse_macro_input,
};

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

            fn encode_hks_payload(self) -> ::hiraku_script::vm::Value {
                match self {
                    #( #encode_arms ),*
                }
            }

            fn decode_hks_payload(
                value: &::hiraku_script::vm::Value,
            ) -> Result<Self, ::hiraku_script::native::NativeError> {
                let ::hiraku_script::vm::Value::Tuple(fields) = value else {
                    return Err(::hiraku_script::native::NativeError::TypeMismatch(
                        stringify!(#type_name),
                    ));
                };
                let Some(::hiraku_script::vm::Value::Symbol(variant)) = fields.first() else {
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
                value: &::hiraku_script::vm::Value,
            ) -> Result<Self, ::hiraku_script::native::NativeError> {
                let ::hiraku_script::vm::Value::Typed { value, .. } = value else {
                    return Err(::hiraku_script::native::NativeError::TypeMismatch(
                        stringify!(#type_name),
                    ));
                };
                <Self as ::hiraku_script::native::HksNativeType>::decode_hks_payload(value)
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
        quote!(::hiraku_script::vm::StaticMemberKind::Getter)
    } else {
        quote!(::hiraku_script::vm::StaticMemberKind::Method)
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
            ::hiraku_script::vm::FunctionSignature {
                receiver: None,
                parameters: vec![ #( #parameter_types ),* ],
                result: ::hiraku_script::vm::ScriptType::Named(type_id),
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
        "f32" | "f64" => Ok(quote!(::hiraku_script::vm::ScriptType::Number)),
        "u8" | "u16" | "u32" | "i8" | "i16" | "i32" => {
            Ok(quote!(::hiraku_script::vm::ScriptType::Int))
        }
        "String" => Ok(quote!(::hiraku_script::vm::ScriptType::String)),
        "bool" => Ok(quote!(::hiraku_script::vm::ScriptType::Bool)),
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
            #type_name::#variant_name => ::hiraku_script::vm::Value::Tuple(vec![
                ::hiraku_script::vm::Value::Symbol(#public_name.to_string()),
            ])
        }),
        syn::Fields::Unnamed(fields) => {
            let bindings = (0..fields.unnamed.len())
                .map(|index| format_ident!("field_{index}"))
                .collect::<Vec<_>>();
            Ok(quote! {
                #type_name::#variant_name( #( #bindings ),* ) =>
                    ::hiraku_script::vm::Value::Tuple(vec![
                        ::hiraku_script::vm::Value::Symbol(#public_name.to_string()),
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
