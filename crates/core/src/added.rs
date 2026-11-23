//! Folders copied into a sandbox after it was made. OpenShell keeps a sandbox's annotations as they were when it was
//! made, so Playpen keeps its own note of these, a file for each sandbox, which says when the sandbox was made: a
//! sandbox made later under the same name isn't given another's folders.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::files::write_atomically;
use crate::openshell::{OpenShell, OpenShellError, Sandbox};

/// Where the folders go in a sandbox, side by side, as they do when it's made.
const INTO: &str = "/sandbox";

/// Why folders couldn't be added to a sandbox.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddError {
    #[error(
        "Two of the folders are called “{0}”, and each is copied in under its own name. Leave one out, or rename it."
    )]
    SameName(String),
    #[error(
        "There's a “{0}” in the sandbox already, which copying would mix into. Rename the folder, or move that one aside in a shell."
    )]
    Taken(String),
    #[error(transparent)]
    OpenShell(#[from] OpenShellError),
    #[error("{0}")]
    Seed(String),
    #[error("Couldn't note the folders added to “{0}”: {1}")]
    Note(String, String),
}

/// What's noted for a sandbox.
#[derive(Debug, Serialize, Deserialize)]
struct Note {
    /// When the sandbox was made, as OpenShell lists it, so the note stays with that sandbox.
    created: String,
    folders: Vec<String>,
}

/// The notes of folders added to sandboxes, kept in a folder of Playpen's.
#[derive(Debug, Clone)]
pub struct AddedFolders {
    notes: PathBuf,
}

impl AddedFolders {
    pub fn new(notes: impl Into<PathBuf>) -> AddedFolders {
        AddedFolders {
            notes: notes.into(),
        }
    }

    fn note_path(&self, name: &str) -> PathBuf {
        self.notes.join(format!("{name}.json"))
    }

    /// The folders added to `sandbox`, oldest first.
    pub fn of(&self, sandbox: &Sandbox) -> Vec<String> {
        std::fs::read_to_string(self.note_path(&sandbox.name))
            .ok()
            .and_then(|text| serde_json::from_str::<Note>(&text).ok())
            .filter(|note| note.created == sandbox.created)
            .map(|note| note.folders)
            .unwrap_or_default()
    }

    /// Puts the folders added to each sandbox after those it was made with.
    pub fn fill(&self, sandboxes: &mut [Sandbox]) {
        for sandbox in sandboxes {
            let added = self.of(sandbox);
            sandbox.folders.extend(added);
        }
    }

    /// Copies `folders` into the running `sandbox`, beside what's there, and notes them. Nothing is copied if any
    /// would land on something of the same name.
    pub fn add(
        &self,
        openshell: &OpenShell,
        sandbox: &Sandbox,
        folders: &[PathBuf],
    ) -> Result<(), AddError> {
        let mut names = BTreeSet::new();
        for name in folders.iter().filter_map(|folder| folder.file_name()) {
            if !names.insert(name.to_os_string()) {
                return Err(AddError::SameName(name.to_string_lossy().into_owned()));
            }
        }
        let there = openshell.exec_with_input(
            &sandbox.name,
            &format!("look in “{}”", sandbox.name),
            &["ls", "-A", INTO],
            b"",
        )?;
        if let Some(taken) = taken(&there, &names) {
            return Err(AddError::Taken(taken));
        }
        for folder in folders {
            match crate::bring_back::Repo::of(folder) {
                Some(repo) => crate::bring_back::seed(openshell, &sandbox.name, &repo)
                    .map_err(|error| AddError::Seed(error.to_string()))?,
                None => openshell.upload(&sandbox.name, folder, INTO)?,
            }
        }
        let mut added = self.of(sandbox);
        added.extend(folders.iter().map(|folder| folder.display().to_string()));
        self.write(sandbox, added)
    }

    fn write(&self, sandbox: &Sandbox, folders: Vec<String>) -> Result<(), AddError> {
        let failed = |error: String| AddError::Note(sandbox.name.clone(), error);
        let note = Note {
            created: sandbox.created.clone(),
            folders,
        };
        let text =
            serde_json::to_string_pretty(&note).map_err(|error| failed(error.to_string()))?;
        std::fs::create_dir_all(&self.notes).map_err(|error| failed(error.to_string()))?;
        write_atomically(&self.note_path(&sandbox.name), text.as_bytes())
            .map_err(|error| failed(error.to_string()))
    }

    /// Forgets what was added to the sandbox `name`, once it's deleted.
    pub fn forget(&self, name: &str) {
        let _ = std::fs::remove_file(self.note_path(name));
    }

    /// Where the notes are.
    pub fn notes(&self) -> &Path {
        &self.notes
    }
}

/// The first of `names` that `ls -A` listed.
fn taken(listed: &str, names: &BTreeSet<OsString>) -> Option<String> {
    listed
        .lines()
        .map(str::trim)
        .find(|there| names.contains(&OsString::from(there)))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::ffi::OsString;

    use super::{AddedFolders, taken};
    use crate::openshell::{Phase, Sandbox};

    fn sandbox(created: &str) -> Sandbox {
        Sandbox {
            name: "claude-patchbay".to_string(),
            phase: Phase::Ready,
            created: created.to_string(),
            agent: Some("claude-code".to_string()),
            folders: vec!["/src/Patchbay".to_string()],
            workdir: Some("/sandbox".to_string()),
            custom: None,
        }
    }

    #[test]
    fn folders_added_follow_those_it_was_made_with_and_stay_with_that_sandbox() {
        let notes = std::env::temp_dir().join(format!("added-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&notes);
        let added = AddedFolders::new(&notes);
        added
            .write(
                &sandbox("2026-11-21 09:41:00"),
                vec!["/src/Typecase".to_string()],
            )
            .unwrap();
        let mut sandboxes = vec![sandbox("2026-11-21 09:41:00")];
        added.fill(&mut sandboxes);
        assert_eq!(sandboxes[0].folders, ["/src/Patchbay", "/src/Typecase"]);
        assert!(
            added.of(&sandbox("2026-11-22 10:00:00")).is_empty(),
            "a sandbox made later under the name"
        );
        added.forget("claude-patchbay");
        assert!(added.of(&sandbox("2026-11-21 09:41:00")).is_empty());
    }

    #[test]
    fn a_name_in_the_sandbox_already_is_found() {
        let names: BTreeSet<OsString> = ["Typecase".into(), "Red-Pen".into()].into();
        assert_eq!(
            taken(".claude\nPatchbay\nTypecase\n", &names).as_deref(),
            Some("Typecase")
        );
        assert_eq!(taken(".claude\nPatchbay\n", &names), None);
    }
}
