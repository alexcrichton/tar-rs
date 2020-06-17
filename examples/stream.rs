use std::fs::{read_dir, File};
use std::io::Write;
use std::iter::Peekable;
use std::path::PathBuf;

struct TarStream<I: Iterator<Item = PathBuf>> {
    files: Peekable<I>,
}

impl<I: Iterator<Item = PathBuf>> TarStream<I> {
    pub fn new(files: I) -> Self {
        Self {
            files: files.peekable(),
        }
    }

    pub fn read(&mut self, buf: &mut Vec<u8>) -> Option<()> {
        self.files.next().map(|path| {
            let new_path = PathBuf::from("archived").join(&path);
            let mut builder = tar::Builder::new(buf);

            builder.contiguous(true);
            builder.append_path_with_name(path, &new_path).unwrap();

            if self.files.peek().is_none() {
                builder.finish().unwrap();
            }
        })
    }
}

fn main() {
    let files = read_dir("examples")
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| p.is_file());
    let mut buf = Vec::with_capacity(1024 * 1024 * 4);
    let mut tar_stream = TarStream::new(files);
    let mut output = File::create("examples.tar").unwrap();

    while tar_stream.read(&mut buf).is_some() {
        output.write(&buf).unwrap();
    }
}
