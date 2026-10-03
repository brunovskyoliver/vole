//! Renders the panel layout: resizable splits, draggable panel headings with a
//! hide button, and drop zones that show where a dragged panel will land.
use crate::{
    Workbench,
    dock::{Axis, Edge, Node, PanelId},
    theme::*,
};
use gpui::{prelude::*, *};
use gpui_kit::component::{
    Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
};

/// Thickness of the draggable gap between panels.
const SPLITTER: f32 = 5.;
const MIN_PANEL_WIDTH: f32 = 160.;
const MIN_PANEL_HEIGHT: f32 = 90.;

/// A panel being dragged by its heading.
#[derive(Clone)]
pub struct PanelDrag {
    pub id: PanelId,
    title: SharedString,
}

impl Render for PanelDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(12.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(READ))
            .font_family(SANS)
            .text_size(px(13.))
            .text_color(rgb(INK))
            .child(self.title.clone())
    }
}

/// The gap between two children of the split at `path` being dragged.
#[derive(Clone)]
struct SplitDrag {
    path: Vec<usize>,
    index: usize,
}

impl Render for SplitDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn path_id(prefix: &str, path: &[usize]) -> SharedString {
    let mut id = String::from(prefix);
    for index in path {
        id.push('-');
        id.push_str(&index.to_string());
    }
    id.into()
}

/// Six dots that mark a heading as a drag handle.
fn grip() -> Div {
    let dot = || div().size(px(2.5)).rounded_full().bg(rgb(MUTED));
    div()
        .flex()
        .gap(px(2.5))
        .flex_none()
        .children((0..2).map(move |_| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.5))
                .children((0..3).map(move |_| dot()))
        }))
}

impl Workbench {
    pub(crate) fn panel_title(&self, id: PanelId) -> &'static str {
        id.title(self.language)
    }

    /// The draggable part of a panel heading: grip and title.
    pub(crate) fn panel_handle(&self, id: PanelId, cx: &mut Context<Self>) -> Stateful<Div> {
        let title = self.panel_title(id);
        let entity = cx.entity().downgrade();
        div()
            .id(SharedString::from(format!("panel-handle-{title}")))
            .flex()
            .items_center()
            .gap(px(10.))
            .flex_none()
            .cursor_grab()
            .child(grip())
            .child(
                div()
                    .text_size(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .whitespace_nowrap()
                    .child(title),
            )
            .on_drag(
                PanelDrag {
                    id,
                    title: title.into(),
                },
                move |drag, _, _, cx| {
                    let _ = entity.update(cx, |this, cx| {
                        this.dragging_panel = Some(drag.id);
                        cx.notify();
                    });
                    cx.new(|_| drag.clone())
                },
            )
    }

    pub(crate) fn hide_button(&self, id: PanelId, cx: &mut Context<Self>) -> Button {
        let title = self.panel_title(id);
        Button::new(SharedString::from(format!("hide-panel-{title}")))
            .ghost()
            .xsmall()
            .icon(IconName::Close)
            .tooltip(format!("Hide {title}. Show it again from Panels."))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.set_panel_visible(id, false, cx);
            }))
    }

    /// Standard panel heading: drag handle, detail text and a hide button.
    pub(crate) fn panel_heading(
        &self,
        id: PanelId,
        detail: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> Div {
        let compact = self.compact;
        div()
            .flex()
            .items_center()
            .flex_none()
            .h(px(52.))
            .pl(px(if compact { 20. } else { 14. }))
            .pr(px(10.))
            .gap(px(12.))
            .border_b_1()
            .border_color(rgb(DIVIDER))
            .child(if compact {
                div()
                    .flex_none()
                    .text_size(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(self.panel_title(id))
                    .into_any_element()
            } else {
                self.panel_handle(id, cx).into_any_element()
            })
            .child(
                // Detail gives way to the title: it shrinks and truncates.
                div().flex_1().min_w_0().flex().justify_end().child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(px(12.))
                        .text_color(rgb(MUTED))
                        .child(detail.into()),
                ),
            )
            .when(!compact, |row| row.child(self.hide_button(id, cx)))
    }

    pub(crate) fn set_panel_visible(&mut self, id: PanelId, visible: bool, cx: &mut Context<Self>) {
        let language = self.language;
        let dock = self.preferences.dock_mut(language);
        if dock.is_visible(id) == visible {
            return;
        }
        if visible {
            dock.show(id);
        } else {
            dock.hide(id);
        }
        self.layout_changed(cx);
    }

    /// Persist the layout and refresh the native View menu's check marks.
    pub(crate) fn layout_changed(&mut self, cx: &mut Context<Self>) {
        self.layout_dirty = true;
        self.refresh_menus(cx);
        cx.notify();
    }

    fn drop_panel(&mut self, id: PanelId, target: PanelId, edge: Edge, cx: &mut Context<Self>) {
        self.dragging_panel = None;
        let language = self.language;
        self.preferences
            .dock_mut(language)
            .move_panel(id, target, edge);
        self.layout_changed(cx);
    }

    fn drop_panel_at_edge(&mut self, id: PanelId, edge: Edge, cx: &mut Context<Self>) {
        self.dragging_panel = None;
        let language = self.language;
        self.preferences.dock_mut(language).move_to_root(id, edge);
        self.layout_changed(cx);
    }

    /// The docked layout for wide windows.
    pub(crate) fn dock_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let dock = self.preferences.dock(self.language);
        let body = match &dock.root {
            Some(root) => self.render_node(root, Vec::new(), cx),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(note(
                    "Every panel is hidden. Choose Panels in the toolbar to show one.",
                ))
                .into_any_element(),
        };
        let mut view = div().size_full().relative().child(body);
        if self.dragging_panel.is_some() {
            for edge in [Edge::Left, Edge::Right, Edge::Bottom] {
                view = view.child(self.edge_zone(edge, cx));
            }
        }
        view.into_any_element()
    }

    fn render_node(&self, node: &Node, path: Vec<usize>, cx: &mut Context<Self>) -> AnyElement {
        let (axis, children) = match node {
            Node::Panel { id } => return self.render_leaf(*id, cx),
            Node::Split { axis, children } => (*axis, children),
        };
        let row = axis == Axis::Row;
        let drag_path = path.clone();
        let mut container = div()
            .id(path_id("split", &path))
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .when(!row, |d| d.flex_col())
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<SplitDrag>, _, cx| {
                    let drag = event.drag(cx);
                    if drag.path != drag_path {
                        return;
                    }
                    let index = drag.index;
                    this.drag_splitter(&drag_path, index, row, event, cx);
                }),
            );
        for (index, child) in children.iter().enumerate() {
            if index > 0 {
                container = container.child(self.splitter(&path, index - 1, row));
            }
            let mut child_path = path.clone();
            child_path.push(index);
            let mut cell = div()
                .relative()
                .overflow_hidden()
                .min_w(px(MIN_PANEL_WIDTH))
                .min_h(px(MIN_PANEL_HEIGHT));
            let style = cell.style();
            style.flex_grow = Some(child.weight.max(0.001));
            style.flex_shrink = Some(1.);
            style.flex_basis = Some(relative(0.).into());
            container = container.child(cell.child(self.render_node(&child.node, child_path, cx)));
        }
        container.into_any_element()
    }

    fn drag_splitter(
        &mut self,
        path: &[usize],
        index: usize,
        row: bool,
        event: &DragMoveEvent<SplitDrag>,
        cx: &mut Context<Self>,
    ) {
        let language = self.language;
        let Some(Node::Split { children, .. }) = self.preferences.dock(language).node(path) else {
            return;
        };
        let bounds = event.bounds;
        let (start, length, position) = if row {
            (bounds.origin.x, bounds.size.width, event.event.position.x)
        } else {
            (bounds.origin.y, bounds.size.height, event.event.position.y)
        };
        let gaps = SPLITTER * children.len().saturating_sub(1) as f32;
        let length = f32::from(length) - gaps;
        if length <= 1. || index + 1 >= children.len() {
            return;
        }
        let before: f32 = children[..index].iter().map(|c| c.weight).sum();
        let pair = children[index].weight + children[index + 1].weight;
        let first_start = before * length + SPLITTER * index as f32;
        let offset = f32::from(position - start) - first_start - SPLITTER / 2.;
        let minimum = if row {
            MIN_PANEL_WIDTH
        } else {
            MIN_PANEL_HEIGHT
        } / length;
        if pair < minimum * 2. {
            return;
        }
        let first = (offset / length).clamp(minimum, pair - minimum);
        self.preferences
            .dock_mut(language)
            .resize(path, index, first);
        self.layout_dirty = true;
        cx.notify();
    }

    fn splitter(&self, path: &[usize], index: usize, row: bool) -> Stateful<Div> {
        let mut id_path = path.to_vec();
        id_path.push(index);
        let drag = SplitDrag {
            path: path.to_vec(),
            index,
        };
        div()
            .id(path_id("splitter", &id_path))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(WORKSPACE))
            .when(row, |d| {
                d.w(px(SPLITTER)).h_full().flex_col().cursor_col_resize()
            })
            .when(!row, |d| d.h(px(SPLITTER)).w_full().cursor_row_resize())
            .child(
                div()
                    .when(row, |d| d.w(px(1.)).h_full())
                    .when(!row, |d| d.h(px(1.)).w_full())
                    .bg(rgb(DIVIDER)),
            )
            .hover(|s| s.bg(rgb(0x2b3b51)))
            .on_drag(drag, |drag, _, _, cx| cx.new(|_| drag.clone()))
    }

    fn render_leaf(&self, id: PanelId, cx: &mut Context<Self>) -> AnyElement {
        let dragging = self.dragging_panel;
        div()
            .size_full()
            .relative()
            .child(self.panel_content(id, cx))
            .when(dragging.is_some_and(|dragged| dragged != id), |leaf| {
                leaf.child(self.drop_zones(id, cx))
            })
            .when(dragging == Some(id), |leaf| {
                leaf.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(rgba(0x151c26b0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(note("Drop on another panel's edge or centre")),
                )
            })
            .into_any_element()
    }

    /// Five targets over a panel while another is dragged: the four edges
    /// split it, the centre trades places.
    fn drop_zones(&self, target: PanelId, cx: &mut Context<Self>) -> Div {
        let title = self.panel_title(target);
        let zone = |edge: Edge, cx: &mut Context<Self>| {
            let base = div()
                .id(SharedString::from(format!("drop-{title}-{edge:?}")))
                .absolute()
                .flex()
                .items_center()
                .justify_center();
            let placed = match edge {
                Edge::Left => base.left_0().top_0().bottom_0().w(relative(0.25)),
                Edge::Right => base.right_0().top_0().bottom_0().w(relative(0.25)),
                Edge::Top => base
                    .top_0()
                    .left(relative(0.25))
                    .right(relative(0.25))
                    .h(relative(0.3)),
                Edge::Bottom => base
                    .bottom_0()
                    .left(relative(0.25))
                    .right(relative(0.25))
                    .h(relative(0.3)),
                Edge::Center => base
                    .top(relative(0.3))
                    .bottom(relative(0.3))
                    .left(relative(0.25))
                    .right(relative(0.25)),
            };
            placed
                .drag_over::<PanelDrag>(|style, _, _, _| {
                    style
                        .bg(rgba(0x9cc8f438))
                        .border_2()
                        .border_color(rgb(READ))
                })
                .on_drop(cx.listener(move |this, drag: &PanelDrag, _, cx| {
                    this.drop_panel(drag.id, target, edge, cx)
                }))
        };
        div()
            .absolute()
            .inset_0()
            .border_1()
            .border_color(rgba(0x9cc8f455))
            .child(zone(Edge::Left, cx))
            .child(zone(Edge::Right, cx))
            .child(zone(Edge::Top, cx))
            .child(zone(Edge::Bottom, cx))
            .child(zone(Edge::Center, cx))
    }

    /// A strip along the dock's edge that makes a full-height column or a
    /// full-width row.
    fn edge_zone(&self, edge: Edge, cx: &mut Context<Self>) -> Stateful<Div> {
        let base = div()
            .id(SharedString::from(format!("drop-dock-{edge:?}")))
            .absolute()
            .bg(rgba(0x9cc8f414));
        let placed = match edge {
            Edge::Left => base.left_0().top_0().bottom_0().w(px(18.)),
            Edge::Right => base.right_0().top_0().bottom_0().w(px(18.)),
            _ => base.bottom_0().left_0().right_0().h(px(18.)),
        };
        placed
            .drag_over::<PanelDrag>(|style, _, _, _| style.bg(rgba(0x9cc8f466)))
            .on_drop(cx.listener(move |this, drag: &PanelDrag, _, cx| {
                this.drop_panel_at_edge(drag.id, edge, cx)
            }))
    }

    /// One panel's content, shared by the docked and compact layouts.
    pub(crate) fn panel_content(&self, id: PanelId, cx: &mut Context<Self>) -> AnyElement {
        match id {
            PanelId::Source => self.source_panel(cx).into_any_element(),
            PanelId::Code => self.instructions_panel(cx).into_any_element(),
            PanelId::Memory => self.memory_panel(self.compact, cx).into_any_element(),
            PanelId::Registers => self.registers_panel(cx).into_any_element(),
            PanelId::Explanation => self.explanation_panel(cx).into_any_element(),
            PanelId::Trace => self.trace_panel(cx).into_any_element(),
            PanelId::CallStack => self.call_stack_panel(cx).into_any_element(),
            PanelId::Variables => self.variables_panel(cx).into_any_element(),
            PanelId::Output => self.output_panel(cx).into_any_element(),
        }
    }

    /// Toolbar menu that shows, hides and resets panels.
    pub(crate) fn panels_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dock = self.preferences.dock(self.language);
        let mut menu = div()
            .id("panels-menu")
            .role(Role::Menu)
            .aria_label("Panels")
            .occlude()
            .w(px(240.))
            .p(px(6.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .bg(rgb(SURFACE))
            .border_1()
            .border_color(rgb(DIVIDER))
            .rounded(px(6.))
            .child(div().px(px(10.)).py(px(4.)).child(note("Show panels")));
        for &id in PanelId::all(self.language) {
            let visible = dock.is_visible(id);
            let title = self.panel_title(id);
            menu = menu.child(
                div()
                    .id(SharedString::from(format!("panel-toggle-{title}")))
                    .role(Role::MenuItemCheckBox)
                    .aria_label(format!(
                        "{title}, {}",
                        if visible { "shown" } else { "hidden" }
                    ))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .py(px(5.))
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0x2b3b51)))
                    .child(
                        div()
                            .size(px(16.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(rgb(READ))
                            .when(visible, |mark| {
                                mark.child(Icon::new(IconName::Check).size(px(14.)))
                            }),
                    )
                    .child(
                        div()
                            .text_color(rgb(if visible { INK } else { MUTED }))
                            .child(title),
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_panel_visible(id, !visible, cx)),
                    ),
            );
        }
        menu.child(div().my(px(4.)).h(px(1.)).bg(rgb(DIVIDER)))
            .child(
                div()
                    .id("panel-reset")
                    .role(Role::MenuItem)
                    .px(px(10.))
                    .py(px(5.))
                    .pl(px(34.))
                    .rounded(px(4.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0x2b3b51)))
                    .child("Reset layout")
                    .on_click(cx.listener(|this, _, _, cx| this.reset_layout(cx))),
            )
            .child(div().px(px(10.)).pt(px(6.)).pb(px(2.)).child(note(
                "Drag a panel by its title to move it. Drag the gaps between panels to resize.",
            )))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.panel_menu = false;
                cx.notify();
            }))
    }

    pub(crate) fn reset_layout(&mut self, cx: &mut Context<Self>) {
        let language = self.language;
        *self.preferences.dock_mut(language) = crate::dock::Dock::default_for(language);
        self.panel_menu = false;
        self.layout_changed(cx);
    }
}
