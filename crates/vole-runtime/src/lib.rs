//! Serialized, bounded execution and immutable desktop observations.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    sync::{Arc, RwLock, mpsc},
    thread,
    time::{Duration, Instant},
};
use vole_core::{Architecture, Diagnostic, Instruction, Machine, Program, SimError, Snapshot};

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
        }
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    Assemble {
        architecture: Architecture,
        source: String,
    },
    MarkStale(String),
    LoadProgram(Program),
    Restore {
        program: Program,
        snapshot: Option<Snapshot>,
    },
    Step,
    Reverse,
    Reset,
    Run,
    Pause,
    ToggleBreakpoint(u64),
    SetBreakpoints(BTreeSet<u64>),
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
        }
    }

    pub fn view(&self) -> &SessionView {
        &self.view
    }

    fn mark_assembling(&mut self) {
        self.view.state = RunState::Assembling;
        self.view.dirty = true;
        self.view.diagnostics.clear();
        self.view.message = "Assembling source.".into();
        self.view.revision += 1;
    }

    pub fn assemble(&mut self, architecture: Architecture, source: String) -> Result<(), SimError> {
        if self.view.architecture != architecture {
            self.view.breakpoints.clear();
            self.view.watchpoints.clear();
            self.view.program = None;
            self.view.snapshot = None;
            self.view.current_instruction = None;
            self.view.disassembly.clear();
            self.machine = None;
        }
        self.view.architecture = architecture;
        self.view.source = source.clone();
        self.view.state = RunState::Assembling;
        self.view.dirty = true;
        self.view.diagnostics.clear();
        let assembled = if source.len() > 1024 * 1024 {
            Err(vec![Diagnostic::new(1, "Source exceeds the 1 MiB limit.")])
        } else {
            match architecture {
                Architecture::Vole => vole_isa_vole::assemble(&source),
                _ => vole_isa_scalar::assemble(architecture, &source),
            }
        };
        let result = assembled.and_then(|program| {
            self.load(program).map_err(|error| {
                vec![Diagnostic::new(
                    1,
                    format!("Executable could not be loaded: {error}"),
                )]
            })
        });
        match result {
            Ok(()) => Ok(()),
            Err(diagnostics) => {
                let message = diagnostics
                    .first()
                    .map(|d| format!("Line {}: {}", d.line, d.message))
                    .unwrap_or_else(|| "Assembly failed.".into());
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
        let mut machine: Box<dyn Machine> = match program.architecture {
            Architecture::Vole => Box::new(vole_isa_vole::VoleMachine::new()),
            architecture => Box::new(vole_isa_scalar::ScalarMachine::new(architecture)),
        };
        machine.load(&program)?;
        if self.view.architecture != program.architecture {
            self.view.breakpoints.clear();
            self.view.watchpoints.clear();
        }
        self.view.architecture = program.architecture;
        self.view.source = program.source.clone();
        self.view.program = Some(program);
        self.view.state = RunState::Ready;
        self.view.message = "Ready to execute.".into();
        self.view.diagnostics.clear();
        self.view.selected_address = None;
        self.view.dirty = false;
        self.bypass_breakpoint = None;
        self.stopped_breakpoint = None;
        self.machine = Some(machine);
        self.code_bytes.clear();
        self.refresh();
        Ok(())
    }

    pub fn apply(&mut self, command: Command) -> Result<(), SimError> {
        let result = (|| match command {
            Command::Assemble {
                architecture,
                source,
            } => self.assemble(architecture, source),
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
                    staged.refresh();
                }
                if self.view.architecture == staged.view.architecture {
                    staged.view.breakpoints = self.view.breakpoints.clone();
                    staged.view.watchpoints = self.view.watchpoints.clone();
                }
                staged.view.revision = self.view.revision + 1;
                *self = staged;
                Ok(())
            }
            Command::MarkStale(source) => {
                self.view.source = source;
                self.view.dirty = true;
                self.view.state = RunState::Editing;
                self.view.message = "Source changed. Assemble before running.".into();
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
                self.view.state = RunState::Running;
                self.view.message = "Executing instructions.".into();
                Ok(())
            }
            Command::Pause => {
                if self.view.state == RunState::Running {
                    self.view.state = RunState::Paused;
                    self.view.message = "Paused by you.".into();
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
    pub fn run_batch(&mut self, max_steps: usize) {
        let started = Instant::now();
        for _ in 0..max_steps.min(RUN_BATCH) {
            if self.view.state != RunState::Running {
                break;
            }
            let pc = match &self.view.snapshot {
                Some(snapshot) => snapshot.pc,
                None => break,
            };
            if self.view.breakpoints.contains(&pc) && self.bypass_breakpoint != Some(pc) {
                self.view.state = RunState::Paused;
                self.view.message = format!("Paused before breakpoint at {pc:X}.");
                self.stopped_breakpoint = Some(pc);
                break;
            }
            self.bypass_breakpoint = None;
            if self.remaining_budget == 0 {
                self.view.state = RunState::Paused;
                self.view.message = "Instruction budget reached. Run again to continue.".into();
                break;
            }
            if self.step().is_err() {
                break;
            }
            self.remaining_budget -= 1;
            if self.view.state == RunState::Halted {
                break;
            }
            let watched = self
                .view
                .snapshot
                .as_ref()
                .and_then(|s| s.trace.last())
                .is_some_and(|record| {
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
                                ranges_overlap(
                                    watch.address,
                                    watch.length,
                                    access.address,
                                    access.length,
                                )
                            })
                        }
                    })
                });
            if watched {
                self.view.state = RunState::Paused;
                self.view.message = format!("Watchpoint accessed by instruction at {pc:X}.");
                break;
            }
            self.view.state = RunState::Running;
            if started.elapsed() >= Duration::from_millis(8) {
                break;
            }
        }
        self.view.revision += 1;
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
                "Assemble the current source before executing.".into(),
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
            self.view.snapshot = Some(snapshot);
            self.view.revision += 1;
        }
    }
}

fn no_machine() -> SimError {
    SimError("Assemble or load a program first.".into())
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
                session.mark_assembling();
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
                            if matches!(&command, Command::Assemble { .. }) {
                                session.mark_assembling();
                                publish(&worker_view, &session);
                            }
                            let _ = session.apply(command);
                            publish(&worker_view, &session);
                            last_publish = Instant::now();
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    if session.view.state == RunState::Running {
                        session.run_batch(RUN_BATCH);
                        if session.view.state != RunState::Running
                            || last_publish.elapsed() >= Duration::from_millis(16)
                        {
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
