#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use gpui::{prelude::*, *};
use gpui_kit::component::{
    Disableable, Sizable,
    button::{Button, ButtonVariants},
    input::{
        Editor, EditorState, Input, InputEvent, InputState, Position, RangeDecoration,
        RangeDecorationCollection, RangeDecorationStyle, TextDecoration, TextDecorationCollection,
    },
    resizable::{ResizableState, h_resizable, resizable_panel, v_resizable},
};
use std::{
    borrow::Cow,
    collections::BTreeSet,
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use vole_core::{Architecture, Instruction};
use vole_project::Project;
use vole_runtime::{Command, RunState, Runtime, SessionView};

mod inspect;
mod prompts;
mod theme;
use theme::*;

actions!(
    vole,
    [
        Assemble,
        RunPause,
        Step,
        ReverseStep,
        Reset,
        Open,
        Save,
        SaveAs,
        LoadExample,
        ToggleBreakpoint,
        ExportBytes,
        ShowHelp,
        ZoomIn,
        ZoomOut,
        Fullscreen,
        Quit,
        MemoryLeft,
        MemoryRight,
        MemoryUp,
        MemoryDown,
        CopyValue
    ]
);

#[derive(Clone, Copy, PartialEq)]
enum CompactTab {
    Assembly,
    Instructions,
    Memory,
    Registers,
    Trace,
}
#[derive(Clone)]
enum DiscardOperation {
    Open,
    Example,
    Close,
    Quit,
}

#[derive(Clone)]
enum EditTarget {
    Memory(u64),
    Register(String, u8),
}

struct DesktopSession {
    initial_architecture: Architecture,
    demo_steps: u8,
    workbench: Option<Entity<Workbench>>,
}
impl Global for DesktopSession {}

struct Workbench {
    runtime: Runtime,
    view: Arc<SessionView>,
    editor: Entity<EditorState>,
    address_input: Entity<InputState>,
    value_input: Entity<InputState>,
    pc_decoration: RangeDecorationCollection,
    breakpoint_decoration: RangeDecorationCollection,
    syntax_decoration: TextDecorationCollection,
    last_source: String,
    last_window_title: String,
    focus: FocusHandle,
    memory_focus: FocusHandle,
    split_main: Entity<ResizableState>,
    split_source: Entity<ResizableState>,
    split_vertical: Entity<ResizableState>,
    pending_layout: Option<vole_project::Layout>,
    instruction_scroll: UniformListScrollHandle,
    architecture: Architecture,
    selected: u64,
    memory_base: u64,
    edit_target: EditTarget,
    edit_width: usize,
    little_endian: bool,
    architecture_menu: bool,
    compact_tab: CompactTab,
    file_path: Option<PathBuf>,
    saved_source: String,
    edit_error: Option<String>,
    zoom: f32,
    last_editor_line: u32,
    demo_remaining: u8,
    _subscriptions: Vec<Subscription>,
    _poll: Task<()>,
}

impl Workbench {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let architecture = cx.global::<DesktopSession>().initial_architecture;
        let demo_remaining = cx.global::<DesktopSession>().demo_steps;
        let source = architecture.example_source().to_string();
        let runtime = Runtime::new(architecture, source.clone());
        let view = runtime.view();
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("text")
                .line_number(true)
                .soft_wrap(false)
                .default_value(source.clone())
                .placeholder("Write assembly, then choose Assemble.")
        });
        let address_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value("00")
                .placeholder("Hex address")
        });
        let value_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value("00")
                .placeholder("Hex value")
        });
        let pc_decoration = editor.update(cx, |state, cx| {
            state.create_range_decorations_collection(Vec::new(), cx)
        });
        let breakpoint_decoration = editor.update(cx, |state, cx| {
            state.create_range_decorations_collection(Vec::new(), cx)
        });
        let syntax_decoration = editor.update(cx, |state, cx| {
            state.create_decorations_collection(syntax_tokens(&source), cx)
        });
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(33))
                    .await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            runtime,
            view,
            editor,
            address_input,
            value_input,
            pc_decoration,
            breakpoint_decoration,
            syntax_decoration,
            last_source: source.clone(),
            last_window_title: String::new(),
            focus: cx.focus_handle(),
            memory_focus: cx.focus_handle(),
            split_main: cx.new(|_| ResizableState::default()),
            split_source: cx.new(|_| ResizableState::default()),
            split_vertical: cx.new(|_| ResizableState::default()),
            pending_layout: None,
            instruction_scroll: UniformListScrollHandle::new(),
            architecture,
            selected: if architecture == Architecture::Vole {
                0
            } else {
                0x1000
            },
            memory_base: if architecture == Architecture::Vole {
                0
            } else {
                0x1000
            },
            edit_target: EditTarget::Memory(if architecture == Architecture::Vole {
                0
            } else {
                0x1000
            }),
            edit_width: 1,
            little_endian: true,
            architecture_menu: false,
            compact_tab: CompactTab::Assembly,
            file_path: None,
            saved_source: source,
            edit_error: None,
            zoom: 1.0,
            last_editor_line: 0,
            demo_remaining,
            _subscriptions: Vec::new(),
            _poll: poll,
        }
    }

    fn bind_window_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.last_window_title.clear();
        self._subscriptions = vec![
            cx.subscribe(&self.editor, |this, editor, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    let source = editor.read(cx).value().to_string();
                    if source != this.last_source {
                        this.last_source = source.clone();
                        this.syntax_decoration.set(syntax_tokens(&source), cx);
                        this.send(Command::MarkStale(source), cx);
                    }
                }
            }),
            cx.subscribe_in(
                &self.address_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.go_to_address(window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &self.value_input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.apply_edit(window, cx);
                    }
                },
            ),
        ];
    }

    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        if let Err(error) = self.runtime.send(command) {
            self.edit_error = Some(error.to_string());
        }
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let next = self.runtime.view();
        if next.revision != self.view.revision {
            let old_pc = self.view.snapshot.as_ref().map(|s| s.pc);
            let new_pc = next.snapshot.as_ref().map(|s| s.pc);
            self.view = next;
            self.architecture = self.view.architecture;
            self.update_source_decorations(cx);
            let input = self.value_input.clone();
            let text = self.edit_value_text();
            if let Some(handle) = cx.windows().first().copied() {
                let _ = handle.update(cx, |_, window, cx| {
                    if !input.focus_handle(cx).is_focused(window) {
                        input.update(cx, |input, cx| input.set_value(text, window, cx));
                    }
                });
            }
            if old_pc != new_pc
                && let Some(pc) = new_pc
                && let Some(index) = self.view.disassembly.iter().position(|i| i.address == pc)
            {
                self.instruction_scroll
                    .scroll_to_item(index, ScrollStrategy::Center);
            }
            if self.demo_remaining > 0 && self.can_execute() {
                self.demo_remaining -= 1;
                self.send(Command::Step, cx);
            }
            cx.notify();
        }
        let cursor_line = self.editor.read(cx).cursor_position().line;
        if cursor_line != self.last_editor_line {
            self.last_editor_line = cursor_line;
            if let Some(instruction) = self
                .view
                .disassembly
                .iter()
                .find(|i| i.source_line == Some(cursor_line as usize + 1))
            {
                let address = instruction.address;
                self.selected = address;
                cx.notify();
            }
        }
        let name = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("Untitled.{}", self.architecture.id()));
        let changed = self.editor.read(cx).value().as_ref() != self.saved_source;
        let title = format!("{}{} | Vole", name, if changed { " *" } else { "" });
        if title != self.last_window_title
            && let Some(handle) = cx.windows().first().copied()
        {
            let _ = handle.update(cx, |_, window, _| {
                window.set_window_title(&title);
                window.set_window_edited(changed);
            });
            self.last_window_title = title;
        }
    }

    fn update_source_decorations(&mut self, cx: &mut Context<Self>) {
        let source = self.editor.read(cx).value().to_string();
        let line_ranges: Vec<_> = source
            .split_inclusive('\n')
            .scan(0, |offset, line| {
                let start = *offset;
                *offset += line.len();
                Some(start..(*offset).saturating_sub(usize::from(line.ends_with('\n'))))
            })
            .collect();
        let decoration = |line: usize, color: Hsla| {
            line_ranges
                .get(line.saturating_sub(1))
                .filter(|r| !r.is_empty())
                .map(|range| {
                    RangeDecoration::new(range.clone())
                        .with_style(RangeDecorationStyle::Fill)
                        .with_color(color)
                })
        };
        let current = self
            .view
            .current_instruction
            .as_ref()
            .and_then(|i| i.source_line)
            .and_then(|l| decoration(l, rgba(0x9cc8f427).into()))
            .into_iter()
            .collect();
        self.pc_decoration.set(current, cx);
        let breakpoints = self
            .view
            .disassembly
            .iter()
            .filter(|i| self.view.breakpoints.contains(&i.address))
            .filter_map(|i| {
                i.source_line
                    .and_then(|line| decoration(line, rgba(0xe4b48b24).into()))
            })
            .collect();
        self.breakpoint_decoration.set(breakpoints, cx);
    }

    fn can_execute(&self) -> bool {
        self.view.snapshot.is_some()
            && !matches!(
                self.view.state,
                RunState::Editing
                    | RunState::Assembling
                    | RunState::Running
                    | RunState::Halted
                    | RunState::Faulted
            )
    }
    fn paused(&self) -> bool {
        self.view.snapshot.is_some()
            && !matches!(
                self.view.state,
                RunState::Running | RunState::Assembling | RunState::Editing
            )
    }
    fn source(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }
    fn assemble(&mut self, _: &Assemble, _: &mut Window, cx: &mut Context<Self>) {
        self.edit_error = None;
        self.send(
            Command::Assemble {
                architecture: self.architecture,
                source: self.source(cx),
            },
            cx,
        );
    }
    fn run_pause(&mut self, _: &RunPause, _: &mut Window, cx: &mut Context<Self>) {
        if self.view.state == RunState::Running {
            self.send(Command::Pause, cx);
        } else if self.can_execute() {
            self.send(Command::Run, cx);
        }
    }
    fn step(&mut self, _: &Step, _: &mut Window, cx: &mut Context<Self>) {
        if self.can_execute() {
            self.send(Command::Step, cx);
        }
    }
    fn reverse(&mut self, _: &ReverseStep, _: &mut Window, cx: &mut Context<Self>) {
        if self.paused() {
            self.send(Command::Reverse, cx);
        }
    }
    fn reset(&mut self, _: &Reset, _: &mut Window, cx: &mut Context<Self>) {
        if self.view.program.is_some() {
            self.send(Command::Reset, cx);
        }
    }
    fn toggle_breakpoint(&mut self, _: &ToggleBreakpoint, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.editor.read(cx).cursor_position().line as usize + 1;
        let address = self
            .view
            .disassembly
            .iter()
            .find(|instruction| instruction.source_line == Some(line))
            .map(|instruction| instruction.address)
            .unwrap_or(self.selected);
        self.send(Command::ToggleBreakpoint(address), cx);
    }
    fn load_example(&mut self, _: &LoadExample, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_discard(DiscardOperation::Example, window, cx) {
            return;
        }
        self.replace_with_example(window, cx);
    }
    fn replace_with_example(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.architecture.example_source().to_string();
        self.last_source = source.clone();
        self.editor.update(cx, |editor, cx| {
            editor.set_value(source.clone(), window, cx)
        });
        self.syntax_decoration.set(syntax_tokens(&source), cx);
        self.file_path = None;
        self.saved_source = source.clone();
        self.selected = if self.architecture == Architecture::Vole {
            0
        } else {
            0x1000
        };
        self.memory_base = self.selected;
        self.send(Command::SetBreakpoints(BTreeSet::new()), cx);
        self.send(
            Command::Assemble {
                architecture: self.architecture,
                source,
            },
            cx,
        );
    }
    fn choose_architecture(
        &mut self,
        architecture: Architecture,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if architecture == self.architecture {
            self.architecture_menu = false;
            cx.notify();
            return;
        }
        self.architecture = architecture;
        self.architecture_menu = false;
        // Keep source when changing a target. The user chooses whether to load its example.
        self.send(
            Command::Assemble {
                architecture,
                source: self.source(cx),
            },
            cx,
        );
        self.memory_base = if architecture == Architecture::Vole {
            0
        } else {
            0x1000
        };
        self.selected = self.memory_base;
        self.address_input.update(cx, |input, cx| {
            input.set_value(
                format!(
                    "{:0width$X}",
                    self.memory_base,
                    width = architecture.address_digits()
                ),
                window,
                cx,
            )
        });
    }
    fn select_instruction(&mut self, address: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = address;
        self.memory_base = address & !0xff;
        self.edit_target = EditTarget::Memory(address);
        self.set_edit_value(window, cx);
        if let Some(line) = self
            .view
            .disassembly
            .iter()
            .find(|i| i.address == address)
            .and_then(|i| i.source_line)
        {
            self.editor.update(cx, |state, cx| {
                state.set_cursor_position(
                    Position::new(line.saturating_sub(1) as u32, 0),
                    window,
                    cx,
                )
            });
        }
        cx.notify();
    }
    fn select_memory(&mut self, address: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = address;
        self.edit_target = EditTarget::Memory(address);
        self.edit_width = 1;
        self.edit_error = None;
        if let Some(line) = self
            .view
            .disassembly
            .iter()
            .find(|instruction| {
                address >= instruction.address
                    && address
                        < instruction
                            .address
                            .saturating_add(instruction.bytes.len() as u64)
            })
            .and_then(|i| i.source_line)
        {
            self.last_editor_line = line.saturating_sub(1) as u32;
            self.editor.update(cx, |editor, cx| {
                editor.set_cursor_position(
                    Position::new(line.saturating_sub(1) as u32, 0),
                    window,
                    cx,
                )
            });
        }
        self.memory_focus.focus(window, cx);
        self.set_edit_value(window, cx);
        cx.notify();
    }
    fn edit_value_text(&self) -> String {
        match &self.edit_target {
            EditTarget::Memory(address) => self
                .view
                .snapshot
                .as_ref()
                .and_then(|s| s.read(*address, self.edit_width))
                .map(|bytes| {
                    let mut bytes = bytes;
                    if self.little_endian {
                        bytes.reverse();
                    }
                    bytes.iter().map(|b| format!("{b:02X}")).collect::<String>()
                })
                .unwrap_or_default(),
            EditTarget::Register(name, bits) => self
                .view
                .snapshot
                .as_ref()
                .and_then(|s| {
                    if name.eq_ignore_ascii_case("PC") {
                        Some(s.pc)
                    } else {
                        s.register(name)
                    }
                })
                .map(|v| format!("{v:0width$X}", width = usize::from(*bits).div_ceil(4)))
                .unwrap_or_default(),
        }
    }
    fn set_edit_value(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.edit_value_text();
        self.value_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
    }
    fn valid_address(&self, address: u64) -> bool {
        match self.architecture.bits() {
            8 => address <= 255,
            32 => address <= u32::MAX as u64,
            _ => true,
        }
    }
    fn go_to_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match inspect::parse_hex(&self.address_input.read(cx).value()) {
            Ok(address) if !self.valid_address(address) => {
                self.edit_error = Some(format!(
                    "Address exceeds this target's {}-bit address space.",
                    self.architecture.bits()
                ));
                cx.notify();
            }
            Ok(address) => {
                self.memory_base = address & !0xff;
                self.select_memory(address, window, cx);
            }
            Err(message) => {
                self.edit_error = Some(message);
                cx.notify();
            }
        }
    }
    fn apply_edit(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if !self.paused() {
            self.edit_error =
                Some("Pause the machine and assemble current source before editing.".into());
            cx.notify();
            return;
        }
        let value = match inspect::parse_hex(&self.value_input.read(cx).value()) {
            Ok(value) => value,
            Err(message) => {
                self.edit_error = Some(message);
                cx.notify();
                return;
            }
        };
        let command = match &self.edit_target {
            EditTarget::Memory(address) => {
                if self.edit_width < 8 && value >= (1u64 << (self.edit_width * 8)) {
                    self.edit_error =
                        Some(format!("Value does not fit in {} bytes.", self.edit_width));
                    cx.notify();
                    return;
                }
                let all = if self.little_endian {
                    value.to_le_bytes()
                } else {
                    value.to_be_bytes()
                };
                let bytes = if self.little_endian {
                    all[..self.edit_width].to_vec()
                } else {
                    all[8 - self.edit_width..].to_vec()
                };
                Command::EditMemory {
                    address: *address,
                    bytes,
                }
            }
            EditTarget::Register(name, bits) => {
                if *bits < 64 && value >= (1u64 << *bits) {
                    self.edit_error =
                        Some(format!("Value does not fit in this {bits}-bit register."));
                    cx.notify();
                    return;
                }
                Command::EditRegister {
                    name: name.clone(),
                    value,
                }
            }
        };
        self.edit_error = None;
        self.send(command, cx);
    }
    fn confirm_discard(
        &mut self,
        operation: DiscardOperation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if window.has_active_prompt() {
            return true;
        }
        if self.source(cx) == self.saved_source {
            return false;
        }
        let confirm = window.prompt(
            PromptLevel::Warning,
            "Discard unsaved assembly changes?",
            Some("Save your current document first if you want to keep these changes."),
            &["Cancel", "Discard changes"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if confirm.await == Ok(1) {
                let _ = this.update_in(cx, |this, window, cx| match operation {
                    DiscardOperation::Open => this.open_file(cx),
                    DiscardOperation::Example => this.replace_with_example(window, cx),
                    DiscardOperation::Close => window.remove_window(),
                    DiscardOperation::Quit => cx.quit(),
                });
            }
        })
        .detach();
        true
    }
    fn quit(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
        if !self.confirm_discard(DiscardOperation::Quit, window, cx) {
            cx.quit();
        }
    }
    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        if self.view.state == RunState::Running {
            self.send(Command::Pause, cx);
        }
        if self.confirm_discard(DiscardOperation::Open, window, cx) {
            return;
        }
        self.open_file(cx);
    }
    fn open_file(&mut self, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open assembly, .voleproject, .bin or .hex".into()),
        });
        let architecture = self.architecture;
        cx.spawn(async move|this,cx|{
            let path=match picker.await{Ok(Ok(Some(mut paths)))=>paths.pop(),Ok(Ok(None))=>None,other=>{let _=this.update(cx,|this,cx|{this.edit_error=Some(format!("Native file picker failed: {other:?}"));cx.notify();});None}};
            let Some(path)=path else{return;};
            let loaded=cx.background_executor().spawn(async move{
                let extension=path.extension().and_then(|e|e.to_str()).unwrap_or("").to_ascii_lowercase();
                if extension=="voleproject"||extension=="json"{
                    let p=Project::open(&path)?;
                    if let Some(image)=p.image.as_ref(){
                        let mut validation=vole_runtime::Session::new(p.architecture,p.source.clone());
                        validation.apply(Command::Restore{program:image.clone(),snapshot:p.snapshot.clone()})?;
                    }
                    return Ok((Some(path),p.architecture,p.source,p.breakpoints,Some(p.layout),p.image,p.snapshot));
                }
                let file=std::fs::File::open(&path).map_err(|e|vole_core::SimError(e.to_string()))?;
                let mut bytes=Vec::new();file.take(1024*1024+1).read_to_end(&mut bytes).map_err(|e|vole_core::SimError(e.to_string()))?;
                if bytes.len()>1024*1024{return Err(vole_core::SimError("File exceeds the 1 MiB limit.".into()));}
                if extension=="bin"||extension=="prg"||extension=="hex"{
                    let image=if extension=="hex"{
                        let text=String::from_utf8(bytes).map_err(|_|vole_core::SimError("Hex file must be UTF-8 text with pairs of hexadecimal digits.".into()))?;
                        let compact=text.split_whitespace().collect::<String>();
                        if compact.len()%2!=0||!compact.is_ascii(){return Err(vole_core::SimError("Hex file must contain complete byte pairs.".into()));}
                        (0..compact.len()).step_by(2).map(|i|u8::from_str_radix(&compact[i..i+2],16).map_err(|_|vole_core::SimError("Invalid hexadecimal byte.".into()))).collect::<Result<Vec<_>,_>>()?
                    }else{bytes};
                    let program=vole_runtime::import_bytes(architecture,&image)?;
                    Ok((None,architecture,program.source.clone(),BTreeSet::new(),None,Some(program),None))
                }else{
                    let source=String::from_utf8(bytes).map_err(|_|vole_core::SimError("Assembly files must contain UTF-8 text. Open binary images with a .bin extension.".into()))?;
                    Ok((Some(path),architecture,source,BTreeSet::new(),None,None,None))
                }
            }).await;
            let _=this.update(cx,|this,cx|{
                match loaded{
                    Ok((path,architecture,source,breakpoints,layout,program,snapshot))=>{
                        this.architecture=architecture;this.saved_source=source.clone();this.last_source=source.clone();this.file_path=path;this.edit_error=None;
                        this.memory_base=program.as_ref().map(|p|p.entry&!0xff).unwrap_or(if architecture==Architecture::Vole{0}else{0x1000});this.selected=this.memory_base;
                        let editor=this.editor.clone();let replacement=source.clone();
                        this.pending_layout=layout;
                        if let Some(handle)=cx.windows().first().copied(){let _=handle.update(cx,|_,window,cx|{
                            editor.update(cx,|state,cx|state.set_value(replacement,window,cx));
                        });}
                        this.syntax_decoration.set(syntax_tokens(&source),cx);
                        if let Some(program)=program{this.send(Command::Restore{program,snapshot},cx);}else{this.send(Command::Assemble{architecture,source},cx);}
                        this.send(Command::SetBreakpoints(breakpoints),cx);
                    }
                    Err(error)=>this.edit_error=Some(format!("Could not open file: {error}")),
                }cx.notify();
            });
        }).detach();
    }
    fn export_bytes(&mut self, _: &ExportBytes, _: &mut Window, cx: &mut Context<Self>) {
        let Some(program) = self.view.program.clone() else {
            self.edit_error = Some("Assemble a program before exporting bytes.".into());
            cx.notify();
            return;
        };
        let picker = cx.prompt_for_new_path(
            &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            Some("program.bin"),
        );
        cx.spawn(async move |this, cx| {
            let path = match picker.await {
                Ok(Ok(path)) => path,
                other => {
                    let _ = this.update(cx, |this, cx| {
                        this.edit_error = Some(format!("Export dialog failed: {other:?}"));
                        cx.notify();
                    });
                    None
                }
            };
            let Some(path) = path else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move {
                    let bytes = vole_runtime::export_bytes(&program)?;
                    atomic_write(&path, &bytes)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.edit_error = result.err().map(|e| format!("Export failed: {e}"));
                cx.notify();
            });
        })
        .detach();
    }
    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        self.save_document(false, window, cx);
    }
    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        self.save_document(true, window, cx);
    }
    fn save_document(&mut self, force_picker: bool, _: &mut Window, cx: &mut Context<Self>) {
        let mut project = Project::new(self.architecture, self.source(cx));
        if self.view.architecture == self.architecture {
            project.breakpoints = self.view.breakpoints.clone();
        }
        if !self.view.dirty
            && self.view.source == project.source
            && self
                .view
                .program
                .as_ref()
                .is_some_and(|image| image.source == project.source)
        {
            project.image = self.view.program.clone();
            project.snapshot = self.view.snapshot.clone();
            if let Some(snapshot) = project.snapshot.as_mut() {
                snapshot.trace.clear();
            }
        }
        if let Some(layout) = self.pending_layout.as_ref() {
            project.layout = layout.clone();
        } else {
            for (state, destination) in [
                (&self.split_source, &mut project.layout.source_fraction),
                (&self.split_vertical, &mut project.layout.top_fraction),
            ] {
                let sizes = state.read(cx).sizes();
                if sizes.len() >= 2 {
                    let total = f32::from(sizes[0] + sizes[1]);
                    if total > 0. {
                        *destination = (f32::from(sizes[0]) / total).clamp(0.15, 0.85);
                    }
                }
            }
            if let Some(width) = self.split_main.read(cx).sizes().get(1) {
                project.layout.inspector_width = f32::from(*width).clamp(180., 600.);
            }
        }
        let existing = if force_picker {
            None
        } else {
            self.file_path.clone()
        };
        let picker = if existing.is_none() {
            Some(cx.prompt_for_new_path(
                &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
                Some("program.voleproject"),
            ))
        } else {
            None
        };
        cx.spawn(async move |this, cx| {
            let path = if let Some(path) = existing {
                Some(path)
            } else if let Some(picker) = picker {
                match picker.await {
                    Ok(Ok(path)) => path,
                    other => {
                        let _ = this.update(cx, |this, cx| {
                            this.edit_error =
                                Some(format!("Could not open save dialog: {other:?}"));
                            cx.notify();
                        });
                        None
                    }
                }
            } else {
                None
            };
            let Some(path) = path else {
                return;
            };
            let source = project.source.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    let result = if path.extension().is_some_and(|e| {
                        e.to_string_lossy().eq_ignore_ascii_case("voleproject")
                            || e.to_string_lossy().eq_ignore_ascii_case("json")
                    }) {
                        project.save(&path)
                    } else {
                        atomic_write(&path, project.source.as_bytes())
                    };
                    (path, result)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    (path, Ok(())) => {
                        this.file_path = Some(path);
                        this.saved_source = source;
                        this.edit_error = None;
                    }
                    (_, Err(error)) => {
                        this.edit_error = Some(format!("Could not save file: {error}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn show_help(&mut self, _: &ShowHelp, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_prompt() {
            return;
        }
        let detail="Assemble: Ctrl/Command+Enter. Run or pause: F5. Step: F10. Reverse: Shift+F10. Breakpoint at source line: F9.

Select an instruction to jump to its source and memory. Select a memory byte or register, enter a hex value, then Apply. Editing clears undo history. Arrow keys move the memory selection.

VOLE uses the SimpSim extended profile. ARM and x86 currently execute the supported scalar teaching subset. Unsupported instructions stop with a fault. Simulated instructions are counted, not hardware cycles.

Save .voleproject documents to preserve source, target, breakpoints, pane sizes and current machine state. Open .bin/.hex to inspect machine bytes. Export writes the assembled image.";
        drop(window.prompt(
            PromptLevel::Info,
            "Using Vole",
            Some(detail),
            &["Close"],
            cx,
        ));
    }
    fn zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom = (self.zoom + 0.1).min(2.0);
        cx.notify();
    }
    fn zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        self.zoom = (self.zoom - 0.1).max(0.8);
        cx.notify();
    }
    fn fullscreen(&mut self, _: &Fullscreen, window: &mut Window, _: &mut Context<Self>) {
        window.toggle_fullscreen();
    }
    fn move_memory(&mut self, delta: i64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(address) = self
            .selected
            .checked_add_signed(delta)
            .filter(|address| self.valid_address(*address))
        {
            self.memory_base = address & !0xff;
            self.select_memory(address, window, cx);
        }
    }
    fn memory_left(&mut self, _: &MemoryLeft, w: &mut Window, cx: &mut Context<Self>) {
        self.move_memory(-1, w, cx);
    }
    fn memory_right(&mut self, _: &MemoryRight, w: &mut Window, cx: &mut Context<Self>) {
        self.move_memory(1, w, cx);
    }
    fn memory_up(&mut self, _: &MemoryUp, w: &mut Window, cx: &mut Context<Self>) {
        self.move_memory(-16, w, cx);
    }
    fn memory_down(&mut self, _: &MemoryDown, w: &mut Window, cx: &mut Context<Self>) {
        self.move_memory(16, w, cx);
    }
    fn copy_value(&mut self, _: &CopyValue, _: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.value_input.read(cx).value().to_string(),
        ));
    }
}

impl Workbench {
    fn toolbar(&self, compact: bool, cx: &mut Context<Self>) -> Div {
        let running = self.view.state == RunState::Running;
        let reverse_disabled = !self.paused()
            || self
                .view
                .snapshot
                .as_ref()
                .is_none_or(|s| s.trace.is_empty());
        div()
            .w_full()
            .min_w_0()
            .flex_none()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(8.))
            .px(px(20.))
            .py(px(12.))
            .border_b_1()
            .border_color(rgb(DIVIDER))
            .bg(rgb(WORKSPACE))
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(px(20.))
                    .mr(px(8.))
                    .child("Vole"),
            )
            .child(
                Button::new("target")
                    .label(self.architecture.name())
                    .w(px(if compact { 160. } else { 205. }))
                    .tooltip("Choose instruction set and register width")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.architecture_menu = !this.architecture_menu;
                        cx.notify();
                    })),
            )
            .when(!compact, |row| {
                row.child(
                    note(if self.architecture == Architecture::Vole {
                        "SimpSim extended"
                    } else {
                        "Scalar teaching subset"
                    })
                    .mr(px(12.)),
                )
            })
            .child(
                Button::new("assemble")
                    .label("Assemble")
                    .tooltip("Assemble source (Ctrl/Command+Enter)")
                    .disabled(matches!(
                        self.view.state,
                        RunState::Running | RunState::Assembling
                    ))
                    .on_click(cx.listener(|this, _, w, cx| this.assemble(&Assemble, w, cx))),
            )
            .child(
                Button::new("run")
                    .primary()
                    .label(if running { "Pause" } else { "Run" })
                    .tooltip("Run / Pause (F5)")
                    .disabled(!running && !self.can_execute())
                    .on_click(cx.listener(|this, _, w, cx| this.run_pause(&RunPause, w, cx))),
            )
            .child(
                Button::new("step")
                    .label("Step")
                    .tooltip("Step one instruction (F10)")
                    .disabled(!self.can_execute())
                    .on_click(cx.listener(|this, _, w, cx| this.step(&Step, w, cx))),
            )
            .child(
                Button::new("reverse")
                    .label(if compact { "Reverse" } else { "Reverse step" })
                    .tooltip("Reverse step (Shift+F10)")
                    .disabled(reverse_disabled)
                    .on_click(cx.listener(|this, _, w, cx| this.reverse(&ReverseStep, w, cx))),
            )
            .child(
                Button::new("reset")
                    .label("Reset")
                    .disabled(self.view.program.is_none() || running)
                    .tooltip("Reset to assembled image (Ctrl/Command+R)")
                    .on_click(cx.listener(|this, _, w, cx| this.reset(&Reset, w, cx))),
            )
            .child(div().flex_1())
            .child(
                Button::new("open")
                    .ghost()
                    .label("Open")
                    .tooltip("Open assembly or project (Ctrl/Command+O)")
                    .on_click(cx.listener(|this, _, w, cx| this.open(&Open, w, cx))),
            )
            .child(
                Button::new("export")
                    .ghost()
                    .label("Export")
                    .tooltip("Export machine bytes (Ctrl/Command+Shift+E)")
                    .disabled(self.view.program.is_none() || self.view.dirty)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.export_bytes(&ExportBytes, window, cx)
                    })),
            )
            .child(
                Button::new("save")
                    .ghost()
                    .label("Save")
                    .tooltip("Save assembly or project (Ctrl/Command+S)")
                    .on_click(cx.listener(|this, _, w, cx| this.save(&Save, w, cx))),
            )
    }

    fn source_panel(&self, cx: &mut Context<Self>) -> Div {
        let byte_count = self
            .view
            .program
            .as_ref()
            .map(|p| {
                p.regions
                    .iter()
                    .filter(|region| region.executable)
                    .map(|region| region.bytes.len())
                    .sum::<usize>()
            })
            .unwrap_or(0);
        let stale = self.view.state == RunState::Editing;
        let name = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("Untitled.{}", self.architecture.id()));
        pane()
            .child(heading("Assembly", name))
            .child(
                div().min_h_0().flex_1().overflow_hidden().child(
                    Editor::new(&self.editor)
                        .aria_label("Assembly source editor")
                        .bordered(false)
                        .readonly(self.view.state == RunState::Running)
                        .p(px(12.))
                        .h(relative(1.))
                        .font_family(MONO)
                        .text_size(px(14. * self.zoom))
                        .bg(rgb(WORKSPACE)),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .px(px(20.))
                    .py(px(8.))
                    .child(note(if stale {
                        "Source changed. Assemble to run.".into()
                    } else {
                        format!("{byte_count} code bytes assembled")
                    }))
                    .child(
                        Button::new("example")
                            .ghost()
                            .small()
                            .label("Load example")
                            .disabled(self.view.state == RunState::Running)
                            .on_click(
                                cx.listener(|this, _, w, cx| {
                                    this.load_example(&LoadExample, w, cx)
                                }),
                            ),
                    ),
            )
            .when(!self.view.diagnostics.is_empty(), |panel| {
                panel.child(
                    div()
                        .id("diagnostics")
                        .max_h(px(110.))
                        .overflow_y_scroll()
                        .px(px(20.))
                        .pb(px(12.))
                        .children(self.view.diagnostics.iter().enumerate().map(|(index, d)| {
                            let line = d.line;
                            div()
                                .id(("diagnostic", index))
                                .text_size(px(12.))
                                .text_color(rgb(ERROR))
                                .py(px(4.))
                                .cursor_pointer()
                                .child(format!("Line {}: {}", d.line, d.message))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.editor.update(cx, |state, cx| {
                                        state.set_cursor_position(
                                            Position::new(line.saturating_sub(1) as u32, 0),
                                            window,
                                            cx,
                                        )
                                    });
                                }))
                        })),
                )
            })
    }

    fn instructions_panel(&self, cx: &mut Context<Self>) -> Div {
        let count = self.view.disassembly.len();
        pane()
            .child(heading(
                "Instructions",
                "Scroll · address / bytes / assembly",
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(20.))
                    .h(px(28.))
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child(div().w(px(22.)).child(""))
                    .child(
                        div()
                            .w(px(if self.architecture == Architecture::Vole {
                                48.
                            } else if self.architecture.bits() == 32 {
                                82.
                            } else {
                                154.
                            }))
                            .child("Address"),
                    )
                    .child(
                        div()
                            .w(px(if self.architecture == Architecture::Vole {
                                78.
                            } else {
                                130.
                            }))
                            .child("Bytes"),
                    )
                    .child("Assembly"),
            )
            .child(
                uniform_list(
                    "instruction-list",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .filter_map(|index| this.view.disassembly.get(index).cloned())
                            .map(|instruction| this.instruction_row(instruction, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.instruction_scroll)
                .flex_1()
                .min_h_0(),
            )
            .when(count == 0, |panel| {
                panel.child(
                    div()
                        .px(px(20.))
                        .py(px(20.))
                        .child(note("Assemble the source to see encoded instructions.")),
                )
            })
            .child(self.decode_strip())
    }
    fn instruction_row(&self, instruction: Instruction, cx: &mut Context<Self>) -> Stateful<Div> {
        let address = instruction.address;
        let pc = self.view.snapshot.as_ref().is_some_and(|s| s.pc == address);
        let selected = self.selected == address;
        let breakpoint = self.view.breakpoints.contains(&address);
        div()
            .id(("instruction", address))
            .w_full()
            .role(Role::Row)
            .aria_label(format!("Address {address:X}, {}", instruction.assembly))
            .flex()
            .items_center()
            .h(px(27. * self.zoom))
            .gap(px(12.))
            .px(px(20.))
            .text_size(px(13. * self.zoom))
            .font_family(MONO)
            .bg(rgb(if pc || selected { SURFACE } else { WORKSPACE }))
            .text_color(rgb(if pc { READ } else { INK }))
            .cursor_pointer()
            .child(
                Button::new(("breakpoint", address))
                    .ghost()
                    .small()
                    .w(px(22.))
                    .p_0()
                    .label(if breakpoint { "●" } else { "·" })
                    .tooltip(format!(
                        "{} breakpoint at {:X}",
                        if breakpoint { "Remove" } else { "Add" },
                        address
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send(Command::ToggleBreakpoint(address), cx)
                    })),
            )
            .child(
                div()
                    .w(px(if self.architecture == Architecture::Vole {
                        48.
                    } else if self.architecture.bits() == 32 {
                        82.
                    } else {
                        154.
                    }))
                    .flex_none()
                    .child(format!(
                        "{address:0width$X}",
                        width = self.architecture.address_digits()
                    )),
            )
            .child(
                div()
                    .w(px(if self.architecture == Architecture::Vole {
                        78.
                    } else {
                        130.
                    }))
                    .flex_none()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(
                        instruction
                            .bytes
                            .iter()
                            .map(|b| format!("{b:02X}"))
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(instruction.assembly),
            )
            .on_click(
                cx.listener(move |this, _, window, cx| {
                    this.select_instruction(address, window, cx)
                }),
            )
    }
    fn decode_strip(&self) -> Div {
        let Some(instruction) = self.view.current_instruction.as_ref() else {
            return div()
                .border_t_1()
                .border_color(rgb(DIVIDER))
                .p(px(14.))
                .child(note(if self.view.state == RunState::Halted {
                    "Machine halted. Reset or reverse step to continue."
                } else {
                    "Decoded instruction appears after assembly."
                }));
        };
        let mut strip = div()
            .flex()
            .flex_col()
            .flex_none()
            .border_t_1()
            .border_color(rgb(DIVIDER))
            .p(px(14.))
            .gap(px(8.))
            .child(note(if self.view.dirty {
                "Previous executable · source changed"
            } else if self.view.snapshot.as_ref().is_some_and(|s| s.halted) {
                "Last decoded instruction"
            } else {
                "Decoded instruction"
            }));
        if self.architecture == Architecture::Vole && instruction.bytes.len() == 2 {
            let word = u16::from_be_bytes([instruction.bytes[0], instruction.bytes[1]]);
            let opcode = word >> 12;
            let groups = match opcode {
                5..=9 => [
                    ("Operation", INK),
                    ("Write", WRITE),
                    ("Read", READ),
                    ("Read", READ),
                ],
                1 => [
                    ("Operation", INK),
                    ("Write", WRITE),
                    ("Address", READ),
                    ("Address", READ),
                ],
                2 => [
                    ("Operation", INK),
                    ("Write", WRITE),
                    ("Value", READ),
                    ("Value", READ),
                ],
                3 => [
                    ("Operation", INK),
                    ("Read", READ),
                    ("Address", WRITE),
                    ("Address", WRITE),
                ],
                4 => [
                    ("Operation", INK),
                    ("Unused", MUTED),
                    ("Read", READ),
                    ("Write", WRITE),
                ],
                10 => [
                    ("Operation", INK),
                    ("Rotate", WRITE),
                    ("Unused", MUTED),
                    ("Count", READ),
                ],
                11 | 15 => [
                    ("Operation", INK),
                    ("Compare", READ),
                    ("Target", READ),
                    ("Target", READ),
                ],
                12 => [
                    ("Halt", INK),
                    ("Unused", MUTED),
                    ("Unused", MUTED),
                    ("Unused", MUTED),
                ],
                13 => [
                    ("Operation", INK),
                    ("Unused", MUTED),
                    ("Write", WRITE),
                    ("Address", READ),
                ],
                14 => [
                    ("Operation", INK),
                    ("Unused", MUTED),
                    ("Read", READ),
                    ("Address", WRITE),
                ],
                _ => [
                    ("Operation", INK),
                    ("Operand", INK),
                    ("Operand", INK),
                    ("Operand", INK),
                ],
            };
            let nibbles = div()
                .flex()
                .items_start()
                .gap(px(10.))
                .children((0..4).map(|index| {
                    let nibble = (word >> (12 - index * 4)) & 15;
                    let color = groups[index].1;
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(5.))
                        .child(
                            div()
                                .w(px(44.))
                                .h(px(44.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(5.))
                                .border_1()
                                .border_color(rgb(DIVIDER))
                                .bg(rgb(SURFACE))
                                .font_family(MONO)
                                .text_size(px(22.))
                                .text_color(rgb(color))
                                .child(format!("{nibble:X}")),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(color))
                                .child(groups[index].0),
                        )
                }));
            strip = strip.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_start()
                    .gap(px(20.))
                    .child(nibbles)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .flex_1()
                            .min_w(px(180.))
                            .child(mono(instruction.assembly.clone()).text_size(px(18.)))
                            .child(note(format!(
                                "{} bytes at {:02X}",
                                instruction.bytes.len(),
                                instruction.address
                            ))),
                    ),
            );
        } else {
            strip = strip
                .child(
                    mono(instruction.assembly.clone())
                        .text_size(px(18.))
                        .text_color(rgb(READ)),
                )
                .child(note(format!(
                    "{}-bit {} instruction · {} encoded bytes",
                    self.architecture.bits(),
                    self.architecture.family(),
                    instruction.bytes.len()
                )));
        }
        if let Some(preview) = self
            .view
            .snapshot
            .as_ref()
            .and_then(|snapshot| inspect::vole_preview(instruction, snapshot))
        {
            strip = strip.child(mono(preview).text_size(px(13.)).text_color(rgb(WRITE)));
        }
        strip.child(
            div()
                .text_size(px(13.))
                .child(instruction.explanation.clone()),
        )
    }

    fn registers_panel(&self, cx: &mut Context<Self>) -> Div {
        let Some(snapshot) = self.view.snapshot.as_ref() else {
            return pane().child(heading("Registers", "")).child(
                div()
                    .p(px(20.))
                    .child(note("Assemble a program to inspect the machine.")),
            );
        };
        let current = self.view.current_instruction.as_ref();
        let last = snapshot.trace.last();
        let columns = if self.architecture == Architecture::Vole {
            2
        } else {
            1
        };
        let mut registers = snapshot.registers.iter().collect::<Vec<_>>();
        if matches!(self.architecture, Architecture::Arm32 | Architecture::Arm64) {
            registers.sort_by_key(|register| {
                register
                    .name
                    .strip_prefix('x')
                    .or_else(|| register.name.strip_prefix('r'))
                    .and_then(|number| number.parse::<usize>().ok())
                    .unwrap_or(usize::MAX)
            });
        }
        let order: &[&str] = match self.architecture {
            Architecture::X64 => &[
                "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp", "r8", "r9", "r10", "r11",
                "r12", "r13", "r14", "r15",
            ],
            Architecture::X86 => &["eax", "ebx", "ecx", "edx", "esi", "edi", "ebp", "esp"],
            _ => &[],
        };
        if !order.is_empty() {
            registers.sort_by_key(|register| {
                order
                    .iter()
                    .position(|name| register.name.eq_ignore_ascii_case(name))
                    .unwrap_or(usize::MAX)
            });
        }
        let chunks = registers.len().div_ceil(columns);
        let mut rows = Vec::new();
        for index in 0..chunks {
            let mut row = div()
                .flex()
                .items_center()
                .gap(px(12.))
                .h(px(29. * self.zoom));
            for column in 0..columns {
                if let Some(register) = registers.get(index + column * chunks) {
                    let name = register.name.clone();
                    let bits = register.bits;
                    let wrote = last.is_some_and(|last| {
                        last.registers
                            .iter()
                            .any(|r| r.name.eq_ignore_ascii_case(&name))
                    });
                    let reads = current
                        .is_some_and(|i| i.reads.iter().any(|r| r.eq_ignore_ascii_case(&name)));
                    let writes = current
                        .is_some_and(|i| i.writes.iter().any(|r| r.eq_ignore_ascii_case(&name)));
                    let color = if wrote || writes {
                        WRITE
                    } else if reads {
                        READ
                    } else {
                        INK
                    };
                    row = row.child(
                        div()
                            .id((ElementId::from("register"), name.clone()))
                            .role(Role::Button)
                            .aria_label(format!(
                                "Register {name}, hexadecimal {:X}, select to edit",
                                register.value
                            ))
                            .flex()
                            .flex_1()
                            .items_center()
                            .justify_between()
                            .gap(px(8.))
                            .px(px(4.))
                            .py(px(3.))
                            .rounded(px(4.))
                            .bg(rgb(if reads || writes || wrote {
                                SURFACE
                            } else {
                                WORKSPACE
                            }))
                            .font_family(MONO)
                            .text_size(px(13. * self.zoom))
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(SURFACE)))
                            .child(div().text_color(rgb(MUTED)).child(name.clone()))
                            .child(div().text_color(rgb(color)).child(format!(
                                "{:0width$X}",
                                register.value,
                                width = usize::from(bits).div_ceil(4)
                            )))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit_target = EditTarget::Register(name.clone(), bits);
                                this.edit_error = None;
                                this.set_edit_value(window, cx);
                                cx.notify();
                            })),
                    );
                }
            }
            rows.push(row);
        }
        pane()
            .child(heading(
                "Registers",
                format!("{}-bit values", self.architecture.bits()),
            ))
            .child(
                div()
                    .px(px(20.))
                    .py(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .child(
                        Button::new("edit-pc")
                            .ghost()
                            .small()
                            .label("PC")
                            .tooltip("Select program counter to edit")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_target =
                                    EditTarget::Register("PC".into(), this.architecture.bits());
                                this.set_edit_value(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        mono(format!(
                            "{:0width$X}",
                            snapshot.pc,
                            width = self.architecture.address_digits()
                        ))
                        .text_color(rgb(READ)),
                    ),
            )
            .child(
                div()
                    .id("register-scroll")
                    .px(px(16.))
                    .pb(px(12.))
                    .overflow_y_scroll()
                    .min_h_0()
                    .flex_1()
                    .children(rows)
                    .when(!snapshot.flags.is_empty(), |panel| {
                        panel.child(div().flex().flex_wrap().gap(px(10.)).py(px(12.)).children(
                            snapshot.flags.iter().map(|(name, value)| {
                                let flag = name.clone();
                                let next = !*value;
                                Button::new((ElementId::from("flag"), name.clone()))
                                    .ghost()
                                    .small()
                                    .label(format!("{name}={}", u8::from(*value)))
                                    .tooltip(format!(
                                        "Toggle {name} flag (clears execution history)"
                                    ))
                                    .disabled(!self.paused())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.send(
                                            Command::EditFlag {
                                                name: flag.clone(),
                                                value: next,
                                            },
                                            cx,
                                        )
                                    }))
                            }),
                        ))
                    })
                    .child(note("Select a register to inspect or edit it.")),
            )
    }
    fn explanation_panel(&self) -> Div {
        let instruction = self.view.current_instruction.as_ref();
        let snapshot = self.view.snapshot.as_ref();
        let mut content = div().flex().flex_col().gap(px(12.)).p(px(20.));
        if let Some(instruction) = instruction {
            content = content.child(
                div()
                    .text_size(px(13.))
                    .child(instruction.explanation.clone()),
            );
            if !instruction.reads.is_empty() {
                content = content.child(div().text_size(px(12.)).text_color(rgb(READ)).child(
                    format!(
                            "Read: {}",
                            instruction
                                .reads
                                .iter()
                                .map(|name| snapshot
                                    .and_then(|s| if name.eq_ignore_ascii_case("PC") {
                                        Some(s.pc)
                                    } else {
                                        s.register(name)
                                    })
                                    .map(|value| format!("{name}={value:X}"))
                                    .unwrap_or_else(|| name.clone()))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                ));
            }
            if !instruction.writes.is_empty() {
                content = content.child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(WRITE))
                        .child(format!("Write: {}", instruction.writes.join(", "))),
                );
            }
            if let Some(pc) = instruction
                .address
                .checked_add(instruction.bytes.len() as u64)
            {
                content = content.child(note(format!(
                    "Sequential next address: {pc:0width$X}",
                    width = self.architecture.address_digits()
                )));
            }
        } else {
            content = content.child(note(if self.view.state == RunState::Halted {
                "Machine halted. No next instruction."
            } else {
                "Instruction effects appear after assembly."
            }));
        }
        div()
            .flex()
            .flex_col()
            .flex_none()
            .border_t_1()
            .border_color(rgb(DIVIDER))
            .child(heading(
                if self.view.dirty {
                    "Previous executable"
                } else {
                    "What happens next"
                },
                if self.view.dirty {
                    "Source changed"
                } else {
                    "Preview"
                },
            ))
            .child(content)
    }
    fn trace_panel(&self) -> Div {
        let snapshot = self.view.snapshot.as_ref();
        let count = snapshot.map(|s| s.steps).unwrap_or(0);
        let mut panel = pane().child(heading("Trace", format!("{count} steps")));
        let records = snapshot
            .map(|s| s.trace.iter().rev().take(40).collect::<Vec<_>>())
            .unwrap_or_default();
        panel = panel.child(
            div()
                .id("trace-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p(px(20.))
                .children(records.into_iter().map(|record| {
                    let changes = record
                        .registers
                        .iter()
                        .map(|r| format!("{}  {:X} → {:X}", r.name, r.before, r.after))
                        .chain(record.memory.iter().map(|m| {
                            format!(
                                "[{:X}] {} → {}",
                                m.address,
                                m.before
                                    .iter()
                                    .map(|b| format!("{b:02X}"))
                                    .collect::<Vec<_>>()
                                    .join(" "),
                                m.after
                                    .iter()
                                    .map(|b| format!("{b:02X}"))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            )
                        }))
                        .collect::<Vec<_>>();
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .pb(px(14.))
                        .child(
                            mono(format!(
                                "{:0width$X}  {}",
                                record.pc_before,
                                record.instruction.assembly,
                                width = self.architecture.address_digits()
                            ))
                            .text_size(px(12.))
                            .text_ellipsis(),
                        )
                        .children(
                            changes
                                .into_iter()
                                .map(|text| mono(text).text_size(px(12.)).text_color(rgb(WRITE))),
                        )
                }))
                .when(count == 0, |panel| {
                    panel.child(note("Step the machine to record actual changes."))
                }),
        );
        let output = snapshot
            .map(|s| String::from_utf8_lossy(&s.output).to_string())
            .unwrap_or_default();
        panel.child(
            div()
                .border_t_1()
                .border_color(rgb(DIVIDER))
                .px(px(20.))
                .py(px(12.))
                .child(note("Output"))
                .child(
                    mono(if output.is_empty() {
                        "No output yet".into()
                    } else {
                        output
                    })
                    .text_size(px(12.))
                    .mt(px(6.)),
                ),
        )
    }

    fn memory_panel(&self, compact: bool, cx: &mut Context<Self>) -> Div {
        let digits = self.architecture.address_digits();
        let mut header = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(10.))
            .px(px(20.))
            .py(px(12.))
            .border_b_1()
            .border_color(rgb(DIVIDER))
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Main memory"),
            )
            .child(note(if self.architecture == Architecture::Vole {
                "256 bytes"
            } else {
                "256-byte page"
            }))
            .child(div().flex_1())
            .child(
                Button::new("memory-prev")
                    .ghost()
                    .small()
                    .label("Previous")
                    .disabled(self.memory_base < 256)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.memory_base = this.memory_base.saturating_sub(256);
                        cx.notify();
                    })),
            )
            .child(
                Button::new("memory-next")
                    .ghost()
                    .small()
                    .label("Next")
                    .disabled(
                        self.architecture == Architecture::Vole
                            || self
                                .memory_base
                                .checked_add(256)
                                .is_none_or(|a| !self.valid_address(a)),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(base) = this.memory_base.checked_add(256) {
                            this.memory_base = base;
                        }
                        cx.notify();
                    })),
            )
            .child(
                Input::new(&self.address_input)
                    .aria_label("Go to hexadecimal memory address")
                    .w(px(if digits == 16 { 168. } else { 105. }))
                    .font_family(MONO)
                    .small(),
            )
            .child(
                Button::new("memory-go")
                    .small()
                    .label("Go")
                    .on_click(cx.listener(|this, _, window, cx| this.go_to_address(window, cx))),
            );
        if compact {
            header = header.child(note("Scroll to inspect all memory columns"));
        }
        let table = div()
            .size_full()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .track_focus(&self.memory_focus)
            .key_context("Memory")
            .on_action(cx.listener(Self::memory_left))
            .on_action(cx.listener(Self::memory_right))
            .on_action(cx.listener(Self::memory_up))
            .on_action(cx.listener(Self::memory_down))
            .on_action(cx.listener(Self::copy_value))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(29.))
                    .px(px(12.))
                    .font_family(MONO)
                    .text_size(px(11.))
                    .text_color(rgb(MUTED))
                    .child(
                        div()
                            .w(px(if digits == 2 {
                                58.
                            } else if digits == 8 {
                                95.
                            } else {
                                160.
                            }))
                            .flex_none()
                            .child("Address"),
                    )
                    .children((0..16).map(|index| {
                        div()
                            .flex_1()
                            .min_w(px(24.))
                            .text_center()
                            .child(format!("{index:X}"))
                    }))
                    .child(
                        div()
                            .w(px(138.))
                            .flex_none()
                            .pl(px(14.))
                            .font_family(SANS)
                            .child("ASCII"),
                    ),
            )
            .child(
                uniform_list(
                    "memory-grid",
                    16,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range.map(|row| this.memory_row(row, cx)).collect()
                    }),
                )
                .flex_1()
                .min_h_0(),
            );
        pane()
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(
                        div()
                            .id("memory-horizontal")
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .overflow_x_scroll()
                            .child(table.min_w(px(if digits == 16 { 730. } else { 620. }))),
                    )
                    .when(!compact, |row| {
                        row.child(
                            self.edit_inspector(cx)
                                .w(px(245.))
                                .flex_none()
                                .border_l_1()
                                .border_color(rgb(DIVIDER)),
                        )
                    }),
            )
            .when(compact, |panel| {
                panel.child(self.edit_inspector(cx).h(px(160.)))
            })
    }
    fn memory_row(&self, row: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let digits = self.architecture.address_digits();
        let address = self.memory_base.saturating_add((row * 16) as u64);
        let snapshot = self.view.snapshot.as_ref();
        let pc = snapshot.map(|s| s.pc);
        let last = snapshot.and_then(|s| s.trace.last());
        let current_length = self
            .view
            .current_instruction
            .as_ref()
            .map(|i| i.bytes.len() as u64)
            .unwrap_or(0);
        let mut line = div()
            .id(("memory-row", address))
            .w_full()
            .flex()
            .items_center()
            .h(px(18. * self.zoom))
            .px(px(12.))
            .font_family(MONO)
            .text_size(px(12. * self.zoom))
            .child(
                div()
                    .w(px(if digits == 2 {
                        58.
                    } else if digits == 8 {
                        95.
                    } else {
                        160.
                    }))
                    .flex_none()
                    .text_color(rgb(MUTED))
                    .child(format!("{address:0digits$X}")),
            );
        let mut bytes = Vec::new();
        for column in 0..16 {
            let Some(byte_address) = address.checked_add(column) else {
                break;
            };
            let value = snapshot.and_then(|s| s.byte(byte_address));
            bytes.push(value.unwrap_or(0));
            let executing = pc.is_some_and(|pc| {
                byte_address >= pc && byte_address < pc.saturating_add(current_length)
            });
            let wrote = last.is_some_and(|r| {
                r.memory.iter().any(|m| {
                    byte_address >= m.address
                        && byte_address < m.address.saturating_add(m.after.len() as u64)
                })
            });
            let read = last.is_some_and(|r| {
                r.memory_reads.iter().any(|m| {
                    byte_address >= m.address
                        && byte_address < m.address.saturating_add(m.length as u64)
                })
            });
            let selected = self.selected == byte_address;
            line = line.child(
                div()
                    .id(("byte", byte_address))
                    .role(Role::Button)
                    .aria_label(format!(
                        "Memory address {byte_address:X}, {}",
                        value
                            .map(|b| format!("hexadecimal {b:02X}"))
                            .unwrap_or_else(|| "unmapped".into())
                    ))
                    .flex_1()
                    .min_w(px(24.))
                    .h(px(17. * self.zoom))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(if selected {
                        WRITE
                    } else if executing {
                        READ
                    } else {
                        WORKSPACE
                    }))
                    .bg(rgb(if selected || executing || wrote {
                        SURFACE
                    } else {
                        WORKSPACE
                    }))
                    .text_color(rgb(if wrote {
                        WRITE
                    } else if executing || read {
                        READ
                    } else if value == Some(0) || value.is_none() {
                        MUTED
                    } else {
                        INK
                    }))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(SURFACE)))
                    .child(
                        value
                            .map(|b| format!("{b:02X}"))
                            .unwrap_or_else(|| "--".into()),
                    )
                    .on_click(
                        cx.listener(move |this, _, w, cx| this.select_memory(byte_address, w, cx)),
                    ),
            );
        }
        line.child(
            div()
                .w(px(138.))
                .flex_none()
                .pl(px(14.))
                .text_size(px(11. * self.zoom))
                .text_color(rgb(MUTED))
                .child(inspect::ascii(&bytes)),
        )
    }
    fn edit_inspector(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let (title, bits, value, bytes) = match &self.edit_target {
            EditTarget::Memory(address) => {
                let value = self.view.snapshot.as_ref().and_then(|s| {
                    inspect::integer(s, *address, self.edit_width, self.little_endian)
                });
                let bytes = self
                    .view
                    .snapshot
                    .as_ref()
                    .and_then(|s| s.read(*address, self.edit_width))
                    .unwrap_or_default();
                (
                    format!(
                        "At {address:0width$X}",
                        width = self.architecture.address_digits()
                    ),
                    self.edit_width * 8,
                    value,
                    bytes,
                )
            }
            EditTarget::Register(name, bits) => {
                let value = self.view.snapshot.as_ref().and_then(|s| {
                    if name.eq_ignore_ascii_case("PC") {
                        Some(s.pc)
                    } else {
                        s.register(name)
                    }
                });
                (
                    format!("Register {name}"),
                    usize::from(*bits),
                    value,
                    Vec::new(),
                )
            }
        };
        let mut panel = div()
            .id("value-inspector")
            .flex()
            .flex_col()
            .min_h_0()
            .gap(px(8.))
            .p(px(16.))
            .overflow_y_scroll()
            .child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_size(px(14.))
                    .child(title),
            )
            .when(matches!(self.edit_target, EditTarget::Memory(_)), |panel| {
                panel.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            Button::new("value-width")
                                .small()
                                .label(format!(
                                    "{} byte{}",
                                    self.edit_width,
                                    if self.edit_width == 1 { "" } else { "s" }
                                ))
                                .tooltip("Cycle inspection width: 1, 2, 4, 8 bytes")
                                .on_click(cx.listener(|this, _, w, cx| {
                                    this.edit_width = match this.edit_width {
                                        1 => 2,
                                        2 => 4,
                                        4 => 8,
                                        _ => 1,
                                    };
                                    this.set_edit_value(w, cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("byte-order")
                                .small()
                                .ghost()
                                .label(if self.little_endian {
                                    "Little endian"
                                } else {
                                    "Big endian"
                                })
                                .on_click(cx.listener(|this, _, w, cx| {
                                    this.little_endian = !this.little_endian;
                                    this.set_edit_value(w, cx);
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(note("Hexadecimal value"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        Input::new(&self.value_input)
                            .aria_label("Hexadecimal register or memory value")
                            .font_family(MONO)
                            .small()
                            .disabled(!self.paused())
                            .flex_1(),
                    )
                    .child(
                        Button::new("apply-value")
                            .small()
                            .label("Apply")
                            .disabled(!self.paused())
                            .on_click(cx.listener(|this, _, w, cx| this.apply_edit(w, cx))),
                    ),
            );
        if let Some(value) = value {
            panel = panel
                .child(
                    mono(format!("Unsigned  {value}"))
                        .text_size(px(11.))
                        .text_color(rgb(MUTED)),
                )
                .child(
                    mono(format!("Signed    {}", inspect::signed(value, bits)))
                        .text_size(px(11.))
                        .text_color(rgb(MUTED)),
                )
                .child(
                    mono(format!("Binary    {}", inspect::binary(value, bits)))
                        .text_size(px(11.))
                        .text_color(rgb(MUTED)),
                );
            if !bytes.is_empty() {
                panel = panel.child(
                    mono(format!("ASCII     {}", inspect::ascii(&bytes)))
                        .text_size(px(11.))
                        .text_color(rgb(MUTED)),
                );
            }
            if self.architecture == Architecture::Vole && bits == 8 {
                panel=panel.child(mono(format!("VOLE float {}",inspect::vole_float(value as u8))).text_size(px(11.)).text_color(rgb(MUTED)))
                    .child(note("Float: 1 sign, 3 exponent (bias 4), 4 fraction bits. This is not IEEE 754."));
            }
        } else {
            panel = panel.child(note(
                "Unmapped address. Choose an address in a loaded memory region.",
            ));
        }
        if let EditTarget::Memory(address) = self.edit_target {
            for write in [true, false] {
                let existing = self
                    .view
                    .watchpoints
                    .iter()
                    .find(|watch| watch.address == address && watch.write == write);
                let length = existing
                    .map(|watch| watch.length)
                    .unwrap_or(self.edit_width);
                let label = format!(
                    "{} {} watchpoint",
                    if existing.is_some() { "Remove" } else { "Add" },
                    if write { "write" } else { "read" }
                );
                panel = panel.child(
                    Button::new(if write { "watch-write" } else { "watch-read" })
                        .small()
                        .ghost()
                        .label(label)
                        .disabled(self.view.snapshot.is_none())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.send(
                                Command::SetWatchpoint {
                                    address,
                                    length,
                                    write,
                                },
                                cx,
                            )
                        })),
                );
            }
        }
        if let Some(error) = &self.edit_error {
            panel = panel.child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(ERROR))
                    .child(error.clone()),
            );
        }
        panel
    }
    fn status_bar(&self, cx: &mut Context<Self>) -> Div {
        let steps = self.view.snapshot.as_ref().map(|s| s.steps).unwrap_or(0);
        let status = if let Some(error) = &self.edit_error {
            error.clone()
        } else if self.view.state == RunState::Editing {
            "Source changed. Assemble before running.".into()
        } else if matches!(self.view.state, RunState::Paused | RunState::Ready) {
            self.view
                .snapshot
                .as_ref()
                .map(|snapshot| {
                    format!(
                        "{} before {:0width$X} · {}",
                        self.view.state.label(),
                        snapshot.pc,
                        self.view.message,
                        width = self.architecture.address_digits()
                    )
                })
                .unwrap_or_else(|| self.view.message.clone())
        } else {
            self.view.message.clone()
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .h(px(34.))
            .flex_none()
            .px(px(20.))
            .border_t_1()
            .border_color(rgb(DIVIDER))
            .bg(rgb(SURFACE))
            .text_size(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_ellipsis()
                    .text_color(rgb(
                        if self.edit_error.is_some() || self.view.state == RunState::Faulted {
                            ERROR
                        } else {
                            READ
                        },
                    ))
                    .child(status),
            )
            .child(note(format!("{steps} instructions")))
            .child(
                mono(format!(
                    "Selected {:0width$X}",
                    self.selected,
                    width = self.architecture.address_digits()
                ))
                .text_size(px(12.))
                .text_color(rgb(MUTED)),
            )
            .child(note(format!("{}%", (self.zoom * 100.).round() as u32)))
            .child(
                Button::new("help")
                    .ghost()
                    .small()
                    .label("Help")
                    .tooltip("Help (F1)")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.show_help(&ShowHelp, window, cx)),
                    ),
            )
    }
}

impl Focusable for Workbench {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for Workbench {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width);
        let compact = width < 1150.;
        if !compact && let Some(layout) = self.pending_layout.take() {
            // Wait until the groups have measured their current viewport before applying ratios.
            // A document opened in compact mode keeps these preferences until the groups return.
            let entity = cx.entity();
            window.defer(cx, move |window, cx| {
                entity.update(cx, |workbench, cx| {
                    for (state, fraction) in [
                        (&workbench.split_source, layout.source_fraction),
                        (&workbench.split_vertical, layout.top_fraction),
                    ] {
                        let total = state.read(cx).sizes().iter().copied().sum::<Pixels>();
                        if total > px(0.) {
                            state.update(cx, |state, cx| {
                                state.resize_panel(0, total * fraction, window, cx)
                            });
                        }
                    }
                    workbench.split_main.update(cx, |state, cx| {
                        state.resize_panel(1, px(layout.inspector_width), window, cx)
                    });
                    cx.notify();
                });
            });
        }
        let content = if compact {
            let mut tabs = div()
                .flex()
                .gap(px(4.))
                .px(px(12.))
                .py(px(8.))
                .border_b_1()
                .border_color(rgb(DIVIDER));
            for (tab, label) in [
                (CompactTab::Assembly, "Assembly"),
                (CompactTab::Instructions, "Instructions"),
                (CompactTab::Memory, "Memory"),
                (CompactTab::Registers, "Registers"),
                (CompactTab::Trace, "Trace"),
            ] {
                tabs = tabs.child(
                    Button::new((ElementId::from("compact-tab"), label))
                        .ghost()
                        .small()
                        .label(label)
                        .when(self.compact_tab == tab, |button| button.primary())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.compact_tab = tab;
                            cx.notify();
                        })),
                );
            }
            let panel = match self.compact_tab {
                CompactTab::Assembly => self.source_panel(cx),
                CompactTab::Instructions => self.instructions_panel(cx),
                CompactTab::Memory => self.memory_panel(true, cx),
                CompactTab::Registers => {
                    let panel = pane().child(self.registers_panel(cx).flex_1().min_h_0());
                    if matches!(self.edit_target, EditTarget::Register(_, _)) {
                        panel.child(self.edit_inspector(cx).h(px(160.)).flex_none())
                    } else {
                        panel
                    }
                }
                CompactTab::Trace => self.trace_panel(),
            };
            pane().child(tabs).child(panel).into_any_element()
        } else {
            let left = v_resizable("source-memory")
                .with_state(&self.split_vertical)
                .child(
                    resizable_panel()
                        .size(px(390.))
                        .size_range(px(220.)..px(1200.))
                        .child(
                            h_resizable("source-instructions")
                                .with_state(&self.split_source)
                                .child(
                                    resizable_panel()
                                        .size(px(460.))
                                        .size_range(px(260.)..px(1200.))
                                        .child(self.source_panel(cx)),
                                )
                                .child(
                                    resizable_panel()
                                        .size_range(px(350.)..px(1800.))
                                        .child(self.instructions_panel(cx)),
                                ),
                        ),
                )
                .child(
                    resizable_panel()
                        .size_range(px(220.)..px(1200.))
                        .child(self.memory_panel(false, cx)),
                );
            let right = pane()
                .child(
                    self.registers_panel(cx)
                        .h(px(if self.architecture == Architecture::Vole {
                            330.
                        } else {
                            350.
                        }))
                        .flex_none(),
                )
                .child(self.explanation_panel())
                .child(self.trace_panel());
            h_resizable("main-inspector")
                .with_state(&self.split_main)
                .child(
                    resizable_panel()
                        .size_range(px(600.)..px(2400.))
                        .child(left),
                )
                .child(
                    resizable_panel()
                        .size(px(if self.architecture.bits() == 64 {
                            360.
                        } else {
                            320.
                        }))
                        .size_range(px(275.)..px(600.))
                        .child(right),
                )
                .into_any_element()
        };
        let mut shell = pane()
            .id("workbench")
            .role(Role::Application)
            .aria_label("Vole instruction simulation workbench")
            .relative()
            .track_focus(&self.focus)
            .key_context("Workbench")
            .font_family(SANS)
            .text_size(px(14.))
            .on_action(cx.listener(Self::assemble))
            .on_action(cx.listener(Self::run_pause))
            .on_action(cx.listener(Self::step))
            .on_action(cx.listener(Self::reverse))
            .on_action(cx.listener(Self::reset))
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(Self::export_bytes))
            .on_action(cx.listener(Self::show_help))
            .on_action(cx.listener(Self::load_example))
            .on_action(cx.listener(Self::toggle_breakpoint))
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::fullscreen))
            .on_action(cx.listener(Self::quit))
            .child(self.toolbar(compact, cx).w(px(width)).max_w(px(width)))
            .child(
                div()
                    .w(px(width))
                    .max_w(px(width))
                    .min_w_0()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(content),
            )
            .child(self.status_bar(cx));
        if self.architecture_menu {
            let menu = div()
                .absolute()
                .top(px(58.))
                .left(px(88.))
                .w(px(260.))
                .p(px(8.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .bg(rgb(SURFACE))
                .border_1()
                .border_color(rgb(DIVIDER))
                .rounded(px(6.))
                .children(Architecture::ALL.into_iter().map(|architecture| {
                    Button::new((ElementId::from("architecture"), architecture.id()))
                        .ghost()
                        .label(architecture.name())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.choose_architecture(architecture, window, cx)
                        }))
                }));
            shell = shell.child(menu);
        }
        shell
    }
}

fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), vole_core::SimError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| vole_core::SimError(error.to_string()))?;
    temporary
        .write_all(bytes)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|error| vole_core::SimError(error.to_string()))?;
    temporary.persist(path).map_err(|error| {
        vole_core::SimError(format!("Could not save {}: {error}", path.display()))
    })?;
    Ok(())
}

fn open_main_window(cx: &mut App) {
    if let Some(handle) = cx.windows().first().copied() {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
        return;
    }
    let existing = cx.global::<DesktopSession>().workbench.clone();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(1440.), px(894.)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Vole".into()),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        kind: WindowKind::Normal,
        is_resizable: true,
        is_minimizable: true,
        window_min_size: Some(size(px(720.), px(520.))),
        app_id: Some("dev.vole.Workbench".into()),
        ..Default::default()
    };
    match gpui_kit::open_window(options, cx, move |window, cx| {
        let entity = existing.unwrap_or_else(|| cx.new(|cx| Workbench::new(window, cx)));
        entity.update(cx, |workbench, cx| workbench.bind_window_inputs(window, cx));
        let closing = entity.clone();
        window.on_window_should_close(cx, move |window, cx| {
            // macOS keeps this entity and its complete editing session alive for Dock reopening.
            if cfg!(target_os = "macos") {
                return true;
            }
            closing.update(cx, |workbench, cx| {
                !workbench.confirm_discard(DiscardOperation::Close, window, cx)
            })
        });
        let focus = entity.focus_handle(cx);
        focus.focus(window, cx);
        entity
    }) {
        Ok((_, workbench)) => cx.global_mut::<DesktopSession>().workbench = Some(workbench),
        Err(error) => eprintln!("Vole could not open a native window: {error}"),
    }
    cx.activate(true);
}

fn main() {
    gpui_log::init();
    gpui_log::init_output_stderr();
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    let initial_architecture = arguments
        .windows(2)
        .find(|a| a[0] == "--arch")
        .and_then(|a| Architecture::parse(&a[1]))
        .unwrap_or_default();
    let demo_steps = if arguments.iter().any(|arg| arg == "--visual-demo") {
        2
    } else {
        0
    };
    let application = gpui_platform::application().with_assets(gpui_kit::assets::Assets);
    application.on_reopen(open_main_window);
    application.run(move |cx| {
        gpui_kit::init(cx);
        if let Err(error) = cx.text_system().add_fonts(vec![
            Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexSans.ttf")),
            Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")),
            Cow::Borrowed(include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf")),
        ]) {
            eprintln!("Could not load bundled fonts: {error}");
        }
        theme::install(cx);
        prompts::install(cx);
        cx.set_window_appearance(Some(WindowAppearance::Dark));
        cx.set_global(DesktopSession {
            workbench: None,
            initial_architecture,
            demo_steps,
        });
        cx.on_action(|_: &Quit, cx: &mut App| {
            open_main_window(cx);
            let entity = cx.global::<DesktopSession>().workbench.clone();
            if let (Some(entity), Some(handle)) = (entity, cx.windows().first().copied()) {
                let _ = handle.update(cx, |_, window, cx| {
                    entity.update(cx, |workbench, cx| {
                        if !workbench.confirm_discard(DiscardOperation::Quit, window, cx) {
                            cx.quit();
                        }
                    });
                });
            }
        });
        if !cfg!(target_os = "macos") {
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
        }
        let primary = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        cx.bind_keys([
            KeyBinding::new(&format!("{primary}-enter"), Assemble, Some("Workbench")),
            // Input's secondary Enter binding otherwise consumes the workbench shortcut.
            KeyBinding::new(&format!("{primary}-enter"), Assemble, Some("Input")),
            KeyBinding::new("f5", RunPause, Some("Workbench")),
            KeyBinding::new("f10", Step, Some("Workbench")),
            KeyBinding::new("shift-f10", ReverseStep, Some("Workbench")),
            KeyBinding::new(&format!("{primary}-r"), Reset, Some("Workbench")),
            KeyBinding::new(&format!("{primary}-o"), Open, Some("Workbench")),
            KeyBinding::new(&format!("{primary}-s"), Save, Some("Workbench")),
            KeyBinding::new(&format!("{primary}-shift-s"), SaveAs, Some("Workbench")),
            KeyBinding::new("f9", ToggleBreakpoint, Some("Workbench")),
            KeyBinding::new(&format!("{primary}-="), ZoomIn, Some("Workbench")),
            KeyBinding::new(&format!("{primary}--"), ZoomOut, Some("Workbench")),
            KeyBinding::new("f11", Fullscreen, Some("Workbench")),
            KeyBinding::new("f1", ShowHelp, Some("Workbench")),
            KeyBinding::new(
                &format!("{primary}-shift-e"),
                ExportBytes,
                Some("Workbench"),
            ),
            KeyBinding::new(&format!("{primary}-q"), Quit, None),
            KeyBinding::new("left", MemoryLeft, Some("Memory")),
            KeyBinding::new("right", MemoryRight, Some("Memory")),
            KeyBinding::new("up", MemoryUp, Some("Memory")),
            KeyBinding::new("down", MemoryDown, Some("Memory")),
            KeyBinding::new(&format!("{primary}-c"), CopyValue, Some("Memory")),
        ]);
        cx.set_menus([
            Menu::new("Vole").items([
                MenuItem::os_submenu("Services", SystemMenuType::Services),
                MenuItem::separator(),
                MenuItem::action("Quit Vole", Quit),
            ]),
            Menu::new("File").items([
                MenuItem::action("Open…", Open),
                MenuItem::action("Save", Save),
                MenuItem::action("Save as…", SaveAs),
                MenuItem::action("Export machine bytes…", ExportBytes),
                MenuItem::separator(),
                MenuItem::action("Load architecture example", LoadExample),
            ]),
            Menu::new("Edit").items([
                MenuItem::action("Undo", gpui_kit::component::input::Undo),
                MenuItem::action("Redo", gpui_kit::component::input::Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", gpui_kit::component::input::Cut),
                MenuItem::action("Copy", gpui_kit::component::input::Copy),
                MenuItem::action("Paste", gpui_kit::component::input::Paste),
                MenuItem::action("Select all", gpui_kit::component::input::SelectAll),
            ]),
            Menu::new("Run").items([
                MenuItem::action("Assemble", Assemble),
                MenuItem::action("Run / Pause", RunPause),
                MenuItem::action("Step", Step),
                MenuItem::action("Reverse step", ReverseStep),
                MenuItem::action("Reset", Reset),
                MenuItem::separator(),
                MenuItem::action("Toggle breakpoint", ToggleBreakpoint),
            ]),
            Menu::new("View").items([
                MenuItem::action("Increase code and data size", ZoomIn),
                MenuItem::action("Decrease code and data size", ZoomOut),
            ]),
            Menu::new("Window").items([MenuItem::action("Toggle fullscreen", Fullscreen)]),
            Menu::new("Help").items([MenuItem::action("Using Vole", ShowHelp)]),
        ]);
        open_main_window(cx);
    });
}

fn syntax_tokens(source: &str) -> Vec<TextDecoration> {
    let mut tokens = Vec::new();
    let mut base = 0;
    for line in source.split_inclusive('\n') {
        let comment = line
            .char_indices()
            .find(|(_, c)| *c == ';' || *c == '@')
            .map(|(i, _)| i)
            .or_else(|| line.find("//"));
        let code = &line[..comment.unwrap_or(line.len())];
        let bytes = code.as_bytes();
        let mut index = 0;
        let mut first = true;
        while index < bytes.len() {
            if !(bytes[index].is_ascii_alphanumeric()
                || bytes[index] == b'_'
                || bytes[index] == b'.')
            {
                index += 1;
                continue;
            }
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || matches!(bytes[index], b'_' | b'.'))
            {
                index += 1;
            }
            let token = &code[start..index];
            let lower = token.to_ascii_lowercase();
            let color = if lower.starts_with('r')
                && lower[1..].chars().all(|c| c.is_ascii_hexdigit())
                || lower.starts_with('x') && lower[1..].chars().all(|c| c.is_ascii_digit())
                || lower.starts_with('w') && lower[1..].chars().all(|c| c.is_ascii_digit())
                || matches!(
                    lower.as_str(),
                    "rax"
                        | "rbx"
                        | "rcx"
                        | "rdx"
                        | "eax"
                        | "ebx"
                        | "ecx"
                        | "edx"
                        | "rsp"
                        | "rbp"
                        | "esp"
                        | "ebp"
                        | "sp"
                        | "lr"
                        | "pc"
                ) {
                Some(INK)
            } else if token.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                Some(WRITE)
            } else if first && bytes.get(index) != Some(&b':') {
                Some(READ)
            } else {
                None
            };
            if let Some(color) = color {
                tokens.push(TextDecoration::new(
                    base + start..base + index,
                    HighlightStyle {
                        color: Some(rgb(color).into()),
                        ..Default::default()
                    },
                ));
            }
            first = false;
        }
        if let Some(comment) = comment {
            tokens.push(TextDecoration::new(
                base + comment..base + line.trim_end_matches('\n').len(),
                HighlightStyle {
                    color: Some(rgb(MUTED).into()),
                    ..Default::default()
                },
            ));
        }
        base += line.len();
    }
    tokens
}
