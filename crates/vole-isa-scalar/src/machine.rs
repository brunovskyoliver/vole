use crate::decode;
use std::collections::{BTreeMap, VecDeque};
use vole_core::{
    Architecture, Instruction, Machine, MemoryAccess, MemoryChange, MemoryRegion, Program,
    RegisterChange, RegisterValue, SimError, Snapshot, StepRecord,
};

const HISTORY_BYTES: usize = 2 * 1024 * 1024;
const HISTORY_STEPS: usize = 4096;
const MEMORY_BUDGET: usize = 4 * 1024 * 1024;
const OUTPUT_BUDGET: usize = 1024 * 1024;

#[derive(Clone)]
struct Undo {
    record: StepRecord,
    flags: BTreeMap<String, bool>,
    size: usize,
}

/// A bounded deterministic interpreter for the documented scalar instruction subset.
/// Host CPU instructions are never used to execute guest bytes.
pub struct ScalarMachine {
    architecture: Architecture,
    program: Option<Program>,
    registers: BTreeMap<String, u64>,
    flags: BTreeMap<String, bool>,
    memory: Vec<MemoryRegion>,
    pc: u64,
    output: Vec<u8>,
    halted: bool,
    steps: u64,
    history: VecDeque<Undo>,
    history_bytes: usize,
    writes: Vec<MemoryChange>,
    reads: Vec<MemoryAccess>,
}

impl ScalarMachine {
    pub fn new(architecture: Architecture) -> Self {
        let mut machine = Self {
            architecture,
            program: None,
            registers: BTreeMap::new(),
            flags: BTreeMap::new(),
            memory: vec![],
            pc: 0,
            output: vec![],
            halted: false,
            steps: 0,
            history: VecDeque::new(),
            history_bytes: 0,
            writes: vec![],
            reads: vec![],
        };
        machine.initialize_registers();
        machine
    }

    fn initialize_registers(&mut self) {
        self.registers.clear();
        self.flags.clear();
        match self.architecture {
            Architecture::Arm32 => {
                for n in 0..15 {
                    self.registers.insert(format!("r{n}"), 0);
                }
            }
            Architecture::Arm64 => {
                for n in 0..31 {
                    self.registers.insert(format!("x{n}"), 0);
                }
                self.registers.insert("sp".into(), 0);
            }
            Architecture::X86 => {
                for name in ["eax", "ebx", "ecx", "edx", "esi", "edi", "esp", "ebp"] {
                    self.registers.insert(name.into(), 0);
                }
            }
            Architecture::X64 => {
                for name in ["rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rsp", "rbp"] {
                    self.registers.insert(name.into(), 0);
                }
                for n in 8..16 {
                    self.registers.insert(format!("r{n}"), 0);
                }
            }
            Architecture::Vole => {}
        }
        for name in if self.is_arm() {
            vec!["N", "Z", "C", "V"]
        } else {
            vec!["SF", "ZF", "CF", "OF", "PF", "AF"]
        } {
            self.flags.insert(name.into(), false);
        }
    }

    fn is_arm(&self) -> bool {
        matches!(self.architecture, Architecture::Arm32 | Architecture::Arm64)
    }
    fn mask(bits: u8) -> u64 {
        if bits == 64 {
            u64::MAX
        } else {
            (1_u64 << bits) - 1
        }
    }
    fn address(&self, value: u64) -> u64 {
        value & Self::mask(self.architecture.bits())
    }
    fn pc_register(&self) -> &'static str {
        match self.architecture {
            Architecture::X86 => "eip",
            Architecture::X64 => "rip",
            _ => "pc",
        }
    }
    fn stack_register(&self) -> &'static str {
        match self.architecture {
            Architecture::Arm32 => "r13",
            Architecture::Arm64 => "sp",
            Architecture::X86 => "esp",
            _ => "rsp",
        }
    }
    fn clear_history(&mut self) {
        self.history.clear();
        self.history_bytes = 0;
    }

    // Returns canonical register, bit width, bit offset, and whether writes zero upper bits.
    fn alias(&self, name: &str) -> Result<(String, u8, u8, bool), SimError> {
        let name = name.trim().to_ascii_lowercase();
        if name == self.pc_register() || self.architecture == Architecture::Arm32 && name == "r15" {
            return Ok(("pc".into(), self.architecture.bits(), 0, true));
        }
        match self.architecture {
            Architecture::Arm32 => {
                let name = match name.as_str() {
                    "sp" => "r13",
                    "lr" => "r14",
                    "fp" => "r11",
                    "ip" => "r12",
                    "sb" => "r9",
                    "sl" => "r10",
                    _ => &name,
                };
                if self.registers.contains_key(name) {
                    return Ok((name.into(), 32, 0, true));
                }
            }
            Architecture::Arm64 => {
                if matches!(name.as_str(), "xzr" | "wzr") {
                    return Ok(("zr".into(), if name == "wzr" { 32 } else { 64 }, 0, true));
                }
                if name == "wsp" {
                    return Ok(("sp".into(), 32, 0, true));
                }
                if let Some(number) = name
                    .strip_prefix('w')
                    .and_then(|n| n.parse::<u8>().ok())
                    .filter(|n| *n < 31)
                {
                    return Ok((format!("x{number}"), 32, 0, true));
                }
                let canonical = match name.as_str() {
                    "fp" => "x29",
                    "lr" => "x30",
                    _ => &name,
                };
                if self.registers.contains_key(canonical) {
                    return Ok((canonical.into(), 64, 0, true));
                }
            }
            Architecture::X86 | Architecture::X64 => {
                for (base, low16, low8, high8, dword) in [
                    ("rax", "ax", "al", "ah", "eax"),
                    ("rbx", "bx", "bl", "bh", "ebx"),
                    ("rcx", "cx", "cl", "ch", "ecx"),
                    ("rdx", "dx", "dl", "dh", "edx"),
                    ("rsi", "si", "sil", "", "esi"),
                    ("rdi", "di", "dil", "", "edi"),
                    ("rsp", "sp", "spl", "", "esp"),
                    ("rbp", "bp", "bpl", "", "ebp"),
                ] {
                    let canonical = if self.architecture == Architecture::X64 {
                        base
                    } else {
                        dword
                    };
                    if name == dword {
                        return Ok((canonical.into(), 32, 0, true));
                    }
                    if name == base && self.architecture == Architecture::X64 {
                        return Ok((canonical.into(), 64, 0, true));
                    }
                    if name == low16 {
                        return Ok((canonical.into(), 16, 0, false));
                    }
                    if name == low8
                        && (!matches!(low8, "sil" | "dil" | "spl" | "bpl")
                            || self.architecture == Architecture::X64)
                    {
                        return Ok((canonical.into(), 8, 0, false));
                    }
                    if !high8.is_empty() && name == high8 {
                        return Ok((canonical.into(), 8, 8, false));
                    }
                }
                if self.architecture == Architecture::X64 {
                    for number in 8..16 {
                        let base = format!("r{number}");
                        for (suffix, bits) in [("", 64), ("d", 32), ("w", 16), ("b", 8)] {
                            if name == format!("{base}{suffix}") {
                                return Ok((base, bits, 0, bits >= 32));
                            }
                        }
                    }
                }
            }
            Architecture::Vole => {}
        }
        Err(SimError(format!(
            "Unknown {} register {name}",
            self.architecture.name()
        )))
    }

    fn reg(&self, name: &str) -> Result<u64, SimError> {
        let (base, bits, offset, _) = self.alias(name)?;
        let value = if base == "zr" {
            0
        } else if base == "pc" {
            self.pc
        } else {
            self.registers[&base]
        };
        Ok((value >> offset) & Self::mask(bits))
    }
    fn operand_reg(&self, name: &str, old_pc: u64, next_pc: u64) -> Result<u64, SimError> {
        if self.alias(name)?.0 == "pc" {
            Ok(if self.architecture == Architecture::Arm32 {
                old_pc.wrapping_add(8) & 0xffff_ffff
            } else {
                next_pc
            })
        } else {
            self.reg(name)
        }
    }
    fn set_reg(&mut self, name: &str, value: u64) -> Result<(), SimError> {
        let (base, bits, offset, zero_upper) = self.alias(name)?;
        if base == "zr" {
            return Ok(());
        }
        if base == "pc" {
            self.branch(value)?;
            return Ok(());
        }
        let value = value & Self::mask(bits);
        let previous = self.registers[&base];
        self.registers.insert(
            base,
            if zero_upper {
                value
            } else {
                previous & !(Self::mask(bits) << offset) | value << offset
            },
        );
        Ok(())
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

    fn read_bytes(&self, address: u64, length: usize) -> Result<Vec<u8>, SimError> {
        if length > MEMORY_BUDGET {
            return Err(SimError("Memory read exceeds allocation budget".into()));
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
    fn read(&mut self, address: u64, length: usize) -> Result<u64, SimError> {
        let bytes = self.read_bytes(address, length)?;
        let mut value = 0;
        for (offset, byte) in bytes.into_iter().enumerate() {
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
            let at = address
                .checked_add(offset as u64)
                .ok_or_else(|| SimError("Memory address overflow".into()))?;
            if !self.memory.iter().any(|r| r.contains(at) && r.writable) {
                return Err(SimError(format!("Read-only memory at 0x{at:x}")));
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
        let mut bytes = Vec::new();
        for offset in 0..maximum {
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

    fn flag(&self, name: &str) -> bool {
        self.flags.get(name).copied().unwrap_or(false)
    }
    fn set_flag(&mut self, name: &str, value: bool) {
        self.flags.insert(name.into(), value);
    }
    fn arithmetic_flags(&mut self, a: u64, b: u64, result: u64, bits: u8, subtract: bool) {
        let mask = Self::mask(bits);
        let a = a & mask;
        let b = b & mask;
        let result = result & mask;
        let sign = 1_u64 << (bits - 1);
        let carry = if subtract {
            a < b
        } else {
            u128::from(a) + u128::from(b) > u128::from(mask)
        };
        let overflow = if subtract {
            (a ^ b) & (a ^ result) & sign != 0
        } else {
            !(a ^ b) & (a ^ result) & sign != 0
        };
        if self.is_arm() {
            self.set_flag("N", result & sign != 0);
            self.set_flag("Z", result == 0);
            self.set_flag("C", if subtract { !carry } else { carry });
            self.set_flag("V", overflow);
        } else {
            self.set_flag("SF", result & sign != 0);
            self.set_flag("ZF", result == 0);
            self.set_flag("CF", carry);
            self.set_flag("OF", overflow);
            self.set_flag("PF", (result as u8).count_ones().is_multiple_of(2));
            self.set_flag("AF", (a ^ b ^ result) & 0x10 != 0);
        }
    }
    fn arm32_shifter_carry(
        &mut self,
        instruction: &Instruction,
        old_pc: u64,
        next_pc: u64,
    ) -> Result<(), SimError> {
        if self.architecture != Architecture::Arm32 {
            return Ok(());
        }
        let word = u32::from_le_bytes(
            instruction
                .bytes
                .as_slice()
                .try_into()
                .map_err(|_| SimError("Invalid A32 word".into()))?,
        );
        if word & (1 << 25) != 0 {
            let rotate = ((word >> 8) & 15) * 2;
            if rotate > 0 {
                self.set_flag("C", (word & 255).rotate_right(rotate) >> 31 != 0);
            }
            return Ok(());
        }
        let value = self.operand_reg(&format!("r{}", word & 15), old_pc, next_pc)? as u32;
        let kind = (word >> 5) & 3;
        let register_shift = word & 16 != 0;
        let mut count = if register_shift {
            self.operand_reg(&format!("r{}", (word >> 8) & 15), old_pc, next_pc)? as u32 & 255
        } else {
            (word >> 7) & 31
        };
        if count == 0 && !register_shift && matches!(kind, 1 | 2) {
            count = 32;
        }
        if count == 0 {
            return Ok(());
        }
        let carry = match kind {
            0 => count <= 32 && value >> (32 - count) & 1 != 0,
            1 => count <= 32 && value >> (count - 1) & 1 != 0,
            2 => {
                if count >= 32 {
                    value >> 31 != 0
                } else {
                    value >> (count - 1) & 1 != 0
                }
            }
            _ => value.rotate_right(count % 32) >> 31 != 0,
        };
        self.set_flag("C", carry);
        Ok(())
    }
    fn logical_flags(&mut self, value: u64, bits: u8) {
        let value = value & Self::mask(bits);
        if self.is_arm() {
            self.set_flag("N", value >> (bits - 1) != 0);
            self.set_flag("Z", value == 0);
            if self.architecture == Architecture::Arm64 {
                self.set_flag("C", false);
                self.set_flag("V", false);
            }
        } else {
            self.set_flag("SF", value >> (bits - 1) != 0);
            self.set_flag("ZF", value == 0);
            self.set_flag("CF", false);
            self.set_flag("OF", false);
            self.set_flag("PF", (value as u8).count_ones().is_multiple_of(2));
        }
    }
    fn arm_condition(&self, condition: &str) -> Result<bool, SimError> {
        let z = self.flag("Z");
        let n = self.flag("N");
        let c = self.flag("C");
        let v = self.flag("V");
        Ok(match condition {
            "eq" => z,
            "ne" => !z,
            "cs" | "hs" => c,
            "cc" | "lo" => !c,
            "mi" => n,
            "pl" => !n,
            "vs" => v,
            "vc" => !v,
            "hi" => c && !z,
            "ls" => !c || z,
            "ge" => n == v,
            "lt" => n != v,
            "gt" => !z && n == v,
            "le" => z || n != v,
            "al" => true,
            _ => return Err(SimError(format!("Unsupported ARM condition {condition}"))),
        })
    }
    fn x86_condition(&self, condition: &str) -> Result<bool, SimError> {
        let z = self.flag("ZF");
        let s = self.flag("SF");
        let c = self.flag("CF");
        let o = self.flag("OF");
        let p = self.flag("PF");
        Ok(match condition {
            "e" | "z" => z,
            "ne" | "nz" => !z,
            "a" | "nbe" => !c && !z,
            "ae" | "nb" | "nc" => !c,
            "b" | "c" | "nae" => c,
            "be" | "na" => c || z,
            "g" | "nle" => !z && s == o,
            "ge" | "nl" => s == o,
            "l" | "nge" => s != o,
            "le" | "ng" => z || s != o,
            "s" => s,
            "ns" => !s,
            "o" => o,
            "no" => !o,
            "p" | "pe" => p,
            "np" | "po" => !p,
            _ => return Err(SimError(format!("Unsupported x86 condition {condition}"))),
        })
    }

    fn immediate(text: &str) -> Result<u64, SimError> {
        let text = text.trim().trim_start_matches('#');
        if let Some(positive) = text.strip_prefix('-') {
            return Ok(Self::immediate(positive)?.wrapping_neg());
        }
        if let Some(hex) = text.strip_prefix("0x") {
            u64::from_str_radix(hex, 16)
        } else {
            text.parse::<u64>()
        }
        .map_err(|_| SimError(format!("Unsupported decoded operand {text}")))
    }
    fn value(
        &mut self,
        operand: &str,
        bits: u8,
        old_pc: u64,
        next_pc: u64,
    ) -> Result<u64, SimError> {
        if operand.contains('[') {
            let address = self.x86_address(operand, old_pc, next_pc)?;
            return self.read(address, usize::from(self.operand_bits(operand, bits) / 8));
        }
        if self.alias(operand).is_ok() {
            self.operand_reg(operand, old_pc, next_pc)
        } else {
            Self::immediate(operand)
        }
    }
    fn operand_bits(&self, operand: &str, fallback: u8) -> u8 {
        if let Ok((_, bits, _, _)) = self.alias(operand) {
            return bits;
        }
        if operand.starts_with("byte ptr ") {
            8
        } else if operand.starts_with("word ptr ") {
            16
        } else if operand.starts_with("dword ptr ") {
            32
        } else if operand.starts_with("qword ptr ") {
            64
        } else {
            fallback
        }
    }
    fn destination(
        &mut self,
        operand: &str,
        bits: u8,
        value: u64,
        old_pc: u64,
        next_pc: u64,
    ) -> Result<(), SimError> {
        if operand.contains('[') {
            let address = self.x86_address(operand, old_pc, next_pc)?;
            self.write(
                address,
                usize::from(self.operand_bits(operand, bits) / 8),
                value,
            )
        } else {
            self.set_reg(operand, value)
        }
    }
    fn x86_address(&self, operand: &str, old_pc: u64, next_pc: u64) -> Result<u64, SimError> {
        let start = operand
            .find('[')
            .ok_or_else(|| SimError("Expected a memory operand".into()))?;
        let end = operand
            .find(']')
            .ok_or_else(|| SimError("Malformed memory operand".into()))?;
        if operand[..start].contains(':') {
            return Err(SimError("Segment-relative memory is unsupported".into()));
        }
        let expression = operand[start + 1..end].replace(' ', "");
        let mut total = 0_u64;
        let mut term = String::new();
        let mut sign = 1_i8;
        for character in expression.chars().chain(std::iter::once('+')) {
            if matches!(character, '+' | '-') {
                if !term.is_empty() {
                    let mut product = 1_u64;
                    for factor in term.split('*') {
                        product = product.wrapping_mul(if self.alias(factor).is_ok() {
                            self.operand_reg(factor, old_pc, next_pc)?
                        } else {
                            Self::immediate(factor)?
                        });
                    }
                    total = if sign < 0 {
                        total.wrapping_sub(product)
                    } else {
                        total.wrapping_add(product)
                    };
                    term.clear();
                }
                sign = if character == '-' { -1 } else { 1 };
            } else {
                term.push(character);
            }
        }
        Ok(self.address(total))
    }

    fn shifted(
        &mut self,
        operand: &str,
        modifier: Option<&str>,
        bits: u8,
        old_pc: u64,
        next_pc: u64,
    ) -> Result<u64, SimError> {
        let value = self.value(operand, bits, old_pc, next_pc)? & Self::mask(bits);
        let Some(modifier) = modifier else {
            return Ok(value);
        };
        let (operation, count) = if let Some(parts) = modifier.split_once(' ') {
            parts
        } else if matches!(
            modifier,
            "uxtw" | "sxtw" | "uxtb" | "uxth" | "sxtb" | "sxth" | "sxtx" | "uxtx"
        ) {
            (modifier, "#0")
        } else {
            return Err(SimError(format!("Unsupported operand modifier {modifier}")));
        };
        let register_count = self.architecture == Architecture::Arm32 && self.alias(count).is_ok();
        let mut count = self.value(count, bits, old_pc, next_pc)? as u32;
        if register_count {
            count &= 255;
        }
        let result = match operation {
            "lsl" => {
                if count >= u32::from(bits) {
                    0
                } else {
                    value << count
                }
            }
            "lsr" => {
                if count >= u32::from(bits) {
                    0
                } else {
                    value >> count
                }
            }
            "asr" => {
                if count >= u32::from(bits) {
                    if value >> (bits - 1) != 0 {
                        Self::mask(bits)
                    } else {
                        0
                    }
                } else {
                    ((Self::signed(value, bits) >> count) as u64) & Self::mask(bits)
                }
            }
            "ror" => {
                let count = count % u32::from(bits);
                if count == 0 {
                    value
                } else {
                    value >> count | value << (u32::from(bits) - count)
                }
            }
            "uxtb" => (value & 0xff).checked_shl(count).unwrap_or(0),
            "uxth" => (value & 0xffff).checked_shl(count).unwrap_or(0),
            "sxtb" => (value as u8 as i8 as i64 as u64)
                .checked_shl(count)
                .unwrap_or(0),
            "sxth" => (value as u16 as i16 as i64 as u64)
                .checked_shl(count)
                .unwrap_or(0),
            "uxtx" | "sxtx" => value.checked_shl(count).unwrap_or(0),
            "uxtw" => (value & 0xffff_ffff).checked_shl(count).unwrap_or(0),
            "sxtw" => (value as u32 as i32 as i64 as u64)
                .checked_shl(count)
                .unwrap_or(0),
            _ => return Err(SimError(format!("Unsupported operand modifier {modifier}"))),
        };
        Ok(result & Self::mask(bits))
    }
    fn signed(value: u64, bits: u8) -> i64 {
        ((value << (64 - bits)) as i64) >> (64 - bits)
    }

    fn arm_address(
        &mut self,
        operand: &str,
        post: Option<&str>,
        old_pc: u64,
        next_pc: u64,
    ) -> Result<(u64, Option<(String, u64)>), SimError> {
        let open = operand
            .find('[')
            .ok_or_else(|| SimError("Expected bracketed ARM memory operand".into()))?;
        let close = operand
            .find(']')
            .ok_or_else(|| SimError("Malformed ARM memory operand".into()))?;
        let parts = split_operands(&operand[open + 1..close]);
        let base = parts
            .first()
            .ok_or_else(|| SimError("Missing ARM address register".into()))?;
        let base_value = self.operand_reg(base, old_pc, next_pc)?;
        let offset = if parts.len() > 1 {
            let negative = parts[1].starts_with('-');
            let value = self.shifted(
                parts[1].trim_start_matches('-'),
                parts.get(2).map(String::as_str),
                self.architecture.bits(),
                old_pc,
                next_pc,
            )?;
            if negative {
                value.wrapping_neg()
            } else {
                value
            }
        } else {
            0
        };
        let adjusted = self.address(base_value.wrapping_add(offset));
        let update = if let Some(post) = post {
            {
                let displacement = self.value(post, self.architecture.bits(), old_pc, next_pc)?;
                Some((
                    base.to_string(),
                    self.address(base_value.wrapping_add(displacement)),
                ))
            }
        } else if operand.ends_with('!') {
            Some((base.to_string(), adjusted))
        } else {
            None
        };
        Ok((if post.is_some() { base_value } else { adjusted }, update))
    }
    fn push(&mut self, value: u64, width: usize) -> Result<(), SimError> {
        let sp = self.stack_register();
        let address = self.address(self.reg(sp)?.wrapping_sub(width as u64));
        self.write(address, width, value)?;
        self.set_reg(sp, address)
    }
    fn pop(&mut self, width: usize) -> Result<u64, SimError> {
        let sp = self.stack_register();
        let address = self.reg(sp)?;
        let value = self.read(address, width)?;
        self.set_reg(sp, self.address(address.wrapping_add(width as u64)))?;
        Ok(value)
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
    fn syscall(&mut self) -> Result<(), SimError> {
        let (number, exit, write, fd, address, count, result) = match self.architecture {
            Architecture::Arm64 => (
                self.reg("x8")?,
                93,
                64,
                self.reg("x0")?,
                self.reg("x1")?,
                self.reg("x2")?,
                "x0",
            ),
            Architecture::Arm32 => (
                self.reg("r7")?,
                1,
                4,
                self.reg("r0")?,
                self.reg("r1")?,
                self.reg("r2")?,
                "r0",
            ),
            Architecture::X86 => (
                self.reg("eax")?,
                1,
                4,
                self.reg("ebx")?,
                self.reg("ecx")?,
                self.reg("edx")?,
                "eax",
            ),
            Architecture::X64 => (
                self.reg("rax")?,
                60,
                1,
                self.reg("rdi")?,
                self.reg("rsi")?,
                self.reg("rdx")?,
                "rax",
            ),
            Architecture::Vole => return Err(SimError("Invalid scalar architecture".into())),
        };
        if number == exit {
            self.halted = true;
            return Ok(());
        }
        if number != write || fd != 1 {
            return Err(SimError(format!(
                "Unsupported teaching syscall {number}; only stdout write and exit are supported"
            )));
        }
        let length =
            usize::try_from(count).map_err(|_| SimError("Output length overflow".into()))?;
        if length > OUTPUT_BUDGET {
            return Err(SimError("Output write exceeds one MiB".into()));
        }
        let bytes = self.read_bytes(address, length)?;
        self.reads.push(MemoryAccess { address, length });
        self.emit(&bytes)?;
        self.set_reg(result, count)
    }

    fn execute(&mut self, instruction: &Instruction) -> Result<(), SimError> {
        if !self.is_arm() {
            for prefix in &instruction.bytes {
                match prefix {
                    0x67 => return Err(SimError("Address-size override is unsupported; use the target's default 32/64-bit addressing".into())),
                    0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 => return Err(SimError("Segment overrides are outside the flat teaching memory model".into())),
                    0x66 | 0xf0 | 0xf2 | 0xf3 => {},
                    0x40..=0x4f if self.architecture == Architecture::X64 => {},
                    _ => break,
                }
            }
        }
        let old_pc = self.pc;
        let next_pc = self.address(old_pc.wrapping_add(instruction.bytes.len() as u64));
        self.pc = next_pc;
        let (mnemonic, operand_text) = instruction
            .assembly
            .split_once(' ')
            .unwrap_or((&instruction.assembly, ""));
        let mut mnemonic = mnemonic.to_owned();
        if self.architecture == Architecture::Arm32 {
            let word = u32::from_le_bytes(
                instruction
                    .bytes
                    .as_slice()
                    .try_into()
                    .map_err(|_| SimError("A32 instruction must contain four bytes".into()))?,
            );
            let condition = word >> 28;
            if condition < 14 {
                let suffix = [
                    "eq", "ne", "hs", "lo", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt",
                    "le",
                ][condition as usize];
                if !self.arm_condition(suffix)? {
                    return Ok(());
                }
                if mnemonic.ends_with(suffix)
                    || condition == 2 && mnemonic.ends_with("cs")
                    || condition == 3 && mnemonic.ends_with("cc")
                {
                    mnemonic.truncate(mnemonic.len() - 2);
                }
            } else if condition == 15 {
                return Err(SimError(
                    "A32 unconditional extension/Thumb instruction is outside the supported subset"
                        .into(),
                ));
            }
        }
        let operands = split_operands(operand_text);
        let at = |index: usize| {
            operands
                .get(index)
                .map(String::as_str)
                .ok_or_else(|| SimError(format!("Missing operand in {}", instruction.assembly)))
        };
        let bits = operands.first().map_or(self.architecture.bits(), |op| {
            self.operand_bits(op, self.architecture.bits())
        });
        let arm = self.is_arm();
        if self.architecture == Architecture::Arm32
            && matches!(
                mnemonic.as_str(),
                "movs"
                    | "mvns"
                    | "adds"
                    | "subs"
                    | "ands"
                    | "bics"
                    | "lsls"
                    | "lsrs"
                    | "asrs"
                    | "rors"
            )
            && operands
                .first()
                .is_some_and(|operand| self.alias(operand).is_ok_and(|alias| alias.0 == "pc"))
        {
            return Err(SimError("A32 flag-setting writes to PC require exception-return semantics and are unsupported".into()));
        }
        match mnemonic.as_str() {
            "nop" => {}
            "int3" | "hlt" => {
                if arm {
                    return Err(SimError("Unexpected ARM halt encoding".into()));
                }
                self.halted = true;
            }
            "brk" | "bkpt" => {
                let trap = Self::immediate(at(0)?)?;
                if trap == 0 {
                    self.halted = true;
                } else if trap == 1 {
                    let register = if self.architecture == Architecture::Arm64 {
                        "x0"
                    } else {
                        "r0"
                    };
                    let byte = self.reg(register)? as u8;
                    self.emit(&[byte])?;
                } else {
                    return Err(SimError(format!(
                        "Unknown teaching trap {trap}; 0 halts and 1 emits register 0"
                    )));
                }
            }
            "svc" => {
                if Self::immediate(at(0)?)? != 0 {
                    return Err(SimError("Only svc #0 supports the teaching ABI".into()));
                }
                self.syscall()?;
            }
            "syscall" => {
                if self.architecture != Architecture::X64 {
                    return Err(SimError("SYSCALL teaching ABI requires x64".into()));
                }
                // SYSCALL architecturally saves RIP and RFLAGS in RCX/R11.
                self.set_reg("rcx", next_pc)?;
                let mut flags = 2_u64;
                for (name, bit) in [
                    ("CF", 0),
                    ("PF", 2),
                    ("AF", 4),
                    ("ZF", 6),
                    ("SF", 7),
                    ("OF", 11),
                ] {
                    if self.flag(name) {
                        flags |= 1 << bit;
                    }
                }
                self.set_reg("r11", flags)?;
                self.syscall()?;
            }
            "int" => {
                if self.architecture != Architecture::X86 || Self::immediate(at(0)?)? != 0x80 {
                    return Err(SimError(
                        "Only x86 int 0x80 supports the teaching ABI".into(),
                    ));
                }
                self.syscall()?;
            }
            "mov" | "movabs" | "movw" | "movz" | "movs" => {
                let mut value = self.shifted(
                    at(1)?,
                    operands.get(2).map(String::as_str),
                    bits,
                    old_pc,
                    next_pc,
                )?;
                if matches!(mnemonic.as_str(), "movw" | "movz") {
                    value &= 0xffff_u64
                        .checked_shl(
                            operands
                                .get(2)
                                .and_then(|s| s.split_once(' '))
                                .and_then(|(_, n)| Self::immediate(n).ok())
                                .unwrap_or(0) as u32,
                        )
                        .unwrap_or(0);
                }
                self.destination(at(0)?, bits, value, old_pc, next_pc)?;
                if mnemonic == "movs" {
                    self.arm32_shifter_carry(instruction, old_pc, next_pc)?;
                    self.logical_flags(value, bits);
                }
            }
            "movt" | "movk" => {
                let shift = if mnemonic == "movt" {
                    16
                } else {
                    operands
                        .get(2)
                        .and_then(|s| s.strip_prefix("lsl "))
                        .map(Self::immediate)
                        .transpose()?
                        .unwrap_or(0) as u32
                };
                if shift >= u32::from(bits) || shift % 16 != 0 {
                    return Err(SimError("Invalid wide-move halfword".into()));
                }
                let previous = self.reg(at(0)?)?;
                let value = Self::immediate(at(1)?)?;
                self.set_reg(
                    at(0)?,
                    previous & !(0xffff_u64 << shift) | (value & 0xffff) << shift,
                )?;
            }
            "movn" | "mvn" | "mvns" => {
                let value = !self.shifted(
                    at(1)?,
                    operands.get(2).map(String::as_str),
                    bits,
                    old_pc,
                    next_pc,
                )?;
                self.destination(at(0)?, bits, value, old_pc, next_pc)?;
                if mnemonic == "mvns" {
                    self.arm32_shifter_carry(instruction, old_pc, next_pc)?;
                    self.logical_flags(value, bits);
                }
            }
            "movzx" | "movsx" | "movsxd" | "uxtb" | "uxth" | "sxtb" | "sxth" | "sxtw" => {
                let source_bits = match mnemonic.as_str() {
                    "uxtb" | "sxtb" => 8,
                    "uxth" | "sxth" => 16,
                    "sxtw" | "movsxd" => 32,
                    _ => self.operand_bits(at(1)?, bits),
                };
                let value =
                    self.value(at(1)?, source_bits, old_pc, next_pc)? & Self::mask(source_bits);
                let sign = matches!(
                    mnemonic.as_str(),
                    "movsx" | "movsxd" | "sxtb" | "sxth" | "sxtw"
                );
                self.set_reg(
                    at(0)?,
                    if sign {
                        Self::signed(value, source_bits) as u64
                    } else {
                        value
                    },
                )?;
            }
            "lea" => {
                let address = self.x86_address(at(1)?, old_pc, next_pc)?;
                self.set_reg(at(0)?, address)?;
            }
            "adr" | "adrp" => {
                self.set_reg(at(0)?, Self::immediate(at(1)?)?)?;
            }
            "add" | "adds" | "sub" | "subs" | "and" | "ands" | "or" | "orr" | "xor" | "eor"
            | "bic" | "bics" | "orn" | "eon" | "cmp" | "cmn" | "test" | "tst" | "teq" => {
                let compare = matches!(mnemonic.as_str(), "cmp" | "cmn" | "test" | "tst" | "teq");
                let (left_index, right_index, modifier) = if arm && !compare {
                    (1, 2, 3)
                } else {
                    (0, 1, 2)
                };
                let left = self.value(at(left_index)?, bits, old_pc, next_pc)? & Self::mask(bits);
                let right = self.shifted(
                    at(right_index)?,
                    operands.get(modifier).map(String::as_str),
                    bits,
                    old_pc,
                    next_pc,
                )?;
                let result = match mnemonic.as_str() {
                    "add" | "adds" | "cmn" => left.wrapping_add(right),
                    "sub" | "subs" | "cmp" => left.wrapping_sub(right),
                    "and" | "ands" | "test" | "tst" => left & right,
                    "or" | "orr" => left | right,
                    "xor" | "eor" | "teq" => left ^ right,
                    "bic" | "bics" => left & !right,
                    "orn" => left | !right,
                    "eon" => left ^ !right,
                    _ => unreachable!(),
                } & Self::mask(bits);
                if !arm || compare || matches!(mnemonic.as_str(), "adds" | "subs" | "ands" | "bics")
                {
                    if matches!(
                        mnemonic.as_str(),
                        "add" | "adds" | "cmn" | "sub" | "subs" | "cmp"
                    ) {
                        self.arithmetic_flags(
                            left,
                            right,
                            result,
                            bits,
                            matches!(mnemonic.as_str(), "sub" | "subs" | "cmp"),
                        );
                    } else {
                        self.arm32_shifter_carry(instruction, old_pc, next_pc)?;
                        self.logical_flags(result, bits);
                    }
                }
                if !compare {
                    self.destination(at(0)?, bits, result, old_pc, next_pc)?;
                }
            }
            "neg" | "negs" | "not" | "inc" | "dec" => {
                let source = if arm { at(1)? } else { at(0)? };
                let value = self.value(source, bits, old_pc, next_pc)? & Self::mask(bits);
                let result = match mnemonic.as_str() {
                    "neg" | "negs" => value.wrapping_neg(),
                    "not" => !value,
                    "inc" => value.wrapping_add(1),
                    "dec" => value.wrapping_sub(1),
                    _ => unreachable!(),
                } & Self::mask(bits);
                if mnemonic != "not" && (!arm || mnemonic == "negs") {
                    let carry = self.flag("CF");
                    if matches!(mnemonic.as_str(), "neg" | "negs") {
                        self.arithmetic_flags(0, value, result, bits, true);
                    } else {
                        self.arithmetic_flags(value, 1, result, bits, mnemonic == "dec");
                        self.set_flag("CF", carry);
                    }
                }
                self.destination(at(0)?, bits, result, old_pc, next_pc)?;
            }
            "lsl" | "lsls" | "lsr" | "lsrs" | "asr" | "asrs" | "ror" | "rors" | "shl" | "sal"
            | "shr" | "sar" | "rol" => {
                let (source, amount) = if arm {
                    (at(1)?, at(2)?)
                } else {
                    (at(0)?, at(1)?)
                };
                let value = self.value(source, bits, old_pc, next_pc)? & Self::mask(bits);
                let mut count = self.value(amount, bits, old_pc, next_pc)? as u32;
                if arm {
                    if self.architecture == Architecture::Arm64 {
                        count %= u32::from(bits);
                    } else if self.alias(amount).is_ok() {
                        count &= 255;
                    }
                } else {
                    count &= if bits == 64 { 63 } else { 31 };
                }
                let operation = match mnemonic.as_str() {
                    "lsl" | "lsls" | "shl" | "sal" => "lsl",
                    "lsr" | "lsrs" | "shr" => "lsr",
                    "asr" | "asrs" | "sar" => "asr",
                    "rol" => "rol",
                    _ => "ror",
                };
                let effective = if matches!(operation, "rol" | "ror") {
                    count % u32::from(bits)
                } else {
                    count
                };
                let result = if effective == 0 {
                    value
                } else {
                    (match operation {
                        "lsl" => {
                            if effective >= u32::from(bits) {
                                0
                            } else {
                                value << effective
                            }
                        }
                        "lsr" => {
                            if effective >= u32::from(bits) {
                                0
                            } else {
                                value >> effective
                            }
                        }
                        "asr" => {
                            if effective >= u32::from(bits) {
                                if value >> (bits - 1) != 0 {
                                    Self::mask(bits)
                                } else {
                                    0
                                }
                            } else {
                                (Self::signed(value, bits) >> effective) as u64
                            }
                        }
                        "ror" => value >> effective | value << (u32::from(bits) - effective),
                        "rol" => value << effective | value >> (u32::from(bits) - effective),
                        _ => unreachable!(),
                    }) & Self::mask(bits)
                };
                if count > 0 && (!arm || mnemonic.ends_with('s')) {
                    let carry = if effective == 0 && !arm && operation == "rol" {
                        value & 1 != 0
                    } else if effective == 0 && !arm && operation == "ror" {
                        value >> (bits - 1) != 0
                    } else if effective == 0 {
                        self.flag(if arm { "C" } else { "CF" })
                    } else if effective > u32::from(bits) {
                        false
                    } else if matches!(operation, "lsl" | "rol") {
                        value >> (u32::from(bits) - effective) & 1 != 0
                    } else {
                        value >> (effective - 1) & 1 != 0
                    };
                    if !matches!(operation, "rol" | "ror") {
                        self.logical_flags(result, bits);
                    }
                    self.set_flag(if arm { "C" } else { "CF" }, carry);
                    if !arm && count == 1 {
                        self.set_flag(
                            "OF",
                            match operation {
                                "lsl" | "rol" => (result >> (bits - 1) != 0) ^ carry,
                                "lsr" => value >> (bits - 1) != 0,
                                "asr" => false,
                                "ror" => ((result >> (bits - 1)) ^ (result >> (bits - 2))) & 1 != 0,
                                _ => false,
                            },
                        );
                    }
                }
                if arm || count > 0 {
                    self.destination(at(0)?, bits, result, old_pc, next_pc)?;
                }
            }
            "mul" | "imul" | "madd" | "msub" => {
                if !arm && mnemonic == "mul" || !arm && mnemonic == "imul" && operands.len() == 1 {
                    return Err(SimError("One-operand x86 multiply with implicit high result is unsupported; use two/three-operand imul".into()));
                }
                let (left, right) = if arm || operands.len() == 3 {
                    (at(1)?, at(2)?)
                } else {
                    (at(0)?, at(1)?)
                };
                let a = self.value(left, bits, old_pc, next_pc)?;
                let b = self.value(right, bits, old_pc, next_pc)?;
                let mut result = a.wrapping_mul(b);
                if matches!(mnemonic.as_str(), "madd" | "msub") {
                    let add = self.value(at(3)?, bits, old_pc, next_pc)?;
                    result = if mnemonic == "madd" {
                        add.wrapping_add(result)
                    } else {
                        add.wrapping_sub(result)
                    };
                }
                if !arm {
                    let product =
                        i128::from(Self::signed(a, bits)) * i128::from(Self::signed(b, bits));
                    let truncated = i128::from(Self::signed(result & Self::mask(bits), bits));
                    self.set_flag("CF", product != truncated);
                    self.set_flag("OF", product != truncated);
                }
                self.set_reg(at(0)?, result)?;
            }
            "ldr" | "ldrb" | "ldrh" | "ldrsb" | "ldrsh" | "ldrsw" | "ldur" | "ldurb" | "ldurh"
            | "ldursb" | "ldursh" | "ldursw" | "str" | "strb" | "strh" | "stur" | "sturb"
            | "sturh" => {
                let load = mnemonic.starts_with("ld");
                let length = if mnemonic.ends_with('b') {
                    1
                } else if mnemonic.ends_with('h') {
                    2
                } else if mnemonic.ends_with("sw") {
                    4
                } else {
                    usize::from(bits / 8)
                };
                let (address, update) = if at(1)?.contains('[') {
                    self.arm_address(at(1)?, operands.get(2).map(String::as_str), old_pc, next_pc)?
                } else {
                    (Self::immediate(at(1)?)?, None)
                };
                if let Some((base, _)) = &update
                    && self.alias(base)?.0 == self.alias(at(0)?)?.0
                {
                    return Err(SimError("Load/store writeback overlaps its value register; constrained/unpredictable encoding is unsupported".into()));
                }
                if load {
                    let value = self.read(address, length)?;
                    let sign = matches!(
                        mnemonic.as_str(),
                        "ldrsb" | "ldrsh" | "ldrsw" | "ldursb" | "ldursh" | "ldursw"
                    );
                    self.set_reg(
                        at(0)?,
                        if sign {
                            Self::signed(value, (length * 8) as u8) as u64
                        } else {
                            value
                        },
                    )?;
                } else {
                    let value = self.operand_reg(at(0)?, old_pc, next_pc)?;
                    self.write(address, length, value)?;
                }
                if let Some((base, value)) = update {
                    self.set_reg(&base, value)?;
                }
            }
            "ldp" | "stp" => {
                if self.architecture != Architecture::Arm64 {
                    return Err(SimError("Pair access requires ARM64".into()));
                }
                let length = usize::from(bits / 8);
                let (address, update) =
                    self.arm_address(at(2)?, operands.get(3).map(String::as_str), old_pc, next_pc)?;
                if let Some((base, _)) = &update
                    && (self.alias(base)?.0 == self.alias(at(0)?)?.0
                        || self.alias(base)?.0 == self.alias(at(1)?)?.0)
                {
                    return Err(SimError(
                        "Pair access writeback overlaps a value register".into(),
                    ));
                }
                if mnemonic == "ldp" {
                    if self.alias(at(0)?)?.0 == self.alias(at(1)?)?.0 {
                        return Err(SimError("Pair load destinations overlap".into()));
                    }
                    let a = self.read(address, length)?;
                    let b = self.read(
                        address
                            .checked_add(length as u64)
                            .ok_or_else(|| SimError("Address overflow".into()))?,
                        length,
                    )?;
                    self.set_reg(at(0)?, a)?;
                    self.set_reg(at(1)?, b)?;
                } else {
                    let a = self.reg(at(0)?)?;
                    let b = self.reg(at(1)?)?;
                    self.write(address, length, a)?;
                    self.write(
                        address
                            .checked_add(length as u64)
                            .ok_or_else(|| SimError("Address overflow".into()))?,
                        length,
                        b,
                    )?;
                }
                if let Some((base, value)) = update {
                    self.set_reg(&base, value)?;
                }
            }
            "push" | "pop" => {
                if self.architecture == Architecture::Arm32 {
                    let list = register_list(at(0)?)?;
                    if list
                        .iter()
                        .any(|reg| self.alias(reg).is_ok_and(|alias| alias.0 == "r13"))
                    {
                        return Err(SimError(
                            "ARM push/pop with SP in the register list is unsupported".into(),
                        ));
                    }
                    if mnemonic == "push" {
                        let mut values = Vec::new();
                        for name in &list {
                            values.push(self.operand_reg(name, old_pc, next_pc)?);
                        }
                        for value in values.into_iter().rev() {
                            self.push(value, 4)?;
                        }
                    } else {
                        for name in list {
                            let value = self.pop(4)?;
                            self.set_reg(&name, value)?;
                        }
                    }
                } else if arm {
                    return Err(SimError("ARM64 stack uses stp/ldp".into()));
                } else {
                    let width = if bits == 16 {
                        2
                    } else {
                        usize::from(self.architecture.bits() / 8)
                    };
                    if mnemonic == "push" {
                        let value = self.value(at(0)?, (width * 8) as u8, old_pc, next_pc)?;
                        self.push(value, width)?;
                    } else {
                        let value = self.pop(width)?;
                        self.destination(at(0)?, (width * 8) as u8, value, old_pc, next_pc)?;
                    }
                }
            }
            "call" | "bl" | "blr" => {
                let target = self.value(at(0)?, self.architecture.bits(), old_pc, next_pc)?;
                if arm {
                    self.set_reg(
                        if self.architecture == Architecture::Arm64 {
                            "x30"
                        } else {
                            "r14"
                        },
                        next_pc,
                    )?;
                } else {
                    self.push(next_pc, usize::from(self.architecture.bits() / 8))?;
                }
                self.branch(target)?;
            }
            "ret" => {
                let target = if arm {
                    self.reg(operands.first().map_or("x30", String::as_str))?
                } else {
                    self.pop(usize::from(self.architecture.bits() / 8))?
                };
                if !arm && !operands.is_empty() {
                    let count = Self::immediate(at(0)?)?;
                    let sp = self.stack_register();
                    self.set_reg(sp, self.address(self.reg(sp)?.wrapping_add(count)))?;
                }
                self.branch(target)?;
            }
            "jmp" | "b" | "br" | "bx" => {
                let target = self.value(at(0)?, self.architecture.bits(), old_pc, next_pc)?;
                self.branch(target)?;
            }
            "cbz" | "cbnz" => {
                let value = self.reg(at(0)?)?;
                if (value == 0) == (mnemonic == "cbz") {
                    self.branch(Self::immediate(at(1)?)?)?;
                }
            }
            "tbz" | "tbnz" => {
                let bit = Self::immediate(at(1)?)?;
                if bit >= u64::from(bits) {
                    return Err(SimError("Test branch bit exceeds register width".into()));
                }
                if (self.reg(at(0)?)? >> bit & 1 == 0) == (mnemonic == "tbz") {
                    self.branch(Self::immediate(at(2)?)?)?;
                }
            }
            "loop" | "loope" | "loopne" => {
                let counter = if self.architecture == Architecture::X64 {
                    "rcx"
                } else {
                    "ecx"
                };
                let value =
                    self.reg(counter)?.wrapping_sub(1) & Self::mask(self.architecture.bits());
                self.set_reg(counter, value)?;
                if value != 0 && (mnemonic == "loop" || self.flag("ZF") == (mnemonic == "loope")) {
                    self.branch(Self::immediate(at(0)?)?)?;
                }
            }
            "jecxz" | "jrcxz" => {
                if self.reg(if mnemonic == "jrcxz" { "rcx" } else { "ecx" })? == 0 {
                    self.branch(Self::immediate(at(0)?)?)?;
                }
            }
            _ if mnemonic.starts_with("b.") => {
                if self.arm_condition(&mnemonic[2..])? {
                    self.branch(Self::immediate(at(0)?)?)?;
                }
            }
            _ if !arm && mnemonic.starts_with('j') => {
                if self.x86_condition(&mnemonic[1..])? {
                    self.branch(Self::immediate(at(0)?)?)?;
                }
            }
            _ if !arm && mnemonic.starts_with("set") => {
                let value = u64::from(self.x86_condition(&mnemonic[3..])?);
                self.destination(at(0)?, 8, value, old_pc, next_pc)?;
            }
            _ if !arm && mnemonic.starts_with("cmov") => {
                let value = self.value(at(1)?, bits, old_pc, next_pc)?;
                if self.x86_condition(&mnemonic[4..])? {
                    self.destination(at(0)?, bits, value, old_pc, next_pc)?;
                }
            }
            _ => {
                return Err(SimError(format!(
                    "Unsupported {} instruction at 0x{old_pc:x}: {}. See docs/isa-support.md for the scalar subset.",
                    self.architecture.name(),
                    instruction.assembly
                )));
            }
        }
        Ok(())
    }
}

fn split_operands(text: &str) -> Vec<String> {
    if text.is_empty() {
        return vec![];
    }
    let mut operands = vec![];
    let mut level = 0_i32;
    let mut start = 0;
    for (offset, character) in text.char_indices() {
        match character {
            '[' | '{' | '(' => level += 1,
            ']' | '}' | ')' => level -= 1,
            ',' if level == 0 => {
                operands.push(text[start..offset].trim().to_string());
                start = offset + 1;
            }
            _ => {}
        }
    }
    operands.push(text[start..].trim().to_string());
    operands
}
fn register_list(text: &str) -> Result<Vec<String>, SimError> {
    if !text.starts_with('{') || !text.ends_with('}') {
        return Err(SimError("Expected ARM register list".into()));
    }
    let mut registers = vec![];
    for register in split_operands(&text[1..text.len() - 1]) {
        if let Some((first, last)) = register.split_once('-') {
            let first = first
                .trim()
                .trim_start_matches('r')
                .parse::<u8>()
                .map_err(|_| SimError("Invalid register range".into()))?;
            let last = last
                .trim()
                .trim_start_matches('r')
                .parse::<u8>()
                .map_err(|_| SimError("Invalid register range".into()))?;
            if first > last || last > 15 {
                return Err(SimError("Invalid register range".into()));
            }
            registers.extend((first..=last).map(|n| format!("r{n}")));
        } else {
            registers.push(register);
        }
    }
    Ok(registers)
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
            fresh.set_reg(name, *value)?;
        }
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
        let bytes = self.fetch()?;
        let mut instruction = decode(self.architecture, self.pc, &bytes)?;
        if let Some(original) = self
            .program
            .as_ref()
            .and_then(|program| program.instruction(self.pc))
            .filter(|original| original.bytes == instruction.bytes)
        {
            instruction.source_line = original.source_line;
        }
        let old_registers = self.registers.clone();
        let old_flags = self.flags.clone();
        let old_pc = self.pc;
        let output_len = self.output.len();
        self.writes.clear();
        self.reads.clear();
        if let Err(error) = self.execute(&instruction) {
            for change in self.writes.clone().into_iter().rev() {
                self.write_bytes(change.address, &change.before, false)?;
            }
            self.registers = old_registers;
            self.flags = old_flags;
            self.pc = old_pc;
            self.output.truncate(output_len);
            self.halted = false;
            self.writes.clear();
            self.reads.clear();
            return Err(error);
        }
        let mut changes: Vec<_> = old_registers
            .iter()
            .filter_map(|(name, before)| {
                let after = self.registers[name];
                (*before != after).then(|| RegisterChange {
                    name: name.clone(),
                    before: *before,
                    after,
                })
            })
            .collect();
        changes.extend(old_flags.iter().filter_map(|(name, before)| {
            let after = self.flags[name];
            (*before != after).then(|| RegisterChange {
                name: name.clone(),
                before: u64::from(*before),
                after: u64::from(after),
            })
        }));
        let record = StepRecord {
            pc_before: old_pc,
            pc_after: self.pc,
            instruction,
            registers: changes,
            memory: std::mem::take(&mut self.writes),
            memory_reads: std::mem::take(&mut self.reads),
            output_added: self.output[output_len..].to_vec(),
            halted: self.halted,
        };
        self.steps += 1;
        let size = 256
            + record.instruction.assembly.len()
            + record.registers.len() * 48
            + record
                .memory
                .iter()
                .map(|c| c.before.len() + c.after.len() + 32)
                .sum::<usize>()
            + record.output_added.len()
            + record.memory_reads.len() * 24;
        self.history.push_back(Undo {
            record: record.clone(),
            flags: old_flags,
            size,
        });
        self.history_bytes += size;
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
        for change in undo.record.memory.iter().rev() {
            self.write_bytes(change.address, &change.before, false)?;
        }
        for change in undo.record.registers {
            if let std::collections::btree_map::Entry::Occupied(mut e) =
                self.registers.entry(change.name)
            {
                e.insert(change.before);
            }
        }
        self.flags = undo.flags;
        self.pc = undo.record.pc_before;
        self.output
            .truncate(self.output.len() - undo.record.output_added.len());
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
        let mut registers: Vec<_> = self
            .registers
            .iter()
            .map(|(name, value)| RegisterValue {
                name: name.clone(),
                value: *value,
                bits: self.architecture.bits(),
            })
            .collect();
        registers.push(RegisterValue {
            name: self.pc_register().into(),
            value: self.pc,
            bits: self.architecture.bits(),
        });
        Snapshot {
            architecture: self.architecture,
            pc: self.pc,
            registers,
            flags: self.flags.clone(),
            memory: self.memory.clone(),
            output: self.output.clone(),
            halted: self.halted,
            steps: self.steps,
            trace: self
                .history
                .iter()
                .map(|entry| entry.record.clone())
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
        if snapshot.flags.keys().ne(self.flags.keys()) {
            return Err(SimError(
                "Snapshot contains unknown or missing flags".into(),
            ));
        }
        let mut restored = Self::new(self.architecture);
        restored.load(program)?;
        if snapshot.registers.len() != restored.registers.len() + 1 {
            return Err(SimError(
                "Snapshot contains missing or duplicate registers".into(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for register in &snapshot.registers {
            let (canonical, bits, _, _) = restored.alias(&register.name)?;
            if register.value > Self::mask(bits)
                || register.bits != bits
                || !seen.insert(canonical.clone())
            {
                return Err(SimError(
                    "Snapshot register width/value or alias is invalid".into(),
                ));
            }
            if canonical == "zr" {
                return Err(SimError("Snapshot cannot contain the zero register".into()));
            }
            restored.set_reg(&register.name, register.value)?;
        }
        if restored.pc != snapshot.pc {
            return Err(SimError("Snapshot PC and PC register disagree".into()));
        }
        restored.memory = snapshot.memory.clone();
        restored.flags = snapshot.flags.clone();
        restored.output = snapshot.output.clone();
        restored.halted = snapshot.halted;
        restored.steps = snapshot.steps;
        *self = restored;
        Ok(())
    }
    fn write_register(&mut self, name: &str, value: u64) -> Result<(), SimError> {
        self.set_reg(name, value)?;
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
}
