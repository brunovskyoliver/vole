//! A32 (ARMv7-A, ARM state) integer instruction semantics (Arm ARM DDI 0406, A8).
use super::arm64::{imm_at, leading_zeros, mem_at, operand_error, reg_at};
use super::operand::{Amount, Mem, Operand, PC, Reg, Shift};
use super::{C, Decoded, N, ScalarMachine, V, Z};
use vole_core::SimError;

/// A32 `Shift_C` for any amount (register-specified amounts may exceed 32).
fn shift_c(value: u32, shift: Shift, amount: u32, carry: bool) -> (u32, bool) {
    let bit = |n: u32| value >> n & 1 != 0;
    match shift {
        Shift::Rrx => (u32::from(carry) << 31 | value >> 1, bit(0)),
        _ if amount == 0 => (value, carry),
        Shift::Lsl => match amount {
            1..=31 => (value << amount, bit(32 - amount)),
            32 => (0, bit(0)),
            _ => (0, false),
        },
        Shift::Lsr => match amount {
            1..=31 => (value >> amount, bit(amount - 1)),
            32 => (0, bit(31)),
            _ => (0, false),
        },
        Shift::Asr => {
            if amount < 32 {
                (((value as i32) >> amount) as u32, bit(amount - 1))
            } else {
                (((value as i32) >> 31) as u32, bit(31))
            }
        }
        Shift::Ror => {
            let result = value.rotate_right(amount % 32);
            (result, result >> 31 != 0)
        }
        _ => (value, carry),
    }
}

const FLAG_SETTING: [&str; 25] = [
    "movs", "mvns", "ands", "eors", "orrs", "bics", "orns", "adds", "adcs", "subs", "sbcs", "rsbs",
    "rscs", "lsls", "lsrs", "asrs", "rors", "rrxs", "muls", "mlas", "umulls", "smulls", "umlals",
    "smlals", "teqs",
];

impl ScalarMachine {
    /// Shifter operand value and carry-out starting at `index`.
    fn a32_operand2(
        &self,
        decoded: &Decoded,
        operands: &[Operand],
        index: usize,
    ) -> Result<(u32, bool), SimError> {
        let carry = self.flag(C);
        match operands.get(index) {
            Some(Operand::Imm(value)) => {
                let word = u32::from_le_bytes(
                    decoded.instruction.bytes[..4].try_into().expect("A32 word"),
                );
                let rotated = word & (1 << 25) != 0 && (word >> 8) & 15 != 0;
                let value = *value as u32;
                Ok((value, if rotated { value >> 31 != 0 } else { carry }))
            }
            Some(Operand::Reg(register)) => {
                let value = self.get(*register) as u32;
                match operands.get(index + 1) {
                    Some(Operand::Modifier(modifier)) => {
                        let amount = match modifier.amount {
                            Amount::Imm(amount) => amount,
                            Amount::Reg(register) => self.get(register) as u32 & 255,
                        };
                        Ok(shift_c(value, modifier.shift, amount, carry))
                    }
                    None => Ok((value, carry)),
                    _ => Err(operand_error(index + 1)),
                }
            }
            _ => Err(operand_error(index)),
        }
    }
    fn a32_offset(
        &self,
        register: Reg,
        negative: bool,
        modifier: Option<&Operand>,
    ) -> Result<u64, SimError> {
        let mut value = self.get(register) as u32;
        match modifier {
            Some(Operand::Modifier(modifier)) => {
                let Amount::Imm(amount) = modifier.amount else {
                    return Err(SimError(
                        "Register-shifted load/store offsets are not A32".into(),
                    ));
                };
                value = shift_c(value, modifier.shift, amount, self.flag(C)).0;
            }
            None => {}
            _ => return Err(operand_error(2)),
        }
        Ok(if negative {
            u64::from(value.wrapping_neg())
        } else {
            u64::from(value)
        })
    }
    /// Address and optional writeback for A32 single/dual loads and stores.
    fn a32_address(
        &self,
        memory: &Mem,
        post: &[Operand],
    ) -> Result<(u64, Option<(Reg, u64)>), SimError> {
        let base = memory.base.ok_or_else(|| operand_error(1))?;
        let base_value = self.get(base);
        let offset = match memory.index {
            Some(index) => self.a32_offset(
                index,
                memory.subtract_index,
                memory.modifier.map(Operand::Modifier).as_ref(),
            )?,
            None => memory.displacement,
        };
        let wrap = |value: u64| value & 0xffff_ffff;
        if let Some(first) = post.first() {
            let step = match first {
                Operand::Imm(step) => *step,
                Operand::Reg(register) => self.a32_offset(*register, false, post.get(1))?,
                Operand::NegReg(register) => self.a32_offset(*register, true, post.get(1))?,
                _ => return Err(operand_error(2)),
            };
            return Ok((
                base_value,
                Some((base, wrap(base_value.wrapping_add(step)))),
            ));
        }
        let address = wrap(base_value.wrapping_add(offset));
        Ok((address, memory.writeback.then_some((base, address))))
    }

    pub(super) fn execute_arm32(
        &mut self,
        decoded: &Decoded,
        ops: &[Operand],
    ) -> Result<bool, SimError> {
        let full = decoded.mnemonic.as_str();
        let set_flags = FLAG_SETTING.contains(&full);
        let mnemonic = if set_flags {
            &full[..full.len() - 1]
        } else {
            full
        };
        if set_flags && matches!(ops.first(), Some(Operand::Reg(register)) if register.index == PC)
        {
            return Err(SimError(
                "A32 flag-setting writes to PC require exception-return semantics and are unsupported"
                    .into(),
            ));
        }
        match mnemonic {
            "mov" | "mvn" => {
                let destination = reg_at(ops, 0)?;
                let (value, carry) = self.a32_operand2(decoded, ops, 1)?;
                let value = if mnemonic == "mvn" { !value } else { value };
                if set_flags {
                    self.set_nz(u64::from(value), 32);
                    self.set_flag(C, carry);
                }
                self.set(destination, u64::from(value))?;
            }
            "and" | "eor" | "orr" | "bic" | "tst" | "teq" => {
                let test = matches!(mnemonic, "tst" | "teq");
                let first = usize::from(!test);
                let a = self.get(reg_at(ops, first)?) as u32;
                let (b, carry) = self.a32_operand2(decoded, ops, first + 1)?;
                let result = match mnemonic {
                    "and" | "tst" => a & b,
                    "eor" | "teq" => a ^ b,
                    "orr" => a | b,
                    _ => a & !b,
                };
                if set_flags || test {
                    self.set_nz(u64::from(result), 32);
                    self.set_flag(C, carry);
                }
                if !test {
                    self.set(reg_at(ops, 0)?, u64::from(result))?;
                }
            }
            "add" | "adc" | "sub" | "sbc" | "rsb" | "rsc" | "cmp" | "cmn" => {
                let compare = matches!(mnemonic, "cmp" | "cmn");
                let first = usize::from(!compare);
                let a = self.get(reg_at(ops, first)?);
                let b = u64::from(self.a32_operand2(decoded, ops, first + 1)?.0);
                let carry = self.flag(C);
                let (x, y, carry_in) = match mnemonic {
                    "add" | "cmn" => (a, b, false),
                    "adc" => (a, b, carry),
                    "sub" | "cmp" => (a, !b, true),
                    "sbc" => (a, !b, carry),
                    "rsb" => (b, !a, true),
                    _ => (b, !a, carry),
                };
                let result = self.add_with_carry(x, y, carry_in, 32, set_flags || compare);
                if !compare {
                    self.set(reg_at(ops, 0)?, result)?;
                }
            }
            "lsl" | "lsr" | "asr" | "ror" | "rrx" => {
                let destination = reg_at(ops, 0)?;
                let value = self.get(reg_at(ops, 1)?) as u32;
                let (shift, amount) = match mnemonic {
                    "rrx" => (Shift::Rrx, 0),
                    _ => {
                        let shift = match mnemonic {
                            "lsl" => Shift::Lsl,
                            "lsr" => Shift::Lsr,
                            "asr" => Shift::Asr,
                            _ => Shift::Ror,
                        };
                        let amount = match ops.get(2) {
                            Some(Operand::Imm(amount)) => *amount as u32,
                            Some(Operand::Reg(register)) => self.get(*register) as u32 & 255,
                            _ => return Err(operand_error(2)),
                        };
                        (shift, amount)
                    }
                };
                let (result, carry) = shift_c(value, shift, amount, self.flag(C));
                if set_flags {
                    self.set_nz(u64::from(result), 32);
                    self.set_flag(C, carry);
                }
                self.set(destination, u64::from(result))?;
            }
            "mul" | "mla" | "mls" => {
                let destination = reg_at(ops, 0)?;
                let product = (self.get(reg_at(ops, 1)?) as u32)
                    .wrapping_mul(self.get(reg_at(ops, 2)?) as u32);
                let result = match mnemonic {
                    "mul" => product,
                    "mla" => (self.get(reg_at(ops, 3)?) as u32).wrapping_add(product),
                    _ => (self.get(reg_at(ops, 3)?) as u32).wrapping_sub(product),
                };
                if set_flags {
                    self.set_nz(u64::from(result), 32);
                }
                self.set(destination, u64::from(result))?;
            }
            "umull" | "smull" | "umlal" | "smlal" | "umaal" => {
                let (low, high) = (reg_at(ops, 0)?, reg_at(ops, 1)?);
                if low.index == high.index {
                    return Err(SimError(
                        "Long multiply with RdLo equal to RdHi is unpredictable".into(),
                    ));
                }
                let (a, b) = (self.get(reg_at(ops, 2)?), self.get(reg_at(ops, 3)?));
                let product = if mnemonic.starts_with('s') {
                    (Self::signed(a, 32) * Self::signed(b, 32)) as u64
                } else {
                    a * b
                };
                let accumulator = self.get(high) << 32 | self.get(low);
                let result = match mnemonic {
                    "umull" | "smull" => product,
                    "umlal" | "smlal" => product.wrapping_add(accumulator),
                    _ => product + self.get(low) + self.get(high),
                };
                if set_flags {
                    self.set_nz(result, 64);
                }
                self.set(low, result & 0xffff_ffff)?;
                self.set(high, result >> 32)?;
            }
            "smmul" | "smmulr" | "smmla" | "smmlar" | "smmls" | "smmlsr" => {
                let destination = reg_at(ops, 0)?;
                let product = Self::signed(self.get(reg_at(ops, 1)?), 32)
                    * Self::signed(self.get(reg_at(ops, 2)?), 32);
                let accumulator = if mnemonic.starts_with("smmul") {
                    0
                } else {
                    Self::signed(self.get(reg_at(ops, 3)?), 32) << 32
                };
                let mut result = if mnemonic.starts_with("smmls") {
                    accumulator.wrapping_sub(product)
                } else {
                    accumulator.wrapping_add(product)
                };
                if mnemonic.ends_with('r') {
                    result = result.wrapping_add(0x8000_0000);
                }
                self.set(destination, (result >> 32) as u64)?;
            }
            "smulbb" | "smulbt" | "smultb" | "smultt" | "smlabb" | "smlabt" | "smlatb"
            | "smlatt" => {
                let destination = reg_at(ops, 0)?;
                let half = |value: u64, top: bool| {
                    i64::from((if top { value >> 16 } else { value }) as u16 as i16)
                };
                let selectors = &mnemonic.as_bytes()[4..6];
                let a = half(self.get(reg_at(ops, 1)?), selectors[0] == b't');
                let b = half(self.get(reg_at(ops, 2)?), selectors[1] == b't');
                let mut result = a * b;
                if mnemonic.starts_with("smla") {
                    // The Q (saturation) flag is not modelled.
                    result += Self::signed(self.get(reg_at(ops, 3)?), 32);
                }
                self.set(destination, result as u64)?;
            }
            "sdiv" | "udiv" => {
                // ARMv7 integer divide returns 0 for division by zero (no trap).
                let destination = reg_at(ops, 0)?;
                let (a, b) = (self.get(reg_at(ops, 1)?), self.get(reg_at(ops, 2)?));
                let result = if b == 0 {
                    0
                } else if mnemonic == "sdiv" {
                    Self::signed(a, 32).wrapping_div(Self::signed(b, 32)) as u64
                } else {
                    a / b
                };
                self.set(destination, result)?;
            }
            "clz" | "rev" | "rev16" | "revsh" | "rbit" => {
                let destination = reg_at(ops, 0)?;
                let value = self.get(reg_at(ops, 1)?) as u32;
                let result = match mnemonic {
                    "clz" => leading_zeros(u64::from(value), 32) as u32,
                    "rev" => value.swap_bytes(),
                    "rev16" => (value & 0xff00_ff00) >> 8 | (value & 0x00ff_00ff) << 8,
                    "revsh" => i32::from((value as u16).swap_bytes() as i16) as u32,
                    _ => value.reverse_bits(),
                };
                self.set(destination, u64::from(result))?;
            }
            "uxtb" | "uxth" | "sxtb" | "sxth" | "uxtab" | "uxtah" | "sxtab" | "sxtah" => {
                let destination = reg_at(ops, 0)?;
                let accumulate = mnemonic.len() == 5;
                let source = usize::from(accumulate) + 1;
                let mut value = self.get(reg_at(ops, source)?) as u32;
                if let Some(Operand::Modifier(modifier)) = ops.get(source + 1) {
                    let (Shift::Ror, Amount::Imm(amount)) = (modifier.shift, modifier.amount)
                    else {
                        return Err(operand_error(source + 1));
                    };
                    value = value.rotate_right(amount);
                }
                let extended = match (mnemonic.as_bytes()[0], *mnemonic.as_bytes().last().unwrap())
                {
                    (b'u', b'b') => value & 0xff,
                    (b'u', _) => value & 0xffff,
                    (_, b'b') => value as u8 as i8 as i32 as u32,
                    _ => value as u16 as i16 as i32 as u32,
                };
                let result = if accumulate {
                    (self.get(reg_at(ops, 1)?) as u32).wrapping_add(extended)
                } else {
                    extended
                };
                self.set(destination, u64::from(result))?;
            }
            "ubfx" | "sbfx" | "bfi" | "bfc" => {
                let destination = reg_at(ops, 0)?;
                let (source, first) = if mnemonic == "bfc" {
                    (0, 1)
                } else {
                    (self.get(reg_at(ops, 1)?), 2)
                };
                let (lsb, width) = (imm_at(ops, first)?, imm_at(ops, first + 1)?);
                if width == 0 || lsb + width > 32 {
                    return Err(SimError("Bitfield exceeds register width".into()));
                }
                let field = Self::mask(width as u8);
                let result = match mnemonic {
                    "ubfx" => source >> lsb & field,
                    "sbfx" => Self::signed(source >> lsb & field, width as u8) as u64 & 0xffff_ffff,
                    _ => self.get(destination) & !(field << lsb) | (source & field) << lsb,
                };
                self.set(destination, result)?;
            }
            "movw" => {
                let destination = reg_at(ops, 0)?;
                self.set(destination, imm_at(ops, 1)? & 0xffff)?;
            }
            "movt" => {
                let destination = reg_at(ops, 0)?;
                let value = self.get(destination) & 0xffff | (imm_at(ops, 1)? & 0xffff) << 16;
                self.set(destination, value)?;
            }
            "ldr" | "ldrb" | "ldrh" | "ldrsb" | "ldrsh" | "ldrd" | "str" | "strb" | "strh"
            | "strd" => {
                let target = reg_at(ops, 0)?;
                let dual = mnemonic.ends_with('d');
                let memory_index = usize::from(dual) + 1;
                let second = if dual { Some(reg_at(ops, 1)?) } else { None };
                let (address, update) =
                    self.a32_address(mem_at(ops, memory_index)?, &ops[memory_index + 1..])?;
                if let Some((base, _)) = update
                    && (base.index == target.index
                        || second.is_some_and(|s| s.index == base.index)
                        || base.index == PC)
                {
                    return Err(SimError("Load/store writeback overlaps its value register; constrained/unpredictable encoding is unsupported".into()));
                }
                let length = match &mnemonic[3..] {
                    "b" | "sb" => 1,
                    "h" | "sh" => 2,
                    _ => 4,
                };
                if mnemonic.starts_with("ld") {
                    let mut value = self.read(address, length)?;
                    if mnemonic[3..].starts_with('s') {
                        value = Self::signed(value, (length * 8) as u8) as u64 & 0xffff_ffff;
                    }
                    let second_value = match second {
                        Some(_) => Some(self.read(address.wrapping_add(4) & 0xffff_ffff, 4)?),
                        None => None,
                    };
                    if let Some((base, value)) = update {
                        self.set(base, value)?;
                    }
                    if let (Some(register), Some(value)) = (second, second_value) {
                        if register.index == PC {
                            return Err(SimError("LDRD into PC is unpredictable".into()));
                        }
                        self.set(register, value)?;
                    }
                    if target.index == PC && length != 4 {
                        return Err(SimError(
                            "Byte/halfword loads into PC are unpredictable".into(),
                        ));
                    }
                    self.set(target, value)?;
                } else {
                    let value = self.get(target);
                    self.write(address, length, value)?;
                    if let Some(register) = second {
                        let value = self.get(register);
                        self.write(address.wrapping_add(4) & 0xffff_ffff, 4, value)?;
                    }
                    if let Some((base, value)) = update {
                        self.set(base, value)?;
                    }
                }
            }
            "push" | "pop" | "ldm" | "ldmia" | "ldmfd" | "ldmib" | "ldmed" | "ldmda" | "ldmfa"
            | "ldmdb" | "ldmea" | "stm" | "stmia" | "stmea" | "stmib" | "stmfa" | "stmda"
            | "stmed" | "stmdb" | "stmfd" => {
                let (base, writeback, list) = match mnemonic {
                    "push" | "pop" => (
                        Reg {
                            index: 13,
                            bits: 32,
                            offset: 0,
                            zero_upper: true,
                        },
                        true,
                        ops.first(),
                    ),
                    _ => match ops.first() {
                        Some(Operand::Reg(register)) => (*register, false, ops.get(1)),
                        Some(Operand::RegBang(register)) => (*register, true, ops.get(1)),
                        _ => return Err(operand_error(0)),
                    },
                };
                let Some(Operand::List(list)) = list else {
                    return Err(operand_error(1));
                };
                if list.is_empty() || base.index == PC {
                    return Err(SimError("Unpredictable load/store multiple form".into()));
                }
                if writeback && list.iter().any(|r| r.index == base.index) {
                    return Err(SimError(if base.index == 13 {
                        "ARM push/pop with SP in the register list is unsupported".into()
                    } else {
                        "Load/store multiple with the written-back base in the list is unpredictable".into()
                    }));
                }
                let load = mnemonic == "pop" || mnemonic.starts_with("ldm");
                let mode = match mnemonic {
                    "push" | "stmdb" | "stmfd" | "ldmdb" | "ldmea" => "db",
                    "ldmib" | "ldmed" | "stmib" | "stmfa" => "ib",
                    "ldmda" | "ldmfa" | "stmda" | "stmed" => "da",
                    _ => "ia",
                };
                let size = 4 * list.len() as u64;
                let base_value = self.get(base);
                let start = match mode {
                    "ia" => base_value,
                    "ib" => base_value + 4,
                    "da" => base_value.wrapping_sub(size) + 4,
                    _ => base_value.wrapping_sub(size),
                } & 0xffff_ffff;
                let final_base = if matches!(mode, "ia" | "ib") {
                    base_value.wrapping_add(size)
                } else {
                    base_value.wrapping_sub(size)
                } & 0xffff_ffff;
                let mut sorted = list.clone();
                sorted.sort_by_key(|r| if r.index == PC { 15 } else { r.index });
                if load {
                    let mut values = Vec::with_capacity(sorted.len());
                    for (slot, register) in sorted.iter().enumerate() {
                        let value =
                            self.read(start.wrapping_add(4 * slot as u64) & 0xffff_ffff, 4)?;
                        values.push((*register, value));
                    }
                    if writeback {
                        self.set(base, final_base)?;
                    }
                    for (register, value) in values {
                        self.set(register, value)?;
                    }
                } else {
                    for (slot, register) in sorted.iter().enumerate() {
                        let value = self.get(*register);
                        self.write(start.wrapping_add(4 * slot as u64) & 0xffff_ffff, 4, value)?;
                    }
                    if writeback {
                        self.set(base, final_base)?;
                    }
                }
            }
            "b" => self.branch(imm_at(ops, 0)?)?,
            "bl" => {
                let target = imm_at(ops, 0)?;
                self.registers[14] = self.next_pc;
                self.branch(target)?;
            }
            "bx" => self.branch(self.get(reg_at(ops, 0)?))?,
            "blx" => {
                let target = self.get(reg_at(ops, 0)?);
                self.registers[14] = self.next_pc;
                self.branch(target)?;
            }
            "mrs" => {
                let destination = reg_at(ops, 0)?;
                if !matches!(ops.get(1), Some(Operand::Name(name)) if name == "apsr" || name == "cpsr")
                {
                    return Err(operand_error(1));
                }
                let value = u64::from(self.flag(N)) << 31
                    | u64::from(self.flag(Z)) << 30
                    | u64::from(self.flag(C)) << 29
                    | u64::from(self.flag(V)) << 28;
                self.set(destination, value)?;
            }
            "msr" => {
                if !matches!(ops.first(), Some(Operand::Name(name)) if name == "apsr_nzcvq") {
                    return Err(SimError(
                        "Only MSR APSR_nzcvq is supported; system state is outside the scalar model".into(),
                    ));
                }
                let value = match ops.get(1) {
                    Some(Operand::Reg(register)) => self.get(*register),
                    Some(Operand::Imm(value)) => *value,
                    _ => return Err(operand_error(1)),
                };
                // The Q (saturation) bit is not modelled.
                self.set_flag(N, value >> 31 & 1 != 0);
                self.set_flag(Z, value >> 30 & 1 != 0);
                self.set_flag(C, value >> 29 & 1 != 0);
                self.set_flag(V, value >> 28 & 1 != 0);
            }
            "nop" => {}
            "bkpt" => self.teaching_trap(imm_at(ops, 0)?)?,
            "svc" => {
                if imm_at(ops, 0)? != 0 {
                    return Err(SimError("Only svc #0 supports the teaching ABI".into()));
                }
                self.syscall()?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
