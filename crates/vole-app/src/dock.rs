//! Workbench panel layout: a tiling tree of panels that the user can hide,
//! show, drag to another place and resize. Each document language keeps its
//! own layout, and both are saved in the user's preferences between launches.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use vole_core::SourceLanguage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PanelId {
    Source,
    /// Instructions in assembly mode, machine code grouped by line in C mode.
    Code,
    Memory,
    Registers,
    /// "What happens next" for the current instruction (assembly only).
    Explanation,
    /// Executed instructions and output (assembly only).
    Trace,
    CallStack,
    Variables,
    /// Program output (C only).
    Output,
}

const ASSEMBLY_PANELS: &[PanelId] = &[
    PanelId::Source,
    PanelId::Code,
    PanelId::Memory,
    PanelId::Registers,
    PanelId::Explanation,
    PanelId::Trace,
];
const C_PANELS: &[PanelId] = &[
    PanelId::Source,
    PanelId::Code,
    PanelId::Memory,
    PanelId::CallStack,
    PanelId::Variables,
    PanelId::Registers,
    PanelId::Output,
];

impl PanelId {
    pub fn all(language: SourceLanguage) -> &'static [PanelId] {
        match language {
            SourceLanguage::Assembly => ASSEMBLY_PANELS,
            SourceLanguage::C => C_PANELS,
        }
    }

    pub fn title(self, language: SourceLanguage) -> &'static str {
        let c = language == SourceLanguage::C;
        match self {
            Self::Source if c => "C source",
            Self::Source => "Assembly",
            Self::Code if c => "Machine code",
            Self::Code => "Instructions",
            Self::Memory => "Main memory",
            Self::Registers => "Registers",
            Self::Explanation => "What happens next",
            Self::Trace => "Trace",
            Self::CallStack => "Call stack",
            Self::Variables => "Variables",
            Self::Output => "Output",
        }
    }

    /// Where a panel reappears when shown: next to the first visible anchor,
    /// otherwise as a new column on the right.
    fn anchors(self) -> &'static [(PanelId, Edge)] {
        use Edge::*;
        use PanelId::*;
        match self {
            Source => &[(Code, Left), (Memory, Top)],
            Code => &[(Source, Right), (Memory, Top)],
            Memory => &[(Source, Bottom), (Code, Bottom)],
            Registers => &[
                (Variables, Bottom),
                (Explanation, Top),
                (Trace, Top),
                (Output, Top),
                (CallStack, Bottom),
            ],
            Explanation => &[(Registers, Bottom), (Trace, Top)],
            Trace => &[(Explanation, Bottom), (Registers, Bottom)],
            CallStack => &[(Variables, Top), (Registers, Top)],
            Variables => &[(CallStack, Bottom), (Registers, Top)],
            Output => &[(Registers, Bottom), (Variables, Bottom)],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Axis {
    /// Children side by side, left to right.
    Row,
    /// Children stacked, top to bottom.
    Column,
}

/// Where a dragged panel lands relative to the panel it is dropped on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
    Top,
    Bottom,
    /// Trade places with the target.
    Center,
}

impl Edge {
    fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Row,
            _ => Axis::Column,
        }
    }
    fn before(self) -> bool {
        matches!(self, Self::Left | Self::Top)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Node {
    Panel { id: PanelId },
    Split { axis: Axis, children: Vec<Child> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Child {
    /// Share of the parent split; the shares of one split sum to 1.
    pub weight: f32,
    pub node: Node,
}

fn panel(id: PanelId, weight: f32) -> Child {
    Child {
        weight,
        node: Node::Panel { id },
    }
}
fn split(axis: Axis, weight: f32, children: Vec<Child>) -> Child {
    Child {
        weight,
        node: Node::Split { axis, children },
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Dock {
    /// `None` when every panel is hidden.
    pub root: Option<Node>,
    pub hidden: Vec<PanelId>,
}

impl Default for Dock {
    fn default() -> Self {
        Self::default_for(SourceLanguage::Assembly)
    }
}

impl Dock {
    pub fn default_for(language: SourceLanguage) -> Self {
        use Axis::*;
        use PanelId::*;
        let side = match language {
            SourceLanguage::Assembly => split(
                Column,
                0.24,
                vec![
                    panel(Registers, 0.42),
                    panel(Explanation, 0.22),
                    panel(Trace, 0.36),
                ],
            ),
            SourceLanguage::C => split(
                Column,
                0.27,
                vec![
                    panel(CallStack, 0.17),
                    panel(Variables, 0.37),
                    panel(Registers, 0.31),
                    panel(Output, 0.15),
                ],
            ),
        };
        let main = split(
            Column,
            0.76 - if language == SourceLanguage::C {
                0.03
            } else {
                0.
            },
            vec![
                split(Row, 0.5, vec![panel(Source, 0.5), panel(Code, 0.5)]),
                panel(Memory, 0.5),
            ],
        );
        Self {
            root: Some(Node::Split {
                axis: Row,
                children: vec![main, side],
            }),
            hidden: Vec::new(),
        }
    }

    /// Visible panels in reading order (left to right, top to bottom).
    pub fn visible(&self) -> Vec<PanelId> {
        fn walk(node: &Node, out: &mut Vec<PanelId>) {
            match node {
                Node::Panel { id } => out.push(*id),
                Node::Split { children, .. } => {
                    for child in children {
                        walk(&child.node, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            walk(root, &mut out);
        }
        out
    }

    pub fn is_visible(&self, id: PanelId) -> bool {
        self.visible().contains(&id)
    }

    /// Share of the dock's width the panel occupies, used to fit content.
    pub fn width_fraction(&self, id: PanelId) -> Option<f32> {
        fn walk(node: &Node, id: PanelId, share: f32) -> Option<f32> {
            match node {
                Node::Panel { id: found } => (*found == id).then_some(share),
                Node::Split { axis, children } => children.iter().find_map(|child| {
                    let share = if *axis == Axis::Row {
                        share * child.weight
                    } else {
                        share
                    };
                    walk(&child.node, id, share)
                }),
            }
        }
        walk(self.root.as_ref()?, id, 1.)
    }

    pub fn hide(&mut self, id: PanelId) {
        if self.remove(id) && !self.hidden.contains(&id) {
            self.hidden.push(id);
        }
    }

    pub fn show(&mut self, id: PanelId) {
        if self.is_visible(id) {
            return;
        }
        self.hidden.retain(|hidden| *hidden != id);
        let visible = self.visible();
        match id
            .anchors()
            .iter()
            .find(|(anchor, _)| visible.contains(anchor))
        {
            Some(&(anchor, edge)) => self.insert(id, anchor, edge),
            None => self.insert_at_root(id, Edge::Right),
        }
    }

    pub fn toggle(&mut self, id: PanelId) {
        if self.is_visible(id) {
            self.hide(id);
        } else {
            self.show(id);
        }
    }

    /// Move a visible or hidden panel next to `target`.
    pub fn move_panel(&mut self, id: PanelId, target: PanelId, edge: Edge) {
        if id == target || !self.is_visible(target) {
            return;
        }
        if edge == Edge::Center {
            if self.is_visible(id) {
                self.swap(id, target);
            }
            return;
        }
        self.remove(id);
        self.hidden.retain(|hidden| *hidden != id);
        self.insert(id, target, edge);
    }

    /// Move a panel to a whole edge of the dock, e.g. a full-height column.
    pub fn move_to_root(&mut self, id: PanelId, edge: Edge) {
        self.remove(id);
        self.hidden.retain(|hidden| *hidden != id);
        self.insert_at_root(id, edge);
    }

    /// Set the shares of two neighbouring children of the split at `path`.
    pub fn resize(&mut self, path: &[usize], index: usize, first: f32) {
        let Some(Node::Split { children, .. }) = self.node_mut(path) else {
            return;
        };
        if index + 1 >= children.len() || !first.is_finite() {
            return;
        }
        let pair = children[index].weight + children[index + 1].weight;
        let first = first.clamp(0., pair);
        children[index].weight = first;
        children[index + 1].weight = pair - first;
    }

    pub fn node(&self, path: &[usize]) -> Option<&Node> {
        let mut node = self.root.as_ref()?;
        for &index in path {
            match node {
                Node::Split { children, .. } => node = &children.get(index)?.node,
                Node::Panel { .. } => return None,
            }
        }
        Some(node)
    }

    fn node_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        let mut node = self.root.as_mut()?;
        for &index in path {
            match node {
                Node::Split { children, .. } => node = &mut children.get_mut(index)?.node,
                Node::Panel { .. } => return None,
            }
        }
        Some(node)
    }

    fn swap(&mut self, a: PanelId, b: PanelId) {
        fn walk(node: &mut Node, a: PanelId, b: PanelId) {
            match node {
                Node::Panel { id } if *id == a => *id = b,
                Node::Panel { id } if *id == b => *id = a,
                Node::Panel { .. } => {}
                Node::Split { children, .. } => {
                    for child in children {
                        walk(&mut child.node, a, b);
                    }
                }
            }
        }
        if let Some(root) = self.root.as_mut() {
            walk(root, a, b);
        }
    }

    fn remove(&mut self, id: PanelId) -> bool {
        fn walk(node: Node, id: PanelId) -> Option<Node> {
            match node {
                Node::Panel { id: found } if found == id => None,
                Node::Panel { .. } => Some(node),
                Node::Split { axis, children } => {
                    let children: Vec<Child> = children
                        .into_iter()
                        .filter_map(|child| {
                            walk(child.node, id).map(|node| Child {
                                weight: child.weight,
                                node,
                            })
                        })
                        .collect();
                    Some(Node::Split { axis, children })
                }
            }
        }
        let present = self.is_visible(id);
        if present {
            self.root = self.root.take().and_then(|root| walk(root, id));
            self.normalize();
        }
        present
    }

    fn insert(&mut self, id: PanelId, target: PanelId, edge: Edge) {
        fn walk(node: &mut Node, id: PanelId, target: PanelId, edge: Edge) -> bool {
            match node {
                Node::Panel { id: found } if *found == target => {
                    let new = panel(id, 0.5);
                    let old = panel(target, 0.5);
                    *node = Node::Split {
                        axis: edge.axis(),
                        children: if edge.before() {
                            vec![new, old]
                        } else {
                            vec![old, new]
                        },
                    };
                    true
                }
                Node::Panel { .. } => false,
                Node::Split { axis, children } => {
                    let same_axis = *axis == edge.axis();
                    for index in 0..children.len() {
                        if same_axis
                            && matches!(children[index].node, Node::Panel { id: found } if found == target)
                        {
                            let weight = children[index].weight / 2.;
                            children[index].weight = weight;
                            let at = if edge.before() { index } else { index + 1 };
                            children.insert(at, panel(id, weight));
                            return true;
                        }
                        if walk(&mut children[index].node, id, target, edge) {
                            return true;
                        }
                    }
                    false
                }
            }
        }
        let inserted = self
            .root
            .as_mut()
            .is_some_and(|root| walk(root, id, target, edge));
        if !inserted {
            self.insert_at_root(id, Edge::Right);
        }
        self.normalize();
    }

    fn insert_at_root(&mut self, id: PanelId, edge: Edge) {
        let edge = if edge == Edge::Center {
            Edge::Right
        } else {
            edge
        };
        self.root = Some(match self.root.take() {
            None => Node::Panel { id },
            Some(root) => {
                let new = panel(id, 0.25);
                let old = Child {
                    weight: 0.75,
                    node: root,
                };
                Node::Split {
                    axis: edge.axis(),
                    children: if edge.before() {
                        vec![new, old]
                    } else {
                        vec![old, new]
                    },
                }
            }
        });
        self.normalize();
    }

    /// Collapse single-child and empty splits, merge nested splits on the
    /// same axis and rescale every split's shares to sum to 1.
    fn normalize(&mut self) {
        fn walk(node: Node) -> Option<Node> {
            let Node::Split { axis, children } = node else {
                return Some(node);
            };
            let mut flat: Vec<Child> = Vec::new();
            for child in children {
                let weight = if child.weight.is_finite() && child.weight > 0. {
                    child.weight
                } else {
                    1.
                };
                match walk(child.node) {
                    None => {}
                    Some(Node::Split {
                        axis: inner,
                        children: grandchildren,
                    }) if inner == axis => {
                        let total: f32 = grandchildren.iter().map(|c| c.weight).sum();
                        for grandchild in grandchildren {
                            flat.push(Child {
                                weight: weight * grandchild.weight / total,
                                node: grandchild.node,
                            });
                        }
                    }
                    Some(node) => flat.push(Child { weight, node }),
                }
            }
            match flat.len() {
                0 => None,
                1 => flat.pop().map(|child| child.node),
                _ => {
                    let total: f32 = flat.iter().map(|c| c.weight).sum();
                    for child in &mut flat {
                        child.weight /= total;
                    }
                    Some(Node::Split {
                        axis,
                        children: flat,
                    })
                }
            }
        }
        self.root = self.root.take().and_then(walk);
    }

    /// Repair a layout read from disk: drop unknown and repeated panels, fix
    /// shares and make sure every panel of the language is visible or hidden.
    pub fn sanitized(mut self, language: SourceLanguage) -> Self {
        fn walk(node: Node, allowed: &[PanelId], seen: &mut Vec<PanelId>) -> Option<Node> {
            match node {
                Node::Panel { id } => {
                    if !allowed.contains(&id) || seen.contains(&id) {
                        return None;
                    }
                    seen.push(id);
                    Some(Node::Panel { id })
                }
                Node::Split { axis, children } => Some(Node::Split {
                    axis,
                    children: children
                        .into_iter()
                        .filter_map(|child| {
                            walk(child.node, allowed, seen).map(|node| Child {
                                weight: child.weight,
                                node,
                            })
                        })
                        .collect(),
                }),
            }
        }
        let allowed = PanelId::all(language);
        let mut seen = Vec::new();
        self.root = self
            .root
            .take()
            .and_then(|root| walk(root, allowed, &mut seen));
        self.normalize();
        let mut hidden = Vec::new();
        for id in self.hidden.drain(..) {
            if allowed.contains(&id) && !seen.contains(&id) && !hidden.contains(&id) {
                hidden.push(id);
            }
        }
        self.hidden = hidden;
        for &id in allowed {
            if !seen.contains(&id) && !self.hidden.contains(&id) {
                self.show(id);
            }
        }
        self
    }
}

/// Workbench preferences kept between launches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub version: u32,
    pub assembly: Dock,
    pub c: Dock,
    /// Code and data text scale, 0.8 to 2.0.
    pub zoom: f32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            assembly: Dock::default_for(SourceLanguage::Assembly),
            c: Dock::default_for(SourceLanguage::C),
            zoom: 1.,
        }
    }
}

impl Preferences {
    pub const MIN_ZOOM: f32 = 0.8;
    pub const MAX_ZOOM: f32 = 2.0;

    pub fn dock(&self, language: SourceLanguage) -> &Dock {
        match language {
            SourceLanguage::Assembly => &self.assembly,
            SourceLanguage::C => &self.c,
        }
    }
    pub fn dock_mut(&mut self, language: SourceLanguage) -> &mut Dock {
        match language {
            SourceLanguage::Assembly => &mut self.assembly,
            SourceLanguage::C => &mut self.c,
        }
    }

    /// Read preferences, falling back to defaults for a missing or damaged file.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .filter(|bytes| bytes.len() <= 1024 * 1024)
            .and_then(|bytes| serde_json::from_slice::<Preferences>(&bytes).ok())
            .map(Self::sanitized)
            .unwrap_or_default()
    }

    pub fn sanitized(self) -> Self {
        Self {
            version: 1,
            assembly: self.assembly.sanitized(SourceLanguage::Assembly),
            c: self.c.sanitized(SourceLanguage::C),
            zoom: if self.zoom.is_finite() {
                self.zoom.clamp(Self::MIN_ZOOM, Self::MAX_ZOOM)
            } else {
                1.
            },
        }
    }

    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).unwrap_or_default()
    }
}

/// `workbench.json` in the platform's per-user settings folder. The
/// `VOLE_CONFIG_DIR` variable overrides the folder, e.g. for tests.
pub fn preferences_path() -> Option<PathBuf> {
    let env = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let folder = if let Some(folder) = env("VOLE_CONFIG_DIR") {
        folder
    } else if cfg!(target_os = "macos") {
        env("HOME")?.join("Library/Application Support/Vole")
    } else if cfg!(target_os = "windows") {
        env("APPDATA")?.join("Vole")
    } else {
        env("XDG_CONFIG_HOME")
            .or_else(|| env("HOME").map(|home| home.join(".config")))?
            .join("vole")
    };
    Some(folder.join("workbench.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use PanelId::*;

    fn shares_sum_to_one(node: &Node) -> bool {
        match node {
            Node::Panel { .. } => true,
            Node::Split { children, .. } => {
                let total: f32 = children.iter().map(|c| c.weight).sum();
                children.len() >= 2
                    && (total - 1.).abs() < 1e-4
                    && children.iter().all(|c| shares_sum_to_one(&c.node))
            }
        }
    }

    #[test]
    fn defaults_show_every_panel_once() {
        for language in [SourceLanguage::Assembly, SourceLanguage::C] {
            let dock = Dock::default_for(language);
            let mut visible = dock.visible();
            visible.sort();
            let mut all = PanelId::all(language).to_vec();
            all.sort();
            assert_eq!(visible, all);
            assert!(shares_sum_to_one(dock.root.as_ref().unwrap()));
        }
    }

    #[test]
    fn hide_and_show_return_a_panel_near_its_neighbours() {
        let mut dock = Dock::default_for(SourceLanguage::C);
        dock.hide(Registers);
        assert!(!dock.is_visible(Registers));
        assert_eq!(dock.hidden, vec![Registers]);
        assert!(shares_sum_to_one(dock.root.as_ref().unwrap()));
        dock.show(Registers);
        assert!(dock.hidden.is_empty());
        let visible = dock.visible();
        let at = |id| visible.iter().position(|v| *v == id).unwrap();
        assert_eq!(at(Registers), at(Variables) + 1);
    }

    #[test]
    fn hiding_every_panel_leaves_an_empty_dock_that_can_refill() {
        let mut dock = Dock::default_for(SourceLanguage::Assembly);
        for &id in PanelId::all(SourceLanguage::Assembly) {
            dock.hide(id);
        }
        assert!(dock.root.is_none());
        assert_eq!(dock.hidden.len(), 6);
        dock.show(Memory);
        assert_eq!(dock.visible(), vec![Memory]);
        dock.show(Source);
        assert_eq!(dock.visible(), vec![Source, Memory]);
    }

    #[test]
    fn dropping_on_an_edge_splits_the_target() {
        let mut dock = Dock::default_for(SourceLanguage::Assembly);
        dock.move_panel(Registers, Source, Edge::Left);
        assert_eq!(&dock.visible()[..3], &[Registers, Source, Code]);
        assert!(shares_sum_to_one(dock.root.as_ref().unwrap()));
        dock.move_panel(Memory, Registers, Edge::Top);
        let Some(Node::Split { children, .. }) = &dock.root else {
            panic!("root split");
        };
        assert!(shares_sum_to_one(&children[0].node));
        assert_eq!(&dock.visible()[..3], &[Memory, Registers, Source]);
    }

    #[test]
    fn center_drop_swaps_and_root_drop_adds_a_full_column() {
        let mut dock = Dock::default_for(SourceLanguage::Assembly);
        dock.move_panel(Source, Trace, Edge::Center);
        assert_eq!(dock.visible()[0], Trace);
        assert_eq!(*dock.visible().last().unwrap(), Source);
        dock.move_to_root(Memory, Edge::Left);
        let Some(Node::Split { axis, children }) = &dock.root else {
            panic!("root split");
        };
        assert_eq!(*axis, Axis::Row);
        assert_eq!(children[0].node, Node::Panel { id: Memory });
        assert!(shares_sum_to_one(dock.root.as_ref().unwrap()));
    }

    #[test]
    fn resize_keeps_the_pair_total() {
        let mut dock = Dock::default_for(SourceLanguage::Assembly);
        dock.resize(&[], 0, 0.6);
        let Some(Node::Split { children, .. }) = dock.node(&[]) else {
            panic!("root split");
        };
        assert!((children[0].weight - 0.6).abs() < 1e-6);
        assert!((children[1].weight - 0.4).abs() < 1e-6);
        dock.resize(&[], 0, f32::NAN);
        dock.resize(&[7], 0, 0.5);
        assert!(shares_sum_to_one(dock.root.as_ref().unwrap()));
    }

    #[test]
    fn width_fraction_follows_row_shares() {
        let dock = Dock::default_for(SourceLanguage::Assembly);
        let registers = dock.width_fraction(Registers).unwrap();
        assert!((registers - 0.24).abs() < 1e-4);
        let source = dock.width_fraction(Source).unwrap();
        assert!((source - 0.38).abs() < 1e-4);
    }

    #[test]
    fn damaged_preferences_are_repaired() {
        let json = r#"{
            "assembly": {"root": {"kind": "split", "axis": "row", "children": [
                {"weight": -3, "node": {"kind": "panel", "id": "source"}},
                {"weight": 1, "node": {"kind": "panel", "id": "source"}},
                {"weight": 1, "node": {"kind": "panel", "id": "variables"}},
                {"weight": 1, "node": {"kind": "split", "axis": "row", "children": []}}
            ]}, "hidden": ["trace", "trace", "output"]},
            "zoom": 9
        }"#;
        let preferences = serde_json::from_str::<Preferences>(json)
            .unwrap()
            .sanitized();
        let dock = &preferences.assembly;
        assert_eq!(dock.visible()[0], Source);
        assert_eq!(dock.hidden, vec![Trace]);
        let mut all: Vec<_> = dock.visible();
        all.extend(&dock.hidden);
        all.sort();
        let mut expected = PanelId::all(SourceLanguage::Assembly).to_vec();
        expected.sort();
        assert_eq!(all, expected);
        assert!(shares_sum_to_one(dock.root.as_ref().unwrap()));
        assert_eq!(preferences.zoom, Preferences::MAX_ZOOM);
        assert_eq!(preferences.c, Dock::default_for(SourceLanguage::C));
    }

    #[test]
    fn preferences_round_trip_through_a_file() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("workbench.json");
        assert_eq!(Preferences::load(&path), Preferences::default());
        let mut preferences = Preferences::default();
        preferences.c.hide(Output);
        preferences.assembly.move_panel(Trace, Source, Edge::Bottom);
        preferences.zoom = 1.5;
        std::fs::write(&path, preferences.to_json()).unwrap();
        assert_eq!(Preferences::load(&path), preferences);
        std::fs::write(&path, b"{ not json").unwrap();
        assert_eq!(Preferences::load(&path), Preferences::default());
    }
}
