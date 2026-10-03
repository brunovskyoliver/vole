//! Instruction fixtures for the forms Clang emits for the freestanding C scope.
//!
//! Every expected register, flag and memory value below was computed by hand
//! from the Arm ARM (DDI 0487 A64, DDI 0406 A32) and the Intel SDM instruction
//! pages, never by running the interpreter. Each case also checks that reverse
//! execution restores the exact initial state.
use vole_core::{Architecture, Machine, MemoryRegion, Program, Snapshot};
use vole_isa_scalar::{ScalarMachine, assemble};

fn program(architecture: Architecture, body: &str) -> Program {
    let header = match architecture {
        Architecture::Arm32 => ".syntax unified\n.arm\n",
        Architecture::X86 | Architecture::X64 => ".intel_syntax noprefix\n",
        _ => "",
    };
    assemble(
        architecture,
        &format!("{header}.text\n.global _start\n_start:\n{body}\n"),
    )
    .unwrap_or_else(|errors| panic!("{architecture:?}: {errors:?}\n{body}"))
}

fn halt(architecture: Architecture) -> &'static str {
    match architecture {
        Architecture::Arm32 => "bkpt #0",
        Architecture::Arm64 => "brk #0",
        _ => "int3",
    }
}

/// Run to the halt, check reverse execution back to the initial state, and
/// return the halted snapshot.
fn run(architecture: Architecture, body: &str) -> Snapshot {
    let (code, data) = body.split_once(".data").unwrap_or((body, ""));
    let source = if data.is_empty() {
        format!("{code}\n{}", halt(architecture))
    } else {
        format!("{code}\n{}\n.data{data}", halt(architecture))
    };
    let image = program(architecture, &source);
    let mut machine = ScalarMachine::new(architecture);
    machine.load(&image).unwrap();
    let initial = machine.snapshot();
    while !machine.halted() {
        machine
            .step()
            .unwrap_or_else(|error| panic!("{architecture:?} {body}: {error}"));
        assert!(machine.steps() < 10_000, "{body} did not halt");
    }
    let halted = machine.snapshot();
    while machine.steps() > 0 {
        machine.reverse_step().unwrap();
    }
    assert_eq!(
        machine.snapshot(),
        initial,
        "reverse of {architecture:?} {body}"
    );
    halted
}

fn flags(snapshot: &Snapshot) -> String {
    let order: &[(&str, char)] = if snapshot.flags.contains_key("N") {
        &[("N", 'n'), ("Z", 'z'), ("C", 'c'), ("V", 'v')]
    } else {
        &[
            ("CF", 'c'),
            ("PF", 'p'),
            ("AF", 'a'),
            ("ZF", 'z'),
            ("SF", 's'),
            ("OF", 'o'),
        ]
    };
    order
        .iter()
        .map(|(name, letter)| {
            if snapshot.flags[*name] {
                letter.to_ascii_uppercase()
            } else {
                *letter
            }
        })
        .collect()
}

fn address_of(image: &Program, assembly: &str) -> u64 {
    image
        .instructions
        .iter()
        .find(|instruction| instruction.assembly == assembly)
        .unwrap_or_else(|| panic!("{assembly} not assembled"))
        .address
}

type Case = (
    &'static str,
    &'static [(&'static str, u64)],
    Option<&'static str>,
);

fn check(architecture: Architecture, cases: &[Case]) {
    for (body, registers, expected_flags) in cases {
        let snapshot = run(architecture, body);
        for (name, value) in *registers {
            assert_eq!(
                snapshot.register(name),
                Some(*value),
                "{architecture:?} `{body}`: {name} = 0x{:x}, expected 0x{value:x}",
                snapshot.register(name).unwrap_or(0)
            );
        }
        if let Some(expected) = expected_flags {
            assert_eq!(
                flags(&snapshot),
                *expected,
                "{architecture:?} flags of `{body}`"
            );
        }
    }
}

/// Step every instruction before the last of `body`, then check that the last
/// faults with `message` and leaves the whole machine unchanged.
fn assert_atomic_fault(image: &Program, prefix_steps: usize, message: &str) {
    let mut machine = ScalarMachine::new(image.architecture);
    machine.load(image).unwrap();
    for _ in 0..prefix_steps {
        machine.step().unwrap();
    }
    let before = machine.snapshot();
    let error = machine.step().unwrap_err();
    assert!(
        error.0.contains(message),
        "{:?}: `{}` should contain `{message}`",
        image.architecture,
        error.0
    );
    assert_eq!(
        machine.snapshot(),
        before,
        "{:?} fault was not atomic",
        image.architecture
    );
}

/// Add a read-only page at 0x30008 after eight writable bytes at 0x30000.
fn with_read_only_boundary(mut image: Program) -> Program {
    image.regions.push(MemoryRegion {
        base: 0x30000,
        bytes: vec![0; 8],
        writable: true,
        executable: false,
        label: "Writable".into(),
    });
    image.regions.push(MemoryRegion {
        base: 0x30008,
        bytes: vec![0xaa; 8],
        writable: false,
        executable: false,
        label: "Read-only data".into(),
    });
    image
}

// ---------------------------------------------------------------- ARM64 --

#[test]
fn arm64_division_follows_a64_rules_for_zero_and_overflow() {
    check(
        Architecture::Arm64,
        &[
            (
                "mov w0, #-7\nmov w1, #2\nsdiv w2, w0, w1\nudiv w3, w0, w1",
                &[("x2", 0xffff_fffd), ("x3", 0x7fff_fffc)],
                None,
            ),
            (
                "mov x2, #5\nmov x3, #5\nmov x0, #77\nmov x1, #0\nsdiv x2, x0, x1\nudiv w3, w0, w1",
                &[("x2", 0), ("x3", 0)],
                None,
            ),
            (
                "mov w0, #0x80000000\nmov w1, #-1\nsdiv w2, w0, w1\nmov x3, #0x8000000000000000\nmov x4, #-1\nsdiv x5, x3, x4",
                &[("x2", 0x8000_0000), ("x5", 0x8000_0000_0000_0000)],
                None,
            ),
            (
                "mov x0, #100\nmov x1, #-7\nsdiv x2, x0, x1\nmsub x3, x2, x1, x0",
                &[("x2", 0xffff_ffff_ffff_fff2), ("x3", 2)],
                None,
            ),
        ],
    );
}

#[test]
fn arm64_multiplies_long_and_high_products() {
    check(
        Architecture::Arm64,
        &[(
            "mov x0, #-3\nmov x1, #5\nmov x4, #100\nmov x9, #-1\nmul x2, x0, x1\nmneg x3, x0, x1\nmadd x5, x0, x1, x4\nmsub x6, x0, x1, x4\nsmull x7, w0, w1\numull x8, w0, w1\nsmulh x10, x9, x9\numulh x11, x9, x9\nsmulh x12, x9, x1\nsmaddl x13, w0, w1, x4\numaddl x14, w0, w1, x4\nsmnegl x15, w0, w1\nmul w16, w0, w1\numsubl x17, w1, w1, x4",
            &[
                ("x2", 0xffff_ffff_ffff_fff1),
                ("x3", 15),
                ("x5", 85),
                ("x6", 115),
                ("x7", 0xffff_ffff_ffff_fff1),
                ("x8", 0x4_ffff_fff1),
                ("x10", 0),
                ("x11", 0xffff_ffff_ffff_fffe),
                ("x12", u64::MAX),
                ("x13", 85),
                ("x14", 0x5_0000_0055),
                ("x15", 15),
                ("x16", 0xffff_fff1),
                ("x17", 75),
            ],
            None,
        )],
    );
}

#[test]
fn arm64_conditional_select_and_compare_families() {
    check(
        Architecture::Arm64,
        &[(
            // 5 - 9: N=1 Z=0 C=0 V=0.
            "mov x0, #5\nmov x1, #9\ncmp x0, x1\ncsel x2, x0, x1, lt\ncsel x3, x0, x1, ge\ncsinc x4, x0, x1, eq\ncsinv x5, x0, x1, eq\ncsneg x6, x0, x1, eq\ncset w7, lo\ncsetm x8, hi\ncinc x9, x0, ne\ncinv x10, x0, eq\ncneg x11, x1, mi\nccmp x0, #5, #2, ne\ncset w12, eq\nccmp x0, x1, #9, eq\nccmp x0, x1, #9, eq\ncset w13, vs\nccmn x0, #5, #0, mi\ncset w14, pl\ncsetm w15, eq",
            &[
                ("x2", 5),
                ("x3", 9),
                ("x4", 10),
                ("x5", !9),
                ("x6", (-9_i64 as u64)),
                ("x7", 1),
                ("x8", 0),
                ("x9", 6),
                ("x10", 5),
                ("x11", (-9_i64 as u64)),
                ("x12", 1),
                ("x13", 1),
                ("x14", 1),
                ("x15", 0),
            ],
            Some("nzcv"),
        )],
    );
}

#[test]
fn arm64_bitfield_extract_insert_shift_and_bit_operations() {
    check(
        Architecture::Arm64,
        &[(
            "mov x0, #0x1234\nmovk x0, #0xfedc, lsl #48\nubfx x1, x0, #4, #8\nsbfx x2, x0, #60, #4\nsbfx w3, w0, #8, #5\nubfiz x4, x0, #8, #12\nsbfiz w5, w0, #4, #4\nmov x6, #-1\nbfi x6, x0, #16, #8\nmov x7, #0\nbfxil x7, x0, #8, #8\nmov w8, #-1\nbfc w8, #4, #8\nextr x9, x0, x0, #16\nmov x10, #0x80\nlsl x11, x10, #56\nasr x12, x0, #4\nlsr x13, x0, #60\nmov x14, #68\nlsl x15, x10, x14\nror w16, w0, w14\nsxtb x17, w0\nmov w18, #0x80\nsxtb w19, w18\nsxth x20, w0\nuxtb w21, w6\nsxtw x22, w8\nrev x23, x0\nrev16 w24, w0\nrev32 x25, x0\nrbit w26, w10\nclz x27, x10\ncls x28, x6",
            &[
                ("x0", 0xfedc_0000_0000_1234),
                ("x1", 0x23),
                ("x2", u64::MAX),
                ("x3", 0xffff_fff2),
                ("x4", 0x23400),
                ("x5", 0x40),
                ("x6", 0xffff_ffff_ff34_ffff),
                ("x7", 0x12),
                ("x8", 0xffff_f00f),
                ("x9", 0x1234_fedc_0000_0000),
                ("x11", 0x8000_0000_0000_0000),
                ("x12", 0xffed_c000_0000_0123),
                ("x13", 0xf),
                ("x15", 0x800),
                ("x16", 0x4000_0123),
                ("x17", 0x34),
                ("x19", 0xffff_ff80),
                ("x20", 0x1234),
                ("x21", 0xff),
                ("x22", 0xffff_ffff_ffff_f00f),
                ("x23", 0x3412_0000_0000_dcfe),
                ("x24", 0x3412),
                ("x25", 0x0000_dcfe_3412_0000),
                ("x26", 0x0100_0000),
                ("x27", 56),
                ("x28", 39),
            ],
            None,
        )],
    );
}

#[test]
fn arm64_flag_setting_carry_chains_and_extended_operands() {
    check(
        Architecture::Arm64,
        &[
            ("mov x0, #-1\nadds x1, x0, #1", &[("x1", 0)], Some("nZCv")),
            (
                "mov x0, #-1\nadds x1, x0, #1\nadcs x2, xzr, xzr",
                &[("x2", 1)],
                Some("nzcv"),
            ),
            (
                "mov x3, #0x7fffffffffffffff\nadds x4, x3, #1",
                &[("x4", 0x8000_0000_0000_0000)],
                Some("NzcV"),
            ),
            ("mov w6, #0\nnegs w7, w6", &[("x7", 0)], Some("nZCv")),
            (
                "mov w8, #0x80000000\nnegs w9, w8",
                &[("x9", 0x8000_0000)],
                Some("NzcV"),
            ),
            (
                "mov x0, #5\nmov x1, #3\ncmp x0, x0\nsbcs x2, x0, x1",
                &[("x2", 2)],
                Some("nzCv"),
            ),
            (
                "mov x0, #3\nmov x1, #5\ncmp x0, x1\nsbcs x2, x0, x1",
                &[("x2", (-3_i64 as u64))],
                Some("Nzcv"),
            ),
            (
                "mov x9, #0\ncmp x9, #1\nmov x6, #0\nngc x10, x6",
                &[("x10", u64::MAX)],
                Some("Nzcv"),
            ),
            (
                "mov x0, #-1\nmov x3, #0x7fffffffffffffff\nmov w11, #-2\nadd x12, x3, w11, sxtw\nadd x13, x0, w11, uxtw #2\nadd x14, x0, w11, uxtb\nsub x15, x3, w11, sxtw #1",
                &[
                    ("x11", 0xffff_fffe),
                    ("x12", 0x7fff_ffff_ffff_fffd),
                    ("x13", 0x3_ffff_fff7),
                    ("x14", 0xfd),
                    ("x15", 0x8000_0000_0000_0003),
                ],
                None,
            ),
            ("mov x13, #0x2000\ncmp x13, #2, lsl #12", &[], Some("nZCv")),
            (
                "cmp xzr, xzr\nmov x3, #0x7fffffffffffffff\ntst x3, #0x8000000000000000",
                &[],
                Some("nZcv"),
            ),
            (
                "cmp xzr, xzr\nmov w11, #-2\nands w15, w11, #0x80000001",
                &[("x15", 0x8000_0000)],
                Some("Nzcv"),
            ),
            (
                "mov x0, #-1\nmov x3, #0x7fffffffffffffff\nbics x16, x0, x3",
                &[("x16", 0x8000_0000_0000_0000)],
                Some("Nzcv"),
            ),
            ("mov w8, #0x80000000\ncmn w8, w8", &[], Some("nZCV")),
            (
                "mov x0, #0x00ff\nmvn x1, x0, lsl #8\neor x2, x0, x0, ror #4\norn w3, wzr, w0\neon x4, x0, x0",
                &[
                    ("x1", 0xffff_ffff_ffff_00ff),
                    ("x2", 0xf000_0000_0000_00f0),
                    ("x3", 0xffff_ff00),
                    ("x4", u64::MAX),
                ],
                None,
            ),
        ],
    );
}

#[test]
fn arm64_loads_stores_extends_writeback_and_page_addresses() {
    let body = "adrp x0, values\nadd x0, x0, :lo12:values\nldrsw x1, [x0]\nldrsh w2, [x0, #4]\nldrsh x3, [x0, #4]\nldrsb w4, [x0, #6]\nldurb w5, [x0, #7]\nadd x6, x0, #8\nldursb x7, [x6, #-2]\nldursh w8, [x6, #-4]\nmov x9, #2\nldrh w10, [x0, x9, lsl #1]\nmov w11, #-1\nldrb w12, [x6, w11, sxtw]\nmov w13, #1\nldr w14, [x0, w13, uxtw #2]\nldr x15, literal\nstp w11, w13, [x0, #16]\nldpsw x16, x17, [x0, #16]\nstr w13, [x0, #24]!\nldr w18, [x0], #-24\nstrh w11, [x0, #28]\nsturb w13, [x6, #-1]\nb over\n.p2align 3\nliteral: .quad 0x1122334455667788\nover:\n.data\nvalues: .word 0xfffffff0\n.hword 0x8001\n.byte 0x80, 0x7f\n.space 32";
    let snapshot = run(Architecture::Arm64, body);
    for (name, value) in [
        ("x0", 0x2000),
        ("x1", 0xffff_ffff_ffff_fff0),
        ("x2", 0xffff_8001),
        ("x3", 0xffff_ffff_ffff_8001),
        ("x4", 0xffff_ff80),
        ("x5", 0x7f),
        ("x6", 0x2008),
        ("x7", 0xffff_ffff_ffff_ff80),
        ("x8", 0xffff_8001),
        ("x10", 0x8001),
        ("x11", 0xffff_ffff),
        ("x12", 0x7f),
        ("x14", 0x7f80_8001),
        ("x15", 0x1122_3344_5566_7788),
        ("x16", u64::MAX),
        ("x17", 1),
        ("x18", 1),
    ] {
        assert_eq!(snapshot.register(name), Some(value), "{name}");
    }
    assert_eq!(
        snapshot.read(0x2010, 14).unwrap(),
        [0xff, 0xff, 0xff, 0xff, 1, 0, 0, 0, 1, 0, 0, 0, 0xff, 0xff]
    );
    assert_eq!(snapshot.byte(0x2007), Some(1));
}

#[test]
fn arm64_register_branches_jump_tables_and_bit_tests() {
    let body = "adr x0, target\nbr x0\nmov x1, #1\ntarget:\nadr x2, function\nblr x2\nreturn_here:\nmov x3, #3\nmov x4, #5\ntbz x4, #1, skip1\nmov x5, #5\nskip1:\ntbnz w4, #2, skip2\nmov x6, #6\nskip2:\ncbnz x4, skip3\nmov x7, #7\nskip3:\nadr x20, table\nmov x21, #2\nldr x22, [x20, x21, lsl #3]\nbr x22\ncase0:\nmov x23, #10\nb done\ncase1:\nmov x23, #11\nb done\ncase2:\nmov x23, #12\nb done\nfunction:\nmov x8, #8\nmov x9, x30\nret x9\n.p2align 3\ntable: .quad case0, case1, case2\ndone:";
    let image = program(Architecture::Arm64, &format!("{body}\nbrk #0"));
    let return_here = address_of(&image, "mov x3, #3");
    let snapshot = run(Architecture::Arm64, body);
    for (name, value) in [
        ("x1", 0),
        ("x3", 3),
        ("x5", 0),
        ("x6", 0),
        ("x7", 0),
        ("x8", 8),
        ("x9", return_here),
        ("x30", return_here),
        ("x23", 12),
    ] {
        assert_eq!(snapshot.register(name), Some(value), "{name}");
    }
}

#[test]
fn arm64_faults_are_atomic_for_pair_store_into_read_only_data() {
    let image = with_read_only_boundary(program(
        Architecture::Arm64,
        "mov x0, #1\nmov x1, #2\nmov x2, #0x30000\nstr x0, [x2]\nstp x0, x1, [x2], #16\nbrk #0",
    ));
    assert_atomic_fault(&image, 4, "read-only");
    let image = with_read_only_boundary(program(
        Architecture::Arm64,
        "mov x2, #0x30000\nadd x2, x2, #8\nldp x0, x1, [x2, #0]!\nbrk #0",
    ));
    assert_atomic_fault(&image, 2, "Unmapped");
}

// ---------------------------------------------------------------- ARM32 --

#[test]
fn arm32_multiplies_long_accumulates_and_most_significant_word() {
    check(
        Architecture::Arm32,
        &[
            (
                "mov r0, #-3\nmov r1, #5\nmov r3, #100\nmul r2, r0, r1\nmla r4, r0, r1, r3\nmls r5, r0, r1, r3\numull r6, r7, r0, r1\nsmull r8, r9, r0, r1",
                &[
                    ("r2", 0xffff_fff1),
                    ("r4", 85),
                    ("r5", 115),
                    ("r6", 0xffff_fff1),
                    ("r7", 4),
                    ("r8", 0xffff_fff1),
                    ("r9", 0xffff_ffff),
                ],
                Some("nzcv"),
            ),
            (
                "mvn r0, #0\nmvn r1, #0\nmov r2, #1\nmov r3, #2\numlal r2, r3, r0, r1\nmvn r4, #0\nmvn r5, #0\nsmlal r4, r5, r0, r1\nmvn r6, #0\nmvn r7, #0\numaal r6, r7, r0, r1\nmov r8, #0x40000000\nmov r9, #6\nsmmul r10, r8, r9\nsmmulr r11, r8, r9\nmov r12, #10\nsmmla r12, r8, r9, r12",
                &[
                    ("r2", 2),
                    ("r3", 0),
                    ("r4", 0),
                    ("r5", 0),
                    ("r6", 0xffff_ffff),
                    ("r7", 0xffff_ffff),
                    ("r10", 1),
                    ("r11", 2),
                    ("r12", 11),
                ],
                None,
            ),
            (
                "movw r0, #0x8000\nmovt r0, #3\nmvn r1, #1\nsmulbb r2, r0, r1\nsmultb r3, r0, r1",
                &[("r2", 0x10000), ("r3", 0xffff_fffa)],
                None,
            ),
            (
                "mov r0, #0\nmov r1, #5\nmuls r2, r0, r1",
                &[("r2", 0)],
                Some("nZcv"),
            ),
            (
                "mvn r0, #0\nmov r1, #1\nsmulls r2, r3, r0, r1",
                &[("r2", 0xffff_ffff), ("r3", 0xffff_ffff)],
                Some("Nzcv"),
            ),
        ],
    );
}

#[test]
fn arm32_hardware_divide_returns_zero_for_division_by_zero() {
    check(
        Architecture::Arm32,
        &[(
            ".cpu cortex-a15\nmov r0, #-7\nmov r1, #2\nsdiv r2, r0, r1\nudiv r3, r0, r1\nmov r4, #0\nmov r5, #9\nmov r6, #9\nsdiv r5, r0, r4\nudiv r6, r0, r4\nmov r7, #0x80000000\nmvn r8, #0\nsdiv r9, r7, r8",
            &[
                ("r2", 0xffff_fffd),
                ("r3", 0x7fff_fffc),
                ("r5", 0),
                ("r6", 0),
                ("r9", 0x8000_0000),
            ],
            None,
        )],
    );
}

#[test]
fn arm32_sixty_four_bit_carry_chains_and_reverse_subtracts() {
    check(
        Architecture::Arm32,
        &[
            (
                "mvn r0, #0\nmov r1, #1\nmov r2, #1\nmov r3, #0\nadds r4, r0, r2\nadc r5, r1, r3\nmov r6, #0\nmov r7, #2\nsubs r8, r6, #1\nsbc r9, r7, #0\nmov r10, #5\nmov r11, #0\nrsbs r12, r10, #0\nrsc r11, r11, #0",
                &[
                    ("r4", 0),
                    ("r5", 2),
                    ("r8", 0xffff_ffff),
                    ("r9", 1),
                    ("r12", 0xffff_fffb),
                    ("r11", 0xffff_ffff),
                ],
                Some("Nzcv"),
            ),
            (
                "mov r0, #0\nsubs r1, r0, #1\nsbcs r2, r0, #0",
                &[("r2", 0xffff_ffff)],
                Some("Nzcv"),
            ),
            (
                "mvn r0, #0x80000000\ncmp r0, r0\nadcs r1, r0, #0",
                &[("r1", 0x8000_0000)],
                Some("NzcV"),
            ),
            ("mov r0, #3\ncmn r0, #3", &[], Some("nzcv")),
            ("mvn r0, #2\ncmn r0, #3", &[], Some("nZCv")),
        ],
    );
}

#[test]
fn arm32_shifts_by_register_rrx_and_shifter_carry() {
    let load = "movw r0, #1\nmovt r0, #0x8000\n";
    type ShiftCase = (String, Vec<(&'static str, u64)>, &'static str);
    let cases: Vec<ShiftCase> = vec![
        (
            format!("{load}mov r3, #32\nlsrs r4, r0, r3"),
            vec![("r4", 0)],
            "nZCv",
        ),
        (
            format!("{load}cmp r0, r0\nmov r1, #33\nlsls r2, r0, r1"),
            vec![("r2", 0)],
            "nZcv",
        ),
        (
            format!("{load}mov r1, #1\nasrs r2, r0, r1"),
            vec![("r2", 0xc000_0000)],
            "NzCv",
        ),
        (
            format!("{load}mov r1, #40\nasr r2, r0, r1"),
            vec![("r2", 0xffff_ffff)],
            "nzcv",
        ),
        (
            format!("{load}cmp r0, r0\nrrxs r2, r0"),
            vec![("r2", 0xc000_0000)],
            "NzCv",
        ),
        (
            format!("{load}mov r1, #36\nror r2, r0, r1"),
            vec![("r2", 0x1800_0000)],
            "nzcv",
        ),
        (
            format!("{load}movs r2, r0, lsr #32"),
            vec![("r2", 0)],
            "nZCv",
        ),
        (
            format!(
                "{load}mov r1, #3\nadd r2, r1, r0, lsl #1\nmov r3, #31\neor r4, r1, r0, lsr r3\nmov r5, #4\nmvn r6, r0, lsl r5"
            ),
            vec![("r2", 5), ("r4", 2), ("r6", 0xffff_ffef)],
            "nzcv",
        ),
        (format!("{load}tst r0, r0, lsl #1"), vec![], "nZCv"),
        (format!("{load}teq r0, #0x80000000"), vec![], "nzCv"),
        (
            format!("{load}mov r1, #0\nbics r2, r0, #0x80000000\nlsl r3, r0, r1"),
            vec![("r2", 1), ("r3", 0x8000_0001)],
            "nzCv",
        ),
    ];
    for (body, registers, expected) in cases {
        let snapshot = run(Architecture::Arm32, &body);
        for (name, value) in registers {
            assert_eq!(snapshot.register(name), Some(value), "{body}: {name}");
        }
        assert_eq!(flags(&snapshot), expected, "{body}");
    }
}

#[test]
fn arm32_extends_byte_reversal_bitfields_and_wide_moves() {
    check(
        Architecture::Arm32,
        &[
            (
                "movw r0, #0x8081\nmovt r0, #0x1282\nuxtb r1, r0\nsxtb r2, r0\nuxth r3, r0\nsxth r4, r0\nuxtb r5, r0, ror #8\nsxtb r6, r0, ror #16\nmov r7, #1000\nuxtab r8, r7, r0\nsxtah r9, r7, r0\nclz r10, r0\nrev r11, r0\nrev16 r12, r0",
                &[
                    ("r0", 0x1282_8081),
                    ("r1", 0x81),
                    ("r2", 0xffff_ff81),
                    ("r3", 0x8081),
                    ("r4", 0xffff_8081),
                    ("r5", 0x80),
                    ("r6", 0xffff_ff82),
                    ("r8", 1129),
                    ("r9", 0xffff_8469),
                    ("r10", 3),
                    ("r11", 0x8180_8212),
                    ("r12", 0x8212_8180),
                ],
                None,
            ),
            (
                "movw r0, #0x8081\nmovt r0, #0x1282\nrevsh r1, r0\nrbit r2, r0\nubfx r3, r0, #4, #8\nsbfx r4, r0, #12, #8\nsbfx r5, r0, #4, #4\nmvn r6, #0\nbfi r6, r0, #8, #12\nmvn r7, #0\nbfc r7, #4, #8",
                &[
                    ("r1", 0xffff_8180),
                    ("r2", 0x8101_4148),
                    ("r3", 0x08),
                    ("r4", 0x28),
                    ("r5", 0xffff_fff8),
                    ("r6", 0xfff0_81ff),
                    ("r7", 0xffff_f00f),
                ],
                None,
            ),
        ],
    );
}

const ARM32_DATA: &str = ".data\nvalues: .word 0xfffffff0\n.hword 0x8001\n.byte 0x80, 0x7f\n.word 0x11223344\n.word 0x55667788\n.space 32";

#[test]
fn arm32_halfword_signed_and_register_offset_loads() {
    let body = format!(
        "movw r0, #0x2000\nldrh r1, [r0, #4]\nldrsh r2, [r0, #4]\nldrsb r3, [r0, #6]\nmov r4, #6\nldrb r5, [r0, r4]\nadd r6, r0, #8\nmov r7, #2\nldrsb r8, [r6, -r7]\nldr r9, [r0, r7, lsl #1]\nmov r10, #4\nldrsh r11, [r6], -r10\nldrh r12, [r6, #-4]\n{ARM32_DATA}"
    );
    let snapshot = run(Architecture::Arm32, &body);
    for (name, value) in [
        ("r1", 0x8001),
        ("r2", 0xffff_8001),
        ("r3", 0xffff_ff80),
        ("r5", 0x80),
        ("r6", 0x2004),
        ("r8", 0xffff_ff80),
        ("r9", 0x7f80_8001),
        ("r11", 0x3344),
        ("r12", 0xfff0),
    ] {
        assert_eq!(snapshot.register(name), Some(value), "{name}");
    }
}

#[test]
fn arm32_dual_and_multiple_transfers_with_writeback() {
    let body = format!(
        "movw r0, #0x2000\nldrd r2, r3, [r0, #8]\nadd r1, r0, #16\nstrd r2, r3, [r1], #8\nstrh r2, [r1, #2]!\nldm r0, {{r5, r6}}\nadd r7, r0, #8\nldmia r7!, {{r8, r9}}\nstmdb r7!, {{r5, r6}}\nldmib r0, {{r10, r11}}\nldmda r7, {{r4, r12}}\n{ARM32_DATA}"
    );
    let snapshot = run(Architecture::Arm32, &body);
    for (name, value) in [
        ("r1", 0x201a),
        ("r2", 0x1122_3344),
        ("r3", 0x5566_7788),
        ("r5", 0xffff_fff0),
        ("r6", 0x7f80_8001),
        ("r7", 0x2008),
        ("r8", 0x1122_3344),
        ("r9", 0x5566_7788),
        ("r10", 0x7f80_8001),
        ("r11", 0xffff_fff0),
        ("r4", 0x7f80_8001),
        ("r12", 0xffff_fff0),
    ] {
        assert_eq!(snapshot.register(name), Some(value), "{name}");
    }
    assert_eq!(
        snapshot.read(0x2008, 20).unwrap(),
        [
            0xf0, 0xff, 0xff, 0xff, 0x01, 0x80, 0x80, 0x7f, 0x44, 0x33, 0x22, 0x11, 0x88, 0x77,
            0x66, 0x55, 0, 0, 0x44, 0x33
        ]
    );
}

#[test]
fn arm32_calls_interworking_returns_jump_tables_and_status_moves() {
    let body = "mov r4, #4\nbl function\nmov r6, #6\nmov r0, #2\nldr pc, [pc, r0, lsl #2]\nnop\n.word case0, case1, case2\ncase0:\nmov r1, #10\nb done\ncase1:\nmov r1, #11\nb done\ncase2:\nmov r1, #12\nb done\nfunction:\npush {r4, lr}\nmov r4, #44\nadr r1, inner\nblx r1\nafter_blx:\npop {r4, pc}\ninner:\nmov r5, #5\nbx lr\ndone:\nmov r0, #1\nadd pc, pc, r0, lsl #2\nmov r2, #1\nmov r2, #2\nmov r2, #3";
    let image = program(Architecture::Arm32, &format!("{body}\nbkpt #0"));
    let after_blx = address_of(&image, "pop {r4, pc}");
    let snapshot = run(Architecture::Arm32, body);
    for (name, value) in [
        ("r1", 12),
        ("r2", 3),
        ("r4", 4),
        ("r5", 5),
        ("r6", 6),
        ("sp", 0x20000),
        ("lr", after_blx),
    ] {
        assert_eq!(snapshot.register(name), Some(value), "{name}");
    }
    check(
        Architecture::Arm32,
        &[(
            "mov r0, #5\ncmp r0, #5\naddne r1, r0, #1\naddeq r2, r0, #2\nmovwgt r3, #9\nldrne r4, [r0]\nmrs r5, apsr\nmov r6, #0x90000000\nmsr APSR_nzcvq, r6\nmovmi r7, #1\nmovvs r8, #1\nmovvc r9, #1",
            &[
                ("r1", 0),
                ("r2", 7),
                ("r3", 0),
                ("r4", 0),
                ("r5", 0x6000_0000),
                ("r7", 1),
                ("r8", 1),
                ("r9", 0),
            ],
            Some("NzcV"),
        )],
    );
}

#[test]
fn arm32_faults_are_atomic_for_multiple_store_and_load() {
    let image = with_read_only_boundary(program(
        Architecture::Arm32,
        "movw r0, #4\nmovt r0, #3\nmov r1, #1\nmov r2, #2\nstm r0!, {r1, r2}\nbkpt #0",
    ));
    assert_atomic_fault(&image, 4, "read-only");
    let image = with_read_only_boundary(program(
        Architecture::Arm32,
        "movw r0, #0xc\nmovt r0, #3\nldm r0!, {r1, r2}\nbkpt #0",
    ));
    assert_atomic_fault(&image, 2, "Unmapped");
}

// ------------------------------------------------------------ x86 / x64 --

#[test]
fn x86_one_operand_multiply_sets_carry_and_overflow_from_the_high_half() {
    for architecture in [Architecture::X86, Architecture::X64] {
        check(
            architecture,
            &[
                (
                    "mov al, 200\nmov cl, 3\nmul cl",
                    &[("ax", 0x258)],
                    Some("CpazsO"),
                ),
                (
                    "mov ax, 0x8000\nmov cx, 4\nmul cx",
                    &[("ax", 0), ("dx", 2)],
                    Some("CpazsO"),
                ),
                (
                    "mov eax, 0xffffffff\nmov ecx, 0xffffffff\nmul ecx",
                    &[("eax", 1), ("edx", 0xffff_fffe)],
                    Some("CpazsO"),
                ),
                (
                    "mov eax, 7\nmov ecx, 6\nmul ecx",
                    &[("eax", 42), ("edx", 0)],
                    Some("cpazso"),
                ),
                (
                    "mov eax, -3\nmov ecx, 5\nimul ecx",
                    &[("eax", 0xffff_fff1), ("edx", 0xffff_ffff)],
                    Some("cpazso"),
                ),
                (
                    "mov eax, 0x40000000\nmov ecx, 4\nimul ecx",
                    &[("eax", 0), ("edx", 1)],
                    Some("CpazsO"),
                ),
                (
                    "mov al, -128\nmov cl, -1\nimul cl",
                    &[("ax", 0x80)],
                    Some("CpazsO"),
                ),
                (
                    "mov ecx, 0x10000\nimul eax, ecx, 0x10000",
                    &[("eax", 0)],
                    Some("CpazsO"),
                ),
                (
                    "mov ecx, 7\nimul eax, ecx, -3",
                    &[("eax", 0xffff_ffeb)],
                    Some("cpazso"),
                ),
            ],
        );
    }
    check(
        Architecture::X64,
        &[
            (
                "mov rax, -1\nmov rcx, 2\nmul rcx",
                &[("rax", 0xffff_ffff_ffff_fffe), ("rdx", 1)],
                Some("CpazsO"),
            ),
            (
                "mov rax, -1\nmov rcx, -1\nimul rcx",
                &[("rax", 1), ("rdx", 0)],
                Some("cpazso"),
            ),
            (
                "mov rdx, -1\nmov eax, 0xffffffff\nmov ecx, 0xffffffff\nmul ecx",
                &[("rax", 1), ("rdx", 0xffff_fffe)],
                None,
            ),
        ],
    );
}

#[test]
fn x86_divide_quotient_remainder_and_sign_rules() {
    for architecture in [Architecture::X86, Architecture::X64] {
        check(
            architecture,
            &[
                (
                    "mov edx, 0\nmov eax, 100\nmov ecx, 7\ndiv ecx",
                    &[("eax", 14), ("edx", 2)],
                    None,
                ),
                ("mov ax, 1000\nmov cl, 7\ndiv cl", &[("ax", 0x068e)], None),
                (
                    "mov dx, 1\nmov ax, 0\nmov cx, 3\ndiv cx",
                    &[("ax", 0x5555), ("dx", 1)],
                    None,
                ),
                (
                    "mov eax, -100\ncdq\nmov ecx, 7\nidiv ecx",
                    &[("eax", 0xffff_fff2), ("edx", 0xffff_fffe)],
                    None,
                ),
                (
                    "mov eax, 100\ncdq\nmov ecx, -7\nidiv ecx",
                    &[("eax", 0xffff_fff2), ("edx", 2)],
                    None,
                ),
                ("mov ax, -100\nmov cl, 7\nidiv cl", &[("ax", 0xfef2)], None),
            ],
        );
    }
    check(
        Architecture::X64,
        &[
            (
                "mov rdx, 1\nmov rax, 0\nmov rcx, 2\ndiv rcx",
                &[("rax", 0x8000_0000_0000_0000), ("rdx", 0)],
                None,
            ),
            (
                "mov rax, -100\ncqo\nmov rcx, 7\nidiv rcx",
                &[("rax", (-14_i64 as u64)), ("rdx", (-2_i64 as u64))],
                None,
            ),
        ],
    );
}

#[test]
fn x86_divide_errors_fault_atomically() {
    for (architecture, body, prefix) in [
        (
            Architecture::X86,
            "mov eax, 5\nmov edx, 3\nmov ecx, 0\ndiv ecx",
            3,
        ),
        (
            Architecture::X86,
            "mov eax, 0x80000000\ncdq\nmov ecx, -1\nidiv ecx",
            3,
        ),
        (
            Architecture::X86,
            "mov edx, 1\nmov eax, 0\nmov ecx, 1\ndiv ecx",
            3,
        ),
        (Architecture::X86, "mov ax, 0x8000\nmov cl, -1\nidiv cl", 2),
        (
            Architecture::X64,
            "mov rax, 0x8000000000000000\ncqo\nmov rcx, -1\nidiv rcx",
            3,
        ),
        (Architecture::X64, "mov eax, 1\nxor ecx, ecx\ndiv rcx", 2),
    ] {
        let image = program(architecture, &format!("{body}\nint3"));
        assert_atomic_fault(&image, prefix, "#DE");
    }
}

#[test]
fn x86_sign_extension_and_zero_extension_forms() {
    check(
        Architecture::X86,
        &[
            ("mov eax, 0x8000\ncwde", &[("eax", 0xffff_8000)], None),
            (
                "mov eax, 0x1234\nmov al, 0x80\ncbw",
                &[("eax", 0xff80)],
                None,
            ),
            (
                "mov edx, 0x12345678\nmov ax, 0x8000\ncwd",
                &[("edx", 0x1234_ffff)],
                None,
            ),
            ("mov eax, 0x80000000\ncdq", &[("edx", 0xffff_ffff)], None),
            ("mov edx, 5\nmov eax, 0x7fffffff\ncdq", &[("edx", 0)], None),
        ],
    );
    check(
        Architecture::X64,
        &[
            (
                "mov eax, 0x80000000\ncdqe",
                &[("rax", 0xffff_ffff_8000_0000)],
                None,
            ),
            ("mov rax, -2\ncqo", &[("rdx", u64::MAX)], None),
            ("mov rdx, 7\nmov rax, 5\ncqo", &[("rdx", 0)], None),
        ],
    );
    let snapshot = run(
        Architecture::X64,
        "lea rbx, [rip + values]\nmovsx eax, byte ptr [rbx]\nmovzx ecx, byte ptr [rbx]\nmovsx rdx, word ptr [rbx + 2]\nmovzx esi, word ptr [rbx + 2]\nmovsxd rdi, dword ptr [rbx + 4]\nmov r8, -1\nmov r8d, dword ptr [rbx + 4]\nmov r9, -1\nmovzx r9d, cl\nmovsx r10, al\nmov r11, -1\nmov r11w, word ptr [rbx + 2]\nmovsx r12w, byte ptr [rbx]\n.data\nvalues: .byte 0x80, 0x7f\n.short 0x8001\n.long 0x80000001",
    );
    for (name, value) in [
        ("rax", 0xffff_ff80),
        ("rcx", 0x80),
        ("rdx", 0xffff_ffff_ffff_8001),
        ("rsi", 0x8001),
        ("rdi", 0xffff_ffff_8000_0001),
        ("r8", 0x8000_0001),
        ("r9", 0x80),
        ("r10", 0xffff_ffff_ffff_ff80),
        ("r11", 0xffff_ffff_ffff_8001),
        ("r12", 0xff80),
    ] {
        assert_eq!(snapshot.register(name), Some(value), "{name}");
    }
}

#[test]
fn x86_carry_chains_negation_and_parity_adjust_flags() {
    for architecture in [Architecture::X86, Architecture::X64] {
        check(
            architecture,
            &[
                (
                    "mov eax, 0xffffffff\nmov edx, 1\nadd eax, 1\nadc edx, 0",
                    &[("eax", 0), ("edx", 2)],
                    Some("cpazso"),
                ),
                (
                    "mov eax, 0\nmov edx, 2\nsub eax, 1\nsbb edx, 0",
                    &[("eax", 0xffff_ffff), ("edx", 1)],
                    Some("cpazso"),
                ),
                (
                    "mov eax, 0\nsub eax, 1",
                    &[("eax", 0xffff_ffff)],
                    Some("CPAzSo"),
                ),
                (
                    "stc\nmov eax, 0x80000000\nsbb eax, 0",
                    &[("eax", 0x7fff_ffff)],
                    Some("cPAzsO"),
                ),
                (
                    "stc\nmov eax, 0x7fffffff\nadc eax, 0",
                    &[("eax", 0x8000_0000)],
                    Some("cPAzSO"),
                ),
                ("mov eax, 0\nneg eax", &[("eax", 0)], Some("cPaZso")),
                (
                    "mov eax, 0x80000000\nneg eax",
                    &[("eax", 0x8000_0000)],
                    Some("CPazSO"),
                ),
                (
                    "mov ecx, 5\nneg ecx",
                    &[("ecx", 0xffff_fffb)],
                    Some("CpAzSo"),
                ),
                (
                    "stc\nmov ecx, 5\nnot ecx\ncmc",
                    &[("ecx", 0xffff_fffa)],
                    Some("cpazso"),
                ),
                ("mov al, 0x0f\nadd al, 1", &[("al", 0x10)], Some("cpAzso")),
                ("mov eax, 0x80\ntest al, al", &[], Some("cpazSo")),
                (
                    "mov ecx, 3\ninc ecx\nstc\ndec ecx",
                    &[("ecx", 3)],
                    Some("CPazso"),
                ),
            ],
        );
    }
    check(
        Architecture::X64,
        &[(
            "mov rax, -1\nmov rdx, 0\nadd rax, 1\nadc rdx, 0\nmov rcx, 0\nsub rcx, 1\nsbb rdx, 0",
            &[("rax", 0), ("rdx", 0), ("rcx", u64::MAX)],
            Some("cPaZso"),
        )],
    );
}

#[test]
fn x86_double_precision_shifts_and_shift_flags() {
    for architecture in [Architecture::X86, Architecture::X64] {
        check(
            architecture,
            &[
                (
                    "mov eax, 0x12345678\nmov edx, 0x9abcdef0\nshld eax, edx, 8",
                    &[("eax", 0x3456_789a), ("edx", 0x9abc_def0)],
                    Some("cPazso"),
                ),
                (
                    "mov eax, 0x12345678\nmov edx, 0x9abcdef0\nmov cl, 4\nshrd eax, edx, cl",
                    &[("eax", 0x0123_4567)],
                    Some("Cpazso"),
                ),
                (
                    "mov eax, 0x40000000\nmov edx, 0x80000000\nshld eax, edx, 1",
                    &[("eax", 0x8000_0001)],
                    Some("cpazSO"),
                ),
                (
                    "mov eax, 0x80000001\nshl eax, 1",
                    &[("eax", 2)],
                    Some("CpazsO"),
                ),
                (
                    "mov eax, 0x80000001\nsar eax, 1",
                    &[("eax", 0xc000_0000)],
                    Some("CPazSo"),
                ),
                (
                    "mov eax, 0x80000001\nshr eax, 1",
                    &[("eax", 0x4000_0000)],
                    Some("CPazsO"),
                ),
                (
                    "mov eax, 0x80000001\nmov cl, 33\nshl eax, cl",
                    &[("eax", 2)],
                    Some("CpazsO"),
                ),
                (
                    "mov eax, 0x80000001\nrol eax, 4",
                    &[("eax", 0x18)],
                    Some("cpazso"),
                ),
                ("mov al, 0x81\nror al, 1", &[("al", 0xc0)], Some("Cpazso")),
                (
                    "mov eax, 0\nsub eax, 1\nmov cl, 32\nshl eax, cl",
                    &[("eax", 0xffff_ffff)],
                    Some("CPAzSo"),
                ),
            ],
        );
    }
    check(
        Architecture::X64,
        &[
            ("mov rax, -8\nsar rax, 2", &[("rax", (-2_i64 as u64))], None),
            (
                "mov rax, 1\nmov rdx, 0x8000000000000000\nshld rax, rdx, 63",
                &[("rax", 0xc000_0000_0000_0000)],
                None,
            ),
            (
                "mov rax, -1\nmov cl, 64\nshr eax, cl",
                &[("rax", 0xffff_ffff)],
                None,
            ),
        ],
    );
}

#[test]
fn x86_conditional_set_move_and_x64_cmov_zero_extension() {
    for architecture in [Architecture::X86, Architecture::X64] {
        let snapshot = run(
            architecture,
            "mov eax, 5\ncmp eax, 7\nsetl bl\nsetb cl\nsetg dl\nsete byte ptr [0x2000]\nsetne byte ptr [0x2001]\nmov esi, 100\nmov edi, 200\ncmovl esi, edi\ncmovg edi, eax\nsetae ah",
        );
        for (name, value) in [
            ("bl", 1),
            ("cl", 1),
            ("dl", 0),
            ("esi", 200),
            ("edi", 200),
            ("eax", 5),
        ] {
            assert_eq!(
                snapshot.register(name),
                Some(value),
                "{architecture:?} {name}"
            );
        }
        assert_eq!(snapshot.read(0x2000, 2).unwrap(), [0, 1]);
    }
    check(
        Architecture::X64,
        &[(
            "mov rax, -1\nmov ecx, 1\ncmp ecx, ecx\ncmovne eax, ecx\nmov rdx, -1\ncmove rdx, rcx",
            &[("rax", 0xffff_ffff), ("rdx", 1)],
            None,
        )],
    );
}

#[test]
fn x86_stack_frames_exchange_and_indirect_control_transfer() {
    let body = "mov eax, 1\nmov ebx, 2\nxchg eax, ebx\npush 0x12345\npush -1\npop ecx\npop edx\nmov dword ptr [0x3000], 77\npush dword ptr [0x3000]\npop dword ptr [0x3004]\npush ebp\nmov ebp, esp\nsub esp, 16\nleave\nmov esi, offset table\nmov eax, 1\njmp dword ptr [esi + eax*4]\ncase0:\nmov edi, 10\nint3\ncase1:\nmov edi, 11\nmov eax, offset function\ncall eax\npush 5\ncall dword ptr [esi + 8]\nint3\nfunction:\nadd ebx, 98\nret\nfunction2:\nadd ebx, 1\nret 4\n.data\ntable: .long case0, case1, function2";
    let snapshot = run(Architecture::X86, body);
    for (name, value) in [
        ("edi", 11),
        ("ebx", 100),
        ("ecx", 0xffff_ffff),
        ("edx", 0x12345),
        ("esp", 0x20000),
        ("ebp", 0),
    ] {
        assert_eq!(snapshot.register(name), Some(value), "{name}");
    }
    assert_eq!(snapshot.read(0x3004, 4).unwrap(), [77, 0, 0, 0]);
    let snapshot = run(
        Architecture::X64,
        "lea rsi, [rip + table]\nmov eax, 2\njmp qword ptr [rsi + rax*8]\ncase0:\nmov edi, 10\nint3\ncase1:\nmov edi, 11\nint3\ncase2:\nmov edi, 12\npush -2\npop r8\nmov qword ptr [0x3000], r8\nmov r9, 7\nxchg r9, qword ptr [0x3000]\nlea rax, [rip + function]\ncall rax\npush qword ptr [0x3000]\npop r10\njmp finish\nfunction:\nmov r11, 99\nret\nfinish:\n.data\n.p2align 3\ntable: .quad case0, case1, case2",
    );
    for (name, value) in [
        ("rdi", 12),
        ("r8", (-2_i64) as u64),
        ("r9", (-2_i64) as u64),
        ("r10", 7),
        ("r11", 99),
        ("rsp", 0x20000),
    ] {
        assert_eq!(snapshot.register(name), Some(value), "{name}");
    }
    assert_eq!(snapshot.read(0x3000, 8).unwrap(), [7, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn x86_bit_tests_scans_and_byte_swaps() {
    for architecture in [Architecture::X86, Architecture::X64] {
        check(
            architecture,
            &[(
                "mov eax, 0x12\nmov ecx, 4\nbt eax, ecx\nsetc bl\nbts eax, 0\nbtr eax, 1\nsetc bh\nbtc eax, 31\nbsr edx, eax\nbsf esi, eax\nmov edi, 7\nmov ecx, 0\nbsf edi, ecx\nmov ecx, 0x11223344\nbswap ecx",
                &[
                    ("eax", 0x8000_0011),
                    ("bl", 1),
                    ("bh", 1),
                    ("edx", 31),
                    ("esi", 0),
                    ("edi", 7),
                    ("ecx", 0x4433_2211),
                ],
                Some("cpaZso"),
            )],
        );
    }
    check(
        Architecture::X64,
        &[(
            "mov rax, 0x1122334455667788\nbswap rax\nmov rcx, -1\nbswap ecx\nmov rdx, 0x8000000000000000\nbsr r8, rdx",
            &[
                ("rax", 0x8877_6655_4433_2211),
                ("rcx", 0xffff_ffff),
                ("r8", 63),
            ],
            None,
        )],
    );
}

#[test]
fn x86_repeated_moves_and_stores_execute_one_element_per_step() {
    let snapshot = run(
        Architecture::X64,
        "lea rsi, [rip + source]\nlea rdi, [rip + destination]\nmov ecx, 3\nrep movsd dword ptr [rdi], dword ptr [rsi]\nmov eax, 0x41\nmov ecx, 4\nrep stosb byte ptr [rdi], al\nmov ecx, 0\nrep stosq qword ptr [rdi], rax\n.data\nsource: .long 1, 2, 3\ndestination: .space 16",
    );
    assert_eq!(
        snapshot.read(0x200c, 16).unwrap(),
        [1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 0x41, 0x41, 0x41, 0x41]
    );
    assert_eq!(snapshot.register("rcx"), Some(0));
    assert_eq!(snapshot.register("rsi"), Some(0x200c));
    assert_eq!(snapshot.register("rdi"), Some(0x201c));
    // lea, lea, mov, 3 x movsd, mov, mov, 4 x stosb, mov, stosq (count 0), int3.
    assert_eq!(snapshot.steps, 15);
    let snapshot = run(
        Architecture::X86,
        "mov esi, offset source\nmov edi, offset destination\nmov ecx, 2\nrep movsd dword ptr es:[edi], dword ptr [esi]\nmovsb byte ptr es:[edi], byte ptr [esi]\n.data\nsource: .long 0x11223344, 0x55667788\n.byte 0x99\ndestination: .space 12",
    );
    assert_eq!(
        snapshot.read(0x2009, 9).unwrap(),
        [0x44, 0x33, 0x22, 0x11, 0x88, 0x77, 0x66, 0x55, 0x99]
    );
}

#[test]
fn x86_multi_byte_nops_execute_without_effect() {
    check(
        Architecture::X64,
        &[(
            ".byte 0x66, 0x2e, 0x0f, 0x1f, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00\nnop dword ptr [rax]\nnop word ptr [rax + rax]\nxchg ax, ax\nmov eax, 5",
            &[("rax", 5)],
            Some("cpazso"),
        )],
    );
}

#[test]
fn x86_faults_are_atomic_for_writes_to_read_only_data() {
    let image = with_read_only_boundary(program(
        Architecture::X64,
        "mov rax, 5\nmov qword ptr [0x30000], 9\nxchg rax, qword ptr [0x30008]\nint3",
    ));
    assert_atomic_fault(&image, 2, "read-only");
    let image = with_read_only_boundary(program(
        Architecture::X86,
        "mov esi, 0x30000\nmov edi, 0x30004\nmov ecx, 2\nrep movsd dword ptr es:[edi], dword ptr [esi]\nint3",
    ));
    // The first element (0x30000 -> 0x30004) commits; the second targets read-only memory.
    assert_atomic_fault(&image, 4, "read-only");
}

#[test]
fn writes_to_code_fault_with_a_clear_message() {
    for (architecture, body) in [
        (Architecture::Arm64, "adr x0, _start\nstr x0, [x0]\nbrk #0"),
        (Architecture::Arm32, "adr r0, _start\nstr r0, [r0]\nbkpt #0"),
        (Architecture::X86, "mov dword ptr [_start], eax\nint3"),
        (Architecture::X64, "mov dword ptr [rip + _start], eax\nint3"),
    ] {
        let mut image = program(architecture, body);
        for region in &mut image.regions {
            if region.executable {
                region.writable = false;
                region.label = "Code".into();
            }
        }
        let prefix = usize::from(matches!(
            architecture,
            Architecture::Arm32 | Architecture::Arm64
        ));
        assert_atomic_fault(&image, prefix, "Write to read-only memory");
    }
}

/// Guest values must never panic the host: the 128-bit signed dividend
/// i128::MIN divided by -1 is a #DE fault, and a descending LDM below address
/// zero wraps to an unmapped address and faults.
#[test]
fn extreme_operands_fault_instead_of_panicking() {
    let image = program(
        Architecture::X64,
        "movabs rdx, 0x8000000000000000\nxor eax, eax\nmov rcx, -1\nidiv rcx\nint3",
    );
    assert_atomic_fault(&image, 3, "#DE");
    let image = program(Architecture::Arm32, "mov r0, #0\nldmda r0, {r1}\nbkpt #0");
    assert_atomic_fault(&image, 1, "nmapped");
}
