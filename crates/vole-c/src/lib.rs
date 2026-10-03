//! Freestanding C compilation for guest targets using Clang and LLD.
//!
//! A document is compiled as `main.c` together with the embedded teaching
//! runtime and startup code, linked with a fixed memory map and loaded as a
//! [`Program`] with [`DebugInfo`] extracted from DWARF. See
//! `docs/c-environment.md` for the contract.
mod check;
mod diagnostics;
mod dwarf;
mod tools;

use object::{Object, ObjectSection, ObjectSymbol, SectionFlags, SymbolKind};
use std::{
    collections::{BTreeMap, HashMap},
    ffi::OsString,
    fs,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tools::{Clang, run_tool};
use vole_core::{
    Architecture, CompilerSettings, DebugInfo, Diagnostic, Instruction, MemoryRegion, Program,
    Severity, SourceLanguage,
};

pub use check::ALLOWED_HEADERS;
pub use tools::clang_status;

const MAX_SOURCE: usize = 256 * 1024;
const CLANG_DEADLINE: Duration = Duration::from_secs(10);
const LINK_DEADLINE: Duration = Duration::from_secs(5);

/// Teaching runtime header, placed in a private include directory.
pub const VOLE_H: &str = include_str!("../runtime/vole.h");
const RUNTIME_C: &str = include_str!("../runtime/vole_runtime.c");

const CODE_BASE: u64 = 0x1000;
const CODE_END: u64 = 0x8000;
const RODATA_BASE: u64 = 0x8000;
const RODATA_SIZE: u64 = 0x2000;
const DATA_BASE: u64 = 0xA000;
const DATA_SIZE: u64 = 0x6000;
const STACK_BASE: u64 = 0x10000;
const STACK_SIZE: u64 = 0x10000;
const STACK_TOP: u64 = 0x20000;

const LINK_SCRIPT: &str = "ENTRY(_start)
SECTIONS {
  .text 0x1000 : { *(.text .text.*) }
  ASSERT(SIZEOF(.text) <= 0x7000, \"VOLE_CODE_TOO_LARGE\")
  .rodata 0x8000 : { *(.rodata .rodata.*) *(.data.rel.ro .data.rel.ro.*) }
  ASSERT(SIZEOF(.rodata) <= 0x2000, \"VOLE_RODATA_TOO_LARGE\")
  . = 0xA000;
  .data : { *(.data .data.*) }
  .bss : { *(.bss .bss.*) *(COMMON) }
  ASSERT(. <= 0x10000, \"VOLE_DATA_TOO_LARGE\")
  /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) *(.ARM.exidx*) *(.ARM.extab*) }
}
";

/// Original teaching programs: file name and source. The first is the default.
pub const EXAMPLES: &[(&str, &str)] = &[
    ("tour.c", include_str!("../../../examples/c/tour.c")),
    ("hello.c", include_str!("../../../examples/c/hello.c")),
    (
        "arithmetic.c",
        include_str!("../../../examples/c/arithmetic.c"),
    ),
    ("loops.c", include_str!("../../../examples/c/loops.c")),
    (
        "functions.c",
        include_str!("../../../examples/c/functions.c"),
    ),
    ("arrays.c", include_str!("../../../examples/c/arrays.c")),
    ("pointers.c", include_str!("../../../examples/c/pointers.c")),
];

/// The program shown in a new C document.
pub fn default_example() -> &'static str {
    EXAMPLES[0].1
}

struct Target {
    triple: &'static str,
    flags: &'static [&'static str],
    crt0_name: &'static str,
    crt0: &'static str,
    stack_register: &'static str,
    /// A32/A64 instructions are four bytes; x86 instructions vary.
    fixed_width: bool,
}

fn target(architecture: Architecture) -> Option<Target> {
    Some(match architecture {
        Architecture::Arm64 => Target {
            triple: "aarch64-none-elf",
            flags: &["-mgeneral-regs-only"],
            crt0_name: "crt0_arm64.s",
            crt0: include_str!("../runtime/crt0_arm64.s"),
            stack_register: "sp",
            fixed_width: true,
        },
        Architecture::X64 => Target {
            triple: "x86_64-none-elf",
            flags: &["-mgeneral-regs-only"],
            crt0_name: "crt0_x64.s",
            crt0: include_str!("../runtime/crt0_x64.s"),
            stack_register: "rsp",
            fixed_width: false,
        },
        Architecture::Arm32 => Target {
            triple: "armv7a-none-eabi",
            flags: &["-marm", "-mfloat-abi=soft", "-mno-unaligned-access"],
            crt0_name: "crt0_arm32.s",
            crt0: include_str!("../runtime/crt0_arm32.s"),
            stack_register: "r13",
            fixed_width: true,
        },
        Architecture::X86 => Target {
            triple: "i386-none-elf",
            flags: &["-mgeneral-regs-only"],
            crt0_name: "crt0_x86.s",
            crt0: include_str!("../runtime/crt0_x86.s"),
            stack_register: "esp",
            fixed_width: false,
        },
        Architecture::Vole => return None,
    })
}

/// Contract flags shared by the document and the runtime, before optimization.
const COMMON_FLAGS: &[&str] = &[
    "-std=c17",
    // Trigraphs (`??=` for `#`) are obsolete and would hide directives from review.
    "-fno-trigraphs",
    "-ffreestanding",
    "-fno-builtin",
    "-nostdlibinc",
    "-fno-pic",
    "-fno-pie",
    "-fno-stack-protector",
    "-fno-exceptions",
    "-fno-asynchronous-unwind-tables",
    "-fno-unwind-tables",
    "-fno-omit-frame-pointer",
    "-fno-common",
    "-fno-vectorize",
    "-fno-slp-vectorize",
    "-gdwarf-5",
    "-fno-color-diagnostics",
    "-ferror-limit=20",
    "-Werror=implicit-function-declaration",
    "-Werror=return-type",
    // Run the compiler in-process so a timeout leaves no orphaned cc1 child.
    "-fintegrated-cc1",
    // Record "." instead of the private build directory in DWARF.
    "-fdebug-compilation-dir=.",
];

/// Compiler flags for a C file, as recorded in [`DebugInfo::compiler_flags`].
fn c_flags(target: &Target, settings: &CompilerSettings) -> Vec<String> {
    let mut flags = vec![format!("--target={}", target.triple)];
    flags.extend(target.flags.iter().map(|flag| flag.to_string()));
    flags.extend(COMMON_FLAGS.iter().map(|flag| flag.to_string()));
    flags.push(settings.optimization.flag().into());
    if settings.warnings {
        flags.extend(["-Wall".into(), "-Wextra".into()]);
    }
    flags
}

/// Adds private paths that are not recorded: headers, resource dir, input and output.
fn invocation(clang: &Clang, flags: &[String], input: &str, output: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = flags.iter().map(OsString::from).collect();
    if let Some(resource_dir) = &clang.resource_dir {
        args.push("-resource-dir".into());
        args.push(resource_dir.into());
    }
    for arg in ["-isystem", "include", "-c", input, "-o", output] {
        args.push(arg.into());
    }
    args
}

const HOST_FILE: &str =
    "A Vole document can only include <vole.h> and Clang's freestanding headers";

fn host_file_error() -> Vec<Diagnostic> {
    vec![Diagnostic::new(1, HOST_FILE).with_hint(format!(
        "The preprocessor tried to open a file outside the allowed headers ({}). \
         Documents cannot read other files on this computer.",
        check::ALLOWED_HEADERS.join(", ")
    ))]
}

/// Preprocess first and let Clang list every file it opened. That list, not
/// the lexical pre-check, decides what a document may read: a build stops,
/// with the preprocessor's own output withheld, if anything outside the
/// private `include` directory and Clang's resource headers was opened.
fn dependency_gate(
    clang: &Clang,
    flags: &[String],
    directory: &Path,
    source: &str,
) -> Result<(), Vec<Diagnostic>> {
    let resource = tools::resource_include(clang).map_err(error)?;
    let private = fs::canonicalize(directory.join("include")).map_err(|e| error(e.to_string()))?;
    let document =
        fs::canonicalize(directory.join(dwarf::USER_FILE)).map_err(|e| error(e.to_string()))?;
    let allowed = |path: &Path| {
        fs::canonicalize(directory.join(path)).is_ok_and(|path| {
            path == document || path.starts_with(&private) || path.starts_with(&resource)
        })
    };
    let mut args: Vec<OsString> = invocation(clang, flags, dwarf::USER_FILE, "main.o")
        .into_iter()
        .take_while(|arg| arg != "-c")
        .collect();
    for arg in ["-M", "-MT", "main", "-MF", "main.d", dwarf::USER_FILE] {
        args.push(arg.into());
    }
    let deps_path = directory.join("main.d");
    let output =
        run_tool(&clang.path, &args, directory, &deps_path, CLANG_DEADLINE).map_err(error)?;
    if !output.success {
        // Report only positions in the document itself; anything that names
        // another file (or a missing one) gets the generic explanation.
        let foreign = output.diagnostics.lines().any(|line| {
            line.contains("file not found")
                || line.contains("cannot open")
                || line.starts_with("In file included from")
                || line
                    .split_once(": ")
                    .and_then(|(position, _)| position.split(':').next())
                    .is_some_and(|file| {
                        !file.is_empty()
                            && file != dwarf::USER_FILE
                            && !file.starts_with("clang")
                            && !allowed(Path::new(file))
                    })
        });
        if foreign {
            return Err(host_file_error());
        }
        let messages = diagnostics::clang(&output.diagnostics, source);
        return Err(if messages.iter().any(Diagnostic::is_error) {
            messages
        } else {
            error("Preprocessing failed")
        });
    }
    let deps = fs::read_to_string(&deps_path).map_err(|e| error(e.to_string()))?;
    let deps = deps.replace("\\\r\n", " ").replace("\\\n", " ");
    let files = deps.split_once(": ").map_or("", |(_, files)| files);
    // Make-style escaping: a backslash protects the following space.
    let mut paths = Vec::new();
    let mut current = String::new();
    let mut chars = files.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars
                .peek()
                .is_some_and(|next| *next == ' ' || *next == '#') =>
            {
                current.push(chars.next().unwrap_or(' '));
            }
            c if c.is_whitespace() => {
                if !current.is_empty() {
                    paths.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        paths.push(current);
    }
    if paths.is_empty() || paths.iter().any(|path| !allowed(Path::new(path))) {
        return Err(host_file_error());
    }
    Ok(())
}

fn error(message: impl Into<String>) -> Vec<Diagnostic> {
    vec![Diagnostic::new(1, message)]
}

/// Compiled startup and runtime objects for one target and Clang installation.
struct RuntimeObjects {
    crt0: Vec<u8>,
    runtime: Vec<u8>,
}

type RuntimeCache = Mutex<HashMap<(&'static str, Clang), Arc<RuntimeObjects>>>;

/// The runtime is identical for every document, so it is compiled once per process.
fn runtime_objects(
    architecture: Architecture,
    target: &Target,
    clang: &Clang,
    directory: &Path,
) -> Result<Arc<RuntimeObjects>, Vec<Diagnostic>> {
    static CACHE: OnceLock<RuntimeCache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (architecture.id(), clang.clone());
    if let Some(objects) = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&key)
    {
        return Ok(objects.clone());
    }
    let failed = |what: &str, message: String| {
        error(format!(
            "The Vole {what} failed to build with this Clang: {}",
            message.trim()
        ))
    };
    fs::write(directory.join("vole_runtime.c"), RUNTIME_C)
        .and_then(|_| fs::write(directory.join(target.crt0_name), target.crt0))
        .map_err(|e| error(e.to_string()))?;
    let runtime_flags = c_flags(
        target,
        &CompilerSettings {
            optimization: vole_core::Optimization::O0,
            warnings: false,
        },
    );
    let runtime_path = directory.join("vole_runtime.o");
    let output = run_tool(
        &clang.path,
        &invocation(clang, &runtime_flags, "vole_runtime.c", "vole_runtime.o"),
        directory,
        &runtime_path,
        CLANG_DEADLINE,
    )
    .map_err(|e| failed("runtime", e))?;
    if !output.success {
        return Err(failed("runtime", output.diagnostics));
    }
    let crt0_path = directory.join("crt0.o");
    let mut crt0_args: Vec<OsString> = vec![
        format!("--target={}", target.triple).into(),
        "-gdwarf-5".into(),
        "-fdebug-compilation-dir=.".into(),
        "-c".into(),
    ];
    crt0_args.extend([target.crt0_name, "-o", "crt0.o"].map(OsString::from));
    let output = run_tool(
        &clang.path,
        &crt0_args,
        directory,
        &crt0_path,
        CLANG_DEADLINE,
    )
    .map_err(|e| failed("startup code", e))?;
    if !output.success {
        return Err(failed("startup code", output.diagnostics));
    }
    let objects = Arc::new(RuntimeObjects {
        crt0: fs::read(&crt0_path).map_err(|e| error(e.to_string()))?,
        runtime: fs::read(&runtime_path).map_err(|e| error(e.to_string()))?,
    });
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(key, objects.clone());
    Ok(objects)
}

/// Compile a single C document with the teaching runtime and link a guest image.
/// Warnings are discarded; use [`compile_with_warnings`] to show them.
pub fn compile(
    architecture: Architecture,
    source: &str,
    settings: &CompilerSettings,
) -> Result<Program, Vec<Diagnostic>> {
    compile_with_warnings(architecture, source, settings).map(|(program, _)| program)
}

/// Compile and link like [`compile`], also returning Clang's warnings and notes
/// for a successful build. Warnings never block a build. On failure the list
/// holds every error, warning and note in Clang's order.
pub fn compile_with_warnings(
    architecture: Architecture,
    source: &str,
    settings: &CompilerSettings,
) -> Result<(Program, Vec<Diagnostic>), Vec<Diagnostic>> {
    let target = target(architecture).ok_or_else(|| {
        error("C programs need a 32- or 64-bit guest; VOLE 8-bit has no C compiler")
    })?;
    let linked = link(architecture, &target, source, settings)?;
    let debug = DebugInfo {
        triple: target.triple.into(),
        settings: settings.clone(),
        compiler_flags: linked.flags,
        ..Default::default()
    };
    let program = load(architecture, &target, source, &linked.image, debug)?;
    Ok((program, linked.messages))
}

/// A linked guest ELF with the compiler's non-error messages and recorded flags.
struct Linked {
    image: Vec<u8>,
    messages: Vec<Diagnostic>,
    flags: Vec<String>,
}

fn link(
    architecture: Architecture,
    target: &Target,
    source: &str,
    settings: &CompilerSettings,
) -> Result<Linked, Vec<Diagnostic>> {
    if source.len() > MAX_SOURCE {
        return Err(error("Source exceeds 256 KiB"));
    }
    let rejected = check::check(source);
    if !rejected.is_empty() {
        return Err(rejected);
    }
    let clang = tools::clang().map_err(error)?;
    let linker = vole_isa_scalar::linker_status().map_err(error)?;
    let temp = tempfile::Builder::new()
        .prefix("vole-c-")
        .tempdir()
        .map_err(|e| error(e.to_string()))?;
    let directory = temp.path();
    fs::create_dir(directory.join("include"))
        .and_then(|_| fs::write(directory.join("include").join("vole.h"), VOLE_H))
        .and_then(|_| fs::write(directory.join(dwarf::USER_FILE), source))
        .and_then(|_| fs::write(directory.join("memory.ld"), LINK_SCRIPT))
        .map_err(|e| error(e.to_string()))?;
    let runtime = runtime_objects(architecture, target, &clang, directory)?;
    fs::write(directory.join("crt0.o"), &runtime.crt0)
        .and_then(|_| fs::write(directory.join("vole_runtime.o"), &runtime.runtime))
        .map_err(|e| error(e.to_string()))?;

    let flags = c_flags(target, settings);
    dependency_gate(&clang, &flags, directory, source)?;
    let object_path = directory.join("main.o");
    let output = run_tool(
        &clang.path,
        &invocation(&clang, &flags, dwarf::USER_FILE, "main.o"),
        directory,
        &object_path,
        CLANG_DEADLINE,
    )
    .map_err(error)?;
    let mut messages = diagnostics::clang(&output.diagnostics, source);
    if !output.success {
        if !messages.iter().any(Diagnostic::is_error) {
            messages.push(Diagnostic::new(
                1,
                format!("Compilation failed: {}", output.diagnostics.trim()),
            ));
        }
        return Err(messages);
    }

    let image_path = directory.join("program.elf");
    let link_args: Vec<OsString> = [
        "-T",
        "memory.ld",
        "--build-id=none",
        "crt0.o",
        "main.o",
        "vole_runtime.o",
        "-o",
        "program.elf",
    ]
    .map(OsString::from)
    .to_vec();
    let output =
        run_tool(&linker, &link_args, directory, &image_path, LINK_DEADLINE).map_err(error)?;
    if !output.success {
        let mut errors = diagnostics::linker(&output.diagnostics, source);
        messages.retain(|message| message.severity != Severity::Error);
        errors.extend(messages);
        return Err(errors);
    }
    let image = fs::read(&image_path).map_err(|e| error(e.to_string()))?;
    Ok(Linked {
        image,
        messages,
        flags,
    })
}

fn decode_text(
    architecture: Architecture,
    target: &Target,
    base: u64,
    bytes: &[u8],
) -> Vec<Instruction> {
    let mut instructions = Vec::new();
    let mut offset = 0;
    while offset < bytes.len() {
        let address = base + offset as u64;
        let instruction = vole_isa_scalar::decode(architecture, address, &bytes[offset..])
            .unwrap_or_else(|_| {
                let width = if target.fixed_width { 4 } else { 1 }.min(bytes.len() - offset);
                let raw = &bytes[offset..offset + width];
                let value = raw
                    .iter()
                    .rev()
                    .fold(String::new(), |text, byte| format!("{text}{byte:02x}"));
                Instruction {
                    address,
                    bytes: raw.to_vec(),
                    assembly: if width == 4 {
                        format!(".word 0x{value}")
                    } else {
                        format!(".byte 0x{value}")
                    },
                    explanation: "Undecodable bytes; execution will fault here.".into(),
                    source_line: None,
                    reads: vec![],
                    writes: vec![],
                }
            });
        offset += instruction.bytes.len().max(1);
        instructions.push(instruction);
    }
    instructions
}

/// Builds the program image from the linked ELF and extracts its debug information.
fn load(
    architecture: Architecture,
    target: &Target,
    source: &str,
    bytes: &[u8],
    debug: DebugInfo,
) -> Result<Program, Vec<Diagnostic>> {
    let file = object::File::parse(bytes).map_err(|e| error(format!("Invalid linked ELF: {e}")))?;
    let mut code = Vec::new();
    let mut rodata = vec![0; RODATA_SIZE as usize];
    let mut data = vec![0; DATA_SIZE as usize];
    for section in file.sections() {
        let allocated = matches!(section.flags(), SectionFlags::Elf { sh_flags } if sh_flags & u64::from(object::elf::SHF_ALLOC) != 0);
        if !allocated || section.size() == 0 {
            continue;
        }
        let name = section.name().unwrap_or("?");
        let address = section.address();
        let size = section.size();
        let content = section.data().map_err(|e| error(e.to_string()))?;
        let (region, base, limit): (&mut Vec<u8>, u64, u64) = match name {
            ".text" if address == CODE_BASE => {
                code = content.to_vec();
                if address + size > CODE_END {
                    return Err(error(
                        "The program's machine code exceeds the 28 KiB code region",
                    ));
                }
                continue;
            }
            ".rodata" => (&mut rodata, RODATA_BASE, RODATA_SIZE),
            ".data" | ".bss" => (&mut data, DATA_BASE, DATA_SIZE),
            _ => {
                return Err(error(format!(
                    "Section {name} at 0x{address:x} is outside the Vole memory map"
                )));
            }
        };
        let offset = address
            .checked_sub(base)
            .filter(|offset| offset + size <= limit)
            .ok_or_else(|| error(format!("Section {name} does not fit its memory region")))?
            as usize;
        // .bss has no file content and stays zero.
        region[offset..offset + content.len()].copy_from_slice(content);
    }
    if code.is_empty() {
        return Err(error("The program has no machine code"));
    }
    let mut symbols = BTreeMap::new();
    let mut function_symbols = Vec::new();
    for symbol in file.symbols() {
        let Ok(name) = symbol.name() else { continue };
        if name.is_empty()
            || name.starts_with('$')
            || name.starts_with(".L")
            || !symbol.is_definition()
            || matches!(symbol.kind(), SymbolKind::Section | SymbolKind::File)
        {
            continue;
        }
        // Prefer global definitions over same-named static ones.
        if symbol.is_global() || !symbols.contains_key(name) {
            symbols.insert(name.to_owned(), symbol.address());
        }
        if symbol.kind() == SymbolKind::Text {
            function_symbols.push(dwarf::FunctionSymbol {
                name: name.to_owned(),
                address: symbol.address(),
                size: symbol.size(),
            });
        }
    }
    let debug = dwarf::extract(&file, architecture, debug, &function_symbols).map_err(error)?;
    let mut instructions = decode_text(architecture, target, CODE_BASE, &code);
    for instruction in &mut instructions {
        instruction.source_line = debug
            .row_for_address(instruction.address)
            .filter(|row| row.file == 0 && row.line > 0)
            .map(|row| row.line as usize);
    }
    Ok(Program {
        architecture,
        source: source.to_owned(),
        entry: file.entry(),
        regions: vec![
            MemoryRegion {
                base: CODE_BASE,
                bytes: code,
                writable: false,
                executable: true,
                label: "Code".into(),
            },
            MemoryRegion {
                base: RODATA_BASE,
                bytes: rodata,
                writable: false,
                executable: false,
                label: "Read-only data".into(),
            },
            MemoryRegion {
                base: DATA_BASE,
                bytes: data,
                writable: true,
                executable: false,
                label: "Data".into(),
            },
            MemoryRegion {
                base: STACK_BASE,
                bytes: vec![0; STACK_SIZE as usize],
                writable: true,
                executable: false,
                label: "Stack".into(),
            },
        ],
        instructions,
        symbols,
        initial_registers: BTreeMap::from([(target.stack_register.to_string(), STACK_TOP)]),
        language: SourceLanguage::C,
        debug: Some(debug),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use vole_core::{Optimization, debug::Location};

    /// The gate itself, without the lexical pre-check in front of it.
    #[test]
    fn dependency_gate_allows_only_private_and_resource_headers() {
        let Ok(clang) = tools::clang() else {
            return;
        };
        let target = target(Architecture::X64).unwrap();
        let flags = c_flags(&target, &CompilerSettings::default());
        let host = tempfile::tempdir().unwrap();
        let secret = host.path().join("secret.h");
        fs::write(&secret, "int secret_value_from_host;\n").unwrap();
        let gate = |source: &str| {
            let directory = tempfile::tempdir().unwrap();
            fs::create_dir(directory.path().join("include")).unwrap();
            fs::write(directory.path().join("include/vole.h"), VOLE_H).unwrap();
            fs::write(directory.path().join(dwarf::USER_FILE), source).unwrap();
            dependency_gate(&clang, &flags, directory.path(), source)
        };
        gate("#include <vole.h>\n#include <stdint.h>\nint main(void) { return 0; }\n").unwrap();
        for source in [
            format!(
                "#include \"{}\"\nint main(void) {{ return 0; }}\n",
                secret.display()
            ),
            "#include \"/no/such/vole/file.h\"\nint main(void) { return 0; }\n".to_string(),
        ] {
            let errors = gate(&source).unwrap_err();
            assert_eq!(errors.len(), 1, "{errors:?}");
            assert_eq!(errors[0].message, HOST_FILE);
        }
    }

    const TARGETS: [Architecture; 4] = [
        Architecture::Arm64,
        Architecture::X64,
        Architecture::Arm32,
        Architecture::X86,
    ];

    fn dwarfdump() -> Option<&'static str> {
        ["llvm-dwarfdump-14", "llvm-dwarfdump"]
            .into_iter()
            .find(|tool| Command::new(tool).arg("--version").output().is_ok())
    }

    fn hex_after(block: &str, attribute: &str) -> Option<u64> {
        let rest = block.split_once(&format!("{attribute}\t(0x"))?.1;
        u64::from_str_radix(rest.split(')').next()?, 16).ok()
    }

    fn string_after<'a>(block: &'a str, attribute: &str) -> Option<&'a str> {
        let rest = block.split_once(&format!("{attribute}\t(\""))?.1;
        rest.split("\")").next()
    }

    /// Cross-checks the extracted model against LLVM's independent DWARF reader.
    #[test]
    fn debug_info_matches_llvm_dwarfdump() {
        let Some(tool) = dwarfdump() else {
            eprintln!("llvm-dwarfdump not installed; skipping");
            return;
        };
        if tools::clang().is_err() || vole_isa_scalar::linker_status().is_err() {
            eprintln!("Clang/LLD not installed; skipping");
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        for architecture in TARGETS {
            for optimization in [Optimization::O0, Optimization::O1] {
                let settings = CompilerSettings {
                    optimization,
                    warnings: true,
                };
                let target = target(architecture).unwrap();
                let linked = link(architecture, &target, default_example(), &settings).unwrap();
                let program = load(
                    architecture,
                    &target,
                    default_example(),
                    &linked.image,
                    DebugInfo::default(),
                )
                .unwrap();
                let debug = program.debug.unwrap();
                let path = directory.path().join("program.elf");
                fs::write(&path, &linked.image).unwrap();
                let dump = |section: &str| {
                    let output = Command::new(tool).arg(section).arg(&path).output().unwrap();
                    String::from_utf8(output.stdout).unwrap()
                };
                let context = format!("{architecture:?} {optimization:?}");

                let info = dump("--debug-info");
                let mut checked = 0;
                for block in info.split("\n\n") {
                    if block.contains("DW_TAG_subprogram")
                        && let (Some(low), Some(high), Some(name)) = (
                            hex_after(block, "DW_AT_low_pc"),
                            hex_after(block, "DW_AT_high_pc"),
                            string_after(block, "DW_AT_name"),
                        )
                    {
                        assert!(
                            debug
                                .functions
                                .iter()
                                .any(|f| f.name == name && f.low_pc == low && f.high_pc == high),
                            "{context}: function {name} 0x{low:x}-0x{high:x}"
                        );
                        checked += 1;
                    }
                    if optimization == Optimization::O0
                        && block.contains("DW_TAG_variable")
                        && string_after(block, "DW_AT_name") == Some("total")
                        && let Some(offset) = block
                            .split_once("DW_OP_fbreg ")
                            .and_then(|(_, rest)| rest.split(')').next())
                            .and_then(|text| text.parse::<i64>().ok())
                    {
                        let ours: Vec<_> = debug
                            .functions
                            .iter()
                            .flat_map(|f| &f.variables)
                            .filter(|v| v.name == "total")
                            .map(|v| v.location.clone())
                            .collect();
                        assert!(
                            ours.contains(&Location::FrameOffset(offset)),
                            "{context}: total at fbreg {offset}, ours {ours:?}"
                        );
                    }
                }
                assert!(checked >= 10, "{context}: only {checked} functions");

                let lines = dump("--debug-line");
                let rows = lines
                    .lines()
                    .filter(|line| line.starts_with("0x") && line.split_whitespace().count() >= 6)
                    .count();
                assert_eq!(rows, debug.lines.len(), "{context}: line rows");

                let frames = dump("--debug-frame");
                let mut fdes = 0;
                for line in frames.lines().filter(|line| line.contains(" FDE ")) {
                    let range = line.split_once("pc=").unwrap().1;
                    let (start, end) = range.split_once("...").unwrap();
                    let start = u64::from_str_radix(start, 16).unwrap();
                    let end = u64::from_str_radix(end.trim(), 16).unwrap();
                    assert!(
                        debug.unwind.iter().any(|row| row.start == start),
                        "{context}: FDE at 0x{start:x}"
                    );
                    assert_eq!(
                        debug.unwind_row(end - 1).map(|row| row.end),
                        Some(end),
                        "{context}: FDE end 0x{end:x}"
                    );
                    fdes += 1;
                }
                assert!(fdes >= 10, "{context}: only {fdes} FDEs");
            }
        }
    }
}
