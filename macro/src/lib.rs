// lint me harder
#![forbid(unsafe_code, non_ascii_idents)]
#![deny(
    future_incompatible,
    keyword_idents,
    elided_lifetimes_in_paths,
    noop_method_call,
    unused_lifetimes,
    unused_qualifications,
    clippy::wildcard_dependencies,
    clippy::debug_assert_with_mut_call,
    clippy::empty_line_after_outer_attr,
    clippy::panic,
    clippy::unwrap_used,
    clippy::useless_let_if_seq
)]
#![allow(clippy::pedantic)]

use proc_macro_crate::{crate_name, FoundCrate};
use proc_macro_error::{abort_call_site, proc_macro_error};
use quote::{format_ident, quote};

#[proc_macro_error]
#[proc_macro_attribute]
pub fn flaky(
    attr: proc_macro::TokenStream,
    body: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    if !attr.is_empty() {
        abort_call_site!("#[flaky] doesn't currently take arguments");
    }

    let self_crate = match crate_name("mark-flaky-tests") {
        Ok(FoundCrate::Itself) => format_ident!("mark_flaky_tests"),
        Ok(FoundCrate::Name(name)) => format_ident!("{name}"),
        Err(err) => abort_call_site!("can't find crate name for `mark_flaky_tests`: {}", err),
    };
    let mut func = syn::parse_macro_input!(body as syn::ItemFn);
    let name = &func.sig.ident;

    let args = &func.sig.inputs;
    let return_ty = &func.sig.output;

    let mut test = None;
    let mut tokio = None;
    let mut sqlx = None;
    let mut attrs = Vec::new();
    func.attrs.retain(|attr| {
        let path = attr.path();
        if path.get_ident().is_some_and(|name| name == "test") {
            test = Some(attr.clone());
            false
        } else if path.segments.len() == 2
            && path.segments[0].ident == "tokio"
            && path.segments[1].ident == "test"
        {
            tokio = Some(attr.clone());
            false
        } else if path.segments.len() == 2
            && path.segments[0].ident == "sqlx"
            && path.segments[1].ident == "test"
        {
            sqlx = Some(attr.clone());
            false
        } else if path.get_ident().is_some_and(|name| name == "ignore")
            || path.get_ident().is_some_and(|name| name == "should_panic")
        {
            attrs.push(attr.clone());
            false
        } else {
            true
        }
    });

    let (test_attr, catch_unwind, async_, await_, inputs) = match (tokio, sqlx, test) {
        (Some(attr), None, None) => (
            quote!(#attr),
            quote!(::#self_crate::_priv::futures::future::FutureExt::catch_unwind(#name())),
            quote!(async),
            quote!(.await),
            quote!(),
        ),
        (None, Some(attr), None) => {
            let pool = match args.first().expect("pool") {
                syn::FnArg::Typed(pat) => match &*pat.pat {
                    syn::Pat::Ident(pat) => &pat.ident,
                    _ => unreachable!("unsupported pattern"),
                },
                _ => unreachable!("unsupported pattern"),
            };

            (
                quote!(#attr),
                quote!(::#self_crate::_priv::futures::future::FutureExt::catch_unwind(::std::panic::AssertUnwindSafe(#name(#pool .clone())))),
                quote!(async),
                quote!(.await),
                quote!(#pool.clone()),
            )
        }
        (None, None, Some(attr)) => (
            quote!(#attr),
            quote!(::std::panic::catch_unwind(#name)),
            quote!(),
            quote!(),
            quote!(),
        ),
        (None, None, None) => (
            quote!(),
            quote!(::std::panic::catch_unwind(#name)),
            quote!(),
            quote!(),
            quote!(),
        ),
        _ => unreachable!("at most one of `tokio::test`, `sqlx::test`, or `test` attribute is allowed at a time"),
    };

    quote! {
        #test_attr
        #(#attrs)*
        #async_ fn #name(#args) #return_ty {
            #func

            let retries_var = ::std::env::var("MARK_FLAKY_TESTS_RETRIES");
            let retries_s = retries_var.as_deref().unwrap_or("3");
            let retries = <::std::primitive::usize as ::std::str::FromStr>::from_str(retries_s)
                .expect("`MARK_FLAKY_TESTS_RETRIES` must contain a number");

            let strict_var = ::std::env::var("MARK_FLAKY_TESTS_STRICT");
            let strict_s = strict_var.as_deref().unwrap_or("false");
            let strict = <::std::primitive::bool as ::std::str::FromStr>::from_str(strict_s)
                .expect("`MARK_FLAKY_TESTS_STRICT` must contain a boolean");

            if strict {
                for _ in 0..(retries - 1) {
                    let res = #name(#inputs) #await_;
                    if #self_crate::_priv::IsFailure::is_failure(&res) {
                        return res;
                    }
                }
            } else {
                for _ in 0..(retries - 1) {
                    if let ::std::result::Result::Ok(res) = #catch_unwind #await_ {
                        if !#self_crate::_priv::IsFailure::is_failure(&res) {
                            return res;
                        }
                    }
                }
            }

            #name(#inputs) #await_
        }
    }
    .into()
}
