//! Source-level debugger queries over a program's debug model and machine state.
use vole_core::{DebugInfo, Program, debug::DebugView};

/// Read-only machine access needed by unwinding and variable evaluation.
pub trait Target {
    fn pc(&self) -> u64;
    fn register(&self, name: &str) -> Option<u64>;
    fn read(&self, address: u64, length: usize) -> Option<Vec<u8>>;
}

impl Target for vole_core::Snapshot {
    fn pc(&self) -> u64 {
        self.pc
    }
    fn register(&self, name: &str) -> Option<u64> {
        vole_core::Snapshot::register(self, name)
    }
    fn read(&self, address: u64, length: usize) -> Option<Vec<u8>> {
        vole_core::Snapshot::read(self, address, length)
    }
}

/// Build the paused debugger view: call stack, variables, location and exit status.
pub fn debug_view(
    program: &Program,
    target: &dyn Target,
    previous: Option<&DebugView>,
) -> Option<DebugView> {
    let _ = (program.debug.as_ref()?, target, previous);
    None
}

/// Resolve a 1-based user source line to a breakpoint address and the line actually used.
pub fn breakpoint_address(debug: &DebugInfo, line: usize) -> Option<(u64, usize)> {
    let _ = (debug, line);
    None
}
