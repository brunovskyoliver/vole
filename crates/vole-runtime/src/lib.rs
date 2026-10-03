//! Serialized, bounded execution and immutable desktop observations.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, RwLock, mpsc},
    thread,
    time::{Duration, Instant},
};
use vole_core::{
    Architecture, CompilerSettings, Diagnostic, Instruction, Machine, Program, SimError, Snapshot,
    SourceLanguage, StepRecord, debug::DebugView,
};

pub const RUN_INSTRUCTION_BUDGET: u64 = 1_000_000;
pub const RUN_BATCH: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState {
    Editing,
    Assembling,
    Ready,
    Running,
    Paused,
    Halted,
    Faulted,
}

impl RunState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Editing => "Needs assembly",
            Self::Assembling => "Assembling",
            Self::Ready => "Ready",
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Halted => "Halted",
            Self::Faulted => "Faulted",
        }
    }
}

/// Source-level stepping requests. Instruction stepping uses [`Command::Step`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceStep {
    /// Stop at the next C statement, entering called user functions.
    Into,
    /// Stop at the next C statement in this frame or a caller.
    Over,
    /// Run until the current function returns to its caller.
    Out,
}

/// A user source-line breakpoint and the address it resolved to, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceBreakpoint {
    /// 1-based line requested by the user.
    pub line: usize,
    /// Line whose code is used, after snapping forward to the next line with code.
    pub resolved_line: Option<usize>,
    pub address: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watchpoint {
    pub address: u64,
    pub length: usize,
    pub write: bool,
}

#[derive(Debug, Clone)]
pub struct SessionView {
    pub revision: u64,
    pub architecture: Architecture,
    pub source: String,
    pub program: Option<Program>,
    pub snapshot: Option<Snapshot>,
    pub current_instruction: Option<Instruction>,
    pub disassembly: Vec<Instruction>,
    pub diagnostics: Vec<Diagnostic>,
    pub state: RunState,
    pub message: String,
    pub breakpoints: BTreeSet<u64>,
    pub watchpoints: Vec<Watchpoint>,
    pub selected_address: Option<u64>,
    pub dirty: bool,
    pub language: SourceLanguage,
    pub settings: CompilerSettings,
    /// User source-line breakpoints keyed by requested line.
    pub source_breakpoints: BTreeMap<usize, SourceBreakpoint>,
    /// Paused source-level debugger observations for compiled C images.
    pub debug: Option<DebugView>,
    /// Active source-level step while the machine is running.
    pub stepping: Option<SourceStep>,
}

impl SessionView {
    fn empty(architecture: Architecture, source: String) -> Self {
        Self {
            revision: 0,
            architecture,
            source,
            program: None,
            snapshot: None,
            current_instruction: None,
            disassembly: Vec::new(),
            diagnostics: Vec::new(),
            state: RunState::Editing,
            message: "Assemble the document to begin.".into(),
            breakpoints: BTreeSet::new(),
            watchpoints: Vec::new(),
            selected_address: None,
            dirty: true,
            language: SourceLanguage::Assembly,
            settings: CompilerSettings::default(),
            source_breakpoints: BTreeMap::new(),
            debug: None,
            stepping: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    Assemble {
        architecture: Architecture,
        source: String,
    },
    /// Assemble or compile `source` in the given language. Compilation runs on the worker.
    Build {
        architecture: Architecture,
        language: SourceLanguage,
        source: String,
        settings: CompilerSettings,
    },
    MarkStale(String),
    LoadProgram(Program),
    Restore {
        program: Program,
        snapshot: Option<Snapshot>,
    },
    Step,
    /// Source-level step for compiled C images; bounded and pausable like Run.
    SourceStep(SourceStep),
    Reverse,
    Reset,
    Run,
    Pause,
    ToggleBreakpoint(u64),
    SetBreakpoints(BTreeSet<u64>),
    /// Toggle a breakpoint on a 1-based user source line.
    ToggleSourceBreakpoint(usize),
    SetSourceBreakpoints(BTreeSet<usize>),
    SelectAddress(u64),
    EditMemory {
        address: u64,
        bytes: Vec<u8>,
    },
    EditRegister {
        name: String,
        value: u64,
    },
    EditFlag {
        name: String,
        value: bool,
    },
    SetWatchpoint {
        address: u64,
        length: usize,
        write: bool,
    },
    Shutdown,
}

/// Synchronous public debugger seam, also used by the asynchronous worker.
pub struct Session {
    view: SessionView,
    machine: Option<Box<dyn Machine>>,
    bypass_breakpoint: Option<u64>,
    stopped_breakpoint: Option<u64>,
    remaining_budget: u64,
    code_bytes: Vec<u8>,
    /// Resolved addresses of `view.source_breakpoints`. They are kept apart
    /// from `view.breakpoints` (the user's instruction breakpoints) so toggling
    /// either kind never removes the other; execution stops on the union.
    source_addresses: BTreeSet<u64>,
    /// Active source-level step, advanced by [`Session::run_batch`].
    goal: Option<SourceGoal>,
    /// Current PC, tracked without building snapshots while running.
    pc: u64,
    /// True when the machine advanced after `view.snapshot` was taken.
    stale: bool,
    /// Replace the halt message with the C exit status at the next refresh.
    announce_exit: bool,
    /// Machine position (steps, PC) of the current paused debug view and the
    /// view from the previous distinct stop, which `changed` compares against.
    /// Refreshing the same stop again must not compare a view with itself.
    debug_stop: Option<(u64, u64)>,
    debug_baseline: Option<DebugView>,
}

/// Bookkeeping for a source step in progress.
///
/// Frames are tracked from the executed calls and returns rather than from
/// call-frame information: Clang's CFI is only precise at call sites (not in
/// shrink-wrapped prologues or epilogues at -O1), while every call and return
/// is visible in the step records.
struct SourceGoal {
    kind: SourceStep,
    /// (file, line) of the user row containing the starting PC.
    start_line: Option<(u32, u32)>,
    /// Function containing the starting PC.
    start_function: Option<u64>,
    /// Calls made since the step began: (return address, SP before the call).
    calls: Vec<(u64, u64)>,
    /// Set once the starting frame returned to its caller.
    returned: bool,
    /// True when the last instruction returned from a call.
    just_returned: bool,
    /// Addresses directly after a call instruction.
    return_sites: BTreeSet<u64>,
    /// Register-only shadow updated from step records, so stack checks never
    /// need a full machine snapshot.
    registers: Snapshot,
}

/// Whether a decoded instruction is a call (it writes a return address).
fn is_call(architecture: Architecture, mnemonic: &str) -> bool {
    const CONDITIONS: [&str; 15] = [
        "eq", "ne", "cs", "hs", "cc", "lo", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt",
    ];
    let condition =
        |rest: &str| rest.is_empty() || rest == "le" || rest == "al" || CONDITIONS.contains(&rest);
    match architecture {
        Architecture::Arm64 => matches!(mnemonic, "bl" | "blr"),
        // `bls`/`blt`/`ble`/`blo` are conditional branches, not calls.
        Architecture::Arm32 => mnemonic
            .strip_prefix("blx")
            .or_else(|| mnemonic.strip_prefix("bl"))
            .is_some_and(condition),
        Architecture::X86 | Architecture::X64 => mnemonic.starts_with("call"),
        Architecture::Vole => false,
    }
}

/// Whether a decoded instruction is a function return (as opposed to a
/// jump-table or other indirect branch).
fn is_return(architecture: Architecture, assembly: &str) -> bool {
    let (mnemonic, operands) = assembly.split_once(' ').unwrap_or((assembly, ""));
    let operands = operands.replace(' ', "");
    match architecture {
        Architecture::Arm64 => mnemonic == "ret",
        Architecture::Arm32 => {
            let lists_pc = |list: &str| {
                list.split_once('{').is_some_and(|(_, registers)| {
                    registers
                        .trim_end_matches('}')
                        .split(',')
                        .any(|register| register == "pc")
                })
            };
            (mnemonic.starts_with("bx") && (operands == "lr" || operands == "r14"))
                || ((mnemonic.starts_with("pop") || mnemonic.starts_with("ldm"))
                    && lists_pc(&operands))
                || (mnemonic.starts_with("mov") && (operands == "pc,lr" || operands == "pc,r14"))
        }
        Architecture::X86 | Architecture::X64 => mnemonic.starts_with("ret"),
        Architecture::Vole => false,
    }
}

impl SourceGoal {
    fn sp(&self) -> u64 {
        self.registers
            .register(stack_register(self.registers.architecture))
            .unwrap_or(0)
    }

    fn observe(&mut self, record: &StepRecord) {
        let sp_before = self.sp();
        for change in &record.registers {
            if let Some(register) = self
                .registers
                .registers
                .iter_mut()
                .find(|register| register.name == change.name)
            {
                register.value = change.after;
            }
        }
        self.registers.pc = record.pc_after;
        let pc_name = pc_register(self.registers.architecture);
        if let Some(register) = self
            .registers
            .registers
            .iter_mut()
            .find(|register| register.name == pc_name)
        {
            register.value = record.pc_after;
        }
        self.just_returned = false;
        let sequential = record
            .pc_before
            .wrapping_add(record.instruction.bytes.len() as u64);
        if record.pc_after == sequential {
            return;
        }
        let mnemonic = record
            .instruction
            .assembly
            .split_whitespace()
            .next()
            .unwrap_or("");
        if is_call(self.registers.architecture, mnemonic) {
            self.calls.push((sequential, sp_before));
            return;
        }
        let sp = self.sp();
        // A return lands on the call's return address with the caller's SP.
        if let Some(index) = self
            .calls
            .iter()
            .rposition(|(address, stack)| *address == record.pc_after && *stack == sp)
        {
            self.calls.truncate(index);
            self.just_returned = true;
        } else if self.calls.is_empty()
            && is_return(self.registers.architecture, &record.instruction.assembly)
            && self.return_sites.contains(&record.pc_after)
        {
            self.returned = true;
            self.just_returned = true;
        }
    }

    /// Whether the step is complete after `record`, per the source debugging contract.
    fn reached(&mut self, debug: &vole_core::DebugInfo, record: &StepRecord) -> bool {
        let pc = record.pc_after;
        if self.kind == SourceStep::Out {
            if !self.returned {
                return false;
            }
            if debug.function_at(pc).is_some_and(|function| function.user) {
                return true;
            }
            // Returned into runtime code: keep going until user code.
            self.returned = false;
            return false;
        }
        let Some(row) = vole_debug::statement_start(debug, pc) else {
            return false;
        };
        // Entering a function: its first row precedes the prologue, where
        // locals are unreadable. Continue to the prologue_end row instead.
        if debug
            .functions
            .iter()
            .any(|f| f.low_pc == pc && f.prologue_end.is_some_and(|end| end > pc))
        {
            return false;
        }
        let in_callee = !self.calls.is_empty();
        if self.kind == SourceStep::Over {
            // Calls run to completion; a tail call into another function
            // continues until control is back in this function or its caller.
            let same_function = debug.function_at(pc).map(|f| f.low_pc) == self.start_function;
            if in_callee || !(same_function || self.returned) {
                return false;
            }
        }
        let backward = record.pc_after <= record.pc_before && !self.just_returned;
        Some((row.file, row.line)) != self.start_line || in_callee || self.returned || backward
    }
}

fn pc_register(architecture: Architecture) -> &'static str {
    match architecture {
        Architecture::X86 => "eip",
        Architecture::X64 => "rip",
        _ => "pc",
    }
}

fn stack_register(architecture: Architecture) -> &'static str {
    match architecture {
        Architecture::Arm32 => "r13",
        Architecture::X86 => "esp",
        Architecture::X64 => "rsp",
        _ => "sp",
    }
}

fn register_shadow(snapshot: &Snapshot) -> Snapshot {
    Snapshot {
        architecture: snapshot.architecture,
        pc: snapshot.pc,
        registers: snapshot.registers.clone(),
        flags: BTreeMap::new(),
        memory: Vec::new(),
        output: Vec::new(),
        halted: false,
        steps: snapshot.steps,
        trace: Vec::new(),
    }
}

impl Session {
    pub fn new(architecture: Architecture, source: String) -> Self {
        Self {
            view: SessionView::empty(architecture, source),
            machine: None,
            bypass_breakpoint: None,
            stopped_breakpoint: None,
            remaining_budget: RUN_INSTRUCTION_BUDGET,
            code_bytes: Vec::new(),
            source_addresses: BTreeSet::new(),
            goal: None,
            pc: 0,
            stale: false,
            announce_exit: false,
            debug_stop: None,
            debug_baseline: None,
        }
    }

    pub fn view(&self) -> &SessionView {
        &self.view
    }

    fn mark_assembling(&mut self, language: SourceLanguage) {
        self.view.state = RunState::Assembling;
        self.view.dirty = true;
        self.view.diagnostics.clear();
        self.view.message = match language {
            SourceLanguage::Assembly => "Assembling source.",
            SourceLanguage::C => "Compiling C source.",
        }
        .into();
        self.view.revision += 1;
    }

    pub fn assemble(&mut self, architecture: Architecture, source: String) -> Result<(), SimError> {
        self.build(
            architecture,
            SourceLanguage::Assembly,
            source,
            CompilerSettings::default(),
        )
    }

    /// Assemble or compile a document and load the resulting image.
    pub fn build(
        &mut self,
        architecture: Architecture,
        language: SourceLanguage,
        source: String,
        settings: CompilerSettings,
    ) -> Result<(), SimError> {
        if self.view.architecture != architecture || self.view.language != language {
            self.view.breakpoints.clear();
            self.view.watchpoints.clear();
            self.view.program = None;
            self.view.snapshot = None;
            self.view.current_instruction = None;
            self.view.disassembly.clear();
            self.view.debug = None;
            self.machine = None;
            self.goal = None;
            self.view.stepping = None;
            self.source_addresses.clear();
        }
        // Source lines keep their meaning across targets, so source
        // breakpoints survive a rebuild unless the language changes.
        if self.view.language != language {
            self.view.source_breakpoints.clear();
        }
        self.view.architecture = architecture;
        self.view.language = language;
        self.view.settings = settings.clone();
        self.view.source = source.clone();
        self.view.state = RunState::Assembling;
        self.view.dirty = true;
        self.view.diagnostics.clear();
        let assembled = if source.len() > 1024 * 1024 {
            Err(vec![Diagnostic::new(1, "Source exceeds the 1 MiB limit.")])
        } else {
            match (language, architecture) {
                (SourceLanguage::C, Architecture::Vole) => Err(vec![Diagnostic::new(
                    1,
                    "C compilation targets ARM32, ARM64, x86 and x64. VOLE programs use VOLE assembly.",
                )]),
                (SourceLanguage::C, _) => compile_c(architecture, &source, &settings),
                (SourceLanguage::Assembly, Architecture::Vole) => {
                    vole_isa_vole::assemble(&source).map(|program| (program, Vec::new()))
                }
                (SourceLanguage::Assembly, _) => vole_isa_scalar::assemble(architecture, &source)
                    .map(|program| (program, Vec::new())),
            }
        };
        let result = assembled.and_then(|(program, warnings)| {
            self.load(program)
                .map_err(|error| {
                    vec![Diagnostic::new(
                        1,
                        format!("Executable could not be loaded: {error}"),
                    )]
                })
                .map(|()| warnings)
        });
        match result {
            Ok(warnings) => {
                if !warnings.is_empty() {
                    self.view.message = format!(
                        "Compiled with {} warning{}. Ready to execute.",
                        warnings.len(),
                        if warnings.len() == 1 { "" } else { "s" }
                    );
                }
                self.view.diagnostics = warnings;
                Ok(())
            }
            Err(diagnostics) => {
                let message = diagnostics
                    .iter()
                    .find(|d| d.is_error())
                    .or(diagnostics.first())
                    .map(|d| format!("Line {}: {}", d.line, d.message))
                    .unwrap_or_else(|| match language {
                        SourceLanguage::Assembly => "Assembly failed.".into(),
                        SourceLanguage::C => "Compilation failed.".into(),
                    });
                self.view.diagnostics = diagnostics;
                self.view.state = RunState::Editing;
                self.view.dirty = true;
                self.view.message = message.clone();
                self.view.revision += 1;
                Err(SimError(message))
            }
        }
    }

    pub fn load(&mut self, program: Program) -> Result<(), SimError> {
        validate_image(&program)?;
        let mut machine: Box<dyn Machine> = match program.architecture {
            Architecture::Vole => Box::new(vole_isa_vole::VoleMachine::new()),
            architecture => Box::new(vole_isa_scalar::ScalarMachine::new(architecture)),
        };
        machine.load(&program)?;
        if self.view.architecture != program.architecture {
            self.view.breakpoints.clear();
            self.view.watchpoints.clear();
        }
        if self.view.language != program.language {
            self.view.source_breakpoints.clear();
        }
        if let Some(debug) = &program.debug {
            self.view.settings = debug.settings.clone();
        }
        self.view.architecture = program.architecture;
        self.view.source = program.source.clone();
        self.view.language = program.language;
        self.view.program = Some(program);
        self.view.debug = None;
        self.debug_stop = None;
        self.debug_baseline = None;
        self.view.stepping = None;
        self.goal = None;
        self.view.state = RunState::Ready;
        self.view.message = "Ready to execute.".into();
        self.view.diagnostics.clear();
        self.view.selected_address = None;
        self.view.dirty = false;
        self.bypass_breakpoint = None;
        self.stopped_breakpoint = None;
        self.machine = Some(machine);
        self.code_bytes.clear();
        self.resolve_source_breakpoints();
        self.refresh();
        Ok(())
    }

    /// Re-resolve source breakpoint lines against the loaded image's line table.
    fn resolve_source_breakpoints(&mut self) {
        let debug = self
            .view
            .program
            .as_ref()
            .and_then(|program| program.debug.as_ref());
        self.source_addresses.clear();
        for breakpoint in self.view.source_breakpoints.values_mut() {
            let resolved =
                debug.and_then(|debug| vole_debug::breakpoint_address(debug, breakpoint.line));
            breakpoint.address = resolved.map(|(address, _)| address);
            breakpoint.resolved_line = resolved.map(|(_, line)| line);
            if let Some(address) = breakpoint.address {
                self.source_addresses.insert(address);
            }
        }
    }

    fn is_breakpoint(&self, address: u64) -> bool {
        self.view.breakpoints.contains(&address) || self.source_addresses.contains(&address)
    }

    fn breakpoint_message(&self, address: u64) -> String {
        if !self.view.breakpoints.contains(&address)
            && let Some(breakpoint) = self
                .view
                .source_breakpoints
                .values()
                .find(|breakpoint| breakpoint.address == Some(address))
        {
            return format!(
                "Paused at breakpoint on line {}.",
                breakpoint.resolved_line.unwrap_or(breakpoint.line)
            );
        }
        format!("Paused before breakpoint at {address:X}.")
    }

    /// Begin a bounded source-level step; [`Session::run_batch`] advances it.
    fn start_source_step(&mut self, kind: SourceStep) -> Result<(), SimError> {
        self.ensure_assembled()?;
        let program = self.view.program.as_ref().ok_or_else(no_machine)?;
        let Some(debug) = program.debug.as_ref() else {
            return Err(SimError(
                "Source stepping needs a compiled C program; use Step for instructions.".into(),
            ));
        };
        if self.view.state == RunState::Running {
            return Err(SimError("Pause before stepping.".into()));
        }
        let snapshot = self.view.snapshot.as_ref().ok_or_else(no_machine)?;
        if snapshot.halted {
            return Err(SimError(
                "Program halted. Reset or reverse before stepping.".into(),
            ));
        }
        let architecture = program.architecture;
        let start_line = vole_debug::line_at(debug, snapshot.pc)
            .filter(|row| row.line != 0 && debug.is_user_file(row.file))
            .map(|row| (row.file, row.line));
        let registers = register_shadow(snapshot);
        let start_function = debug.function_at(snapshot.pc).map(|f| f.low_pc);
        let return_sites = program
            .instructions
            .iter()
            .filter(|instruction| {
                let mnemonic = instruction.assembly.split_whitespace().next().unwrap_or("");
                is_call(architecture, mnemonic)
            })
            .map(|instruction| instruction.address + instruction.bytes.len() as u64)
            .collect();
        // Without a user line there is nothing to step over: from startup code
        // `main` itself is a callee. Step Over then behaves like Step Into.
        let effective = if kind == SourceStep::Over && start_line.is_none() {
            SourceStep::Into
        } else {
            kind
        };
        self.goal = Some(SourceGoal {
            kind: effective,
            start_line,
            start_function,
            calls: Vec::new(),
            returned: false,
            just_returned: false,
            return_sites,
            registers,
        });
        // Leaving a breakpoint location must not stop on that same breakpoint.
        self.bypass_breakpoint = Some(snapshot.pc);
        self.stopped_breakpoint = None;
        self.remaining_budget = RUN_INSTRUCTION_BUDGET;
        self.view.state = RunState::Running;
        self.view.stepping = Some(kind);
        self.view.message = match kind {
            SourceStep::Into => "Stepping to the next statement.",
            SourceStep::Over => "Stepping over the current line.",
            SourceStep::Out => "Running until the current function returns.",
        }
        .into();
        Ok(())
    }

    /// Run batches until the machine stops. Intended for synchronous callers
    /// such as tests and the CLI; the worker interleaves batches with commands.
    pub fn run_to_stop(&mut self) {
        while self.view.state == RunState::Running {
            self.run_batch(RUN_BATCH);
        }
    }

    pub fn apply(&mut self, command: Command) -> Result<(), SimError> {
        self.sync();
        let result = (|| match command {
            Command::Assemble {
                architecture,
                source,
            } => self.assemble(architecture, source),
            Command::Build {
                architecture,
                language,
                source,
                settings,
            } => self.build(architecture, language, source, settings),
            Command::SourceStep(kind) => self.start_source_step(kind),
            Command::ToggleSourceBreakpoint(line) => {
                validate_line(line)?;
                if self.view.source_breakpoints.remove(&line).is_none() {
                    self.view.source_breakpoints.insert(
                        line,
                        SourceBreakpoint {
                            line,
                            resolved_line: None,
                            address: None,
                        },
                    );
                }
                self.resolve_source_breakpoints();
                Ok(())
            }
            Command::SetSourceBreakpoints(lines) => {
                for line in &lines {
                    validate_line(*line)?;
                }
                self.view.source_breakpoints = lines
                    .into_iter()
                    .map(|line| {
                        (
                            line,
                            SourceBreakpoint {
                                line,
                                resolved_line: None,
                                address: None,
                            },
                        )
                    })
                    .collect();
                self.resolve_source_breakpoints();
                Ok(())
            }
            Command::LoadProgram(program) => self.load(program),
            Command::Restore { program, snapshot } => {
                let mut staged = Session::new(program.architecture, program.source.clone());
                staged.load(program)?;
                if let Some(snapshot) = snapshot {
                    staged
                        .machine
                        .as_mut()
                        .ok_or_else(no_machine)?
                        .restore_snapshot(&snapshot)?;
                    staged.view.state = if snapshot.halted {
                        RunState::Halted
                    } else {
                        RunState::Paused
                    };
                    staged.view.message =
                        "Restored the saved machine state. New undo history begins here.".into();
                    // A saved pause resumes like any pause: Run first leaves this
                    // location even when a breakpoint is set on it.
                    staged.stopped_breakpoint = Some(snapshot.pc);
                    staged.refresh();
                }
                if self.view.architecture == staged.view.architecture {
                    staged.view.breakpoints = self.view.breakpoints.clone();
                    staged.view.watchpoints = self.view.watchpoints.clone();
                }
                if self.view.language == staged.view.language {
                    staged.view.source_breakpoints = self.view.source_breakpoints.clone();
                    staged.resolve_source_breakpoints();
                }
                staged.view.revision = self.view.revision + 1;
                *self = staged;
                Ok(())
            }
            Command::MarkStale(source) => {
                self.view.source = source;
                self.view.dirty = true;
                self.view.state = RunState::Editing;
                self.view.message = match self.view.language {
                    SourceLanguage::Assembly => "Source changed. Assemble before running.",
                    SourceLanguage::C => "Source changed. Compile before running.",
                }
                .into();
                Ok(())
            }
            Command::Step => self.step(),
            Command::Reverse => {
                self.ensure_assembled()?;
                self.machine
                    .as_mut()
                    .ok_or_else(no_machine)?
                    .reverse_step()?;
                self.stopped_breakpoint = None;
                self.bypass_breakpoint = None;
                self.view.state = RunState::Paused;
                self.view.message = "Restored the previous instruction state.".into();
                self.refresh();
                Ok(())
            }
            Command::Reset => {
                self.ensure_assembled()?;
                self.machine.as_mut().ok_or_else(no_machine)?.reset()?;
                self.view.state = RunState::Ready;
                self.view.message = "Restored the loaded image.".into();
                self.bypass_breakpoint = None;
                self.stopped_breakpoint = None;
                self.refresh();
                Ok(())
            }
            Command::Run => {
                self.ensure_assembled()?;
                if self.view.snapshot.as_ref().is_some_and(|s| s.halted) {
                    return Err(SimError(
                        "Program halted. Reset or reverse before running.".into(),
                    ));
                }
                self.bypass_breakpoint = self.stopped_breakpoint.take().filter(|address| {
                    self.view
                        .snapshot
                        .as_ref()
                        .is_some_and(|s| s.pc == *address)
                });
                self.remaining_budget = RUN_INSTRUCTION_BUDGET;
                self.goal = None;
                self.view.stepping = None;
                self.view.state = RunState::Running;
                self.view.message = "Executing instructions.".into();
                Ok(())
            }
            Command::Pause => {
                if self.view.state == RunState::Running {
                    self.view.state = RunState::Paused;
                    self.view.message = "Paused by you.".into();
                    // Builds the paused source view; the snapshot is already fresh.
                    self.refresh();
                }
                Ok(())
            }
            Command::ToggleBreakpoint(address) => {
                self.validate_address(address)?;
                if !self.view.breakpoints.remove(&address) {
                    self.view.breakpoints.insert(address);
                }
                Ok(())
            }
            Command::SetBreakpoints(addresses) => {
                for address in &addresses {
                    self.validate_address(*address)?;
                }
                self.view.breakpoints = addresses;
                Ok(())
            }
            Command::SelectAddress(address) => {
                self.view.selected_address = Some(address);
                Ok(())
            }
            Command::EditMemory { address, bytes } => {
                self.ensure_editable()?;
                self.machine
                    .as_mut()
                    .ok_or_else(no_machine)?
                    .write_memory(address, &bytes)?;
                self.stopped_breakpoint = None;
                self.bypass_breakpoint = None;
                self.view.state = RunState::Paused;
                self.view.message = "Memory changed. Execution history was cleared.".into();
                self.refresh();
                Ok(())
            }
            Command::EditRegister { name, value } => {
                self.ensure_editable()?;
                self.machine
                    .as_mut()
                    .ok_or_else(no_machine)?
                    .write_register(&name, value)?;
                self.stopped_breakpoint = None;
                self.bypass_breakpoint = None;
                self.view.state = RunState::Paused;
                self.view.message = "Register changed. Execution history was cleared.".into();
                self.refresh();
                Ok(())
            }
            Command::EditFlag { name, value } => {
                self.ensure_editable()?;
                let machine = self.machine.as_mut().ok_or_else(no_machine)?;
                let mut snapshot = machine.snapshot();
                let flag = snapshot
                    .flags
                    .get_mut(&name)
                    .ok_or_else(|| SimError(format!("Unknown flag {name}.")))?;
                *flag = value;
                snapshot.trace.clear();
                machine.restore_snapshot(&snapshot)?;
                self.stopped_breakpoint = None;
                self.bypass_breakpoint = None;
                self.view.state = if snapshot.halted {
                    RunState::Halted
                } else {
                    RunState::Paused
                };
                self.view.message = "Flag changed. Execution history was cleared.".into();
                self.refresh();
                Ok(())
            }
            Command::SetWatchpoint {
                address,
                length,
                write,
            } => {
                self.validate_address(address)?;
                if length == 0 || length > 65536 || address.checked_add(length as u64 - 1).is_none()
                {
                    return Err(SimError("Watchpoint range is invalid.".into()));
                }
                self.validate_address(address + length as u64 - 1)?;
                let watchpoint = Watchpoint {
                    address,
                    length,
                    write,
                };
                if let Some(index) = self.view.watchpoints.iter().position(|w| *w == watchpoint) {
                    self.view.watchpoints.remove(index);
                } else {
                    self.view.watchpoints.push(watchpoint);
                }
                Ok(())
            }
            Command::Shutdown => Ok(()),
        })();
        if let Err(error) = &result {
            self.view.message = error.to_string();
        }
        if self.view.state != RunState::Running {
            self.goal = None;
            self.view.stepping = None;
        }
        self.view.revision += 1;
        result
    }

    pub fn step(&mut self) -> Result<(), SimError> {
        self.ensure_assembled()?;
        self.stopped_breakpoint = None;
        let result = self.machine.as_mut().ok_or_else(no_machine)?.step();
        match result {
            Ok(record) => {
                self.view.state = if record.halted {
                    RunState::Halted
                } else {
                    RunState::Paused
                };
                self.view.message = if record.halted {
                    self.announce_exit = true;
                    "Program halted.".into()
                } else {
                    format!("Executed instruction at {:X}.", record.pc_before)
                };
                self.refresh();
                Ok(())
            }
            Err(error) => {
                self.view.state = RunState::Faulted;
                self.view.message = error.to_string();
                self.refresh();
                Err(error)
            }
        }
    }

    /// Advance a running machine for a bounded amount of work, yielding to commands.
    ///
    /// Runs and source steps share this loop: breakpoints are checked before
    /// each instruction, watchpoints and step goals after it using the step
    /// record, and the published snapshot is rebuilt once at the end.
    pub fn run_batch(&mut self, max_steps: usize) {
        self.execute_batch(max_steps);
        self.sync();
    }

    /// The instruction loop of [`Session::run_batch`] without the final snapshot.
    fn execute_batch(&mut self, max_steps: usize) {
        let started = Instant::now();
        let mut last_executed = None;
        for _ in 0..max_steps.min(RUN_BATCH) {
            if self.view.state != RunState::Running {
                break;
            }
            let pc = self.pc;
            if self.is_breakpoint(pc) && self.bypass_breakpoint != Some(pc) {
                self.view.state = RunState::Paused;
                self.view.message = self.breakpoint_message(pc);
                self.stopped_breakpoint = Some(pc);
                break;
            }
            self.bypass_breakpoint = None;
            if self.remaining_budget == 0 {
                self.view.state = RunState::Paused;
                self.view.message = if self.goal.is_some() {
                    "Instruction budget reached before the source step finished. Step again or Run to continue."
                } else {
                    "Instruction budget reached. Run again to continue."
                }
                .into();
                break;
            }
            let Some(machine) = self.machine.as_mut() else {
                break;
            };
            self.stopped_breakpoint = None;
            let record = match machine.step() {
                Ok(record) => record,
                Err(error) => {
                    self.view.state = RunState::Faulted;
                    self.view.message = error.to_string();
                    self.stale = true;
                    break;
                }
            };
            self.stale = true;
            self.pc = record.pc_after;
            self.remaining_budget -= 1;
            if record.halted {
                self.view.state = RunState::Halted;
                self.view.message = "Program halted.".into();
                self.announce_exit = true;
                break;
            }
            if self.watchpoint_hit(&record) {
                self.view.state = RunState::Paused;
                self.view.message = format!(
                    "Watchpoint accessed by instruction at {:X}.",
                    record.pc_before
                );
                break;
            }
            if let Some(goal) = &mut self.goal {
                goal.observe(&record);
                let debug = self
                    .view
                    .program
                    .as_ref()
                    .and_then(|program| program.debug.as_ref());
                if let Some(debug) = debug
                    && goal.reached(debug, &record)
                {
                    let line = vole_debug::line_at(debug, record.pc_after)
                        .filter(|row| debug.is_user_file(row.file))
                        .map(|row| row.line);
                    self.view.state = RunState::Paused;
                    self.view.message = match line {
                        Some(line) => format!("Stepped to line {line}."),
                        None => format!("Stepped to {:X}.", record.pc_after),
                    };
                    // Run from here continues past a breakpoint at this address.
                    if self.is_breakpoint(record.pc_after) {
                        self.stopped_breakpoint = Some(record.pc_after);
                    }
                    break;
                }
            }
            last_executed = Some(record.pc_before);
            if started.elapsed() >= Duration::from_millis(8) {
                break;
            }
        }
        // A stop before any instruction in this batch (e.g. a breakpoint at
        // the batch boundary) still needs a fresh paused view.
        if self.view.state != RunState::Running {
            self.stale = true;
        }
        if self.view.state == RunState::Running
            && self.goal.is_none()
            && let Some(pc) = last_executed
        {
            self.view.message = format!("Executed instruction at {pc:X}.");
        }
        self.view.revision += 1;
    }

    fn watchpoint_hit(&self, record: &StepRecord) -> bool {
        self.view.watchpoints.iter().any(|watch| {
            if watch.write {
                record.memory.iter().any(|change| {
                    ranges_overlap(
                        watch.address,
                        watch.length,
                        change.address,
                        change.after.len(),
                    )
                })
            } else {
                record.memory_reads.iter().any(|access| {
                    ranges_overlap(watch.address, watch.length, access.address, access.length)
                })
            }
        })
    }

    /// Rebuild the published snapshot if the machine advanced since the last one.
    fn sync(&mut self) {
        if self.stale {
            self.refresh();
        }
    }

    fn validate_address(&self, address: u64) -> Result<(), SimError> {
        if (self.view.architecture == Architecture::Vole && address > 255)
            || (self.view.architecture.bits() == 32 && address > u32::MAX as u64)
        {
            Err(SimError(
                "Address does not fit the selected architecture.".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn ensure_assembled(&self) -> Result<(), SimError> {
        if self.view.dirty {
            Err(SimError(
                match self.view.language {
                    SourceLanguage::Assembly => "Assemble the current source before executing.",
                    SourceLanguage::C => "Compile the current source before executing.",
                }
                .into(),
            ))
        } else if self.machine.is_none() {
            Err(no_machine())
        } else {
            Ok(())
        }
    }

    fn ensure_editable(&self) -> Result<(), SimError> {
        self.ensure_assembled()?;
        if self.view.state == RunState::Running {
            Err(SimError("Pause before editing machine state.".into()))
        } else {
            Ok(())
        }
    }

    fn refresh(&mut self) {
        if let Some(machine) = &self.machine {
            let snapshot = machine.snapshot();
            let code_bytes: Vec<u8> = snapshot
                .memory
                .iter()
                .filter(|region| region.executable)
                .flat_map(|region| region.bytes.iter().copied())
                .collect();
            if code_bytes != self.code_bytes {
                if let Some(program) = &self.view.program {
                    self.view.disassembly = live_disassembly(program, &snapshot);
                }
                self.code_bytes = code_bytes;
            }
            let address = snapshot.pc;
            let decoded = if snapshot.architecture == Architecture::Vole {
                let bytes = [
                    snapshot.byte(address).unwrap_or(0),
                    snapshot.byte((address + 1) & 255).unwrap_or(0),
                ];
                vole_isa_vole::decode(address, &bytes)
            } else {
                let bytes: Vec<u8> = (0..16)
                    .map_while(|offset| address.checked_add(offset).and_then(|a| snapshot.byte(a)))
                    .collect();
                vole_isa_scalar::decode(snapshot.architecture, address, &bytes)
            };
            self.view.current_instruction = decoded.ok().map(|mut instruction| {
                if let Some(known) = self
                    .view
                    .program
                    .as_ref()
                    .and_then(|program| program.instruction(address))
                    .filter(|known| instruction.bytes == known.bytes)
                {
                    instruction.source_line = known.source_line;
                }
                instruction
            });
            if self.view.state != RunState::Running {
                // Paused observations only; `changed` compares with the previous stop.
                let stop = (snapshot.steps, snapshot.pc);
                if self.debug_stop != Some(stop) {
                    self.debug_baseline = self.view.debug.take();
                    self.debug_stop = Some(stop);
                }
                self.view.debug = self.view.program.as_ref().and_then(|program| {
                    vole_debug::debug_view(program, &snapshot, self.debug_baseline.as_ref())
                });
                self.goal = None;
                self.view.stepping = None;
            }
            if std::mem::take(&mut self.announce_exit)
                && self.view.state == RunState::Halted
                && let Some(status) = self.view.debug.as_ref().and_then(|d| d.exit_status)
            {
                self.view.message = format!("Program exited with status {status}.");
            }
            self.pc = snapshot.pc;
            self.stale = false;
            self.view.snapshot = Some(snapshot);
            self.view.revision += 1;
        }
    }
}

fn no_machine() -> SimError {
    SimError("Assemble or load a program first.".into())
}

fn validate_line(line: usize) -> Result<(), SimError> {
    if line == 0 || line > 1_000_000 {
        Err(SimError("Source breakpoint line is invalid.".into()))
    } else {
        Ok(())
    }
}

/// Checks that keep the debugger's table lookups meaningful for saved C images.
fn validate_image(program: &Program) -> Result<(), SimError> {
    match (program.language, &program.debug) {
        (SourceLanguage::C, _) if program.architecture == Architecture::Vole => {
            Err(SimError("C images target ARM32, ARM64, x86 or x64.".into()))
        }
        (SourceLanguage::C, None) => Err(SimError(
            "C image is missing its debug information. Rebuild the document.".into(),
        )),
        (_, Some(debug)) => vole_debug::validate(debug)
            .map_err(|error| SimError(format!("Invalid debug information: {error}"))),
        (SourceLanguage::Assembly, None) => Ok(()),
    }
}

/// Compile a C document, returning the image and any warnings or notes.
fn compile_c(
    architecture: Architecture,
    source: &str,
    settings: &CompilerSettings,
) -> Result<(Program, Vec<Diagnostic>), Vec<Diagnostic>> {
    vole_c::compile_with_warnings(architecture, source, settings)
}

fn live_disassembly(program: &Program, snapshot: &Snapshot) -> Vec<Instruction> {
    let mut originals: Vec<&Instruction> = program.instructions.iter().collect();
    originals.sort_by_key(|instruction| instruction.address);
    let sources: std::collections::BTreeMap<u64, &Instruction> = originals
        .iter()
        .map(|instruction| (instruction.address, *instruction))
        .collect();
    let mut spans: Vec<(u64, u64)> = Vec::new();
    for instruction in originals {
        let Some(end) = instruction
            .address
            .checked_add(instruction.bytes.len() as u64)
        else {
            continue;
        };
        if let Some(last) = spans
            .last_mut()
            .filter(|last| last.1 == instruction.address)
        {
            last.1 = end;
        } else {
            spans.push((instruction.address, end));
        }
    }
    let mut instructions = Vec::new();
    for (mut address, end) in spans {
        while address < end {
            let Some(first) = snapshot.byte(address) else {
                break;
            };
            let decoded = if program.architecture == Architecture::Vole {
                let bytes = [first, snapshot.byte((address + 1) & 255).unwrap_or(0)];
                vole_isa_vole::decode(address, &bytes)
            } else {
                let bytes: Vec<u8> = (0..16)
                    .map_while(|offset| {
                        address
                            .checked_add(offset)
                            .filter(|a| *a < end)
                            .and_then(|a| snapshot.byte(a))
                    })
                    .collect();
                vole_isa_scalar::decode(program.architecture, address, &bytes)
            };
            let mut instruction = decoded.unwrap_or_else(|_| Instruction {
                address,
                bytes: vec![first],
                assembly: format!("db 0x{first:02X}"),
                explanation: "Data byte or unsupported instruction.".into(),
                source_line: None,
                reads: Vec::new(),
                writes: Vec::new(),
            });
            if let Some(original) = sources
                .get(&address)
                .filter(|original| original.bytes == instruction.bytes)
            {
                instruction.source_line = original.source_line;
            }
            let Some(next) = address.checked_add(instruction.bytes.len().max(1) as u64) else {
                break;
            };
            address = next;
            instructions.push(instruction);
        }
    }
    instructions
}

fn ranges_overlap(a: u64, a_len: usize, b: u64, b_len: usize) -> bool {
    a_len > 0
        && b_len > 0
        && a <= b.saturating_add(b_len as u64 - 1)
        && b <= a.saturating_add(a_len as u64 - 1)
}

/// Import guest machine bytes without an assembler or host toolchain.
pub fn import_bytes(architecture: Architecture, bytes: &[u8]) -> Result<Program, SimError> {
    let maximum = if architecture == Architecture::Vole {
        256
    } else {
        0x1F000
    };
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(SimError(format!(
            "Machine image must contain 1 through {maximum} bytes."
        )));
    }
    let entry = if architecture == Architecture::Vole {
        0
    } else {
        0x1000
    };
    let mut instructions = Vec::new();
    let mut offset = 0;
    let code_length = if architecture == Architecture::Vole {
        bytes.len()
    } else {
        bytes.len().min(4096)
    };
    while offset < code_length {
        let address = entry + offset as u64;
        let decoded = if architecture == Architecture::Vole {
            vole_isa_vole::decode(address, &bytes[offset..code_length])
        } else {
            vole_isa_scalar::decode(architecture, address, &bytes[offset..code_length])
        };
        match decoded {
            Ok(instruction) => {
                offset += instruction.bytes.len().max(1);
                instructions.push(instruction);
            }
            Err(_) => {
                instructions.push(Instruction {
                    address,
                    bytes: vec![bytes[offset]],
                    assembly: format!("db 0x{:02X}", bytes[offset]),
                    explanation: "Data byte or unsupported instruction.".into(),
                    source_line: None,
                    reads: vec![],
                    writes: vec![],
                });
                offset += 1;
            }
        }
    }
    let mut regions = vec![vole_core::MemoryRegion {
        base: entry,
        bytes: bytes[..code_length].to_vec(),
        writable: true,
        executable: true,
        label: "Imported image".into(),
    }];
    let mut initial_registers = std::collections::BTreeMap::new();
    if architecture != Architecture::Vole {
        for (base, length, label) in [
            (0x2000_u64, 0xE000_usize, "Main memory"),
            (0x10000, 0x10000, "Stack"),
        ] {
            let mut data = vec![0; length];
            let offset = (base - entry) as usize;
            if offset < bytes.len() {
                let count = length.min(bytes.len() - offset);
                data[..count].copy_from_slice(&bytes[offset..offset + count]);
            }
            regions.push(vole_core::MemoryRegion {
                base,
                bytes: data,
                writable: true,
                executable: false,
                label: label.into(),
            });
        }
        let stack = match architecture {
            Architecture::Arm32 => "r13",
            Architecture::Arm64 => "sp",
            Architecture::X86 => "esp",
            Architecture::X64 => "rsp",
            Architecture::Vole => unreachable!(),
        };
        initial_registers.insert(stack.into(), 0x20000);
    }
    Ok(Program {
        architecture,
        source: instructions
            .iter()
            .map(|i| i.assembly.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        entry,
        regions,
        instructions,
        symbols: Default::default(),
        initial_registers,
        language: Default::default(),
        debug: None,
    })
}

/// Export loaded segments, retaining address gaps. Addresses are relative to the
/// first segment; the architecture and entry point belong in the project file.
pub fn export_bytes(program: &Program) -> Result<Vec<u8>, SimError> {
    let base = program
        .regions
        .iter()
        .map(|r| r.base)
        .min()
        .ok_or_else(|| SimError("Program contains no bytes.".into()))?;
    let default_entry = if program.architecture == Architecture::Vole {
        0
    } else {
        0x1000
    };
    if program.entry != default_entry || base != default_entry {
        return Err(SimError("Raw bytes cannot preserve this program's origin/entry. Save a project to retain its addresses.".into()));
    }
    let end = program
        .regions
        .iter()
        .map(|r| r.base.checked_add(r.bytes.len() as u64))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| SimError("Program image address overflow.".into()))?
        .into_iter()
        .max()
        .unwrap_or(base);
    let raw_limit = if program.architecture == Architecture::Vole {
        256
    } else {
        0x1f000
    };
    if end - base > raw_limit {
        return Err(SimError(
            "Raw export exceeds this target's importable memory range. Save a project instead."
                .into(),
        ));
    }
    let mut bytes = vec![0; (end - base) as usize];
    for region in &program.regions {
        let offset = (region.base - base) as usize;
        bytes[offset..offset + region.bytes.len()].copy_from_slice(&region.bytes);
    }
    Ok(bytes)
}

#[derive(Clone)]
pub struct Runtime {
    sender: mpsc::Sender<Command>,
    view: Arc<RwLock<Arc<SessionView>>>,
}

impl Runtime {
    pub fn new(architecture: Architecture, source: String) -> Self {
        let (sender, receiver) = mpsc::channel();
        let view = Arc::new(RwLock::new(Arc::new(SessionView::empty(
            architecture,
            source.clone(),
        ))));
        let worker_view = view.clone();
        thread::Builder::new()
            .name("vole-runtime".into())
            .spawn(move || {
                let mut session = Session::new(architecture, source.clone());
                session.mark_assembling(SourceLanguage::Assembly);
                publish(&worker_view, &session);
                let _ = session.assemble(architecture, source);
                publish(&worker_view, &session);
                let mut last_publish = Instant::now();
                loop {
                    let wait = if session.view.state == RunState::Running {
                        Duration::ZERO
                    } else {
                        Duration::from_millis(50)
                    };
                    match receiver.recv_timeout(wait) {
                        Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Ok(command) => {
                            match &command {
                                Command::Assemble { .. } => {
                                    session.mark_assembling(SourceLanguage::Assembly);
                                    publish(&worker_view, &session);
                                }
                                Command::Build { language, .. } => {
                                    session.mark_assembling(*language);
                                    publish(&worker_view, &session);
                                }
                                _ => {}
                            }
                            let _ = session.apply(command);
                            publish(&worker_view, &session);
                            last_publish = Instant::now();
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if session.view.state == RunState::Running {
                        // Snapshots are only rebuilt when they are published.
                        session.execute_batch(RUN_BATCH);
                        if session.view.state != RunState::Running
                            || last_publish.elapsed() >= Duration::from_millis(16)
                        {
                            session.sync();
                            publish(&worker_view, &session);
                            last_publish = Instant::now();
                        }
                    }
                }
            })
            .expect("could not start the simulation worker");
        Self { sender, view }
    }

    pub fn view(&self) -> Arc<SessionView> {
        self.view
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn send(&self, command: Command) -> Result<(), SimError> {
        self.sender
            .send(command)
            .map_err(|_| SimError("The simulation worker stopped. Reopen the application.".into()))
    }
}

fn publish(shared: &RwLock<Arc<SessionView>>, session: &Session) {
    *shared
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(session.view.clone());
}
