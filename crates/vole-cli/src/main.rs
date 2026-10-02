use std::{env, fs, io::Read, path::Path, process::ExitCode};
use vole_core::{Architecture, SimError};
use vole_project::Project;
use vole_runtime::{Command, RunState, Session, export_bytes, import_bytes};

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
    let mut architecture = Architecture::Vole;
    let mut source_path = None;
    let mut bytes_path = None;
    let mut project_path = None;
    let mut export_path = None;
    let mut budget = 10000_u64;
    let mut json = false;
    let mut assemble_only = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "Vole headless assembler and simulator\n\nvole-cli [--arch vole|arm32|arm64|x86|x64] [--source FILE | --bytes FILE | --project FILE]\n         [--steps N] [--json] [--assemble-only] [--export-bytes FILE]\n\nWithout a file, runs the selected target's addition example.\nReal-ISA source assembly requires llvm-mc and ld.lld on PATH or VOLE_TOOLCHAIN_DIR.\nImported machine bytes and VOLE do not require LLVM. Execution uses documented scalar subsets."
                );
                return Ok(());
            }
            "--arch" => {
                let value = next(&mut args, "--arch")?;
                architecture = Architecture::parse(&value)
                    .ok_or_else(|| SimError(format!("Unknown architecture: {value}")))?;
            }
            "--source" => source_path = Some(next(&mut args, "--source")?),
            "--bytes" => bytes_path = Some(next(&mut args, "--bytes")?),
            "--project" => project_path = Some(next(&mut args, "--project")?),
            "--export-bytes" => export_path = Some(next(&mut args, "--export-bytes")?),
            "--steps" => {
                budget = next(&mut args, "--steps")?
                    .parse()
                    .map_err(|_| SimError("--steps requires a positive integer.".into()))?
            }
            "--json" => json = true,
            "--assemble-only" => assemble_only = true,
            _ => return Err(SimError(format!("Unknown argument {arg}. Use --help."))),
        }
    }
    if budget == 0 || budget > 1_000_000 {
        return Err(SimError("--steps must be between 1 and 1000000.".into()));
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
    if let Some(project) = &project {
        architecture = project.architecture;
    }
    let source = if let Some(project) = &project {
        project.source.clone()
    } else if let Some(path) = source_path {
        read_limited(Path::new(&path), 1024 * 1024).and_then(|b| {
            String::from_utf8(b).map_err(|_| SimError("Source must be UTF-8.".into()))
        })?
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
            session.assemble(architecture, source)?;
        }
    } else if let Some(path) = bytes_path {
        session.load(import_bytes(
            architecture,
            &read_limited(Path::new(&path), 0x1F000)?,
        )?)?;
    } else {
        session.assemble(architecture, source)?;
    }
    if let Some(project) = project {
        session.apply(Command::SetBreakpoints(project.breakpoints))?;
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
