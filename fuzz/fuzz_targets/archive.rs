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

use std::fs::{File, OpenOptions};
use std::io::{Cursor, Write, Read};
use tar::{Archive, Builder, EntryType, Header};
use tempfile::tempdir;
use std::convert::TryInto;
use std::str;

fuzz_target!(|data: &[u8]| {
    // Skip this iteration when data is not enough
    if data.len() < 10 {
        return;
    }

    // Create temp file and dir
    let temp_dir = tempdir().unwrap();
    let file_name = match str::from_utf8(&data[0..data.len().min(10)]) {
        Ok(name) => name.to_string(),
        Err(_) => "default_file_name".to_string(),
    };
    let dir_name = match str::from_utf8(&data[data.len().min(10)..data.len().min(20)]) {
        Ok(name) => name.to_string(),
        Err(_) => "default_dir_name".to_string(),
    };
    let temp_file_path = temp_dir.path().join(format!("{}_file.tar", file_name));

    // Initialise builder and cursor
    let mut builder = Builder::new(Vec::new());
    let mut cursor = Cursor::new(data.to_vec());

    // Choose an etnry type
    let entry_type_byte = data[0];
    let entry_type = match entry_type_byte % 5 {
        0 => EntryType::Regular,
        1 => EntryType::Directory,
        2 => EntryType::Symlink,
        3 => EntryType::hard_link(),
        _ => EntryType::character_special(),
    };

    // Initilaise header
    let mut header = Header::new_gnu();
    let file_size = u64::from_le_bytes(
        data.get(1..9)
            .unwrap_or(&[0; 8])
            .try_into()
            .unwrap_or([0; 8]),
    );
    header.set_size(file_size);
    header.set_entry_type(entry_type);
    header.set_cksum();

    // Prepare sample tar file
    let tar_file_path = format!("{}/{}", dir_name, file_name);
    let _ = builder.append_data(&mut header, tar_file_path.clone(), &mut cursor).ok();
    cursor.set_position(0);
    for i in 1..5 {
        let start = i * 10 % data.len();
        let end = std::cmp::min(start + 10, data.len());
        let entry_data = &data[start..end];
        let entry_name = match str::from_utf8(&entry_data) {
            Ok(name) => name.to_string(),
            Err(_) => format!("entry_{}", i),
        };

        let mut entry_header = Header::new_gnu();
        entry_header.set_size(entry_data.len() as u64);
        entry_header.set_entry_type(entry_type);
        entry_header.set_cksum();

        let mut entry_cursor = Cursor::new(entry_data.to_vec());
        let _ = builder.append_data(&mut entry_header, entry_name, &mut entry_cursor).ok();
    }

    // Prepare malformed tar header
    if data.len() > 512 {
        let corrupt_header_data = &data[data.len() - 512..];
        let corrupt_header = Header::from_byte_slice(corrupt_header_data);
        let mut corrupt_cursor = Cursor::new(data.to_vec());
        let corrupt_entry_name = "corrupt_entry.txt";
        let _ = builder.append_data(&mut corrupt_header.clone(), corrupt_entry_name, &mut corrupt_cursor).ok();
    }

    if let Ok(mut tar_file) = File::create(&temp_file_path) {
        if let Ok(tar_data) = builder.into_inner() {
            let _ = tar_file.write_all(&tar_data);
        }
    }

    // Fuzz archive and builder unpack with malformed tar archvie
    if let Ok(mut tar_file) = OpenOptions::new().read(true).open(&temp_file_path) {
        let mut tar_data = Vec::new();
        let _ = tar_file.read_to_end(&mut tar_data);
        let mut tar_cursor = Cursor::new(tar_data);
        let mut archive = Archive::new(&mut tar_cursor);
        let _ = archive.unpack(temp_dir.path()).ok();
    }

    // Fuzz archive and builder
    for i in 0..3 {
        let name_data = &data[i * 5 % data.len()..(i * 5 + 5) % data.len()];
        let name = match str::from_utf8(name_data) {
            Ok(n) => n.to_string(),
            Err(_) => format!("random_name_{}", i),
        };
        let path = temp_dir.path().join(name);
        if i % 2 == 0 {
            // Create a file
            if let Ok(mut file) = File::create(&path) {
                let _ = file.write_all(data);
            }
        } else {
            // Create a directory
            let _ = std::fs::create_dir(&path);
        }
    }

    // Fuzz unpacking
    let mut data_cursor = Cursor::new(data.to_vec());
    let mut data_archive = Archive::new(&mut data_cursor);
    let _ = data_archive.unpack(temp_dir.path()).ok();
});
