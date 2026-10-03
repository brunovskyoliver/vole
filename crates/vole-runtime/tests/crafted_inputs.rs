//! Crafted debug metadata and stop timing must not crash, hang or mislead.
#[path = "../../vole-debug/tests/support/synthetic.rs"]
mod synthetic;

use std::{sync::mpsc, thread, time::Duration};
use vole_core::{Architecture, CompilerSettings, SourceLanguage, debug::*};
use vole_runtime::{Command, RunState, Session};

fn fixture_with(
    mutate: impl FnOnce(&mut DebugInfo, &synthetic::Fixture),
) -> Option<vole_core::Program> {
    let mut fixture = synthetic::fixture()?;
    let mut debug = fixture.program.debug.take().unwrap();
    mutate(&mut debug, &fixture);
    fixture.program.debug = Some(debug);
    Some(fixture.program)
}

fn global(name: &str, ty: usize, address: u64) -> Variable {
    Variable {
        name: name.into(),
        type_id: Some(ty),
        location: Location::Address(address),
        decl_file: 0,
        decl_line: 1,
        parameter: false,
        scope: vec![],
    }
}

#[test]
fn types_containing_themselves_are_rejected() {
    let Some(program) = fixture_with(|debug, fixture| {
        let id = debug.types.len();
        debug.types.push(Type {
            name: "loop".into(),
            size: 0,
            kind: TypeKind::Array {
                element: Some(id),
                count: Some(2),
            },
        });
        debug
            .globals
            .push(global("evil", id, fixture.program.symbols["counter"]));
    }) else {
        return;
    };
    let mut session = Session::new(Architecture::Arm64, String::new());
    let error = session.apply(Command::LoadProgram(program)).unwrap_err();
    assert!(error.0.contains("contains itself"), "{error}");
}

#[test]
fn huge_and_overflowing_types_render_within_bounds() {
    let Some(program) = fixture_with(|debug, fixture| {
        let nested = debug.types.len();
        // Without a pointer this nests to the depth limit: 64^6 values unbounded.
        debug.types.push(Type {
            name: "wide".into(),
            size: 4,
            kind: TypeKind::Array {
                element: Some(nested + 1),
                count: Some(64),
            },
        });
        debug.types.push(Type {
            name: "wider".into(),
            size: 4,
            kind: TypeKind::Array {
                element: Some(nested + 2),
                count: Some(64),
            },
        });
        debug.types.push(Type {
            name: "big".into(),
            size: 1 << 63,
            kind: TypeKind::Base(BaseEncoding::Signed),
        });
        let address = fixture.program.symbols["counter"];
        debug.globals.push(global("wide", nested, address));
        let main = debug
            .functions
            .iter_mut()
            .find(|f| f.name == "main")
            .unwrap();
        main.frame_base = Some(Location::RegisterOffset {
            register: "x29".into(),
            offset: i64::MAX,
        });
    }) else {
        return;
    };
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut session = Session::new(Architecture::Arm64, String::new());
        session.apply(Command::LoadProgram(program)).unwrap();
        for _ in 0..40 {
            if session.view().state == RunState::Halted {
                break;
            }
            session.apply(Command::Step).unwrap();
        }
        sender.send(session.view().debug.is_some()).unwrap();
    });
    assert_eq!(receiver.recv_timeout(Duration::from_secs(30)), Ok(true));
}

const C: &str = r#"#include <vole.h>
int total = 5;
int main(void) {
    int a = 1;
    a = a + 2;
    total = a;
    return a;
}
"#;

/// A breakpoint found before any instruction runs in a batch still publishes
/// the new location, call stack and variables.
#[test]
fn breakpoint_at_a_batch_boundary_refreshes_the_debug_view() {
    if vole_c::clang_status().is_err() {
        return;
    }
    let mut session = Session::new(Architecture::X64, String::new());
    session
        .build(
            Architecture::X64,
            SourceLanguage::C,
            C.into(),
            CompilerSettings::default(),
        )
        .unwrap();
    session
        .apply(Command::SetSourceBreakpoints([6].into()))
        .unwrap();
    session.apply(Command::Run).unwrap();
    while session.view().state == RunState::Running {
        session.run_batch(1);
    }
    let line = session
        .view()
        .debug
        .as_ref()
        .and_then(|debug| debug.location.as_ref())
        .map(|location| location.line);
    assert_eq!(line, Some(6));
}
