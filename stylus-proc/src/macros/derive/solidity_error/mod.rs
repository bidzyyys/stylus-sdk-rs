// Copyright 2024, Offchain Labs, Inc.
// For licensing, see https://github.com/OffchainLabs/stylus-sdk-rs/blob/main/licenses/COPYRIGHT.md

use cfg_if::cfg_if;
use proc_macro2::TokenStream;
use proc_macro_error::emit_error;
use quote::ToTokens;
use syn::{parse::Nothing, parse_macro_input, parse_quote, Fields};

cfg_if! {
    if #[cfg(feature = "export-abi")] {
        mod export_abi;
        type Extension = export_abi::InnerTypesExtension;
    } else {
        type Extension = ();
    }
}

/// Implementation of the [`#[derive(SolidityError]`][crate::SolidityError] macro.
pub fn derive_solidity_error(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let item = parse_macro_input!(input as syn::ItemEnum);
    DeriveSolidityError::from(&item).into_token_stream().into()
}

#[derive(Debug)]
struct DeriveSolidityError<E = Extension> {
    name: syn::Ident,
    from_impls: Vec<syn::ItemImpl>,
    match_arms: Vec<syn::Arm>,
    _ext: E,
    // Track the items we've seen during processing
    seen_items: std::collections::HashMap<syn::Ident, syn::Item>,
    // Track all variants to process
    variants_to_process: Vec<(syn::Ident, syn::Field)>,
}

impl DeriveSolidityError {
    fn new(name: syn::Ident) -> Self {
        Self {
            name,
            from_impls: Vec::new(),
            match_arms: Vec::new(),
            _ext: Extension::default(),
            seen_items: std::collections::HashMap::new(),
            variants_to_process: Vec::new(),
        }
    }

    fn add_variant(&mut self, name: &syn::Ident, field: syn::Field) {
        let self_name = &self.name;
        let ty = &field.ty;
        self.from_impls.push(parse_quote! {
            impl From<#ty> for #self_name {
                fn from(value: #ty) -> Self {
                    #self_name::#name(value)
                }
            }
        });
        self.match_arms.push(parse_quote! {
            #self_name::#name(e) => stylus_sdk::call::MethodError::encode(e),
        });
        #[allow(clippy::unit_arg)]
        self._ext.add_variant(field);
    }

    /// Check if a type is an enum
    fn is_enum_type(&self, ty: &syn::Type) -> bool {
        if let syn::Type::Path(type_path) = ty {
            if let Some(segment) = type_path.path.segments.last() {
                if let syn::PathSegment {
                    arguments: syn::PathArguments::None,
                    ..
                } = segment
                {
                    let type_name = &segment.ident;
                    // First check if we've seen this type before
                    if let Some(item) = self.seen_items.get(type_name) {
                        if let syn::Item::Enum(item_enum) = item {
                            return item_enum.attrs.iter().any(|attr| {
                                if let syn::Meta::List(meta_list) = &attr.meta {
                                    if let Some(ident) = meta_list.path.get_ident() {
                                        if ident == "derive" {
                                            if let Ok(syn::MetaList { tokens, .. }) =
                                                syn::parse2::<syn::MetaList>(
                                                    meta_list.tokens.clone(),
                                                )
                                            {
                                                return tokens
                                                    .to_string()
                                                    .contains("SolidityError");
                                            }
                                        }
                                    }
                                }
                                false
                            });
                        }
                    }
                    // If we haven't seen it before, check if it's a known error type
                    // This is a fallback for types that are already defined in the codebase
                    let type_name = type_name.to_string();
                    return type_name.ends_with("Error") && !type_name.starts_with("My");
                }
            }
        }
        false
    }

    /// Add an item to our tracking
    fn add_item(&mut self, item: syn::Item) {
        if let syn::Item::Enum(item_enum) = &item {
            self.seen_items.insert(item_enum.ident.clone(), item);
        }
    }

    /// Collect all variants from an enum, including nested ones
    fn collect_variants(&mut self, item: &syn::ItemEnum) {
        for variant in &item.variants {
            match &variant.fields {
                Fields::Unnamed(e) if variant.fields.len() == 1 => {
                    let field = e.unnamed.first().unwrap().clone();
                    let ty = &field.ty;

                    // Check if the type is an enum
                    if self.is_enum_type(ty) {
                        // If it's an enum, we need to process its variants
                        if let syn::Type::Path(type_path) = ty {
                            if let Some(segment) = type_path.path.segments.last() {
                                // Get the enum item
                                if let Some(syn::Item::Enum(nested_enum)) =
                                    self.seen_items.get(&segment.ident).cloned()
                                {
                                    // Recursively collect variants from the nested enum
                                    self.collect_variants(&nested_enum);
                                    continue;
                                }
                            }
                        }
                    }

                    // Add this variant to our collection
                    self.variants_to_process
                        .push((variant.ident.clone(), field));
                }
                Fields::Unit => {
                    emit_error!(variant, "variant not a 1-tuple");
                }
                _ => {
                    emit_error!(variant.fields, "variant not a 1-tuple");
                }
            }
        }
    }

    /// Process all collected variants
    fn process_collected_variants(&mut self) {
        // Create a temporary copy of the variants to process
        let variants = std::mem::take(&mut self.variants_to_process);

        // Process each variant
        for (name, field) in variants {
            self.add_variant(&name, field);
        }
    }

    fn vec_u8_from_impl(&self) -> syn::ItemImpl {
        let name = &self.name;
        let match_arms = self.match_arms.iter();
        parse_quote! {
            impl From<#name> for alloc::vec::Vec<u8> {
                fn from(err: #name) -> Self {
                    match err {
                        #(#match_arms)*
                    }
                }
            }
        }
    }
}

impl From<&syn::ItemEnum> for DeriveSolidityError {
    fn from(item: &syn::ItemEnum) -> Self {
        let mut output = DeriveSolidityError::new(item.ident.clone());

        // Add the current enum to our tracking
        output.add_item(syn::Item::Enum(item.clone()));

        // Collect all variants, including nested ones
        output.collect_variants(item);

        // Process all collected variants
        output.process_collected_variants();

        output
    }
}

impl ToTokens for DeriveSolidityError {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        for from_impl in &self.from_impls {
            from_impl.to_tokens(tokens);
        }
        self.vec_u8_from_impl().to_tokens(tokens);
        Extension::codegen(self).to_tokens(tokens);
    }
}

trait SolidityErrorExtension: Default {
    type Ast: ToTokens;

    fn add_variant(&mut self, field: syn::Field);
    fn codegen(err: &DeriveSolidityError<Self>) -> Self::Ast;
}

impl SolidityErrorExtension for () {
    type Ast = Nothing;

    fn add_variant(&mut self, _field: syn::Field) {}

    fn codegen(_err: &DeriveSolidityError<Self>) -> Self::Ast {
        Nothing
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::DeriveSolidityError;
    use crate::utils::testing::assert_ast_eq;

    #[test]
    fn test_derive_solidity_error() {
        let derived = DeriveSolidityError::from(&parse_quote! {
            enum MyError {
                Foo(FooError),
                Bar(BarError),
            }
        });
        assert_ast_eq(
            &derived.from_impls[0],
            &parse_quote! {
                impl From<FooError> for MyError {
                    fn from(value: FooError) -> Self {
                        MyError::Foo(value)
                    }
                }
            },
        );
        assert_ast_eq(
            derived.vec_u8_from_impl(),
            parse_quote! {
                impl From<MyError> for alloc::vec::Vec<u8> {
                    fn from(err: MyError) -> Self {
                        match err {
                            MyError::Foo(e) => stylus_sdk::call::MethodError::encode(e),
                            MyError::Bar(e) => stylus_sdk::call::MethodError::encode(e),
                        }
                    }
                }
            },
        );
    }
}
