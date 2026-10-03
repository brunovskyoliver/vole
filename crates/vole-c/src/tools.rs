//! Clang discovery and bounded external-tool execution.
use std::{
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Largest object file or linked image accepted from a tool.
const MAX_OUTPUT: u64 = 16 * 1024 * 1024;
/// Everything a tool may write into its private directory while it runs.
const MAX_DIRECTORY: u64 = 64 * 1024 * 1024;
/// Diagnostics beyond this size stop the tool; only the first part is read.
const MAX_DIAGNOSTICS: u64 = 1024 * 1024;
const READ_DIAGNOSTICS: u64 = 256 * 1024;

/// Environment variables that would add host include paths or driver options.
const SCRUBBED_ENVIRONMENT: &[&str] = &[
    "CPATH",
    "C_INCLUDE_PATH",
    "CPLUS_INCLUDE_PATH",
    "OBJC_INCLUDE_PATH",
    "OBJCPLUS_INCLUDE_PATH",
    "CCC_OVERRIDE_OPTIONS",
    "COMPILER_PATH",
    "GCC_EXEC_PREFIX",
    "LIBRARY_PATH",
    "CL",
    "_CL_",
];

/// A discovered Clang driver and the resource directory holding its freestanding headers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Clang {
    pub path: PathBuf,
    /// `None` lets Clang use its built-in default.
    pub resource_dir: Option<PathBuf>,
}

fn candidate_names() -> Vec<String> {
    let mut names = vec!["clang-14".to_string(), "clang".to_string()];
    names.extend((15..=23).rev().map(|version| format!("clang-{version}")));
    names
        .into_iter()
        .map(|name| format!("{name}{}", std::env::consts::EXE_SUFFIX))
        .collect()
}

fn find_in(directory: &Path, names: &[String]) -> Option<PathBuf> {
    names
        .iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
}

fn clang_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("VOLE_CLANG") {
        return Some(PathBuf::from(path));
    }
    let names = candidate_names();
    if let Some(directory) = std::env::var_os("VOLE_TOOLCHAIN_DIR")
        && let Some(path) = find_in(Path::new(&directory), &names)
    {
        return Some(path);
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(parent) = executable.parent()
    {
        for directory in [
            parent.join("toolchain"),
            parent.join("../Resources/toolchain"),
        ] {
            if let Some(path) = find_in(&directory, &names) {
                return Some(path);
            }
        }
    }
    let search = std::env::var_os("PATH")?;
    for name in &names {
        for directory in std::env::split_paths(&search) {
            let path = directory.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Bundled layout: `toolchain/clang` beside `toolchain/lib/clang/<N>/include`.
fn bundled_resource_dir(clang: &Path) -> Option<PathBuf> {
    let versions = clang.parent()?.join("lib").join("clang");
    let mut entries: Vec<PathBuf> = fs::read_dir(versions)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.join("include").join("stddef.h").is_file())
        .collect();
    entries.sort();
    entries.pop()
}

pub(crate) fn clang() -> Result<Clang, String> {
    let path = clang_path().ok_or(
        "Clang not found. Install Clang 14 or newer (for example the clang-14 package), \
         or set VOLE_CLANG to the clang executable.",
    )?;
    let resource_dir = std::env::var_os("VOLE_CLANG_RESOURCE_DIR")
        .map(PathBuf::from)
        .or_else(|| bundled_resource_dir(&path));
    Ok(Clang { path, resource_dir })
}

/// Directory of Clang's freestanding headers, from the explicit resource
/// directory or by asking the driver. Cached per driver for the process.
pub(crate) fn resource_include(clang: &Clang) -> Result<PathBuf, String> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<Clang, PathBuf>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(path) = cache
        .lock()
        .ok()
        .and_then(|cache| cache.get(clang).cloned())
    {
        return Ok(path);
    }
    let resource = match &clang.resource_dir {
        Some(directory) => directory.clone(),
        None => {
            let mut command = Command::new(&clang.path);
            command
                .arg("-print-resource-dir")
                .stdin(Stdio::null())
                .stderr(Stdio::null());
            for name in SCRUBBED_ENVIRONMENT {
                command.env_remove(name);
            }
            let output = command
                .output()
                .map_err(|e| format!("Cannot run {}: {e}", clang.path.display()))?;
            PathBuf::from(String::from_utf8_lossy(&output.stdout).trim())
        }
    };
    let include = fs::canonicalize(resource.join("include"))
        .map_err(|e| format!("Clang's resource headers are missing: {e}"))?;
    if let Ok(mut cache) = cache.lock() {
        cache.insert(clang.clone(), include.clone());
    }
    Ok(include)
}

/// Total size of the files directly inside `directory`, including tools'
/// temporary outputs that are renamed only when they finish.
fn directory_size(directory: &Path) -> u64 {
    fs::read_dir(directory)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.metadata().ok())
                .filter(|metadata| metadata.is_file())
                .map(|metadata| metadata.len())
                .sum()
        })
        .unwrap_or(0)
}

/// Locate the Clang driver used for C documents, with an installation hint on failure.
pub fn clang_status() -> Result<PathBuf, String> {
    clang().map(|clang| clang.path)
}

/// Captured result of a tool that ran to completion within its budget.
pub(crate) struct ToolOutput {
    pub success: bool,
    pub diagnostics: String,
}

/// Run a tool with an argument array, closed stdin, a private working and
/// temporary directory, a deadline and bounded output. `Err` means the tool
/// could not run or exceeded its budget.
pub(crate) fn run_tool(
    tool: &Path,
    args: &[OsString],
    directory: &Path,
    output: &Path,
    deadline: Duration,
) -> Result<ToolOutput, String> {
    let errors = directory.join(format!(
        "{}.diagnostics.txt",
        output
            .file_name()
            .map_or("tool".into(), |n| n.to_string_lossy())
    ));
    let stderr = fs::File::create(&errors).map_err(|e| e.to_string())?;
    let mut command = Command::new(tool);
    command
        .args(args)
        .current_dir(directory)
        .env("TMPDIR", directory)
        .env("TEMP", directory)
        .env("TMP", directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr);
    for name in SCRUBBED_ENVIRONMENT {
        command.env_remove(name);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot run {}: {e}", tool.display()))?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        let elapsed = start.elapsed();
        if elapsed > deadline
            || fs::metadata(output).is_ok_and(|m| m.len() > MAX_OUTPUT)
            || fs::metadata(&errors).is_ok_and(|m| m.len() > MAX_DIAGNOSTICS)
            || directory_size(directory) > MAX_DIRECTORY
        {
            let _ = child.kill();
            let _ = child.wait();
            let name = tool
                .file_name()
                .map_or("tool".into(), |n| n.to_string_lossy());
            return Err(if elapsed > deadline {
                format!(
                    "{name} did not finish within {} seconds",
                    deadline.as_secs()
                )
            } else {
                format!("{name} exceeded its output-size budget")
            });
        }
        thread::sleep(Duration::from_millis(5));
    };
    let mut diagnostics = Vec::new();
    fs::File::open(&errors)
        .and_then(|file| file.take(READ_DIAGNOSTICS).read_to_end(&mut diagnostics))
        .map_err(|e| e.to_string())?;
    if status.success() && fs::metadata(output).map_err(|e| e.to_string())?.len() > MAX_OUTPUT {
        return Err("Tool output exceeds 16 MiB".into());
    }
    Ok(ToolOutput {
        success: status.success(),
        diagnostics: String::from_utf8_lossy(&diagnostics).into_owned(),
    })
}
