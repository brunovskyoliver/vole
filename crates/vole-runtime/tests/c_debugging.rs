//! Source debugging acceptance over real compiled C, per target.
//!
//! These tests skip (with a message) when Clang is unavailable or when the
//! compiler does not yet produce line tables, so they can land before the
//! compiler. Any other build failure is a test failure.
use vole_core::{
    Architecture, CompilerSettings, Optimization, SourceLanguage,
    debug::{DebugView, ValueText, VariableView},
};
use vole_runtime::{Command, RunState, Session, SourceStep};

const SOURCE: &str = r#"#include <vole.h>

int total = 0;
int numbers[4] = {3, 1, 4, 1};
const char *greeting = "hi";

int g(int x) {
    int doubled = x * 2;
    return doubled + 1;
}

int f(int n) {
    int r = g(n + 1);
    return r;
}

int main(void) {
    int sum = 0;
    int *p = &numbers[2];
    for (int i = 0; i < 4; i++) {
        sum += numbers[i];
    }
    total = f(sum);
    vole_print_int(total);
    return *p + total;
}
"#;

const LOOP_BODY: usize = 21;
const CALL_F: usize = 23;
const PRINT: usize = 24;
const RETURN: usize = 25;

fn compiled(architecture: Architecture) -> Option<Session> {
    if let Err(reason) = vole_c::clang_status() {
        eprintln!(
            "skipping {}: Clang unavailable: {reason}",
            architecture.name()
        );
        return None;
    }
    let mut session = Session::new(architecture, String::new());
    if let Err(error) = session.build(
        architecture,
        SourceLanguage::C,
        SOURCE.into(),
        CompilerSettings::default(),
    ) {
        if error.0.contains("not available yet") {
            eprintln!("skipping {}: compiler stub: {error}", architecture.name());
            return None;
        }
        panic!(
            "{} build failed: {error}\n{:#?}",
            architecture.name(),
            session.view().diagnostics
        );
    }
    let debug = session.view().program.as_ref()?.debug.as_ref()?;
    if debug.lines.is_empty() || debug.functions.is_empty() {
        eprintln!(
            "skipping {}: compiler does not extract DWARF yet",
            architecture.name()
        );
        return None;
    }
    Some(session)
}

fn debug(session: &Session) -> &DebugView {
    session
        .view()
        .debug
        .as_ref()
        .expect("paused C image has a debug view")
}

fn line(session: &Session) -> usize {
    debug(session).location.as_ref().expect("location").line as usize
}

fn stack(session: &Session) -> Vec<(String, usize)> {
    debug(session)
        .frames
        .iter()
        .map(|frame| {
            (
                frame.function.clone(),
                frame.location.as_ref().map_or(0, |l| l.line as usize),
            )
        })
        .collect()
}

fn named<'a>(views: &'a [VariableView], name: &str) -> &'a VariableView {
    views
        .iter()
        .find(|view| view.name == name)
        .unwrap_or_else(|| panic!("no variable {name} in {views:#?}"))
}

fn text(view: &VariableView) -> String {
    match &view.value {
        ValueText::Value(text) => text.clone(),
        ValueText::Unavailable(reason) => panic!("{} unavailable: {reason}", view.name),
    }
}

fn local(session: &Session, name: &str) -> String {
    text(named(&debug(session).frames[0].variables, name))
}

fn global(session: &Session, name: &str) -> String {
    text(named(&debug(session).globals, name))
}

fn run(session: &mut Session) {
    session.apply(Command::Run).unwrap();
    session.run_to_stop();
}

fn step(session: &mut Session, kind: SourceStep) {
    session.apply(Command::SourceStep(kind)).unwrap();
    session.run_to_stop();
    assert_eq!(
        session.view().state,
        RunState::Paused,
        "{}",
        session.view().message
    );
}

fn run_to_line(session: &mut Session, line: usize) {
    session
        .apply(Command::SetSourceBreakpoints([line].into()))
        .unwrap();
    run(session);
    assert_eq!(self::line(session), line, "{}", session.view().message);
}

fn output(session: &Session) -> String {
    String::from_utf8_lossy(&session.view().snapshot.as_ref().unwrap().output).into()
}

fn finishes_with_exit_status(session: &mut Session) {
    session
        .apply(Command::SetSourceBreakpoints(Default::default()))
        .unwrap();
    run(session);
    assert_eq!(
        session.view().state,
        RunState::Halted,
        "{}",
        session.view().message
    );
    assert_eq!(debug(session).exit_status, Some(25));
    assert_eq!(output(session), "21");
}

fn loop_breakpoint_hits_every_iteration(architecture: Architecture) {
    let Some(mut session) = compiled(architecture) else {
        return;
    };
    session
        .apply(Command::ToggleSourceBreakpoint(LOOP_BODY))
        .unwrap();
    let resolved = &session.view().source_breakpoints[&LOOP_BODY];
    assert_eq!(resolved.resolved_line, Some(LOOP_BODY));
    let mut observed = Vec::new();
    for _ in 0..4 {
        run(&mut session);
        assert_eq!(line(&session), LOOP_BODY, "{}", session.view().message);
        observed.push((local(&session, "i"), local(&session, "sum")));
    }
    let expected: Vec<(String, String)> = [("0", "0"), ("1", "3"), ("2", "4"), ("3", "8")]
        .iter()
        .map(|(i, sum)| (i.to_string(), sum.to_string()))
        .collect();
    assert_eq!(observed, expected);
    finishes_with_exit_status(&mut session);
}

fn steps_follow_calls(architecture: Architecture) {
    let Some(mut session) = compiled(architecture) else {
        return;
    };
    run_to_line(&mut session, CALL_F);
    step(&mut session, SourceStep::Over);
    assert_eq!(stack(&session), [("main".to_string(), PRINT)]);
    assert_eq!(global(&session, "total"), "21");

    session.apply(Command::Reset).unwrap();
    run_to_line(&mut session, CALL_F);
    step(&mut session, SourceStep::Into);
    assert_eq!(stack(&session), [("f".into(), 13), ("main".into(), CALL_F)]);
    assert_eq!(local(&session, "n"), "9");
    step(&mut session, SourceStep::Into);
    assert_eq!(
        stack(&session),
        [("g".into(), 8), ("f".into(), 13), ("main".into(), CALL_F)]
    );
    assert_eq!(local(&session, "x"), "10");
    step(&mut session, SourceStep::Over);
    assert_eq!(line(&session), 9);
    assert_eq!(local(&session, "doubled"), "20");
    step(&mut session, SourceStep::Out);
    assert_eq!(stack(&session)[0], ("f".into(), 13));
    assert_eq!(debug(&session).frames.len(), 2);
    step(&mut session, SourceStep::Out);
    assert_eq!(stack(&session)[0], ("main".into(), CALL_F));
    step(&mut session, SourceStep::Over);
    assert_eq!(line(&session), PRINT);
    assert_eq!(global(&session, "total"), "21");
    // Stepping over a runtime call stays in main and keeps its output.
    step(&mut session, SourceStep::Over);
    assert_eq!(line(&session), RETURN);
    assert_eq!(output(&session), "21");
    finishes_with_exit_status(&mut session);
}

fn values_render_locals_pointers_arrays_and_globals(architecture: Architecture) {
    let Some(mut session) = compiled(architecture) else {
        return;
    };
    run_to_line(&mut session, RETURN);
    assert_eq!(local(&session, "sum"), "9");
    let frame = &debug(&session).frames[0];
    let p = named(&frame.variables, "p");
    assert_eq!(p.type_name, "int *");
    assert!(text(p).starts_with("0x"), "{}", text(p));
    assert_eq!(text(&p.children[0]), "4");
    let numbers = named(&debug(&session).globals, "numbers");
    assert_eq!(numbers.type_name, "int [4]");
    assert_eq!(text(numbers), "{3, 1, 4, 1}");
    assert_eq!(numbers.children.len(), 4);
    let greeting = named(&debug(&session).globals, "greeting");
    assert_eq!(greeting.type_name, "const char *");
    assert!(text(greeting).ends_with("→ \"hi\""), "{}", text(greeting));
    assert_eq!(global(&session, "total"), "21");
    // `i` belongs to the finished loop's scope.
    assert!(frame.variables.iter().all(|v| v.name != "i"));
}

fn reverse_after_source_step_restores_state(architecture: Architecture) {
    let Some(mut session) = compiled(architecture) else {
        return;
    };
    run_to_line(&mut session, CALL_F);
    let before = session.view().snapshot.clone().unwrap();
    step(&mut session, SourceStep::Over);
    let after = session.view().snapshot.as_ref().unwrap().steps;
    for _ in before.steps..after {
        session.apply(Command::Reverse).unwrap();
    }
    let restored = session.view().snapshot.as_ref().unwrap();
    assert_eq!(restored.pc, before.pc);
    assert_eq!(restored.registers, before.registers);
    assert_eq!(restored.memory, before.memory);
    assert_eq!(line(&session), CALL_F);
    assert_eq!(global(&session, "total"), "0");
}

fn project_round_trip_continues_to_the_same_result(architecture: Architecture) {
    let Some(mut session) = compiled(architecture) else {
        return;
    };
    session
        .apply(Command::ToggleSourceBreakpoint(LOOP_BODY))
        .unwrap();
    run(&mut session);
    run(&mut session);
    let settings = session.view().settings.clone();
    let mut project = vole_project::Project::new_c(architecture, SOURCE, settings.clone());
    project.image = session.view().program.clone();
    project.snapshot = session.view().snapshot.clone();
    project.source_breakpoints = session.view().source_breakpoints.keys().copied().collect();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("lesson.voleproject");
    project.save(&path).unwrap();
    let reopened = vole_project::Project::open(&path).unwrap();
    assert_eq!(reopened.language, SourceLanguage::C);
    assert_eq!(reopened.compiler, settings);
    assert_eq!(reopened.source, SOURCE);
    assert_eq!(reopened.source_breakpoints, [LOOP_BODY].into());

    let mut restored = Session::new(architecture, String::new());
    restored
        .apply(Command::Restore {
            program: reopened.image.unwrap(),
            snapshot: reopened.snapshot,
        })
        .unwrap();
    restored
        .apply(Command::SetSourceBreakpoints(reopened.source_breakpoints))
        .unwrap();
    assert_eq!(line(&restored), LOOP_BODY);
    assert_eq!(local(&restored, "sum"), "3");
    run(&mut restored);
    assert_eq!(local(&restored, "sum"), "4");
    finishes_with_exit_status(&mut restored);
}

macro_rules! target_tests {
    ($module:ident, $architecture:expr) => {
        mod $module {
            use super::*;

            #[test]
            fn loop_breakpoint_hits_every_iteration() {
                super::loop_breakpoint_hits_every_iteration($architecture);
            }
            #[test]
            fn steps_follow_calls() {
                super::steps_follow_calls($architecture);
            }
            #[test]
            fn values_render_locals_pointers_arrays_and_globals() {
                super::values_render_locals_pointers_arrays_and_globals($architecture);
            }
            #[test]
            fn reverse_after_source_step_restores_state() {
                super::reverse_after_source_step_restores_state($architecture);
            }
            #[test]
            fn project_round_trip_continues_to_the_same_result() {
                super::project_round_trip_continues_to_the_same_result($architecture);
            }
        }
    };
}

target_tests!(arm64, Architecture::Arm64);
target_tests!(x64, Architecture::X64);
target_tests!(arm32, Architecture::Arm32);
target_tests!(x86, Architecture::X86);

/// `changed` compares with the previous stop, even after the same stop is
/// refreshed again (e.g. by toggling a breakpoint while paused).
#[test]
fn changed_values_survive_repeated_refreshes() {
    for architecture in [Architecture::Arm64, Architecture::X64] {
        let Some(mut session) = compiled(architecture) else {
            return;
        };
        session
            .apply(Command::ToggleSourceBreakpoint(LOOP_BODY))
            .unwrap();
        run(&mut session);
        run(&mut session);
        let sum = named(&debug(&session).frames[0].variables, "sum");
        assert!(sum.changed, "{architecture:?}: sum changed between stops");
        session
            .apply(Command::ToggleSourceBreakpoint(PRINT))
            .unwrap();
        let sum = named(&debug(&session).frames[0].variables, "sum");
        assert!(sum.changed, "{architecture:?}: refresh kept the comparison");
        let storage = sum.storage.as_ref().expect("frame storage");
        assert!(
            matches!(storage, vole_core::debug::Storage::RegisterOffset { .. }),
            "{architecture:?}: {storage:?}"
        );
        let numbers = named(&debug(&session).globals, "numbers");
        assert_eq!(numbers.storage, Some(vole_core::debug::Storage::Static));
    }
}

const RECURSIVE: &str = r#"int fact(int n) {
    if (n <= 1)
        return 1;
    return n * fact(n - 1);
}
int main(void) {
    int r = fact(5);
    return r;
}
"#;

/// Optimized code has call-frame information that is only precise at call
/// sites. Stepping follows executed calls and returns instead, so recursion
/// and leaf functions behave the same at -O1 as at -O0.
#[test]
fn optimized_recursion_steps_one_frame_at_a_time() {
    for architecture in [
        Architecture::Arm64,
        Architecture::X64,
        Architecture::Arm32,
        Architecture::X86,
    ] {
        if vole_c::clang_status().is_err() {
            return;
        }
        for optimization in [Optimization::O0, Optimization::O1] {
            let settings = CompilerSettings {
                optimization,
                warnings: true,
            };
            let mut session = Session::new(architecture, String::new());
            session
                .build(architecture, SourceLanguage::C, RECURSIVE.into(), settings)
                .unwrap();
            let label = format!("{architecture:?} {optimization:?}");
            step(&mut session, SourceStep::Into);
            assert_eq!(line(&session), 7, "{label}: first line of main");
            step(&mut session, SourceStep::Over);
            assert_eq!(stack(&session)[0].0, "main", "{label}: Over stays in main");
            assert_eq!(line(&session), 8, "{label}");

            session.apply(Command::Reset).unwrap();
            session.apply(Command::ToggleSourceBreakpoint(4)).unwrap();
            // Line 4 stops once per level; continue until four calls deep.
            for _ in 0..8 {
                run(&mut session);
                if stack(&session).len() >= 5 {
                    break;
                }
            }
            let depth = stack(&session).len();
            assert!(depth >= 5, "{label}: deep call {:?}", stack(&session));
            step(&mut session, SourceStep::Out);
            let after = stack(&session);
            assert_eq!(
                after.len(),
                depth - 1,
                "{label}: Out leaves one frame {after:?}"
            );
            assert_eq!(after[0].0, "fact", "{label}");
            step(&mut session, SourceStep::Over);
            assert!(
                stack(&session).len() < depth,
                "{label}: Over never descends"
            );
        }
    }
}
