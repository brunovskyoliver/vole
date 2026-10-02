//! Versioned portable project documents, independent of the desktop UI.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::Path,
};
use vole_core::{Architecture, CompilerSettings, Program, SimError, Snapshot, SourceLanguage};

/// Assembly projects keep version 1 so earlier releases can still open them.
pub const PROJECT_VERSION: u32 = 1;
/// C projects add language, compiler settings and source-line breakpoints.
pub const C_PROJECT_VERSION: u32 = 2;
pub const MAX_PROJECT_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub version: u32,
    pub architecture: Architecture,
    pub source: String,
    pub breakpoints: BTreeSet<u64>,
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub image: Option<Program>,
    #[serde(default)]
    pub snapshot: Option<Snapshot>,
    #[serde(default, skip_serializing_if = "is_assembly")]
    pub language: SourceLanguage,
    #[serde(default, skip_serializing_if = "is_default_settings")]
    pub compiler: CompilerSettings,
    /// 1-based user source lines. Addresses are re-resolved after each build.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub source_breakpoints: BTreeSet<usize>,
}

fn is_assembly(language: &SourceLanguage) -> bool {
    *language == SourceLanguage::Assembly
}

fn is_default_settings(settings: &CompilerSettings) -> bool {
    *settings == CompilerSettings::default()
}

fn default_profile() -> String {
    "simpsim-extended-v1".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Layout {
    pub source_fraction: f32,
    pub top_fraction: f32,
    pub inspector_width: f32,
    pub memory_rows: usize,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            source_fraction: 0.43,
            top_fraction: 0.50,
            inspector_width: 320.0,
            memory_rows: 16,
        }
    }
}

impl Project {
    pub fn new(architecture: Architecture, source: impl Into<String>) -> Self {
        Self {
            version: PROJECT_VERSION,
            architecture,
            source: source.into(),
            breakpoints: BTreeSet::new(),
            profile: if architecture == Architecture::Vole {
                default_profile()
            } else {
                "scalar-v1".into()
            },
            layout: Layout::default(),
            image: None,
            snapshot: None,
            language: SourceLanguage::Assembly,
            compiler: CompilerSettings::default(),
            source_breakpoints: BTreeSet::new(),
        }
    }

    /// A C document for a real-ISA target, saved as a version 2 project.
    pub fn new_c(
        architecture: Architecture,
        source: impl Into<String>,
        compiler: CompilerSettings,
    ) -> Self {
        let mut project = Self::new(architecture, source);
        project.version = C_PROJECT_VERSION;
        project.language = SourceLanguage::C;
        project.compiler = compiler;
        project
    }

    pub fn validate(&self) -> Result<(), SimError> {
        if self.version != PROJECT_VERSION && self.version != C_PROJECT_VERSION {
            return Err(SimError(format!(
                "Project version {} is unsupported; this app reads versions {PROJECT_VERSION} and {C_PROJECT_VERSION}.",
                self.version
            )));
        }
        if self.version == PROJECT_VERSION
            && (self.language != SourceLanguage::Assembly
                || !self.source_breakpoints.is_empty()
                || !is_default_settings(&self.compiler))
        {
            return Err(SimError(
                "Version 1 projects contain assembly only; C documents use version 2.".into(),
            ));
        }
        if self.language == SourceLanguage::C && self.architecture == Architecture::Vole {
            return Err(SimError(
                "C projects target ARM32, ARM64, x86 or x64.".into(),
            ));
        }
        if self
            .source_breakpoints
            .iter()
            .any(|line| *line == 0 || *line > 1_000_000)
        {
            return Err(SimError("Source breakpoint lines are invalid.".into()));
        }
        if self
            .image
            .as_ref()
            .is_some_and(|image| image.language != self.language)
        {
            return Err(SimError(
                "Saved executable was built from a different source language.".into(),
            ));
        }
        if self.source.len() > 1024 * 1024 {
            return Err(SimError("Assembly source exceeds the 1 MiB limit.".into()));
        }
        let expected = if self.architecture == Architecture::Vole {
            "simpsim-extended-v1"
        } else {
            "scalar-v1"
        };
        if self.profile != expected {
            return Err(SimError(format!(
                "Profile {} is unsupported for {}.",
                self.profile,
                self.architecture.name()
            )));
        }
        if !self.layout.source_fraction.is_finite()
            || !(0.15..=0.85).contains(&self.layout.source_fraction)
            || !self.layout.top_fraction.is_finite()
            || !(0.15..=0.85).contains(&self.layout.top_fraction)
            || !self.layout.inspector_width.is_finite()
            || !(180.0..=600.0).contains(&self.layout.inspector_width)
            || !(4..=64).contains(&self.layout.memory_rows)
        {
            return Err(SimError("Project pane sizes are invalid.".into()));
        }
        if self.breakpoints.iter().any(|address| {
            self.architecture == Architecture::Vole && *address > 255
                || self.architecture.bits() == 32 && *address > u32::MAX as u64
        }) {
            return Err(SimError(
                "Breakpoint addresses must fit the selected architecture.".into(),
            ));
        }
        if self
            .image
            .as_ref()
            .is_some_and(|image| image.architecture != self.architecture)
            || self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.architecture != self.architecture)
            || self.snapshot.is_some() && self.image.is_none()
        {
            return Err(SimError(
                "Saved machine state does not match the project architecture/image.".into(),
            ));
        }
        if self
            .image
            .as_ref()
            .is_some_and(|image| image.source != self.source)
        {
            return Err(SimError("Saved executable belongs to different source. Assemble the current document or save source only.".into()));
        }
        Ok(())
    }

    pub fn from_json(json: &str) -> Result<Self, SimError> {
        if json.len() > MAX_PROJECT_BYTES {
            return Err(SimError("Project exceeds the 32 MiB limit.".into()));
        }
        let value: Self = serde_json::from_str(json)
            .map_err(|error| SimError(format!("Invalid project: {error}")))?;
        value.validate()?;
        Ok(value)
    }

    pub fn to_json(&self) -> Result<String, SimError> {
        self.validate()?;
        let mut persisted = self.clone();
        if let Some(snapshot) = &mut persisted.snapshot {
            snapshot.trace.clear();
        }
        let json = serde_json::to_string_pretty(&persisted)
            .map_err(|error| SimError(error.to_string()))?;
        if json.len() > MAX_PROJECT_BYTES {
            return Err(SimError("Project exceeds the 32 MiB limit.".into()));
        }
        Ok(json)
    }

    pub fn open(path: &Path) -> Result<Self, SimError> {
        let mut bytes = Vec::new();
        fs::File::open(path)
            .map_err(|error| SimError(error.to_string()))?
            .take(MAX_PROJECT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| SimError(error.to_string()))?;
        let json =
            std::str::from_utf8(&bytes).map_err(|_| SimError("Project must be UTF-8.".into()))?;
        Self::from_json(json)
    }

    pub fn save(&self, path: &Path) -> Result<(), SimError> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| SimError(error.to_string()))?;
        temporary
            .write_all(self.to_json()?.as_bytes())
            .and_then(|_| temporary.as_file().sync_all())
            .map_err(|error| SimError(error.to_string()))?;
        temporary
            .persist(path)
            .map_err(|error| SimError(format!("Could not save {}: {error}", path.display())))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_round_trip_architecture_source_and_breakpoints() {
        let mut project = Project::new(Architecture::Arm64, Architecture::Arm64.example_source());
        project.breakpoints.insert(0x1008);
        project.layout.top_fraction = 0.65;
        assert_eq!(
            Project::from_json(&project.to_json().unwrap()).unwrap(),
            project
        );
    }

    #[test]
    fn previous_layout_documents_receive_the_default_vertical_split() {
        let json = Project::new(Architecture::Vole, "halt").to_json().unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["layout"]
            .as_object_mut()
            .unwrap()
            .remove("top_fraction");
        let restored = Project::from_json(&value.to_string()).unwrap();
        assert_eq!(restored.layout.top_fraction, 0.5);
        assert_eq!(restored.layout.source_fraction, 0.43);
    }

    #[test]
    fn newer_project_versions_and_wrong_profiles_are_rejected() {
        let mut project = Project::new(Architecture::Vole, "halt");
        project.version = 3;
        assert!(project.to_json().unwrap_err().0.contains("version 3"));
        project.version = 1;
        project.profile = "ieee-float8".into();
        assert!(project.to_json().unwrap_err().0.contains("unsupported"));
    }

    #[test]
    fn invalid_layout_and_breakpoint_addresses_are_rejected() {
        let mut project = Project::new(Architecture::Vole, "halt");
        project.breakpoints.insert(256);
        assert!(project.validate().is_err());
        project.breakpoints.clear();
        project.layout.source_fraction = 0.0;
        assert!(project.validate().is_err());
    }

    #[test]
    fn failed_save_preserves_the_existing_document() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("lesson.voleproject");
        let mut project = Project::new(Architecture::Vole, "halt");
        project.save(&path).unwrap();
        let original = fs::read(&path).unwrap();
        project.version = 99;
        assert!(project.save(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(Project::open(&path).unwrap().source, "halt");
    }

    #[test]
    fn executable_from_previous_source_is_rejected_before_saving() {
        let mut project = Project::new(Architecture::Vole, "load R1, 2\nhalt");
        project.image = Some(Program {
            architecture: Architecture::Vole,
            source: "load R1, 1\nhalt".into(),
            entry: 0,
            regions: vec![],
            instructions: vec![],
            symbols: Default::default(),
            initial_registers: Default::default(),
            language: Default::default(),
            debug: None,
        });
        assert!(
            project
                .to_json()
                .unwrap_err()
                .0
                .contains("different source")
        );
        project.image = None;
        let restored = Project::from_json(&project.to_json().unwrap()).unwrap();
        assert_eq!(restored.source, "load R1, 2\nhalt");
        assert!(restored.image.is_none());
    }

    fn c_image(source: &str) -> Program {
        use vole_core::debug::{DebugInfo, LineRow, SourceFile};
        Program {
            architecture: Architecture::Arm64,
            source: source.into(),
            entry: 0x1000,
            regions: vec![vole_core::MemoryRegion {
                base: 0x1000,
                bytes: vec![0x00, 0x00, 0x20, 0xd4],
                writable: false,
                executable: true,
                label: "Code".into(),
            }],
            instructions: vec![],
            symbols: [("main".to_string(), 0x1000)].into(),
            initial_registers: [("sp".to_string(), 0x20000)].into(),
            language: SourceLanguage::C,
            debug: Some(DebugInfo {
                producer: "clang".into(),
                triple: "aarch64-none-elf".into(),
                settings: CompilerSettings {
                    optimization: vole_core::Optimization::O1,
                    warnings: false,
                },
                files: vec![SourceFile {
                    name: "main.c".into(),
                    user: true,
                }],
                lines: vec![LineRow {
                    address: 0x1000,
                    file: 0,
                    line: 1,
                    column: 1,
                    is_stmt: true,
                    prologue_end: true,
                    end_sequence: false,
                }],
                return_address_register: "x30".into(),
                ..Default::default()
            }),
        }
    }

    #[test]
    fn c_projects_round_trip_language_settings_breakpoints_image_and_state() {
        let source = "int main(void) { return 0; }\n";
        let settings = CompilerSettings {
            optimization: vole_core::Optimization::O1,
            warnings: false,
        };
        let mut project = Project::new_c(Architecture::Arm64, source, settings.clone());
        project.source_breakpoints = [1, 4].into();
        project.breakpoints.insert(0x1000);
        project.image = Some(c_image(source));
        project.snapshot = Some(Snapshot {
            architecture: Architecture::Arm64,
            pc: 0x1000,
            registers: vec![],
            flags: Default::default(),
            memory: vec![],
            output: b"hi".to_vec(),
            halted: false,
            steps: 3,
            trace: vec![],
        });
        let json = project.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"], 2);
        assert_eq!(value["language"], "c");
        assert_eq!(value["compiler"]["optimization"], "O1");
        let restored = Project::from_json(&json).unwrap();
        assert_eq!(restored, project);
        assert_eq!(restored.compiler, settings);
        assert_eq!(restored.image.unwrap().debug.unwrap().lines.len(), 1);
    }

    #[test]
    fn version_one_assembly_documents_from_earlier_releases_still_open() {
        let json = r#"{
            "version": 1,
            "architecture": "arm64",
            "source": ".text\n.global _start\n_start:\n    brk #0\n",
            "breakpoints": [4096],
            "profile": "scalar-v1",
            "layout": {"source_fraction": 0.5, "top_fraction": 0.5, "inspector_width": 320.0, "memory_rows": 16},
            "image": null,
            "snapshot": null
        }"#;
        let project = Project::from_json(json).unwrap();
        assert_eq!(project.language, SourceLanguage::Assembly);
        assert_eq!(project.compiler, CompilerSettings::default());
        assert!(project.source_breakpoints.is_empty());
        assert!(project.breakpoints.contains(&0x1000));
    }

    #[test]
    fn version_one_documents_cannot_contain_c() {
        let mut project = Project::new_c(
            Architecture::Arm64,
            "int main(void) { return 0; }",
            CompilerSettings::default(),
        );
        project.version = PROJECT_VERSION;
        assert!(project.to_json().unwrap_err().0.contains("version 2"));
        let mut value: serde_json::Value = serde_json::from_str(
            &Project::new_c(
                Architecture::X64,
                "int main(void){return 0;}",
                Default::default(),
            )
            .to_json()
            .unwrap(),
        )
        .unwrap();
        value["version"] = 1.into();
        assert!(Project::from_json(&value.to_string()).is_err());
        let mut breakpoints_only = Project::new(Architecture::Arm64, "brk #0");
        breakpoints_only.source_breakpoints.insert(3);
        assert!(breakpoints_only.validate().is_err());
    }

    #[test]
    fn c_projects_reject_vole_targets_and_mismatched_images() {
        let mut project =
            Project::new_c(Architecture::Vole, "int main(void){}", Default::default());
        project.profile = default_profile();
        assert!(project.validate().unwrap_err().0.contains("C projects"));
        let mut project = Project::new_c(Architecture::Arm64, "int x;", Default::default());
        let mut image = c_image("int x;");
        image.language = SourceLanguage::Assembly;
        project.image = Some(image);
        assert!(project.validate().unwrap_err().0.contains("language"));
    }

    #[test]
    fn assembly_saves_stay_version_one_without_c_fields() {
        let mut project = Project::new(Architecture::Arm64, Architecture::Arm64.example_source());
        project.breakpoints.insert(0x1004);
        let json = project.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"], 1);
        let object = value.as_object().unwrap();
        for field in ["language", "compiler", "source_breakpoints"] {
            assert!(!object.contains_key(field), "{field} leaked into {json}");
        }
    }
}
