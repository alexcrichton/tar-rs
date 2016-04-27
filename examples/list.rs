extern crate tar;

use std::io::{stdin};

use tar::Archive;


fn main() {
    let mut arch = Archive::new(stdin());
    for file in arch.entries().unwrap() {
        let f= file.unwrap();
        println!("{}", f.header().path().unwrap().display());
    }
}
