//! IA-32 and x86-64 general-purpose instruction semantics (Intel SDM volume 2).
//! Flags documented as undefined keep their previous, deterministic value.
use super::arm64::{imm_at, operand_error, reg_at};
use super::operand::{Operand, Reg, x86_condition_code};
use super::{AF, CF, Decoded, OF, PF, SF, ScalarMachine, ZF};
use vole_core::{Architecture, SimError};

const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;
const RSP: u8 = 4;
const RBP: u8 = 5;
const RSI: u8 = 6;
const RDI: u8 = 7;

fn view(index: u8, bits: u8) -> Reg {
    Reg {
        index,
        bits,
        offset: 0,
        zero_upper: bits >= 32,
    }
}

impl ScalarMachine {
    fn x86_bits(operand: Option<&Operand>, fallback: u8) -> u8 {
        match operand {
            Some(Operand::Reg(register)) => register.bits,
            Some(Operand::Mem(memory)) if memory.bits != 0 => memory.bits,
            _ => fallback,
        }
    }
    fn x86_read(&mut self, operand: Option<&Operand>, bits: u8) -> Result<u64, SimError> {
        match operand {
            Some(Operand::Reg(register)) => Ok(self.get(*register)),
            Some(Operand::Imm(value)) => Ok(value & Self::mask(bits)),
            Some(Operand::Mem(memory)) => {
                let address = self.x86_address(memory)?;
                let size = if memory.bits != 0 { memory.bits } else { bits };
                self.read(address, usize::from(size / 8))
            }
            _ => Err(SimError("Unsupported x86 operand form".into())),
        }
    }
    fn x86_write(
        &mut self,
        operand: Option<&Operand>,
        bits: u8,
        value: u64,
    ) -> Result<(), SimError> {
        match operand {
            Some(Operand::Reg(register)) => self.set(*register, value),
            Some(Operand::Mem(memory)) => {
                let address = self.x86_address(memory)?;
                let size = if memory.bits != 0 { memory.bits } else { bits };
                self.write(address, usize::from(size / 8), value)
            }
            _ => Err(SimError("Unsupported x86 destination".into())),
        }
    }
    fn set_szp(&mut self, result: u64, bits: u8) {
        let result = result & Self::mask(bits);
        self.set_flag(SF, result >> (bits - 1) & 1 != 0);
        self.set_flag(ZF, result == 0);
        self.set_flag(PF, (result as u8).count_ones().is_multiple_of(2));
    }
    /// ADD/ADC: all six status flags.
    fn x86_add(&mut self, a: u64, b: u64, carry: bool, bits: u8) -> u64 {
        let mask = Self::mask(bits);
        let (a, b) = (a & mask, b & mask);
        let wide = u128::from(a) + u128::from(b) + u128::from(carry);
        let result = wide as u64 & mask;
        let sign = 1_u64 << (bits - 1);
        self.set_flag(CF, wide > u128::from(mask));
        self.set_flag(OF, !(a ^ b) & (a ^ result) & sign != 0);
        self.set_flag(AF, (a ^ b ^ result) & 0x10 != 0);
        self.set_szp(result, bits);
        result
    }
    /// SUB/SBB/CMP/NEG: all six status flags, CF is the borrow.
    fn x86_sub(&mut self, a: u64, b: u64, borrow: bool, bits: u8) -> u64 {
        let mask = Self::mask(bits);
        let (a, b) = (a & mask, b & mask);
        let result = a.wrapping_sub(b).wrapping_sub(u64::from(borrow)) & mask;
        let sign = 1_u64 << (bits - 1);
        self.set_flag(CF, u128::from(a) < u128::from(b) + u128::from(borrow));
        self.set_flag(OF, (a ^ b) & (a ^ result) & sign != 0);
        self.set_flag(AF, (a ^ b ^ result) & 0x10 != 0);
        self.set_szp(result, bits);
        result
    }
    fn x86_logic(&mut self, result: u64, bits: u8) {
        self.set_flag(CF, false);
        self.set_flag(OF, false);
        self.set_szp(result, bits);
    }
    fn x86_condition(&self, code: u8) -> bool {
        let result = match code >> 1 {
            0 => self.flag(OF),
            1 => self.flag(CF),
            2 => self.flag(ZF),
            3 => self.flag(CF) || self.flag(ZF),
            4 => self.flag(SF),
            5 => self.flag(PF),
            6 => self.flag(SF) != self.flag(OF),
            _ => self.flag(ZF) || self.flag(SF) != self.flag(OF),
        };
        result != (code & 1 == 1)
    }
    fn stack_view(&self) -> Reg {
        view(RSP, self.architecture.bits())
    }
    fn push(&mut self, value: u64, width: usize) -> Result<(), SimError> {
        let sp = self.stack_view();
        let address = self.address(self.get(sp).wrapping_sub(width as u64));
        self.write(address, width, value)?;
        self.set(sp, address)
    }
    fn pop(&mut self, width: usize) -> Result<u64, SimError> {
        let sp = self.stack_view();
        let address = self.get(sp);
        let value = self.read(address, width)?;
        self.set(sp, self.address(address.wrapping_add(width as u64)))?;
        Ok(value)
    }
    fn divide_error(&self, reason: &str) -> SimError {
        SimError(format!("Divide error (#DE): {reason}"))
    }

    fn x86_shift(&mut self, mnemonic: &str, ops: &[Operand], bits: u8) -> Result<(), SimError> {
        let value = self.x86_read(ops.first(), bits)? & Self::mask(bits);
        let count = match ops.get(1) {
            None => 1,
            some => self.x86_read(some, 8)?,
        } as u32;
        let raw = count & if bits == 64 { 63 } else { 31 };
        let width = u32::from(bits);
        let msb = |v: u64| v >> (bits - 1) & 1 != 0;
        if raw == 0 {
            if let Some(Operand::Reg(register)) = ops.first() {
                self.set(*register, value)?;
            }
            return Ok(());
        }
        let (result, carry) = match mnemonic {
            "shl" | "sal" => (
                value.checked_shl(raw).unwrap_or(0),
                raw <= width && value >> (width - raw) & 1 != 0,
            ),
            "shr" => (
                value.checked_shr(raw).unwrap_or(0),
                raw <= width && value >> (raw - 1) & 1 != 0,
            ),
            "sar" => {
                let signed = Self::signed(value, bits);
                (
                    (signed >> raw.min(63)) as u64,
                    if raw >= width {
                        msb(value)
                    } else {
                        value >> (raw - 1) & 1 != 0
                    },
                )
            }
            "rol" | "ror" => {
                let r = raw % width;
                let result = if r == 0 {
                    value
                } else if mnemonic == "rol" {
                    value << r | value >> (width - r)
                } else {
                    value >> r | value << (width - r)
                } & Self::mask(bits);
                let carry = if mnemonic == "rol" {
                    result & 1 != 0
                } else {
                    msb(result)
                };
                (result, carry)
            }
            _ => unreachable!(),
        };
        let result = result & Self::mask(bits);
        if !matches!(mnemonic, "rol" | "ror") {
            self.set_szp(result, bits);
        }
        self.set_flag(CF, carry);
        if raw == 1 {
            let overflow = match mnemonic {
                "shl" | "sal" | "rol" => msb(result) ^ carry,
                "shr" => msb(value),
                "sar" => false,
                _ => msb(result) ^ (result >> (bits - 2) & 1 != 0),
            };
            self.set_flag(OF, overflow);
        }
        self.x86_write(ops.first(), bits, result)
    }

    fn x86_multiply_divide(
        &mut self,
        mnemonic: &str,
        ops: &[Operand],
        bits: u8,
    ) -> Result<(), SimError> {
        let source = self.x86_read(ops.first(), bits)?;
        let signed = mnemonic.starts_with('i');
        let mask = Self::mask(bits);
        let low_register = view(RAX, bits);
        let high_register = if bits == 8 {
            Reg {
                index: RAX,
                bits: 8,
                offset: 8,
                zero_upper: false,
            }
        } else {
            view(RDX, bits)
        };
        let accumulator = self.get(low_register);
        if mnemonic.ends_with("mul") {
            let (low, high, overflow) = if signed {
                let product = i128::from(Self::signed(accumulator, bits))
                    * i128::from(Self::signed(source, bits));
                let low = product as u64 & mask;
                let high = (product >> bits) as u64 & mask;
                (low, high, product != i128::from(Self::signed(low, bits)))
            } else {
                let product = u128::from(accumulator) * u128::from(source);
                let high = (product >> bits) as u64 & mask;
                (product as u64 & mask, high, high != 0)
            };
            if bits == 8 {
                self.set(view(RAX, 16), high << 8 | low)?;
            } else {
                self.set(low_register, low)?;
                self.set(high_register, high)?;
            }
            self.set_flag(CF, overflow);
            self.set_flag(OF, overflow);
            return Ok(());
        }
        if source & mask == 0 {
            return Err(self.divide_error("division by zero"));
        }
        let high = self.get(high_register);
        let dividend = u128::from(high) << bits | u128::from(accumulator);
        let (quotient, remainder) = if signed {
            let double = bits * 2;
            let dividend = if double == 128 {
                dividend as i128
            } else {
                ((dividend << (128 - u32::from(double))) as i128) >> (128 - u32::from(double))
            };
            let divisor = i128::from(Self::signed(source, bits));
            let quotient = dividend / divisor;
            let remainder = dividend % divisor;
            let limit = 1_i128 << (bits - 1);
            if quotient < -limit || quotient >= limit {
                return Err(
                    self.divide_error(&format!("signed quotient does not fit in {bits} bits"))
                );
            }
            (quotient as u64 & mask, remainder as u64 & mask)
        } else {
            let quotient = dividend / u128::from(source);
            if quotient > u128::from(mask) {
                return Err(self.divide_error(&format!("quotient does not fit in {bits} bits")));
            }
            (quotient as u64, (dividend % u128::from(source)) as u64)
        };
        if bits == 8 {
            self.set(view(RAX, 16), remainder << 8 | quotient)?;
        } else {
            self.set(low_register, quotient)?;
            self.set(high_register, remainder)?;
        }
        Ok(())
    }

    fn x86_string(&mut self, decoded: &Decoded, ops: &[Operand]) -> Result<bool, SimError> {
        if ops
            .iter()
            .any(|op| matches!(op, Operand::Name(name) if name.starts_with("xmm")))
        {
            return Ok(false);
        }
        let mnemonic = decoded.mnemonic.as_str();
        let size: u8 = match mnemonic.as_bytes()[mnemonic.len() - 1] {
            b'b' => 1,
            b'w' => 2,
            b'd' => 4,
            _ => 8,
        };
        let address_bits = self.architecture.bits();
        let counter = view(RCX, address_bits);
        let mut count = self.get(counter);
        if decoded.repeat && count == 0 {
            return Ok(true);
        }
        let destination = view(RDI, address_bits);
        let target = self.get(destination);
        if mnemonic.starts_with("movs") {
            let source = view(RSI, address_bits);
            let from = self.get(source);
            let value = self.read(from, usize::from(size))?;
            self.write(target, usize::from(size), value)?;
            self.set(source, from.wrapping_add(u64::from(size)))?;
        } else {
            let value = self.get(view(RAX, size * 8));
            self.write(target, usize::from(size), value)?;
        }
        self.set(destination, target.wrapping_add(u64::from(size)))?;
        if decoded.repeat {
            // One element per step keeps repeated moves interruptible, like hardware.
            count -= 1;
            self.set(counter, count)?;
            if count != 0 {
                self.pc = decoded.instruction.address;
            }
        }
        Ok(true)
    }

    pub(super) fn execute_x86(
        &mut self,
        decoded: &Decoded,
        ops: &[Operand],
    ) -> Result<bool, SimError> {
        let mnemonic = decoded.mnemonic.as_str();
        let x64 = self.architecture == Architecture::X64;
        if mnemonic != "nop" {
            for prefix in &decoded.instruction.bytes {
                match prefix {
                    0x67 => return Err(SimError("Address-size override is unsupported; use the target's default 32/64-bit addressing".into())),
                    0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 => return Err(SimError("Segment overrides are outside the flat teaching memory model".into())),
                    0x66 | 0xf0 | 0xf2 | 0xf3 => {}
                    0x40..=0x4f if x64 => {}
                    _ => break,
                }
            }
        }
        let arch_bits = self.architecture.bits();
        let bits = Self::x86_bits(ops.first(), arch_bits);
        match mnemonic {
            "mov" | "movabs" => {
                let value = self.x86_read(ops.get(1), bits)?;
                self.x86_write(ops.first(), bits, value)?;
            }
            "add" | "sub" | "adc" | "sbb" | "cmp" => {
                let a = self.x86_read(ops.first(), bits)?;
                let b = self.x86_read(ops.get(1), bits)?;
                let carry = self.flag(CF);
                let result = match mnemonic {
                    "add" => self.x86_add(a, b, false, bits),
                    "adc" => self.x86_add(a, b, carry, bits),
                    "sbb" => self.x86_sub(a, b, carry, bits),
                    _ => self.x86_sub(a, b, false, bits),
                };
                if mnemonic != "cmp" {
                    self.x86_write(ops.first(), bits, result)?;
                }
            }
            "and" | "or" | "xor" | "test" => {
                let a = self.x86_read(ops.first(), bits)?;
                let b = self.x86_read(ops.get(1), bits)?;
                let result = match mnemonic {
                    "and" | "test" => a & b,
                    "or" => a | b,
                    _ => a ^ b,
                } & Self::mask(bits);
                self.x86_logic(result, bits);
                if mnemonic != "test" {
                    self.x86_write(ops.first(), bits, result)?;
                }
            }
            "lea" => {
                let Some(Operand::Mem(memory)) = ops.get(1) else {
                    return Err(operand_error(1));
                };
                let address = self.x86_address(memory)?;
                self.set(reg_at(ops, 0)?, address)?;
            }
            "movzx" | "movsx" | "movsxd" => {
                let source_bits = Self::x86_bits(ops.get(1), 32);
                let value = self.x86_read(ops.get(1), source_bits)?;
                let value = if mnemonic == "movzx" {
                    value
                } else {
                    Self::signed(value, source_bits) as u64
                };
                self.set(reg_at(ops, 0)?, value)?;
            }
            "push" => {
                let width = match ops.first() {
                    Some(Operand::Imm(_)) => arch_bits,
                    other => Self::x86_bits(other, arch_bits),
                };
                let value = self.x86_read(ops.first(), width)?;
                self.push(value, usize::from(width / 8))?;
            }
            "pop" => {
                let value = self.pop(usize::from(bits / 8))?;
                self.x86_write(ops.first(), bits, value)?;
            }
            "call" => {
                let target = self.x86_read(ops.first(), arch_bits)?;
                self.push(self.next_pc, usize::from(arch_bits / 8))?;
                self.branch(target)?;
            }
            "ret" => {
                let target = self.pop(usize::from(arch_bits / 8))?;
                if !ops.is_empty() {
                    let count = imm_at(ops, 0)?;
                    let sp = self.stack_view();
                    let value = self.address(self.get(sp).wrapping_add(count));
                    self.set(sp, value)?;
                }
                self.branch(target)?;
            }
            "jmp" => {
                let target = self.x86_read(ops.first(), arch_bits)?;
                self.branch(target)?;
            }
            "leave" => {
                let frame = self.get(view(RBP, arch_bits));
                self.set(self.stack_view(), frame)?;
                let value = self.pop(usize::from(arch_bits / 8))?;
                self.set(view(RBP, arch_bits), value)?;
            }
            "inc" | "dec" => {
                let value = self.x86_read(ops.first(), bits)?;
                let carry = self.flag(CF);
                let result = if mnemonic == "inc" {
                    self.x86_add(value, 1, false, bits)
                } else {
                    self.x86_sub(value, 1, false, bits)
                };
                self.set_flag(CF, carry);
                self.x86_write(ops.first(), bits, result)?;
            }
            "neg" => {
                let value = self.x86_read(ops.first(), bits)?;
                let result = self.x86_sub(0, value, false, bits);
                self.x86_write(ops.first(), bits, result)?;
            }
            "not" => {
                let value = self.x86_read(ops.first(), bits)?;
                self.x86_write(ops.first(), bits, !value)?;
            }
            "shl" | "sal" | "shr" | "sar" | "rol" | "ror" => self.x86_shift(mnemonic, ops, bits)?,
            "shld" | "shrd" => {
                let destination = self.x86_read(ops.first(), bits)?;
                let source = self.x86_read(ops.get(1), bits)?;
                let raw = (self.x86_read(ops.get(2), 8)? as u32) & if bits == 64 { 63 } else { 31 };
                let width = u32::from(bits);
                if raw == 0 {
                    if let Some(Operand::Reg(register)) = ops.first() {
                        self.set(*register, destination)?;
                    }
                    return Ok(true);
                }
                if raw > width {
                    return Err(SimError(format!(
                        "{mnemonic} count {raw} exceeds the {bits}-bit operand; the result is architecturally undefined"
                    )));
                }
                let mask = Self::mask(bits);
                let (result, carry) = if mnemonic == "shld" {
                    (
                        (destination.checked_shl(raw).unwrap_or(0)
                            | source.checked_shr(width - raw).unwrap_or(0))
                            & mask,
                        destination >> (width - raw) & 1 != 0,
                    )
                } else {
                    (
                        (destination.checked_shr(raw).unwrap_or(0)
                            | source.checked_shl(width - raw).unwrap_or(0))
                            & mask,
                        destination >> (raw - 1) & 1 != 0,
                    )
                };
                self.set_szp(result, bits);
                self.set_flag(CF, carry);
                if raw == 1 {
                    self.set_flag(OF, (result ^ destination) >> (bits - 1) & 1 != 0);
                }
                self.x86_write(ops.first(), bits, result)?;
            }
            "mul" | "div" | "idiv" => self.x86_multiply_divide(mnemonic, ops, bits)?,
            "imul" => {
                if ops.len() == 1 {
                    self.x86_multiply_divide(mnemonic, ops, bits)?;
                } else {
                    let (left, right) = if ops.len() == 3 { (1, 2) } else { (0, 1) };
                    let a = self.x86_read(ops.get(left), bits)?;
                    let b = self.x86_read(ops.get(right), bits)?;
                    let product =
                        i128::from(Self::signed(a, bits)) * i128::from(Self::signed(b, bits));
                    let result = product as u64 & Self::mask(bits);
                    let overflow = product != i128::from(Self::signed(result, bits));
                    self.set_flag(CF, overflow);
                    self.set_flag(OF, overflow);
                    self.set(reg_at(ops, 0)?, result)?;
                }
            }
            "cbw" | "cwde" | "cdqe" => {
                let to = match mnemonic {
                    "cbw" => 16,
                    "cwde" => 32,
                    _ => 64,
                };
                let value = self.get(view(RAX, to / 2));
                self.set(
                    Reg {
                        zero_upper: to >= 32,
                        ..view(RAX, to)
                    },
                    Self::signed(value, to / 2) as u64,
                )?;
            }
            "cwd" | "cdq" | "cqo" => {
                let size = match mnemonic {
                    "cwd" => 16,
                    "cdq" => 32,
                    _ => 64,
                };
                let negative = self.get(view(RAX, size)) >> (size - 1) & 1 != 0;
                let register = Reg {
                    zero_upper: size >= 32,
                    ..view(RDX, size)
                };
                self.set(register, if negative { u64::MAX } else { 0 })?;
            }
            "bswap" => {
                let register = reg_at(ops, 0)?;
                let value = self.get(register);
                let swapped = if register.bits == 64 {
                    value.swap_bytes()
                } else {
                    u64::from((value as u32).swap_bytes())
                };
                self.set(register, swapped)?;
            }
            "xchg" => {
                let a = self.x86_read(ops.first(), bits)?;
                let b = self.x86_read(ops.get(1), bits)?;
                self.x86_write(ops.first(), bits, b)?;
                self.x86_write(ops.get(1), bits, a)?;
            }
            "bt" | "bts" | "btr" | "btc" => {
                let offset = self.x86_read(ops.get(1), bits)?;
                let (target, bit) = match (ops.first(), ops.get(1)) {
                    (Some(Operand::Mem(memory)), Some(Operand::Reg(_))) => {
                        let signed = Self::signed(offset, bits);
                        let words = signed.div_euclid(i64::from(bits));
                        let address = self
                            .x86_address(memory)?
                            .wrapping_add((words * i64::from(bits / 8)) as u64);
                        let mut memory = memory.clone();
                        memory.base = None;
                        memory.index = None;
                        memory.segment = None;
                        memory.bits = bits;
                        memory.displacement = self.address(address);
                        (
                            Operand::Mem(memory),
                            signed.rem_euclid(i64::from(bits)) as u32,
                        )
                    }
                    (Some(operand), _) => (operand.clone(), (offset % u64::from(bits)) as u32),
                    _ => return Err(operand_error(0)),
                };
                let value = self.x86_read(Some(&target), bits)?;
                self.set_flag(CF, value >> bit & 1 != 0);
                let updated = match mnemonic {
                    "bts" => value | 1 << bit,
                    "btr" => value & !(1 << bit),
                    "btc" => value ^ 1 << bit,
                    _ => return Ok(true),
                };
                self.x86_write(Some(&target), bits, updated)?;
            }
            "bsf" | "bsr" => {
                let source = self.x86_read(ops.get(1), bits)? & Self::mask(bits);
                self.set_flag(ZF, source == 0);
                if source != 0 {
                    let index = if mnemonic == "bsf" {
                        source.trailing_zeros()
                    } else {
                        63 - source.leading_zeros()
                    };
                    self.set(reg_at(ops, 0)?, u64::from(index))?;
                }
            }
            "loop" | "loope" | "loopne" => {
                let counter = view(RCX, arch_bits);
                let value = self.get(counter).wrapping_sub(1) & Self::mask(arch_bits);
                self.set(counter, value)?;
                if value != 0 && (mnemonic == "loop" || self.flag(ZF) == (mnemonic == "loope")) {
                    self.branch(imm_at(ops, 0)?)?;
                }
            }
            "jecxz" | "jrcxz" | "jcxz" => {
                let size = match mnemonic {
                    "jrcxz" => 64,
                    "jecxz" => 32,
                    _ => 16,
                };
                if self.get(view(RCX, size)) == 0 {
                    self.branch(imm_at(ops, 0)?)?;
                }
            }
            "movsb" | "movsw" | "movsd" | "movsq" | "stosb" | "stosw" | "stosd" | "stosq" => {
                return self.x86_string(decoded, ops);
            }
            "nop" | "cld" | "endbr32" | "endbr64" | "pause" => {}
            "stc" => self.set_flag(CF, true),
            "clc" => self.set_flag(CF, false),
            "cmc" => self.set_flag(CF, !self.flag(CF)),
            "int3" | "hlt" => self.halted = true,
            "int" => {
                if x64 || imm_at(ops, 0)? != 0x80 {
                    return Err(SimError(
                        "Only x86 int 0x80 supports the teaching ABI".into(),
                    ));
                }
                self.syscall()?;
            }
            "syscall" => {
                if !x64 {
                    return Err(SimError("SYSCALL teaching ABI requires x64".into()));
                }
                // SYSCALL architecturally saves RIP and RFLAGS in RCX/R11.
                self.registers[usize::from(RCX)] = self.next_pc;
                let mut flags = 2_u64;
                for (flag, bit) in [(CF, 0), (PF, 2), (AF, 4), (ZF, 6), (SF, 7), (OF, 11)] {
                    if self.flag(flag) {
                        flags |= 1 << bit;
                    }
                }
                self.registers[11] = flags;
                self.syscall()?;
            }
            _ if mnemonic.starts_with("set") => {
                let code = x86_condition_code(&mnemonic[3..])
                    .ok_or_else(|| SimError(format!("Unsupported x86 condition {mnemonic}")))?;
                let value = u64::from(self.x86_condition(code));
                self.x86_write(ops.first(), 8, value)?;
            }
            _ if mnemonic.starts_with("cmov") => {
                let code = x86_condition_code(&mnemonic[4..])
                    .ok_or_else(|| SimError(format!("Unsupported x86 condition {mnemonic}")))?;
                let destination = reg_at(ops, 0)?;
                // The source is read (and may fault) whether or not the move happens;
                // a 32-bit destination is always written, clearing bits 63:32.
                let value = self.x86_read(ops.get(1), bits)?;
                let current = self.get(destination);
                let value = if self.x86_condition(code) {
                    value
                } else {
                    current
                };
                self.set(destination, value)?;
            }
            _ if mnemonic.starts_with('j') => {
                let code = x86_condition_code(&mnemonic[1..])
                    .ok_or_else(|| SimError(format!("Unsupported x86 condition {mnemonic}")))?;
                if self.x86_condition(code) {
                    self.branch(imm_at(ops, 0)?)?;
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
