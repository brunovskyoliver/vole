//! Deterministic scalar interpreters for ARM32, ARM64, x86 and x64 guests.
//!
//! Machine bytes are decoded by Capstone. Each address's decoded text is parsed
//! once into typed operands and cached; the cache entry is reused only while the
//! guest bytes at that address are unchanged, so self-modifying code still
//! executes its current bytes.
mod arm32;
mod arm64;
mod operand;
mod x86;

use crate::decode;
use operand::{Mem, Operand, PC, Reg, ZR};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::Arc,
};
use vole_core::{
    Architecture, Instruction, Machine, MemoryAccess, MemoryChange, MemoryRegion, Program,
    RegisterChange, RegisterValue, SimError, Snapshot, StepRecord,
};

const HISTORY_BYTES: usize = 2 * 1024 * 1024;
const HISTORY_STEPS: usize = 4096;
const MEMORY_BUDGET: usize = 4 * 1024 * 1024;
const OUTPUT_BUDGET: usize = 1024 * 1024;
const DECODE_CACHE_LIMIT: usize = 1 << 16;
const REGISTER_SLOTS: usize = 32;

// ARM flag bits.
const N: u8 = 1;
const Z: u8 = 2;
const C: u8 = 4;
const V: u8 = 8;
// x86 flag bits.
const CF: u8 = 1;
const PF: u8 = 2;
const AF: u8 = 4;
const ZF: u8 = 8;
const SF: u8 = 16;
const OF: u8 = 32;

const ARM_FLAG_NAMES: [(&str, u8); 4] = [("C", C), ("N", N), ("V", V), ("Z", Z)];
const X86_FLAG_NAMES: [(&str, u8); 6] = [
    ("AF", AF),
    ("CF", CF),
    ("OF", OF),
    ("PF", PF),
    ("SF", SF),
    ("ZF", ZF),
];

/// A decoded instruction with its operands parsed once.
pub(crate) struct Decoded {
    instruction: Arc<Instruction>,
    /// Mnemonic without ARM32 condition suffix or x86 repeat prefix.
    mnemonic: String,
    /// ARM32 condition field when the instruction is conditional.
    condition: Option<u8>,
    /// x86 `rep`/`repe`/`repne` prefix.
    repeat: bool,
    operands: Result<Vec<Operand>, String>,
}

struct Undo {
    pc_before: u64,
    pc_after: u64,
    instruction: Arc<Instruction>,
    registers: Vec<(u8, u64, u64)>,
    flags_before: u8,
    flags_after: u8,
    memory: Vec<MemoryChange>,
    memory_reads: Vec<MemoryAccess>,
    output_added: Vec<u8>,
    halted: bool,
    size: usize,
}

/// A bounded deterministic interpreter for the documented scalar instruction subset.
/// Host CPU instructions are never used to execute guest bytes.
pub struct ScalarMachine {
    architecture: Architecture,
    program: Option<Program>,
    /// Address to index in `program.instructions`, for source lines.
    program_index: HashMap<u64, usize>,
    names: Vec<String>,
    /// Storage indexes in snapshot (name) order.
    order: Vec<u8>,
    registers: [u64; REGISTER_SLOTS],
    flags: u8,
    memory: Vec<MemoryRegion>,
    pc: u64,
    /// Value an instruction observes when it reads the PC as an operand.
    pc_read: u64,
    /// Fall-through address of the executing instruction.
    next_pc: u64,
    output: Vec<u8>,
    halted: bool,
    steps: u64,
    history: VecDeque<Undo>,
    history_bytes: usize,
    writes: Vec<MemoryChange>,
    reads: Vec<MemoryAccess>,
    cache: HashMap<u64, Arc<Decoded>>,
}

impl ScalarMachine {
    pub fn new(architecture: Architecture) -> Self {
        let names = operand::storage_names(architecture);
        let mut order: Vec<u8> = (0..names.len() as u8).collect();
        order.sort_by(|a, b| names[usize::from(*a)].cmp(&names[usize::from(*b)]));
        Self {
            architecture,
            program: None,
            program_index: HashMap::new(),
            names,
            order,
            registers: [0; REGISTER_SLOTS],
            flags: 0,
            memory: vec![],
            pc: 0,
            pc_read: 0,
            next_pc: 0,
            output: vec![],
            halted: false,
            steps: 0,
            history: VecDeque::new(),
            history_bytes: 0,
            writes: vec![],
            reads: vec![],
            cache: HashMap::new(),
        }
    }

    fn is_arm(&self) -> bool {
        matches!(self.architecture, Architecture::Arm32 | Architecture::Arm64)
    }
    fn mask(bits: u8) -> u64 {
        if bits >= 64 {
            u64::MAX
        } else {
            (1_u64 << bits) - 1
        }
    }
    fn signed(value: u64, bits: u8) -> i64 {
        if bits >= 64 {
            value as i64
        } else {
            ((value << (64 - bits)) as i64) >> (64 - bits)
        }
    }
    fn address(&self, value: u64) -> u64 {
        value & Self::mask(self.architecture.bits())
    }
    fn pc_name(&self) -> &'static str {
        match self.architecture {
            Architecture::X86 => "eip",
            Architecture::X64 => "rip",
            _ => "pc",
        }
    }
    fn flag_names(&self) -> &'static [(&'static str, u8)] {
        if self.is_arm() {
            &ARM_FLAG_NAMES
        } else {
            &X86_FLAG_NAMES
        }
    }
    fn clear_history(&mut self) {
        self.history.clear();
        self.history_bytes = 0;
    }

    fn register(&self, name: &str) -> Result<Reg, SimError> {
        operand::parse_register(self.architecture, name.trim()).ok_or_else(|| {
            SimError(format!(
                "Unknown {} register {}",
                self.architecture.name(),
                name.trim()
            ))
        })
    }
    /// Stored bits of a register view, without the ARM32 PC + 8 operand rule.
    fn peek(&self, register: Reg) -> u64 {
        let value = match register.index {
            ZR => 0,
            PC => self.pc,
            index => self.registers[usize::from(index)],
        };
        (value >> register.offset) & Self::mask(register.bits)
    }
    /// Register value as an executing instruction's operand.
    fn get(&self, register: Reg) -> u64 {
        match register.index {
            ZR => 0,
            PC => self.pc_read & Self::mask(register.bits),
            index => {
                (self.registers[usize::from(index)] >> register.offset) & Self::mask(register.bits)
            }
        }
    }
    fn set(&mut self, register: Reg, value: u64) -> Result<(), SimError> {
        match register.index {
            ZR => Ok(()),
            PC => self.branch(value),
            index => {
                let slot = &mut self.registers[usize::from(index)];
                let value = value & Self::mask(register.bits);
                *slot = if register.zero_upper {
                    value
                } else {
                    *slot & !(Self::mask(register.bits) << register.offset)
                        | value << register.offset
                };
                Ok(())
            }
        }
    }
    fn branch(&mut self, target: u64) -> Result<(), SimError> {
        let target = self.address(target);
        if self.is_arm() && !target.is_multiple_of(4) {
            return Err(SimError(format!(
                "Branch target 0x{target:x} is not A32/A64 aligned; Thumb is unsupported"
            )));
        }
        self.pc = target;
        Ok(())
    }

    fn region_of(&self, address: u64) -> Option<usize> {
        self.memory.iter().position(|r| r.contains(address))
    }
    fn read_bytes(&self, address: u64, length: usize) -> Result<Vec<u8>, SimError> {
        if length > MEMORY_BUDGET {
            return Err(SimError("Memory read exceeds allocation budget".into()));
        }
        if let Some(index) = self.region_of(address) {
            let region = &self.memory[index];
            let start = (address - region.base) as usize;
            if let Some(bytes) = region.bytes.get(start..start + length) {
                return Ok(bytes.to_vec());
            }
        }
        (0..length)
            .map(|offset| {
                let address = address
                    .checked_add(offset as u64)
                    .ok_or_else(|| SimError("Memory address overflow".into()))?;
                self.memory
                    .iter()
                    .find(|r| r.contains(address))
                    .map(|r| r.bytes[(address - r.base) as usize])
                    .ok_or_else(|| SimError(format!("Unmapped memory at 0x{address:x}")))
            })
            .collect()
    }
    /// Guest data read; recorded for watchpoints and the trace.
    fn read(&mut self, address: u64, length: usize) -> Result<u64, SimError> {
        let mut value = 0;
        if let Some(index) = self.region_of(address) {
            let region = &self.memory[index];
            let start = (address - region.base) as usize;
            if let Some(bytes) = region.bytes.get(start..start + length) {
                for (offset, byte) in bytes.iter().enumerate() {
                    value |= u64::from(*byte) << (offset * 8);
                }
                self.reads.push(MemoryAccess { address, length });
                return Ok(value);
            }
        }
        for (offset, byte) in self.read_bytes(address, length)?.into_iter().enumerate() {
            value |= u64::from(byte) << (offset * 8);
        }
        self.reads.push(MemoryAccess { address, length });
        Ok(value)
    }
    fn write_bytes(&mut self, address: u64, bytes: &[u8], record: bool) -> Result<(), SimError> {
        if bytes.len() > MEMORY_BUDGET {
            return Err(SimError("Memory write exceeds allocation budget".into()));
        }
        let old = self.read_bytes(address, bytes.len())?;
        for offset in 0..bytes.len() {
            let at = address + offset as u64;
            if let Some(region) = self.memory.iter().find(|r| r.contains(at) && !r.writable) {
                return Err(SimError(format!(
                    "Write to read-only memory at 0x{at:x} ({}); code and constant data cannot be modified by the program",
                    region.label
                )));
            }
        }
        for (offset, byte) in bytes.iter().enumerate() {
            let at = address + offset as u64;
            let region = self
                .memory
                .iter_mut()
                .find(|r| r.contains(at))
                .expect("Prevalidated memory");
            region.bytes[(at - region.base) as usize] = *byte;
        }
        if record {
            self.writes.push(MemoryChange {
                address,
                before: old,
                after: bytes.to_vec(),
            });
        }
        Ok(())
    }
    fn write(&mut self, address: u64, length: usize, value: u64) -> Result<(), SimError> {
        self.write_bytes(address, &value.to_le_bytes()[..length], true)
    }
    fn fetch(&self) -> Result<Vec<u8>, SimError> {
        let maximum = if self.is_arm() { 4 } else { 15 };
        let mut bytes = Vec::with_capacity(maximum);
        for offset in 0..maximum as u64 {
            let address = self
                .pc
                .checked_add(offset)
                .ok_or_else(|| SimError("Instruction address overflow".into()))?;
            let Some(region) = self
                .memory
                .iter()
                .find(|region| region.contains(address) && region.executable)
            else {
                break;
            };
            bytes.push(region.bytes[(address - region.base) as usize]);
        }
        if bytes.is_empty() {
            return Err(SimError(format!(
                "Instruction fetch from unmapped or non-executable address 0x{:x}",
                self.pc
            )));
        }
        Ok(bytes)
    }
    /// True when the executable bytes at the PC still equal `bytes`.
    fn code_matches(&self, bytes: &[u8]) -> bool {
        let pc = self.pc;
        self.memory.iter().any(|region| {
            region.executable
                && region.contains(pc)
                && region
                    .bytes
                    .get((pc - region.base) as usize..(pc - region.base) as usize + bytes.len())
                    .is_some_and(|current| current == bytes)
        })
    }
    fn decoded(&mut self) -> Result<Arc<Decoded>, SimError> {
        if let Some(entry) = self.cache.get(&self.pc)
            && self.code_matches(&entry.instruction.bytes)
        {
            return Ok(entry.clone());
        }
        let bytes = self.fetch()?;
        let mut instruction = decode::decode(self.architecture, self.pc, &bytes)?;
        if let Some(original) = self
            .program_index
            .get(&self.pc)
            .and_then(|index| self.program.as_ref()?.instructions.get(*index))
            .filter(|original| original.bytes == instruction.bytes)
        {
            instruction.source_line = original.source_line;
        }
        let entry = Arc::new(self.parse(instruction));
        if self.cache.len() >= DECODE_CACHE_LIMIT {
            self.cache.clear();
        }
        self.cache.insert(self.pc, entry.clone());
        Ok(entry)
    }
    fn parse(&self, instruction: Instruction) -> Decoded {
        let (mut mnemonic, mut operand_text) = instruction
            .assembly
            .split_once(' ')
            .unwrap_or((instruction.assembly.as_str(), ""));
        let mut repeat = false;
        if !self.is_arm() && matches!(mnemonic, "rep" | "repe" | "repz" | "repne" | "repnz") {
            repeat = true;
            (mnemonic, operand_text) = operand_text.split_once(' ').unwrap_or((operand_text, ""));
            if !matches!(mnemonic, "movsb" | "movsw" | "movsd" | "movsq")
                && !mnemonic.starts_with("stos")
                || instruction.assembly.starts_with("repn")
            {
                // Only rep movs/stos are modelled; other repeated forms fault.
                mnemonic = "rep-unsupported";
            }
        }
        let mut mnemonic = mnemonic.to_string();
        let mut condition = None;
        if self.architecture == Architecture::Arm32 && instruction.bytes.len() == 4 {
            let word = u32::from_le_bytes(instruction.bytes[..4].try_into().expect("four bytes"));
            let code = (word >> 28) as u8;
            if code < 14 {
                condition = Some(code);
                let suffix = operand::ARM_CONDITIONS[usize::from(code)];
                if mnemonic.ends_with(suffix)
                    || code == 2 && mnemonic.ends_with("cs")
                    || code == 3 && mnemonic.ends_with("cc")
                {
                    mnemonic.truncate(mnemonic.len() - 2);
                }
            } else if code == 15 {
                condition = Some(15);
            }
        }
        let operands = operand::split_operands(operand_text)
            .into_iter()
            .map(|text| operand::parse_operand(self.architecture, text))
            .collect();
        Decoded {
            instruction: Arc::new(instruction),
            mnemonic,
            condition,
            repeat,
            operands,
        }
    }

    fn flag(&self, bit: u8) -> bool {
        self.flags & bit != 0
    }
    fn set_flag(&mut self, bit: u8, value: bool) {
        if value {
            self.flags |= bit;
        } else {
            self.flags &= !bit;
        }
    }
    /// ARM `AddWithCarry`: returns the result and sets NZCV when `update`.
    fn add_with_carry(&mut self, x: u64, y: u64, carry: bool, bits: u8, update: bool) -> u64 {
        let mask = Self::mask(bits);
        let (x, y) = (x & mask, y & mask);
        let unsigned = u128::from(x) + u128::from(y) + u128::from(carry);
        let result = unsigned as u64 & mask;
        if update {
            let sign = 1_u64 << (bits - 1);
            self.set_flag(N, result & sign != 0);
            self.set_flag(Z, result == 0);
            self.set_flag(C, unsigned > u128::from(mask));
            self.set_flag(V, !(x ^ y) & (x ^ result) & sign != 0);
        }
        result
    }
    fn set_nz(&mut self, value: u64, bits: u8) {
        let value = value & Self::mask(bits);
        self.set_flag(N, value >> (bits - 1) & 1 != 0);
        self.set_flag(Z, value == 0);
    }
    fn arm_condition(&self, code: u8) -> bool {
        let (n, z, c, v) = (self.flag(N), self.flag(Z), self.flag(C), self.flag(V));
        let result = match code >> 1 {
            0 => z,
            1 => c,
            2 => n,
            3 => v,
            4 => c && !z,
            5 => n == v,
            6 => !z && n == v,
            _ => true,
        };
        if code & 1 == 1 && code != 15 {
            !result
        } else {
            result
        }
    }

    fn emit(&mut self, bytes: &[u8]) -> Result<(), SimError> {
        if self
            .output
            .len()
            .checked_add(bytes.len())
            .is_none_or(|length| length > OUTPUT_BUDGET)
        {
            return Err(SimError("Teaching output exceeded one MiB".into()));
        }
        self.output.extend_from_slice(bytes);
        Ok(())
    }
    fn named(&self, name: &str) -> Result<u64, SimError> {
        Ok(self.peek(self.register(name)?))
    }
    fn syscall(&mut self) -> Result<(), SimError> {
        let (number, exit, write, fd, address, count, result) = match self.architecture {
            Architecture::Arm64 => ("x8", 93, 64, "x0", "x1", "x2", "x0"),
            Architecture::Arm32 => ("r7", 1, 4, "r0", "r1", "r2", "r0"),
            Architecture::X86 => ("eax", 1, 4, "ebx", "ecx", "edx", "eax"),
            Architecture::X64 => ("rax", 60, 1, "rdi", "rsi", "rdx", "rax"),
            Architecture::Vole => return Err(SimError("Invalid scalar architecture".into())),
        };
        let number = self.named(number)?;
        if number == exit {
            self.halted = true;
            return Ok(());
        }
        if number != write || self.named(fd)? != 1 {
            return Err(SimError(format!(
                "Unsupported teaching syscall {number}; only stdout write and exit are supported"
            )));
        }
        let address = self.named(address)?;
        let count = self.named(count)?;
        let length =
            usize::try_from(count).map_err(|_| SimError("Output length overflow".into()))?;
        if length > OUTPUT_BUDGET {
            return Err(SimError("Output write exceeds one MiB".into()));
        }
        let bytes = self.read_bytes(address, length)?;
        self.reads.push(MemoryAccess { address, length });
        self.emit(&bytes)?;
        let result = self.register(result)?;
        self.set(result, count)
    }
    /// ARM teaching trap: 0 halts, 1 emits the low byte of register 0.
    fn teaching_trap(&mut self, trap: u64) -> Result<(), SimError> {
        if trap == 0 {
            self.halted = true;
            Ok(())
        } else if trap == 1 {
            let byte = self.registers[0] as u8;
            self.emit(&[byte])
        } else {
            Err(SimError(format!(
                "Unknown teaching trap {trap}; 0 halts and 1 emits register 0"
            )))
        }
    }

    fn unsupported(&self, decoded: &Decoded) -> SimError {
        SimError(format!(
            "Unsupported {} instruction at 0x{:x}: {}. See docs/isa-support.md for the scalar subset.",
            self.architecture.name(),
            decoded.instruction.address,
            decoded.instruction.assembly
        ))
    }

    fn execute(&mut self, decoded: &Decoded) -> Result<(), SimError> {
        let old_pc = self.pc;
        let next_pc = self.address(old_pc.wrapping_add(decoded.instruction.bytes.len() as u64));
        self.next_pc = next_pc;
        self.pc = next_pc;
        self.pc_read = match self.architecture {
            Architecture::Arm32 => old_pc.wrapping_add(8) & 0xffff_ffff,
            Architecture::Arm64 => old_pc,
            _ => next_pc,
        };
        if decoded.condition == Some(15) {
            return Err(SimError(
                "A32 unconditional extension/Thumb instruction is outside the supported subset"
                    .into(),
            ));
        }
        if let Some(code) = decoded.condition
            && !self.arm_condition(code)
        {
            return Ok(());
        }
        let operands = match &decoded.operands {
            Ok(operands) => operands.as_slice(),
            Err(message) => {
                return Err(SimError(format!(
                    "{message} at 0x{old_pc:x}: {}",
                    decoded.instruction.assembly
                )));
            }
        };
        let handled = match self.architecture {
            Architecture::Arm64 => self.execute_arm64(decoded, operands)?,
            Architecture::Arm32 => self.execute_arm32(decoded, operands)?,
            Architecture::X86 | Architecture::X64 => self.execute_x86(decoded, operands)?,
            Architecture::Vole => false,
        };
        if handled {
            Ok(())
        } else {
            Err(self.unsupported(decoded))
        }
    }

    fn record_size(undo: &Undo) -> usize {
        256 + undo.instruction.assembly.len()
            + undo.registers.len() * 48
            + undo
                .memory
                .iter()
                .map(|c| c.before.len() + c.after.len() + 32)
                .sum::<usize>()
            + undo.output_added.len()
            + undo.memory_reads.len() * 24
    }
    fn register_changes(
        &self,
        deltas: &[(u8, u64, u64)],
        before: u8,
        after: u8,
    ) -> Vec<RegisterChange> {
        let mut changes: Vec<_> = deltas
            .iter()
            .map(|(index, before, after)| RegisterChange {
                name: self.names[usize::from(*index)].clone(),
                before: *before,
                after: *after,
            })
            .collect();
        for (name, bit) in self.flag_names() {
            if (before ^ after) & bit != 0 {
                changes.push(RegisterChange {
                    name: (*name).to_string(),
                    before: u64::from(before & bit != 0),
                    after: u64::from(after & bit != 0),
                });
            }
        }
        changes
    }
    fn trace_record(&self, undo: &Undo) -> StepRecord {
        StepRecord {
            pc_before: undo.pc_before,
            pc_after: undo.pc_after,
            instruction: (*undo.instruction).clone(),
            registers: self.register_changes(&undo.registers, undo.flags_before, undo.flags_after),
            memory: undo.memory.clone(),
            memory_reads: undo.memory_reads.clone(),
            output_added: undo.output_added.clone(),
            halted: undo.halted,
        }
    }
}

/// Memory operand helpers shared by the executors.
impl ScalarMachine {
    fn x86_address(&self, memory: &Mem) -> Result<u64, SimError> {
        if let Some(segment) = &memory.segment
            && segment != "es"
        {
            return Err(SimError(
                "Segment-relative memory is outside the flat teaching memory model".into(),
            ));
        }
        let mut address = memory.displacement;
        if let Some(base) = memory.base {
            address = address.wrapping_add(self.get(base));
        }
        if let Some(index) = memory.index {
            address = address.wrapping_add(self.get(index).wrapping_mul(memory.scale));
        }
        Ok(self.address(address))
    }
}

impl Machine for ScalarMachine {
    fn load(&mut self, program: &Program) -> Result<(), SimError> {
        if program.architecture != self.architecture || self.architecture == Architecture::Vole {
            return Err(SimError(
                "Program architecture does not match scalar machine".into(),
            ));
        }
        let mut total = 0_usize;
        let mut intervals = vec![];
        for region in &program.regions {
            total = total
                .checked_add(region.bytes.len())
                .ok_or_else(|| SimError("Memory size overflow".into()))?;
            if total > MEMORY_BUDGET {
                return Err(SimError("Machine mappings exceed four MiB".into()));
            }
            let end = region
                .base
                .checked_add(region.bytes.len() as u64)
                .ok_or_else(|| SimError("Memory mapping address overflow".into()))?;
            if self.architecture.bits() == 32 && end > 1_u64 << 32 {
                return Err(SimError("Mapping exceeds 32-bit address space".into()));
            }
            if !region.bytes.is_empty() {
                intervals.push((region.base, end));
            }
        }
        intervals.sort_unstable();
        if intervals.windows(2).any(|pair| pair[0].1 > pair[1].0) {
            return Err(SimError("Memory mappings overlap".into()));
        }
        if !program
            .regions
            .iter()
            .any(|r| r.contains(program.entry) && r.executable)
            || self.is_arm() && !program.entry.is_multiple_of(4)
        {
            return Err(SimError(
                "Entry must be mapped, executable, and aligned for the target".into(),
            ));
        }
        let mut fresh = Self::new(self.architecture);
        fresh.memory = program.regions.clone();
        fresh.pc = program.entry;
        for (name, value) in &program.initial_registers {
            let register = fresh.register(name)?;
            fresh.set(register, *value)?;
        }
        fresh.program_index = program
            .instructions
            .iter()
            .enumerate()
            .map(|(index, instruction)| (instruction.address, index))
            .collect();
        fresh.program = Some(program.clone());
        *self = fresh;
        Ok(())
    }
    fn step(&mut self) -> Result<StepRecord, SimError> {
        if self.program.is_none() {
            return Err(SimError("Assemble and load a program first".into()));
        }
        if self.halted {
            return Err(SimError(
                "Machine has halted; reset or reverse to continue".into(),
            ));
        }
        if self.steps == u64::MAX {
            return Err(SimError(
                "Instruction counter overflow; reset the machine".into(),
            ));
        }
        let decoded = self.decoded()?;
        let old_registers = self.registers;
        let old_flags = self.flags;
        let old_pc = self.pc;
        let output_len = self.output.len();
        self.writes.clear();
        self.reads.clear();
        if let Err(error) = self.execute(&decoded) {
            for change in std::mem::take(&mut self.writes).into_iter().rev() {
                self.write_bytes(change.address, &change.before, false)?;
            }
            self.registers = old_registers;
            self.flags = old_flags;
            self.pc = old_pc;
            self.output.truncate(output_len);
            self.halted = false;
            self.reads.clear();
            return Err(error);
        }
        let deltas: Vec<(u8, u64, u64)> = self
            .order
            .iter()
            .filter_map(|index| {
                let i = usize::from(*index);
                (old_registers[i] != self.registers[i]).then_some((
                    *index,
                    old_registers[i],
                    self.registers[i],
                ))
            })
            .collect();
        let mut undo = Undo {
            pc_before: old_pc,
            pc_after: self.pc,
            instruction: decoded.instruction.clone(),
            registers: deltas,
            flags_before: old_flags,
            flags_after: self.flags,
            memory: std::mem::take(&mut self.writes),
            memory_reads: std::mem::take(&mut self.reads),
            output_added: self.output[output_len..].to_vec(),
            halted: self.halted,
            size: 0,
        };
        undo.size = Self::record_size(&undo);
        let record = self.trace_record(&undo);
        self.steps += 1;
        self.history_bytes += undo.size;
        self.history.push_back(undo);
        while self.history.len() > HISTORY_STEPS || self.history_bytes > HISTORY_BYTES {
            if let Some(removed) = self.history.pop_front() {
                self.history_bytes -= removed.size;
            }
        }
        Ok(record)
    }
    fn reverse_step(&mut self) -> Result<(), SimError> {
        let undo = self
            .history
            .pop_back()
            .ok_or_else(|| SimError("No retained instruction to reverse".into()))?;
        for change in undo.memory.iter().rev() {
            self.write_bytes(change.address, &change.before, false)?;
        }
        for (index, before, _) in &undo.registers {
            self.registers[usize::from(*index)] = *before;
        }
        self.flags = undo.flags_before;
        self.pc = undo.pc_before;
        self.output
            .truncate(self.output.len() - undo.output_added.len());
        self.halted = false;
        self.steps -= 1;
        self.history_bytes -= undo.size;
        Ok(())
    }
    fn reset(&mut self) -> Result<(), SimError> {
        let program = self
            .program
            .clone()
            .ok_or_else(|| SimError("No loaded program to reset".into()))?;
        self.load(&program)
    }
    fn snapshot(&self) -> Snapshot {
        let bits = self.architecture.bits();
        let mut registers: Vec<_> = self
            .order
            .iter()
            .map(|index| RegisterValue {
                name: self.names[usize::from(*index)].clone(),
                value: self.registers[usize::from(*index)],
                bits,
            })
            .collect();
        registers.push(RegisterValue {
            name: self.pc_name().into(),
            value: self.pc,
            bits,
        });
        Snapshot {
            architecture: self.architecture,
            pc: self.pc,
            registers,
            flags: self
                .flag_names()
                .iter()
                .map(|(name, bit)| ((*name).to_string(), self.flag(*bit)))
                .collect::<BTreeMap<_, _>>(),
            memory: self.memory.clone(),
            output: self.output.clone(),
            halted: self.halted,
            steps: self.steps,
            trace: self
                .history
                .iter()
                .map(|undo| self.trace_record(undo))
                .collect(),
        }
    }
    fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), SimError> {
        let program = self
            .program
            .as_ref()
            .ok_or_else(|| SimError("Load the snapshot's program before restoring".into()))?;
        if snapshot.architecture != self.architecture {
            return Err(SimError(
                "Snapshot architecture does not match the machine".into(),
            ));
        }
        if snapshot.memory.len() != program.regions.len() {
            return Err(SimError(
                "Snapshot memory layout differs from the program".into(),
            ));
        }
        for (saved, original) in snapshot.memory.iter().zip(&program.regions) {
            if saved.base != original.base
                || saved.bytes.len() != original.bytes.len()
                || saved.writable != original.writable
                || saved.executable != original.executable
            {
                return Err(SimError(
                    "Snapshot mappings or permissions differ from the loaded program".into(),
                ));
            }
        }
        if snapshot.output.len() > OUTPUT_BUDGET {
            return Err(SimError("Snapshot output exceeds one MiB".into()));
        }
        if snapshot.pc != self.address(snapshot.pc)
            || self.is_arm() && !snapshot.pc.is_multiple_of(4)
        {
            return Err(SimError(
                "Snapshot PC exceeds the target width or alignment".into(),
            ));
        }
        if !snapshot
            .flags
            .keys()
            .map(String::as_str)
            .eq(self.flag_names().iter().map(|(name, _)| *name))
        {
            return Err(SimError(
                "Snapshot contains unknown or missing flags".into(),
            ));
        }
        let mut restored = Self::new(self.architecture);
        restored.load(program)?;
        if snapshot.registers.len() != restored.names.len() + 1 {
            return Err(SimError(
                "Snapshot contains missing or duplicate registers".into(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for register in &snapshot.registers {
            let view = restored.register(&register.name)?;
            if register.value > Self::mask(view.bits)
                || register.bits != view.bits
                || view.offset != 0
                || !seen.insert(view.index)
            {
                return Err(SimError(
                    "Snapshot register width/value or alias is invalid".into(),
                ));
            }
            if view.index == ZR {
                return Err(SimError("Snapshot cannot contain the zero register".into()));
            }
            if view.index == PC {
                restored.pc = register.value;
            } else {
                restored.set(view, register.value)?;
            }
        }
        if restored.pc != snapshot.pc {
            return Err(SimError("Snapshot PC and PC register disagree".into()));
        }
        restored.memory = snapshot.memory.clone();
        restored.flags = 0;
        for (name, bit) in self.flag_names() {
            if snapshot.flags[*name] {
                restored.flags |= bit;
            }
        }
        restored.output = snapshot.output.clone();
        restored.halted = snapshot.halted;
        restored.steps = snapshot.steps;
        *self = restored;
        Ok(())
    }
    fn write_register(&mut self, name: &str, value: u64) -> Result<(), SimError> {
        let register = self.register(name)?;
        if register.index == PC {
            self.branch(value)?;
        } else {
            self.set(register, value)?;
        }
        self.clear_history();
        self.halted = false;
        Ok(())
    }
    fn write_memory(&mut self, address: u64, bytes: &[u8]) -> Result<(), SimError> {
        self.write_bytes(address, bytes, false)?;
        self.clear_history();
        self.halted = false;
        Ok(())
    }

    fn pc(&self) -> u64 {
        self.pc
    }
    fn read_register(&self, name: &str) -> Option<u64> {
        operand::parse_register(self.architecture, name.trim()).map(|register| self.peek(register))
    }
    fn read_memory(&self, address: u64, length: usize) -> Option<Vec<u8>> {
        self.read_bytes(address, length).ok()
    }
    fn halted(&self) -> bool {
        self.halted
    }
    fn steps(&self) -> u64 {
        self.steps
    }
}
