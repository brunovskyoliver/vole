//! Runs Clang-compiled C programs (tests/corpus/*.c) on every scalar target at
//! -O0 and -O1, linked with the teaching runtime, and compares their results
//! with independent Rust models of the same computations.
//!
//! The programs are compiled with the flags of docs/c-environment.md and linked
//! with LLD into the contract memory map. If Clang, LLD or the runtime sources
//! are missing the test prints why and passes without running.
use object::{Object, ObjectSection, ObjectSymbol};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};
use vole_core::{Architecture, Machine, MemoryRegion, Program};
use vole_isa_scalar::ScalarMachine;

const COMMON_FLAGS: &[&str] = &[
    "-std=c17",
    "-ffreestanding",
    "-fno-builtin",
    "-nostdlibinc",
    "-fno-pic",
    "-fno-pie",
    "-fno-stack-protector",
    "-fno-exceptions",
    "-fno-asynchronous-unwind-tables",
    "-fno-unwind-tables",
    "-fno-omit-frame-pointer",
    "-fno-common",
    "-fno-vectorize",
    "-fno-slp-vectorize",
    "-gdwarf-5",
    "-fno-color-diagnostics",
    "-Wno-format",
];
const LINK_SCRIPT: &str = "ENTRY(_start)\nSECTIONS {\n . = 0x1000; .text : { *(.text*) }\n ASSERT(SIZEOF(.text) <= 0x7000, \"code exceeds 28 KiB\")\n . = 0x8000; .rodata : { *(.rodata*) }\n . = 0xA000; .data : { *(.data*) }\n .bss : { *(.bss*) *(COMMON) }\n ASSERT(. <= 0x10000, \"data exceeds 24 KiB\")\n /DISCARD/ : { *(.comment) *(.note*) *(.ARM.exidx*) *(.ARM.extab*) *(.eh_frame*) }\n}\n";
const STEP_BUDGET: u64 = 20_000_000;

fn find_tool(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    names.iter().find_map(|name| {
        std::env::split_paths(&path)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

struct Toolchain {
    clang: PathBuf,
    lld: PathBuf,
    runtime: PathBuf,
    corpus: PathBuf,
}

fn toolchain() -> Option<Toolchain> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let runtime = manifest.join("../vole-c/runtime");
    let Some(clang) = std::env::var_os("VOLE_CLANG")
        .map(PathBuf::from)
        .or_else(|| find_tool(&["clang-14", "clang"]))
    else {
        eprintln!("SKIPPED: clang-14/clang not found on PATH (set VOLE_CLANG)");
        return None;
    };
    let Some(lld) = find_tool(&["ld.lld-14", "ld.lld"]) else {
        eprintln!("SKIPPED: ld.lld not found on PATH");
        return None;
    };
    if !runtime.join("vole_runtime.c").is_file() {
        eprintln!(
            "SKIPPED: runtime sources not found at {}",
            runtime.display()
        );
        return None;
    }
    Some(Toolchain {
        clang,
        lld,
        runtime,
        corpus: manifest.join("tests/corpus"),
    })
}

fn target_flags(architecture: Architecture) -> &'static [&'static str] {
    match architecture {
        Architecture::Arm64 => &["--target=aarch64-none-elf", "-mgeneral-regs-only"],
        Architecture::X64 => &["--target=x86_64-none-elf", "-mgeneral-regs-only"],
        Architecture::Arm32 => &[
            "--target=armv7a-none-eabi",
            "-marm",
            "-mfloat-abi=soft",
            "-mno-unaligned-access",
        ],
        Architecture::X86 => &["--target=i386-none-elf", "-mgeneral-regs-only"],
        Architecture::Vole => unreachable!(),
    }
}

fn run(command: &mut Command) {
    let output = command.output().expect("tool starts");
    assert!(
        output.status.success(),
        "{command:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn compile(
    tools: &Toolchain,
    directory: &Path,
    architecture: Architecture,
    source: &Path,
    optimization: &str,
) -> PathBuf {
    let object = directory.join(format!(
        "{}-{}-{optimization}.o",
        source.file_stem().unwrap().to_string_lossy(),
        architecture.id()
    ));
    run(Command::new(&tools.clang)
        .args(target_flags(architecture))
        .args(COMMON_FLAGS)
        .arg(format!("-{optimization}"))
        .arg("-I")
        .arg(&tools.runtime)
        .arg("-c")
        .arg(source)
        .arg("-o")
        .arg(&object));
    object
}

/// Load a linked ELF into the contract memory map.
fn image(architecture: Architecture, elf: &[u8]) -> Program {
    let file = object::File::parse(elf).expect("ELF");
    let mut code = vec![];
    let mut rodata = vec![0; 0x2000];
    let mut data = vec![0; 0x6000];
    for section in file.sections() {
        let address = section.address();
        let Ok(bytes) = section.uncompressed_data() else {
            continue;
        };
        if section.name() == Ok(".text") {
            assert_eq!(address, 0x1000);
            code = bytes.to_vec();
        } else if (0x8000..0xa000).contains(&address) && !bytes.is_empty() {
            let start = (address - 0x8000) as usize;
            rodata[start..start + bytes.len()].copy_from_slice(&bytes);
        } else if (0xa000..0x10000).contains(&address) && section.name() != Ok(".bss") {
            let start = (address - 0xa000) as usize;
            data[start..start + bytes.len()].copy_from_slice(&bytes);
        }
    }
    let symbols: BTreeMap<String, u64> = file
        .symbols()
        .filter_map(|symbol| Some((symbol.name().ok()?.to_string(), symbol.address())))
        .collect();
    let region = |base, bytes, writable, executable, label: &str| MemoryRegion {
        base,
        bytes,
        writable,
        executable,
        label: label.into(),
    };
    let stack = match architecture {
        Architecture::Arm32 => "sp",
        Architecture::Arm64 => "sp",
        Architecture::X86 => "esp",
        _ => "rsp",
    };
    Program {
        architecture,
        source: String::new(),
        entry: file.entry(),
        regions: vec![
            region(0x1000, code, false, true, "Code"),
            region(0x8000, rodata, false, false, "Read-only data"),
            region(0xa000, data, true, false, "Data"),
            region(0x10000, vec![0; 0x10000], true, false, "Stack"),
        ],
        instructions: vec![],
        symbols,
        initial_registers: BTreeMap::from([(stack.to_string(), 0x20000)]),
        language: Default::default(),
        debug: None,
    }
}

struct Outcome {
    status: u64,
    output: Vec<u8>,
    results: Vec<u64>,
    steps: u64,
}

fn execute(program: &Program, name: &str) -> Outcome {
    let mut machine = ScalarMachine::new(program.architecture);
    machine.load(program).unwrap();
    while !machine.halted() {
        if let Err(error) = machine.step() {
            panic!(
                "{name}: fault after {} steps at 0x{:x}: {error}",
                machine.steps(),
                machine.pc()
            );
        }
        assert!(
            machine.steps() < STEP_BUDGET,
            "{name}: step budget exhausted"
        );
    }
    let status_register = match program.architecture {
        Architecture::Arm32 => "r0",
        Architecture::Arm64 => "x0",
        Architecture::X86 => "ebx",
        _ => "rdi",
    };
    let results = match program.symbols.get("results") {
        Some(address) => {
            let count_address = program.symbols["result_count"];
            let count = u32::from_le_bytes(
                machine
                    .read_memory(count_address, 4)
                    .unwrap()
                    .try_into()
                    .unwrap(),
            ) as usize;
            let bytes = machine.read_memory(*address, count * 8).unwrap();
            bytes
                .chunks(8)
                .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
                .collect()
        }
        None => vec![],
    };
    Outcome {
        status: machine.read_register(status_register).unwrap() & 0xffff_ffff,
        output: machine.snapshot().output,
        results,
        steps: machine.steps(),
    }
}

// ---- Independent models -------------------------------------------------

/// Convert to a C integer type of `bits` width and signedness.
fn cast(value: i128, bits: u32, signed: bool) -> i128 {
    let wrapped = value.rem_euclid(1_i128 << bits);
    if signed && wrapped >= 1_i128 << (bits - 1) {
        wrapped - (1_i128 << bits)
    } else {
        wrapped
    }
}

fn arith_model(long_bits: u32) -> Vec<u64> {
    let left: [i128; 6] = [100, -1234567, 0x7fffffff, -5, 0x123456789abcdef, 255];
    let right: [i128; 6] = [7, 89, -3, -3, 0x1000000003, 13];
    let types = [
        (8, true),
        (8, false),
        (16, true),
        (16, false),
        (32, true),
        (32, false),
        (long_bits, true),
        (long_bits, false),
        (64, true),
        (64, false),
    ];
    let mut out = vec![];
    for (l, r) in left.iter().zip(right) {
        for (bits, signed) in types {
            let a = cast(*l, bits, signed);
            let b = cast(r, bits, signed);
            let promoted_bits = bits.max(32);
            let t = |v: i128| cast(v, bits, signed) as u64;
            out.push(t(a + b));
            out.push(t(a - b));
            out.push(t(a.wrapping_mul(b)));
            out.push(t(a / b));
            out.push(t(a % b));
            out.push(t(a & b));
            out.push(t(a | b));
            out.push(t(a ^ b));
            out.push(t(!a));
            out.push(t(-a));
            let s = (cast(b, 32, false) as u32) % promoted_bits;
            out.push(t(a.wrapping_shl(s)));
            out.push(t(a >> s));
            let flags = [
                a < b,
                a <= b,
                a > b,
                a >= b,
                a == b,
                a != b,
                a == 0,
                a != 0 && b != 0,
            ];
            out.push(
                flags
                    .iter()
                    .enumerate()
                    .map(|(bit, set)| u64::from(*set) << bit)
                    .sum(),
            );
        }
    }
    out
}

fn control_model() -> Vec<u64> {
    let seed = 7_i32;
    let wide: i64 = -123456789012;
    let mut out = vec![];
    let table = [11, 23, 37, 41, 59, 61, 73, 89, 97];
    for i in -1..10 {
        let value: i64 = if (0..9).contains(&i) {
            table[i as usize]
        } else {
            -1
        };
        out.push(value as u64);
    }
    let sparse = |x: u32| match x {
        3 => 1,
        100 => 2,
        1000 => 3,
        65536 => 4,
        u32::MAX => 5,
        _ => 0,
    };
    out.push(
        sparse(3)
            + 10 * sparse(100)
            + 100 * sparse(1000)
            + 1000 * sparse(65536)
            + 10000 * sparse(u32::MAX)
            + 100000 * sparse(seed as u32),
    );
    fn fib(n: u32) -> u32 {
        if n < 2 { n } else { fib(n - 1) + fib(n - 2) }
    }
    out.push(u64::from(fib(seed as u32 + 8)));
    let (mut a, mut b) = (1071 * seed, 462 * seed);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    out.push(a as u64);
    out.push(
        (0..200_u64)
            .filter(|n| *n >= 2 && (2..*n).all(|d| n % d != 0))
            .count() as u64,
    );
    out.push((i64::from(-seed * 100000) * 300000) as u64);
    out.push(u64::from(0xfffffff0_u32 + seed as u32) * 0xffffff00);
    for i in (0..64).step_by(13) {
        let s = (i + seed as u32) & 63;
        out.push((wide as u64) << s);
        out.push((wide as u64) >> s);
        out.push((wide >> s) as u64);
    }
    let compare = |a: i64, b: i64| {
        u64::from(a < b) + 2 * u64::from(a == b) + 4 * u64::from((a as u64) < (b as u64))
    };
    out.push(compare(wide, 5));
    out.push(compare(5, wide));
    out.push(compare(wide, wide));
    let (mut n, mut total) = (seed * 3, 0);
    loop {
        if n % 3 == 0 {
            n -= 1;
        } else if n == 2 {
            break;
        } else {
            total += n;
            n -= 1;
        }
        if n <= 0 {
            break;
        }
    }
    out.push(total as u64);
    let (mut n, mut steps) = (seed * 3, 0);
    while n != 1 {
        n = if n & 1 == 1 { 3 * n + 1 } else { n / 2 };
        steps += 1;
    }
    out.push(steps);
    let casts = |v: i64| {
        let sum = i64::from(v as i8)
            + i64::from(v as u8)
            + i64::from(v as i16)
            + i64::from(v as u16)
            + i64::from(v as i32)
            + i64::from(v as u32)
            + i64::from(v & 0x100 != 0);
        sum as u64
    };
    out.push(casts(wide));
    out.push(casts(0x1ff80));
    for value in [11_i64, -28, 3] {
        out.push(value as u64);
    }
    out
}

fn memory_model() -> Vec<u64> {
    let scale = 3_i64;
    let table: Vec<Vec<i64>> = (0..3)
        .map(|i| (0..4).map(|k| 4 * i + k + 1).collect())
        .collect();
    let b: Vec<Vec<i64>> = (0..4)
        .map(|i| (0..3).map(|j| (i + 1) * scale - j).collect())
        .collect();
    let mut out = vec![];
    let mut product = [[0_i64; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            product[i][j] = (0..4).map(|k| table[i][k] * b[k][j]).sum();
            out.push(product[i][j] as u64);
        }
    }
    out.push((-300_i64) as u64);
    out.push((-299_i64) as u64);
    out.push((-300_i64 * 70000) as u64);
    out.push(u64::from(b'q'));
    out.push(19 * 3 + 1);
    out.push(19 * 3);
    out.push(70000);
    let word = (0x11223344_u32 * 3).to_le_bytes();
    out.push(
        u64::from(word[0])
            + (u64::from(word[3]) << 8)
            + (u64::from(u16::from_le_bytes([word[2], word[3]])) << 16),
    );
    out.push(b"Vole C!".iter().map(|b| u64::from(*b)).sum());
    let mut text: Vec<u8> = b"Vole C!".iter().rev().copied().collect();
    text.push(0);
    out.push(u64::from(text[0]) | u64::from(text[6]) << 8);
    text[..4].fill(b'z');
    let moved = text[0..6].to_vec();
    text[1..7].copy_from_slice(&moved);
    out.push(u64::from(text[0]) | u64::from(text[4]) << 8 | u64::from(text[6]) << 16);
    out.push(11 + 12 * 100);
    out.push((product[2][1] - product[0][2]) as u64);
    out.push(8);
    out
}

fn idioms_model(architecture: Architecture) -> Vec<u64> {
    let _ = architecture;
    let ivalues = [-1000003_i32, 77, 0x7fffffff, i32::MIN, 0, 12345];
    let lvalues = [-99999999999_i64, 1234567890123, i64::MAX, 3];
    let mut out = vec![];
    // struct flags { unsigned low:3; signed mid:7; unsigned high:12; unsigned top:10; }
    let (mut low, mut mid, mut high, mut top) = (1_u32, 2_i32, 3_u32, 1023_u32);
    for (i, x) in ivalues.iter().copied().enumerate() {
        let i = i as u32;
        let u = x as u32;
        out.push(i64::from(x / 7) as u64);
        out.push(i64::from(x % 10) as u64);
        out.push(u64::from(u / 10));
        out.push(u64::from(u % 3));
        out.push(i64::from(x / 8) as u64);
        let s = i * 7;
        out.push(u64::from(
            u << (s & 31) | u >> (32_u32.wrapping_sub(s) & 31),
        ));
        out.push(u64::from(u.swap_bytes()));
        out.push(u64::from(if u == 0 { 32 } else { u.leading_zeros() }));
        out.push(i64::from(x.min(5)) as u64 ^ u64::from(u.max(1000)) << 32);
        let magnitude = if x == i32::MIN { 1 } else { x.abs() };
        out.push((i64::from(magnitude) as u64).wrapping_add((i64::from(x.signum()) * 1000) as u64));
        low = u & 7;
        mid = ((x >> 3) << 25) >> 25;
        high = (u >> 10) & 0xfff;
        top = (top + 1) & 0x3ff;
        out.push(u64::from(
            low.wrapping_add((mid * 1000) as u32)
                .wrapping_add(high * 7)
                .wrapping_add(top),
        ));
        let lower = (u.wrapping_add(97) as u8).is_ascii_lowercase();
        let swapped = (u as u16).rotate_left(8);
        let narrow = u.wrapping_mul(3) as u8;
        out.push(u64::from(lower) + u64::from(swapped) * 2 + (u64::from(narrow) << 20));
    }
    let _ = (low, mid, high);
    for (i, x) in lvalues.iter().copied().enumerate() {
        let s = i as u32 * 21;
        let u = x as u64;
        out.push((x / 7) as u64);
        out.push(u / 10);
        out.push((x % 1000) as u64);
        out.push(u >> (s & 63) | u << (64_u32.wrapping_sub(s) & 63));
        out.push(u64::from(u.count_ones()));
        out.push(x.unsigned_abs());
        out.push(((u128::from(u) * 0x9e3779b97f4a7c15_u128) >> 64) as u64);
    }
    out
}

const EXPECTED_OUTPUT: &str = "-42 17 3000000000 beef BEEF 10 V ok %\n[  -42][-42  ][-0042][  a][b  ]\n-42 99 -81985529216486895 81985529216486895 123456789abcdef\n4464 44 4464 44 12\ndone\n-7 0xff\n";

#[test]
fn clang_compiled_corpus_matches_independent_models() {
    let Some(tools) = toolchain() else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("link.ld");
    std::fs::write(&script, LINK_SCRIPT).unwrap();
    let mut total_steps = 0;
    for architecture in [
        Architecture::Arm64,
        Architecture::X64,
        Architecture::Arm32,
        Architecture::X86,
    ] {
        let runtime = compile(
            &tools,
            directory.path(),
            architecture,
            &tools.runtime.join("vole_runtime.c"),
            "O0",
        );
        let crt0 = directory
            .path()
            .join(format!("crt0-{}.o", architecture.id()));
        run(Command::new(&tools.clang)
            .args(target_flags(architecture))
            .arg("-c")
            .arg(tools.runtime.join(format!("crt0_{}.s", architecture.id())))
            .arg("-o")
            .arg(&crt0));
        let long_bits = u32::from(architecture.bits());
        for optimization in ["O0", "O1"] {
            let mut extra = vec![];
            if architecture == Architecture::Arm32 {
                extra.push(compile(
                    &tools,
                    directory.path(),
                    architecture,
                    &tools.corpus.join("aeabi_mem.c"),
                    optimization,
                ));
            }
            for (name, expected) in [
                ("arith", Some(arith_model(long_bits))),
                ("control", Some(control_model())),
                ("memory", Some(memory_model())),
                ("idioms", Some(idioms_model(architecture))),
                ("output", None),
            ] {
                let label = format!("{name} {} -{optimization}", architecture.id());
                let main = compile(
                    &tools,
                    directory.path(),
                    architecture,
                    &tools.corpus.join(format!("{name}.c")),
                    optimization,
                );
                let elf = directory
                    .path()
                    .join(format!("{label}.elf").replace(' ', "_"));
                run(Command::new(&tools.lld)
                    .arg("-T")
                    .arg(&script)
                    .arg(&crt0)
                    .arg(&main)
                    .arg(&runtime)
                    .args(&extra)
                    .arg("-o")
                    .arg(&elf));
                let program = image(architecture, &std::fs::read(&elf).unwrap());
                let outcome = execute(&program, &label);
                total_steps += outcome.steps;
                match expected {
                    Some(expected) => {
                        for (index, (actual, wanted)) in
                            outcome.results.iter().zip(&expected).enumerate()
                        {
                            assert_eq!(
                                actual, wanted,
                                "{label}: result {index} is 0x{actual:x}, expected 0x{wanted:x}"
                            );
                        }
                        assert_eq!(outcome.results.len(), expected.len(), "{label}");
                        assert_eq!(outcome.status, expected.len() as u64, "{label}");
                    }
                    None => {
                        assert_eq!(
                            String::from_utf8_lossy(&outcome.output),
                            EXPECTED_OUTPUT,
                            "{label}"
                        );
                        assert_eq!(outcome.status, 3, "{label}");
                    }
                }
            }
        }
    }
    eprintln!("corpus executed {total_steps} guest instructions");
}
