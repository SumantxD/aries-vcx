#![allow(clippy::or_fun_call)]
#![allow(clippy::module_inception)]
#![allow(clippy::derive_partial_eq_without_eq)]
#![allow(clippy::new_without_default)]
#![allow(clippy::inherent_to_string)]
#![allow(clippy::large_enum_variant)]
#![allow(clippy::missing_safety_doc)]
#![allow(clippy::type_complexity)]
#![allow(clippy::await_holding_lock)]
#![allow(clippy::len_without_is_empty)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![deny(clippy::unwrap_used)]
#![cfg_attr(feature = "fatal_warnings", deny(warnings))]
// todo: is this recursion_limit still needed and justified?
//this is needed for some large json macro invocations
#![recursion_limit = "128"]
#[macro_use]
pub extern crate aries_vcx;

extern crate num_traits;

#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

#[macro_use]
extern crate serde_derive;

#[macro_use]
pub extern crate serde_json;

#[macro_use]
pub mod api_vcx;

pub mod errors;
