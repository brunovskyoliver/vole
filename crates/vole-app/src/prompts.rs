use crate::theme::{DIVIDER, INK, MUTED, SANS, SURFACE, WORKSPACE};
use gpui::{prelude::*, *};
use gpui_kit::component::button::{Button, ButtonVariants};

pub fn install(cx: &mut App) {
    if cfg!(target_os = "macos") {
        return;
    }
    cx.set_prompt_builder(|_, message, detail, actions, handle, window, cx| {
        let prompt = cx.new(|cx| WorkbenchPrompt {
            message: message.into(),
            detail: detail.map(Into::into),
            actions: actions.to_vec(),
            focus: cx.focus_handle(),
            selected: 0,
        });
        handle.with_view(prompt, window, cx)
    });
}
struct WorkbenchPrompt {
    message: SharedString,
    detail: Option<SharedString>,
    actions: Vec<PromptButton>,
    focus: FocusHandle,
    selected: usize,
}
impl EventEmitter<PromptResponse> for WorkbenchPrompt {}
impl Focusable for WorkbenchPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for WorkbenchPrompt {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = (f32::from(window.viewport_size().width) - 48.).clamp(280., 520.);
        let height = (f32::from(window.viewport_size().height) - 64.).max(240.);
        div()
            .size_full()
            .absolute()
            .top_0()
            .left_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x080d16c9))
            .font_family(SANS)
            .text_color(rgb(INK))
            .child(
                div()
                    .id("workbench-prompt")
                    .role(Role::Dialog)
                    .aria_label(self.message.clone())
                    .track_focus(&self.focus)
                    .key_context("VolePrompt")
                    .w(px(width))
                    .max_h(px(height))
                    .p(px(24.))
                    .bg(rgb(SURFACE))
                    .border_1()
                    .border_color(rgb(DIVIDER))
                    .rounded(px(8.))
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => cx.emit(PromptResponse(0)),
                            "enter" => cx.emit(PromptResponse(this.selected)),
                            "tab" | "right" | "left" => {
                                let count = this.actions.len().max(1);
                                let backwards = event.keystroke.modifiers.shift
                                    || event.keystroke.key == "left";
                                this.selected = if backwards {
                                    (this.selected + count - 1) % count
                                } else {
                                    (this.selected + 1) % count
                                };
                                cx.notify();
                            }
                            _ => return,
                        }
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .text_size(px(19.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.message.clone()),
                    )
                    .when_some(self.detail.clone(), |panel, detail| {
                        panel.child(
                            div()
                                .id("prompt-description")
                                .min_h_0()
                                .overflow_y_scroll()
                                .text_size(px(14.))
                                .text_color(rgb(MUTED))
                                .child(detail),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .justify_end()
                            .gap(px(10.))
                            .children(self.actions.iter().enumerate().map(|(index, action)| {
                                Button::new(("prompt-answer", index))
                                    .label(action.label().clone())
                                    .when(index == self.selected, |button| button.primary())
                                    .when(index != self.selected, |button| {
                                        button.bg(rgb(WORKSPACE))
                                    })
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(PromptResponse(index));
                                        cx.stop_propagation();
                                    }))
                            })),
                    ),
            )
    }
}
