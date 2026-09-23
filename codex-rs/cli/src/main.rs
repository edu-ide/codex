#![recursion_limit = "8192"]

#[cfg(feature = "ilhae")]
const IS_ILHAE_BINARY: bool = false;

include!("main_body.rs");
