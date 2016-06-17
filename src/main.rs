extern crate xattr;
use std::fs::File;

use std::os::unix::prelude::*;
use std::ffi::*;

fn main() {

    let _ = File::create("foo");
    xattr::set("foo", OsStr::from_bytes(b"user.\x01"), &[1]).unwrap();
}
