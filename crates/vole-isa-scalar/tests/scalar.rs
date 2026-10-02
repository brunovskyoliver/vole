use vole_core::{Architecture, Machine, MemoryRegion, Program};
use vole_isa_scalar::{ScalarMachine, assemble, decode, toolchain_status};

fn program(architecture: Architecture, body: &str) -> Program {
    let header = if architecture == Architecture::Arm32 {
        ".syntax unified\n.arm\n"
    } else if matches!(architecture, Architecture::X86 | Architecture::X64) {
        ".intel_syntax noprefix\n"
    } else {
        ""
    };
    assemble(
        architecture,
        &format!("{header}.text\n.global _start\n_start:\n{body}\n"),
    )
    .unwrap_or_else(|errors| panic!("{architecture:?}: {errors:?}"))
}
fn loaded(program: &Program) -> ScalarMachine {
    let mut machine = ScalarMachine::new(program.architecture);
    machine.load(program).unwrap();
    machine
}
fn finish(machine: &mut ScalarMachine) {
    for _ in 0..1000 {
        if machine.snapshot().halted {
            return;
        }
        machine.step().unwrap_or_else(|error| panic!("{error}"));
    }
    panic!("Program failed to halt within 1000 instructions");
}

#[test]
fn every_architecture_assembles_real_bytes_stores_125_and_reverses() {
    toolchain_status().expect("Tests require LLVM MC and LLD 14+ on PATH");
    for architecture in [
        Architecture::Arm32,
        Architecture::Arm64,
        Architecture::X86,
        Architecture::X64,
    ] {
        let program = assemble(architecture, architecture.example_source()).unwrap();
        let first = &program.instructions[0];
        assert!(first.source_line.is_some());
        let expected: &[u8] = match architecture {
            Architecture::Arm32 => &[0x3a, 0x10, 0xa0, 0xe3],
            Architecture::Arm64 => &[0x41, 0x07, 0x80, 0xd2],
            Architecture::X86 => &[0xb8, 0x3a, 0, 0, 0],
            Architecture::X64 => &[0x48, 0xc7, 0xc0, 0x3a, 0, 0, 0],
            _ => unreachable!(),
        };
        assert_eq!(first.bytes, expected, "{architecture:?}");
        let mut machine = loaded(&program);
        let initial = machine.snapshot();
        finish(&mut machine);
        assert_eq!(machine.snapshot().read(0x2000, 4).unwrap(), [125, 0, 0, 0]);
        let count = machine.snapshot().steps;
        for _ in 0..count {
            machine.reverse_step().unwrap();
        }
        assert_eq!(machine.snapshot(), initial, "{architecture:?}");
        finish(&mut machine);
        machine.reset().unwrap();
        assert_eq!(machine.snapshot(), initial);
    }
}

#[test]
fn arm64_width_aliases_flags_conditions_and_stack_call() {
    let program = program(
        Architecture::Arm64,
        "mov x0, #-1\nmov w0, #58\nmov x1, #67\nbl sum\ncmp x0, #125\nb.ne failed\nmov x4, #8192\nstr x0, [x4]\nmov xzr, #9\nmov x5, xzr\nbrk #0\nfailed:\nmov x0, #0\nbrk #0\nsum:\nstp x29, x30, [sp, #-16]!\nmov x29, sp\nadd x0, x0, x1\nldp x29, x30, [sp], #16\nret",
    );
    let mut machine = loaded(&program);
    finish(&mut machine);
    let snapshot = machine.snapshot();
    assert_eq!(snapshot.register("x0"), Some(125));
    assert_eq!(snapshot.register("x5"), Some(0));
    assert_eq!(snapshot.register("sp"), Some(0x20000));
    assert!(snapshot.flags["Z"]);
    assert_eq!(
        snapshot.read(0x2000, 8).unwrap(),
        [125, 0, 0, 0, 0, 0, 0, 0]
    );
    assert!(snapshot.trace.iter().any(|record| {
        record
            .memory_reads
            .iter()
            .any(|read| read.address == 0x1fff0)
    }));
}

#[test]
fn arm32_conditional_execution_pc_pipeline_stack_and_flags() {
    let program = program(
        Architecture::Arm32,
        "mov r0, #0\ncmp r0, #0\nmovne r1, #99\nmoveq r1, #58\nmov r2, #67\nbl sum\nmov r4, #8192\nstr r1, [r4]\nmov r6, pc\nbkpt #0\nsum:\npush {r4, lr}\nadd r1, r1, r2\npop {r4, pc}",
    );
    let pc_read_address = program
        .instructions
        .iter()
        .find(|i| i.assembly == "mov r6, pc")
        .unwrap()
        .address;
    let mut machine = loaded(&program);
    finish(&mut machine);
    let snapshot = machine.snapshot();
    assert_eq!(snapshot.register("r1"), Some(125));
    assert_eq!(snapshot.register("r6"), Some(pc_read_address + 8));
    assert_eq!(snapshot.register("r13"), Some(0x20000));
    assert_eq!(snapshot.read(0x2000, 4).unwrap(), [125, 0, 0, 0]);
    assert!(snapshot.flags["Z"]);
}

#[test]
fn arm64_word_and_signed_byte_loads_post_index_and_address_symbols() {
    let program = program(
        Architecture::Arm64,
        "adr x0, value\nldrb w1, [x0]\nldrsb x2, [x0], #1\nldrb w3, [x0]\nmov x4, #8192\nstrb w3, [x4, #1]\nbrk #0\n.data\nvalue: .byte 255, 125",
    );
    let mut machine = loaded(&program);
    finish(&mut machine);
    let snapshot = machine.snapshot();
    assert_eq!(snapshot.register("x1"), Some(255));
    assert_eq!(snapshot.register("x2"), Some(u64::MAX));
    assert_eq!(snapshot.register("x3"), Some(125));
    assert_eq!(snapshot.byte(0x2001), Some(125));
}

#[test]
fn arm32_literal_load_relocations_and_register_indexed_memory() {
    let program = program(
        Architecture::Arm32,
        "ldr r0, =value\nmov r1, #1\nldrb r2, [r0, r1]\nmov r4, #8192\nstrb r2, [r4, #5]!\nbkpt #0\n.data\nvalue: .byte 2, 125",
    );
    let mut machine = loaded(&program);
    finish(&mut machine);
    let snapshot = machine.snapshot();
    assert_eq!(snapshot.register("r2"), Some(125));
    assert_eq!(snapshot.register("r4"), Some(0x2005));
    assert_eq!(snapshot.byte(0x2005), Some(125));
}

#[test]
fn x86_aliases_signed_condition_stack_and_variable_lengths() {
    for architecture in [Architecture::X86, Architecture::X64] {
        let (accumulator, base, stack) = if architecture == Architecture::X64 {
            ("rax", "rbx", "rsp")
        } else {
            ("eax", "ebx", "esp")
        };
        let body = format!(
            "mov {accumulator}, -1\nmov al, 58\nmov ah, 0\nmov eax, 58\nmov {base}, 67\ncall sum\ncmp eax, 125\njne failed\nmov dword ptr [8192], eax\nint3\nfailed:\nmov eax, 0\nint3\nsum:\npush {base}\nadd eax, ebx\npop {base}\nret"
        );
        let program = program(architecture, &body);
        let mut machine = loaded(&program);
        finish(&mut machine);
        let snapshot = machine.snapshot();
        assert_eq!(snapshot.register(accumulator), Some(125));
        assert_eq!(snapshot.register(stack), Some(0x20000));
        assert_eq!(snapshot.read(0x2000, 4).unwrap(), [125, 0, 0, 0]);
        assert!(snapshot.flags["ZF"]);
        let lengths: std::collections::BTreeSet<_> = snapshot
            .trace
            .iter()
            .map(|r| r.instruction.bytes.len())
            .collect();
        assert!(lengths.len() >= 3);
    }
}

#[test]
fn x64_rip_relative_load_and_high_byte_preservation() {
    let program = program(
        Architecture::X64,
        "lea rbx, [rip + value]\nmov rax, qword ptr [rbx]\nmov ah, 0x7d\nmov r8d, eax\nmov qword ptr [8192], r8\nint3\n.data\nvalue: .quad 0x1122334455667788",
    );
    let mut machine = loaded(&program);
    finish(&mut machine);
    let snapshot = machine.snapshot();
    assert_eq!(snapshot.register("rax"), Some(0x1122334455667d88));
    assert_eq!(snapshot.register("r8"), Some(0x55667d88));
    assert_eq!(snapshot.register("rbx"), Some(0x2000));
}

#[test]
fn actual_bytes_execute_after_source_and_predecoded_instructions_are_removed() {
    for architecture in [
        Architecture::Arm32,
        Architecture::Arm64,
        Architecture::X86,
        Architecture::X64,
    ] {
        let mut program = assemble(architecture, architecture.example_source()).unwrap();
        program.source.clear();
        program.instructions.clear();
        let mut machine = loaded(&program);
        match architecture {
            Architecture::Arm32 => machine.write_memory(0x1000, &[59]).unwrap(),
            Architecture::Arm64 => {
                let first = machine.snapshot().read(0x1000, 4).unwrap();
                let word = u32::from_le_bytes(first.try_into().unwrap()) + (1 << 5);
                machine.write_memory(0x1000, &word.to_le_bytes()).unwrap();
            }
            Architecture::X86 => machine.write_memory(0x1001, &[59]).unwrap(),
            Architecture::X64 => machine.write_memory(0x1003, &[59]).unwrap(),
            _ => unreachable!(),
        }
        finish(&mut machine);
        assert_eq!(
            machine.snapshot().byte(0x2000),
            Some(126),
            "{architecture:?}"
        );
    }
}

#[test]
fn faults_are_atomic_for_a_partially_completed_pair_store() {
    let mut program = program(
        Architecture::Arm64,
        "mov x0, #0x7d\nmov x1, #0x3a\nmov x2, #0x3000\nstp x0, x1, [x2]\nbrk #0",
    );
    program.regions.retain(|r| r.executable);
    program.regions.push(MemoryRegion {
        base: 0x3000,
        bytes: vec![0; 8],
        writable: true,
        executable: false,
        label: "Eight bytes only".into(),
    });
    let mut machine = loaded(&program);
    for _ in 0..3 {
        machine.step().unwrap();
    }
    let before = machine.snapshot();
    let error = machine.step().unwrap_err();
    assert!(error.0.contains("Unmapped"));
    assert_eq!(machine.snapshot(), before);
}

#[test]
fn faults_are_atomic_for_x86_pop_to_invalid_memory_and_unknown_instruction() {
    let program = program(
        Architecture::X64,
        "mov eax, 125\npush rax\npop qword ptr [0x400000]\nud2\nint3",
    );
    let mut machine = loaded(&program);
    machine.step().unwrap();
    machine.step().unwrap();
    let before = machine.snapshot();
    assert!(machine.step().is_err());
    assert_eq!(machine.snapshot(), before);
    machine
        .write_register("rip", program.instructions[3].address)
        .unwrap();
    let before = machine.snapshot();
    assert!(machine.step().unwrap_err().0.contains("Unsupported"));
    assert_eq!(machine.snapshot(), before);
}

#[test]
fn deterministic_output_and_syscall_exit_are_reversible() {
    for (architecture, body) in [
        (Architecture::Arm32, "mov r0, #86\nbkpt #1\nbkpt #0"),
        (Architecture::Arm64, "mov x0, #86\nbrk #1\nbrk #0"),
        (
            Architecture::X86,
            "mov eax, 4\nmov ebx, 1\nmov ecx, offset message\nmov edx, 1\nint 0x80\nmov eax, 1\nint 0x80\n.data\nmessage: .ascii \"V\"",
        ),
        (
            Architecture::X64,
            "mov eax, 1\nmov edi, 1\nlea rsi, [rip + message]\nmov edx, 1\nsyscall\nmov eax, 60\nsyscall\n.data\nmessage: .ascii \"V\"",
        ),
    ] {
        let program = program(architecture, body);
        let mut machine = loaded(&program);
        let initial = machine.snapshot();
        finish(&mut machine);
        assert_eq!(machine.snapshot().output, b"V", "{architecture:?}");
        while machine.snapshot().steps > 0 {
            machine.reverse_step().unwrap();
        }
        assert_eq!(machine.snapshot(), initial);
    }
}

#[test]
fn load_rejects_overlaps_and_overflow_without_destroying_loaded_state() {
    let program = assemble(Architecture::X64, Architecture::X64.example_source()).unwrap();
    let mut machine = loaded(&program);
    machine.step().unwrap();
    let before = machine.snapshot();
    let mut invalid = program.clone();
    invalid.regions.push(invalid.regions[0].clone());
    assert!(machine.load(&invalid).is_err());
    assert_eq!(machine.snapshot(), before);
    let mut invalid = program.clone();
    invalid.regions.push(MemoryRegion {
        base: u64::MAX,
        bytes: vec![0, 0],
        writable: true,
        executable: false,
        label: "Overflow".into(),
    });
    assert!(machine.load(&invalid).is_err());
    assert_eq!(machine.snapshot(), before);
}

#[test]
fn diagnostics_map_to_original_lines_and_external_files_are_rejected() {
    let errors = assemble(
        Architecture::Arm64,
        ".text\n.global _start\n_start:\nmov x0, #1\ncompletely_invalid x1\n",
    )
    .unwrap_err();
    assert_eq!(errors[0].line, 5, "{errors:?}");
    for directive in [
        ".include \"/etc/passwd\"",
        ".incbin \"/etc/passwd\"",
        ".rept 999999",
        ".zero 999999999",
    ] {
        let errors = assemble(Architecture::Arm64, directive).unwrap_err();
        assert!(!errors.is_empty());
    }
}

#[test]
fn decode_reports_the_actual_instruction_size_and_truncated_bytes() {
    assert_eq!(
        decode(Architecture::X64, 0x1000, &[0x48, 0x83, 0xc0, 1])
            .unwrap()
            .assembly,
        "add rax, 1"
    );
    assert_eq!(
        decode(Architecture::Arm64, 0x1000, &[0x41, 0x07, 0x80, 0xd2])
            .unwrap()
            .assembly,
        "mov x1, #0x3a"
    );
    assert!(decode(Architecture::Arm64, 0x1001, &[0; 4]).is_err());
    assert!(decode(Architecture::X64, 0x1000, &[0x48, 0x83]).is_err());
}

#[test]
fn saved_state_restores_all_guest_state_and_rejects_invalid_snapshots_atomically() {
    for architecture in [
        Architecture::Arm32,
        Architecture::Arm64,
        Architecture::X86,
        Architecture::X64,
    ] {
        let program = assemble(architecture, architecture.example_source()).unwrap();
        let mut machine = loaded(&program);
        finish(&mut machine);
        let mut saved = machine.snapshot();
        machine.reset().unwrap();
        machine.restore_snapshot(&saved).unwrap();
        saved.trace.clear();
        assert_eq!(machine.snapshot(), saved);
        assert!(machine.reverse_step().is_err());
        let before = machine.snapshot();
        let mut bad = saved.clone();
        bad.memory[0].writable = !bad.memory[0].writable;
        assert!(machine.restore_snapshot(&bad).is_err());
        assert_eq!(machine.snapshot(), before);
        let mut bad = saved.clone();
        bad.registers[0].bits = 8;
        assert!(machine.restore_snapshot(&bad).is_err());
        assert_eq!(machine.snapshot(), before);
    }
}

#[test]
fn flags_follow_architectural_overflow_and_unsigned_borrow_rules() {
    for (architecture, body, z, carry, negative, overflow) in [
        (
            Architecture::Arm32,
            "mov r0, #0\nsubs r1, r0, #1\nbkpt #0",
            "Z",
            "C",
            "N",
            "V",
        ),
        (
            Architecture::Arm64,
            "mov x0, #0\nsubs x1, x0, #1\nbrk #0",
            "Z",
            "C",
            "N",
            "V",
        ),
        (
            Architecture::X86,
            "mov eax, 0\nsub eax, 1\nint3",
            "ZF",
            "CF",
            "SF",
            "OF",
        ),
        (
            Architecture::X64,
            "mov rax, 0\nsub rax, 1\nint3",
            "ZF",
            "CF",
            "SF",
            "OF",
        ),
    ] {
        let program = program(architecture, body);
        let mut machine = loaded(&program);
        finish(&mut machine);
        let snapshot = machine.snapshot();
        assert!(!snapshot.flags[z]);
        assert!(snapshot.flags[negative]);
        assert!(!snapshot.flags[overflow]);
        assert_eq!(
            snapshot.flags[carry],
            !matches!(architecture, Architecture::Arm32 | Architecture::Arm64)
        );
    }
}

#[test]
fn inline_directives_cannot_read_host_files_or_allocate_unbounded_images() {
    for source in [
        ".text; .incbin \"/etc/passwd\"",
        "file: .include \"/etc/passwd\"",
        ".text; .zero 999999999",
        ".section .evil,\"ax\"",
    ] {
        assert!(assemble(Architecture::Arm64, source).is_err());
    }
}

#[test]
fn arm32_thumb_mode_and_invalid_instruction_branches_are_explicit_faults() {
    assert!(
        assemble(
            Architecture::Arm32,
            ".thumb\n.text\n.global _start\n_start:\nnop"
        )
        .is_err()
    );
    let program = program(Architecture::Arm32, "mov r0, #0x1001\nbx r0\nbkpt #0");
    let mut machine = loaded(&program);
    machine.step().unwrap();
    let before = machine.snapshot();
    assert!(machine.step().unwrap_err().0.contains("Thumb"));
    assert_eq!(machine.snapshot(), before);
}

#[test]
fn shifts_and_bitwise_operations_update_results_and_carry() {
    for (architecture, body, destination, carry) in [
        (
            Architecture::Arm32,
            "mov r0, #0x80000000\nmovs r1, r0, lsl #1\nmov r2, #125\norr r1, r1, r2\nbkpt #0",
            "r1",
            "C",
        ),
        (
            Architecture::Arm64,
            "mov x0, #7\nlsl x1, x0, #4\nmov x3, #13\norr x1, x1, x3\nmov x2, #0\nsubs x2, x2, xzr\nbrk #0",
            "x1",
            "C",
        ),
        (
            Architecture::X86,
            "mov eax, 0x80000000\nshl eax, 1\nmov ebx, 125\nlea eax, [eax + ebx]\nint3",
            "eax",
            "CF",
        ),
        (
            Architecture::X64,
            "mov rax, 0x8000000000000000\nshl rax, 1\nmov rbx, 125\nlea rax, [rax + rbx]\nint3",
            "rax",
            "CF",
        ),
    ] {
        let program = program(architecture, body);
        let mut machine = loaded(&program);
        finish(&mut machine);
        let snapshot = machine.snapshot();
        assert_eq!(snapshot.register(destination), Some(125));
        assert!(snapshot.flags[carry], "{architecture:?}");
    }
}

#[test]
fn arithmetic_signed_overflow_and_arm32_immediate_rotate_carry() {
    for (architecture, body, overflow) in [
        (
            Architecture::Arm32,
            "mov r0, #0x7fffffff\nadds r0, r0, #1\nbkpt #0",
            "V",
        ),
        (
            Architecture::Arm64,
            "mov w0, #0x7fffffff\nadds w0, w0, #1\nbrk #0",
            "V",
        ),
        (
            Architecture::X86,
            "mov eax, 0x7fffffff\nadd eax, 1\nint3",
            "OF",
        ),
        (
            Architecture::X64,
            "mov rax, 0x7fffffffffffffff\nadd rax, 1\nint3",
            "OF",
        ),
    ] {
        let program = program(architecture, body);
        let mut machine = loaded(&program);
        finish(&mut machine);
        assert!(machine.snapshot().flags[overflow], "{architecture:?}");
    }
    let program = program(Architecture::Arm32, "movs r0, #0x80000000\nbkpt #0");
    let mut machine = loaded(&program);
    finish(&mut machine);
    assert!(machine.snapshot().flags["C"]);
}

#[test]
fn shipped_samples_run_using_the_public_assembler_and_machine() {
    let samples = [
        (
            Architecture::Arm32,
            include_str!("../../../examples/arm32/sum.s"),
            false,
        ),
        (
            Architecture::Arm32,
            include_str!("../../../examples/arm32/loop-and-call.s"),
            false,
        ),
        (
            Architecture::Arm32,
            include_str!("../../../examples/arm32/hello.s"),
            true,
        ),
        (
            Architecture::Arm64,
            include_str!("../../../examples/arm64/sum.s"),
            false,
        ),
        (
            Architecture::Arm64,
            include_str!("../../../examples/arm64/loop-and-call.s"),
            false,
        ),
        (
            Architecture::Arm64,
            include_str!("../../../examples/arm64/hello.s"),
            true,
        ),
        (
            Architecture::X86,
            include_str!("../../../examples/x86/sum.s"),
            false,
        ),
        (
            Architecture::X86,
            include_str!("../../../examples/x86/loop-and-call.s"),
            false,
        ),
        (
            Architecture::X86,
            include_str!("../../../examples/x86/hello.s"),
            true,
        ),
        (
            Architecture::X64,
            include_str!("../../../examples/x64/sum.s"),
            false,
        ),
        (
            Architecture::X64,
            include_str!("../../../examples/x64/loop-and-call.s"),
            false,
        ),
        (
            Architecture::X64,
            include_str!("../../../examples/x64/hello.s"),
            true,
        ),
    ];
    for (architecture, source, output) in samples {
        let program = assemble(architecture, source).unwrap();
        let mut machine = loaded(&program);
        finish(&mut machine);
        if output {
            assert_eq!(machine.snapshot().output, b"Vole!\n");
        } else {
            assert_eq!(machine.snapshot().byte(0x2000), Some(125));
        }
    }
}

#[test]
fn arm64_extended_index_and_flag_setting_pc_writes_are_handled_explicitly() {
    let image = program(
        Architecture::Arm64,
        "mov x0, #8192\nmov w1, #1\nmov w2, #125\nstrb w2, [x0, w1, uxtw]\nldrb w3, [x0, w1, uxtw]\nbrk #0",
    );
    let mut machine = loaded(&image);
    finish(&mut machine);
    assert_eq!(machine.snapshot().register("x3"), Some(125));
    let image = program(Architecture::Arm32, "mov r0, #0x1000\nmovs pc, r0");
    let mut machine = loaded(&image);
    machine.step().unwrap();
    let before = machine.snapshot();
    assert!(machine.step().unwrap_err().0.contains("exception-return"));
    assert_eq!(machine.snapshot(), before);
}

#[test]
fn repeated_string_instructions_fault_promptly_and_state_is_unchanged() {
    let image = program(Architecture::X64, "mov rcx, 0x7fffffff\nrep movsb\nint3");
    let mut machine = loaded(&image);
    machine.step().unwrap();
    let before = machine.snapshot();
    let start = std::time::Instant::now();
    assert!(machine.step().unwrap_err().0.contains("Unsupported"));
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(machine.snapshot(), before);
}

#[test]
fn history_and_imported_step_counters_remain_bounded() {
    let image = program(Architecture::X64, "jmp _start");
    let mut machine = loaded(&image);
    for _ in 0..5000 {
        machine.step().unwrap();
    }
    let saved = machine.snapshot();
    assert_eq!(saved.steps, 5000);
    assert!(saved.trace.len() <= 4096);
    let mut oversized = saved;
    oversized.steps = u64::MAX;
    machine.restore_snapshot(&oversized).unwrap();
    let before = machine.snapshot();
    assert!(machine.step().unwrap_err().0.contains("counter"));
    assert_eq!(machine.snapshot(), before);
}
