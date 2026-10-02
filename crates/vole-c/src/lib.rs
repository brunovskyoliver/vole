//! Freestanding C compilation for guest targets using Clang and LLD.
use vole_core::{Architecture, CompilerSettings, Diagnostic, Program};

/// Compile a single C document with the teaching runtime and link a guest image.
pub fn compile(
    architecture: Architecture,
    source: &str,
    settings: &CompilerSettings,
) -> Result<Program, Vec<Diagnostic>> {
    let _ = (architecture, source, settings);
    Err(vec![Diagnostic::new(
        1,
        "C compilation is not available yet",
    )])
}
