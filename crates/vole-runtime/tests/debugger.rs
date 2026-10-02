use vole_core::Architecture;
use vole_runtime::{Command, RunState, Session};

fn sample() -> Session {
    let mut session = Session::new(Architecture::Vole, String::new());
    session
        .assemble(
            Architecture::Vole,
            Architecture::Vole.example_source().into(),
        )
        .unwrap();
    session
}

#[test]
fn user_can_assemble_step_reverse_run_and_reset() {
    let mut session = sample();
    session.apply(Command::Step).unwrap();
    assert_eq!(
        session.view().snapshot.as_ref().unwrap().register("R1"),
        Some(0x3A)
    );
    session.apply(Command::Reverse).unwrap();
    assert_eq!(
        session.view().snapshot.as_ref().unwrap().register("R1"),
        Some(0)
    );
    session.apply(Command::Run).unwrap();
    while session.view().state == RunState::Running {
        session.run_batch(128);
    }
    assert_eq!(session.view().state, RunState::Halted);
    assert_eq!(
        session.view().snapshot.as_ref().unwrap().byte(0xBB),
        Some(0x7D)
    );
    session.apply(Command::Reset).unwrap();
    assert_eq!(
        session.view().snapshot.as_ref().unwrap().byte(0xBB),
        Some(0)
    );
}

#[test]
fn breakpoint_stops_before_execution_and_resume_passes_it_once() {
    let mut session = sample();
    session.apply(Command::ToggleBreakpoint(4)).unwrap();
    session.apply(Command::Run).unwrap();
    session.run_batch(128);
    let snapshot = session.view().snapshot.as_ref().unwrap();
    assert_eq!((snapshot.pc, snapshot.register("R3")), (4, Some(0)));
    assert!(session.view().message.contains("breakpoint"));
    session.apply(Command::Run).unwrap();
    session.run_batch(128);
    assert_eq!(session.view().state, RunState::Halted);
}

#[test]
fn source_edits_and_failed_assembly_cannot_run_stale_bytes() {
    let mut session = sample();
    session
        .apply(Command::MarkStale("load R10, 4".into()))
        .unwrap();
    assert!(session.apply(Command::Step).is_err());
    assert!(
        session
            .assemble(Architecture::Vole, "load R10, 4".into())
            .is_err()
    );
    assert!(!session.view().diagnostics.is_empty());
    assert!(session.apply(Command::Run).is_err());
}

#[test]
fn write_watchpoint_stops_after_the_store_and_reset_keeps_breakpoints() {
    let mut session = sample();
    session.apply(Command::ToggleBreakpoint(8)).unwrap();
    session
        .apply(Command::SetWatchpoint {
            address: 0xBB,
            length: 1,
            write: true,
        })
        .unwrap();
    session.apply(Command::Run).unwrap();
    session.run_batch(128);
    assert!(session.view().message.contains("Watchpoint"));
    assert_eq!(
        session.view().snapshot.as_ref().unwrap().byte(0xBB),
        Some(0x7D)
    );
    session.apply(Command::Reset).unwrap();
    assert!(session.view().breakpoints.contains(&8));
}

#[test]
fn machine_edits_require_pause_and_clear_history() {
    let mut session = sample();
    session.apply(Command::Run).unwrap();
    assert!(
        session
            .apply(Command::EditRegister {
                name: "R1".into(),
                value: 2
            })
            .is_err()
    );
    session.apply(Command::Pause).unwrap();
    session.apply(Command::Step).unwrap();
    session
        .apply(Command::EditMemory {
            address: 0xBB,
            bytes: vec![42],
        })
        .unwrap();
    assert_eq!(
        session.view().snapshot.as_ref().unwrap().byte(0xBB),
        Some(42)
    );
    assert!(session.apply(Command::Reverse).is_err());
}

#[test]
fn breakpoint_at_entry_stops_on_first_run_and_resumes_once() {
    let mut session = sample();
    session.apply(Command::ToggleBreakpoint(0)).unwrap();
    session.apply(Command::Run).unwrap();
    session.run_batch(128);
    assert_eq!(session.view().snapshot.as_ref().unwrap().steps, 0);
    assert_eq!(session.view().state, RunState::Paused);
    session.apply(Command::Run).unwrap();
    session.run_batch(128);
    assert_eq!(session.view().state, RunState::Halted);
}

#[test]
fn read_watchpoint_uses_data_reads_and_ignores_instruction_fetch() {
    let mut session = Session::new(Architecture::Vole, String::new());
    session
        .assemble(
            Architecture::Vole,
            "load R1, [80h]\nhalt\norg 80h\ndb 42".into(),
        )
        .unwrap();
    session
        .apply(Command::SetWatchpoint {
            address: 0x80,
            length: 1,
            write: false,
        })
        .unwrap();
    session.apply(Command::Run).unwrap();
    session.run_batch(128);
    assert_eq!(session.view().snapshot.as_ref().unwrap().pc, 2);
    assert!(session.view().message.contains("Watchpoint"));
}

#[test]
fn invalid_edit_commands_explain_the_failure_in_the_published_view() {
    let mut session = sample();
    let revision = session.view().revision;
    assert!(session.apply(Command::ToggleBreakpoint(256)).is_err());
    assert!(session.view().message.contains("Address"));
    assert!(session.view().revision > revision);
}

#[test]
fn persisted_machine_state_resumes_without_reassembling_or_old_undo() {
    let mut session = sample();
    session.apply(Command::Step).unwrap();
    session.apply(Command::Step).unwrap();
    let program = session.view().program.clone().unwrap();
    let snapshot = session.view().snapshot.clone().unwrap();
    let mut restored = Session::new(Architecture::Vole, String::new());
    restored
        .apply(Command::Restore {
            program,
            snapshot: Some(snapshot),
        })
        .unwrap();
    assert_eq!(restored.view().snapshot.as_ref().unwrap().pc, 4);
    assert!(restored.apply(Command::Reverse).is_err());
    restored.apply(Command::Step).unwrap();
    assert_eq!(
        restored.view().snapshot.as_ref().unwrap().register("R3"),
        Some(0x7D)
    );
}

#[test]
fn invalid_saved_state_cannot_inject_out_of_width_values() {
    let mut session = sample();
    session.apply(Command::Step).unwrap();
    session.apply(Command::Step).unwrap();
    let previous = session.view().snapshot.clone().unwrap();
    let program = session.view().program.clone().unwrap();
    let mut snapshot = session.view().snapshot.clone().unwrap();
    snapshot.registers[1].value = 256;
    assert!(
        session
            .apply(Command::Restore {
                program,
                snapshot: Some(snapshot)
            })
            .is_err()
    );
    assert_eq!(session.view().snapshot.as_ref().unwrap(), &previous);
}

#[test]
fn imported_machine_bytes_execute_without_source_assembly() {
    let program = vole_runtime::import_bytes(
        Architecture::Vole,
        &[0x21, 0x3A, 0x22, 0x43, 0x53, 0x12, 0x33, 0xBB, 0xC0, 0x00],
    )
    .unwrap();
    let mut session = Session::new(Architecture::Vole, String::new());
    session.apply(Command::LoadProgram(program)).unwrap();
    session.apply(Command::Run).unwrap();
    while session.view().state == RunState::Running {
        session.run_batch(128);
    }
    assert_eq!(
        session.view().snapshot.as_ref().unwrap().byte(0xBB),
        Some(125)
    );
}
