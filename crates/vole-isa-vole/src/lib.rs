//! VOLE assembler and the project's documented `simpsim-extended-v1` machine.
use std::collections::{BTreeMap, VecDeque};
use vole_core::*;

pub const PROFILE: &str = "simpsim-extended-v1";
pub const HISTORY_LIMIT: usize = 4096;

/// Decode the instruction's two bytes. Fetch wrapping is performed by the machine.
pub fn decode(address: u64, bytes: &[u8]) -> Result<Instruction, SimError> {
    if address > 255 || bytes.len() < 2 {
        return Err(SimError(
            "VOLE decode needs an 8-bit address and two bytes".into(),
        ));
    }
    let op = bytes[0] >> 4;
    let r = bytes[0] & 15;
    let s = bytes[1] >> 4;
    let t = bytes[1] & 15;
    let xy = bytes[1];
    let reg = |n| format!("R{n:X}");
    let mem = |n| format!("memory[{n:02X}h]");
    let (assembly, explanation, reads, writes) = match op {
        1 => (
            format!("load {}, [{xy:02X}h]", reg(r)),
            format!("{} = {}", reg(r), mem(xy)),
            vec![mem(xy)],
            vec![reg(r)],
        ),
        2 => (
            format!("load {}, {xy:02X}h", reg(r)),
            format!("{} = {xy:02X}h", reg(r)),
            vec![],
            vec![reg(r)],
        ),
        3 => (
            format!("store {}, [{xy:02X}h]", reg(r)),
            format!("{} = {}", mem(xy), reg(r)),
            vec![reg(r)],
            vec![mem(xy)],
        ),
        4 if r == 0 => (
            format!("move {}, {}", reg(t), reg(s)),
            format!("{} = {}", reg(t), reg(s)),
            vec![reg(s)],
            vec![reg(t)],
        ),
        5..=9 => {
            let (name, effect) = match op {
                5 => ("addi", format!("wrap8({} + {})", reg(s), reg(t))),
                6 => (
                    "addf",
                    format!("teaching_float_add({}, {})", reg(s), reg(t)),
                ),
                7 => ("or", format!("{} | {}", reg(s), reg(t))),
                8 => ("and", format!("{} & {}", reg(s), reg(t))),
                _ => ("xor", format!("{} ^ {}", reg(s), reg(t))),
            };
            (
                format!("{name} {}, {}, {}", reg(r), reg(s), reg(t)),
                format!("{} = {effect}", reg(r)),
                vec![reg(s), reg(t)],
                vec![reg(r)],
            )
        }
        10 if s == 0 => (
            format!("ror {}, {t}", reg(r)),
            format!("{} = rotate_right8({}, {t})", reg(r), reg(r)),
            vec![reg(r)],
            vec![reg(r)],
        ),
        11 => (
            if r == 0 {
                format!("jmp {xy:02X}h")
            } else {
                format!("jmpEQ {}=R0, {xy:02X}h", reg(r))
            },
            format!("if {} == R0: PC = {xy:02X}h", reg(r)),
            vec![reg(r), "R0".into()],
            vec!["PC".into()],
        ),
        12 if r == 0 && xy == 0 => ("halt".into(), "Stop execution".into(), vec![], vec![]),
        13 if r == 0 => (
            format!("load {}, [{}]", reg(s), reg(t)),
            format!("{} = memory[{}]", reg(s), reg(t)),
            vec![reg(t), format!("memory[{}]", reg(t))],
            vec![reg(s)],
        ),
        14 if r == 0 => (
            format!("store {}, [{}]", reg(s), reg(t)),
            format!("memory[{}] = {}", reg(t), reg(s)),
            vec![reg(s), reg(t)],
            vec![format!("memory[{}]", reg(t))],
        ),
        15 => (
            format!("jmpLE {}<=R0, {xy:02X}h", reg(r)),
            format!("if signed8({}) <= signed8(R0): PC = {xy:02X}h", reg(r)),
            vec![reg(r), "R0".into()],
            vec!["PC".into()],
        ),
        _ => {
            return Err(SimError(format!(
                "Invalid VOLE instruction {:02X}{:02X} at {address:02X}h: unknown opcode or nonzero reserved bits",
                bytes[0], bytes[1]
            )));
        }
    };
    Ok(Instruction {
        address,
        bytes: bytes[..2].to_vec(),
        assembly,
        explanation,
        source_line: None,
        reads,
        writes,
    })
}

#[derive(Clone)]
struct SourceLine {
    number: usize,
    address: usize,
    mnemonic: String,
    args: Vec<String>,
    data: Option<Vec<u8>>,
}

fn uncomment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if Some(ch) == quote {
            quote = None;
        } else if quote.is_none() && (ch == '"' || ch == '\'') {
            quote = Some(ch);
        } else if quote.is_none() && ch == ';' {
            return &line[..index];
        }
    }
    line
}

fn arguments(text: &str) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    for (i, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if Some(ch) == quote {
            quote = None;
        } else if quote.is_none() && (ch == '"' || ch == '\'') {
            quote = Some(ch);
        } else if quote.is_none() && ch == ',' {
            result.push(text[start..i].trim().to_owned());
            start = i + 1;
        }
    }
    if quote.is_some() {
        return Err("Unterminated string".into());
    }
    if !text.trim().is_empty() {
        result.push(text[start..].trim().to_owned());
    }
    if result.iter().any(|s| s.is_empty()) {
        return Err("Empty operand".into());
    }
    Ok(result)
}

fn number(text: &str, symbols: &BTreeMap<String, u64>) -> Result<i64, String> {
    let text = text.trim();
    if let Some(n) = symbols.get(&text.to_ascii_lowercase()) {
        return Ok(*n as i64);
    }
    for (index, ch) in text.char_indices().skip(1) {
        if ch == '+' || ch == '-' {
            let a = number(&text[..index], symbols)?;
            let b = number(&text[index + 1..], symbols)?;
            return if ch == '+' {
                a.checked_add(b)
            } else {
                a.checked_sub(b)
            }
            .ok_or_else(|| "Numeric expression overflow".into());
        }
    }
    let (sign, unsigned) = text.strip_prefix('-').map_or((1, text), |s| (-1, s));
    let unsigned = unsigned.strip_prefix('+').unwrap_or(unsigned);
    let (radix, digits) = if let Some(s) = unsigned
        .strip_prefix("0x")
        .or_else(|| unsigned.strip_prefix("0X"))
    {
        (16, s)
    } else if let Some(s) = unsigned
        .strip_suffix('h')
        .or_else(|| unsigned.strip_suffix('H'))
    {
        (16, s)
    } else {
        (10, unsigned)
    };
    i64::from_str_radix(digits, radix)
        .ok()
        .and_then(|v| v.checked_mul(sign))
        .ok_or_else(|| format!("Unknown label or invalid number '{text}'"))
}

fn byte(text: &str, symbols: &BTreeMap<String, u64>) -> Result<u8, String> {
    let n = number(text, symbols)?;
    if !(-128..=255).contains(&n) {
        Err(format!("Byte value {n} is outside -128..255"))
    } else {
        Ok(n as u8)
    }
}
fn address(text: &str, symbols: &BTreeMap<String, u64>) -> Result<u8, String> {
    let n = number(text, symbols)?;
    u8::try_from(n).map_err(|_| format!("Address {n} is outside 0..255"))
}
fn register(text: &str) -> Result<u8, String> {
    let text = text.trim();
    let suffix = text
        .strip_prefix('R')
        .or_else(|| text.strip_prefix('r'))
        .unwrap_or("");
    if suffix.len() != 1 {
        return Err(format!("Invalid register '{text}'; expected R0..RF"));
    }
    u8::from_str_radix(suffix, 16)
        .map_err(|_| format!("Invalid register '{text}'; expected R0..RF"))
}
fn string_bytes(text: &str) -> Result<Option<Vec<u8>>, String> {
    if !text.starts_with(['"', '\'']) {
        return Ok(None);
    }
    let quote = text.chars().next().unwrap();
    if text.len() < 2 || !text.ends_with(quote) {
        return Err("Unterminated string".into());
    }
    let mut bytes = Vec::new();
    let mut chars = text[1..text.len() - 1].chars();
    while let Some(ch) = chars.next() {
        let ch = if ch == '\\' {
            match chars.next() {
                Some('n') => '\n',
                Some('r') => '\r',
                Some('t') => '\t',
                Some('0') => '\0',
                Some('\\') => '\\',
                Some('"') => '"',
                Some('\'') => '\'',
                _ => return Err("Unknown string escape".into()),
            }
        } else {
            ch
        };
        if !ch.is_ascii() {
            return Err(
                "VOLE strings accept ASCII only; use db byte values for other encodings".into(),
            );
        }
        bytes.push(ch as u8);
    }
    Ok(Some(bytes))
}

fn encode(line: &SourceLine, symbols: &BTreeMap<String, u64>) -> Result<[u8; 2], String> {
    let args = &line.args;
    let expected = match line.mnemonic.as_str() {
        "halt" => 0,
        "jmp" => 1,
        "addi" | "addf" | "or" | "and" | "xor" => 3,
        _ => 2,
    };
    if args.len() != expected {
        return Err(format!("{} expects {expected} operands", line.mnemonic));
    }
    let reg = |i: usize| register(&args[i]);
    Ok(match line.mnemonic.as_str() {
        "halt" => [0xc0, 0],
        "load" | "store" => {
            let r = reg(0)?;
            let operand = &args[1];
            if let Some(inner) = operand.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                if let Ok(s) = register(inner) {
                    [
                        if line.mnemonic == "load" { 0xd0 } else { 0xe0 },
                        (r << 4) | s,
                    ]
                } else {
                    [
                        if line.mnemonic == "load" {
                            0x10 | r
                        } else {
                            0x30 | r
                        },
                        address(inner, symbols)?,
                    ]
                }
            } else if line.mnemonic == "load" {
                [0x20 | r, byte(operand, symbols)?]
            } else {
                return Err("store expects a memory operand in brackets".into());
            }
        }
        "move" | "mov" => [0x40, (reg(1)? << 4) | reg(0)?],
        "addi" | "addf" | "or" | "and" | "xor" => {
            let op = match line.mnemonic.as_str() {
                "addi" => 5,
                "addf" => 6,
                "or" => 7,
                "and" => 8,
                _ => 9,
            };
            [(op << 4) | reg(0)?, (reg(1)? << 4) | reg(2)?]
        }
        "ror" => {
            let n = number(&args[1], symbols)?;
            if !(0..=15).contains(&n) {
                return Err("Rotation count must be 0..15".into());
            }
            [0xa0 | reg(0)?, n as u8]
        }
        "jmp" => [0xb0, address(&args[0], symbols)?],
        "jmpeq" | "jmple" => {
            let operand = &args[0];
            let relation = if line.mnemonic == "jmpeq" { "=" } else { "<=" };
            let r = if let Some((left, right)) = operand.split_once(relation) {
                if !right.trim().eq_ignore_ascii_case("R0") {
                    return Err("Conditional branches compare against R0".into());
                }
                register(left)?
            } else {
                register(operand)?
            };
            [
                if line.mnemonic == "jmpeq" {
                    0xb0 | r
                } else {
                    0xf0 | r
                },
                address(&args[1], symbols)?,
            ]
        }
        _ => return Err(format!("Unknown instruction '{}'", line.mnemonic)),
    })
}

/// Assemble case-insensitive VOLE instructions and labels with source mappings.
pub fn assemble(source: &str) -> Result<Program, Vec<Diagnostic>> {
    let mut symbols = BTreeMap::new();
    let mut parsed = Vec::new();
    let mut diagnostics = Vec::new();
    let mut pc = 0usize;
    for (index, raw) in source.lines().enumerate() {
        let line_number = index + 1;
        let mut text = uncomment(raw).trim();
        if text.is_empty() {
            continue;
        }
        if let Some((label, rest)) = text
            .split_once(':')
            .filter(|(prefix, _)| !prefix.contains([' ', '\t', '"', '\'']))
        {
            if label.contains([' ', '\t', '"', '\''])
                || !label.chars().enumerate().all(|(i, c)| {
                    c == '_' || c.is_ascii_alphabetic() || i > 0 && c.is_ascii_digit()
                })
                || label.is_empty()
            {
                diagnostics.push(Diagnostic::new(line_number, "Invalid label"));
                continue;
            }
            let label = label.to_ascii_lowercase();
            if symbols.insert(label.clone(), pc as u64).is_some() {
                diagnostics.push(Diagnostic::new(
                    line_number,
                    format!("Duplicate label '{label}'"),
                ));
            }
            text = rest.trim();
            if text.is_empty() {
                continue;
            }
        }
        let split = text.find(char::is_whitespace).unwrap_or(text.len());
        let mnemonic = text[..split].trim_start_matches('.').to_ascii_lowercase();
        let args = match arguments(text[split..].trim()) {
            Ok(v) => v,
            Err(e) => {
                diagnostics.push(Diagnostic::new(line_number, e));
                continue;
            }
        };
        if mnemonic == "org" {
            if args.len() != 1 {
                diagnostics.push(Diagnostic::new(line_number, "org expects one address"));
                continue;
            }
            match address(&args[0], &symbols) {
                Ok(n) => pc = n as usize,
                Err(e) => diagnostics.push(Diagnostic::new(line_number, e)),
            };
            continue;
        }
        let data = if mnemonic == "db" {
            if args.is_empty() {
                diagnostics.push(Diagnostic::new(
                    line_number,
                    "db expects at least one byte or string",
                ));
                continue;
            }
            let mut bytes = Vec::new();
            let mut valid = true;
            for arg in &args {
                match string_bytes(arg) {
                    Ok(Some(value)) => bytes.extend(value),
                    Ok(None) => bytes.push(0),
                    Err(e) => {
                        diagnostics.push(Diagnostic::new(line_number, e));
                        valid = false;
                    }
                }
            }
            if !valid {
                continue;
            }
            Some(bytes)
        } else {
            None
        };
        let size = data.as_ref().map_or(2, Vec::len);
        if pc.checked_add(size).is_none_or(|end| end > 256) {
            diagnostics.push(Diagnostic::new(
                line_number,
                "Emission exceeds VOLE's 256-byte memory",
            ));
            continue;
        }
        parsed.push(SourceLine {
            number: line_number,
            address: pc,
            mnemonic,
            args,
            data,
        });
        pc += size;
    }
    let mut memory = [None; 256];
    let mut instructions = Vec::new();
    let mut entry = None;
    for line in parsed {
        let bytes = if line.data.is_some() {
            let mut bytes = Vec::new();
            let mut valid = true;
            for arg in &line.args {
                match string_bytes(arg) {
                    Ok(Some(s)) => bytes.extend(s),
                    Ok(None) => match byte(arg, &symbols) {
                        Ok(b) => bytes.push(b),
                        Err(e) => {
                            diagnostics.push(Diagnostic::new(line.number, e));
                            valid = false;
                        }
                    },
                    Err(e) => {
                        diagnostics.push(Diagnostic::new(line.number, e));
                        valid = false;
                    }
                }
            }
            if !valid {
                continue;
            }
            bytes
        } else {
            match encode(&line, &symbols) {
                Ok(bytes) => {
                    let mut ins = decode(line.address as u64, &bytes)
                        .expect("encoder emits valid instructions");
                    ins.source_line = Some(line.number);
                    instructions.push(ins);
                    entry.get_or_insert(line.address as u64);
                    bytes.to_vec()
                }
                Err(e) => {
                    diagnostics.push(Diagnostic::new(line.number, e));
                    continue;
                }
            }
        };
        if bytes
            .iter()
            .enumerate()
            .any(|(offset, _)| memory[line.address + offset].is_some())
        {
            diagnostics.push(Diagnostic::new(
                line.number,
                "Origin overlaps previously emitted bytes",
            ));
            continue;
        }
        for (offset, b) in bytes.into_iter().enumerate() {
            memory[line.address + offset] = Some(b);
        }
    }
    if !diagnostics.is_empty() {
        return Err(diagnostics);
    }
    let mut regions = Vec::new();
    let mut cursor = 0;
    while cursor < 256 {
        if memory[cursor].is_none() {
            cursor += 1;
            continue;
        }
        let base = cursor;
        let mut bytes = Vec::new();
        while cursor < 256 {
            if let Some(b) = memory[cursor] {
                bytes.push(b);
                cursor += 1;
            } else {
                break;
            }
        }
        regions.push(MemoryRegion {
            base: base as u64,
            bytes,
            writable: true,
            executable: true,
            label: "VOLE program".into(),
        });
    }
    if regions.is_empty() {
        return Err(vec![Diagnostic::new(1, "Program is empty")]);
    }
    Ok(Program {
        architecture: Architecture::Vole,
        source: source.into(),
        entry: entry.unwrap_or(regions[0].base),
        regions,
        instructions,
        symbols,
        initial_registers: BTreeMap::new(),
        language: Default::default(),
        debug: None,
    })
}

#[derive(Clone)]
struct SavedState {
    registers: [u8; 16],
    memory: [u8; 256],
    pc: u8,
    halted: bool,
    steps: u64,
    output_len: usize,
}

/// Independent, byte-addressed VOLE engine. Successful steps can be reversed.
pub struct VoleMachine {
    registers: [u8; 16],
    memory: [u8; 256],
    pc: u8,
    halted: bool,
    steps: u64,
    output: Vec<u8>,
    history: VecDeque<SavedState>,
    trace: VecDeque<StepRecord>,
    initial: Option<SavedState>,
    source_instructions: BTreeMap<u64, Instruction>,
}

impl Default for VoleMachine {
    fn default() -> Self {
        Self::new()
    }
}
impl VoleMachine {
    pub fn new() -> Self {
        Self {
            registers: [0; 16],
            memory: [0; 256],
            pc: 0,
            halted: false,
            steps: 0,
            output: Vec::new(),
            history: VecDeque::new(),
            trace: VecDeque::new(),
            initial: None,
            source_instructions: BTreeMap::new(),
        }
    }
    fn save(&self) -> SavedState {
        SavedState {
            registers: self.registers,
            memory: self.memory,
            pc: self.pc,
            halted: self.halted,
            steps: self.steps,
            output_len: self.output.len(),
        }
    }
    fn restore(&mut self, saved: SavedState) {
        self.registers = saved.registers;
        self.memory = saved.memory;
        self.pc = saved.pc;
        self.halted = saved.halted;
        self.steps = saved.steps;
        self.output.truncate(saved.output_len);
    }
    fn clear_history(&mut self) {
        self.history.clear();
        self.trace.clear();
    }
    pub fn current_instruction(&self) -> Result<Instruction, SimError> {
        let bytes = [
            self.memory[self.pc as usize],
            self.memory[self.pc.wrapping_add(1) as usize],
        ];
        let mut ins = decode(self.pc as u64, &bytes)?;
        if let Some(original) = self
            .source_instructions
            .get(&(self.pc as u64))
            .filter(|ins| ins.bytes == bytes)
        {
            ins.source_line = original.source_line;
        }
        Ok(ins)
    }
}

// An exact fixed-point representation avoids host floating-point rounding.
// In units of 1/256, byte 0SEEEMMMM has magnitude MMMM * 2^EEE.
fn float_add(a: u8, b: u8) -> Result<u8, SimError> {
    let units = |byte: u8| {
        let magnitude = ((byte & 15) as i32) << ((byte >> 4) & 7);
        if byte & 128 != 0 {
            -magnitude
        } else {
            magnitude
        }
    };
    let sum = units(a) + units(b);
    let magnitude = sum.unsigned_abs();
    if magnitude > 1920 {
        return Err(SimError(
            "VOLE teaching-float overflow: result exceeds +/-7.5".into(),
        ));
    }
    if magnitude == 0 {
        return Ok(0);
    }
    let mut exponent = 0;
    while magnitude >> exponent > 15 {
        exponent += 1;
    }
    Ok(if sum < 0 { 128 } else { 0 } | (exponent << 4) as u8 | (magnitude >> exponent) as u8)
}

impl Machine for VoleMachine {
    fn load(&mut self, program: &Program) -> Result<(), SimError> {
        if program.architecture != Architecture::Vole {
            return Err(SimError(
                "VOLE machine cannot load a different architecture".into(),
            ));
        }
        let pc = u8::try_from(program.entry)
            .map_err(|_| SimError("VOLE entry is outside 0..255".into()))?;
        let mut memory = [0; 256];
        let mut occupied = [false; 256];
        for region in &program.regions {
            let base = usize::try_from(region.base)
                .map_err(|_| SimError("VOLE region address is outside memory".into()))?;
            let end = base
                .checked_add(region.bytes.len())
                .filter(|end| *end <= 256)
                .ok_or_else(|| SimError("VOLE region exceeds 256-byte memory".into()))?;
            if occupied[base..end].iter().any(|value| *value) {
                return Err(SimError("VOLE program regions overlap".into()));
            }
            memory[base..end].copy_from_slice(&region.bytes);
            occupied[base..end].fill(true);
        }
        let mut registers = [0; 16];
        for (name, value) in &program.initial_registers {
            let index = register(name).map_err(SimError)? as usize;
            registers[index] = u8::try_from(*value)
                .map_err(|_| SimError(format!("Initial {name} exceeds 8 bits")))?;
        }
        self.memory = memory;
        self.registers = registers;
        self.pc = pc;
        self.halted = false;
        self.steps = 0;
        self.output.clear();
        self.clear_history();
        self.source_instructions = program
            .instructions
            .iter()
            .map(|ins| (ins.address, ins.clone()))
            .collect();
        self.initial = Some(self.save());
        Ok(())
    }
    fn step(&mut self) -> Result<StepRecord, SimError> {
        if self.initial.is_none() {
            return Err(SimError("Load a program before stepping".into()));
        }
        if self.halted {
            return Err(SimError(
                "Machine is halted; reset or edit PC to continue".into(),
            ));
        }
        let instruction = self.current_instruction()?;
        let next_steps = self
            .steps
            .checked_add(1)
            .ok_or_else(|| SimError("Instruction counter overflow".into()))?;
        let [first, second] = [instruction.bytes[0], instruction.bytes[1]];
        let op = first >> 4;
        let r = (first & 15) as usize;
        let s = (second >> 4) as usize;
        let t = (second & 15) as usize;
        // Resolve arithmetic that can fail before modifying state or device output.
        let float_result = if op == 6 {
            Some(float_add(self.registers[s], self.registers[t])?)
        } else {
            None
        };
        let saved = self.save();
        let mut register_write = None;
        let mut memory_write = None;
        let mut memory_reads = Vec::new();
        let mut next_pc = self.pc.wrapping_add(2);
        let mut halted = false;
        match op {
            1 => {
                register_write = Some((r, self.memory[second as usize]));
                memory_reads.push(MemoryAccess {
                    address: second as u64,
                    length: 1,
                });
            }
            2 => register_write = Some((r, second)),
            3 => memory_write = Some((second as usize, self.registers[r])),
            4 => register_write = Some((t, self.registers[s])),
            5 => register_write = Some((r, self.registers[s].wrapping_add(self.registers[t]))),
            6 => register_write = Some((r, float_result.unwrap())),
            7 => register_write = Some((r, self.registers[s] | self.registers[t])),
            8 => register_write = Some((r, self.registers[s] & self.registers[t])),
            9 => register_write = Some((r, self.registers[s] ^ self.registers[t])),
            10 => register_write = Some((r, self.registers[r].rotate_right(t as u32))),
            11 => {
                if self.registers[r] == self.registers[0] {
                    next_pc = second;
                }
            }
            12 => halted = true,
            13 => {
                let address = self.registers[t];
                register_write = Some((s, self.memory[address as usize]));
                memory_reads.push(MemoryAccess {
                    address: address as u64,
                    length: 1,
                });
            }
            14 => memory_write = Some((self.registers[t] as usize, self.registers[s])),
            15 => {
                if self.registers[r] as i8 <= self.registers[0] as i8 {
                    next_pc = second;
                }
            }
            _ => unreachable!("decoder rejected unknown opcodes"),
        }
        if register_write.is_some_and(|(register, _)| register == 15)
            && self.output.len() >= 1024 * 1024
        {
            return Err(SimError("Simulated output reached its 1 MiB limit".into()));
        }
        let mut register_changes = Vec::new();
        let mut memory_changes = Vec::new();
        let mut output_added = Vec::new();
        if let Some((index, value)) = register_write {
            let before = self.registers[index];
            self.registers[index] = value;
            if before != value {
                register_changes.push(RegisterChange {
                    name: format!("R{index:X}"),
                    before: before as u64,
                    after: value as u64,
                });
            }
            if index == 15 {
                self.output.push(value);
                output_added.push(value);
            }
        }
        if let Some((address, value)) = memory_write {
            let before = self.memory[address];
            self.memory[address] = value;
            memory_changes.push(MemoryChange {
                address: address as u64,
                before: vec![before],
                after: vec![value],
            });
        }
        self.pc = next_pc;
        self.halted = halted;
        self.steps = next_steps;
        let record = StepRecord {
            pc_before: saved.pc as u64,
            pc_after: self.pc as u64,
            instruction,
            registers: register_changes,
            memory: memory_changes,
            memory_reads,
            output_added,
            halted,
        };
        self.history.push_back(saved);
        self.trace.push_back(record.clone());
        if self.history.len() > HISTORY_LIMIT {
            self.history.pop_front();
            self.trace.pop_front();
        }
        Ok(record)
    }
    fn reverse_step(&mut self) -> Result<(), SimError> {
        let saved = self
            .history
            .pop_back()
            .ok_or_else(|| SimError("No retained step to reverse".into()))?;
        self.restore(saved);
        self.trace.pop_back();
        Ok(())
    }
    fn reset(&mut self) -> Result<(), SimError> {
        let initial = self
            .initial
            .clone()
            .ok_or_else(|| SimError("Load a program before resetting".into()))?;
        self.restore(initial);
        self.clear_history();
        Ok(())
    }
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            architecture: Architecture::Vole,
            pc: self.pc as u64,
            registers: self
                .registers
                .iter()
                .enumerate()
                .map(|(i, value)| RegisterValue {
                    name: format!("R{i:X}"),
                    value: *value as u64,
                    bits: 8,
                })
                .collect(),
            flags: BTreeMap::new(),
            memory: vec![MemoryRegion {
                base: 0,
                bytes: self.memory.to_vec(),
                writable: true,
                executable: true,
                label: "Main memory".into(),
            }],
            output: self.output.clone(),
            halted: self.halted,
            steps: self.steps,
            trace: self.trace.iter().cloned().collect(),
        }
    }
    fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), SimError> {
        if self.initial.is_none() {
            return Err(SimError(
                "Load a program before restoring saved state".into(),
            ));
        }
        if snapshot.architecture != Architecture::Vole
            || snapshot.pc > 255
            || snapshot.registers.len() != 16
            || !snapshot.flags.is_empty()
            || snapshot.output.len() > 1024 * 1024
            || snapshot.memory.len() != 1
        {
            return Err(SimError(
                "Saved state is invalid for the VOLE machine".into(),
            ));
        }
        let region = &snapshot.memory[0];
        if region.base != 0 || region.bytes.len() != 256 || !region.writable || !region.executable {
            return Err(SimError(
                "Saved VOLE memory must contain the 256-byte main memory".into(),
            ));
        }
        let mut registers = [0; 16];
        let mut seen = [false; 16];
        for saved in &snapshot.registers {
            let index = register(&saved.name).map_err(SimError)? as usize;
            if saved.bits != 8 || saved.value > 255 || seen[index] {
                return Err(SimError(
                    "Saved VOLE registers contain invalid or duplicate values".into(),
                ));
            }
            registers[index] = saved.value as u8;
            seen[index] = true;
        }
        self.registers = registers;
        self.memory.copy_from_slice(&region.bytes);
        self.pc = snapshot.pc as u8;
        self.output = snapshot.output.clone();
        self.halted = snapshot.halted;
        self.steps = snapshot.steps;
        self.clear_history();
        Ok(())
    }
    fn write_register(&mut self, name: &str, value: u64) -> Result<(), SimError> {
        let value = u8::try_from(value)
            .map_err(|_| SimError("VOLE register values must fit in 8 bits".into()))?;
        if name.eq_ignore_ascii_case("PC") {
            self.pc = value;
            self.halted = false;
        } else {
            let index = register(name).map_err(SimError)? as usize;
            self.registers[index] = value;
        }
        self.clear_history();
        Ok(())
    }
    fn write_memory(&mut self, address: u64, bytes: &[u8]) -> Result<(), SimError> {
        let base = usize::try_from(address)
            .map_err(|_| SimError("Memory address is outside VOLE memory".into()))?;
        let end = base
            .checked_add(bytes.len())
            .filter(|end| *end <= 256)
            .ok_or_else(|| SimError("Memory edit exceeds VOLE memory".into()))?;
        self.memory[base..end].copy_from_slice(bytes);
        self.clear_history();
        Ok(())
    }
}
