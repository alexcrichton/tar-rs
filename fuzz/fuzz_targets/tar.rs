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

use libfuzzer_sys::fuzz_target;

use tar::{Builder, Header, Archive, EntryType};
use std::io::{Cursor, Read, Write, Seek};
use tempfile::{tempdir, NamedTempFile};

fuzz_target!(|data: &[u8]| {
    // Setup temporary directory and path
    let temp_dir = tempdir().unwrap();
    let archive_data = Cursor::new(data);
    let mut builder = Builder::new(Cursor::new(Vec::new()));
    let mut header = Header::new_gnu();

    // Set header metadata
    header.set_size(data.len() as u64);
    header.set_cksum();
    header.set_entry_type(EntryType::file());

    // Append data and a temp file to tar
    let _ = builder.append_data(&mut header, "fuzzed/file", archive_data);
    let mut temp_file = NamedTempFile::new().unwrap();
    let _ = temp_file.write_all(data);
    let _ = builder.append_file("fuzzed/file2", temp_file.as_file_mut()).ok();

    #[cfg(unix)]
    let _ = builder.append_link(&mut header, "symlink/path", "target/path").ok();

    let _ = builder.finish();

    // Fuzzing Archive and Entry logic
    let mut archive = Archive::new(Cursor::new(data));
    if let Ok(mut entries) = archive.entries() {
        while let Some(Ok(mut entry)) = entries.next() {
            let _ = entry.path().map(|p| p.to_owned());
            let _ = entry.link_name().map(|l| l.map(|ln| ln.to_owned()));
            let _ = entry.size();
            let _ = entry.header();
            let _ = entry.raw_header_position();
            let _ = entry.raw_file_position();

            match entry.header().entry_type() {
                EntryType::Regular => { /* Do nothing */ }
                EntryType::Directory => {
                    let _ = entry.unpack_in(temp_dir.path()).ok();
                }
                EntryType::Symlink => {
                    let _ = entry.unpack_in(temp_dir.path()).ok();
                }
                EntryType::Link => {
                    let _ = entry.unpack_in(temp_dir.path()).ok();
                }
                EntryType::Fifo => { /* Do nothing */ }
                _ => { /* Do nothing */ }
            }

            let mut buffer = Vec::new();
            let _ = entry.read_to_end(&mut buffer).ok();
            entry.set_mask(0o755);
            entry.set_unpack_xattrs(true);
            entry.set_preserve_permissions(true);
            entry.set_preserve_mtime(true);

            // Fuzz unpack
            let dst_path = temp_dir.path().join("unpacked_file");
            let _ = entry.unpack(&dst_path).ok();
            let _ = entry.unpack_in(temp_dir.path()).ok();

            // Fuzz PaxExtensions
            if let Ok(Some(pax_extensions)) = entry.pax_extensions() {
                for ext in pax_extensions {
                    let _ = ext.ok();
                }
            }

            // Fuzzing file search with tar entry position
            if entry.size() > 0 {
                let mut data_cursor = Cursor::new(data);
                let _ = data_cursor.seek(std::io::SeekFrom::Start(entry.raw_file_position())).ok();
                let _ = data_cursor.read(&mut buffer).ok();
            }
        }
    }
});
