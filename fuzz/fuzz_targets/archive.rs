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
use cap_std::fs::Dir;
use cap_std::ambient_authority;
use libfuzzer_sys::fuzz_target;
use std::io::{Cursor, Write};
use tar::{Archive, Builder, EntryType, Header};
use tempfile::tempdir;

// Define ArchiveEntry for arbitrary crate
#[derive(Debug, Arbitrary)]
struct ArchiveEntry {
    path: String,
    entry_type: u8,
    content: Vec<u8>,
}

// Define FuzzInput for arbitrary crate
#[derive(Debug, Arbitrary)]
struct FuzzInput {
    entries: Vec<ArchiveEntry>,
}

fuzz_target!(|data: &[u8]| {
    // Prepare FuzzInput with Arbitrary
    let mut unstructured = Unstructured::new(data);
    let input: FuzzInput = match FuzzInput::arbitrary(&mut unstructured) {
        Ok(val) => val,
        Err(_) => return,
    };

    // Create a sandbox directory with cap_std
    let temp_dir = match tempdir() {
        Ok(dir) => dir,
        Err(_) => return,
    };
    let sandbox_dir = match Dir::open_ambient_dir(temp_dir.path(), ambient_authority()) {
        Ok(dir) => dir,
        Err(_) => return,
    };
    let temp_file_path = "archive_file.tar";
    let mut builder = Builder::new(Vec::new());

    // Iterate through the archive entries to build a tar structure
    for entry in &input.entries {
        let mut header = Header::new_gnu();

        // Ensure content size is reasonable to avoid potential overflow issues
        let file_size = entry.content.len() as u64;
        if file_size > u32::MAX as u64 {
            continue;
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

        // Process entry types using cap_std sandbox
        match entry_type {
            EntryType::Directory => {
                if let Err(_) = sandbox_dir.create_dir_all(&entry.path) {
                    continue;
                }
                if builder.append_dir(&entry.path, &entry.path).is_err() {
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

    // Write the builder content to the temporary tar file within the sandbox
    if let Ok(mut temp_file) = sandbox_dir.create(temp_file_path) {
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
    }

    // Cleanup temp directory and sandbox directory
    drop(sandbox_dir);
    drop(temp_dir);
});
