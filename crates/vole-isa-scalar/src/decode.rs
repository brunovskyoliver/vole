use capstone::prelude::*;
use vole_core::{Architecture, Instruction, SimError};

pub(crate) fn engine(architecture: Architecture) -> Result<Capstone, SimError> {
    let engine = match architecture {
        Architecture::Arm32 => Capstone::new()
            .arm()
            .mode(arch::arm::ArchMode::Arm)
            .detail(true)
            .build(),
        Architecture::Arm64 => Capstone::new()
            .arm64()
            .mode(arch::arm64::ArchMode::Arm)
            .detail(true)
            .build(),
        Architecture::X86 => Capstone::new()
            .x86()
            .mode(arch::x86::ArchMode::Mode32)
            .syntax(arch::x86::ArchSyntax::Intel)
            .detail(true)
            .build(),
        Architecture::X64 => Capstone::new()
            .x86()
            .mode(arch::x86::ArchMode::Mode64)
            .syntax(arch::x86::ArchSyntax::Intel)
            .detail(true)
            .build(),
        Architecture::Vole => {
            return Err(SimError(
                "Use the VOLE decoder for 8-bit instructions".into(),
            ));
        }
    };
    engine.map_err(|error| SimError(format!("Cannot initialize decoder: {error}")))
}

pub fn decode(
    architecture: Architecture,
    address: u64,
    bytes: &[u8],
) -> Result<Instruction, SimError> {
    decode_with(&engine(architecture)?, architecture, address, bytes)
}

pub(crate) fn decode_with(
    cs: &Capstone,
    architecture: Architecture,
    address: u64,
    bytes: &[u8],
) -> Result<Instruction, SimError> {
    if matches!(architecture, Architecture::Arm32 | Architecture::Arm64)
        && !address.is_multiple_of(4)
    {
        return Err(SimError(format!(
            "Instruction address 0x{address:x} is not four-byte aligned"
        )));
    }
    let decoded = cs
        .disasm_count(bytes, address, 1)
        .map_err(|error| SimError(format!("Cannot decode at 0x{address:x}: {error}")))?;
    let instruction = decoded.iter().next().ok_or_else(|| {
        SimError(format!(
            "Invalid or incomplete instruction at 0x{address:x}"
        ))
    })?;
    let mnemonic = instruction.mnemonic().unwrap_or("?");
    let operands = instruction.op_str().unwrap_or("");
    let assembly = if operands.is_empty() {
        mnemonic.to_string()
    } else {
        format!("{mnemonic} {operands}")
    };
    let detail = cs
        .insn_detail(instruction)
        .map_err(|error| SimError(error.to_string()))?;
    let mut reads: Vec<String> = detail
        .regs_read()
        .iter()
        .filter_map(|r| cs.reg_name(*r))
        .collect();
    let mut writes: Vec<String> = detail
        .regs_write()
        .iter()
        .filter_map(|r| cs.reg_name(*r))
        .collect();
    for operand in detail.arch_detail().operands() {
        use capstone::{AccessType, arch::ArchOperand};
        let (register, access, address) = match operand {
            ArchOperand::ArmOperand(op) => match op.op_type {
                arch::arm::ArmOperandType::Reg(register) => (Some(register), op.access, vec![]),
                arch::arm::ArmOperandType::Mem(memory) => {
                    (None, None, vec![memory.base(), memory.index()])
                }
                _ => (None, None, vec![]),
            },
            ArchOperand::Arm64Operand(op) => match op.op_type {
                arch::arm64::Arm64OperandType::Reg(register) => (Some(register), op.access, vec![]),
                arch::arm64::Arm64OperandType::Mem(memory) => {
                    (None, None, vec![memory.base(), memory.index()])
                }
                _ => (None, None, vec![]),
            },
            ArchOperand::X86Operand(op) => match op.op_type {
                arch::x86::X86OperandType::Reg(register) => (Some(register), op.access, vec![]),
                arch::x86::X86OperandType::Mem(memory) => {
                    (None, None, vec![memory.base(), memory.index()])
                }
                _ => (None, None, vec![]),
            },
        };
        if let Some(name) = register.and_then(|r| cs.reg_name(r)) {
            if matches!(access, Some(AccessType::ReadOnly | AccessType::ReadWrite)) {
                reads.push(name.clone());
            }
            if matches!(access, Some(AccessType::WriteOnly | AccessType::ReadWrite)) {
                writes.push(name);
            }
        }
        reads.extend(address.into_iter().filter_map(|r| cs.reg_name(r)));
    }
    reads.sort();
    reads.dedup();
    writes.sort();
    writes.dedup();
    Ok(Instruction {
        address,
        bytes: instruction.bytes().to_vec(),
        explanation: explain(mnemonic, operands),
        assembly,
        source_line: None,
        reads,
        writes,
    })
}

fn explain(mnemonic: &str, operands: &str) -> String {
    let action = match mnemonic {
        "mov" | "movabs" | "movw" | "movz" => "Copy a value into the destination",
        "movk" | "movt" => "Replace the selected halfword and retain the other bits",
        "movzx" | "movzb" | "uxtb" | "uxth" => "Extend the value with zero bits",
        "movsx" | "movsxd" | "sxtb" | "sxth" | "sxtw" => "Extend the value with its sign bit",
        "add" | "adds" => "Add the operands with destination-width wrapping",
        "sub" | "subs" => "Subtract the operands with destination-width wrapping",
        "cmp" => "Compare by subtraction; update flags without retaining the result",
        "and" | "ands" => "Keep the bits set in both operands",
        "or" | "orr" => "Combine bits set in either operand",
        "xor" | "eor" => "Set the bits that differ between operands",
        "ldr" | "ldrb" | "ldrh" | "ldrsb" | "ldrsh" | "ldrsw" | "ldur" => "Read the little-endian value from mapped memory",
        "str" | "strb" | "strh" | "stur" => "Write the little-endian value into mapped memory",
        "lea" | "adr" | "adrp" => "Compute an address without reading its memory",
        "push" => "Move the stack pointer down and save the operand",
        "pop" => "Restore a saved value and move the stack pointer up",
        "call" | "bl" | "blr" => "Save the return address and branch to the target",
        "ret" => "Branch to the saved return address",
        "bx" => "Branch to the address held in the operand register",
        "jmp" | "b" | "br" => "Branch to the target address",
        "bkpt" | "brk" => "Teaching trap: 0 halts, 1 emits the low byte of register 0, other values fault",
        "int3" | "hlt" => "Halt the simulated program at a teaching stop",
        "svc" | "syscall" | "int" => "Handle the deterministic teaching output/exit ABI",
        "nop" => "Advance to the next instruction without changing data",
        "shl" | "sal" | "lsl" => "Shift bits to the left",
        "shr" | "lsr" => "Shift bits to the right, filling with zeros",
        "sar" | "asr" => "Shift bits to the right, retaining the sign",
        "test" | "tst" => "Compare bits using AND; update flags without retaining the result",
        "inc" => "Increment the destination and update flags",
        "dec" => "Decrement the destination and update flags",
        _ if mnemonic.starts_with('j') || mnemonic.starts_with("b.") || matches!(mnemonic, "beq" | "bne" | "bgt" | "blt" | "bge" | "ble" | "cbz" | "cbnz" | "tbz" | "tbnz") => "Branch when the instruction's condition is true",
        _ => return "Decoded instruction. Execution support is checked when stepping; see the ISA support matrix.".into(),
    };
    if operands.is_empty() {
        format!("{action}.")
    } else {
        format!("{action}: {operands}.")
    }
}
