//! Parsing of Capstone's Intel/ARM operand text into typed operands.
//! Each decoded address is parsed once and cached; execution never re-parses text.
use vole_core::Architecture;

/// Pseudo register indexes outside the general register file.
pub(super) const ZR: u8 = 0xfe;
pub(super) const PC: u8 = 0xff;

/// A register view: storage index, width, bit offset (x86 AH..DH) and whether
/// a write clears the bits above the view (ARM, x86 32/64-bit writes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Reg {
    pub index: u8,
    pub bits: u8,
    pub offset: u8,
    pub zero_upper: bool,
}

impl Reg {
    const fn new(index: u8, bits: u8) -> Self {
        Self {
            index,
            bits,
            offset: 0,
            zero_upper: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shift {
    Lsl,
    Lsr,
    Asr,
    Ror,
    Rrx,
    Uxtb,
    Uxth,
    Uxtw,
    Uxtx,
    Sxtb,
    Sxth,
    Sxtw,
    Sxtx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Amount {
    Imm(u32),
    Reg(Reg),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Modifier {
    pub shift: Shift,
    pub amount: Amount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Mem {
    /// x86 `byte/word/dword/qword ptr` width in bits, or 0.
    pub bits: u8,
    pub base: Option<Reg>,
    pub index: Option<Reg>,
    /// x86 index scale.
    pub scale: u64,
    /// ARM32 `[rn, -rm]`.
    pub subtract_index: bool,
    /// ARM index shift/extend such as `lsl #3` or `sxtw #2`.
    pub modifier: Option<Modifier>,
    pub displacement: u64,
    /// ARM pre-index writeback `]!`.
    pub writeback: bool,
    /// An explicit x86 segment such as `fs:`.
    pub segment: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Operand {
    Reg(Reg),
    /// ARM32 post-index register subtracted from the base (`-r2`).
    NegReg(Reg),
    /// ARM32 load/store-multiple base with writeback (`r0!`).
    RegBang(Reg),
    Imm(u64),
    Mem(Box<Mem>),
    Modifier(Modifier),
    List(Vec<Reg>),
    /// ARM condition code in the A64 encoding order (eq = 0 ... nv = 15).
    Cond(u8),
    Name(String),
}

pub(super) const ARM_CONDITIONS: [&str; 16] = [
    "eq", "ne", "hs", "lo", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le", "al", "nv",
];

pub(super) fn arm_condition_code(text: &str) -> Option<u8> {
    match text {
        "cs" => Some(2),
        "cc" => Some(3),
        _ => ARM_CONDITIONS
            .iter()
            .position(|name| *name == text)
            .map(|index| index as u8),
    }
}

/// x86 condition codes in the Intel `tttn` encoding order.
pub(super) fn x86_condition_code(text: &str) -> Option<u8> {
    Some(match text {
        "o" => 0,
        "no" => 1,
        "b" | "c" | "nae" => 2,
        "ae" | "nb" | "nc" => 3,
        "e" | "z" => 4,
        "ne" | "nz" => 5,
        "be" | "na" => 6,
        "a" | "nbe" => 7,
        "s" => 8,
        "ns" => 9,
        "p" | "pe" => 10,
        "np" | "po" => 11,
        "l" | "nge" => 12,
        "ge" | "nl" => 13,
        "le" | "ng" => 14,
        "g" | "nle" => 15,
        _ => return None,
    })
}

const X86_NAMES: [(&str, &str, &str, &str, &str); 8] = [
    ("rax", "eax", "ax", "al", "ah"),
    ("rcx", "ecx", "cx", "cl", "ch"),
    ("rdx", "edx", "dx", "dl", "dh"),
    ("rbx", "ebx", "bx", "bl", "bh"),
    ("rsp", "esp", "sp", "spl", ""),
    ("rbp", "ebp", "bp", "bpl", ""),
    ("rsi", "esi", "si", "sil", ""),
    ("rdi", "edi", "di", "dil", ""),
];

/// Storage names, indexed by register number.
pub(super) fn storage_names(architecture: Architecture) -> Vec<String> {
    match architecture {
        Architecture::Arm32 => (0..15).map(|n| format!("r{n}")).collect(),
        Architecture::Arm64 => (0..31)
            .map(|n| format!("x{n}"))
            .chain(std::iter::once("sp".to_string()))
            .collect(),
        Architecture::X86 => X86_NAMES.iter().map(|n| n.1.to_string()).collect(),
        Architecture::X64 => X86_NAMES
            .iter()
            .map(|n| n.0.to_string())
            .chain((8..16).map(|n| format!("r{n}")))
            .collect(),
        Architecture::Vole => vec![],
    }
}

pub(super) fn parse_register(architecture: Architecture, name: &str) -> Option<Reg> {
    let lower;
    let name = if name.bytes().any(|b| b.is_ascii_uppercase()) {
        lower = name.to_ascii_lowercase();
        lower.as_str()
    } else {
        name
    };
    let number = |prefix: &str, limit: u8| {
        name.strip_prefix(prefix)
            .filter(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|digits| digits.parse::<u8>().ok())
            .filter(|n| *n < limit)
    };
    match architecture {
        Architecture::Arm32 => {
            let index = match name {
                "sp" => 13,
                "lr" => 14,
                "pc" => 15,
                "fp" => 11,
                "ip" => 12,
                "sb" => 9,
                "sl" => 10,
                _ => number("r", 16)?,
            };
            Some(if index == 15 {
                Reg::new(PC, 32)
            } else {
                Reg::new(index, 32)
            })
        }
        Architecture::Arm64 => match name {
            "xzr" => Some(Reg::new(ZR, 64)),
            "wzr" => Some(Reg::new(ZR, 32)),
            "sp" => Some(Reg::new(31, 64)),
            "wsp" => Some(Reg::new(31, 32)),
            "fp" => Some(Reg::new(29, 64)),
            "lr" => Some(Reg::new(30, 64)),
            "pc" => Some(Reg::new(PC, 64)),
            _ => number("x", 31)
                .map(|n| Reg::new(n, 64))
                .or_else(|| number("w", 31).map(|n| Reg::new(n, 32))),
        },
        Architecture::X86 | Architecture::X64 => {
            let x64 = architecture == Architecture::X64;
            if name == "rip" && x64 || name == "eip" && !x64 {
                return Some(Reg::new(PC, architecture.bits()));
            }
            for (index, (quad, dword, word, low, high)) in X86_NAMES.iter().enumerate() {
                let index = index as u8;
                if name == *quad && x64 {
                    return Some(Reg::new(index, 64));
                }
                if name == *dword {
                    return Some(Reg::new(index, 32));
                }
                let partial = |bits, offset| Reg {
                    index,
                    bits,
                    offset,
                    zero_upper: false,
                };
                if name == *word {
                    return Some(partial(16, 0));
                }
                if name == *low && (x64 || !high.is_empty()) {
                    return Some(partial(8, 0));
                }
                if !high.is_empty() && name == *high {
                    return Some(partial(8, 8));
                }
            }
            if x64 && let Some(rest) = name.strip_prefix('r') {
                let digits = rest.trim_end_matches(['d', 'w', 'b']);
                let n = digits.parse::<u8>().ok().filter(|n| (8..16).contains(n))?;
                return match &rest[digits.len()..] {
                    "" => Some(Reg::new(n, 64)),
                    "d" => Some(Reg::new(n, 32)),
                    "w" => Some(Reg {
                        index: n,
                        bits: 16,
                        offset: 0,
                        zero_upper: false,
                    }),
                    "b" => Some(Reg {
                        index: n,
                        bits: 8,
                        offset: 0,
                        zero_upper: false,
                    }),
                    _ => None,
                };
            }
            None
        }
        Architecture::Vole => None,
    }
}

pub(super) fn parse_immediate(text: &str) -> Option<u64> {
    let text = text.trim();
    let text = text.strip_prefix('#').unwrap_or(text);
    if let Some(positive) = text.strip_prefix('-') {
        return parse_immediate(positive).map(u64::wrapping_neg);
    }
    if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        text.parse::<u64>().ok()
    } else {
        None
    }
}

pub(super) fn split_operands(text: &str) -> Vec<&str> {
    if text.trim().is_empty() {
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
                operands.push(text[start..offset].trim());
                start = offset + 1;
            }
            _ => {}
        }
    }
    operands.push(text[start..].trim());
    operands
}

fn parse_shift_kind(text: &str) -> Option<Shift> {
    Some(match text {
        "lsl" => Shift::Lsl,
        "lsr" => Shift::Lsr,
        "asr" => Shift::Asr,
        "ror" => Shift::Ror,
        "rrx" => Shift::Rrx,
        "uxtb" => Shift::Uxtb,
        "uxth" => Shift::Uxth,
        "uxtw" => Shift::Uxtw,
        "uxtx" => Shift::Uxtx,
        "sxtb" => Shift::Sxtb,
        "sxth" => Shift::Sxth,
        "sxtw" => Shift::Sxtw,
        "sxtx" => Shift::Sxtx,
        _ => return None,
    })
}

fn parse_modifier(architecture: Architecture, text: &str) -> Option<Modifier> {
    let (kind, amount) = match text.split_once(' ') {
        Some((kind, amount)) => (kind, Some(amount.trim())),
        None => (text, None),
    };
    let shift = parse_shift_kind(kind)?;
    let amount = match amount {
        None => Amount::Imm(0),
        Some(amount) => {
            if let Some(value) = parse_immediate(amount) {
                Amount::Imm(u32::try_from(value).ok()?)
            } else {
                Amount::Reg(parse_register(architecture, amount)?)
            }
        }
    };
    Some(Modifier { shift, amount })
}

fn parse_arm_memory(architecture: Architecture, text: &str) -> Result<Mem, String> {
    let open = text.find('[').ok_or("Expected an ARM memory operand")?;
    let close = text.rfind(']').ok_or("Malformed ARM memory operand")?;
    let parts = split_operands(&text[open + 1..close]);
    let base = parts
        .first()
        .and_then(|name| parse_register(architecture, name))
        .ok_or_else(|| format!("Unsupported ARM address base in {text}"))?;
    let mut memory = Mem {
        bits: 0,
        base: Some(base),
        index: None,
        scale: 1,
        subtract_index: false,
        modifier: None,
        displacement: 0,
        writeback: text[close..].contains('!'),
        segment: None,
    };
    if let Some(offset) = parts.get(1) {
        if let Some(value) = parse_immediate(offset) {
            memory.displacement = value;
        } else {
            let (negative, name) = match offset.strip_prefix('-') {
                Some(name) => (true, name),
                None => (false, offset.strip_prefix('+').unwrap_or(offset)),
            };
            memory.index = Some(
                parse_register(architecture, name)
                    .ok_or_else(|| format!("Unsupported ARM address index in {text}"))?,
            );
            memory.subtract_index = negative;
        }
    }
    if let Some(modifier) = parts.get(2) {
        memory.modifier = Some(
            parse_modifier(architecture, modifier)
                .ok_or_else(|| format!("Unsupported ARM address modifier in {text}"))?,
        );
    }
    if parts.len() > 3 {
        return Err(format!("Unsupported ARM memory operand {text}"));
    }
    Ok(memory)
}

fn parse_x86_memory(architecture: Architecture, text: &str) -> Result<Mem, String> {
    let open = text.find('[').ok_or("Expected an x86 memory operand")?;
    let close = text.rfind(']').ok_or("Malformed x86 memory operand")?;
    let mut prefix = text[..open].trim();
    let mut segment = None;
    if let Some(stripped) = prefix.strip_suffix(':') {
        let (rest, name) = stripped.rsplit_once(' ').unwrap_or(("", stripped));
        segment = Some(name.to_string());
        prefix = rest.trim();
    }
    let bits = match prefix {
        "byte ptr" => 8,
        "word ptr" => 16,
        "dword ptr" => 32,
        "qword ptr" => 64,
        "" => 0,
        other => return Err(format!("Unsupported x86 memory size {other}")),
    };
    let mut memory = Mem {
        bits,
        base: None,
        index: None,
        scale: 1,
        subtract_index: false,
        modifier: None,
        displacement: 0,
        writeback: false,
        segment,
    };
    let expression: String = text[open + 1..close]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let mut negative = false;
    let mut term = String::new();
    for character in expression.chars().chain(std::iter::once('+')) {
        if matches!(character, '+' | '-') && !term.is_empty() {
            if let Some((register, scale)) = term.split_once('*') {
                let register = parse_register(architecture, register)
                    .ok_or_else(|| format!("Unsupported x86 index in {text}"))?;
                let scale = parse_immediate(scale)
                    .filter(|s| matches!(s, 1 | 2 | 4 | 8))
                    .ok_or_else(|| format!("Unsupported x86 scale in {text}"))?;
                if negative || memory.index.is_some() {
                    return Err(format!("Unsupported x86 address {text}"));
                }
                memory.index = Some(register);
                memory.scale = scale;
            } else if let Some(register) = parse_register(architecture, &term) {
                if negative {
                    return Err(format!("Unsupported x86 address {text}"));
                }
                if memory.base.is_none() {
                    memory.base = Some(register);
                } else if memory.index.is_none() {
                    memory.index = Some(register);
                } else {
                    return Err(format!("Unsupported x86 address {text}"));
                }
            } else {
                let value = parse_immediate(&term)
                    .ok_or_else(|| format!("Unsupported x86 displacement in {text}"))?;
                memory.displacement = if negative {
                    memory.displacement.wrapping_sub(value)
                } else {
                    memory.displacement.wrapping_add(value)
                };
            }
            term.clear();
            negative = character == '-';
        } else if matches!(character, '+' | '-') {
            negative = character == '-';
        } else {
            term.push(character);
        }
    }
    Ok(memory)
}

fn parse_list(architecture: Architecture, text: &str) -> Result<Vec<Reg>, String> {
    let inner = text
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .ok_or("Expected an ARM register list")?;
    let mut registers = vec![];
    for item in split_operands(inner) {
        if let Some((first, last)) = item.split_once('-') {
            let first = parse_register(architecture, first.trim());
            let last = parse_register(architecture, last.trim());
            let (Some(first), Some(last)) = (first, last) else {
                return Err("Invalid register range".into());
            };
            let number = |r: Reg| if r.index == PC { 15 } else { r.index };
            let (first, last) = (number(first), number(last));
            if first > last {
                return Err("Invalid register range".into());
            }
            for n in first..=last {
                registers.push(if n == 15 {
                    Reg::new(PC, 32)
                } else {
                    Reg::new(n, 32)
                });
            }
        } else {
            registers.push(
                parse_register(architecture, item)
                    .ok_or_else(|| format!("Invalid register {item} in list"))?,
            );
        }
    }
    Ok(registers)
}

pub(super) fn parse_operand(architecture: Architecture, text: &str) -> Result<Operand, String> {
    let arm = matches!(architecture, Architecture::Arm32 | Architecture::Arm64);
    if text.starts_with('{') {
        return parse_list(architecture, text).map(Operand::List);
    }
    if text.contains('[') {
        return if arm {
            parse_arm_memory(architecture, text)
        } else {
            parse_x86_memory(architecture, text)
        }
        .map(|m| Operand::Mem(Box::new(m)));
    }
    if let Some(register) = parse_register(architecture, text) {
        return Ok(Operand::Reg(register));
    }
    if let Some(value) = parse_immediate(text) {
        return Ok(Operand::Imm(value));
    }
    if arm {
        if let Some(name) = text.strip_suffix('!')
            && let Some(register) = parse_register(architecture, name)
        {
            return Ok(Operand::RegBang(register));
        }
        if let Some(name) = text.strip_prefix('-')
            && let Some(register) = parse_register(architecture, name)
        {
            return Ok(Operand::NegReg(register));
        }
        if let Some(modifier) = parse_modifier(architecture, text) {
            return Ok(Operand::Modifier(modifier));
        }
        if architecture == Architecture::Arm64
            && let Some(code) = arm_condition_code(text)
        {
            return Ok(Operand::Cond(code));
        }
    }
    Ok(Operand::Name(text.to_ascii_lowercase()))
}
