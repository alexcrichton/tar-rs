// Copyright 2024 Google LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use std::io::{Cursor, Write};
use std::fs::File;
use std::path::{Path, PathBuf, Component};
use tar::{Archive, Builder, EntryType, Header};
use tempfile::tempdir;

// Define ArchiveEntry for arbitrary crate
#[derive(Debug)]
struct ArchiveEntry {
    path: String,
    entry_type: u8,
    content: Vec<u8>,
}

// Implement Arbitrary for ArchiveEntry
impl<'a> Arbitrary<'a> for ArchiveEntry {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let path: String = u.arbitrary::<&str>()?.to_string();
        let entry_type: u8 = u.arbitrary()?;
        let content: Vec<u8> = u.arbitrary()?;
        
        Ok(ArchiveEntry { path, entry_type, content })
    }
}

// Define FuzzInput for arbitrary crate
#[derive(Debug)]
struct FuzzInput {
    entries: Vec<ArchiveEntry>,
}

// Implement Arbitrary for FuzzInput
impl<'a> Arbitrary<'a> for FuzzInput {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        let entries: Vec<ArchiveEntry> = u.arbitrary()?;
        Ok(FuzzInput { entries })
    }
}

fuzz_target!(|data: &[u8]| {
    // Prepare FuzzInput with Arbitrary
    let mut unstructured = Unstructured::new(data);
    let input: FuzzInput = match FuzzInput::arbitrary(&mut unstructured) {
        Ok(val) => val,
        Err(_) => return,
    };

    // Create a temporary directory for the tar file; exit if creation fails
    let temp_dir = match tempdir() {
        Ok(dir) => dir,
        Err(_) => return,
    };
    let temp_file_path = temp_dir.path().join("archive_file.tar");
    let mut builder = Builder::new(Vec::new());

    // Iterate through the archive entries to build a tar structure
    for entry in &input.entries {
        let mut header = Header::new_gnu();

        // Ensure content size is reasonable to avoid potential overflow issues
        let file_size = entry.content.len() as u64;
        if file_size > u32::MAX as u64 {
            continue; // Skip large entries
        }
        header.set_size(file_size);

        // Determine the entry type from fuzzed data
        let entry_type = match entry.entry_type % 5 {
            0 => EntryType::Regular,
            1 => EntryType::Directory,
            2 => EntryType::Symlink,
            3 => EntryType::hard_link(),
            _ => EntryType::character_special(),
        };
        header.set_entry_type(entry_type);

        // Process entry types based on directory structure and content
        match entry_type {
            EntryType::Directory => {
                // Sanitize the path to prevent directory traversal
                let safe_path: PathBuf = Path::new(&entry.path)
                    .components()
                    .filter(|component| matches!(component, Component::Normal(_)))
                    .collect();

                if safe_path != Path::new(&entry.path) {
                    continue;
                }

                let dir_path = temp_dir.path().join(&safe_path);
                if std::fs::create_dir_all(&dir_path).is_err() {
                    continue;
                }
                if builder.append_dir(safe_path.clone(), &dir_path).is_err() {
                    continue;
                }
            }
            EntryType::Regular => {
                let mut cursor = Cursor::new(entry.content.clone());
                if builder.append_data(&mut header, entry.path.as_str(), &mut cursor).is_err() {
                    continue;
                }
            }
            _ => {
                // Handle other types with appropriate mock content or skip unsupported
                let mut cursor = Cursor::new(entry.content.clone());
                if builder.append_data(&mut header, entry.path.as_str(), &mut cursor).is_err() {
                    continue;
                }
            }
        }
    }

    // Write the builder content to the temporary tar file
    let mut temp_file = File::create(&temp_file_path).unwrap();
    if temp_file.write_all(&builder.into_inner().unwrap_or_default()).is_ok() {
        let mut archive = Archive::new(temp_file);
        if let Ok(entries) = archive.entries() {
            for entry in entries {
                if entry.is_err() {
                    return;
                }
            }
        }
    }
});
