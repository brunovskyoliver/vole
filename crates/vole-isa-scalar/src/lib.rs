//! Scalar teaching interpreters. Instructions are decoded from mapped machine bytes.
mod decode;
mod machine;
mod toolchain;

pub use decode::decode;
pub use machine::ScalarMachine;
pub use toolchain::{assemble, linker_status, toolchain_status};
