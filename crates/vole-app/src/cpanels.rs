//! Panels for C source debugging: machine code grouped by C line, call stack,
//! variables with storage locations, program output and compiler diagnostics.
use crate::{
    Workbench,
    cmodel::{self, ChipTarget, MachineRow},
    dock::PanelId,
    theme::*,
};
use gpui::{prelude::*, *};
use gpui_kit::component::{
    IconName, Sizable,
    button::{Button, ButtonVariants},
    tooltip::Tooltip,
};
use vole_core::{
    Severity,
    debug::{FrameView, ValueText, VariableView},
};
use vole_runtime::RunState;

const ROW: f32 = 27.;
/// Gap between the group rail and the row content.
const RAIL: f32 = 3.;

impl Workbench {
    /// C images live below 0x20000 on every target, so machine code uses eight
    /// digits even on 64-bit targets; this leaves room for the assembly text.
    fn address_width(&self) -> f32 {
        82. * self.zoom
    }

    pub(crate) fn machine_code_panel(&self, cx: &mut Context<Self>) -> Div {
        let count = if self.image_matches() {
            self.machine_layout.rows.len()
        } else {
            0
        };
        let empty = if self.view.state == RunState::Assembling {
            "Compiling the C source with Clang."
        } else if self.view.diagnostics.iter().any(|d| d.is_error()) {
            "Fix the errors listed under the C source, then compile."
        } else {
            "Compile to see the machine code for each C line."
        };
        pane()
            .child(self.panel_heading(
                PanelId::Code,
                if count == 0 {
                    String::new()
                } else {
                    format!(
                        "{} instructions, grouped by C line",
                        self.view.disassembly.len()
                    )
                },
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .pl(px(20. + RAIL))
                    .pr(px(20.))
                    .h(px(28. * self.zoom))
                    .flex_none()
                    .border_b_1()
                    .border_color(rgb(DIVIDER))
                    .text_size(px(12. * self.zoom))
                    .text_color(rgb(MUTED))
                    .child(div().w(px(22.)).flex_none())
                    .child(
                        div()
                            .w(px(self.address_width()))
                            .flex_none()
                            .child("Address"),
                    )
                    .child(div().w(px(130. * self.zoom)).flex_none().child("Bytes"))
                    .child("Assembly"),
            )
            .child(
                uniform_list(
                    "machine-code",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                        range
                            .filter_map(|index| this.machine_layout.rows.get(index).copied())
                            .map(|row| this.machine_row(row, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.instruction_scroll)
                .flex_1()
                .min_h_0(),
            )
            .when(count == 0, |panel| {
                panel.child(div().px(px(20.)).py(px(20.)).child(note(empty)))
            })
            .when(count > 0 && !self.image_current(), |panel| {
                panel.child(
                    div()
                        .flex_none()
                        .px(px(20.))
                        .py(px(8.))
                        .border_t_1()
                        .border_color(rgb(DIVIDER))
                        .child(note(
                            if self.view.state == RunState::Assembling
                                || self.pending_build.is_some()
                            {
                                "Showing the last successful build while compiling."
                            } else {
                                "Showing the last successful build; compile to update."
                            },
                        )),
                )
            })
            .child(self.decode_strip())
    }

    /// Index of the group containing the PC.
    fn pc_group(&self) -> Option<usize> {
        let pc = self.view.snapshot.as_ref()?.pc;
        let index = self.view.disassembly.iter().position(|i| i.address == pc)?;
        self.machine_layout.group_of_instruction(index)
    }

    /// Breakpoint marks compare the editor's stored lines with the image's
    /// lines, so they only appear while both describe the same text.
    fn line_has_breakpoint(&self, line: usize) -> bool {
        self.image_current()
            && (self.view.source_breakpoints.contains_key(&line)
                || self
                    .view
                    .source_breakpoints
                    .values()
                    .any(|b| b.resolved_line == Some(line)))
    }

    fn machine_row(&self, row: MachineRow, cx: &mut Context<Self>) -> AnyElement {
        let group_index = match row {
            MachineRow::Header(group) | MachineRow::Instruction { group, .. } => group,
        };
        let Some(group) = self.machine_layout.groups.get(group_index) else {
            return div().into_any_element();
        };
        let current = self.image_current();
        let cursor_line = self.editor.read(cx).cursor_position().line as usize + 1;
        let in_pc_group = self.pc_group() == Some(group_index);
        let line_selected = current && group.line == Some(cursor_line);
        let rail = div()
            .w(px(RAIL))
            .h_full()
            .flex_none()
            .when(in_pc_group, |rail| rail.bg(rgb(READ)));
        let base = |id: ElementId| {
            div()
                .id(id)
                .w_full()
                .flex()
                .items_center()
                .h(px(ROW * self.zoom))
                .gap(px(12.))
                .pr(px(20.))
                .bg(rgb(if line_selected { SURFACE } else { WORKSPACE }))
                .child(rail)
        };
        match row {
            MachineRow::Header(_) => {
                let Some(line) = group.line else {
                    return base(("group", group_index).into())
                        .role(Role::Row)
                        .aria_label(cmodel::unlined_group_label(group))
                        .child(div().w(px(17. + 22.)).flex_none())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .text_size(px(12. * self.zoom))
                                .text_color(rgb(MUTED))
                                .child(cmodel::unlined_group_label(group)),
                        )
                        .into_any_element();
                };
                // The image's own source: the editor may hold newer, uncompiled text.
                let text = self
                    .view
                    .program
                    .as_ref()
                    .and_then(|p| p.source.lines().nth(line.saturating_sub(1)))
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let breakpoint = self.line_has_breakpoint(line);
                let dot = div()
                    .id(("line-breakpoint", group_index))
                    .role(Role::Button)
                    .aria_label(format!(
                        "{} breakpoint on line {line}",
                        if breakpoint { "Remove" } else { "Add" }
                    ))
                    .tooltip(move |window, cx| {
                        Tooltip::new(format!(
                            "{} breakpoint on line {line} (F9)",
                            if breakpoint { "Remove" } else { "Add" }
                        ))
                        .build(window, cx)
                    })
                    .w(px(22.))
                    .h(px(22.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(SURFACE)))
                    .child(
                        div()
                            .size(px(9.))
                            .rounded_full()
                            .when(breakpoint, |d| d.bg(rgb(WRITE)))
                            .when(!breakpoint, |d| d.border_1().border_color(rgb(DIVIDER))),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        if !this.image_current() {
                            return;
                        }
                        let stored = cmodel::breakpoint_line_to_toggle(
                            line,
                            this.view
                                .source_breakpoints
                                .values()
                                .map(|b| (b.line, b.resolved_line)),
                        );
                        this.toggle_source_breakpoint(stored, cx);
                    }));
                base(("group", group_index).into())
                    .role(Role::Row)
                    .aria_label(format!(
                        "Line {line}: {text}{}",
                        if in_pc_group {
                            ", contains the next instruction"
                        } else {
                            ""
                        }
                    ))
                    .cursor_pointer()
                    .child(div().w(px(17. - 12.)).flex_none())
                    .child(if current {
                        dot.into_any_element()
                    } else {
                        div().w(px(22.)).flex_none().into_any_element()
                    })
                    .child(
                        div()
                            .w(px(self.address_width()))
                            .flex_none()
                            .text_size(px(12. * self.zoom))
                            .text_color(rgb(if in_pc_group { READ } else { MUTED }))
                            .child(format!("Line {line}")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .font_family(MONO)
                            .text_size(px(13. * self.zoom))
                            .text_color(rgb(if in_pc_group { INK } else { MUTED }))
                            .child(text),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if this.image_current() {
                            this.reveal_line(line, 1, window, cx)
                        }
                    }))
                    .into_any_element()
            }
            MachineRow::Instruction { index, .. } => {
                let Some(instruction) = self.view.disassembly.get(index) else {
                    return div().into_any_element();
                };
                let address = instruction.address;
                let pc = self.view.snapshot.as_ref().is_some_and(|s| s.pc == address);
                let selected = self.selected == address;
                let breakpoint = self.view.breakpoints.contains(&address);
                base(("instruction", address).into())
                    .role(Role::Row)
                    .aria_label(format!(
                        "Address {address:X}, {}{}",
                        instruction.assembly,
                        if pc { ", next instruction" } else { "" }
                    ))
                    .font_family(MONO)
                    .text_size(px(13. * self.zoom))
                    .when(pc || selected, |row| row.bg(rgb(SURFACE)))
                    .text_color(rgb(if pc { READ } else { INK }))
                    .cursor_pointer()
                    .child(div().w(px(17. - 12.)).flex_none())
                    .child(
                        Button::new(("breakpoint", address))
                            .ghost()
                            .small()
                            .w(px(22.))
                            .p_0()
                            .label(if breakpoint { "●" } else { "·" })
                            .tooltip(format!(
                                "{} instruction breakpoint at {:X}",
                                if breakpoint { "Remove" } else { "Add" },
                                address
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(vole_runtime::Command::ToggleBreakpoint(address), cx)
                            })),
                    )
                    .child(
                        div()
                            .w(px(self.address_width()))
                            .flex_none()
                            .child(format!("{address:08X}")),
                    )
                    .child(
                        div()
                            .w(px(130. * self.zoom))
                            .flex_none()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(rgb(if pc { READ } else { MUTED }))
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
                            .child(instruction.assembly.clone()),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_instruction(address, window, cx)
                    }))
                    .into_any_element()
            }
        }
    }

    pub(crate) fn diagnostics_list(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let c_mode = self.language == vole_core::SourceLanguage::C;
        div()
            .id("diagnostics")
            .role(Role::List)
            .aria_label("Diagnostics")
            .flex_none()
            .max_h(px(if c_mode { 190. } else { 110. }))
            .overflow_y_scroll()
            .border_t_1()
            .border_color(rgb(DIVIDER))
            .py(px(6.))
            .children(self.view.diagnostics.iter().enumerate().map(|(index, d)| {
                let (label, color) = match d.severity {
                    Severity::Error => ("Error", ERROR),
                    Severity::Warning => ("Warning", WRITE),
                    Severity::Note => ("Note", READ),
                };
                let line = d.line;
                let column = d.column;
                div()
                    .id(("diagnostic", index))
                    .role(Role::ListItem)
                    .aria_label(format!(
                        "{label}, line {line}, column {column}: {}{}",
                        d.message,
                        d.hint
                            .as_ref()
                            .map(|h| format!(". {h}"))
                            .unwrap_or_default()
                    ))
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .px(px(20.))
                    .py(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(SURFACE)))
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(8.))
                            .text_size(px(12.))
                            .child(
                                div()
                                    .flex_none()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(color))
                                    .child(label),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_color(rgb(MUTED))
                                    .child(format!("Line {line}, column {column}")),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(INK))
                            .child(d.message.clone()),
                    )
                    .when_some(d.hint.clone(), |item, hint| {
                        item.child(div().text_size(px(12.)).text_color(rgb(MUTED)).child(hint))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.reveal_line(line, column, window, cx)
                    }))
            }))
    }

    fn selected_frame_view(&self) -> Option<&FrameView> {
        self.view.debug.as_ref()?.frames.get(self.selected_frame)
    }

    /// Why there are no frames to show yet.
    fn debug_empty_text(&self) -> &'static str {
        match self.view.state {
            RunState::Running => "Running. Pause or wait for a breakpoint to inspect frames.",
            RunState::Assembling => "Compiling.",
            RunState::Halted => "The program has finished.",
            RunState::Faulted => "The program stopped with a fault.",
            _ if self.view.program.is_none() || self.view.dirty => {
                "Compile, then step into the program to see its call stack."
            }
            _ => "Step into or continue to a breakpoint to stop inside a function.",
        }
    }

    pub(crate) fn call_stack_panel(&self, cx: &mut Context<Self>) -> Div {
        let debug = self.view.debug.as_ref();
        let frames = debug.map(|d| d.frames.as_slice()).unwrap_or_default();
        let mut list = div()
            .id("call-stack")
            .role(Role::List)
            .aria_label("Call stack")
            .flex()
            .flex_col()
            .max_h(px(150.))
            .overflow_y_scroll()
            .py(px(6.));
        for frame in frames {
            let index = frame.index;
            let selected = index == self.selected_frame;
            let place = match frame.location.as_ref() {
                Some(location) if location.user => format!("line {}", location.line),
                _ => "runtime code, no C source".into(),
            };
            list =
                list.child(
                    div()
                        .id(("frame", index))
                        .role(Role::ListItem)
                        .aria_label(format!(
                            "Frame {index}: {}, {place}{}",
                            frame.function,
                            if selected { ", selected" } else { "" }
                        ))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .h(px(26.))
                        .px(px(20.))
                        .cursor_pointer()
                        .bg(rgb(if selected { SURFACE } else { WORKSPACE }))
                        .hover(|s| s.bg(rgb(SURFACE)))
                        .child(
                            div()
                                .w(px(RAIL))
                                .h(px(16.))
                                .flex_none()
                                .when(index == 0, |d| d.bg(rgb(READ))),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_ellipsis()
                                .font_family(MONO)
                                .text_size(px(13.))
                                .text_color(rgb(if frame.user { INK } else { MUTED }))
                                .child(frame.function.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(rgb(MUTED))
                                .child(place),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.select_frame(index, window, cx)
                        })),
                );
        }
        let note_text = debug.and_then(|d| d.note.clone());
        pane()
            .child(self.panel_heading(
                PanelId::CallStack,
                match frames.len() {
                    0 => String::new(),
                    1 => "1 frame".into(),
                    n => format!("{n} frames"),
                },
                cx,
            ))
            .when(frames.is_empty(), |panel| {
                panel.child(
                    div()
                        .px(px(20.))
                        .py(px(12.))
                        .child(note(self.debug_empty_text())),
                )
            })
            .child(list)
            .when_some(note_text, |panel, text| {
                panel.child(div().px(px(20.)).pb(px(10.)).child(note(text)))
            })
    }

    fn select_frame(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_frame = index;
        self.update_source_decorations(cx);
        let line = self
            .selected_frame_view()
            .and_then(|f| f.location.as_ref())
            .filter(|l| l.user)
            .map(|l| l.line as usize);
        if let Some(line) = line.filter(|_| self.image_current()) {
            self.editor.update(cx, |state, cx| {
                state.set_cursor_position(
                    gpui_kit::component::input::Position::new(line.saturating_sub(1) as u32, 0),
                    window,
                    cx,
                )
            });
        }
        cx.notify();
    }

    pub(crate) fn variables_panel(&self, cx: &mut Context<Self>) -> Div {
        let debug = self.view.debug.as_ref();
        let frame = self.selected_frame_view();
        let mut rows: Vec<AnyElement> = Vec::new();
        if let Some(frame) = frame {
            if frame.variables.is_empty() {
                rows.push(
                    div()
                        .px(px(20.))
                        .py(px(6.))
                        .child(note(if frame.user {
                            format!("{} has no parameters or locals here.", frame.function)
                        } else {
                            format!("{} is runtime code without C variables.", frame.function)
                        }))
                        .into_any_element(),
                );
            }
            for variable in &frame.variables {
                self.variable_rows(
                    variable,
                    format!("{}:{}", frame.function, variable.name),
                    0,
                    &mut rows,
                    cx,
                );
            }
        }
        let globals = debug.map(|d| d.globals.as_slice()).unwrap_or_default();
        if !globals.is_empty() {
            rows.push(
                div()
                    .px(px(20.))
                    .pt(px(12.))
                    .pb(px(4.))
                    .text_size(px(12.))
                    .text_color(rgb(MUTED))
                    .child("Globals")
                    .into_any_element(),
            );
            for variable in globals {
                self.variable_rows(
                    variable,
                    format!("global:{}", variable.name),
                    0,
                    &mut rows,
                    cx,
                );
            }
        }
        let empty = rows.is_empty();
        pane()
            .child(self.panel_heading(
                PanelId::Variables,
                frame
                    .map(|f| f.function.clone())
                    .unwrap_or_default(),
                cx,
            ))
            .child(
                div()
                    .id("variables")
                    .role(Role::Tree)
                    .aria_label("Variables")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py(px(6.))
                    .children(rows)
                    .when(empty, |list| {
                        list.child(div().px(px(20.)).py(px(6.)).child(note(
                            if self.view.program.is_none() || self.view.dirty {
                                "Compile, then step into the program to see its variables."
                            } else if self.view.state == RunState::Running {
                                "Running. Pause to inspect variables."
                            } else {
                                "Step into or continue to a breakpoint to see variables and their locations."
                            },
                        )))
                    }),
            )
    }

    fn variable_rows(
        &self,
        variable: &VariableView,
        key: String,
        depth: usize,
        rows: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let expandable = !variable.children.is_empty();
        let expanded = expandable && self.expanded.contains(&key);
        let chip = cmodel::location_chip(variable);
        let (value, unavailable) = match &variable.value {
            ValueText::Value(text) => (text.clone(), false),
            ValueText::Unavailable(reason) => (cmodel::unavailable_text(reason), true),
        };
        let indent = 20. + depth as f32 * 16.;
        let toggle_key = key.clone();
        let disclosure = if expandable {
            Button::new(SharedString::from(format!("disclose-{key}")))
                .ghost()
                .xsmall()
                .icon(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .tooltip(if expanded { "Collapse" } else { "Expand" })
                .on_click(cx.listener(move |this, _, _, cx| {
                    if !this.expanded.remove(&toggle_key) {
                        this.expanded.insert(toggle_key.clone());
                    }
                    cx.notify();
                }))
                .into_any_element()
        } else {
            div().w(px(20.)).flex_none().into_any_element()
        };
        let name = div()
            .flex_none()
            .font_family(MONO)
            .text_size(px(13. * self.zoom))
            .text_color(rgb(INK))
            .child(variable.name.clone());
        let type_name = div()
            .flex_1()
            .min_w(px(24.))
            .overflow_hidden()
            .text_ellipsis()
            .whitespace_nowrap()
            .font_family(MONO)
            .text_size(px(12. * self.zoom))
            .text_color(rgb(MUTED))
            .child(variable.type_name.clone());
        let value_element = div()
            .font_family(if unavailable { SANS } else { MONO })
            .text_size(px(13. * self.zoom))
            .text_color(rgb(if unavailable {
                MUTED
            } else if variable.changed {
                WRITE
            } else {
                INK
            }))
            .child(value.clone());
        let chip_element = chip.map(|chip| self.location_chip(&key, chip, cx));
        let label = format!(
            "{} {}, {}{}{}",
            variable.type_name,
            variable.name,
            value,
            if variable.changed { ", changed" } else { "" },
            if expandable {
                if expanded {
                    ", expanded"
                } else {
                    ", collapsed"
                }
            } else {
                ""
            }
        );
        let row = div()
            .id(SharedString::from(format!("variable-{key}")))
            .role(Role::TreeItem)
            .aria_label(label)
            .flex()
            .flex_col()
            .gap(px(2.))
            .pl(px(indent - 6.))
            .pr(px(16.))
            .py(px(3.))
            .hover(|s| s.bg(rgb(SURFACE)));
        let line = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(disclosure)
            .child(name)
            .child(type_name);
        // Short values share the name line; long values and reasons get their
        // own wrapped line so pointers, strings and arrays stay readable.
        let row = if unavailable || value.chars().count() > 14 {
            row.child(line.children(chip_element)).child(
                div()
                    .pl(px(28.))
                    .min_w_0()
                    .child(value_element.text_size(px(12. * self.zoom))),
            )
        } else {
            row.child(
                line.child(value_element.flex_none().whitespace_nowrap())
                    .children(chip_element),
            )
        };
        rows.push(row.into_any_element());
        if expanded {
            for child in &variable.children {
                self.variable_rows(child, format!("{key}/{}", child.name), depth + 1, rows, cx);
            }
        }
    }

    fn location_chip(
        &self,
        key: &str,
        chip: cmodel::LocationChip,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = match &chip.target {
            ChipTarget::Memory { address, size } => {
                self.selected == *address && self.selection_len == (*size).clamp(1, 256)
            }
            ChipTarget::Register(name) => matches!(
                &self.edit_target,
                crate::EditTarget::Register(selected, _) if selected.eq_ignore_ascii_case(name)
            ),
        };
        let description = chip.description.clone();
        let target = chip.target.clone();
        div()
            .id(SharedString::from(format!("location-{key}")))
            .role(Role::Button)
            .aria_label(chip.description.clone())
            .tooltip(move |window, cx| Tooltip::new(description.clone()).build(window, cx))
            .flex_none()
            .px(px(6.))
            .h(px(20. * self.zoom))
            .flex()
            .items_center()
            .rounded(px(4.))
            .border_1()
            .border_color(rgb(if active { READ } else { DIVIDER }))
            .font_family(MONO)
            .text_size(px(11. * self.zoom))
            .text_color(rgb(if active { READ } else { MUTED }))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(SURFACE)).text_color(rgb(INK)))
            .child(chip.text)
            .on_click(cx.listener(move |this, _, window, cx| match &target {
                ChipTarget::Memory { address, size } => {
                    this.show_in_memory(*address, *size, window, cx)
                }
                ChipTarget::Register(name) => this.show_register(name.clone(), window, cx),
            }))
            .into_any_element()
    }

    pub(crate) fn output_panel(&self, cx: &mut Context<Self>) -> Div {
        let snapshot = self.view.snapshot.as_ref();
        let output = snapshot
            .map(|s| String::from_utf8_lossy(&s.output).to_string())
            .unwrap_or_default();
        let exit = self.view.debug.as_ref().and_then(|d| d.exit_status);
        pane()
            .child(self.panel_heading(
                PanelId::Output,
                format!("{} instructions", snapshot.map_or(0, |s| s.steps)),
                cx,
            ))
            .child(
                div()
                    .id("program-output")
                    .track_scroll(&self.output_scroll)
                    .role(Role::Log)
                    .aria_label("Program output")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px(px(20.))
                    .py(px(10.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(if output.is_empty() {
                        note("Nothing printed yet. printf, puts and vole_print write here.")
                    } else {
                        mono(output)
                            .text_size(px(13. * self.zoom))
                            .whitespace_normal()
                    })
                    .when_some(exit, |panel, status| {
                        panel.child(
                            div()
                                .text_size(px(13.))
                                .text_color(rgb(if status == 0 { READ } else { WRITE }))
                                .child(format!("Program exited with status {status}")),
                        )
                    }),
            )
    }
}
