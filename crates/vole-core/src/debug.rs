//! Serializable source-level debug metadata extracted from compiler DWARF.
//!
//! The compiler crate converts DWARF into this model once, at build time. The
//! debugger, runtime and desktop only read this model, so saved projects keep
//! working without re-reading ELF files or depending on a DWARF parser.
use serde::{Deserialize, Serialize};

/// Language of the document that produced a program image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum SourceLanguage {
    #[default]
    Assembly,
    C,
}

impl SourceLanguage {
    pub fn name(self) -> &'static str {
        match self {
            Self::Assembly => "Assembly",
            Self::C => "C",
        }
    }
}

/// Clang optimization levels inside the documented teaching scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, Hash)]
pub enum Optimization {
    /// Every variable lives in memory; all values are inspectable.
    #[default]
    #[serde(rename = "O0")]
    O0,
    /// Optimized code; some variables are reported as unavailable.
    #[serde(rename = "O1")]
    O1,
}

impl Optimization {
    pub fn flag(self) -> &'static str {
        match self {
            Self::O0 => "-O0",
            Self::O1 => "-O1",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(default, deny_unknown_fields)]
pub struct CompilerSettings {
    pub optimization: Optimization,
    /// Adds -Wall -Wextra. Warnings never block a build.
    pub warnings: bool,
}

impl Default for CompilerSettings {
    fn default() -> Self {
        Self {
            optimization: Optimization::O0,
            warnings: true,
        }
    }
}

/// One compiler line-table row. Rows are sorted by address within a sequence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRow {
    pub address: u64,
    /// Index into [`DebugInfo::files`].
    pub file: u32,
    pub line: u32,
    pub column: u32,
    pub is_stmt: bool,
    pub prologue_end: bool,
    /// Marks the first address after a sequence; it does not start a line.
    pub end_sequence: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFile {
    pub name: String,
    /// True for the user's document; false for startup code and the runtime.
    pub user: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Location {
    /// DW_OP_fbreg: frame base plus a signed offset.
    FrameOffset(i64),
    /// DW_OP_addr: a fixed guest address.
    Address(u64),
    /// DW_OP_bregN: a register value plus a signed offset is the address.
    RegisterOffset { register: String, offset: i64 },
    /// DW_OP_regN: the value itself is in the register.
    Register(String),
    /// Location-list entries; addresses are absolute and half-open.
    List(Vec<LocationRange>),
    /// The compiler did not describe a supported location. The text explains why.
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocationRange {
    pub start: u64,
    pub end: u64,
    pub location: Location,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    /// Index into [`DebugInfo::types`]; `None` means an unknown type.
    pub type_id: Option<usize>,
    pub location: Location,
    pub decl_file: u32,
    pub decl_line: u32,
    pub parameter: bool,
    /// Half-open lexical-block ranges. Empty means the whole function.
    pub scope: Vec<(u64, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    pub low_pc: u64,
    /// First address after the function.
    pub high_pc: u64,
    pub decl_file: u32,
    pub decl_line: u32,
    pub frame_base: Option<Location>,
    pub return_type: Option<usize>,
    pub variables: Vec<Variable>,
    /// Address of the first line-table row flagged prologue_end, if any.
    pub prologue_end: Option<u64>,
    /// True when the function belongs to the user's document.
    pub user: bool,
}

impl Function {
    pub fn contains(&self, address: u64) -> bool {
        (self.low_pc..self.high_pc).contains(&address)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaseEncoding {
    Signed,
    Unsigned,
    SignedChar,
    UnsignedChar,
    Boolean,
    Float,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub type_id: Option<usize>,
    pub offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypeKind {
    Void,
    Base(BaseEncoding),
    Pointer(Option<usize>),
    Array {
        element: Option<usize>,
        count: Option<u64>,
    },
    Struct(Vec<Member>),
    Union(Vec<Member>),
    Enum(Vec<(String, i64)>),
    Typedef(Option<usize>),
    Const(Option<usize>),
    Volatile(Option<usize>),
    Function,
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Type {
    pub name: String,
    pub size: u64,
    pub kind: TypeKind,
}

/// Canonical frame address rule for an unwind row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CfaRule {
    RegisterOffset { register: String, offset: i64 },
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegisterRule {
    Undefined,
    SameValue,
    /// Saved in memory at CFA + offset.
    Offset(i64),
    /// The value is CFA + offset.
    ValOffset(i64),
    /// Saved in another register.
    Register(String),
    Unsupported(String),
}

/// One row of the DWARF call-frame table, covering `[start, end)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnwindRow {
    pub start: u64,
    pub end: u64,
    pub cfa: CfaRule,
    /// Rules for registers the row describes, using machine register names.
    pub registers: Vec<(String, RegisterRule)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct DebugInfo {
    /// Compiler identification from DW_AT_producer.
    pub producer: String,
    pub triple: String,
    pub settings: CompilerSettings,
    /// Exact compiler arguments, excluding private temporary paths.
    pub compiler_flags: Vec<String>,
    pub files: Vec<SourceFile>,
    /// Rows sorted by address. Sequences do not overlap after linking.
    pub lines: Vec<LineRow>,
    pub functions: Vec<Function>,
    pub globals: Vec<Variable>,
    pub types: Vec<Type>,
    pub unwind: Vec<UnwindRow>,
    /// Machine register name of the DWARF return-address column.
    pub return_address_register: String,
}

impl DebugInfo {
    pub fn function_at(&self, address: u64) -> Option<&Function> {
        self.functions.iter().find(|f| f.contains(address))
    }

    pub fn is_user_file(&self, file: u32) -> bool {
        self.files.get(file as usize).is_some_and(|f| f.user)
    }

    /// The row whose address range contains `address` (not necessarily its start).
    pub fn row_for_address(&self, address: u64) -> Option<&LineRow> {
        let index = self.lines.partition_point(|row| row.address <= address);
        let row = self.lines.get(index.checked_sub(1)?)?;
        (!row.end_sequence).then_some(row)
    }

    pub fn unwind_row(&self, address: u64) -> Option<&UnwindRow> {
        let index = self.unwind.partition_point(|row| row.start <= address);
        let row = self.unwind.get(index.checked_sub(1)?)?;
        (address < row.end).then_some(row)
    }
}

/// A resolved source position for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    pub file: u32,
    pub line: u32,
    pub column: u32,
    /// True when the position is in the user's document.
    pub user: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueText {
    Value(String),
    /// The debugger cannot show a value; the text explains why.
    Unavailable(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VariableKind {
    Parameter,
    Local,
    Global,
    Element,
    Member,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariableView {
    pub name: String,
    pub type_name: String,
    pub kind: VariableKind,
    pub value: ValueText,
    /// Guest address of the object when it lives in memory.
    pub address: Option<u64>,
    pub size: u64,
    /// Bounded array elements or struct members.
    pub children: Vec<VariableView>,
    /// True when the value differs from the previous paused observation.
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameView {
    /// 0 is the innermost (current) frame.
    pub index: usize,
    pub function: String,
    pub pc: u64,
    pub cfa: Option<u64>,
    pub location: Option<SourceLocation>,
    pub user: bool,
    pub variables: Vec<VariableView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DebugView {
    pub frames: Vec<FrameView>,
    pub globals: Vec<VariableView>,
    /// Source position containing the current PC.
    pub location: Option<SourceLocation>,
    /// Status passed to the teaching exit call, once the program has exited.
    pub exit_status: Option<i64>,
    /// Explains incomplete information, e.g. execution inside runtime code.
    pub note: Option<String>,
}
