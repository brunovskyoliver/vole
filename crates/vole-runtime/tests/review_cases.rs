//! Regression cases found during independent review of the debugger boundary.
use std::time::{Duration, Instant};
use vole_core::Architecture;
use vole_runtime::{Command, RunState, Runtime, Session, export_bytes};

fn assembled(source: &str) -> Session {
    let mut session = Session::new(Architecture::Vole, source.into());
    session.assemble(Architecture::Vole, source.into()).unwrap();
    session
}

#[test]
fn reset_restores_source_associations_after_an_executable_memory_edit() {
    let mut session = assembled("load R1, 58\nhalt\n");
    assert_eq!(
        session
            .view()
            .current_instruction
            .as_ref()
            .unwrap()
            .source_line,
        Some(1)
    );
    session
        .apply(Command::EditMemory {
            address: 1,
            bytes: vec![59],
        })
        .unwrap();
    assert_eq!(
        session
            .view()
            .current_instruction
            .as_ref()
            .unwrap()
            .source_line,
        None
    );
    let edited = session
        .view()
        .disassembly
        .iter()
        .find(|instruction| instruction.address == 0)
        .unwrap();
    assert_eq!(edited.bytes, [0x21, 59]);
    assert_eq!(edited.source_line, None);
    assert_eq!(
        session
            .view()
            .program
            .as_ref()
            .unwrap()
            .instruction(0)
            .unwrap()
            .bytes,
        [0x21, 58]
    );
    session.apply(Command::Reset).unwrap();
    assert_eq!(session.view().snapshot.as_ref().unwrap().byte(1), Some(58));
    assert_eq!(
        session
            .view()
            .current_instruction
            .as_ref()
            .unwrap()
            .source_line,
        Some(1)
    );
}

#[test]
fn manually_stepping_a_loop_breakpoint_consumes_the_old_resume_marker() {
    let mut session = assembled("jmp 0\n");
    session.apply(Command::ToggleBreakpoint(0)).unwrap();
    session.apply(Command::Run).unwrap();
    session.run_batch(128);
    assert_eq!(session.view().snapshot.as_ref().unwrap().steps, 0);
    session.apply(Command::Step).unwrap();
    assert_eq!(session.view().snapshot.as_ref().unwrap().steps, 1);
    session.apply(Command::Run).unwrap();
    session.run_batch(128);
    assert_eq!(session.view().snapshot.as_ref().unwrap().steps, 1);
    assert!(session.view().message.contains("breakpoint"));
}

#[test]
fn raw_export_rejects_an_entry_that_the_fixed_origin_import_cannot_represent() {
    let session =
        assembled("org 40h\njmp done\nload R1, 1\ndone:\nload R1, 125\nstore R1, [0BBh]\nhalt\n");
    let image = session.view().program.as_ref().unwrap();
    // A raw image has no entry metadata. Returning a relocated byte stream here
    // silently changes absolute branch addresses when imported at address zero.
    assert!(export_bytes(image).is_err());
}

#[test]
fn raw_export_rejects_mappings_outside_the_importable_span() {
    let mut image = vole_isa_scalar::assemble(
        Architecture::X64,
        ".intel_syntax noprefix\n.text\n.global _start\n_start:\nint3\n",
    )
    .unwrap();
    image.regions.push(vole_core::MemoryRegion {
        base: 0x20000,
        bytes: vec![125],
        writable: true,
        executable: false,
        label: "Extra teaching data".into(),
    });
    let mut machine = vole_isa_scalar::ScalarMachine::new(Architecture::X64);
    vole_core::Machine::load(&mut machine, &image).unwrap();
    assert!(
        export_bytes(&image)
            .unwrap_err()
            .0
            .contains("Save a project")
    );
}

#[test]
fn runtime_worker_can_pause_a_long_loop_without_consuming_its_run_budget() {
    let runtime = Runtime::new(Architecture::Vole, "jmp 0\n".into());
    let deadline = Instant::now() + Duration::from_secs(3);
    while runtime.view().state != RunState::Ready {
        assert!(Instant::now() < deadline, "worker did not assemble");
        std::thread::sleep(Duration::from_millis(2));
    }
    runtime.send(Command::Run).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while runtime
        .view()
        .snapshot
        .as_ref()
        .is_none_or(|snapshot| snapshot.steps == 0)
    {
        assert!(Instant::now() < deadline, "worker did not run");
        std::thread::sleep(Duration::from_millis(2));
    }
    let start = Instant::now();
    runtime.send(Command::Pause).unwrap();
    while runtime.view().state != RunState::Paused {
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "pause did not yield promptly"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    let stopped = runtime.view().snapshot.as_ref().unwrap().steps;
    assert!(stopped < vole_runtime::RUN_INSTRUCTION_BUDGET);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(runtime.view().snapshot.as_ref().unwrap().steps, stopped);
    runtime.send(Command::Shutdown).unwrap();
}

#[test]
fn assembly_publication_advances_revision_and_marks_the_previous_image_stale() {
    let runtime = Runtime::new(
        Architecture::Arm64,
        Architecture::Arm64.example_source().into(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.view().state != RunState::Ready {
        assert!(Instant::now() < deadline, "initial assembly did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
    let revision = runtime.view().revision;
    let source = format!(
        ".text\n.global _start\n_start:\n{}brk #0\n",
        "mov x0, #1\n".repeat(400)
    );
    runtime
        .send(Command::Assemble {
            architecture: Architecture::Arm64,
            source,
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let view = runtime.view();
        if view.state == RunState::Assembling {
            assert!(view.revision > revision);
            assert!(view.dirty);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "assembly state was not published"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    runtime.send(Command::Shutdown).unwrap();
}

#[test]
fn watchpoint_ranges_are_validated_before_the_debugger_changes() {
    let mut session = assembled("halt\n");
    session
        .apply(Command::SetWatchpoint {
            address: 0xfe,
            length: 2,
            write: false,
        })
        .unwrap();
    let watchpoints = session.view().watchpoints.clone();
    assert!(
        session
            .apply(Command::SetWatchpoint {
                address: 0xfe,
                length: 3,
                write: false
            })
            .is_err()
    );
    assert_eq!(session.view().watchpoints, watchpoints);
}

#[test]
fn editing_a_scalar_flag_changes_the_branch_and_clears_undo() {
    let source = ".intel_syntax noprefix\n.text\n.global _start\n_start:\nmov eax, 0\ncmp eax, 0\njne changed\nmov byte ptr [8192], 125\nint3\nchanged:\nmov byte ptr [8192], 1\nint3\n";
    let mut session = Session::new(Architecture::X64, source.into());
    session.assemble(Architecture::X64, source.into()).unwrap();
    session.apply(Command::Step).unwrap();
    session.apply(Command::Step).unwrap();
    assert!(session.view().snapshot.as_ref().unwrap().flags["ZF"]);
    session
        .apply(Command::EditFlag {
            name: "ZF".into(),
            value: false,
        })
        .unwrap();
    assert!(session.view().snapshot.as_ref().unwrap().trace.is_empty());
    assert!(session.apply(Command::Reverse).is_err());
    session.apply(Command::Run).unwrap();
    while session.view().state == RunState::Running {
        session.run_batch(128);
    }
    assert_eq!(session.view().state, RunState::Halted);
    assert_eq!(
        session.view().snapshot.as_ref().unwrap().byte(0x2000),
        Some(1)
    );
}

#[test]
fn guest_self_modification_and_reversal_update_live_disassembly() {
    let mut session = assembled("load R1, 59\nload R2, 60\nstore R2, [1]\njmp 0\n");
    for _ in 0..3 {
        session.apply(Command::Step).unwrap();
    }
    let edited = session
        .view()
        .disassembly
        .iter()
        .find(|instruction| instruction.address == 0)
        .unwrap();
    assert_eq!(edited.bytes, [0x21, 60]);
    assert_eq!(edited.source_line, None);
    session.apply(Command::Reverse).unwrap();
    let restored = session
        .view()
        .disassembly
        .iter()
        .find(|instruction| instruction.address == 0)
        .unwrap();
    assert_eq!(restored.bytes, [0x21, 59]);
    assert_eq!(restored.source_line, Some(1));
}

#[test]
fn reversing_a_loop_breakpoint_does_not_keep_a_stale_resume_marker() {
    let mut session = assembled("jmp 0\n");
    session.apply(Command::ToggleBreakpoint(0)).unwrap();
    session.apply(Command::Run).unwrap();
    session.run_batch(128);
    session.apply(Command::Run).unwrap();
    session.run_batch(128);
    assert_eq!(session.view().snapshot.as_ref().unwrap().steps, 1);
    session.apply(Command::Reverse).unwrap();
    assert_eq!(session.view().snapshot.as_ref().unwrap().steps, 0);
    session.apply(Command::Run).unwrap();
    session.run_batch(128);
    assert_eq!(session.view().snapshot.as_ref().unwrap().steps, 0);
}

#[test]
fn a_successfully_assembled_image_with_invalid_entry_cannot_run_stale_bytes() {
    let mut session = Session::new(Architecture::Arm64, String::new());
    session
        .assemble(
            Architecture::Arm64,
            Architecture::Arm64.example_source().into(),
        )
        .unwrap();
    let before = session.view().snapshot.clone().unwrap();
    let source = ".text\n.byte 0\n.global _start\n_start:\n    mov x0, #1\n    brk #0\n";
    // LLVM accepts this source, but the A64 entry is one byte off alignment.
    let image = vole_isa_scalar::assemble(Architecture::Arm64, source).unwrap();
    assert_eq!(image.entry, 0x1001);
    assert!(
        session
            .apply(Command::Assemble {
                architecture: Architecture::Arm64,
                source: source.into()
            })
            .is_err()
    );
    assert_eq!(session.view().state, RunState::Editing);
    assert!(session.view().dirty);
    assert!(!session.view().diagnostics.is_empty());
    assert!(session.view().message.contains("aligned"));
    assert_eq!(session.view().snapshot.as_ref().unwrap(), &before);
    assert!(session.apply(Command::Run).is_err());
    assert!(session.apply(Command::Step).is_err());
    session
        .assemble(
            Architecture::Arm64,
            Architecture::Arm64.example_source().into(),
        )
        .unwrap();
    assert_eq!(session.view().state, RunState::Ready);
    assert!(!session.view().dirty);
}
