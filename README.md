# tar-rs

[![Build Status](https://travis-ci.org/alexcrichton/tar-rs.svg?branch=master)](https://travis-ci.org/alexcrichton/tar-rs)
[![Build status](https://ci.appveyor.com/api/projects/status/0udgokm2fc6ljorj?svg=true)](https://ci.appveyor.com/project/alexcrichton/tar-rs)
[![Coverage Status](https://coveralls.io/repos/alexcrichton/tar-rs/badge.svg?branch=master&service=github)](https://coveralls.io/github/alexcrichton/tar-rs?branch=master)

[Documentation](http://alexcrichton.com/tar-rs/tar/index.html)

A tar archive reading/writing library for Rust.

```toml
# Cargo.toml
[dependencies]
tar = "0.3"
```

## Reading an archive

```rust,no_run
extern crate tar;

use std::io::prelude::*;
use std::fs::File;
use tar::Archive;

fn main() {
    let file = File::open("foo.tar").unwrap();
    let mut a = Archive::new(file);

    for file in a.entries().unwrap() {
        // Make sure there wasn't an I/O error
        let mut file = file.unwrap();

        // Inspect metadata about the file
        println!("{:?}", file.header().path().unwrap());
        println!("{}", file.header().size().unwrap());

        // files implement the Read trait
        let mut s = String::new();
        file.read_to_string(&mut s).unwrap();
        println!("{}", s);
    }
}

```

## Writing an archive

```rust,no_run
extern crate tar;

use std::io::prelude::*;
use std::fs::File;
use tar::Builder;

fn main() {
    let file = File::create("foo.tar").unwrap();
    let mut a = Builder::new(file);

    a.append_path("file1.txt");
    a.append_file("file2.txt", &mut File::open("file3.txt").unwrap());
}
```

# License

`tar-rs` is primarily distributed under the terms of both the MIT license and
the Apache License (Version 2.0), with portions covered by various BSD-like
licenses.

See LICENSE-APACHE, and LICENSE-MIT for details.
