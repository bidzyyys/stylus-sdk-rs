// Copyright 2023-2024, Offchain Labs, Inc.
// For licensing, see https://github.com/OffchainLabs/stylus-sdk-rs/blob/main/licenses/COPYRIGHT.md

use proc_macro2::TokenStream;
use proc_macro_error::emit_error;
use quote::{quote, ToTokens};
use syn::{
    parse_macro_input, parse_quote, punctuated::Punctuated, spanned::Spanned, ItemStruct, Token,
    Type,
};

use crate::utils::attrs::consume_flag;

/// Implementation of the [`#[storage]`][crate::storage] macro.
pub fn storage(
    attr: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    if !attr.is_empty() {
        emit_error!(
            TokenStream::from(attr).span(),
            "this macro is not configurable"
        );
    }

    let mut item = parse_macro_input!(input as ItemStruct);
    let storage = Storage::from(&mut item);
    let mut output = item.into_token_stream();
    storage.to_tokens(&mut output);
    output.into()
}

#[derive(Debug)]
struct Storage {
    name: syn::Ident,
    generics: syn::Generics,
    fields: Vec<StorageField>,
}

impl Storage {
    fn item_impl(&self) -> syn::ItemImpl {
        let name = &self.name;
        let (impl_generics, ty_generics, where_clause) = self.generics.split_for_impl();
        let size = TokenStream::from_iter(self.fields.iter().map(StorageField::size));
        parse_quote! {
            impl #impl_generics #name #ty_generics #where_clause {
                const fn required_slots() -> usize {
                    use stylus_sdk::storage;
                    let mut total: usize = 0;
                    let mut space: usize = 32;
                    #size
                    if space != 32 || total == 0 {
                        total += 1;
                    }
                    total
                }
            }
        }
    }

    fn impl_storage_type(&self) -> syn::ItemImpl {
        let name = &self.name;
        let (impl_generics, ty_generics, where_clause) = self.generics.split_for_impl();
        let init = Punctuated::<syn::FieldValue, Token![,]>::from_iter(
            self.fields.iter().filter_map(StorageField::init),
        );
        parse_quote! {
            impl #impl_generics stylus_sdk::storage::StorageType<H> for #name #ty_generics #where_clause {
                type Wraps<'a> = stylus_sdk::storage::StorageGuard<'a, Self> where Self: 'a;
                type WrapsMut<'a> = stylus_sdk::storage::StorageGuardMut<'a, Self> where Self: 'a;

                // start a new word
                const SLOT_BYTES: usize = 32;
                const REQUIRED_SLOTS: usize = Self::required_slots();

                unsafe fn new(mut root: stylus_sdk::alloy_primitives::U256, offset: u8, host: Rc<H>) -> Self {
                    use stylus_sdk::{storage, alloy_primitives};
                    debug_assert!(offset == 0);

                    let mut space: usize = 32;
                    let mut slot: usize = 0;
                    let accessor = Self {
                        #init,
                        host,
                    };
                    accessor
                }

                fn load<'s>(self) -> Self::Wraps<'s> {
                    stylus_sdk::storage::StorageGuard::new(self)
                }

                fn load_mut<'s>(self) -> Self::WrapsMut<'s> {
                    stylus_sdk::storage::StorageGuardMut::new(self)
                }
            }
        }
    }
}

impl From<&mut syn::ItemStruct> for Storage {
    fn from(node: &mut syn::ItemStruct) -> Self {
        let name = node.ident.clone();
        let generics = node.generics.clone();
        let fields = node
            .fields
            .iter_mut()
            .enumerate()
            .filter_map(|(idx, field)| {
                if let syn::Type::Path(..) = &field.ty {
                    Some(StorageField::new(idx, field))
                } else {
                    None
                }
            })
            .collect();
        Self {
            name,
            generics,
            fields,
        }
    }
}

impl ToTokens for Storage {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.item_impl().to_tokens(tokens);
        self.impl_storage_type().to_tokens(tokens);
        for field in &self.fields {
            field.impl_borrow(&self.name).to_tokens(tokens);
            field.impl_borrow_mut(&self.name).to_tokens(tokens);
        }
    }
}

#[derive(Debug)]
struct StorageField {
    name: Option<syn::Ident>,
    ty: syn::Type,
    accessor: syn::Member,
    borrow: bool,
}

impl StorageField {
    fn new(idx: usize, field: &mut syn::Field) -> Self {
        check_type(field);
        let name = field.ident.clone();
        let ty = field.ty.clone();
        let accessor = field
            .ident
            .clone()
            .map(syn::Member::from)
            .unwrap_or_else(|| idx.into());

        let borrow = consume_flag(&mut field.attrs, "borrow");
        Self {
            name,
            ty,
            accessor,
            borrow,
        }
    }

    fn init(&self) -> Option<syn::FieldValue> {
        let Some(ident) = &self.name else {
            return None;
        };
        let ty = &self.ty;
        Some(parse_quote! {
            #ident: {
                let bytes = <#ty as storage::StorageType<H>>::SLOT_BYTES;
                let words = <#ty as storage::StorageType<H>>::REQUIRED_SLOTS;
                if space < bytes {
                    space = 32;
                    slot += 1;
                }
                space -= bytes;

                let root = root + alloy_primitives::U256::from(slot);
                let field = <#ty as storage::StorageType<H>>::new(root, space as u8, Rc::clone(&self.host));
                if words > 0 {
                    slot += words;
                    space = 32;
                }
                field
            }
        })
    }

    fn size(&self) -> TokenStream {
        let ty = &self.ty;
        quote! {
            let bytes = <#ty as storage::StorageType<H>>::SLOT_BYTES;
            let words = <#ty as storage::StorageType<H>>::REQUIRED_SLOTS;

            if words > 0 {
                total += words;
                space = 32;
            } else {
                if space < bytes {
                    space = 32;
                    total += 1;
                }
                space -= bytes;
            }
        }
    }

    fn impl_borrow(&self, name: &syn::Ident) -> Option<syn::ItemImpl> {
        let Self { ty, accessor, .. } = self;
        self.borrow.then(|| {
            parse_quote! {
                impl core::borrow::Borrow<#ty> for #name {
                    fn borrow(&self) -> &#ty {
                        &self.#accessor
                    }
                }
            }
        })
    }

    fn impl_borrow_mut(&self, name: &syn::Ident) -> Option<syn::ItemImpl> {
        let Self { ty, accessor, .. } = self;
        self.borrow.then(|| {
            parse_quote! {
                impl core::borrow::BorrowMut<#ty> for #name {
                    fn borrow_mut(&mut self) -> &mut #ty {
                        &mut self.#accessor
                    }
                }
            }
        })
    }
}

fn check_type(field: &syn::Field) {
    let Type::Path(ty) = &field.ty else {
        unreachable!();
    };

    let path = &ty.path.segments.last().unwrap().ident;
    let not_supported = format!("Type `{path}` not supported for EVM state storage");

    match path.to_string().as_str() {
        x @ ("u8" | "u16" | "u32" | "u64" | "u128" | "i8" | "i16" | "i32" | "i64" | "i128"
        | "U8" | "U16" | "U32" | "U64" | "U128" | "I8" | "I16" | "I32" | "I64" | "I128") => {
            emit_error!(
                &field,
                "{not_supported}. Instead try `Storage{}`.",
                x.to_uppercase()
            );
        }
        "usize" => emit_error!(&field, "{not_supported}."),
        "isize" => emit_error!(&field, "{not_supported}."),
        "bool" => emit_error!(&field, "{not_supported}. Instead try `StorageBool`."),
        "f32" | "f64" => emit_error!(&field, "{not_supported}. Consider fixed-point arithmetic."),
        _ => {}
    }
}
