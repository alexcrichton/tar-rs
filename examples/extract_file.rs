extern crate tar;

use std::io::{stdin, stdout, copy};
use std::env::args_os;
use std::path::Path;

use tar::Archive;


fn main() {
    let first_arg = args_os().skip(1).next().unwrap();
    let filename = Path::new(&first_arg);
    let mut arch = Archive::new(stdin());
    for file in arch.entries().unwrap() {
        let mut f = file.unwrap();
        if f.header().path().unwrap() == filename {
            copy(&mut f, &mut stdout()).unwrap();
        }
    }
}
