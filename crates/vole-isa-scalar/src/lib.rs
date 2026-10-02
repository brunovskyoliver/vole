//! Scalar teaching interpreters. Instructions are decoded from mapped machine bytes.
mod decode;
mod machine;
mod toolchain;

pub use decode::decode;
pub use machine::ScalarMachine;
pub use toolchain::{assemble, toolchain_status};
