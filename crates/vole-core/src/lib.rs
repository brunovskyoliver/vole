//! Shared, UI-independent machine images and debugger observations.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Architecture {
    #[default]
    Vole,
    Arm32,
    Arm64,
    X86,
    X64,
}

impl Architecture {
    pub const ALL: [Self; 5] = [Self::Vole, Self::Arm32, Self::Arm64, Self::X86, Self::X64];

    pub fn name(self) -> &'static str {
        match self {
            Self::Vole => "VOLE 8-bit",
            Self::Arm32 => "ARM 32-bit",
            Self::Arm64 => "ARM64 / AArch64",
            Self::X86 => "x86 32-bit",
            Self::X64 => "x64 / x86-64",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Vole => "vole",
            Self::Arm32 => "arm32",
            Self::Arm64 => "arm64",
            Self::X86 => "x86",
            Self::X64 => "x64",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.to_lowercase().as_str() {
            "vole" | "8" => Some(Self::Vole),
            "arm" | "arm32" | "aarch32" => Some(Self::Arm32),
            "arm64" | "aarch64" => Some(Self::Arm64),
            "x86" | "i386" | "ia32" => Some(Self::X86),
            "x64" | "x86-64" | "x86_64" => Some(Self::X64),
            _ => None,
        }
    }

    pub fn bits(self) -> u8 {
        match self {
            Self::Vole => 8,
            Self::Arm32 | Self::X86 => 32,
            Self::Arm64 | Self::X64 => 64,
        }
    }

    pub fn address_digits(self) -> usize {
        if self == Self::Vole {
            2
        } else if self.bits() == 32 {
            8
        } else {
            16
        }
    }

    pub fn family(self) -> &'static str {
        match self {
            Self::Vole => "Teaching machine",
            Self::Arm32 | Self::Arm64 => "RISC",
            Self::X86 | Self::X64 => "CISC",
        }
    }

    pub fn example_source(self) -> &'static str {
        match self {
            Self::Vole => "load R1, 3Ah\nload R2, 43h\naddi R3, R1, R2\nstore R3, [0BBh]\nhalt\n",
            Self::Arm32 => {
                ".syntax unified\n.text\n.global _start\n_start:\n    mov r1, #58\n    mov r2, #67\n    add r3, r1, r2\n    mov r4, #8192\n    str r3, [r4]\n    bkpt #0\n"
            }
            Self::Arm64 => {
                ".text\n.global _start\n_start:\n    mov x1, #58\n    mov x2, #67\n    add x3, x1, x2\n    mov x4, #8192\n    str x3, [x4]\n    brk #0\n"
            }
            Self::X86 => {
                ".intel_syntax noprefix\n.text\n.global _start\n_start:\n    mov eax, 58\n    mov ebx, 67\n    add eax, ebx\n    mov dword ptr [8192], eax\n    int3\n"
            }
            Self::X64 => {
                ".intel_syntax noprefix\n.text\n.global _start\n_start:\n    mov rax, 58\n    mov rbx, 67\n    add rax, rbx\n    mov qword ptr [8192], rax\n    int3\n"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl Diagnostic {
    pub fn new(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            column: 1,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRegion {
    pub base: u64,
    pub bytes: Vec<u8>,
    pub writable: bool,
    pub executable: bool,
    pub label: String,
}

impl MemoryRegion {
    pub fn contains(&self, address: u64) -> bool {
        address
            .checked_sub(self.base)
            .is_some_and(|offset| offset < self.bytes.len() as u64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Instruction {
    pub address: u64,
    pub bytes: Vec<u8>,
    pub assembly: String,
    pub explanation: String,
    pub source_line: Option<usize>,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Program {
    pub architecture: Architecture,
    pub source: String,
    pub entry: u64,
    pub regions: Vec<MemoryRegion>,
    pub instructions: Vec<Instruction>,
    pub symbols: BTreeMap<String, u64>,
    pub initial_registers: BTreeMap<String, u64>,
}

impl Program {
    pub fn instruction(&self, address: u64) -> Option<&Instruction> {
        self.instructions
            .iter()
            .find(|instruction| instruction.address == address)
    }

    pub fn byte_count(&self) -> usize {
        self.regions.iter().map(|region| region.bytes.len()).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterValue {
    pub name: String,
    pub value: u64,
    pub bits: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterChange {
    pub name: String,
    pub before: u64,
    pub after: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryChange {
    pub address: u64,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryAccess {
    pub address: u64,
    pub length: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRecord {
    pub pc_before: u64,
    pub pc_after: u64,
    pub instruction: Instruction,
    pub registers: Vec<RegisterChange>,
    pub memory: Vec<MemoryChange>,
    pub memory_reads: Vec<MemoryAccess>,
    pub output_added: Vec<u8>,
    pub halted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub architecture: Architecture,
    pub pc: u64,
    pub registers: Vec<RegisterValue>,
    pub flags: BTreeMap<String, bool>,
    pub memory: Vec<MemoryRegion>,
    pub output: Vec<u8>,
    pub halted: bool,
    pub steps: u64,
    pub trace: Vec<StepRecord>,
}

impl Snapshot {
    /// Read a register or its architectural alias without changing machine state.
    /// Alias reads expose the stored bits; ARM's PC + 8 operand rule belongs to execution.
    pub fn register(&self, name: &str) -> Option<u64> {
        let name = name.trim().to_ascii_lowercase();
        match self.architecture {
            Architecture::Arm32 => {
                let canonical = match name.as_str() {
                    "sp" => "r13",
                    "lr" => "r14",
                    "fp" => "r11",
                    "ip" => "r12",
                    "sb" => "r9",
                    "sl" => "r10",
                    "r15" => "pc",
                    _ => &name,
                };
                self.register_part(canonical, 32, 0)
            }
            Architecture::Arm64 => {
                match name.as_str() {
                    "xzr" | "wzr" => return Some(0),
                    "wsp" => return self.register_part("sp", 32, 0),
                    "fp" => return self.register_part("x29", 64, 0),
                    "lr" => return self.register_part("x30", 64, 0),
                    _ => {}
                }
                if let Some(number) = name
                    .strip_prefix('w')
                    .and_then(|number| number.parse::<u8>().ok())
                    .filter(|number| *number < 31)
                {
                    return self.register_part(&format!("x{number}"), 32, 0);
                }
                self.register_part(&name, 64, 0)
            }
            Architecture::X86 | Architecture::X64 => {
                for (quad, dword, word, low, high) in [
                    ("rax", "eax", "ax", "al", "ah"),
                    ("rbx", "ebx", "bx", "bl", "bh"),
                    ("rcx", "ecx", "cx", "cl", "ch"),
                    ("rdx", "edx", "dx", "dl", "dh"),
                    ("rsi", "esi", "si", "sil", ""),
                    ("rdi", "edi", "di", "dil", ""),
                    ("rsp", "esp", "sp", "spl", ""),
                    ("rbp", "ebp", "bp", "bpl", ""),
                ] {
                    let canonical = if self.architecture == Architecture::X64 {
                        quad
                    } else {
                        dword
                    };
                    if name == dword {
                        return self.register_part(canonical, 32, 0);
                    }
                    if name == word {
                        return self.register_part(canonical, 16, 0);
                    }
                    if name == low && (self.architecture == Architecture::X64 || !high.is_empty()) {
                        return self.register_part(canonical, 8, 0);
                    }
                    if !high.is_empty() && name == high {
                        return self.register_part(canonical, 8, 8);
                    }
                }
                if self.architecture == Architecture::X64 {
                    for number in 8..16 {
                        for (suffix, bits) in [("d", 32), ("w", 16), ("b", 8)] {
                            if name == format!("r{number}{suffix}") {
                                return self.register_part(&format!("r{number}"), bits, 0);
                            }
                        }
                    }
                }
                self.register_part(&name, self.architecture.bits(), 0)
            }
            Architecture::Vole => self.register_part(&name, 8, 0),
        }
    }

    fn register_part(&self, canonical: &str, bits: u8, offset: u8) -> Option<u64> {
        let mask = if bits == 64 {
            u64::MAX
        } else {
            (1_u64 << bits) - 1
        };
        self.registers
            .iter()
            .find(|register| register.name.eq_ignore_ascii_case(canonical))
            .map(|register| (register.value >> offset) & mask)
    }

    pub fn byte(&self, address: u64) -> Option<u8> {
        self.memory
            .iter()
            .find(|region| region.contains(address))
            .map(|region| region.bytes[(address - region.base) as usize])
    }

    pub fn read(&self, address: u64, length: usize) -> Option<Vec<u8>> {
        (0..length)
            .map(|index| address.checked_add(index as u64).and_then(|a| self.byte(a)))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimError(pub String);

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SimError {}

/// The public execution seam. Each step is atomic on failure; edits clear undo.
pub trait Machine: Send {
    fn load(&mut self, program: &Program) -> Result<(), SimError>;
    fn step(&mut self) -> Result<StepRecord, SimError>;
    fn reverse_step(&mut self) -> Result<(), SimError>;
    fn reset(&mut self) -> Result<(), SimError>;
    fn snapshot(&self) -> Snapshot;
    /// Restore persisted state into the loaded layout. Prior undo is not retained.
    fn restore_snapshot(&mut self, snapshot: &Snapshot) -> Result<(), SimError>;
    fn write_register(&mut self, name: &str, value: u64) -> Result<(), SimError>;
    fn write_memory(&mut self, address: u64, bytes: &[u8]) -> Result<(), SimError>;
}
