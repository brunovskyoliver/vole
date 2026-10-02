//! Versioned portable project documents, independent of the desktop UI.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::Path,
};
use vole_core::{Architecture, Program, SimError, Snapshot};

pub const PROJECT_VERSION: u32 = 1;
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
        }
    }

    pub fn validate(&self) -> Result<(), SimError> {
        if self.version != PROJECT_VERSION {
            return Err(SimError(format!(
                "Project version {} is unsupported; this app reads version {PROJECT_VERSION}.",
                self.version
            )));
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
        project.version = 2;
        assert!(project.to_json().unwrap_err().0.contains("version 2"));
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
}
