//! Variable locations, type names and value formatting.
use crate::{Target, UnwoundFrame, abi, address_mask, unwind::little_endian};
use vole_core::{
    Architecture, DebugInfo,
    debug::{
        BaseEncoding, Function, Location, Member, Type, TypeKind, ValueText, Variable,
        VariableKind, VariableView,
    },
};

/// Elements shown as children of an array.
const MAX_ELEMENTS: u64 = 64;
/// Characters shown in string previews.
const MAX_STRING: usize = 64;
/// Nesting depth for members, elements and pointer targets.
const MAX_DEPTH: usize = 6;
/// Characters in an aggregate summary before it is elided.
const SUMMARY_CHARS: usize = 48;

pub const PROLOGUE: &str = "Not available until the function prologue finishes";
pub const NOT_REACHED: &str = "Declared later; not reached yet";
pub const OPTIMIZED_OUT: &str = "Optimized out at this point";
pub const UNKNOWN_TYPE: &str = "Unknown type";

enum Place {
    Memory(u64),
    Value(u64),
    Unavailable(String),
}

pub struct Evaluator<'a> {
    pub debug: &'a DebugInfo,
    pub architecture: Architecture,
    pub target: &'a dyn Target,
}

impl Evaluator<'_> {
    /// Parameters first, then locals whose scope contains the frame's lookup PC.
    pub fn frame_variables(
        &self,
        index: usize,
        frame: &UnwoundFrame,
        function: &Function,
    ) -> Vec<VariableView> {
        let pc = frame.lookup_pc;
        let in_prologue = index == 0 && function.prologue_end.is_some_and(|end| pc < end);
        let current_line = (index == 0)
            .then(|| self.debug.row_for_address(pc))
            .flatten()
            .filter(|row| self.debug.is_user_file(row.file) && row.line != 0)
            .map(|row| row.line);
        let parameters = function.variables.iter().filter(|v| v.parameter);
        let locals = function.variables.iter().filter(|v| {
            !v.parameter
                && (v.scope.is_empty()
                    || v.scope
                        .iter()
                        .any(|(start, end)| (*start..*end).contains(&pc)))
        });
        parameters
            .chain(locals)
            .map(|variable| {
                let kind = if variable.parameter {
                    VariableKind::Parameter
                } else {
                    VariableKind::Local
                };
                let frame_relative = matches!(
                    variable.location,
                    Location::FrameOffset(_) | Location::RegisterOffset { .. }
                );
                let place = if in_prologue && frame_relative {
                    Place::Unavailable(PROLOGUE.into())
                } else if !variable.parameter
                    && !matches!(variable.location, Location::Address(_))
                    && self.debug.is_user_file(variable.decl_file)
                    && current_line.is_some_and(|line| line < variable.decl_line)
                {
                    Place::Unavailable(NOT_REACHED.into())
                } else {
                    self.place(frame, Some(function), &variable.location, 0)
                };
                self.view(&variable.name, kind, variable.type_id, place, 0)
            })
            .collect()
    }

    /// User-document globals (runtime globals are hidden).
    pub fn globals(&self, frame: &UnwoundFrame) -> Vec<VariableView> {
        self.debug
            .globals
            .iter()
            .filter(|v: &&Variable| self.debug.is_user_file(v.decl_file))
            .map(|variable| {
                let place = self.place(frame, None, &variable.location, 0);
                self.view(
                    &variable.name,
                    VariableKind::Global,
                    variable.type_id,
                    place,
                    0,
                )
            })
            .collect()
    }

    fn mask(&self) -> u64 {
        address_mask(self.architecture)
    }

    fn frame_base(
        &self,
        frame: &UnwoundFrame,
        function: Option<&Function>,
        location: Option<&Location>,
        depth: usize,
    ) -> Result<u64, String> {
        let location = location
            .or_else(|| function.and_then(|f| f.frame_base.as_ref()))
            .ok_or_else(|| "Unsupported location: the function has no frame base".to_string())?;
        match location {
            Location::Register(register) => frame
                .register(self.architecture, self.target, register)
                .ok_or_else(|| format!("Frame base register {register} is not available")),
            Location::RegisterOffset { register, offset } => frame
                .register(self.architecture, self.target, register)
                .map(|value| value.wrapping_add_signed(*offset) & self.mask())
                .ok_or_else(|| format!("Frame base register {register} is not available")),
            Location::Address(address) => Ok(*address),
            Location::List(ranges) if depth < 2 => ranges
                .iter()
                .find(|range| (range.start..range.end).contains(&frame.lookup_pc))
                .ok_or_else(|| OPTIMIZED_OUT.to_string())
                .and_then(|range| {
                    self.frame_base(frame, function, Some(&range.location), depth + 1)
                }),
            Location::Unavailable(reason) => Err(reason.clone()),
            _ => Err("Unsupported location: frame base".into()),
        }
    }

    fn place(
        &self,
        frame: &UnwoundFrame,
        function: Option<&Function>,
        location: &Location,
        depth: usize,
    ) -> Place {
        match location {
            Location::FrameOffset(offset) => match self.frame_base(frame, function, None, 0) {
                Ok(base) => Place::Memory(base.wrapping_add_signed(*offset) & self.mask()),
                Err(reason) => Place::Unavailable(reason),
            },
            Location::Address(address) => Place::Memory(*address),
            Location::RegisterOffset { register, offset } => {
                match frame.register(self.architecture, self.target, register) {
                    Some(value) => Place::Memory(value.wrapping_add_signed(*offset) & self.mask()),
                    None => Place::Unavailable(format!(
                        "Register {register} is not preserved in this caller frame"
                    )),
                }
            }
            Location::Register(register) => {
                match frame.register(self.architecture, self.target, register) {
                    Some(value) => Place::Value(value),
                    None => Place::Unavailable(format!(
                        "The value was in register {register}, which this caller frame does not preserve"
                    )),
                }
            }
            Location::List(ranges) if depth < 2 => ranges
                .iter()
                .find(|range| (range.start..range.end).contains(&frame.lookup_pc))
                .map(|range| self.place(frame, function, &range.location, depth + 1))
                .unwrap_or_else(|| Place::Unavailable(OPTIMIZED_OUT.into())),
            Location::List(_) => Place::Unavailable("Unsupported location: nested list".into()),
            Location::Unavailable(reason) => Place::Unavailable(reason.clone()),
        }
    }

    /// Strip typedefs and qualifiers.
    fn resolve(&self, mut id: Option<usize>) -> Option<&Type> {
        for _ in 0..32 {
            let ty = self.debug.types.get(id?)?;
            match &ty.kind {
                TypeKind::Typedef(inner) | TypeKind::Const(inner) | TypeKind::Volatile(inner) => {
                    id = Some((*inner)?);
                }
                _ => return Some(ty),
            }
        }
        None
    }

    fn size_of(&self, id: Option<usize>) -> Option<u64> {
        let own = self.debug.types.get(id?)?;
        if own.size > 0 {
            return Some(own.size);
        }
        let ty = self.resolve(id)?;
        if ty.size > 0 {
            return Some(ty.size);
        }
        match &ty.kind {
            TypeKind::Pointer(_) => Some(abi(self.architecture).pointer_bytes),
            TypeKind::Array {
                element,
                count: Some(count),
            } => self.size_of(*element)?.checked_mul(*count),
            _ => None,
        }
    }

    /// C spelling of a type, e.g. `const char *`, `int [8]`, `struct point`.
    pub fn type_name(&self, id: Option<usize>) -> String {
        self.type_name_depth(id, 0)
    }

    fn type_name_depth(&self, id: Option<usize>, depth: usize) -> String {
        if depth > 16 {
            return "…".into();
        }
        let Some(id) = id else {
            return "void".into();
        };
        let Some(ty) = self.debug.types.get(id) else {
            return "<unknown type>".into();
        };
        let tagged = |tag: &str| {
            if ty.name.is_empty() {
                format!("{tag} <anonymous>")
            } else if ty.name.starts_with(&format!("{tag} ")) {
                ty.name.clone()
            } else {
                format!("{tag} {}", ty.name)
            }
        };
        match &ty.kind {
            TypeKind::Void => "void".into(),
            TypeKind::Base(_) | TypeKind::Typedef(_) => ty.name.clone(),
            TypeKind::Struct(_) => tagged("struct"),
            TypeKind::Union(_) => tagged("union"),
            TypeKind::Enum(_) => tagged("enum"),
            TypeKind::Function => {
                if ty.name.is_empty() {
                    "function".into()
                } else {
                    ty.name.clone()
                }
            }
            TypeKind::Unsupported(_) => {
                if ty.name.is_empty() {
                    "<unsupported type>".into()
                } else {
                    ty.name.clone()
                }
            }
            TypeKind::Pointer(inner) => {
                let inner = self.type_name_depth(*inner, depth + 1);
                if inner.ends_with('*') {
                    format!("{inner}*")
                } else {
                    format!("{inner} *")
                }
            }
            TypeKind::Const(inner) | TypeKind::Volatile(inner) => {
                let qualifier = if matches!(ty.kind, TypeKind::Const(_)) {
                    "const"
                } else {
                    "volatile"
                };
                let inner_is_pointer = inner
                    .and_then(|i| self.debug.types.get(i))
                    .is_some_and(|t| matches!(t.kind, TypeKind::Pointer(_)));
                let name = self.type_name_depth(*inner, depth + 1);
                if inner_is_pointer {
                    format!("{name}{qualifier}")
                } else {
                    format!("{qualifier} {name}")
                }
            }
            TypeKind::Array { .. } => {
                let mut dimensions = String::new();
                let mut current = Some(id);
                let mut guard = 0;
                while let Some(TypeKind::Array { element, count }) = current
                    .and_then(|c| self.debug.types.get(c))
                    .map(|t| &t.kind)
                {
                    match count {
                        Some(count) => dimensions.push_str(&format!("[{count}]")),
                        None => dimensions.push_str("[]"),
                    }
                    current = *element;
                    guard += 1;
                    if guard > 16 {
                        break;
                    }
                }
                format!("{} {dimensions}", self.type_name_depth(current, depth + 1))
            }
        }
    }

    fn view(
        &self,
        name: &str,
        kind: VariableKind,
        type_id: Option<usize>,
        place: Place,
        depth: usize,
    ) -> VariableView {
        let type_name = if type_id.is_none() {
            "<unknown type>".to_string()
        } else {
            self.type_name(type_id)
        };
        let size = self.size_of(type_id).unwrap_or(0);
        let address = match place {
            Place::Memory(address) => Some(address),
            _ => None,
        };
        let (value, children) = match place {
            Place::Unavailable(reason) => (ValueText::Unavailable(reason), Vec::new()),
            _ if type_id.is_none() => (ValueText::Unavailable(UNKNOWN_TYPE.into()), Vec::new()),
            place => self.render(name, type_id, &place, depth),
        };
        VariableView {
            name: name.to_string(),
            type_name,
            kind,
            value,
            address,
            size,
            children,
            changed: false,
        }
    }

    fn scalar(&self, place: &Place, size: u64) -> Result<u64, String> {
        if size == 0 || size > 8 {
            return Err(format!("Unsupported location: {size}-byte scalar"));
        }
        match place {
            Place::Memory(address) => self
                .target
                .read(*address, size as usize)
                .map(|bytes| little_endian(&bytes))
                .ok_or_else(|| unmapped(*address)),
            Place::Value(value) => Ok(truncate(*value, size)),
            Place::Unavailable(reason) => Err(reason.clone()),
        }
    }

    fn render(
        &self,
        name: &str,
        type_id: Option<usize>,
        place: &Place,
        depth: usize,
    ) -> (ValueText, Vec<VariableView>) {
        let Some(ty) = self.resolve(type_id) else {
            return (ValueText::Unavailable(UNKNOWN_TYPE.into()), Vec::new());
        };
        let size = self.size_of(type_id).unwrap_or(ty.size);
        let unavailable = |reason: String| (ValueText::Unavailable(reason), Vec::new());
        match &ty.kind {
            TypeKind::Base(encoding) => match self.scalar(place, size) {
                Ok(raw) => (
                    ValueText::Value(format_base(*encoding, raw, size)),
                    Vec::new(),
                ),
                Err(reason) => unavailable(reason),
            },
            TypeKind::Enum(enumerators) => match self.scalar(place, size) {
                Ok(raw) => {
                    let signed = sign_extend(raw, size);
                    let text = enumerators
                        .iter()
                        .find(|(_, value)| *value == signed || *value as u64 == raw)
                        .map(|(name, value)| format!("{name} ({value})"))
                        .unwrap_or_else(|| signed.to_string());
                    (ValueText::Value(text), Vec::new())
                }
                Err(reason) => unavailable(reason),
            },
            TypeKind::Pointer(pointee) => match self.scalar(place, size) {
                Ok(pointer) => self.render_pointer(name, *pointee, pointer, depth),
                Err(reason) => unavailable(reason),
            },
            TypeKind::Array { element, count } => {
                let Place::Memory(address) = place else {
                    return unavailable("Unsupported location: array outside memory".into());
                };
                self.render_array(*address, *element, *count, depth)
            }
            TypeKind::Struct(members) | TypeKind::Union(members) => {
                let Place::Memory(address) = place else {
                    return unavailable("Unsupported location: aggregate outside memory".into());
                };
                self.render_members(*address, members, depth)
            }
            TypeKind::Void => (ValueText::Value("void".into()), Vec::new()),
            TypeKind::Function => match place {
                Place::Memory(address) => (
                    ValueText::Value(format!("function at 0x{address:x}")),
                    Vec::new(),
                ),
                _ => unavailable("Unsupported location: function".into()),
            },
            TypeKind::Unsupported(reason) => unavailable(reason.clone()),
            TypeKind::Typedef(_) | TypeKind::Const(_) | TypeKind::Volatile(_) => {
                unavailable(UNKNOWN_TYPE.into())
            }
        }
    }

    fn is_char(&self, id: Option<usize>) -> bool {
        self.resolve(id).is_some_and(|ty| {
            matches!(
                ty.kind,
                TypeKind::Base(BaseEncoding::SignedChar | BaseEncoding::UnsignedChar)
            ) && ty.size <= 1
        })
    }

    fn render_pointer(
        &self,
        name: &str,
        pointee: Option<usize>,
        pointer: u64,
        depth: usize,
    ) -> (ValueText, Vec<VariableView>) {
        if pointer == 0 {
            return (ValueText::Value("NULL".into()), Vec::new());
        }
        let resolved = self.resolve(pointee);
        if resolved.is_some_and(|ty| matches!(ty.kind, TypeKind::Function)) {
            let text = match self
                .debug
                .functions
                .iter()
                .find(|function| function.low_pc == pointer)
            {
                Some(function) => format!("0x{pointer:x} <{}>", function.name),
                None => format!("0x{pointer:x}"),
            };
            return (ValueText::Value(text), Vec::new());
        }
        if self.is_char(pointee) {
            let text = match self.string_preview(pointer, MAX_STRING) {
                Some(preview) => format!("0x{pointer:x} → {preview}"),
                None => format!("0x{pointer:x} (unmapped memory)"),
            };
            return (ValueText::Value(text), Vec::new());
        }
        let mut children = Vec::new();
        let pointee_known = resolved.is_some_and(|ty| {
            !matches!(
                ty.kind,
                TypeKind::Void | TypeKind::Unsupported(_) | TypeKind::Function
            )
        });
        if pointee_known && depth < MAX_DEPTH {
            children.push(self.view(
                &format!("*{name}"),
                VariableKind::Element,
                pointee,
                Place::Memory(pointer),
                depth + 1,
            ));
        }
        (ValueText::Value(format!("0x{pointer:x}")), children)
    }

    /// Quoted, escaped C string at `address`, bounded by `limit` bytes.
    /// `None` when the first byte is unmapped.
    fn string_preview(&self, address: u64, limit: usize) -> Option<String> {
        let mut text = String::from("\"");
        for index in 0..limit as u64 {
            let Some(byte) = self.target.read(address.wrapping_add(index), 1) else {
                if index == 0 {
                    return None;
                }
                text.push_str("\" (unmapped memory follows)");
                return Some(text);
            };
            if byte[0] == 0 {
                text.push('"');
                return Some(text);
            }
            text.push_str(&escape(byte[0], '"'));
        }
        text.push_str("\"…");
        Some(text)
    }

    fn render_array(
        &self,
        address: u64,
        element: Option<usize>,
        count: Option<u64>,
        depth: usize,
    ) -> (ValueText, Vec<VariableView>) {
        let Some(count) = count else {
            return (
                ValueText::Value(format!("array of unknown length at 0x{address:x}")),
                Vec::new(),
            );
        };
        let Some(stride) = self.size_of(element).filter(|size| *size > 0) else {
            return (ValueText::Unavailable(UNKNOWN_TYPE.into()), Vec::new());
        };
        let shown = count.min(MAX_ELEMENTS);
        let children: Vec<VariableView> = if depth < MAX_DEPTH {
            (0..shown)
                .map(|index| {
                    let place = Place::Memory(address.wrapping_add(index * stride) & self.mask());
                    self.view(
                        &format!("[{index}]"),
                        VariableKind::Element,
                        element,
                        place,
                        depth + 1,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        if self.is_char(element) {
            let bytes = (count as usize).min(MAX_STRING);
            return match self.target.read(address, bytes) {
                Some(bytes) => {
                    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
                    let mut text: String = String::from("\"");
                    for byte in &bytes[..end] {
                        text.push_str(&escape(*byte, '"'));
                    }
                    text.push('"');
                    if end == bytes.len() && (count as usize) > bytes.len() {
                        text.push('…');
                    }
                    (ValueText::Value(text), children)
                }
                None => (ValueText::Unavailable(unmapped(address)), children),
            };
        }
        if children.is_empty() && count > 0 {
            return (ValueText::Value("{…}".into()), children);
        }
        let summary = summarize(
            children.iter().map(|child| text_of(&child.value)),
            count as usize,
        );
        (ValueText::Value(summary), children)
    }

    fn render_members(
        &self,
        address: u64,
        members: &[Member],
        depth: usize,
    ) -> (ValueText, Vec<VariableView>) {
        if depth >= MAX_DEPTH {
            return (ValueText::Value("{…}".into()), Vec::new());
        }
        let children: Vec<VariableView> = members
            .iter()
            .map(|member| {
                let place = Place::Memory(address.wrapping_add(member.offset) & self.mask());
                let name = if member.name.is_empty() {
                    "<anonymous>"
                } else {
                    &member.name
                };
                self.view(name, VariableKind::Member, member.type_id, place, depth + 1)
            })
            .collect();
        let summary = summarize(
            children
                .iter()
                .map(|child| format!("{} = {}", child.name, text_of(&child.value))),
            children.len(),
        );
        (ValueText::Value(summary), children)
    }
}

fn unmapped(address: u64) -> String {
    format!("Memory at 0x{address:x} is not mapped")
}

fn text_of(value: &ValueText) -> String {
    match value {
        ValueText::Value(text) => text.clone(),
        ValueText::Unavailable(_) => "?".into(),
    }
}

fn summarize(items: impl Iterator<Item = String>, total: usize) -> String {
    let mut summary = String::from("{");
    let mut shown = 0;
    for item in items {
        if shown > 0 && summary.chars().count() + item.chars().count() > SUMMARY_CHARS {
            break;
        }
        if shown > 0 {
            summary.push_str(", ");
        }
        summary.push_str(&item);
        shown += 1;
    }
    if shown < total {
        summary.push_str(if shown > 0 { ", …" } else { "…" });
    }
    summary.push('}');
    summary
}

fn truncate(value: u64, size: u64) -> u64 {
    if size >= 8 {
        value
    } else {
        value & ((1_u64 << (size * 8)) - 1)
    }
}

fn sign_extend(raw: u64, size: u64) -> i64 {
    if size == 0 || size >= 8 {
        return raw as i64;
    }
    let shift = 64 - size * 8;
    ((raw << shift) as i64) >> shift
}

/// C escape for one byte inside `quote` delimiters.
fn escape(byte: u8, quote: char) -> String {
    match byte {
        b'\n' => "\\n".into(),
        b'\t' => "\\t".into(),
        b'\r' => "\\r".into(),
        0 => "\\0".into(),
        b'\\' => "\\\\".into(),
        byte if byte as char == quote => format!("\\{quote}"),
        0x20..=0x7e => (byte as char).to_string(),
        byte => format!("\\x{byte:02x}"),
    }
}

fn format_base(encoding: BaseEncoding, raw: u64, size: u64) -> String {
    match encoding {
        BaseEncoding::Signed => sign_extend(raw, size).to_string(),
        BaseEncoding::Unsigned => raw.to_string(),
        BaseEncoding::SignedChar => {
            format!("{} '{}'", sign_extend(raw, size), escape(raw as u8, '\''))
        }
        BaseEncoding::UnsignedChar => format!("{raw} '{}'", escape(raw as u8, '\'')),
        BaseEncoding::Boolean => match raw {
            0 => "false".into(),
            1 => "true".into(),
            other => format!("true ({other})"),
        },
        BaseEncoding::Float | BaseEncoding::Other => {
            format!("0x{raw:0width$x}", width = (size * 2) as usize)
        }
    }
}

/// Flag values that differ from the previous observation of the same variable.
pub fn mark_changed(current: &mut [VariableView], previous: &[VariableView]) {
    for view in current {
        let Some(old) = previous
            .iter()
            .find(|old| old.name == view.name && old.kind == view.kind)
        else {
            continue;
        };
        view.changed = matches!(
            (&view.value, &old.value),
            (ValueText::Value(new), ValueText::Value(before)) if new != before
        );
        mark_changed(&mut view.children, &old.children);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_and_formats_characters() {
        assert_eq!(format_base(BaseEncoding::SignedChar, 65, 1), "65 'A'");
        assert_eq!(format_base(BaseEncoding::SignedChar, 0xff, 1), "-1 '\\xff'");
        assert_eq!(format_base(BaseEncoding::SignedChar, 10, 1), "10 '\\n'");
        assert_eq!(format_base(BaseEncoding::SignedChar, 39, 1), "39 '\\''");
        assert_eq!(format_base(BaseEncoding::Signed, 0xffff_fffe, 4), "-2");
        assert_eq!(
            format_base(BaseEncoding::Unsigned, 0xffff_fffe, 4),
            "4294967294"
        );
        assert_eq!(format_base(BaseEncoding::Boolean, 1, 1), "true");
    }

    #[test]
    fn summaries_elide_long_aggregates() {
        let short = summarize(["3", "1", "4"].into_iter().map(String::from), 3);
        assert_eq!(short, "{3, 1, 4}");
        let long = summarize((0..64).map(|n| n.to_string()), 64);
        assert!(long.ends_with(", …}"), "{long}");
        let partial = summarize(["3", "1"].into_iter().map(String::from), 10);
        assert_eq!(partial, "{3, 1, …}");
    }
}
