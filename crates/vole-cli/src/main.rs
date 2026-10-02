use std::{collections::BTreeSet, env, fs, io::Read, path::Path, process::ExitCode};
use vole_core::{
    Architecture, CompilerSettings, Optimization, SimError, SourceLanguage,
    debug::{DebugView, ValueText, VariableView},
};
use vole_project::Project;
use vole_runtime::{Command, RunState, Session, SourceStep, export_bytes, import_bytes};

const HELP: &str = "Vole headless assembler, C compiler and simulator

vole-cli [--arch vole|arm32|arm64|x86|x64] [--source FILE | --bytes FILE | --project FILE]
         [--language asm|c] [--opt O0|O1] [--no-warnings]
         [--break LINE]... [--step-into N | --step-over N]
         [--steps N] [--json] [--assemble-only] [--export-bytes FILE]

Without a file, runs the selected target's addition example.
Real-ISA source assembly requires llvm-mc and ld.lld on PATH or VOLE_TOOLCHAIN_DIR.
Imported machine bytes and VOLE do not require LLVM. Execution uses documented scalar subsets.

C documents (--language c, the default for .c files and C projects) are compiled
with Clang for ARM64 unless --arch selects arm32, x86 or x64. C runs stop at each
--break source line and print the call stack and variables, then continue; the
program's output and exit status are printed at the end. --step-into/--step-over
perform N source-level steps from the start and print each stop instead.
--steps bounds the instructions executed (default 10000 for assembly, 1000000 for C).";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), SimError> {
    let mut architecture = None;
    let mut language = None;
    let mut settings = CompilerSettings::default();
    let mut source_path = None;
    let mut bytes_path = None;
    let mut project_path = None;
    let mut export_path = None;
    let mut budget = None;
    let mut json = false;
    let mut assemble_only = false;
    let mut source_breakpoints = BTreeSet::new();
    let mut steps: Option<(SourceStep, usize)> = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{HELP}");
                return Ok(());
            }
            "--arch" => {
                let value = next(&mut args, "--arch")?;
                architecture = Some(
                    Architecture::parse(&value)
                        .ok_or_else(|| SimError(format!("Unknown architecture: {value}")))?,
                );
            }
            "--language" => {
                language = Some(
                    match next(&mut args, "--language")?.to_lowercase().as_str() {
                        "c" => SourceLanguage::C,
                        "asm" | "assembly" => SourceLanguage::Assembly,
                        other => return Err(SimError(format!("Unknown language: {other}"))),
                    },
                )
            }
            "--opt" => {
                settings.optimization = match next(&mut args, "--opt")?.to_uppercase().as_str() {
                    "O0" | "-O0" => Optimization::O0,
                    "O1" | "-O1" => Optimization::O1,
                    other => {
                        return Err(SimError(format!(
                            "Unsupported optimization {other}; use O0 or O1."
                        )));
                    }
                }
            }
            "--no-warnings" => settings.warnings = false,
            "--break" => {
                let line: usize = next(&mut args, "--break")?
                    .parse()
                    .ok()
                    .filter(|line| *line > 0)
                    .ok_or_else(|| SimError("--break requires a positive line number.".into()))?;
                source_breakpoints.insert(line);
            }
            "--step-into" | "--step-over" => {
                let count = next(&mut args, &arg)?
                    .parse()
                    .ok()
                    .filter(|count| (1..=10_000).contains(count))
                    .ok_or_else(|| SimError(format!("{arg} requires a count from 1 to 10000.")))?;
                let kind = if arg == "--step-into" {
                    SourceStep::Into
                } else {
                    SourceStep::Over
                };
                steps = Some((kind, count));
            }
            "--source" => source_path = Some(next(&mut args, "--source")?),
            "--bytes" => bytes_path = Some(next(&mut args, "--bytes")?),
            "--project" => project_path = Some(next(&mut args, "--project")?),
            "--export-bytes" => export_path = Some(next(&mut args, "--export-bytes")?),
            "--steps" => {
                budget = Some(
                    next(&mut args, "--steps")?
                        .parse()
                        .map_err(|_| SimError("--steps requires a positive integer.".into()))?,
                )
            }
            "--json" => json = true,
            "--assemble-only" => assemble_only = true,
            _ => return Err(SimError(format!("Unknown argument {arg}. Use --help."))),
        }
    }
    if [
        source_path.is_some(),
        bytes_path.is_some(),
        project_path.is_some(),
    ]
    .into_iter()
    .filter(|b| *b)
    .count()
        > 1
    {
        return Err(SimError(
            "Choose one input: source, bytes, or project.".into(),
        ));
    }
    let project = match project_path {
        Some(path) => Some(Project::open(Path::new(&path))?),
        None => None,
    };
    let language = match &project {
        Some(project) => {
            if language.is_some_and(|language| language != project.language) {
                return Err(SimError(
                    "--language does not match the project's language.".into(),
                ));
            }
            project.language
        }
        None if bytes_path.is_some() => SourceLanguage::Assembly,
        None => language.unwrap_or_else(|| {
            if source_path
                .as_deref()
                .is_some_and(|path| path.to_ascii_lowercase().ends_with(".c"))
            {
                SourceLanguage::C
            } else {
                SourceLanguage::Assembly
            }
        }),
    };
    let architecture = match (&project, architecture) {
        (Some(project), _) => project.architecture,
        (None, Some(architecture)) => architecture,
        (None, None) if language == SourceLanguage::C => Architecture::Arm64,
        (None, None) => Architecture::Vole,
    };
    if let Some(project) = &project {
        settings = project.compiler.clone();
    }
    let budget = budget.unwrap_or(match language {
        SourceLanguage::Assembly => 10_000,
        SourceLanguage::C => vole_runtime::RUN_INSTRUCTION_BUDGET,
    });
    if budget == 0 || budget > 1_000_000 {
        return Err(SimError("--steps must be between 1 and 1000000.".into()));
    }
    if language == SourceLanguage::Assembly && (!source_breakpoints.is_empty() || steps.is_some()) {
        return Err(SimError(
            "--break, --step-into and --step-over need a C document.".into(),
        ));
    }
    let source = if let Some(project) = &project {
        project.source.clone()
    } else if let Some(path) = source_path {
        read_limited(Path::new(&path), 1024 * 1024).and_then(|b| {
            String::from_utf8(b).map_err(|_| SimError("Source must be UTF-8.".into()))
        })?
    } else if language == SourceLanguage::C {
        return Err(SimError(
            "C mode needs --source FILE.c or a C project.".into(),
        ));
    } else {
        architecture.example_source().into()
    };
    let mut session = Session::new(architecture, source.clone());
    if let Some(project) = &project {
        if let Some(program) = &project.image {
            session.apply(Command::Restore {
                program: program.clone(),
                snapshot: project.snapshot.clone(),
            })?;
        } else {
            build(&mut session, architecture, language, source, &settings)?;
        }
    } else if let Some(path) = bytes_path {
        session.load(import_bytes(
            architecture,
            &read_limited(Path::new(&path), 0x1F000)?,
        )?)?;
    } else {
        build(&mut session, architecture, language, source, &settings)?;
    }
    if let Some(project) = project {
        session.apply(Command::SetBreakpoints(project.breakpoints))?;
        source_breakpoints.extend(project.source_breakpoints);
    }
    if !source_breakpoints.is_empty() {
        session.apply(Command::SetSourceBreakpoints(source_breakpoints))?;
    }
    if let Some(path) = export_path {
        fs::write(
            path,
            export_bytes(session.view().program.as_ref().unwrap())?,
        )
        .map_err(|e| SimError(e.to_string()))?;
    }
    if assemble_only {
        let program = session.view().program.as_ref().unwrap();
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(program).map_err(|e| SimError(e.to_string()))?
            );
        } else {
            for instruction in &program.instructions {
                println!(
                    "{:08X}  {:<24} {}",
                    instruction.address,
                    instruction
                        .bytes
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                    instruction.assembly
                );
            }
        }
        return Ok(());
    }
    if language == SourceLanguage::C {
        return run_c(&mut session, budget, steps, json);
    }
    if session.view().state == RunState::Halted {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(session.view().snapshot.as_ref().unwrap())
                    .map_err(|e| SimError(e.to_string()))?
            );
        } else {
            println!("Saved program is halted. Open it in the desktop app to reset or edit PC.");
        }
        return Ok(());
    }
    let mut invocation_steps = 0;
    for _ in 0..budget {
        let pc = session.view().snapshot.as_ref().unwrap().pc;
        if session.view().breakpoints.contains(&pc) {
            break;
        }
        session.apply(Command::Step)?;
        invocation_steps += 1;
        if session.view().state == RunState::Halted {
            break;
        }
    }
    let snapshot = session.view().snapshot.as_ref().unwrap();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(snapshot).map_err(|e| SimError(e.to_string()))?
        );
    } else {
        println!(
            "{}: {} after {} instructions; PC={:X}",
            architecture.name(),
            session.view().state.label(),
            snapshot.steps,
            snapshot.pc
        );
        for register in &snapshot.registers {
            println!(
                "{} = {:0width$X}",
                register.name,
                register.value,
                width = (register.bits / 4) as usize
            );
        }
        if !snapshot.output.is_empty() {
            println!("Output: {}", String::from_utf8_lossy(&snapshot.output));
        }
    }
    if invocation_steps == budget && !snapshot.halted {
        return Err(SimError("Instruction budget reached before halt.".into()));
    }
    Ok(())
}

fn build(
    session: &mut Session,
    architecture: Architecture,
    language: SourceLanguage,
    source: String,
    settings: &CompilerSettings,
) -> Result<(), SimError> {
    let result = session.build(architecture, language, source, settings.clone());
    for diagnostic in &session.view().diagnostics {
        eprintln!(
            "{}:{}: {:?}: {}{}",
            diagnostic.line,
            diagnostic.column,
            diagnostic.severity,
            diagnostic.message,
            diagnostic
                .hint
                .as_ref()
                .map(|hint| format!(" ({hint})"))
                .unwrap_or_default()
        );
    }
    result
}

/// Run a compiled C program: stop at source breakpoints or perform source
/// steps, printing each stop, then report output and exit status.
fn run_c(
    session: &mut Session,
    budget: u64,
    steps: Option<(SourceStep, usize)>,
    json: bool,
) -> Result<(), SimError> {
    let start = session.view().snapshot.as_ref().map_or(0, |s| s.steps);
    let used = |session: &Session| {
        session
            .view()
            .snapshot
            .as_ref()
            .map_or(0, |s| s.steps.saturating_sub(start))
    };
    let mut stops = Vec::new();
    let mut remaining_steps = steps.map_or(0, |(_, count)| count);
    loop {
        let state = session.view().state;
        if matches!(state, RunState::Halted | RunState::Faulted) || used(session) >= budget {
            break;
        }
        match steps {
            Some((kind, _)) if remaining_steps > 0 => {
                session.apply(Command::SourceStep(kind))?;
                remaining_steps -= 1;
            }
            Some(_) => break,
            None => session.apply(Command::Run)?,
        }
        while session.view().state == RunState::Running {
            if used(session) >= budget {
                session.apply(Command::Pause)?;
                break;
            }
            session.run_batch(vole_runtime::RUN_BATCH);
        }
        if session.view().state == RunState::Paused
            && let Some(debug) = &session.view().debug
        {
            if !json {
                print_stop(&session.view().message, debug);
            }
            stops.push(serde_json::json!({
                "message": session.view().message,
                "debug": debug,
            }));
        }
        if steps.is_none() && session.view().state == RunState::Paused && used(session) >= budget {
            break;
        }
    }
    let view = session.view();
    let snapshot = view
        .snapshot
        .as_ref()
        .ok_or_else(|| SimError("No machine state.".into()))?;
    let exit_status = view.debug.as_ref().and_then(|debug| debug.exit_status);
    let output = String::from_utf8_lossy(&snapshot.output).to_string();
    if json {
        let report = serde_json::json!({
            "architecture": view.architecture.id(),
            "state": view.state.label(),
            "message": view.message,
            "steps": snapshot.steps,
            "output": output,
            "exit_status": exit_status,
            "stops": stops,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|e| SimError(e.to_string()))?
        );
    } else {
        if !output.is_empty() {
            println!("Output: {output}");
        }
        println!(
            "{}: {} after {} instructions. {}",
            view.architecture.name(),
            view.state.label(),
            snapshot.steps,
            view.message
        );
        if let Some(status) = exit_status {
            println!("Exit status: {status}");
        }
    }
    if view.state == RunState::Faulted {
        return Err(SimError(view.message.clone()));
    }
    if steps.is_none() && !snapshot.halted {
        return Err(SimError("Instruction budget reached before exit.".into()));
    }
    Ok(())
}

fn print_stop(message: &str, debug: &DebugView) {
    println!("{message}");
    for frame in &debug.frames {
        let line = frame
            .location
            .as_ref()
            .filter(|location| location.user)
            .map(|location| format!("line {}", location.line))
            .unwrap_or_else(|| format!("{:#x}", frame.pc));
        println!("  #{} {} at {line}", frame.index, frame.function);
        if frame.index == 0 {
            for variable in &frame.variables {
                println!("      {}", describe(variable));
            }
        }
    }
    for global in &debug.globals {
        println!("    global {}", describe(global));
    }
    if let Some(note) = &debug.note {
        println!("  note: {note}");
    }
}

fn describe(variable: &VariableView) -> String {
    let value = match &variable.value {
        ValueText::Value(text) => text.clone(),
        ValueText::Unavailable(reason) => format!("<{reason}>"),
    };
    format!(
        "{} {} = {value}{}",
        variable.type_name,
        variable.name,
        if variable.changed { "  (changed)" } else { "" }
    )
}

fn next(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, SimError> {
    args.next()
        .ok_or_else(|| SimError(format!("{flag} requires a value.")))
}

fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>, SimError> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| SimError(e.to_string()))?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| SimError(e.to_string()))?;
    if bytes.len() > limit {
        return Err(SimError(format!(
            "{} exceeds the {} byte limit.",
            path.display(),
            limit
        )));
    }
    Ok(bytes)
}
