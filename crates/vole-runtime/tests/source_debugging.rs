//! Source stepping and source breakpoints over a hand-built ARM64 C image.
#[path = "../../vole-debug/tests/support/synthetic.rs"]
mod synthetic;

use synthetic::{Fixture, fixture};
use vole_core::{
    Architecture,
    debug::{DebugView, ValueText},
};
use vole_runtime::{Command, RunState, Session, SourceStep};

fn session(fixture: &Fixture) -> Session {
    let mut session = Session::new(Architecture::Arm64, String::new());
    session
        .apply(Command::LoadProgram(fixture.program.clone()))
        .unwrap();
    session
}

fn source_step(session: &mut Session, kind: SourceStep) {
    session.apply(Command::SourceStep(kind)).unwrap();
    assert_eq!(session.view().stepping, Some(kind));
    session.run_to_stop();
    assert_eq!(session.view().stepping, None);
}

fn run(session: &mut Session) {
    session.apply(Command::Run).unwrap();
    session.run_to_stop();
}

fn debug(session: &Session) -> &DebugView {
    session
        .view()
        .debug
        .as_ref()
        .expect("paused C image has a debug view")
}

fn line(session: &Session) -> u32 {
    debug(session).location.as_ref().unwrap().line
}

fn pc(session: &Session) -> u64 {
    session.view().snapshot.as_ref().unwrap().pc
}

fn stack(session: &Session) -> Vec<(String, u32)> {
    debug(session)
        .frames
        .iter()
        .map(|f| (f.function.clone(), f.location.as_ref().unwrap().line))
        .collect()
}

fn global(session: &Session, name: &str) -> (String, bool) {
    let view = debug(session)
        .globals
        .iter()
        .find(|g| g.name == name)
        .unwrap();
    match &view.value {
        ValueText::Value(text) => (text.clone(), view.changed),
        ValueText::Unavailable(reason) => panic!("{name}: {reason}"),
    }
}

#[test]
fn step_into_from_startup_stops_after_mains_prologue() {
    let Some(fixture) = fixture() else { return };
    let mut session = session(&fixture);
    assert!(debug(&session).note.as_ref().unwrap().contains("startup"));
    source_step(&mut session, SourceStep::Into);
    assert_eq!(pc(&session), fixture.main_at(8));
    assert_eq!(line(&session), 7);
    assert_eq!(session.view().state, RunState::Paused);
    assert!(session.view().message.contains("line 7"));
}

#[test]
fn step_over_from_startup_code_enters_main_instead_of_running_it() {
    let Some(fixture) = fixture() else { return };
    let mut session = session(&fixture);
    source_step(&mut session, SourceStep::Over);
    assert_eq!(pc(&session), fixture.main_at(8));
}

#[test]
fn step_over_stays_in_main_and_step_into_enters_nested_calls() {
    let Some(fixture) = fixture() else { return };
    let mut session = session(&fixture);
    source_step(&mut session, SourceStep::Into);
    source_step(&mut session, SourceStep::Over);
    assert_eq!(line(&session), 8);
    source_step(&mut session, SourceStep::Over);
    assert_eq!(line(&session), 9);
    assert_eq!(stack(&session), [("main".to_string(), 9)]);

    session.apply(Command::Reset).unwrap();
    source_step(&mut session, SourceStep::Into);
    source_step(&mut session, SourceStep::Into);
    assert_eq!(line(&session), 8);
    source_step(&mut session, SourceStep::Into);
    assert_eq!(pc(&session), fixture.f + 8);
    assert_eq!(
        stack(&session),
        [("f".to_string(), 4), ("main".to_string(), 8)]
    );
    source_step(&mut session, SourceStep::Into);
    assert_eq!(
        stack(&session),
        [
            ("g".to_string(), 2),
            ("f".to_string(), 4),
            ("main".to_string(), 8)
        ]
    );
    source_step(&mut session, SourceStep::Out);
    assert_eq!(pc(&session), fixture.f + 12);
    assert_eq!(stack(&session)[0], ("f".to_string(), 5));
    source_step(&mut session, SourceStep::Out);
    assert_eq!(pc(&session), fixture.main_at(20));
    assert_eq!(stack(&session), [("main".to_string(), 9)]);
}

#[test]
fn step_over_follows_loop_iterations_through_backward_jumps() {
    let Some(fixture) = fixture() else { return };
    let mut session = session(&fixture);
    session.apply(Command::ToggleSourceBreakpoint(10)).unwrap();
    run(&mut session);
    assert_eq!(line(&session), 10);
    let mut lines = Vec::new();
    for _ in 0..5 {
        source_step(&mut session, SourceStep::Over);
        lines.push(line(&session));
    }
    assert_eq!(lines, [9, 10, 9, 10, 9]);
    source_step(&mut session, SourceStep::Over);
    assert_eq!(line(&session), 11);
}

#[test]
fn source_breakpoint_inside_a_loop_hits_every_iteration_then_exits() {
    let Some(fixture) = fixture() else { return };
    let mut session = session(&fixture);
    session.apply(Command::ToggleSourceBreakpoint(10)).unwrap();
    let breakpoint = &session.view().source_breakpoints[&10];
    assert_eq!(breakpoint.address, Some(fixture.main_at(28)));
    assert_eq!(breakpoint.resolved_line, Some(10));
    // Instruction breakpoints stay the user's own set.
    assert!(session.view().breakpoints.is_empty());
    let mut counters = Vec::new();
    for _ in 0..3 {
        run(&mut session);
        assert_eq!(session.view().state, RunState::Paused);
        assert!(session.view().message.contains("breakpoint on line 10"));
        counters.push(global(&session, "counter"));
    }
    assert_eq!(
        counters,
        [
            ("3".to_string(), false),
            ("4".to_string(), true),
            ("5".to_string(), true)
        ]
    );
    run(&mut session);
    assert_eq!(session.view().state, RunState::Halted);
    assert_eq!(debug(&session).exit_status, Some(42));
    assert_eq!(session.view().message, "Program exited with status 42.");
    session.apply(Command::ToggleSourceBreakpoint(10)).unwrap();
    assert!(session.view().source_breakpoints.is_empty());
}

#[test]
fn breakpoints_inside_callees_stop_a_step_over() {
    let Some(fixture) = fixture() else { return };
    let mut session = session(&fixture);
    source_step(&mut session, SourceStep::Into);
    source_step(&mut session, SourceStep::Over);
    session
        .apply(Command::SetSourceBreakpoints([2].into()))
        .unwrap();
    session
        .apply(Command::SourceStep(SourceStep::Over))
        .unwrap();
    session.run_to_stop();
    assert_eq!(pc(&session), fixture.g);
    assert!(session.view().message.contains("breakpoint on line 2"));
}

#[test]
fn reverse_steps_after_a_source_step_restore_the_previous_state() {
    let Some(fixture) = fixture() else { return };
    let mut session = session(&fixture);
    source_step(&mut session, SourceStep::Into);
    source_step(&mut session, SourceStep::Into);
    let before = session.view().snapshot.clone().unwrap();
    source_step(&mut session, SourceStep::Over);
    let after = session.view().snapshot.as_ref().unwrap().steps;
    assert!(after > before.steps + 1);
    for _ in before.steps..after {
        session.apply(Command::Reverse).unwrap();
    }
    let restored = session.view().snapshot.as_ref().unwrap();
    assert_eq!(restored.pc, before.pc);
    assert_eq!(restored.registers, before.registers);
    assert_eq!(restored.memory, before.memory);
    assert_eq!(line(&session), 8);
}

#[test]
fn source_steps_explain_programs_without_debug_information() {
    let Some(_) = fixture() else { return };
    let mut session = Session::new(Architecture::Arm64, String::new());
    session
        .assemble(
            Architecture::Arm64,
            Architecture::Arm64.example_source().into(),
        )
        .unwrap();
    let error = session
        .apply(Command::SourceStep(SourceStep::Over))
        .unwrap_err();
    assert!(error.0.contains("compiled C program"));
    assert_eq!(session.view().state, RunState::Ready);
    assert!(session.view().debug.is_none());
}

#[test]
fn pause_interrupts_a_source_step() {
    let Some(fixture) = fixture() else { return };
    let mut session = session(&fixture);
    session
        .apply(Command::SourceStep(SourceStep::Into))
        .unwrap();
    assert_eq!(session.view().state, RunState::Running);
    session.apply(Command::Pause).unwrap();
    assert_eq!(session.view().state, RunState::Paused);
    assert_eq!(session.view().stepping, None);
    assert!(session.view().debug.is_some());
}

#[test]
fn restore_keeps_source_breakpoints_and_continues_to_the_same_result() {
    let Some(fixture) = fixture() else { return };
    let mut session = session(&fixture);
    session.apply(Command::ToggleSourceBreakpoint(10)).unwrap();
    run(&mut session);
    run(&mut session);
    let mut project = vole_project::Project::new_c(
        Architecture::Arm64,
        fixture.program.source.clone(),
        Default::default(),
    );
    project.image = session.view().program.clone();
    project.snapshot = session.view().snapshot.clone();
    project.source_breakpoints = session.view().source_breakpoints.keys().copied().collect();
    let reopened = vole_project::Project::from_json(&project.to_json().unwrap()).unwrap();
    assert_eq!(reopened, {
        let mut expected = project.clone();
        expected.snapshot.as_mut().unwrap().trace.clear();
        expected
    });

    let mut restored = Session::new(Architecture::Arm64, String::new());
    restored
        .apply(Command::Restore {
            program: reopened.image.clone().unwrap(),
            snapshot: reopened.snapshot.clone(),
        })
        .unwrap();
    restored
        .apply(Command::SetSourceBreakpoints(reopened.source_breakpoints))
        .unwrap();
    assert_eq!(restored.view().language, vole_core::SourceLanguage::C);
    assert_eq!(global(&restored, "counter").0, "4");
    assert_eq!(line(&restored), 10);
    // The saved PC sits on the breakpoint; Run must first leave it.
    run(&mut restored);
    assert_eq!(global(&restored, "counter").0, "5");
    run(&mut restored);
    assert_eq!(restored.view().state, RunState::Halted);
    assert_eq!(debug(&restored).exit_status, Some(42));
}

#[test]
fn restore_rejects_c_images_with_inconsistent_debug_information() {
    let Some(fixture) = fixture() else { return };
    let mut session = session(&fixture);
    let mut broken = fixture.program.clone();
    broken.debug.as_mut().unwrap().unwind.reverse();
    assert!(
        session
            .apply(Command::Restore {
                program: broken,
                snapshot: None
            })
            .is_err()
    );
    let mut missing = fixture.program.clone();
    missing.debug = None;
    assert!(
        session
            .apply(Command::Restore {
                program: missing,
                snapshot: None
            })
            .is_err()
    );
    assert_eq!(session.view().program.as_ref(), Some(&fixture.program));
}
