# tar-rs

[![Build Status](https://travis-ci.org/alexcrichton/tar-rs.svg?branch=master)](https://travis-ci.org/alexcrichton/tar-rs)
[![Build status](https://ci.appveyor.com/api/projects/status/0udgokm2fc6ljorj?svg=true)](https://ci.appveyor.com/project/alexcrichton/tar-rs)

[Documentation](http://alexcrichton.com/tar-rs/tar/index.html)

A tar archive reading/writing library for Rust.

```toml
# Cargo.toml
[dependencies.tar]
git = "https://github.com/alexcrichton/tar-rs"
```

## Reading an archive

```rust,no_run
extern crate tar;

use std::io::prelude::*;
use std::io::SeekFrom;
use std::fs::File;
use tar::Archive;

fn main() {
    let file = File::open("foo.tar").unwrap();
    let a = Archive::new(file);

    for file in a.files().unwrap() {
        // Make sure there wasn't an I/O error
        let mut file = file.unwrap();

        // Inspect metadata about the file
        println!("{:?}", file.filename());
        println!("{}", file.size());

        // files implement the Read trait
        let mut s = String::new();
        file.read_to_string(&mut s).unwrap();
        println!("{}", s);

        // files also implement the Seek trait
        file.seek(SeekFrom::Current(0)).unwrap();
    }
}

```

## Writing an archive

```rust,no_run
# #![allow(unused_must_use, unstable)]
extern crate tar;

use std::io::prelude::*;
use std::fs::File;
use tar::Archive;

fn main() {
    let file = File::create("foo.tar").unwrap();
    let a = Archive::new(file);

    a.append("file1.txt", &mut File::open("file1.txt").unwrap());
    a.append("file2.txt", &mut File::open("file2.txt").unwrap());
    a.finish();
}
```

# License

`tar-rs` is primarily distributed under the terms of both the MIT license and
the Apache License (Version 2.0), with portions covered by various BSD-like
licenses.

See LICENSE-APACHE, and LICENSE-MIT for details.
