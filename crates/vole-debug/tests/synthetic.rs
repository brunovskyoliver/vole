//! Unwinding, breakpoints and value rendering over a hand-built ARM64 image.
#[path = "support/synthetic.rs"]
mod synthetic;

use synthetic::{Fixture, fixture};
use vole_core::{
    Architecture, Machine, Snapshot,
    debug::{DebugView, ValueText, VariableKind, VariableView},
};
use vole_debug::{
    MachineTarget, backtrace, breakpoint_address, current_cfa, debug_view, statement_start,
};
use vole_isa_scalar::ScalarMachine;

fn machine(fixture: &Fixture) -> ScalarMachine {
    let mut machine = ScalarMachine::new(Architecture::Arm64);
    machine.load(&fixture.program).unwrap();
    machine
}

fn run_to(machine: &mut ScalarMachine, address: u64) -> Snapshot {
    for _ in 0..10_000 {
        if machine.pc() == address {
            return machine.snapshot();
        }
        machine.step().unwrap();
    }
    panic!("never reached {address:X}");
}

fn value(view: &VariableView) -> &str {
    match &view.value {
        ValueText::Value(text) => text,
        ValueText::Unavailable(reason) => panic!("{} unavailable: {reason}", view.name),
    }
}

fn reason(view: &VariableView) -> &str {
    match &view.value {
        ValueText::Unavailable(reason) => reason,
        ValueText::Value(text) => panic!("{} unexpectedly has value {text}", view.name),
    }
}

fn find<'a>(views: &'a [VariableView], name: &str) -> &'a VariableView {
    views
        .iter()
        .find(|view| view.name == name)
        .unwrap_or_else(|| panic!("no variable {name}"))
}

fn view_at(fixture: &Fixture, machine: &mut ScalarMachine, address: u64) -> DebugView {
    let snapshot = run_to(machine, address);
    debug_view(&fixture.program, &snapshot, None).unwrap()
}

#[test]
fn backtrace_walks_g_f_main_with_caller_lines() {
    let Some(fixture) = fixture() else { return };
    let debug = fixture.program.debug.as_ref().unwrap();
    let mut machine = machine(&fixture);
    let snapshot = run_to(&mut machine, fixture.g);
    let frames = backtrace(debug, Architecture::Arm64, &snapshot);
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0].pc, fixture.g);
    assert_eq!(frames[1].pc, fixture.f + 12);
    assert_eq!(frames[2].pc, fixture.main_at(20));
    assert!(frames[0].cfa < frames[1].cfa && frames[1].cfa < frames[2].cfa);
    assert_eq!(frames[2].cfa, Some(0x20000));

    let view = debug_view(&fixture.program, &snapshot, None).unwrap();
    let names: Vec<_> = view.frames.iter().map(|f| f.function.as_str()).collect();
    assert_eq!(names, ["g", "f", "main"]);
    let lines: Vec<_> = view
        .frames
        .iter()
        .map(|f| f.location.as_ref().unwrap().line)
        .collect();
    assert_eq!(lines, [2, 4, 8]);
    assert!(view.frames.iter().all(|f| f.user));
    assert_eq!(view.location.as_ref().unwrap().line, 2);
    assert!(view.note.is_none());
    // main's local lives in memory and is readable from an outer frame.
    assert_eq!(value(find(&view.frames[2].variables, "a")), "5");
    // f's parameter lived in x0, which callees do not preserve.
    let n = find(&view.frames[1].variables, "n");
    assert_eq!(n.kind, VariableKind::Parameter);
    assert!(reason(n).contains("x0"), "{}", reason(n));
}

#[test]
fn the_live_machine_target_matches_the_snapshot() {
    let Some(fixture) = fixture() else { return };
    let debug = fixture.program.debug.as_ref().unwrap();
    let mut machine = machine(&fixture);
    let snapshot = run_to(&mut machine, fixture.f + 8);
    let live = MachineTarget(&machine);
    assert_eq!(current_cfa(debug, &live), current_cfa(debug, &snapshot));
    assert_eq!(
        debug_view(&fixture.program, &live, None),
        debug_view(&fixture.program, &snapshot, None)
    );
}

#[test]
fn unwinding_works_at_function_entry_before_the_frame_record_exists() {
    let Some(fixture) = fixture() else { return };
    let debug = fixture.program.debug.as_ref().unwrap();
    let mut machine = machine(&fixture);
    let snapshot = run_to(&mut machine, fixture.f);
    let frames = backtrace(debug, Architecture::Arm64, &snapshot);
    assert_eq!(
        frames.iter().map(|f| f.pc).collect::<Vec<_>>(),
        [fixture.f, fixture.main_at(20)]
    );
    let view = debug_view(&fixture.program, &snapshot, None).unwrap();
    // At entry the parameter is still in x0 (location list), so it is valid.
    assert_eq!(value(find(&view.frames[0].variables, "n")), "5");
}

#[test]
fn locals_explain_prologue_and_declarations_not_yet_reached() {
    let Some(fixture) = fixture() else { return };
    let mut machine = machine(&fixture);
    let view = view_at(&fixture, &mut machine, fixture.main);
    let a = find(&view.frames[0].variables, "a");
    assert!(reason(a).contains("prologue"), "{}", reason(a));
    let view = view_at(&fixture, &mut machine, fixture.main_at(8));
    let a = find(&view.frames[0].variables, "a");
    assert_eq!(view.location.as_ref().unwrap().line, 7);
    assert_eq!(a.kind, VariableKind::Local);
    assert_eq!(a.type_name, "int");
    assert_eq!(a.address, Some(0x20000 - 32 + 28));
    // `i` is scoped to the loop and is not listed before it.
    assert!(view.frames[0].variables.iter().all(|v| v.name != "i"));
    let view = view_at(&fixture, &mut machine, fixture.main_at(28));
    assert_eq!(value(find(&view.frames[0].variables, "i")), "3");
}

#[test]
fn location_lists_report_values_optimized_out_after_their_range() {
    let Some(fixture) = fixture() else { return };
    let mut machine = machine(&fixture);
    let view = view_at(&fixture, &mut machine, fixture.f + 12);
    let n = find(&view.frames[0].variables, "n");
    assert!(reason(n).contains("Optimized out"), "{}", reason(n));
}

#[test]
fn globals_render_arrays_structs_and_string_pointers() {
    let Some(fixture) = fixture() else { return };
    let mut machine = machine(&fixture);
    let view = view_at(&fixture, &mut machine, fixture.main_at(8));
    let globals = &view.globals;
    assert!(globals.iter().all(|g| g.name != "runtime_state"));
    let counter = find(globals, "counter");
    assert_eq!((value(counter), counter.kind), ("3", VariableKind::Global));
    let numbers = find(globals, "numbers");
    assert_eq!(numbers.type_name, "int [3]");
    assert_eq!(value(numbers), "{3, 1, 4}");
    assert_eq!(numbers.children.len(), 3);
    assert_eq!(numbers.children[2].name, "[2]");
    assert_eq!(value(&numbers.children[2]), "4");
    let origin = find(globals, "origin");
    assert_eq!(origin.type_name, "struct point");
    assert_eq!(value(origin), "{x = 10, y = -2}");
    assert_eq!(origin.children[1].kind, VariableKind::Member);
    let message = find(globals, "message");
    assert_eq!(message.type_name, "const char *");
    let text = fixture.program.symbols["text"];
    assert_eq!(value(message), format!("0x{text:x} → \"hi\\n\""));
    let cursor = find(globals, "cursor");
    assert_eq!(cursor.type_name, "int *");
    assert_eq!(cursor.children[0].name, "*cursor");
    assert_eq!(value(&cursor.children[0]), "682344");
}

#[test]
fn changed_flags_compare_with_the_previous_paused_view() {
    let Some(fixture) = fixture() else { return };
    let mut machine = machine(&fixture);
    let first = view_at(&fixture, &mut machine, fixture.main_at(40));
    machine.step().unwrap();
    let snapshot = run_to(&mut machine, fixture.main_at(40));
    let second = debug_view(&fixture.program, &snapshot, Some(&first)).unwrap();
    assert!(find(&second.globals, "counter").changed);
    assert!(!find(&second.globals, "numbers").changed);
    assert!(find(&second.frames[0].variables, "i").changed);
    assert!(!find(&second.frames[0].variables, "a").changed);
}

#[test]
fn breakpoints_resolve_to_statements_and_skip_prologues() {
    let Some(fixture) = fixture() else { return };
    let debug = fixture.program.debug.as_ref().unwrap();
    assert_eq!(
        breakpoint_address(debug, 10),
        Some((fixture.main_at(28), 10))
    );
    // A function's opening line moves to its prologue_end statement.
    assert_eq!(breakpoint_address(debug, 6), Some((fixture.main_at(8), 7)));
    assert_eq!(breakpoint_address(debug, 3), Some((fixture.f + 8, 4)));
    // The loop header's lowest address is the initialisation.
    assert_eq!(breakpoint_address(debug, 9), Some((fixture.main_at(20), 9)));
    // Lines without code snap forward.
    assert_eq!(breakpoint_address(debug, 1), Some((fixture.g, 2)));
    assert_eq!(breakpoint_address(debug, 13), None);
    assert_eq!(breakpoint_address(debug, 0), None);
    assert!(statement_start(debug, fixture.main_at(28)).is_some());
    assert!(statement_start(debug, fixture.main_at(32)).is_none());
    // Runtime rows are not user statements.
    assert!(statement_start(debug, fixture.vole_exit).is_none());
}

#[test]
fn halting_in_vole_exit_reports_the_exit_status() {
    let Some(fixture) = fixture() else { return };
    let mut machine = machine(&fixture);
    let start = debug_view(&fixture.program, &machine.snapshot(), None).unwrap();
    assert_eq!(start.frames[0].function, "_start");
    assert!(start.note.as_ref().unwrap().contains("startup"));
    let inside = view_at(&fixture, &mut machine, fixture.vole_exit + 4);
    assert!(inside.exit_status.is_none());
    assert!(inside.note.as_ref().unwrap().contains("runtime"));
    assert_eq!(inside.frames[0].function, "vole_exit");
    assert_eq!(value(find(&inside.frames[0].variables, "status")), "42");
    machine.step().unwrap();
    let snapshot = machine.snapshot();
    assert!(snapshot.halted);
    // The halting PC is the next function's first byte; PC - 1 is in vole_exit.
    assert_eq!(snapshot.pc, fixture.main);
    let view = debug_view(&fixture.program, &snapshot, None).unwrap();
    assert_eq!(view.exit_status, Some(42));
    assert!(view.note.is_none());
}

#[test]
fn programs_without_debug_information_have_no_source_view() {
    let Ok(program) =
        vole_isa_scalar::assemble(Architecture::Arm64, Architecture::Arm64.example_source())
    else {
        return;
    };
    let mut machine = ScalarMachine::new(Architecture::Arm64);
    machine.load(&program).unwrap();
    assert!(debug_view(&program, &machine.snapshot(), None).is_none());
}

#[test]
fn validation_rejects_unsorted_tables() {
    let Some(fixture) = fixture() else { return };
    let mut debug = fixture.program.debug.clone().unwrap();
    assert!(vole_debug::validate(&debug).is_ok());
    debug.lines.reverse();
    assert!(vole_debug::validate(&debug).is_err());
    let mut debug = fixture.program.debug.clone().unwrap();
    debug.globals[0].type_id = Some(999);
    assert!(vole_debug::validate(&debug).is_err());
}
