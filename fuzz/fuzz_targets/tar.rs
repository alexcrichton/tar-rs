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
use std::io::{Cursor, Read, Seek, Write};
use tar::{Archive, Builder, EntryType, Header};
use tempfile::{tempdir, NamedTempFile};

// Define FuzzInput for arbitrary crate
#[derive(Debug)]
struct FuzzInput {
    data: Vec<u8>,
    file_name: String,
    link_path: String,
    target_path: String,
    entry_type: u8,
    metadata_size: u64,
}

// Implement Arbitrary for FuzzInput
impl<'a> Arbitrary<'a> for FuzzInput {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(FuzzInput {
            data: u.arbitrary()?,
            file_name: u.arbitrary::<&str>()?.to_string(),
            link_path: u.arbitrary::<&str>()?.to_string(),
            target_path: u.arbitrary::<&str>()?.to_string(),
            entry_type: u.arbitrary()?,
            metadata_size: u.int_in_range(0..=1000)?,
        })
    }
}

fuzz_target!(|data: &[u8]| {
    // Prepare FuzzInput by Arbitrary crate
    let mut unstructured = Unstructured::new(data);
    let input: FuzzInput = match FuzzInput::arbitrary(&mut unstructured) {
        Ok(val) => val,
        Err(_) => return,
    };

    // Setup temporary directory and initialize builder
    let temp_dir = match tempdir() {
        Ok(dir) => dir,
        Err(_) => return,
    };
    let archive_data = Cursor::new(&input.data);
    let mut builder = Builder::new(Cursor::new(Vec::new()));
    let mut header = Header::new_gnu();

    // Set random header metadata
    header.set_size(input.metadata_size.min(input.data.len() as u64));
    header.set_cksum();
    let entry_type = match input.entry_type % 5 {
        0 => EntryType::Regular,
        1 => EntryType::Directory,
        2 => EntryType::Symlink,
        3 => EntryType::Link,
        _ => EntryType::Fifo,
    };
    header.set_entry_type(entry_type);

    // Append data
    let _ = builder.append_data(&mut header, &input.file_name, archive_data);
    if let Ok(mut temp_file) = NamedTempFile::new() {
        let _ = temp_file.write_all(&input.data);
        let _ = builder.append_file("fuzzed/file2", temp_file.as_file_mut()).ok();
    }

    #[cfg(unix)]
    let _ = builder.append_link(&mut header, &input.link_path, &input.target_path).ok();
    let _ = builder.finish();

    // Fuzzing Archive and Entry logic
    let mut archive = Archive::new(Cursor::new(&input.data));
    if let Ok(mut entries) = archive.entries() {
        while let Some(Ok(mut entry)) = entries.next() {
            let _ = entry.path().map(|p| p.to_owned());
            let _ = entry.link_name().map(|l| l.map(|ln| ln.to_owned()));
            let _ = entry.size();
            let _ = entry.header();
            let _ = entry.raw_header_position();
            let _ = entry.raw_file_position();

            // Randomly choose entry actions based on entry type
            match entry.header().entry_type() {
                EntryType::Regular => { /* Do nothing */ }
                EntryType::Directory | EntryType::Symlink | EntryType::Link => {
                    let _ = entry.unpack_in(temp_dir.path()).ok();
                }
                EntryType::Fifo => { /* Do nothing */ }
                _ => { /* Do nothing */ }
            }

            // Randomly read contents and adjust permissions and attributes
            let mut buffer = Vec::new();
            let _ = entry.read_to_end(&mut buffer).ok();
            entry.set_mask(0o755);
            entry.set_unpack_xattrs(true);
            entry.set_preserve_permissions(true);
            entry.set_preserve_mtime(true);

            // Fuzz unpack to randomized destination path
            let dst_path = temp_dir.path().join(&input.file_name);
            let _ = entry.unpack(&dst_path).ok();
            let _ = entry.unpack_in(temp_dir.path()).ok();

            // Fuzz PaxExtensions
            if let Ok(Some(pax_extensions)) = entry.pax_extensions() {
                for ext in pax_extensions {
                    let _ = ext.ok();
                }
            }

            // Randomized file search with tar entry position
            if entry.size() > 0 {
                let mut data_cursor = Cursor::new(&input.data);
                let _ = data_cursor.seek(std::io::SeekFrom::Start(entry.raw_file_position())).ok();
                let _ = data_cursor.read(&mut buffer).ok();
            }
        }
    }
});
