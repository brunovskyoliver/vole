//! Converts DWARF 5 from the linked guest ELF into [`vole_core::DebugInfo`].
use gimli::{
    AttributeValue, BaseAddresses, CieOrFde, ColumnType, DebugFrame, DwAt, EndianSlice,
    EntriesTreeNode, Expression, LittleEndian, Operation, UnitOffset, UnitRef, UnwindContext,
    UnwindSection, UnwindTable, constants,
};
use object::{Object, ObjectSection};
use std::collections::HashMap;
use vole_core::{
    Architecture,
    debug::{
        BaseEncoding, CfaRule, DebugInfo, Function, LineRow, Location, LocationRange, Member,
        RegisterRule, SourceFile, Type, TypeKind, UnwindRow, Variable,
    },
};

type Slice<'a> = EndianSlice<'a, LittleEndian>;
type Result<T> = std::result::Result<T, gimli::Error>;

/// Name of the user's document inside the compiler's private directory.
pub(crate) const USER_FILE: &str = "main.c";
/// File entry used when DWARF does not name a file.
const UNKNOWN_FILE: &str = "<unknown>";

/// Machine register name for a DWARF register number, per the C environment contract.
pub(crate) fn register_name(architecture: Architecture, number: u16) -> Option<String> {
    let name = match architecture {
        Architecture::Arm64 => match number {
            0..=30 => format!("x{number}"),
            31 => "sp".into(),
            _ => return None,
        },
        Architecture::Arm32 => match number {
            0..=14 => format!("r{number}"),
            15 => "pc".into(),
            _ => return None,
        },
        Architecture::X86 => match number {
            0..=7 => {
                ["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"][number as usize].into()
            }
            8 => "eip".into(),
            _ => return None,
        },
        Architecture::X64 => match number {
            0..=7 => {
                ["rax", "rdx", "rcx", "rbx", "rsi", "rdi", "rbp", "rsp"][number as usize].into()
            }
            8..=15 => format!("r{number}"),
            16 => "rip".into(),
            _ => return None,
        },
        Architecture::Vole => return None,
    };
    Some(name)
}

/// A function symbol from the ELF symbol table: name, address and size.
pub(crate) struct FunctionSymbol {
    pub name: String,
    pub address: u64,
    pub size: u64,
}

struct Builder<'a> {
    architecture: Architecture,
    dwarf: &'a gimli::Dwarf<Slice<'a>>,
    info: DebugInfo,
    /// Global type index for each (unit index, DIE offset).
    type_ids: HashMap<(usize, usize), usize>,
    void_type: Option<usize>,
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Extract source-level debug metadata. `info` supplies build fields (triple,
/// settings, flags); everything else is read from the image.
pub(crate) fn extract(
    image: &object::File<'_>,
    architecture: Architecture,
    mut info: DebugInfo,
    function_symbols: &[FunctionSymbol],
) -> std::result::Result<DebugInfo, String> {
    let load = |id: gimli::SectionId| -> Result<Slice<'_>> {
        let data = image
            .section_by_name(id.name())
            .and_then(|section| section.data().ok())
            .unwrap_or(&[]);
        Ok(EndianSlice::new(data, LittleEndian))
    };
    let dwarf = gimli::Dwarf::load(load).map_err(|e| format!("Cannot read DWARF: {e}"))?;
    info.files = vec![SourceFile {
        name: USER_FILE.into(),
        user: true,
    }];
    let mut builder = Builder {
        architecture,
        dwarf: &dwarf,
        info,
        type_ids: HashMap::new(),
        void_type: None,
    };
    builder
        .units()
        .map_err(|e| format!("Cannot read DWARF debug information: {e}"))?;
    let frame = image
        .section_by_name(".debug_frame")
        .and_then(|section| section.data().ok())
        .unwrap_or(&[]);
    builder
        .unwind(frame, image.is_64())
        .map_err(|e| format!("Cannot read DWARF call frames: {e}"))?;
    builder.symbol_functions(function_symbols);
    let mut info = builder.info;
    info.lines
        .sort_by_key(|row| (row.address, !row.end_sequence));
    info.functions.sort_by_key(|function| function.low_pc);
    info.globals.sort_by_key(|global| global.decl_file != 0);
    info.unwind.sort_by_key(|row| row.start);
    for function in &mut info.functions {
        function.prologue_end = info
            .lines
            .iter()
            .find(|row| row.prologue_end && !row.end_sequence && function.contains(row.address))
            .map(|row| row.address);
    }
    Ok(info)
}

impl<'a> Builder<'a> {
    fn file_index(&mut self, path: &str) -> u32 {
        let name = file_name(path);
        if let Some(index) = self.info.files.iter().position(|file| file.name == name) {
            return index as u32;
        }
        self.info.files.push(SourceFile {
            name: name.into(),
            user: false,
        });
        (self.info.files.len() - 1) as u32
    }

    fn units(&mut self) -> Result<()> {
        let mut headers = self.dwarf.units();
        let mut unit_index = 0;
        while let Some(header) = headers.next()? {
            let unit = self.dwarf.unit(header)?;
            let unit = unit.unit_ref(self.dwarf);
            let files = self.lines(unit)?;
            let mut tree = unit.entries_tree(None)?;
            let root = tree.root()?;
            let entry = root.entry();
            if let Some(producer) = entry.attr_value(constants::DW_AT_producer)? {
                let producer = unit.attr_string(producer)?.to_string_lossy().into_owned();
                let name = match entry.attr_value(constants::DW_AT_name)? {
                    Some(name) => unit.attr_string(name)?.to_string_lossy().into_owned(),
                    None => String::new(),
                };
                if self.info.producer.is_empty() || file_name(&name) == USER_FILE {
                    self.info.producer = producer;
                }
            }
            let context = UnitContext {
                unit,
                index: unit_index,
                files: &files,
            };
            let mut children = root.children();
            while let Some(child) = children.next()? {
                match child.entry().tag() {
                    constants::DW_TAG_subprogram => self.function(&context, child)?,
                    constants::DW_TAG_variable => {
                        let entry = child.entry();
                        if entry.attr_value(constants::DW_AT_declaration)?.is_none() {
                            let global = self.variable(&context, entry, false, &[])?;
                            self.info.globals.push(global);
                        }
                    }
                    _ => {}
                }
            }
            unit_index += 1;
        }
        Ok(())
    }

    /// Reads the unit's line table and returns its file-index map.
    fn lines(&mut self, unit: UnitRef<'_, Slice<'a>>) -> Result<HashMap<u64, u32>> {
        let mut files = HashMap::new();
        let Some(program) = unit.line_program.clone() else {
            return Ok(files);
        };
        let header = program.header();
        let count = header.file_names().len() as u64;
        // DWARF 5 numbers files from 0; older versions from 1.
        for index in 0..=count {
            if let Some(file) = header.file(index) {
                let path = unit
                    .attr_string(file.path_name())?
                    .to_string_lossy()
                    .into_owned();
                files.insert(index, self.file_index(&path));
            }
        }
        let mut rows = program.rows();
        while let Some((_, row)) = rows.next_row()? {
            let file = match files.get(&row.file_index()) {
                Some(file) => *file,
                None => self.file_index(UNKNOWN_FILE),
            };
            self.info.lines.push(LineRow {
                address: row.address(),
                file,
                line: row.line().map_or(0, |line| line.get() as u32),
                column: match row.column() {
                    ColumnType::LeftEdge => 0,
                    ColumnType::Column(column) => column.get() as u32,
                },
                is_stmt: row.is_stmt(),
                prologue_end: row.prologue_end(),
                end_sequence: row.end_sequence(),
            });
        }
        Ok(files)
    }

    /// Attribute value, following abstract-origin and specification links.
    fn attribute(
        &self,
        unit: UnitRef<'_, Slice<'a>>,
        entry: &gimli::DebuggingInformationEntry<'_, '_, Slice<'a>>,
        name: DwAt,
    ) -> Result<Option<AttributeValue<Slice<'a>>>> {
        if let Some(value) = entry.attr_value(name)? {
            return Ok(Some(value));
        }
        let mut origin = None;
        for link in [
            constants::DW_AT_abstract_origin,
            constants::DW_AT_specification,
        ] {
            if let Some(AttributeValue::UnitRef(offset)) = entry.attr_value(link)? {
                origin = Some(offset);
                break;
            }
        }
        // Follow a short chain of links within the unit.
        for _ in 0..4 {
            let Some(offset) = origin.take() else { break };
            let target = unit.entry(offset)?;
            if let Some(value) = target.attr_value(name)? {
                return Ok(Some(value));
            }
            for link in [
                constants::DW_AT_abstract_origin,
                constants::DW_AT_specification,
            ] {
                if let Some(AttributeValue::UnitRef(next)) = target.attr_value(link)? {
                    origin = Some(next);
                    break;
                }
            }
        }
        Ok(None)
    }

    fn string(
        &self,
        unit: UnitRef<'_, Slice<'a>>,
        entry: &gimli::DebuggingInformationEntry<'_, '_, Slice<'a>>,
        name: DwAt,
    ) -> Result<Option<String>> {
        match self.attribute(unit, entry, name)? {
            Some(value) => Ok(Some(
                unit.attr_string(value)?.to_string_lossy().into_owned(),
            )),
            None => Ok(None),
        }
    }

    fn unsigned(
        &self,
        unit: UnitRef<'_, Slice<'a>>,
        entry: &gimli::DebuggingInformationEntry<'_, '_, Slice<'a>>,
        name: DwAt,
    ) -> Result<Option<u64>> {
        Ok(self
            .attribute(unit, entry, name)?
            .and_then(|value| value.udata_value()))
    }

    fn decl_file(
        &mut self,
        context: &UnitContext<'_, '_, 'a>,
        entry: &gimli::DebuggingInformationEntry<'_, '_, Slice<'a>>,
    ) -> Result<u32> {
        let index = match self.attribute(context.unit, entry, constants::DW_AT_decl_file)? {
            Some(AttributeValue::FileIndex(index)) => Some(index),
            Some(value) => value.udata_value(),
            None => None,
        };
        Ok(index
            .and_then(|index| context.files.get(&index).copied())
            .unwrap_or_else(|| self.file_index(UNKNOWN_FILE)))
    }

    fn ranges(
        &self,
        unit: UnitRef<'_, Slice<'a>>,
        entry: &gimli::DebuggingInformationEntry<'_, '_, Slice<'a>>,
    ) -> Result<Vec<(u64, u64)>> {
        let mut result = Vec::new();
        let mut ranges = unit.die_ranges(entry)?;
        while let Some(range) = ranges.next()? {
            if range.end > range.begin {
                result.push((range.begin, range.end));
            }
        }
        result.sort_unstable();
        Ok(result)
    }

    fn function(
        &mut self,
        context: &UnitContext<'_, '_, 'a>,
        node: EntriesTreeNode<'_, '_, '_, Slice<'a>>,
    ) -> Result<()> {
        let unit = context.unit;
        let entry = node.entry();
        if entry.attr_value(constants::DW_AT_declaration)?.is_some() {
            return Ok(());
        }
        let ranges = self.ranges(unit, entry)?;
        let (Some(low_pc), Some(high_pc)) = (
            ranges.iter().map(|r| r.0).min(),
            ranges.iter().map(|r| r.1).max(),
        ) else {
            // Abstract instances of inlined functions have no code of their own.
            return Ok(());
        };
        let name = self
            .string(unit, entry, constants::DW_AT_name)?
            .unwrap_or_else(|| format!("<function at 0x{low_pc:x}>"));
        let decl_file = self.decl_file(context, entry)?;
        let decl_line = self
            .unsigned(unit, entry, constants::DW_AT_decl_line)?
            .unwrap_or(0) as u32;
        let frame_base = match entry.attr_value(constants::DW_AT_frame_base)? {
            Some(AttributeValue::Exprloc(expression)) => Some(self.location(unit, expression)),
            Some(value) => Some(self.location_list(unit, value)?),
            None => None,
        };
        let return_type = Some(match self.attribute(unit, entry, constants::DW_AT_type)? {
            Some(AttributeValue::UnitRef(offset)) => self.resolve_type(context, offset)?,
            _ => self.void(),
        });
        let mut variables = Vec::new();
        self.variables(context, node, &[], &mut variables)?;
        self.info.functions.push(Function {
            name,
            low_pc,
            high_pc,
            decl_file,
            decl_line,
            frame_base,
            return_type,
            variables,
            prologue_end: None,
            user: decl_file == 0,
        });
        Ok(())
    }

    fn variables(
        &mut self,
        context: &UnitContext<'_, '_, 'a>,
        node: EntriesTreeNode<'_, '_, '_, Slice<'a>>,
        scope: &[(u64, u64)],
        output: &mut Vec<Variable>,
    ) -> Result<()> {
        let mut children = node.children();
        while let Some(child) = children.next()? {
            let entry = child.entry();
            match entry.tag() {
                constants::DW_TAG_formal_parameter => {
                    output.push(self.variable(context, entry, true, scope)?)
                }
                constants::DW_TAG_variable => {
                    if entry.attr_value(constants::DW_AT_declaration)?.is_none() {
                        output.push(self.variable(context, entry, false, scope)?)
                    }
                }
                constants::DW_TAG_lexical_block => {
                    let ranges = self.ranges(context.unit, entry)?;
                    let inner = if ranges.is_empty() {
                        scope.to_vec()
                    } else {
                        ranges
                    };
                    self.variables(context, child, &inner, output)?;
                }
                // Inlined calls describe another function's variables; nested
                // types, labels and call sites carry no variables.
                _ => {}
            }
        }
        Ok(())
    }

    fn variable(
        &mut self,
        context: &UnitContext<'_, '_, 'a>,
        entry: &gimli::DebuggingInformationEntry<'_, '_, Slice<'a>>,
        parameter: bool,
        scope: &[(u64, u64)],
    ) -> Result<Variable> {
        let unit = context.unit;
        let name = self
            .string(unit, entry, constants::DW_AT_name)?
            .unwrap_or_else(|| "<unnamed>".into());
        let type_id = match self.attribute(unit, entry, constants::DW_AT_type)? {
            Some(AttributeValue::UnitRef(offset)) => Some(self.resolve_type(context, offset)?),
            _ => None,
        };
        let location = match entry.attr_value(constants::DW_AT_location)? {
            Some(AttributeValue::Exprloc(expression)) => self.location(unit, expression),
            Some(value) => self.location_list(unit, value)?,
            None => match self.attribute(unit, entry, constants::DW_AT_const_value)? {
                Some(value) => Location::Unavailable(match value.sdata_value() {
                    Some(constant) => format!("optimized into the constant {constant}"),
                    None => "optimized into a constant".into(),
                }),
                None => Location::Unavailable("optimized out".into()),
            },
        };
        Ok(Variable {
            name,
            type_id,
            location,
            decl_file: self.decl_file(context, entry)?,
            decl_line: self
                .unsigned(unit, entry, constants::DW_AT_decl_line)?
                .unwrap_or(0) as u32,
            parameter,
            scope: scope.to_vec(),
        })
    }

    fn location_list(
        &self,
        unit: UnitRef<'_, Slice<'a>>,
        value: AttributeValue<Slice<'a>>,
    ) -> Result<Location> {
        let Some(mut entries) = unit.attr_locations(value)? else {
            return Ok(Location::Unavailable(
                "unsupported location attribute form".into(),
            ));
        };
        let mut ranges = Vec::new();
        while let Some(entry) = entries.next()? {
            if entry.range.end > entry.range.begin {
                ranges.push(LocationRange {
                    start: entry.range.begin,
                    end: entry.range.end,
                    location: self.location(unit, entry.data),
                });
            }
        }
        ranges.sort_by_key(|range| range.start);
        Ok(Location::List(ranges))
    }

    fn register(&self, number: u16) -> std::result::Result<String, Location> {
        register_name(self.architecture, number).ok_or_else(|| {
            Location::Unavailable(format!(
                "uses DWARF register {number}, which has no Vole equivalent"
            ))
        })
    }

    fn location(
        &self,
        unit: UnitRef<'_, Slice<'a>>,
        expression: Expression<Slice<'a>>,
    ) -> Location {
        let mut operations = expression.operations(unit.encoding());
        let mut list = Vec::new();
        loop {
            match operations.next() {
                Ok(Some(operation)) if list.len() < 16 => list.push(operation),
                Ok(Some(_)) | Ok(None) => break,
                Err(error) => {
                    return Location::Unavailable(format!("unreadable DWARF expression: {error}"));
                }
            }
        }
        let unsupported = |text: &str| Location::Unavailable(text.into());
        match list.as_slice() {
            [] => unsupported("optimized out"),
            [Operation::FrameOffset { offset }] => Location::FrameOffset(*offset),
            [Operation::Address { address }] => Location::Address(*address),
            [Operation::AddressIndex { index }] => match unit.address(*index) {
                Ok(address) => Location::Address(address),
                Err(error) => Location::Unavailable(format!("unreadable address: {error}")),
            },
            [
                Operation::RegisterOffset {
                    register, offset, ..
                },
            ] => match self.register(register.0) {
                Ok(register) => Location::RegisterOffset {
                    register,
                    offset: *offset,
                },
                Err(location) => location,
            },
            [Operation::Register { register }] => match self.register(register.0) {
                Ok(register) => Location::Register(register),
                Err(location) => location,
            },
            [Operation::CallFrameCFA] => {
                unsupported("the frame base is the call-frame address (DW_OP_call_frame_cfa)")
            }
            [Operation::UnsignedConstant { value }, Operation::StackValue] => {
                Location::Unavailable(format!("optimized into the constant {value}"))
            }
            [Operation::SignedConstant { value }, Operation::StackValue] => {
                Location::Unavailable(format!("optimized into the constant {value}"))
            }
            list if list
                .iter()
                .any(|op| matches!(op, Operation::EntryValue { .. })) =>
            {
                unsupported("only the value at function entry is described, and it is gone")
            }
            list if list.iter().any(|op| matches!(op, Operation::Piece { .. })) => {
                unsupported("the value is split into pieces across registers or memory")
            }
            list if list.iter().any(|op| {
                matches!(op, Operation::StackValue | Operation::ImplicitValue { .. })
            }) =>
            {
                unsupported("the value is computed by optimized code and not stored")
            }
            _ => unsupported("unsupported DWARF location expression"),
        }
    }

    fn void(&mut self) -> usize {
        if let Some(id) = self.void_type {
            return id;
        }
        self.info.types.push(Type {
            name: "void".into(),
            size: 0,
            kind: TypeKind::Void,
        });
        let id = self.info.types.len() - 1;
        self.void_type = Some(id);
        id
    }

    fn type_name(&self, id: usize) -> &str {
        &self.info.types[id].name
    }

    fn type_size(&self, id: usize) -> u64 {
        self.info.types[id].size
    }

    /// Resolves the `DW_AT_type` of an entry; a missing type means void.
    fn target(
        &mut self,
        context: &UnitContext<'_, '_, 'a>,
        entry: &gimli::DebuggingInformationEntry<'_, '_, Slice<'a>>,
    ) -> Result<usize> {
        match entry.attr_value(constants::DW_AT_type)? {
            Some(AttributeValue::UnitRef(offset)) => self.resolve_type(context, offset),
            _ => Ok(self.void()),
        }
    }

    fn resolve_type(
        &mut self,
        context: &UnitContext<'_, '_, 'a>,
        offset: UnitOffset,
    ) -> Result<usize> {
        let key = (context.index, offset.0);
        if let Some(id) = self.type_ids.get(&key) {
            return Ok(*id);
        }
        let id = self.info.types.len();
        self.info.types.push(Type {
            name: String::new(),
            size: 0,
            kind: TypeKind::Unsupported("recursive type".into()),
        });
        self.type_ids.insert(key, id);
        let unit = context.unit;
        let entry = unit.entry(offset)?;
        let name = self.string(unit, &entry, constants::DW_AT_name)?;
        let byte_size = entry
            .attr_value(constants::DW_AT_byte_size)?
            .and_then(|value| value.udata_value());
        let address_size = u64::from(unit.encoding().address_size);
        let ty = match entry.tag() {
            constants::DW_TAG_base_type => {
                let encoding = match entry.attr_value(constants::DW_AT_encoding)? {
                    Some(AttributeValue::Encoding(encoding)) => match encoding {
                        constants::DW_ATE_signed => BaseEncoding::Signed,
                        constants::DW_ATE_unsigned => BaseEncoding::Unsigned,
                        constants::DW_ATE_signed_char => BaseEncoding::SignedChar,
                        constants::DW_ATE_unsigned_char => BaseEncoding::UnsignedChar,
                        constants::DW_ATE_boolean => BaseEncoding::Boolean,
                        constants::DW_ATE_float => BaseEncoding::Float,
                        _ => BaseEncoding::Other,
                    },
                    _ => BaseEncoding::Other,
                };
                Type {
                    name: name.unwrap_or_else(|| "<base type>".into()),
                    size: byte_size.unwrap_or(0),
                    kind: TypeKind::Base(encoding),
                }
            }
            constants::DW_TAG_pointer_type | constants::DW_TAG_reference_type => {
                let target = self.target(context, &entry)?;
                Type {
                    name: format!("{} *", self.type_name(target)),
                    size: byte_size.unwrap_or(address_size),
                    kind: TypeKind::Pointer(Some(target)),
                }
            }
            constants::DW_TAG_const_type
            | constants::DW_TAG_volatile_type
            | constants::DW_TAG_restrict_type
            | constants::DW_TAG_typedef => {
                let target = self.target(context, &entry)?;
                let tag = entry.tag();
                let (name, kind) = if tag == constants::DW_TAG_const_type {
                    (
                        format!("const {}", self.type_name(target)),
                        TypeKind::Const(Some(target)),
                    )
                } else if tag == constants::DW_TAG_volatile_type {
                    (
                        format!("volatile {}", self.type_name(target)),
                        TypeKind::Volatile(Some(target)),
                    )
                } else if tag == constants::DW_TAG_restrict_type {
                    (
                        format!("{} restrict", self.type_name(target)),
                        TypeKind::Typedef(Some(target)),
                    )
                } else {
                    (
                        name.unwrap_or_else(|| "<typedef>".into()),
                        TypeKind::Typedef(Some(target)),
                    )
                };
                Type {
                    name,
                    size: self.type_size(target),
                    kind,
                }
            }
            constants::DW_TAG_array_type => self.array(context, offset, &entry)?,
            constants::DW_TAG_structure_type | constants::DW_TAG_union_type => {
                let union = entry.tag() == constants::DW_TAG_union_type;
                let keyword = if union { "union" } else { "struct" };
                // Name and size first, so members that point back here can use them.
                self.info.types[id].name =
                    format!("{keyword} {}", name.as_deref().unwrap_or("<anonymous>"));
                self.info.types[id].size = byte_size.unwrap_or(0);
                let members = self.members(context, offset)?;
                Type {
                    name: self.info.types[id].name.clone(),
                    size: byte_size.unwrap_or(0),
                    kind: if union {
                        TypeKind::Union(members)
                    } else {
                        TypeKind::Struct(members)
                    },
                }
            }
            constants::DW_TAG_enumeration_type => {
                let mut enumerators = Vec::new();
                let mut tree = unit.entries_tree(Some(offset))?;
                let mut children = tree.root()?.children();
                while let Some(child) = children.next()? {
                    let child = child.entry();
                    if child.tag() != constants::DW_TAG_enumerator {
                        continue;
                    }
                    let label = self
                        .string(unit, child, constants::DW_AT_name)?
                        .unwrap_or_default();
                    let value = match child.attr_value(constants::DW_AT_const_value)? {
                        Some(AttributeValue::Udata(value)) => value as i64,
                        Some(value) => value.sdata_value().unwrap_or(0),
                        None => 0,
                    };
                    enumerators.push((label, value));
                }
                Type {
                    name: format!("enum {}", name.as_deref().unwrap_or("<anonymous>")),
                    size: byte_size.unwrap_or(4),
                    kind: TypeKind::Enum(enumerators),
                }
            }
            constants::DW_TAG_subroutine_type => Type {
                name: "function".into(),
                size: 0,
                kind: TypeKind::Function,
            },
            constants::DW_TAG_unspecified_type => Type {
                name: name.unwrap_or_else(|| "void".into()),
                size: 0,
                kind: TypeKind::Void,
            },
            tag => Type {
                name: name.unwrap_or_else(|| "<unsupported type>".into()),
                size: byte_size.unwrap_or(0),
                kind: TypeKind::Unsupported(
                    tag.static_string()
                        .map_or_else(|| format!("DWARF tag 0x{:x}", tag.0), str::to_owned),
                ),
            },
        };
        self.info.types[id] = ty;
        Ok(id)
    }

    /// Multi-dimensional arrays are one DWARF type with several subranges;
    /// they become nested single-dimension array types.
    fn array(
        &mut self,
        context: &UnitContext<'_, '_, 'a>,
        offset: UnitOffset,
        entry: &gimli::DebuggingInformationEntry<'_, '_, Slice<'a>>,
    ) -> Result<Type> {
        let element = self.target(context, entry)?;
        let mut counts = Vec::new();
        let mut tree = context.unit.entries_tree(Some(offset))?;
        let mut children = tree.root()?.children();
        while let Some(child) = children.next()? {
            let child = child.entry();
            if child.tag() != constants::DW_TAG_subrange_type {
                continue;
            }
            let count = match child.attr_value(constants::DW_AT_count)? {
                Some(value) => value.udata_value(),
                None => child
                    .attr_value(constants::DW_AT_upper_bound)?
                    .and_then(|value| value.udata_value())
                    .map(|upper| upper + 1),
            };
            counts.push(count);
        }
        if counts.is_empty() {
            counts.push(None);
        }
        let base = self.type_name(element).to_string();
        let dimensions = |counts: &[Option<u64>]| -> String {
            counts
                .iter()
                .map(|count| count.map_or("[]".into(), |count| format!("[{count}]")))
                .collect()
        };
        let mut current = element;
        for level in (1..counts.len()).rev() {
            let size = self.type_size(current) * counts[level].unwrap_or(0);
            self.info.types.push(Type {
                name: format!("{base}{}", dimensions(&counts[level..])),
                size,
                kind: TypeKind::Array {
                    element: Some(current),
                    count: counts[level],
                },
            });
            current = self.info.types.len() - 1;
        }
        Ok(Type {
            name: format!("{base}{}", dimensions(&counts)),
            size: self.type_size(current) * counts[0].unwrap_or(0),
            kind: TypeKind::Array {
                element: Some(current),
                count: counts[0],
            },
        })
    }

    fn members(
        &mut self,
        context: &UnitContext<'_, '_, 'a>,
        offset: UnitOffset,
    ) -> Result<Vec<Member>> {
        let unit = context.unit;
        let mut members = Vec::new();
        let mut tree = unit.entries_tree(Some(offset))?;
        let mut children = tree.root()?.children();
        while let Some(child) = children.next()? {
            let child = child.entry();
            if child.tag() != constants::DW_TAG_member {
                continue;
            }
            let name = self
                .string(unit, child, constants::DW_AT_name)?
                .unwrap_or_default();
            let mut type_id = match child.attr_value(constants::DW_AT_type)? {
                Some(AttributeValue::UnitRef(target)) => Some(self.resolve_type(context, target)?),
                _ => None,
            };
            let offset = match child.attr_value(constants::DW_AT_data_member_location)? {
                Some(value) => value.udata_value().unwrap_or(0),
                None => child
                    .attr_value(constants::DW_AT_data_bit_offset)?
                    .and_then(|value| value.udata_value())
                    .map_or(0, |bits| bits / 8),
            };
            if let Some(bits) = child
                .attr_value(constants::DW_AT_bit_size)?
                .and_then(|value| value.udata_value())
            {
                let base = type_id.map_or("int".into(), |id| self.type_name(id).to_string());
                self.info.types.push(Type {
                    name: format!("{base} : {bits}"),
                    size: type_id.map_or(0, |id| self.type_size(id)),
                    kind: TypeKind::Unsupported("bit-field".into()),
                });
                type_id = Some(self.info.types.len() - 1);
            }
            members.push(Member {
                name,
                type_id,
                offset,
            });
        }
        Ok(members)
    }

    fn unwind(&mut self, data: &[u8], is_64: bool) -> Result<()> {
        let fallback = match self.architecture {
            Architecture::Arm64 => 30,
            Architecture::Arm32 => 14,
            Architecture::X86 => 8,
            _ => 16,
        };
        self.info.return_address_register =
            register_name(self.architecture, fallback).unwrap_or_default();
        if data.is_empty() {
            return Ok(());
        }
        let mut frame = DebugFrame::new(data, LittleEndian);
        frame.set_address_size(if is_64 { 8 } else { 4 });
        let bases = BaseAddresses::default();
        let mut context = Box::new(UnwindContext::new());
        let mut entries = frame.entries(&bases);
        let mut return_address = None;
        while let Some(entry) = entries.next()? {
            let CieOrFde::Fde(partial) = entry else {
                continue;
            };
            let fde = partial.parse(DebugFrame::cie_from_offset)?;
            return_address.get_or_insert(fde.cie().return_address_register().0);
            let mut table = UnwindTable::new(&frame, &bases, &mut context, &fde)?;
            while let Some(row) = table.next_row()? {
                let cfa = match row.cfa() {
                    gimli::CfaRule::RegisterAndOffset { register, offset } => {
                        match register_name(self.architecture, register.0) {
                            Some(register) => CfaRule::RegisterOffset {
                                register,
                                offset: *offset,
                            },
                            None => CfaRule::Unsupported(format!(
                                "CFA uses DWARF register {}",
                                register.0
                            )),
                        }
                    }
                    gimli::CfaRule::Expression(_) => {
                        CfaRule::Unsupported("CFA is a DWARF expression".into())
                    }
                };
                let mut registers = Vec::new();
                for (register, rule) in row.registers() {
                    let Some(name) = register_name(self.architecture, register.0) else {
                        continue;
                    };
                    let rule = match rule {
                        gimli::RegisterRule::Undefined => RegisterRule::Undefined,
                        gimli::RegisterRule::SameValue => RegisterRule::SameValue,
                        gimli::RegisterRule::Offset(offset) => RegisterRule::Offset(*offset),
                        gimli::RegisterRule::ValOffset(offset) => RegisterRule::ValOffset(*offset),
                        gimli::RegisterRule::Register(other) => {
                            match register_name(self.architecture, other.0) {
                                Some(other) => RegisterRule::Register(other),
                                None => RegisterRule::Unsupported(format!(
                                    "saved in DWARF register {}",
                                    other.0
                                )),
                            }
                        }
                        _ => RegisterRule::Unsupported("DWARF expression rule".into()),
                    };
                    registers.push((name, rule));
                }
                self.info.unwind.push(UnwindRow {
                    start: row.start_address(),
                    end: row.end_address(),
                    cfa,
                    registers,
                });
            }
        }
        if let Some(name) = return_address.and_then(|r| register_name(self.architecture, r)) {
            self.info.return_address_register = name;
        }
        Ok(())
    }

    /// Adds functions known only from the symbol table, such as `_start`.
    fn symbol_functions(&mut self, symbols: &[FunctionSymbol]) {
        for symbol in symbols {
            if symbol.size == 0
                || self
                    .info
                    .functions
                    .iter()
                    .any(|function| function.contains(symbol.address))
            {
                continue;
            }
            let decl_file = self
                .info
                .lines
                .iter()
                .find(|row| row.address == symbol.address && !row.end_sequence)
                .map(|row| row.file);
            let decl_file = decl_file.unwrap_or_else(|| self.file_index(UNKNOWN_FILE));
            self.info.functions.push(Function {
                name: symbol.name.clone(),
                low_pc: symbol.address,
                high_pc: symbol.address + symbol.size,
                decl_file,
                decl_line: 0,
                frame_base: None,
                return_type: None,
                variables: Vec::new(),
                prologue_end: None,
                user: false,
            });
        }
    }
}

struct UnitContext<'u, 'f, 'a> {
    unit: UnitRef<'u, Slice<'a>>,
    index: usize,
    files: &'f HashMap<u64, u32>,
}
