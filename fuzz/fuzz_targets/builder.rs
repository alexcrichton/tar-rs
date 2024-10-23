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

use std::io::Cursor;
use tar::Builder;
use tempfile::{tempdir, tempfile};

fuzz_target!(|data: &[u8]| {
    // Initialization
    let random_bool = data.first().map(|&b| b % 2 == 0).unwrap_or(false);
    let temp_dir = tempdir().expect("");
    
    // Create a temporary file for testing
    if let Ok(temp_file) = tempfile() {
        let mut builder = Builder::new(temp_file);

        // Randomly choose a function target from builder to fuzz
        match data.first().map(|&b| b % 8) {
            Some(0) => {
                builder.mode(if random_bool { tar::HeaderMode::Deterministic } else { tar::HeaderMode::Complete });
            }
            Some(1) => {
                if let Ok(mut file) = tempfile() {
                    let _ = builder.append_file("testfile.txt", &mut file);
                }
            }
            Some(2) => {
                let _ = builder.append_data(&mut tar::Header::new_old(), "randomfile", Cursor::new(data));
            }
            Some(3) => {
                if let Ok(mut file) = tempfile() {
                    let _ = builder.append_data(&mut tar::Header::new_old(), "testwrite.txt", &mut file);
                }
            }
            Some(4) => {
                let link_path = temp_dir.path().join("testlink");
                let _ = builder.append_link(&mut tar::Header::new_old(), "testlink.txt", &link_path);
            }
            Some(5) => {
                let _ = builder.append_path(temp_dir.path());
            }
            Some(6) => {
                let link_path = temp_dir.path().join("testlink_with_path");
                let _ = builder.append_link(&mut tar::Header::new_old(), temp_dir.path(), &link_path);
            }
            Some(7) => {
                let _ = builder.append_dir_all("testdir", temp_dir.path());
            }
            _ => {}
        }
    }
});
