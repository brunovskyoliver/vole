//! End-to-end C builds with the host Clang and LLD. Tests skip when the
//! toolchain is not installed, like the assembler tests.
use vole_core::{
    Architecture, CompilerSettings, Diagnostic, Machine, Optimization, Program, Severity,
    debug::{Location, TypeKind},
};
use vole_isa_scalar::ScalarMachine;

const TARGETS: [Architecture; 4] = [
    Architecture::Arm64,
    Architecture::X64,
    Architecture::Arm32,
    Architecture::X86,
];

fn toolchain() -> bool {
    let available = vole_c::clang_status().is_ok() && vole_isa_scalar::toolchain_status().is_ok();
    if !available {
        eprintln!("Clang or LLD not installed; skipping C build test");
    }
    available
}

fn settings(optimization: Optimization) -> CompilerSettings {
    CompilerSettings {
        optimization,
        warnings: true,
    }
}

fn build(
    architecture: Architecture,
    source: &str,
) -> Result<(Program, Vec<Diagnostic>), Vec<Diagnostic>> {
    vole_c::compile_with_warnings(architecture, source, &settings(Optimization::O0))
}

fn errors(architecture: Architecture, source: &str) -> Vec<Diagnostic> {
    match build(architecture, source) {
        Ok(_) => panic!("{architecture:?}: expected the build to fail"),
        Err(diagnostics) => diagnostics,
    }
}

/// Expected console output of each example; `{pointer}` is the pointer size.
const EXPECTED: &[(&str, &str)] = &[
    (
        "tour.c",
        "Vole C tour\nx + y = 10, x - y = 4\nx * y = 21, x / y = 2, x % y = 1\n\
         after swap: x = 3, y = 7\ntotal score = 399, average = 79\n1! = 1\n2! = 2\n3! = 6\n\
         4! = 24\n5! = 120\nmask = 0xf4\n",
    ),
    (
        "hello.c",
        "Hi\nHello, Vole!\nputs adds a newline too.\nint: -42, hex: 0xbeef\n\
         Name: Ada, initial: A, length: 3\nDecimal 255, hex ff, upper hex FF, octal 377\n\
         Width: [   42] [42   ] [00042]\nPercent sign: 100%\n",
    ),
    (
        "arithmetic.c",
        "a = 17, b = 5\na + b = 22\na - b = 12\na * b = 85\n\
         a / b = 3 (division drops the fraction)\na % b = 2 (the remainder)\n\
         -a / b = -3, -a % b = -2 (rounds toward zero)\n\
         bits = 0x5a, bits & 0x0F = 0xa, bits | 0x0F = 0x5f\n\
         bits ^ 0xFF = 0xa5, bits << 2 = 0x168, bits >> 3 = 0xb\n\
         250 + 10 in an unsigned char = 4\n\
         big = 1234567890123, big / 1000 = 1234567890, big % 1000 = 123\naverage = 18\n",
    ),
    (
        "loops.c",
        "1 2 3 4 5 \n40 -> 20 -> 10 -> 5 -> 2 -> 1\n9075 has 4 digits\n1 3 5 7 9 11 \n\
         Monday\nTuesday\nAnother day\n   1   2   3   4\n   2   4   6   8\n   3   6   9  12\n",
    ),
    (
        "functions.c",
        "square(7) = 49\nsum_of_squares(3, 4) = 25\n0 1 1 2 3 5 8 13 21 34 55 \n\
         gcd(84, 36) = 12\ntickets: 101, 102\n",
    ),
    (
        "arrays.c",
        "31 4 15 9 26 5 \n4 5 9 15 26 31 \ngrid total = 21\nreversed: kcats (5 letters)\n\
         0 1 4 9 16 \n",
    ),
    (
        "pointers.c",
        "a = 10, b = 2\nafter swap: a = 2, b = 10\n*p = 5, *(p + 2) = 15, p[3] = 20\n\
         elements between: 3\nspot = (5, 3)\nletters 's' in \"mississippi\": 4\n\
         a pointer is {pointer} bytes on this machine\n",
    ),
];

fn check_structure(context: &str, program: &Program, source: &str, optimization: Optimization) {
    assert_eq!(program.source, source, "{context}");
    assert_eq!(program.language, vole_core::SourceLanguage::C, "{context}");
    assert_eq!(
        Some(&program.entry),
        program.symbols.get("_start"),
        "{context}: entry"
    );
    let layout: Vec<_> = program
        .regions
        .iter()
        .map(|r| (r.label.as_str(), r.base, r.writable, r.executable))
        .collect();
    assert_eq!(
        layout,
        [
            ("Code", 0x1000, false, true),
            ("Read-only data", 0x8000, false, false),
            ("Data", 0xA000, true, false),
            ("Stack", 0x10000, true, false),
        ],
        "{context}"
    );
    let sizes: Vec<_> = program.regions.iter().map(|r| r.bytes.len()).collect();
    assert!(sizes[0] > 0 && sizes[0] <= 0x7000, "{context}: code size");
    assert_eq!(sizes[1..], [0x2000, 0x6000, 0x10000], "{context}");
    let code_bytes: usize = program.instructions.iter().map(|i| i.bytes.len()).sum();
    assert_eq!(
        code_bytes, sizes[0],
        "{context}: instructions cover the code"
    );
    assert_eq!(
        program
            .initial_registers
            .values()
            .copied()
            .collect::<Vec<_>>(),
        [0x20000]
    );

    let debug = program.debug.as_ref().expect("debug info");
    assert_eq!(debug.files[0].name, "main.c");
    assert!(
        debug.files[0].user && debug.files[1..].iter().all(|f| !f.user),
        "{context}"
    );
    assert!(
        debug.producer.contains("clang"),
        "{context}: {}",
        debug.producer
    );
    assert!(
        debug
            .compiler_flags
            .contains(&optimization.flag().to_string())
    );
    assert!(
        !debug.compiler_flags.iter().any(|f| f.contains("vole-c-")),
        "{context}: temp path"
    );
    assert!(
        debug.lines.windows(2).all(|w| w[0].address <= w[1].address),
        "{context}"
    );
    assert!(
        debug.lines.iter().any(|row| row.file == 0 && row.is_stmt),
        "{context}"
    );
    let lines = source.lines().count();
    for instruction in &program.instructions {
        if let Some(line) = instruction.source_line {
            assert!((1..=lines).contains(&line), "{context}: line {line}");
        }
    }
    assert!(
        program
            .instructions
            .iter()
            .filter(|i| i.source_line.is_some())
            .count()
            > 10,
        "{context}: user lines"
    );

    let main = debug
        .functions
        .iter()
        .find(|f| f.name == "main")
        .expect("main function");
    assert!(main.user && main.decl_file == 0, "{context}");
    assert_eq!(Some(&main.low_pc), program.symbols.get("main"), "{context}");
    let prologue_end = main.prologue_end.expect("main prologue end");
    assert!(main.contains(prologue_end), "{context}");
    assert!(
        debug
            .functions
            .iter()
            .any(|f| f.name == "_start" && !f.user),
        "{context}"
    );
    assert!(
        debug
            .functions
            .iter()
            .any(|f| f.name == "printf" && !f.user),
        "{context}"
    );
    if optimization == Optimization::O0 {
        assert!(!main.variables.is_empty(), "{context}: main variables");
        for function in debug.functions.iter().filter(|f| f.user) {
            for variable in &function.variables {
                assert!(
                    matches!(
                        variable.location,
                        Location::FrameOffset(_)
                            | Location::Address(_)
                            | Location::RegisterOffset { .. }
                    ),
                    "{context}: {}.{} at {:?}",
                    function.name,
                    variable.name,
                    variable.location
                );
            }
        }
    }
    for variable in debug
        .functions
        .iter()
        .flat_map(|f| &f.variables)
        .chain(&debug.globals)
    {
        let id = variable.type_id.expect("variable type");
        let ty = &debug.types[id];
        assert!(!ty.name.is_empty(), "{context}: {} type", variable.name);
        assert!(
            !matches!(ty.kind, TypeKind::Unsupported(_)),
            "{context}: {} has {ty:?}",
            variable.name
        );
    }
    for global in debug.globals.iter().filter(|g| g.decl_file == 0) {
        match global.location {
            Location::Address(address) => {
                assert!(
                    (0x8000..0x10000).contains(&address),
                    "{context}: {}",
                    global.name
                )
            }
            ref other => panic!("{context}: global {} at {other:?}", global.name),
        }
    }
    for address in [main.low_pc, prologue_end, main.high_pc - 1] {
        assert!(
            debug.unwind_row(address).is_some(),
            "{context}: unwind row at 0x{address:x}"
        );
    }
    assert!(!debug.return_address_register.is_empty());
}

#[test]
fn examples_build_on_every_target() {
    if !toolchain() {
        return;
    }
    assert_eq!(vole_c::default_example(), vole_c::EXAMPLES[0].1);
    assert_eq!(vole_c::EXAMPLES.len(), EXPECTED.len());
    for (name, source) in vole_c::EXAMPLES {
        let lines = source.lines().count();
        assert!(lines < 90, "{name} has {lines} lines");
        for architecture in TARGETS {
            for optimization in [Optimization::O0, Optimization::O1] {
                let context = format!("{name} {architecture:?} {optimization:?}");
                let (program, warnings) =
                    vole_c::compile_with_warnings(architecture, source, &settings(optimization))
                        .unwrap_or_else(|e| panic!("{context}: {e:#?}"));
                assert!(warnings.is_empty(), "{context}: {warnings:#?}");
                check_structure(&context, &program, source, optimization);
            }
        }
    }
}

#[test]
fn tour_has_globals_types_and_scopes() {
    if !toolchain() {
        return;
    }
    let (program, _) = build(Architecture::Arm64, vole_c::default_example()).unwrap();
    let debug = program.debug.unwrap();
    let scores = debug.globals.iter().find(|g| g.name == "scores").unwrap();
    assert_eq!(scores.location, Location::Address(0xA000));
    let ty = &debug.types[scores.type_id.unwrap()];
    assert_eq!((ty.name.as_str(), ty.size), ("int[5]", 20));
    assert!(matches!(ty.kind, TypeKind::Array { count: Some(5), .. }));
    let sum = debug.functions.iter().find(|f| f.name == "sum").unwrap();
    let i = sum.variables.iter().find(|v| v.name == "i").unwrap();
    assert_eq!(i.scope.len(), 1);
    assert!(sum.contains(i.scope[0].0) && i.scope[0].1 <= sum.high_pc);
    let values = sum.variables.iter().find(|v| v.name == "values").unwrap();
    assert!(values.parameter);
    assert_eq!(debug.types[values.type_id.unwrap()].name, "const int *");
    assert_eq!(debug.return_address_register, "x30");
}

#[test]
fn types_cover_structs_enums_and_matrices() {
    if !toolchain() {
        return;
    }
    let source = "#include <vole.h>\n\
        struct node { int value; struct node *next; };\n\
        union word { unsigned u; unsigned char bytes[4]; };\n\
        enum color { RED, GREEN = 5, BLUE = -1 };\n\
        typedef struct node node_t;\n\
        node_t list[2];\n\
        union word w;\n\
        enum color c = GREEN;\n\
        int matrix[2][3];\n\
        volatile const int answer = 42;\n\
        int main(void) { static int calls; calls++; return list[0].value + w.bytes[1] + c + matrix[1][2] + answer + calls; }\n";
    for architecture in TARGETS {
        let (program, _) = build(architecture, source).unwrap();
        let debug = program.debug.unwrap();
        let ty = |name: &str| {
            let global = debug.globals.iter().find(|g| g.name == name).unwrap();
            &debug.types[global.type_id.unwrap()]
        };
        let list = ty("list");
        let TypeKind::Array { element, count } = &list.kind else {
            panic!("{list:?}")
        };
        assert_eq!(*count, Some(2));
        let node_t = &debug.types[element.unwrap()];
        assert_eq!(node_t.name, "node_t");
        let TypeKind::Typedef(Some(node)) = node_t.kind else {
            panic!()
        };
        let TypeKind::Struct(members) = &debug.types[node].kind else {
            panic!()
        };
        assert_eq!(members[1].name, "next");
        assert_eq!(
            members[1].offset,
            if architecture.bits() == 64 { 8 } else { 4 }
        );
        assert_eq!(
            debug.types[members[1].type_id.unwrap()].name,
            "struct node *"
        );
        assert!(matches!(ty("w").kind, TypeKind::Union(ref m) if m.len() == 2));
        assert_eq!(
            ty("c").kind,
            TypeKind::Enum(vec![
                ("RED".into(), 0),
                ("GREEN".into(), 5),
                ("BLUE".into(), -1)
            ])
        );
        let matrix = ty("matrix");
        assert_eq!((matrix.name.as_str(), matrix.size), ("int[2][3]", 24));
        let TypeKind::Array {
            element,
            count: Some(2),
        } = matrix.kind
        else {
            panic!()
        };
        assert_eq!(debug.types[element.unwrap()].name, "int[3]");
        assert_eq!(ty("answer").name, "const volatile int");
        // Constant data goes to the read-only region.
        let answer = debug.globals.iter().find(|g| g.name == "answer").unwrap();
        assert!(matches!(answer.location, Location::Address(a) if (0x8000..0xA000).contains(&a)));
        let main = debug.functions.iter().find(|f| f.name == "main").unwrap();
        let calls = main.variables.iter().find(|v| v.name == "calls").unwrap();
        assert!(matches!(calls.location, Location::Address(a) if a >= 0xA000));
    }
}

#[test]
fn unsupported_features_are_rejected_before_clang() {
    // These checks need no toolchain.
    let float = vole_c::compile(
        Architecture::X64,
        "int main(void) {\n    float x = 1;\n    return 0;\n}\n",
        &CompilerSettings::default(),
    )
    .unwrap_err();
    assert_eq!((float[0].line, float[0].column), (2, 5));
    assert!(float[0].message.contains("float"));
    assert!(float[0].hint.as_deref().unwrap().contains("integer"));

    let include = vole_c::compile(
        Architecture::Arm64,
        "#include <vole.h>\n#include <stdio.h>\nint main(void) { return 0; }\n",
        &CompilerSettings::default(),
    )
    .unwrap_err();
    assert_eq!((include[0].line, include[0].column), (2, 10));
    assert!(include[0].hint.as_deref().unwrap().contains("<vole.h>"));

    let vole = vole_c::compile(
        Architecture::Vole,
        "int main(void){return 0;}",
        &CompilerSettings::default(),
    );
    assert!(vole.is_err());
    let large = "/* padding */\n".repeat(20_000);
    assert!(vole_c::compile(Architecture::X64, &large, &CompilerSettings::default()).is_err());
}

#[test]
fn compiler_errors_and_warnings_are_mapped() {
    if !toolchain() {
        return;
    }
    let syntax = errors(
        Architecture::X86,
        "#include <vole.h>\n\nint main(void) {\n    int x = 1\n    return x;\n}\n",
    );
    let error = syntax.iter().find(|d| d.is_error()).unwrap();
    assert_eq!(error.line, 4, "{syntax:#?}");
    assert!(error.message.contains("expected ';'"), "{syntax:#?}");

    let undeclared = errors(
        Architecture::Arm32,
        "int main(void) {\n    return missing(2);\n}\n",
    );
    assert_eq!(
        (undeclared[0].line, undeclared[0].column),
        (2, 12),
        "{undeclared:#?}"
    );

    let conflict = errors(
        Architecture::X64,
        "#include <vole.h>\nint strlen(int x);\nint main(void) { return 0; }\n",
    );
    assert_eq!(conflict[0].line, 2);
    assert!(
        conflict
            .iter()
            .any(|d| d.severity == Severity::Note && d.message.starts_with("vole.h:"))
    );

    let (program, warnings) = build(
        Architecture::Arm64,
        "#include <vole.h>\nint main(void) {\n    int unused;\n    return 0;\n}\n",
    )
    .unwrap();
    assert!(program.debug.is_some());
    assert_eq!(warnings.len(), 1, "{warnings:#?}");
    assert_eq!(
        (warnings[0].line, warnings[0].column, warnings[0].severity),
        (3, 9, Severity::Warning)
    );

    let quiet = vole_c::compile_with_warnings(
        Architecture::Arm64,
        "int main(void) {\n    int unused;\n    return 0;\n}\n",
        &CompilerSettings {
            optimization: Optimization::O0,
            warnings: false,
        },
    )
    .unwrap();
    assert!(quiet.1.is_empty());
}

#[test]
fn link_errors_are_explained() {
    if !toolchain() {
        return;
    }
    let malloc = errors(
        Architecture::Arm64,
        "#include <vole.h>\nvoid *malloc(size_t size);\n\nint main(void) {\n    char *p = malloc(16);\n    return p[0];\n}\n",
    );
    assert_eq!((malloc[0].line, malloc[0].column), (5, 15), "{malloc:#?}");
    assert!(malloc[0].message.contains("malloc"));
    let hint = malloc[0].hint.as_deref().unwrap();
    assert!(hint.contains("heap") && hint.contains("printf"), "{hint}");

    let missing = errors(Architecture::X64, "int helper(void) { return 1; }\n");
    assert!(missing[0].message.contains("no main"), "{missing:#?}");

    let duplicate = errors(
        Architecture::X86,
        "void vole_putc(char c) { (void)c; }\nint main(void) { return 0; }\n",
    );
    assert_eq!(duplicate[0].line, 1, "{duplicate:#?}");
    assert!(duplicate[0].message.contains("runtime"));

    let large = "char big[30000] = {1};\nint main(void) { return big[0]; }\n";
    let data = errors(Architecture::X64, large);
    assert!(data[0].message.contains("data region"), "{data:#?}");
}

#[test]
fn documents_may_replace_library_functions() {
    if !toolchain() {
        return;
    }
    let source = "unsigned long strlen(const char *s) {\n    unsigned long n = 0;\n    while (s[n]) n++;\n    return n;\n}\nint main(void) { return (int)strlen(\"abc\"); }\n";
    for architecture in TARGETS {
        let (program, _) = build(architecture, source).unwrap();
        let debug = program.debug.unwrap();
        let user = debug
            .functions
            .iter()
            .find(|f| f.name == "strlen" && f.user)
            .unwrap();
        assert_eq!(program.symbols.get("strlen"), Some(&user.low_pc));
    }
}

#[test]
fn runtime_fits_comfortably() {
    if !toolchain() {
        return;
    }
    for architecture in TARGETS {
        let (program, _) = build(architecture, "int main(void) { return 0; }\n").unwrap();
        let code = program.regions[0].bytes.len();
        assert!(
            code < 0x2400,
            "{architecture:?}: runtime uses {code} bytes of code"
        );
        let rodata = &program.regions[1].bytes;
        let used = rodata.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
        assert!(
            used < 0x400,
            "{architecture:?}: runtime uses {used} bytes of read-only data"
        );
    }
}

/// Runs a program until it halts; returns output or the fault with its instruction.
fn run(program: &Program, budget: u64) -> Result<String, String> {
    let mut machine = ScalarMachine::new(program.architecture);
    machine.load(program).map_err(|e| format!("load: {e}"))?;
    let mut pc = program.entry;
    let mut output = Vec::new();
    for _ in 0..budget {
        match machine.step() {
            Ok(record) => {
                output.extend_from_slice(&record.output_added);
                if record.halted {
                    return Ok(String::from_utf8_lossy(&output).into_owned());
                }
                pc = record.pc_after;
            }
            Err(error) => {
                let instruction = program
                    .instruction(pc)
                    .map_or("?".into(), |i| i.assembly.clone());
                let function = program
                    .debug
                    .as_ref()
                    .and_then(|d| d.function_at(pc))
                    .map_or("?", |f| f.name.as_str());
                return Err(format!("0x{pc:x} in {function}: `{instruction}`: {error}"));
            }
        }
    }
    Err(format!("did not halt within {budget} instructions"))
}

/// Executes every example on the interpreter at both optimization levels and
/// compares the output with expectations checked against a native build.
#[test]
fn examples_run_on_every_target() {
    if !toolchain() {
        return;
    }
    let mut failures = Vec::new();
    for (name, source) in vole_c::EXAMPLES {
        let expected = EXPECTED.iter().find(|(n, _)| n == name).unwrap().1;
        for architecture in TARGETS {
            for optimization in [Optimization::O0, Optimization::O1] {
                let program =
                    vole_c::compile(architecture, source, &settings(optimization)).unwrap();
                let pointer = (architecture.bits() / 8).to_string();
                let expected = expected.replace("{pointer}", &pointer);
                match run(&program, 2_000_000) {
                    Ok(output) if output == expected => {}
                    Ok(output) => failures.push(format!(
                        "{name} {architecture:?} {optimization:?}: wrong output {output:?}"
                    )),
                    Err(fault) => {
                        failures.push(format!("{name} {architecture:?} {optimization:?}: {fault}"))
                    }
                }
            }
        }
    }
    for failure in &failures {
        eprintln!("{failure}");
    }
    assert!(failures.is_empty(), "{} runs failed", failures.len());
}

/// Struct assignment and zero-initialized aggregates call memory helpers
/// (`__aeabi_memcpy4` and friends on ARM32, `rep movs` on x86/x64).
#[test]
fn struct_copies_run_on_every_target() {
    if !toolchain() {
        return;
    }
    let source = "#include <vole.h>\n\
        struct point { int x, y, z; long tag; };\n\
        struct point make(int x) { struct point p = {x, x * 2, x * 3, 7}; return p; }\n\
        int main(void) {\n\
            struct point a = make(4);\n\
            struct point b = a;\n\
            int zero[16] = {0};\n\
            b.y += zero[15];\n\
            printf(\"%d %d %d %ld\\n\", b.x, b.y, b.z, b.tag);\n\
            return 0;\n\
        }\n";
    for architecture in TARGETS {
        for optimization in [Optimization::O0, Optimization::O1] {
            let program = vole_c::compile(architecture, source, &settings(optimization)).unwrap();
            assert_eq!(
                run(&program, 200_000).unwrap(),
                "4 8 12 7\n",
                "{architecture:?} {optimization:?}"
            );
        }
    }
}

/// With an empty .data, LLD drops that output section; .bss must still be
/// placed in the data region.
#[test]
fn zero_initialized_globals_alone_build_and_run() {
    if !toolchain() {
        return;
    }
    let source = "int total = 0;\nint values[4];\nint main(void) { total = 1; values[3] = 2; return total + values[3]; }\n";
    for architecture in TARGETS {
        for optimization in [Optimization::O0, Optimization::O1] {
            let program = vole_c::compile(architecture, source, &settings(optimization))
                .unwrap_or_else(|errors| panic!("{architecture:?}: {errors:?}"));
            let debug = program.debug.as_ref().unwrap();
            let total = debug.globals.iter().find(|g| g.name == "total").unwrap();
            assert!(
                matches!(total.location, vole_core::debug::Location::Address(a) if (0xA000..0x10000).contains(&a)),
                "{architecture:?}: {:?}",
                total.location
            );
            run(&program, 10_000).unwrap();
        }
    }
}
