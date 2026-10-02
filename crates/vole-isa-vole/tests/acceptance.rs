use vole_core::{Architecture, Machine};
use vole_isa_vole::assemble;

#[test]
fn assemble_original_addition_program() {
    let program = assemble(Architecture::Vole.example_source()).unwrap();
    assert_eq!(program.entry, 0);
    assert_eq!(
        program.regions[0].bytes,
        [0x21, 0x3a, 0x22, 0x43, 0x53, 0x12, 0x33, 0xbb, 0xc0, 0x00]
    );
    assert_eq!(program.instructions[2].assembly, "addi R3, R1, R2");
}

#[test]
fn run_addition_and_reverse_memory_write() {
    let program = assemble(Architecture::Vole.example_source()).unwrap();
    let mut machine = vole_isa_vole::VoleMachine::new();
    machine.load(&program).unwrap();
    for _ in 0..4 {
        machine.step().unwrap();
    }
    assert_eq!(machine.snapshot().register("R3"), Some(125));
    assert_eq!(machine.snapshot().byte(0xbb), Some(125));
    machine.reverse_step().unwrap();
    assert_eq!(machine.snapshot().byte(0xbb), Some(0));
    assert_eq!(machine.snapshot().pc, 6);
    machine.step().unwrap();
    assert!(machine.step().unwrap().halted);
}

#[test]
fn labels_strings_and_literals_assemble_without_treating_string_colons_as_labels() {
    let program = assemble("org 10h\nstart: load R1, message\njmp end\nmessage: db \"a:b;z\", -1, 0x80, 255\nend: halt").unwrap();
    assert_eq!(program.entry, 16);
    assert_eq!(program.symbols["message"], 20);
    assert_eq!(program.symbols["end"], 28);
    assert_eq!(
        program.regions[0].bytes,
        [
            0x21, 20, 0xb0, 28, b'a', b':', b'b', b';', b'z', 255, 128, 255, 0xc0, 0
        ]
    );
    let colon = assemble("load R1, 1\ndb \"x:y\"\nhalt").unwrap();
    assert_eq!(&colon.regions[0].bytes[2..5], b"x:y");
}

fn make_machine(source: &str) -> vole_isa_vole::VoleMachine {
    let mut machine = vole_isa_vole::VoleMachine::new();
    machine.load(&assemble(source).unwrap()).unwrap();
    machine
}

#[test]
fn all_data_operations_match_the_instruction_table() {
    let mut machine = make_machine(
        "load R1, 0F0h\nload R2, 0Fh\nmove R3, R1\naddi R4, R1, R2\nor R5, R1, R2\nand R6, R1, R2\nxor R7, R1, R2\nror R7, 4\nload R8, 80h\nstore R7, [R8]\nload R9, [R8]\nload RA, [80h]\nhalt",
    );
    for _ in 0..13 {
        machine.step().unwrap();
    }
    let state = machine.snapshot();
    for (name, expected) in [
        ("R3", 240),
        ("R4", 255),
        ("R5", 255),
        ("R6", 0),
        ("R7", 255),
        ("R9", 255),
        ("RA", 255),
    ] {
        assert_eq!(state.register(name), Some(expected), "{name}");
    }
    assert_eq!(state.byte(128), Some(255));
    assert!(state.halted);
    assert_eq!(state.trace[10].memory_reads[0].address, 128);
    assert_eq!(state.trace[11].memory_reads[0].address, 128);
}

#[test]
fn arithmetic_wrap_rotation_and_move_operand_order() {
    let mut machine =
        make_machine("load R1, 255\nload R2, 2\naddi R3, R1, R2\nmove R4, R3\nror R4, 1\nhalt");
    for _ in 0..5 {
        machine.step().unwrap();
    }
    assert_eq!(machine.snapshot().register("R3"), Some(1));
    assert_eq!(machine.snapshot().register("R4"), Some(128));
}

#[test]
fn signed_less_equal_and_equal_branches_take_only_their_conditions() {
    let mut machine = make_machine(
        "load R0, 1\nload R1, -1\njmpEQ R1=R0, fail\njmpLE R1<=R0, passed\nfail: load RF, 88\npassed: load R2, 1\njmpEQ R2=R0, done\nload RF, 89\ndone: halt",
    );
    for _ in 0..7 {
        machine.step().unwrap();
    }
    assert!(machine.snapshot().halted);
    assert!(machine.snapshot().output.is_empty());
    assert_eq!(machine.snapshot().trace[2].pc_after, 6);
    assert_eq!(machine.snapshot().trace[3].pc_after, 10);
    assert_eq!(machine.snapshot().trace[5].pc_after, 16);
    let mut unequal =
        make_machine("load R0, -1\nload R1, 1\njmpLE R1<=R0, fail\nhalt\nfail: load RF, 88");
    for _ in 0..4 {
        unequal.step().unwrap();
    }
    assert!(unequal.snapshot().halted);
}

#[test]
fn teaching_float_exact_results_and_round_towards_zero() {
    for (left, right, expected) in [
        (0x48, 0x48, 0x58),
        (0x68, 0x38, 0x69),
        (0x18, 0x1f, 0x2b),
        (0xc8, 0x48, 0),
        (1, 1, 2),
        (0x80, 0, 0),
    ] {
        let mut machine = make_machine(&format!(
            "load R1, {left}\nload R2, {right}\naddf R3, R1, R2\nhalt"
        ));
        for _ in 0..3 {
            machine.step().unwrap();
        }
        assert_eq!(
            machine.snapshot().register("R3"),
            Some(expected),
            "float {left:02X} + {right:02X}"
        );
    }
    let mut overflow = make_machine("load R1, 7Fh\naddf RF, R1, R1\nhalt");
    overflow.step().unwrap();
    let before = overflow.snapshot();
    assert!(overflow.step().unwrap_err().0.contains("overflow"));
    assert_eq!(overflow.snapshot(), before);
}

#[test]
fn output_undo_and_reset_restore_every_observable_state() {
    let mut machine = make_machine("load RF, 65\nload RF, 65\nload R1, 9\nstore R1, [0]\nhalt");
    let initial = machine.snapshot();
    for _ in 0..5 {
        machine.step().unwrap();
    }
    assert_eq!(machine.snapshot().output, b"AA");
    assert!(machine.snapshot().halted);
    machine.reverse_step().unwrap();
    assert!(!machine.snapshot().halted);
    machine.reverse_step().unwrap();
    assert_eq!(machine.snapshot().byte(0), Some(0x2f));
    machine.reverse_step().unwrap();
    machine.reverse_step().unwrap();
    assert_eq!(machine.snapshot().output, b"A");
    machine.reset().unwrap();
    assert_eq!(machine.snapshot(), initial);
    assert!(machine.reverse_step().is_err());
}

#[test]
fn reserved_bits_and_unknown_opcode_fault_atomically() {
    for bytes in [
        [0, 0],
        [0x41, 0x12],
        [0xa1, 0x10],
        [0xc0, 1],
        [0xc1, 0],
        [0xd1, 0x12],
        [0xe1, 0x12],
    ] {
        assert!(vole_isa_vole::decode(0, &bytes).is_err());
        let mut machine = make_machine(&format!("db {}, {}", bytes[0], bytes[1]));
        let before = machine.snapshot();
        assert!(machine.step().is_err());
        assert_eq!(machine.snapshot(), before);
    }
}

#[test]
fn fetch_and_pc_wrap_and_odd_targets_are_byte_addressed() {
    let mut machine = make_machine("org 0\ndb 3Ah\norg 0FFh\ndb 21h");
    machine.write_register("PC", 255).unwrap();
    let record = machine.step().unwrap();
    assert_eq!(record.instruction.bytes, [0x21, 0x3a]);
    assert_eq!(record.pc_after, 1);
    assert_eq!(machine.snapshot().register("R1"), Some(58));
    let mut odd = make_machine("jmp 3\ndb 0\nhalt");
    odd.step().unwrap();
    assert_eq!(odd.snapshot().pc, 3);
    assert!(odd.step().unwrap().halted);
}

#[test]
fn bounds_overlap_undefined_labels_and_strings_report_source_lines() {
    for (source, line) in [
        ("org 255\nhalt", 2),
        ("load R1, 256", 1),
        ("load R1, -129", 1),
        ("load R1, absent", 1),
        ("org 0\nhalt\norg 1\ndb 0", 4),
        ("load RG, 1", 1),
        ("db \"é\"", 1),
        ("load R1, 1\nload R1, [256]", 2),
        ("ror R1, 16", 1),
        ("a: halt\na: halt", 2),
    ] {
        let errors = assemble(source).unwrap_err();
        assert!(
            errors.iter().any(|error| error.line == line),
            "{source}: {errors:?}"
        );
    }
}

#[test]
fn debugger_edits_clear_history_but_invalid_edits_preserve_it() {
    let mut machine = make_machine("load R1, 9\nhalt");
    machine.step().unwrap();
    let before = machine.snapshot();
    assert!(machine.write_register("R1", 256).is_err());
    assert!(machine.write_memory(255, &[1, 2]).is_err());
    assert_eq!(machine.snapshot(), before);
    machine.reverse_step().unwrap();
    machine.step().unwrap();
    machine.write_register("R2", 7).unwrap();
    assert!(machine.reverse_step().is_err());
    machine.write_register("RF", 65).unwrap();
    assert!(machine.snapshot().output.is_empty());
}

#[test]
fn reverse_history_stops_at_retention_boundary() {
    let mut machine = make_machine("jmp 0");
    for _ in 0..=vole_isa_vole::HISTORY_LIMIT {
        machine.step().unwrap();
    }
    assert_eq!(machine.snapshot().trace.len(), vole_isa_vole::HISTORY_LIMIT);
    for _ in 0..vole_isa_vole::HISTORY_LIMIT {
        machine.reverse_step().unwrap();
    }
    assert_eq!(machine.snapshot().steps, 1);
    let before = machine.snapshot();
    assert!(machine.reverse_step().is_err());
    assert_eq!(machine.snapshot(), before);
}

#[test]
fn bundled_original_examples_run_to_their_expected_results() {
    let fixtures = [
        (
            include_str!("../../../examples/vole/addition.asm"),
            "addition",
        ),
        (include_str!("../../../examples/vole/output.asm"), "output"),
        (
            include_str!("../../../examples/vole/signed-branch.asm"),
            "signed",
        ),
        (include_str!("../../../examples/vole/float.asm"), "float"),
    ];
    for (source, name) in fixtures {
        let mut machine = make_machine(source);
        for _ in 0..128 {
            if machine.step().unwrap().halted {
                break;
            }
        }
        let state = machine.snapshot();
        assert!(state.halted, "{name}");
        match name {
            "addition" => assert_eq!(state.byte(0xbb), Some(125)),
            "output" => assert_eq!(state.output, b"Vole ready.\n"),
            "signed" => assert_eq!(state.output, b"Y"),
            "float" => assert_eq!(state.byte(128), Some(0x58)),
            _ => unreachable!(),
        }
    }
}

#[test]
fn invalid_load_does_not_replace_a_running_program() {
    let mut machine = make_machine("load R1, 9\nhalt");
    machine.step().unwrap();
    let before = machine.snapshot();
    let mut invalid = assemble("halt").unwrap();
    invalid.entry = 256;
    assert!(machine.load(&invalid).is_err());
    assert_eq!(machine.snapshot(), before);
    invalid.entry = 0;
    invalid.initial_registers.insert("R1".into(), 256);
    assert!(machine.load(&invalid).is_err());
    assert_eq!(machine.snapshot(), before);
    invalid.initial_registers.clear();
    invalid.regions[0].base = 255;
    assert!(machine.load(&invalid).is_err());
    assert_eq!(machine.snapshot(), before);
}
