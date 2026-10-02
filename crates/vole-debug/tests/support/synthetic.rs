//! A hand-written ARM64 program with hand-built debug information that mirrors
//! what Clang emits at -O0, so the debugger can be tested without a C compiler.
//!
//! The conceptual C document (line numbers matter):
//!
//! ```c
//!  1  int counter = 3;
//!  2  int g(void) { return 7; }
//!  3  int f(int n) {
//!  4      return g();
//!  5  }
//!  6  int main(void) {
//!  7      int a = 5;
//!  8      f(a);
//!  9      for (int i = 3; i; i--)
//! 10          counter++;
//! 11      return 42;
//! 12  }
//! ```
#![allow(dead_code)]
use vole_core::{
    Architecture, CompilerSettings, Program, SourceLanguage,
    debug::{
        BaseEncoding, CfaRule, DebugInfo, Function, LineRow, Location, LocationRange, Member,
        RegisterRule, SourceFile, Type, TypeKind, UnwindRow, Variable,
    },
};

pub const SOURCE: &str = "\
.text
.global _start
.type _start, %function
.type vole_exit, %function
.type main, %function
.type f, %function
.type g, %function
.type counter, %object
.type numbers, %object
.type origin, %object
.type text, %object
.type message, %object
.type runtime_state, %object
_start:
    mov x29, #0
    mov x30, #0
    bl main
    bl vole_exit
vole_exit:
    mov x8, #93
    svc #0
main:
    stp x29, x30, [sp, #-32]!
    mov x29, sp
    mov w0, #5
    str w0, [x29, #28]
    bl f
    mov x1, #8192
    mov w3, #3
loop:
    ldr w2, [x1]
    add w2, w2, #1
    str w2, [x1]
    subs w3, w3, #1
    b.ne loop
    mov w0, #42
    ldp x29, x30, [sp], #32
    ret
f:
    stp x29, x30, [sp, #-16]!
    mov x29, sp
    bl g
    ldp x29, x30, [sp], #16
    ret
g:
    mov w0, #7
    ret
.data
counter: .word 3
numbers: .word 3, 1, 4
origin: .word 10, -2
text: .asciz \"hi\\n\"
.balign 8
message: .quad text
runtime_state: .word 9
";

pub const INT: usize = 0;
pub const CHAR: usize = 1;
pub const CONST_CHAR: usize = 2;
pub const CONST_CHAR_POINTER: usize = 3;
pub const INT_ARRAY: usize = 4;
pub const POINT: usize = 5;
pub const INT_POINTER: usize = 6;

pub struct Fixture {
    pub program: Program,
    pub main: u64,
    pub f: u64,
    pub g: u64,
    pub vole_exit: u64,
    pub start: u64,
}

impl Fixture {
    /// Address of `main` plus a byte offset.
    pub fn main_at(&self, offset: u64) -> u64 {
        self.main + offset
    }
}

/// Assemble the fixture; `None` when LLVM tools are unavailable.
pub fn fixture() -> Option<Fixture> {
    let mut program = match vole_isa_scalar::assemble(Architecture::Arm64, SOURCE) {
        Ok(program) => program,
        Err(diagnostics) => {
            eprintln!("skipping: ARM64 assembly is unavailable: {diagnostics:?}");
            return None;
        }
    };
    let symbol = |name: &str| program.symbols[name];
    let (start, vole_exit, main, f, g) = (
        symbol("_start"),
        symbol("vole_exit"),
        symbol("main"),
        symbol("f"),
        symbol("g"),
    );
    let data = |name: &str| program.symbols[name];
    let row = |address: u64, file: u32, line: u32, prologue_end: bool| LineRow {
        address,
        file,
        line,
        column: 1,
        is_stmt: true,
        prologue_end,
        end_sequence: false,
    };
    let mut lines = vec![
        row(start, 1, 3, false),
        row(vole_exit, 2, 10, false),
        row(main, 0, 6, false),
        row(main + 8, 0, 7, true),
        row(main + 16, 0, 8, false),
        row(main + 20, 0, 9, false),
        row(main + 28, 0, 10, false),
        row(main + 40, 0, 9, false),
        row(main + 48, 0, 11, false),
        row(main + 52, 0, 12, false),
        row(f, 0, 3, false),
        row(f + 8, 0, 4, true),
        row(f + 12, 0, 5, false),
        row(g, 0, 2, false),
    ];
    lines.push(LineRow {
        end_sequence: true,
        is_stmt: false,
        ..row(g + 8, 0, 2, false)
    });
    let frame_rows = |low: u64, size: i64, body_end: u64, high: u64| {
        vec![
            UnwindRow {
                start: low,
                end: low + 4,
                cfa: CfaRule::RegisterOffset {
                    register: "sp".into(),
                    offset: 0,
                },
                registers: vec![],
            },
            UnwindRow {
                start: low + 4,
                end: body_end,
                cfa: CfaRule::RegisterOffset {
                    register: "sp".into(),
                    offset: size,
                },
                registers: vec![
                    ("x29".into(), RegisterRule::Offset(-size)),
                    ("x30".into(), RegisterRule::Offset(-size + 8)),
                ],
            },
            UnwindRow {
                start: body_end,
                end: high,
                cfa: CfaRule::RegisterOffset {
                    register: "sp".into(),
                    offset: 0,
                },
                registers: vec![],
            },
        ]
    };
    let mut unwind = frame_rows(main, 32, main + 56, main + 60);
    unwind.extend(frame_rows(f, 16, f + 16, f + 20));
    unwind.push(UnwindRow {
        start: g,
        end: g + 8,
        cfa: CfaRule::RegisterOffset {
            register: "sp".into(),
            offset: 0,
        },
        registers: vec![],
    });
    let variable = |name: &str, type_id: usize, location: Location, line: u32| Variable {
        name: name.into(),
        type_id: Some(type_id),
        location,
        decl_file: 0,
        decl_line: line,
        parameter: false,
        scope: vec![],
    };
    let functions = vec![
        Function {
            name: "vole_exit".into(),
            low_pc: vole_exit,
            high_pc: vole_exit + 8,
            decl_file: 2,
            decl_line: 10,
            frame_base: Some(Location::Register("sp".into())),
            return_type: None,
            variables: vec![Variable {
                parameter: true,
                decl_file: 2,
                ..variable("status", INT, Location::Register("x0".into()), 10)
            }],
            prologue_end: None,
            user: false,
        },
        Function {
            name: "main".into(),
            low_pc: main,
            high_pc: main + 60,
            decl_file: 0,
            decl_line: 6,
            frame_base: Some(Location::Register("x29".into())),
            return_type: Some(INT),
            variables: vec![
                variable("a", INT, Location::FrameOffset(28), 7),
                Variable {
                    scope: vec![(main + 20, main + 48)],
                    ..variable("i", INT, Location::Register("x3".into()), 9)
                },
            ],
            prologue_end: Some(main + 8),
            user: true,
        },
        Function {
            name: "f".into(),
            low_pc: f,
            high_pc: f + 20,
            decl_file: 0,
            decl_line: 3,
            frame_base: Some(Location::Register("x29".into())),
            return_type: Some(INT),
            variables: vec![Variable {
                parameter: true,
                ..variable(
                    "n",
                    INT,
                    Location::List(vec![LocationRange {
                        start: f,
                        end: f + 12,
                        location: Location::Register("x0".into()),
                    }]),
                    3,
                )
            }],
            prologue_end: Some(f + 8),
            user: true,
        },
        Function {
            name: "g".into(),
            low_pc: g,
            high_pc: g + 8,
            decl_file: 0,
            decl_line: 2,
            frame_base: Some(Location::Register("sp".into())),
            return_type: Some(INT),
            variables: vec![],
            prologue_end: None,
            user: true,
        },
    ];
    let globals = vec![
        variable("counter", INT, Location::Address(data("counter")), 1),
        variable("numbers", INT_ARRAY, Location::Address(data("numbers")), 1),
        variable("origin", POINT, Location::Address(data("origin")), 1),
        variable(
            "message",
            CONST_CHAR_POINTER,
            Location::Address(data("message")),
            1,
        ),
        variable("cursor", INT_POINTER, Location::Address(data("message")), 1),
        Variable {
            decl_file: 2,
            ..variable(
                "runtime_state",
                INT,
                Location::Address(data("runtime_state")),
                1,
            )
        },
    ];
    let types = vec![
        Type {
            name: "int".into(),
            size: 4,
            kind: TypeKind::Base(BaseEncoding::Signed),
        },
        Type {
            name: "char".into(),
            size: 1,
            kind: TypeKind::Base(BaseEncoding::SignedChar),
        },
        Type {
            name: String::new(),
            size: 0,
            kind: TypeKind::Const(Some(CHAR)),
        },
        Type {
            name: String::new(),
            size: 8,
            kind: TypeKind::Pointer(Some(CONST_CHAR)),
        },
        Type {
            name: String::new(),
            size: 12,
            kind: TypeKind::Array {
                element: Some(INT),
                count: Some(3),
            },
        },
        Type {
            name: "point".into(),
            size: 8,
            kind: TypeKind::Struct(vec![
                Member {
                    name: "x".into(),
                    type_id: Some(INT),
                    offset: 0,
                },
                Member {
                    name: "y".into(),
                    type_id: Some(INT),
                    offset: 4,
                },
            ]),
        },
        Type {
            name: String::new(),
            size: 8,
            kind: TypeKind::Pointer(Some(INT)),
        },
    ];
    program.language = SourceLanguage::C;
    program.debug = Some(DebugInfo {
        producer: "hand-built fixture".into(),
        triple: "aarch64-none-elf".into(),
        settings: CompilerSettings::default(),
        compiler_flags: vec![],
        files: vec![
            SourceFile {
                name: "main.c".into(),
                user: true,
            },
            SourceFile {
                name: "crt0.s".into(),
                user: false,
            },
            SourceFile {
                name: "vole_runtime.c".into(),
                user: false,
            },
        ],
        lines,
        functions,
        globals,
        types,
        unwind,
        return_address_register: "x30".into(),
    });
    Some(Fixture {
        program,
        main,
        f,
        g,
        vole_exit,
        start,
    })
}
