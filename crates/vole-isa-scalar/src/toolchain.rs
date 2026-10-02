use crate::decode;
use object::{Object, ObjectSection, ObjectSymbol, SectionKind};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use vole_core::{Architecture, Diagnostic, Instruction, MemoryRegion, Program};

const MAX_IMAGE: u64 = 4 * 1024 * 1024;
const MAX_SOURCE: usize = 256 * 1024;
const LINK_SCRIPT: &str = "ENTRY(_start)\nSECTIONS {\n . = 0x1000; .text : { *(.text*) }\n ASSERT(SIZEOF(.text) <= 0x1000, \"Teaching code exceeds 4096 bytes\")\n . = 0x2000; .data : { *(.rodata*) *(.data*) }\n .bss : { *(.bss*) *(COMMON) }\n ASSERT(. <= 0x10000, \"Teaching data exceeds mapped memory\")\n /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }\n}\n";

fn tool_path(variable: &str, basename: &str) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(variable) {
        return Some(PathBuf::from(path));
    }
    let mut names = vec![format!("{basename}-14"), basename.to_string()];
    names.extend(
        (15..=23)
            .rev()
            .map(|version| format!("{basename}-{version}")),
    );
    if let Some(directory) = std::env::var_os("VOLE_TOOLCHAIN_DIR") {
        for name in &names {
            let path =
                PathBuf::from(&directory).join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(parent) = executable.parent()
    {
        for directory in [
            parent.join("toolchain"),
            parent.join("../Resources/toolchain"),
        ] {
            for name in &names {
                let path = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    let search = std::env::var_os("PATH")?;
    for name in names {
        for directory in std::env::split_paths(&search) {
            let path = directory.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

pub fn toolchain_status() -> Result<(PathBuf, PathBuf), String> {
    Ok((
        tool_path("VOLE_LLVM_MC", "llvm-mc")
            .ok_or("LLVM MC not found. Install LLVM 14+ or set VOLE_LLVM_MC to llvm-mc.")?,
        tool_path("VOLE_LLD", "ld.lld")
            .ok_or("LLD not found. Install LLD 14+ or set VOLE_LLD to ld.lld.")?,
    ))
}

fn run_tool(
    tool: &Path,
    args: &[&std::ffi::OsStr],
    directory: &Path,
    output: &Path,
) -> Result<String, String> {
    let errors = directory.join("diagnostics.txt");
    let stderr = fs::File::create(&errors).map_err(|e| e.to_string())?;
    let mut child = Command::new(tool)
        .args(args)
        .current_dir(directory)
        .env("TMPDIR", directory)
        .env("TEMP", directory)
        .env("TMP", directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .map_err(|e| format!("Cannot run {}: {e}", tool.display()))?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if start.elapsed() > Duration::from_secs(5)
            || fs::metadata(output).is_ok_and(|m| m.len() > MAX_IMAGE)
            || fs::metadata(&errors).is_ok_and(|m| m.len() > 128 * 1024)
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Assembly exceeded its five-second or output-size budget".into());
        }
        thread::sleep(Duration::from_millis(10));
    };
    let mut diagnostics = Vec::new();
    fs::File::open(&errors)
        .and_then(|file| file.take(64 * 1024).read_to_end(&mut diagnostics))
        .map_err(|e| e.to_string())?;
    let message = String::from_utf8_lossy(&diagnostics).into_owned();
    if !status.success() {
        return Err(message);
    }
    if fs::metadata(output).map_err(|e| e.to_string())?.len() > MAX_IMAGE {
        return Err("Machine image exceeds four MiB".into());
    }
    Ok(message)
}

fn diagnostic(message: String, line_map: &[usize]) -> Vec<Diagnostic> {
    let mut result = vec![];
    for line in message.lines().filter(|line| line.contains("error:")) {
        let pieces: Vec<_> = line.split(':').collect();
        let number = pieces
            .iter()
            .find_map(|piece| piece.parse::<usize>().ok())
            .unwrap_or(1);
        let original = line_map.get(number.saturating_sub(1)).copied().unwrap_or(1);
        let text = line
            .split_once("error:")
            .map_or(line, |(_, text)| text)
            .trim();
        result.push(Diagnostic::new(original, text));
        if result.len() == 32 {
            break;
        }
    }
    if result.is_empty() {
        result.push(Diagnostic::new(1, message.trim()));
    }
    result
}

fn prepare_source(source: &str) -> Result<(String, Vec<usize>), Vec<Diagnostic>> {
    if source.len() > MAX_SOURCE {
        return Err(vec![Diagnostic::new(1, "Source exceeds 256 KiB")]);
    }
    let mut expanded = String::new();
    let mut map = Vec::new();
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.contains("__vole_line_") {
            return Err(vec![Diagnostic::new(
                index + 1,
                "The __vole_line_ prefix is reserved for source mapping",
            )]);
        }
        // Assembly is intentionally file-local. Avoid arbitrary host-file reads and unbounded expansion.
        let visible = outside_strings(line);
        for statement in visible.split(';') {
            let statement = statement
                .rsplit_once(':')
                .map_or(statement, |(_, suffix)| suffix)
                .trim();
            if !statement.starts_with('.') {
                continue;
            }
            let directive = statement.split_whitespace().next().unwrap_or("");
            let allowed = [
                ".syntax",
                ".text",
                ".data",
                ".rodata",
                ".bss",
                ".section",
                ".global",
                ".globl",
                ".local",
                ".type",
                ".size",
                ".intel_syntax",
                ".arm",
                ".byte",
                ".short",
                ".hword",
                ".word",
                ".long",
                ".quad",
                ".ascii",
                ".asciz",
                ".string",
                ".align",
                ".balign",
                ".p2align",
                ".space",
                ".zero",
                ".equ",
                ".set",
                ".arch",
                ".cpu",
            ];
            if !allowed.contains(&directive) {
                return Err(vec![Diagnostic::new(
                    index + 1,
                    format!("Directive {directive} is outside the file-local teaching assembler"),
                )]);
            }
            if directive == ".section" {
                let name = statement[directive.len()..]
                    .trim()
                    .split([',', ' '])
                    .next()
                    .unwrap_or("");
                if ![".text", ".data", ".rodata", ".bss"]
                    .iter()
                    .any(|prefix| name == *prefix || name.starts_with(&format!("{prefix}.")))
                {
                    return Err(vec![Diagnostic::new(
                        index + 1,
                        "Only .text, .data, .rodata and .bss sections are mapped",
                    )]);
                }
            }
            if [".space", ".zero", ".align", ".balign", ".p2align"].contains(&directive) {
                let argument = statement[directive.len()..]
                    .trim()
                    .split(',')
                    .next()
                    .unwrap_or("");
                let value = parse_unsigned(argument).ok_or_else(|| {
                    vec![Diagnostic::new(
                        index + 1,
                        "Allocation/alignment must be a bounded integer literal",
                    )]
                })?;
                let limit = if directive == ".p2align" || directive == ".align" {
                    12
                } else {
                    65536
                };
                if value > limit {
                    return Err(vec![Diagnostic::new(
                        index + 1,
                        "Allocation/alignment exceeds the teaching image limit",
                    )]);
                }
            }
        }
        if !trimmed.is_empty()
            && !trimmed.starts_with(['.', '#', ';', '@'])
            && !trimmed.ends_with(':')
            && !trimmed.starts_with("//")
        {
            expanded.push_str(&format!("__vole_line_{}:\n", index + 1));
            map.push(index + 1);
        }
        expanded.push_str(line);
        expanded.push('\n');
        map.push(index + 1);
    }
    Ok((expanded, map))
}

// Remove quoted text before validating statement directives. Escapes remain inside quotes.
fn outside_strings(line: &str) -> String {
    let mut result = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for character in line.chars() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
            result.push(' ');
        } else if character == '"' {
            quoted = true;
            result.push(' ');
        } else {
            result.push(character);
        }
    }
    result
}

fn parse_unsigned(text: &str) -> Option<u64> {
    if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16).ok()
    } else {
        text.parse().ok()
    }
}

/// Assemble to ELF using LLVM MC and resolve relocations using LLD. Never executes guest code.
pub fn assemble(architecture: Architecture, source: &str) -> Result<Program, Vec<Diagnostic>> {
    let triple = match architecture {
        Architecture::Arm32 => "armv7-none-eabi",
        Architecture::Arm64 => "aarch64-none-elf",
        Architecture::X86 => "i386-none-elf",
        Architecture::X64 => "x86_64-none-elf",
        Architecture::Vole => {
            return Err(vec![Diagnostic::new(
                1,
                "Use the VOLE assembler for 8-bit source",
            )]);
        }
    };
    let (mut prepared, mut line_map) = prepare_source(source)?;
    if architecture == Architecture::Arm32 {
        prepared = format!(".arch armv7-a\n.arm\n{prepared}");
        line_map.splice(0..0, [1, 1]);
    }
    let (mc, linker) = toolchain_status().map_err(|e| vec![Diagnostic::new(1, e)])?;
    let temp = tempfile::Builder::new()
        .prefix("vole-assembly-")
        .tempdir()
        .map_err(|e| vec![Diagnostic::new(1, e.to_string())])?;
    let input = temp.path().join("source.s");
    let object_path = temp.path().join("source.o");
    let image_path = temp.path().join("program.elf");
    let script = temp.path().join("memory.ld");
    fs::write(&input, prepared)
        .and_then(|_| fs::write(&script, LINK_SCRIPT))
        .map_err(|e| vec![Diagnostic::new(1, e.to_string())])?;
    let triple_flag = format!("--triple={triple}");
    run_tool(
        &mc,
        &[
            triple_flag.as_ref(),
            "--filetype=obj".as_ref(),
            input.as_os_str(),
            "-o".as_ref(),
            object_path.as_os_str(),
        ],
        temp.path(),
        &object_path,
    )
    .map_err(|e| diagnostic(e, &line_map))?;
    run_tool(
        &linker,
        &[
            "--fatal-warnings".as_ref(),
            "-T".as_ref(),
            script.as_os_str(),
            object_path.as_os_str(),
            "-o".as_ref(),
            image_path.as_os_str(),
        ],
        temp.path(),
        &image_path,
    )
    .map_err(|e| diagnostic(e, &line_map))?;
    let bytes = fs::read(&image_path).map_err(|e| vec![Diagnostic::new(1, e.to_string())])?;
    let file = object::File::parse(bytes.as_slice())
        .map_err(|e| vec![Diagnostic::new(1, format!("Invalid linked ELF: {e}"))])?;
    let mut symbols = BTreeMap::new();
    let mut source_map = BTreeMap::new();
    for symbol in file.symbols() {
        if let Ok(name) = symbol.name() {
            if let Some(line) = name
                .strip_prefix("__vole_line_")
                .and_then(|n| n.parse::<usize>().ok())
            {
                source_map.insert(symbol.address(), line);
            } else if !name.is_empty() && symbol.is_definition() && !name.starts_with('$') {
                symbols.insert(name.to_owned(), symbol.address());
            }
        }
    }
    let mut regions = Vec::new();
    let mut data = vec![0; 0xe000];
    let mut instructions = Vec::new();
    for section in file.sections() {
        if !matches!(
            section.kind(),
            SectionKind::Text
                | SectionKind::Data
                | SectionKind::ReadOnlyData
                | SectionKind::UninitializedData
        ) || section.size() == 0
        {
            continue;
        }
        let address = section.address();
        let section_bytes = section
            .data()
            .map_err(|e| vec![Diagnostic::new(1, e.to_string())])?;
        if section.kind() == SectionKind::Text {
            let mut offset = 0;
            while offset < section_bytes.len() {
                let address = address + offset as u64;
                let mut instruction = decode(architecture, address, &section_bytes[offset..])
                    .unwrap_or_else(|_| Instruction {
                        address,
                        bytes: vec![section_bytes[offset]],
                        assembly: format!(".byte 0x{:02x}", section_bytes[offset]),
                        explanation: "Undecodable byte; execution will fault here.".into(),
                        source_line: None,
                        reads: vec![],
                        writes: vec![],
                    });
                instruction.source_line = source_map.get(&address).copied();
                offset += instruction.bytes.len();
                instructions.push(instruction);
            }
            regions.push(MemoryRegion {
                base: address,
                bytes: section_bytes.to_vec(),
                writable: true,
                executable: true,
                label: "Program".into(),
            });
        } else {
            let offset = address.checked_sub(0x2000).ok_or_else(|| {
                vec![Diagnostic::new(
                    1,
                    "Data is outside the teaching memory map",
                )]
            })? as usize;
            if offset
                .checked_add(section.size() as usize)
                .is_none_or(|end| end > data.len())
            {
                return Err(vec![Diagnostic::new(1, "Data exceeds mapped memory")]);
            }
            if section.kind() != SectionKind::UninitializedData {
                data[offset..offset + section_bytes.len()].copy_from_slice(section_bytes);
            }
        }
    }
    if regions.is_empty() {
        return Err(vec![Diagnostic::new(
            1,
            "The program has no .text instructions",
        )]);
    }
    regions.push(MemoryRegion {
        base: 0x2000,
        bytes: data,
        writable: true,
        executable: false,
        label: "Main memory".into(),
    });
    regions.push(MemoryRegion {
        base: 0x10000,
        bytes: vec![0; 0x10000],
        writable: true,
        executable: false,
        label: "Stack".into(),
    });
    let stack = match architecture {
        Architecture::Arm32 => "r13",
        Architecture::Arm64 => "sp",
        Architecture::X86 => "esp",
        Architecture::X64 => "rsp",
        Architecture::Vole => unreachable!(),
    };
    Ok(Program {
        architecture,
        source: source.to_owned(),
        entry: file.entry(),
        regions,
        instructions,
        symbols,
        initial_registers: BTreeMap::from([(stack.to_string(), 0x20000)]),
    })
}
