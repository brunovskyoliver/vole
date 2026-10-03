//! Source-level debugger queries over a program's debug model and machine state.
//!
//! Everything here is read-only: the runtime owns execution and calls these
//! functions with a [`Target`] (a paused snapshot, a live machine, or a light
//! register shadow) to unwind the stack, resolve breakpoints and render values.
use std::collections::BTreeMap;
use vole_core::{
    Architecture, DebugInfo, Machine, Program, Snapshot,
    debug::{DebugView, FrameView, SourceLocation},
};

mod lines;
mod unwind;
mod values;

pub use lines::{breakpoint_address, line_at, statement_start};
pub use unwind::{UnwoundFrame, backtrace, caller_frame, current_cfa, frame_zero};

/// Read-only machine access needed by unwinding and variable evaluation.
pub trait Target {
    fn pc(&self) -> u64;
    /// Read a register by machine name or architectural alias.
    fn register(&self, name: &str) -> Option<u64>;
    /// Read mapped guest bytes; `None` when any byte is unmapped.
    fn read(&self, address: u64, length: usize) -> Option<Vec<u8>>;
    fn halted(&self) -> bool {
        false
    }
}

impl Target for Snapshot {
    fn pc(&self) -> u64 {
        self.pc
    }
    fn register(&self, name: &str) -> Option<u64> {
        Snapshot::register(self, name)
    }
    fn read(&self, address: u64, length: usize) -> Option<Vec<u8>> {
        Snapshot::read(self, address, length)
    }
    fn halted(&self) -> bool {
        self.halted
    }
}

/// A live machine viewed through its fast, side-effect-free read methods.
pub struct MachineTarget<'a>(pub &'a dyn Machine);

impl Target for MachineTarget<'_> {
    fn pc(&self) -> u64 {
        self.0.pc()
    }
    fn register(&self, name: &str) -> Option<u64> {
        self.0.read_register(name)
    }
    fn read(&self, address: u64, length: usize) -> Option<Vec<u8>> {
        self.0.read_memory(address, length)
    }
    fn halted(&self) -> bool {
        self.0.halted()
    }
}

/// Register conventions the unwinder and exit-status rule depend on.
pub(crate) struct Abi {
    pub sp: &'static str,
    /// Registers a callee must preserve; they keep their value in caller frames
    /// unless the unwind row says otherwise.
    pub preserved: &'static [&'static str],
    /// Argument register of the teaching exit call.
    pub exit_argument: &'static str,
    pub pointer_bytes: u64,
}

pub(crate) fn abi(architecture: Architecture) -> Abi {
    match architecture {
        Architecture::Arm64 => Abi {
            sp: "sp",
            // x30 is not callee-saved, but at a function's first instruction it
            // still holds the return address and CFI leaves it implicit.
            preserved: &[
                "x19", "x20", "x21", "x22", "x23", "x24", "x25", "x26", "x27", "x28", "x29", "x30",
            ],
            exit_argument: "x0",
            pointer_bytes: 8,
        },
        Architecture::Arm32 => Abi {
            sp: "r13",
            preserved: &["r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r14"],
            exit_argument: "r0",
            pointer_bytes: 4,
        },
        Architecture::X64 => Abi {
            sp: "rsp",
            preserved: &["rbx", "rbp", "r12", "r13", "r14", "r15"],
            exit_argument: "rdi",
            pointer_bytes: 8,
        },
        Architecture::X86 => Abi {
            sp: "esp",
            preserved: &["ebx", "esi", "edi", "ebp"],
            exit_argument: "ebx",
            pointer_bytes: 4,
        },
        Architecture::Vole => Abi {
            sp: "sp",
            preserved: &[],
            exit_argument: "r0",
            pointer_bytes: 1,
        },
    }
}

pub(crate) fn address_mask(architecture: Architecture) -> u64 {
    match architecture.bits() {
        64 => u64::MAX,
        bits => (1_u64 << bits) - 1,
    }
}

/// Canonical machine register name so aliases share one unwound value.
pub(crate) fn canonical(architecture: Architecture, name: &str) -> String {
    let name = name.trim().to_ascii_lowercase();
    let mapped = match (architecture, name.as_str()) {
        (Architecture::Arm64, "fp") => "x29",
        (Architecture::Arm64, "lr") => "x30",
        (Architecture::Arm64, "x31" | "wsp") => "sp",
        (Architecture::Arm32, "sp") => "r13",
        (Architecture::Arm32, "lr") => "r14",
        (Architecture::Arm32, "fp") => "r11",
        (Architecture::Arm32, "ip") => "r12",
        (Architecture::Arm32, "sb") => "r9",
        (Architecture::Arm32, "sl") => "r10",
        (Architecture::Arm32, "r15") => "pc",
        _ => return name,
    };
    mapped.into()
}

/// Structural checks for saved or compiled debug metadata. The debugger never
/// panics on bad indices, but sorted tables are required for address lookup.
pub fn validate(debug: &DebugInfo) -> Result<(), String> {
    if debug.files.first().is_some_and(|file| !file.user) {
        return Err("Debug information must list the user's document first.".into());
    }
    if debug.files.len() > 4096
        || debug.lines.len() > 1_000_000
        || debug.functions.len() > 100_000
        || debug.types.len() > 1_000_000
        || debug.unwind.len() > 1_000_000
    {
        return Err("Debug information exceeds the supported size.".into());
    }
    if debug.lines.windows(2).any(|w| w[0].address > w[1].address) {
        return Err("Debug line table is not sorted by address.".into());
    }
    if debug
        .lines
        .iter()
        .any(|row| row.file as usize >= debug.files.len())
    {
        return Err("Debug line table refers to an unknown file.".into());
    }
    if debug
        .unwind
        .windows(2)
        .any(|w| w[0].start > w[1].start || w[0].end > w[1].start)
        || debug.unwind.iter().any(|row| row.start > row.end)
    {
        return Err("Unwind table rows must be sorted and must not overlap.".into());
    }
    if debug.functions.iter().any(|f| f.low_pc > f.high_pc) {
        return Err("Debug function has an inverted address range.".into());
    }
    let types = debug.types.len();
    let bad_type = |id: &Option<usize>| id.is_some_and(|id| id >= types);
    let variables = debug
        .functions
        .iter()
        .flat_map(|f| f.variables.iter())
        .chain(debug.globals.iter());
    for variable in variables {
        if bad_type(&variable.type_id) {
            return Err(format!(
                "Variable {} refers to an unknown type.",
                variable.name
            ));
        }
    }
    use vole_core::debug::TypeKind;
    for ty in &debug.types {
        let ok = match &ty.kind {
            TypeKind::Pointer(t)
            | TypeKind::Typedef(t)
            | TypeKind::Const(t)
            | TypeKind::Volatile(t) => !bad_type(t),
            TypeKind::Array { element, .. } => !bad_type(element),
            TypeKind::Struct(members) | TypeKind::Union(members) => {
                members.iter().all(|m| !bad_type(&m.type_id))
            }
            _ => true,
        };
        if !ok {
            return Err(format!("Type {} refers to an unknown type.", ty.name));
        }
    }
    type_cycle(debug)
}

/// Types may refer to themselves only through pointers (`struct node *next`).
/// Any cycle through arrays, members, typedefs or qualifiers has no size.
fn type_cycle(debug: &DebugInfo) -> Result<(), String> {
    use vole_core::debug::TypeKind;
    let edges = |id: usize| -> Vec<usize> {
        match &debug.types[id].kind {
            TypeKind::Typedef(t) | TypeKind::Const(t) | TypeKind::Volatile(t) => {
                t.iter().copied().collect()
            }
            TypeKind::Array { element, .. } => element.iter().copied().collect(),
            TypeKind::Struct(members) | TypeKind::Union(members) => {
                members.iter().filter_map(|m| m.type_id).collect()
            }
            _ => Vec::new(),
        }
    };
    // 0 = unvisited, 1 = on the current path, 2 = finished.
    let mut state = vec![0_u8; debug.types.len()];
    for root in 0..debug.types.len() {
        if state[root] != 0 {
            continue;
        }
        let mut stack = vec![(root, edges(root), 0_usize)];
        state[root] = 1;
        while let Some((node, children, next)) = stack.last_mut() {
            if let Some(&child) = children.get(*next) {
                *next += 1;
                match state[child] {
                    0 => {
                        state[child] = 1;
                        let grandchildren = edges(child);
                        stack.push((child, grandchildren, 0));
                    }
                    1 => {
                        return Err(format!(
                            "Type {} contains itself without a pointer.",
                            debug.types[child].name
                        ));
                    }
                    _ => {}
                }
            } else {
                state[*node] = 2;
                stack.pop();
            }
        }
    }
    Ok(())
}

/// Name of the code at `address`: the debug function, else the nearest symbol.
pub fn function_name(program: &Program, address: u64) -> Option<String> {
    if let Some(function) = program
        .debug
        .as_ref()
        .and_then(|debug| debug.function_at(address))
    {
        return Some(function.name.clone());
    }
    nearest_symbol(&program.symbols, address).map(|(name, _)| name.to_string())
}

fn nearest_symbol(symbols: &BTreeMap<String, u64>, address: u64) -> Option<(&str, u64)> {
    symbols
        .iter()
        .filter(|(name, value)| **value <= address && !name.starts_with('$') && !name.is_empty())
        .max_by_key(|(_, value)| **value)
        .map(|(name, value)| (name.as_str(), *value))
}

fn source_location(debug: &DebugInfo, address: u64) -> Option<SourceLocation> {
    let row = debug.row_for_address(address)?;
    Some(SourceLocation {
        file: row.file,
        line: row.line,
        column: row.column,
        user: debug.is_user_file(row.file),
    })
}

/// Exit status rule: the machine halted and the halting instruction belongs to
/// `vole_exit` (PC is inside it, or PC - 1 is when the exit call was its last
/// instruction). The status is the exit-call argument register truncated to
/// `int`. Halts from traps elsewhere (e.g. a `brk`) report no status.
pub fn exit_status(program: &Program, target: &dyn Target) -> Option<i64> {
    if !target.halted() {
        return None;
    }
    let pc = target.pc();
    let in_exit = |address: u64| function_name(program, address).as_deref() == Some("vole_exit");
    if !(in_exit(pc) || pc.checked_sub(1).is_some_and(in_exit)) {
        return None;
    }
    let value = target.register(abi(program.architecture).exit_argument)?;
    Some(value as u32 as i32 as i64)
}

/// Build the paused debugger view: call stack, variables, location and exit status.
pub fn debug_view(
    program: &Program,
    target: &dyn Target,
    previous: Option<&DebugView>,
) -> Option<DebugView> {
    let debug = program.debug.as_ref()?;
    let architecture = program.architecture;
    let unwound = backtrace(debug, architecture, target);
    let evaluator = values::Evaluator {
        debug,
        architecture,
        target,
        budget: std::cell::Cell::new(values::MAX_NODES),
    };
    let mut frames: Vec<FrameView> = unwound
        .iter()
        .enumerate()
        .map(|(index, frame)| {
            let function = debug.function_at(frame.lookup_pc);
            FrameView {
                index,
                function: function
                    .map(|f| f.name.clone())
                    .or_else(|| function_name(program, frame.lookup_pc))
                    .unwrap_or_else(|| "??".into()),
                pc: frame.pc,
                cfa: frame.cfa,
                location: source_location(debug, frame.lookup_pc),
                user: function.is_some_and(|f| f.user),
                variables: function
                    .map(|f| evaluator.frame_variables(index, frame, f))
                    .unwrap_or_default(),
            }
        })
        .collect();
    let mut globals = evaluator.globals(&unwound[0]);
    if let Some(previous) = previous {
        for frame in &mut frames {
            if let Some(old) = previous
                .frames
                .iter()
                .find(|old| old.function == frame.function && old.cfa == frame.cfa)
            {
                values::mark_changed(&mut frame.variables, &old.variables);
            }
        }
        values::mark_changed(&mut globals, &previous.globals);
    }
    let pc = target.pc();
    let location = source_location(debug, pc);
    let exit_status = exit_status(program, target);
    let note = if exit_status.is_some() || frames.first().is_some_and(|f| f.user) {
        None
    } else if nearest_symbol(&program.symbols, pc).is_some_and(|(name, _)| name == "_start") {
        Some(
            "Executing startup code before main; Step Into stops at the first line of main.".into(),
        )
    } else if debug.function_at(pc).is_some() || frames.iter().any(|f| f.user) {
        Some("Executing runtime code without C source; Step Out returns to your program.".into())
    } else {
        Some("Executing code without C source information.".into())
    };
    Some(DebugView {
        frames,
        globals,
        location,
        exit_status,
        note,
    })
}
