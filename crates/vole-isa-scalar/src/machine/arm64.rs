//! A64 integer instruction semantics (Arm ARM DDI 0487, chapter C6).
use super::operand::{Amount, Mem, Operand, Reg, Shift, ZR};
use super::{C, Decoded, N, ScalarMachine, V, Z};
use vole_core::SimError;

pub(super) fn operand_error(index: usize) -> SimError {
    SimError(format!("Unsupported operand form at operand {}", index + 1))
}
pub(super) fn reg_at(operands: &[Operand], index: usize) -> Result<Reg, SimError> {
    match operands.get(index) {
        Some(Operand::Reg(register)) => Ok(*register),
        _ => Err(operand_error(index)),
    }
}
pub(super) fn imm_at(operands: &[Operand], index: usize) -> Result<u64, SimError> {
    match operands.get(index) {
        Some(Operand::Imm(value)) => Ok(*value),
        _ => Err(operand_error(index)),
    }
}
pub(super) fn mem_at(operands: &[Operand], index: usize) -> Result<&Mem, SimError> {
    match operands.get(index) {
        Some(Operand::Mem(memory)) => Ok(memory),
        _ => Err(operand_error(index)),
    }
}
fn cond_at(operands: &[Operand], index: usize) -> Result<u8, SimError> {
    match operands.get(index) {
        Some(Operand::Cond(code)) => Ok(*code),
        _ => Err(operand_error(index)),
    }
}

/// Apply an A64 shift or extend to `value`, producing a `bits`-wide result.
pub(super) fn modify(value: u64, bits: u8, shift: Shift, amount: u32) -> u64 {
    let mask = ScalarMachine::mask(bits);
    let shl = |v: u64| v.checked_shl(amount).unwrap_or(0);
    let value = match shift {
        Shift::Lsl => shl(value & mask),
        Shift::Lsr => (value & mask).checked_shr(amount).unwrap_or(0),
        Shift::Asr => (ScalarMachine::signed(value & mask, bits) >> amount.min(63)) as u64,
        Shift::Ror => {
            let amount = amount % u32::from(bits);
            let value = value & mask;
            if amount == 0 {
                value
            } else {
                value >> amount | value << (u32::from(bits) - amount)
            }
        }
        Shift::Rrx => value,
        Shift::Uxtb => shl(value & 0xff),
        Shift::Uxth => shl(value & 0xffff),
        Shift::Uxtw => shl(value & 0xffff_ffff),
        Shift::Uxtx => shl(value),
        Shift::Sxtb => shl(value as u8 as i8 as i64 as u64),
        Shift::Sxth => shl(value as u16 as i16 as i64 as u64),
        Shift::Sxtw => shl(value as u32 as i32 as i64 as u64),
        Shift::Sxtx => shl(value),
    };
    value & mask
}

fn reverse_bytes_in(value: u64, bits: u8, chunk: u8) -> u64 {
    let mut result = 0;
    let mut offset = 0;
    while offset < bits {
        let part = (value >> offset) & ScalarMachine::mask(chunk);
        let swapped = part.swap_bytes() >> (64 - u32::from(chunk));
        result |= swapped << offset;
        offset += chunk;
    }
    result
}

pub(super) fn leading_zeros(value: u64, bits: u8) -> u64 {
    let value = value & ScalarMachine::mask(bits);
    u64::from(value.leading_zeros()) - (64 - u64::from(bits))
}

impl ScalarMachine {
    fn a64_value(&self, operands: &[Operand], index: usize, bits: u8) -> Result<u64, SimError> {
        let value = match operands.get(index) {
            Some(Operand::Reg(register)) => self.get(*register),
            Some(Operand::Imm(value)) => *value,
            _ => return Err(operand_error(index)),
        };
        Ok(match operands.get(index + 1) {
            Some(Operand::Modifier(modifier)) => {
                let amount = match modifier.amount {
                    Amount::Imm(amount) => amount,
                    Amount::Reg(register) => self.get(register) as u32,
                };
                modify(value, bits, modifier.shift, amount)
            }
            _ => value & Self::mask(bits),
        })
    }
    /// A64 effective address and optional base writeback.
    fn a64_address(
        &mut self,
        memory: &Mem,
        post: Option<&Operand>,
    ) -> Result<(u64, Option<(Reg, u64)>), SimError> {
        let base = memory.base.ok_or_else(|| operand_error(1))?;
        let base_value = self.get(base);
        let mut offset = memory.displacement;
        if let Some(index) = memory.index {
            let value = self.get(index);
            offset = match memory.modifier {
                Some(modifier) => {
                    let Amount::Imm(amount) = modifier.amount else {
                        return Err(operand_error(1));
                    };
                    modify(value, 64, modifier.shift, amount)
                }
                None => value,
            };
        }
        let address = base_value.wrapping_add(offset);
        Ok(match post {
            Some(Operand::Imm(step)) => (base_value, Some((base, base_value.wrapping_add(*step)))),
            Some(_) => return Err(operand_error(2)),
            None if memory.writeback => (address, Some((base, address))),
            None => (address, None),
        })
    }

    fn bitfield_move(
        &mut self,
        destination: Reg,
        source: u64,
        immr: u32,
        imms: u32,
        kind: u8,
    ) -> Result<(), SimError> {
        // kind: 0 = UBFM, 1 = SBFM, 2 = BFM (Arm ARM shared/functions DecodeBitMasks).
        let bits = u32::from(destination.bits);
        if immr >= bits || imms >= bits {
            return Err(SimError("Bitfield position exceeds register width".into()));
        }
        let mask = Self::mask(destination.bits);
        let source = source & mask;
        let field = |width: u32| Self::mask(width as u8);
        let (value, positioned_mask) = if imms >= immr {
            let width = imms - immr + 1;
            ((source >> immr) & field(width), field(width))
        } else {
            let width = imms + 1;
            let shift = bits - immr;
            (
                ((source & field(width)) << shift) & mask,
                (field(width) << shift) & mask,
            )
        };
        let result = match kind {
            0 => value,
            1 => {
                let top = if imms >= immr {
                    imms - immr
                } else {
                    bits - immr + imms
                };
                let sign = source >> imms & 1 != 0;
                if sign {
                    value | (mask & !Self::mask((top + 1) as u8))
                } else {
                    value
                }
            }
            _ => (self.get(destination) & !positioned_mask) | value,
        };
        self.set(destination, result)
    }

    pub(super) fn execute_arm64(
        &mut self,
        decoded: &Decoded,
        ops: &[Operand],
    ) -> Result<bool, SimError> {
        let mnemonic = decoded.mnemonic.as_str();
        match mnemonic {
            "ldr" | "ldrb" | "ldrh" | "ldrsb" | "ldrsh" | "ldrsw" | "ldur" | "ldurb" | "ldurh"
            | "ldursb" | "ldursh" | "ldursw" | "str" | "strb" | "strh" | "stur" | "sturb"
            | "sturh" => {
                let target = reg_at(ops, 0)?;
                let tail = mnemonic[2..]
                    .strip_prefix("ur")
                    .or_else(|| mnemonic[2..].strip_prefix('r'))
                    .unwrap_or("");
                let length = match tail {
                    "b" | "sb" => 1,
                    "h" | "sh" => 2,
                    "sw" => 4,
                    _ => usize::from(target.bits / 8),
                };
                let load = mnemonic.starts_with("ld");
                let (address, update) = match ops.get(1) {
                    Some(Operand::Imm(address)) if load => (*address, None),
                    Some(Operand::Mem(memory)) => self.a64_address(memory, ops.get(2))?,
                    _ => return Err(operand_error(1)),
                };
                if let Some((base, _)) = update
                    && base.index == target.index
                {
                    return Err(SimError("Load/store writeback overlaps its value register; constrained/unpredictable encoding is unsupported".into()));
                }
                if load {
                    let mut value = self.read(address, length)?;
                    if tail.starts_with('s') {
                        value = Self::signed(value, (length * 8) as u8) as u64;
                    }
                    self.set(target, value)?;
                } else {
                    let value = self.get(target);
                    self.write(address, length, value)?;
                }
                if let Some((base, value)) = update {
                    self.set(base, value)?;
                }
            }
            "ldp" | "stp" | "ldpsw" | "ldnp" | "stnp" => {
                let (first, second) = (reg_at(ops, 0)?, reg_at(ops, 1)?);
                let length = if mnemonic == "ldpsw" {
                    4
                } else {
                    usize::from(first.bits / 8)
                };
                let (address, update) = self.a64_address(mem_at(ops, 2)?, ops.get(3))?;
                if let Some((base, _)) = update
                    && (base.index == first.index || base.index == second.index)
                {
                    return Err(SimError(
                        "Pair access writeback overlaps a value register".into(),
                    ));
                }
                let second_address = address
                    .checked_add(length as u64)
                    .ok_or_else(|| SimError("Address overflow".into()))?;
                if mnemonic.starts_with("ld") {
                    if first.index == second.index && first.index != ZR {
                        return Err(SimError("Pair load destinations overlap".into()));
                    }
                    let mut a = self.read(address, length)?;
                    let mut b = self.read(second_address, length)?;
                    if mnemonic == "ldpsw" {
                        a = Self::signed(a, 32) as u64;
                        b = Self::signed(b, 32) as u64;
                    }
                    self.set(first, a)?;
                    self.set(second, b)?;
                } else {
                    let (a, b) = (self.get(first), self.get(second));
                    self.write(address, length, a)?;
                    self.write(second_address, length, b)?;
                }
                if let Some((base, value)) = update {
                    self.set(base, value)?;
                }
            }
            "mov" | "movz" => {
                let destination = reg_at(ops, 0)?;
                let value = self.a64_value(ops, 1, destination.bits)?;
                self.set(destination, value)?;
            }
            "movn" => {
                let destination = reg_at(ops, 0)?;
                let value = !self.a64_value(ops, 1, destination.bits)?;
                self.set(destination, value)?;
            }
            "movk" => {
                let destination = reg_at(ops, 0)?;
                let shift = match ops.get(2) {
                    Some(Operand::Modifier(modifier)) => match modifier.amount {
                        Amount::Imm(amount) => amount,
                        Amount::Reg(_) => return Err(operand_error(2)),
                    },
                    _ => 0,
                };
                if shift >= u32::from(destination.bits) || shift % 16 != 0 {
                    return Err(SimError("Invalid wide-move halfword".into()));
                }
                let value = imm_at(ops, 1)? & 0xffff;
                let previous = self.get(destination);
                self.set(
                    destination,
                    previous & !(0xffff_u64 << shift) | value << shift,
                )?;
            }
            "mvn" => {
                let destination = reg_at(ops, 0)?;
                let value = !self.a64_value(ops, 1, destination.bits)?;
                self.set(destination, value)?;
            }
            "neg" | "negs" | "ngc" | "ngcs" => {
                let destination = reg_at(ops, 0)?;
                let value = self.a64_value(ops, 1, destination.bits)?;
                let carry = if mnemonic.starts_with("ngc") {
                    self.flag(C)
                } else {
                    true
                };
                let result = self.add_with_carry(
                    0,
                    !value,
                    carry,
                    destination.bits,
                    mnemonic.ends_with('s'),
                );
                self.set(destination, result)?;
            }
            "add" | "adds" | "sub" | "subs" | "adc" | "adcs" | "sbc" | "sbcs" | "cmp" | "cmn" => {
                let compare = matches!(mnemonic, "cmp" | "cmn");
                let first = usize::from(!compare);
                let left = reg_at(ops, first)?;
                let bits = left.bits;
                let a = self.get(left);
                let b = self.a64_value(ops, first + 1, bits)?;
                let (y, carry) = match mnemonic {
                    "add" | "adds" | "cmn" => (b, false),
                    "sub" | "subs" | "cmp" => (!b, true),
                    "adc" | "adcs" => (b, self.flag(C)),
                    _ => (!b, self.flag(C)),
                };
                let update = compare || mnemonic.ends_with('s');
                let result = self.add_with_carry(a, y, carry, bits, update);
                if !compare {
                    self.set(reg_at(ops, 0)?, result)?;
                }
            }
            "and" | "ands" | "orr" | "eor" | "bic" | "bics" | "orn" | "eon" | "tst" => {
                let test = mnemonic == "tst";
                let first = usize::from(!test);
                let left = reg_at(ops, first)?;
                let bits = left.bits;
                let a = self.get(left);
                let b = self.a64_value(ops, first + 1, bits)?;
                let result = match mnemonic {
                    "and" | "ands" | "tst" => a & b,
                    "orr" => a | b,
                    "eor" => a ^ b,
                    "bic" | "bics" => a & !b,
                    "orn" => a | !b,
                    _ => a ^ !b,
                } & Self::mask(bits);
                if matches!(mnemonic, "ands" | "bics" | "tst") {
                    self.set_nz(result, bits);
                    self.set_flag(C, false);
                    self.set_flag(V, false);
                }
                if !test {
                    self.set(reg_at(ops, 0)?, result)?;
                }
            }
            "lsl" | "lsr" | "asr" | "ror" => {
                let destination = reg_at(ops, 0)?;
                let bits = destination.bits;
                let value = self.get(reg_at(ops, 1)?);
                let amount = match ops.get(2) {
                    Some(Operand::Imm(amount)) => *amount as u32,
                    Some(Operand::Reg(register)) => (self.get(*register) % u64::from(bits)) as u32,
                    _ => return Err(operand_error(2)),
                };
                if amount >= u32::from(bits) {
                    return Err(SimError("Shift amount exceeds register width".into()));
                }
                let shift = match mnemonic {
                    "lsl" => Shift::Lsl,
                    "lsr" => Shift::Lsr,
                    "asr" => Shift::Asr,
                    _ => Shift::Ror,
                };
                self.set(destination, modify(value, bits, shift, amount))?;
            }
            "mul" | "mneg" | "madd" | "msub" => {
                let destination = reg_at(ops, 0)?;
                let product = self
                    .get(reg_at(ops, 1)?)
                    .wrapping_mul(self.get(reg_at(ops, 2)?));
                let result = match mnemonic {
                    "mul" => product,
                    "mneg" => product.wrapping_neg(),
                    "madd" => self.get(reg_at(ops, 3)?).wrapping_add(product),
                    _ => self.get(reg_at(ops, 3)?).wrapping_sub(product),
                };
                self.set(destination, result)?;
            }
            "smull" | "umull" | "smnegl" | "umnegl" | "smaddl" | "umaddl" | "smsubl" | "umsubl" => {
                let destination = reg_at(ops, 0)?;
                let (a, b) = (self.get(reg_at(ops, 1)?), self.get(reg_at(ops, 2)?));
                let product = if mnemonic.starts_with('s') {
                    (Self::signed(a, 32) * Self::signed(b, 32)) as u64
                } else {
                    (a & 0xffff_ffff) * (b & 0xffff_ffff)
                };
                let result = match &mnemonic[1..] {
                    "mull" => product,
                    "mnegl" => product.wrapping_neg(),
                    "maddl" => self.get(reg_at(ops, 3)?).wrapping_add(product),
                    _ => self.get(reg_at(ops, 3)?).wrapping_sub(product),
                };
                self.set(destination, result)?;
            }
            "smulh" | "umulh" => {
                let destination = reg_at(ops, 0)?;
                let (a, b) = (self.get(reg_at(ops, 1)?), self.get(reg_at(ops, 2)?));
                let high = if mnemonic == "smulh" {
                    ((i128::from(a as i64) * i128::from(b as i64)) >> 64) as u64
                } else {
                    ((u128::from(a) * u128::from(b)) >> 64) as u64
                };
                self.set(destination, high)?;
            }
            "sdiv" | "udiv" => {
                // A64 integer division never traps: x / 0 = 0, MIN / -1 = MIN.
                let destination = reg_at(ops, 0)?;
                let bits = destination.bits;
                let (a, b) = (self.get(reg_at(ops, 1)?), self.get(reg_at(ops, 2)?));
                let result = if b == 0 {
                    0
                } else if mnemonic == "sdiv" {
                    Self::signed(a, bits).wrapping_div(Self::signed(b, bits)) as u64
                } else {
                    a / b
                };
                self.set(destination, result)?;
            }
            "csel" | "csinc" | "csinv" | "csneg" => {
                let destination = reg_at(ops, 0)?;
                let (a, b) = (self.get(reg_at(ops, 1)?), self.get(reg_at(ops, 2)?));
                let result = if self.arm_condition(cond_at(ops, 3)?) {
                    a
                } else {
                    match mnemonic {
                        "csel" => b,
                        "csinc" => b.wrapping_add(1),
                        "csinv" => !b,
                        _ => b.wrapping_neg(),
                    }
                };
                self.set(destination, result)?;
            }
            "cset" | "csetm" => {
                let destination = reg_at(ops, 0)?;
                let holds = self.arm_condition(cond_at(ops, 1)?);
                let value = match (holds, mnemonic) {
                    (false, _) => 0,
                    (true, "cset") => 1,
                    (true, _) => u64::MAX,
                };
                self.set(destination, value)?;
            }
            "cinc" | "cinv" | "cneg" => {
                let destination = reg_at(ops, 0)?;
                let value = self.get(reg_at(ops, 1)?);
                let result = if self.arm_condition(cond_at(ops, 2)?) {
                    match mnemonic {
                        "cinc" => value.wrapping_add(1),
                        "cinv" => !value,
                        _ => value.wrapping_neg(),
                    }
                } else {
                    value
                };
                self.set(destination, result)?;
            }
            "ccmp" | "ccmn" => {
                let left = reg_at(ops, 0)?;
                let a = self.get(left);
                let b = self.a64_value(ops, 1, left.bits)?;
                let nzcv = imm_at(ops, 2)?;
                if self.arm_condition(cond_at(ops, 3)?) {
                    if mnemonic == "ccmp" {
                        self.add_with_carry(a, !b, true, left.bits, true);
                    } else {
                        self.add_with_carry(a, b, false, left.bits, true);
                    }
                } else {
                    self.set_flag(N, nzcv & 8 != 0);
                    self.set_flag(Z, nzcv & 4 != 0);
                    self.set_flag(C, nzcv & 2 != 0);
                    self.set_flag(V, nzcv & 1 != 0);
                }
            }
            "ubfx" | "sbfx" | "ubfiz" | "sbfiz" | "bfi" | "bfxil" | "bfc" => {
                let destination = reg_at(ops, 0)?;
                let bits = u64::from(destination.bits);
                let (source, first) = if mnemonic == "bfc" {
                    (0, 1)
                } else {
                    (self.get(reg_at(ops, 1)?), 2)
                };
                let (lsb, width) = (imm_at(ops, first)?, imm_at(ops, first + 1)?);
                if width == 0 || lsb + width > bits {
                    return Err(SimError("Bitfield exceeds register width".into()));
                }
                let (immr, imms) = match mnemonic {
                    "ubfx" | "sbfx" | "bfxil" => (lsb, lsb + width - 1),
                    _ => ((bits - lsb) % bits, width - 1),
                };
                let kind = match mnemonic {
                    "ubfx" | "ubfiz" => 0,
                    "sbfx" | "sbfiz" => 1,
                    _ => 2,
                };
                self.bitfield_move(destination, source, immr as u32, imms as u32, kind)?;
            }
            "ubfm" | "sbfm" | "bfm" => {
                let destination = reg_at(ops, 0)?;
                let source = self.get(reg_at(ops, 1)?);
                let (immr, imms) = (imm_at(ops, 2)? as u32, imm_at(ops, 3)? as u32);
                let kind = match mnemonic {
                    "ubfm" => 0,
                    "sbfm" => 1,
                    _ => 2,
                };
                self.bitfield_move(destination, source, immr, imms, kind)?;
            }
            "sxtb" | "sxth" | "sxtw" | "uxtb" | "uxth" => {
                let destination = reg_at(ops, 0)?;
                let value = self.get(reg_at(ops, 1)?);
                let shift = match mnemonic {
                    "sxtb" => Shift::Sxtb,
                    "sxth" => Shift::Sxth,
                    "sxtw" => Shift::Sxtw,
                    "uxtb" => Shift::Uxtb,
                    _ => Shift::Uxth,
                };
                self.set(destination, modify(value, destination.bits, shift, 0))?;
            }
            "extr" => {
                let destination = reg_at(ops, 0)?;
                let bits = destination.bits;
                let (high, low) = (self.get(reg_at(ops, 1)?), self.get(reg_at(ops, 2)?));
                let lsb = imm_at(ops, 3)? as u32;
                if lsb >= u32::from(bits) {
                    return Err(SimError("Extract position exceeds register width".into()));
                }
                let result = if lsb == 0 {
                    low
                } else {
                    low >> lsb | high << (u32::from(bits) - lsb)
                };
                self.set(destination, result)?;
            }
            "rev" | "rev64" | "rev16" | "rev32" | "rbit" | "clz" | "cls" => {
                let destination = reg_at(ops, 0)?;
                let bits = destination.bits;
                let value = self.get(reg_at(ops, 1)?);
                let result = match mnemonic {
                    "rev" | "rev64" => reverse_bytes_in(value, bits, bits),
                    "rev16" => reverse_bytes_in(value, bits, 16),
                    "rev32" => reverse_bytes_in(value, bits, 32),
                    "rbit" => value.reverse_bits() >> (64 - u32::from(bits)),
                    "clz" => leading_zeros(value, bits),
                    _ => leading_zeros((value ^ (value >> 1)) & Self::mask(bits - 1), bits - 1),
                };
                self.set(destination, result)?;
            }
            "adr" | "adrp" => {
                self.set(reg_at(ops, 0)?, imm_at(ops, 1)?)?;
            }
            "b" => self.branch(imm_at(ops, 0)?)?,
            "bl" => {
                let target = imm_at(ops, 0)?;
                self.registers[30] = self.next_pc;
                self.branch(target)?;
            }
            "br" => self.branch(self.get(reg_at(ops, 0)?))?,
            "blr" => {
                let target = self.get(reg_at(ops, 0)?);
                self.registers[30] = self.next_pc;
                self.branch(target)?;
            }
            "ret" => {
                let target = match ops.first() {
                    Some(Operand::Reg(register)) => self.get(*register),
                    None => self.registers[30],
                    _ => return Err(operand_error(0)),
                };
                self.branch(target)?;
            }
            "cbz" | "cbnz" => {
                let value = self.get(reg_at(ops, 0)?);
                if (value == 0) == (mnemonic == "cbz") {
                    self.branch(imm_at(ops, 1)?)?;
                }
            }
            "tbz" | "tbnz" => {
                let register = reg_at(ops, 0)?;
                let bit = imm_at(ops, 1)?;
                if bit >= u64::from(register.bits) {
                    return Err(SimError("Test branch bit exceeds register width".into()));
                }
                if (self.get(register) >> bit & 1 == 0) == (mnemonic == "tbz") {
                    self.branch(imm_at(ops, 2)?)?;
                }
            }
            _ if mnemonic.starts_with("b.") => {
                let code = super::operand::arm_condition_code(&mnemonic[2..])
                    .ok_or_else(|| SimError(format!("Unsupported ARM condition {mnemonic}")))?;
                if self.arm_condition(code) {
                    self.branch(imm_at(ops, 0)?)?;
                }
            }
            "nop" => {}
            "brk" => self.teaching_trap(imm_at(ops, 0)?)?,
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
