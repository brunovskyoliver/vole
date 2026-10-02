//! Call-frame-information unwinding over [`DebugInfo::unwind`] rows.
use crate::{Target, abi, address_mask, canonical};
use std::collections::BTreeMap;
use vole_core::{
    Architecture, DebugInfo,
    debug::{CfaRule, RegisterRule},
};

/// Maximum number of frames a backtrace reports.
pub const MAX_FRAMES: usize = 64;

/// One activation found by unwinding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnwoundFrame {
    /// Resume address: the current PC for frame 0, the return address otherwise.
    pub pc: u64,
    /// Address used for line, scope and location lookup. Callers use PC - 1
    /// because the call instruction belongs to the line that made the call.
    pub lookup_pc: u64,
    /// Canonical frame address, when the unwind table describes this PC.
    pub cfa: Option<u64>,
    /// Recovered registers for caller frames; `None` reads the live target.
    registers: Option<BTreeMap<String, u64>>,
}

impl UnwoundFrame {
    /// Value of a register in this frame. Caller frames only know registers
    /// recovered by CFI rules, callee-saved registers and the stack pointer.
    pub fn register(
        &self,
        architecture: Architecture,
        target: &dyn Target,
        name: &str,
    ) -> Option<u64> {
        match &self.registers {
            None => target.register(name),
            Some(registers) => registers.get(&canonical(architecture, name)).copied(),
        }
    }

    pub fn is_innermost(&self) -> bool {
        self.registers.is_none()
    }
}

fn evaluate_cfa(
    debug: &DebugInfo,
    architecture: Architecture,
    address: u64,
    register: impl Fn(&str) -> Option<u64>,
) -> Option<u64> {
    match &debug.unwind_row(address)?.cfa {
        CfaRule::RegisterOffset {
            register: name,
            offset,
        } => Some(register(name)?.wrapping_add_signed(*offset) & address_mask(architecture)),
        CfaRule::Unsupported(_) => None,
    }
}

/// Canonical frame address of the innermost frame.
pub fn current_cfa(debug: &DebugInfo, target: &dyn Target) -> Option<u64> {
    match &debug.unwind_row(target.pc())?.cfa {
        CfaRule::RegisterOffset { register, offset } => {
            Some(target.register(register)?.wrapping_add_signed(*offset))
        }
        CfaRule::Unsupported(_) => None,
    }
}

/// The innermost frame for the target's current PC.
pub fn frame_zero(
    debug: &DebugInfo,
    architecture: Architecture,
    target: &dyn Target,
) -> UnwoundFrame {
    let pc = target.pc();
    UnwoundFrame {
        pc,
        lookup_pc: pc,
        cfa: evaluate_cfa(debug, architecture, pc, |name| target.register(name)),
        registers: None,
    }
}

/// Recover the caller of `frame` by applying the unwind row at its lookup PC.
/// Returns `None` when there is no row, the CFA or return address is unknown,
/// or the return address is zero (startup clears the link register).
pub fn caller_frame(
    debug: &DebugInfo,
    architecture: Architecture,
    target: &dyn Target,
    frame: &UnwoundFrame,
) -> Option<UnwoundFrame> {
    let row = debug.unwind_row(frame.lookup_pc)?;
    let cfa = frame.cfa?;
    let abi = abi(architecture);
    let mask = address_mask(architecture);
    let callee = |name: &str| frame.register(architecture, target, name);
    let mut registers = BTreeMap::new();
    for name in abi.preserved {
        if let Some(value) = callee(name) {
            registers.insert((*name).to_string(), value);
        }
    }
    for (name, rule) in &row.registers {
        let name = canonical(architecture, name);
        let value = match rule {
            RegisterRule::Undefined | RegisterRule::Unsupported(_) => None,
            RegisterRule::SameValue => callee(&name),
            RegisterRule::Offset(offset) => {
                let address = cfa.wrapping_add_signed(*offset) & mask;
                target
                    .read(address, abi.pointer_bytes as usize)
                    .map(|bytes| little_endian(&bytes))
            }
            RegisterRule::ValOffset(offset) => Some(cfa.wrapping_add_signed(*offset) & mask),
            RegisterRule::Register(other) => callee(other),
        };
        match value {
            Some(value) => registers.insert(name, value),
            None => registers.remove(&name),
        };
    }
    registers.insert(abi.sp.to_string(), cfa);
    let return_register = canonical(architecture, &debug.return_address_register);
    let mut pc = *registers.get(&return_register)? & mask;
    if architecture == Architecture::Arm32 {
        pc &= !1;
    }
    if pc == 0 {
        return None;
    }
    let lookup_pc = pc - 1;
    let caller_cfa = evaluate_cfa(debug, architecture, lookup_pc, |name| {
        registers.get(&canonical(architecture, name)).copied()
    });
    Some(UnwoundFrame {
        pc,
        lookup_pc,
        cfa: caller_cfa,
        registers: Some(registers),
    })
}

/// Walk the stack from the current PC. Frame 0 is always present. The walk
/// stops at the startup code (`_start` or code without a debug function), an
/// undefined or zero return address, a PC without an unwind row, a CFA that
/// does not increase, or [`MAX_FRAMES`].
pub fn backtrace(
    debug: &DebugInfo,
    architecture: Architecture,
    target: &dyn Target,
) -> Vec<UnwoundFrame> {
    let mut frames = vec![frame_zero(debug, architecture, target)];
    while frames.len() < MAX_FRAMES {
        let last = frames.last().expect("frame zero exists");
        let Some(caller) = caller_frame(debug, architecture, target, last) else {
            break;
        };
        if debug
            .function_at(caller.lookup_pc)
            .is_none_or(|function| function.name == "_start")
        {
            break;
        }
        if let (Some(outer), Some(inner)) = (caller.cfa, last.cfa)
            && outer <= inner
        {
            break;
        }
        let stop = caller.cfa.is_none();
        frames.push(caller);
        if stop {
            break;
        }
    }
    frames
}

pub(crate) fn little_endian(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .take(8)
        .enumerate()
        .fold(0, |value, (index, byte)| {
            value | (u64::from(*byte) << (index * 8))
        })
}
