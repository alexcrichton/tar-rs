# tar-rs

[![Build Status](https://travis-ci.org/alexcrichton/tar-rs.svg?branch=master)](https://travis-ci.org/alexcrichton/tar-rs)

[Documentation](http://alexcrichton.com/tar-rs/tar/index.html)

A tar archive reading/writing library for Rust.

```toml
# Cargo.toml
[dependencies.tar]
git = "https://github.com/alexcrichton/tar-rs"
```

## Reading an archive

```rust
extern crate tar;

use tar::Archive;
use std::io::{File, SeekSet};

fn main() {
    let file = File::open(&Path::new("foo.tar")).unwrap();
    let a = Archive::new(file);

    for file in a.files().unwrap() {
        // Make sure there wasn't an I/O error
        let mut file = file.unwrap();

        // Inspect metadata about the file
        println!("{}", file.filename());
        println!("{}", file.size());

        // files implement the Reader trait
        println!("{}", file.read_to_string());

        // files also implement the Seek trait
        file.seek(0, SeekSet);
    }
}

```

## Writing an archive

```rust
extern crate tar;

use tar::Archive;
use std::io::{File, SeekSet};

fn main() {
    let file = File::create(&Path::new("foo.tar")).unwrap();
    let a = Archive::new(file);

    a.append("file1.txt", &mut File::open(&Path::new("file1.txt")).unwrap());
    a.append("file2.txt", &mut File::open(&Path::new("file2.txt")).unwrap());
    a.finish();
}
```
