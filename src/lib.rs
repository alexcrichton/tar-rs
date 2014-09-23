//! A library for reading and writing TAR archives
//!
//! This library provides utilities necessary to manage TAR archives [1]
//! abstracted over a reader or writer. Great strides are taken to ensure that
//! an archive is never required to be fully resident in memory, all objects
//! provide largely a streaming interface to read bytes from.
//!
//! [1]: http://en.wikipedia.org/wiki/Tar_%28computing%29

#![feature(macro_rules)]
#![deny(missing_doc)]

use std::cell::{RefCell, Cell};
use std::cmp;
use std::io::{IoResult, IoError};
use std::io;
use std::iter::{AdditiveIterator, Repeat};
use std::fmt;
use std::mem;
use std::num;
use std::slice::bytes;
use std::str;

/// A top-level representation of an archive file.
///
/// This archive can have a file added to it and it can be iterated over.
pub struct Archive<R> {
    obj: RefCell<R>,
    pos: Cell<u64>,
}

/// An iterator over the files of an archive.
pub struct Files<'a, R:'a> {
    archive: &'a Archive<R>,
    done: bool,
    offset: u64,
}

/// A read-only view into a file of an archive.
///
/// This structure is a windows into a portion of a borrowed archive which can
/// be inspected. It acts as a file handle by implementing the Reader and Seek
/// traits. A file cannot be rewritten once inserted into an archive.
pub struct File<'a, R:'a> {
    header: Header,
    archive: &'a Archive<R>,
    tar_offset: u64,
    pos: u64,
    size: u64,
    filename: Vec<u8>,
}

/// Representation of the header of a file in an archive
#[repr(C)]
#[allow(missing_doc)]
pub struct Header {
    pub name: [u8, ..100],
    pub mode: [u8, ..8],
    pub owner_id: [u8, ..8],
    pub group_id: [u8, ..8],
    pub size: [u8, ..12],
    pub mtime: [u8, ..12],
    pub cksum: [u8, ..8],
    pub link: [u8, ..1],
    pub linkname: [u8, ..100],

    // UStar format
    pub ustar: [u8, ..6],
    pub ustar_version: [u8, ..2],
    pub owner_name: [u8, ..32],
    pub group_name: [u8, ..32],
    pub dev_major: [u8, ..8],
    pub dev_minor: [u8, ..8],
    pub prefix: [u8, ..155],
    _rest: [u8, ..12],
}

impl<O> Archive<O> {
    /// Create a new archive with the underlying object as the reader/writer.
    ///
    /// Different methods are available on an archive depending on the traits
    /// that the underlying object implements.
    pub fn new(obj: O) -> Archive<O> {
        Archive { obj: RefCell::new(obj), pos: Cell::new(0) }
    }

    /// Unwrap this archive, returning the underlying object.
    pub fn unwrap(self) -> O {
        self.obj.unwrap()
    }
}

impl<R: Seek + Reader> Archive<R> {
    /// Construct an iterator over the files of this archive.
    ///
    /// This function can return an error if any underlying I/O operation files
    /// while attempting to construct the iterator.
    ///
    /// Additionally, the iterator yields `IoResult<File>` instead of `File` to
    /// handle invalid tar archives as well as any intermittent I/O error that
    /// occurs.
    pub fn files<'a>(&'a self) -> IoResult<Files<'a, R>> {
        try!(self.seek(0));
        Ok(Files { archive: self, done: false, offset: 0 })
    }

    fn seek(&self, pos: u64) -> IoResult<()> {
        if self.pos.get() == pos { return Ok(()) }
        try!(self.obj.borrow_mut().seek(pos as i64, io::SeekSet));
        self.pos.set(pos);
        Ok(())
    }
}

impl<W: Writer> Archive<W> {
    /// Add the file at the specified path to this archive.
    ///
    /// This function will insert the file into the archive with the appropriate
    /// metadata set, returning any I/O error which occurs while writing.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    pub fn append(&self, path: &str, file: &mut io::File) -> IoResult<()> {
        let stat = try!(file.stat());

        // Prepare the header, flagging it as a UStar archive
        let mut header: Header = unsafe { mem::zeroed() };
        header.ustar = [b'u', b's', b't', b'a', b'r', 0];
        header.ustar_version = [b'0', b'0'];

        // Prepare the filename
        let cstr = path.to_c_str();
        let path = cstr.as_bytes();
        let (namelen, prefixlen) = (header.name.len(), header.prefix.len());
        if path.len() < namelen {
            bytes::copy_memory(header.name, path);
        } else if path.len() < namelen + prefixlen {
            let prefix = path.slice_to(cmp::min(path.len(), prefixlen));
            let pos = match prefix.iter().rposition(|&b| b == b'/' || b == b'\\') {
                Some(i) => i,
                None => return Err(IoError {
                    kind: io::OtherIoError,
                    desc: "path cannot be split to be inserted into archive",
                    detail: None,
                })
            };
            bytes::copy_memory(header.name, path.slice_from(pos + 1));
            bytes::copy_memory(header.prefix, path.slice_to(pos));
        } else {
            return Err(IoError {
                kind: io::OtherIoError,
                desc: "path is too long to insert into archive",
                detail: None,
            })
        }

        // Prepare the metadata fields.
        octal(header.mode, stat.perm.bits()); // TODO: is this right?
        octal(header.mtime, stat.modified / 1000);
        octal(header.owner_id, stat.unstable.uid);
        octal(header.group_id, stat.unstable.gid);
        octal(header.size, stat.size);
        octal(header.dev_minor, 0i);
        octal(header.dev_major, 0i);

        header.link[0] = match stat.kind {
            io::TypeFile => b'0',
            io::TypeDirectory => b'5',
            io::TypeNamedPipe => b'6',
            io::TypeBlockSpecial => b'4',
            io::TypeSymlink => b'2',
            io::TypeUnknown => b' ',
        };

        // Final step, calculate the checksum
        let cksum = {
            let bytes = header.as_bytes();
            bytes.slice_to(148).iter().map(|i| *i as uint).sum() +
                bytes.slice_from(156).iter().map(|i| *i as uint).sum() +
                32 * header.cksum.len()
        };
        octal(header.cksum, cksum);

        // Write out the header, the entire file, then pad with zeroes.
        let mut obj = self.obj.borrow_mut();
        try!(obj.write(header.as_bytes().as_slice()));
        try!(io::util::copy(file, &mut *obj));
        let buf = [0, ..512];
        let remaining = 512 - (stat.size % 512);
        if remaining < 512 {
            try!(obj.write(buf.slice_to(remaining as uint)));
        }

        // And we're done!
        return Ok(());

        fn octal<T: fmt::Octal>(dst: &mut [u8], val: T) {
            let o = format!("{:o}", val);
            let value = o.as_slice().bytes().rev().chain(Repeat::new(b'0'));
            for (slot, value) in dst.iter_mut().rev().skip(1).zip(value) {
                *slot = value;
            }
        }
    }

    /// Finish writing this archive, emitting the termination sections.
    ///
    /// This function is required to be called to complete the archive, it will
    /// be invalid if this is not called.
    pub fn finish(&self) -> IoResult<()> {
        let b = [0, ..1024];
        self.obj.borrow_mut().write(b)
    }
}

impl<'a, R: Seek + Reader> Iterator<IoResult<File<'a, R>>> for Files<'a, R> {
    fn next(&mut self) -> Option<IoResult<File<'a, R>>> {
        macro_rules! try( ($e:expr) => (
            match $e {
                Ok(e) => e,
                Err(e) => { self.done = true; return Some(Err(e)) }
            }
        ) )
        macro_rules! bail( () => ({
            self.done = true;
            return Some(Err(bad_archive()))
        }) )

        // If we hit a previous error, or we reached the end, we're done here
        if self.done { return None }

        // Make sure that we've seeked to the start of the next file in this
        // iterator, and then parse the chunk. If we have 2 or more sections of
        // all 0s, then the archive is done.
        try!(self.archive.seek(self.offset));
        let mut chunk = [0, ..512];
        let mut cnt = 0i;
        loop {
            if try!(self.archive.read(chunk)) != 512 {
                bail!()
            }
            self.offset += 512;
            if chunk.iter().any(|i| *i != 0) { break }
            cnt += 1;
            if cnt > 1 {
                self.done = true;
                return None
            }
        }

        let sum = chunk.slice_to(148).iter().map(|i| *i as uint).sum() +
                  chunk.slice_from(156).iter().map(|i| *i as uint).sum() +
                  32 * 8;

        let mut ret = File {
            archive: self.archive,
            header: unsafe { mem::transmute(chunk) },
            pos: 0,
            size: 0,
            tar_offset: self.offset,
            filename: Vec::new(),
        };

        // Make sure the checksum is ok
        let cksum = try!(ret.header.cksum());
        if sum != cksum { bail!() }

        // Figure out where the next file is
        let size = try!(ret.header.size());
        ret.size = size;
        let size = (size + 511) & !(512 - 1);
        self.offset += size;

        if ret.header.is_ustar() && ret.header.prefix[0] != 0 {
            ret.filename.push_all(truncate(ret.header.prefix));
            ret.filename.push(b'/');
        }
        ret.filename.push_all(truncate(ret.header.name));

        Some(Ok(ret))
    }
}

impl Header {
    fn size(&self) -> IoResult<u64> { octal(self.size) }
    fn cksum(&self) -> IoResult<uint> { octal(self.cksum) }
    fn is_ustar(&self) -> bool {
        self.ustar.slice_to(5) == b"ustar"
    }
    fn as_bytes<'a>(&'a self) -> &'a [u8, ..512] {
        unsafe { &*(self as *const _ as *const [u8, ..512]) }
    }
}

impl<'a, R: Seek + Reader> File<'a, R> {
    /// Returns the filename of this archive as a byte array
    pub fn filename_bytes<'a>(&'a self) -> &'a [u8] {
        self.filename.as_slice()
    }

    /// Returns the filename of this archive as a utf8 string.
    ///
    /// If `None` is returned, then the filename is not valid utf8
    pub fn filename<'a>(&'a self) -> Option<&'a str> {
        str::from_utf8(self.filename_bytes())
    }

    /// Returns the value of the owner's user ID field
    pub fn uid(&self) -> IoResult<uint> { octal(self.header.owner_id) }
    /// Returns the value of the group's user ID field
    pub fn gid(&self) -> IoResult<uint> { octal(self.header.group_id) }
    /// Returns the last modification time in Unix time format
    pub fn mtime(&self) -> IoResult<uint> { octal(self.header.mtime) }

    /// Classify the type of file that this entry represents
    pub fn classify(&self) -> io::FileType {
        match (self.header.is_ustar(), self.header.link[0]) {
            (_, b'0') => io::TypeFile,
            (_, b'1') => io::TypeUnknown, // need a hard link enum?
            (_, b'2') => io::TypeSymlink,
            (false, _) => io::TypeUnknown, // not technically valid...

            (_, b'3') => io::TypeUnknown, // character special...
            (_, b'4') => io::TypeBlockSpecial,
            (_, b'5') => io::TypeDirectory,
            (_, b'6') => io::TypeNamedPipe,
            (_, _) => io::TypeUnknown, // not technically valid...
        }
    }

    /// Returns the username of the owner of this file, if present
    pub fn username_bytes<'a>(&'a self) -> Option<&'a [u8]> {
        if self.header.is_ustar() {
            Some(truncate(self.header.owner_name))
        } else {
            None
        }
    }
    /// Returns the group name of the owner of this file, if present
    pub fn groupname_bytes<'a>(&'a self) -> Option<&'a [u8]> {
        if self.header.is_ustar() {
            Some(truncate(self.header.group_name))
        } else {
            None
        }
    }
    /// Return the username of the owner of this file, if present and if valid
    /// utf8
    pub fn username<'a>(&'a self) -> Option<&'a str> {
        self.username_bytes().and_then(str::from_utf8)
    }
    /// Return the group name of the owner of this file, if present and if valid
    /// utf8
    pub fn groupname<'a>(&'a self) -> Option<&'a str> {
        self.groupname_bytes().and_then(str::from_utf8)
    }

    /// Returns the device major number, if present.
    ///
    /// This field is only present in UStar archives. A value of `None` means
    /// that this archive is not a UStar archive, while a value of `Some`
    /// represents the attempt to decode the field in the header.
    pub fn device_major(&self) -> Option<IoResult<uint>> {
        if self.header.is_ustar() {
            Some(octal(self.header.dev_major))
        } else {
            None
        }
    }
    /// Returns the device minor number, if present.
    ///
    /// This field is only present in UStar archives. A value of `None` means
    /// that this archive is not a UStar archive, while a value of `Some`
    /// represents the attempt to decode the field in the header.
    pub fn device_minor(&self) -> Option<IoResult<uint>> {
        if self.header.is_ustar() {
            Some(octal(self.header.dev_minor))
        } else {
            None
        }
    }

    /// Returns raw access to the header of this file in the archive.
    pub fn raw_header<'a>(&'a self) -> &'a Header { &self.header }

    /// Returns the size of the file in the archive.
    pub fn size(&self) -> u64 { self.size }
}

impl<'a, R: Reader> Reader for &'a Archive<R> {
    fn read(&mut self, into: &mut [u8]) -> IoResult<uint> {
        self.obj.borrow_mut().read(into).map(|i| {
            self.pos.set(self.pos.get() + i as u64);
            i
        })
    }
}

impl<'a, R: Reader + Seek> Reader for File<'a, R> {
    fn read(&mut self, into: &mut [u8]) -> IoResult<uint> {
        if self.size == self.pos {
            return Err(io::standard_error(io::EndOfFile))
        }

        try!(self.archive.seek(self.tar_offset + self.pos));

        let amt = cmp::min((self.size - self.pos) as uint, into.len());
        let amt = try!(self.archive.read(into.mut_slice_to(amt)));
        self.pos += amt as u64;
        Ok(amt)
    }
}

impl<'a, R> Seek for File<'a, R> {
    fn tell(&self) -> IoResult<u64> { Ok(self.pos) }
    fn seek(&mut self, pos: i64, style: io::SeekStyle) -> IoResult<()> {
        let next = match style {
            io::SeekSet => pos as i64,
            io::SeekCur => self.pos as i64 + pos,
            io::SeekEnd => self.size as i64 + pos,
        };
        if next < 0 {
            Err(io::standard_error(io::OtherIoError))
        } else if next as u64 > self.size {
            Err(io::standard_error(io::OtherIoError))
        } else {
            self.pos = next as u64;
            Ok(())
        }
    }
}

fn bad_archive() -> IoError {
    IoError {
        kind: io::OtherIoError,
        desc: "invalid tar archive",
        detail: None,
    }
}

fn octal<T: num::FromStrRadix>(slice: &[u8]) -> IoResult<T> {
    let num = match str::from_utf8(truncate(slice)) {
        Some(n) => n,
        None => return Err(bad_archive()),
    };
    match num::from_str_radix(num, 8) {
        Some(n) => Ok(n),
        None => Err(bad_archive())
    }
}

fn truncate<'a>(slice: &'a [u8]) -> &'a [u8] {
    match slice.iter().position(|i| *i == 0) {
        Some(i) => slice.slice_to(i),
        None => slice,
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::io::{BufReader, MemWriter, MemReader, File, TempDir};
    use super::Archive;

    #[test]
    fn simple() {
        let rdr = BufReader::new(include_bin!("tests/simple.tar"));
        let ar = Archive::new(rdr);
        for file in ar.files().unwrap() {
            file.unwrap();
        }
    }

    #[test]
    fn reading_files() {
        let rdr = BufReader::new(include_bin!("tests/reading_files.tar"));
        let ar = Archive::new(rdr);
        let mut files = ar.files().unwrap();
        let mut a = files.next().unwrap().unwrap();
        let mut b = files.next().unwrap().unwrap();
        assert!(files.next().is_none());

        assert_eq!(a.filename(), Some("a"));
        assert_eq!(b.filename(), Some("b"));
        assert_eq!(a.read_to_string().unwrap().as_slice(),
                   "a\na\na\na\na\na\na\na\na\na\na\n");
        assert_eq!(b.read_to_string().unwrap().as_slice(),
                   "b\nb\nb\nb\nb\nb\nb\nb\nb\nb\nb\n");
        a.seek(0, io::SeekSet).unwrap();
        assert_eq!(a.read_to_string().unwrap().as_slice(),
                   "a\na\na\na\na\na\na\na\na\na\na\n");
    }

    #[test]
    fn writing_files() {
        let wr = MemWriter::new();
        let ar = Archive::new(wr);
        let td = TempDir::new("tar-rs").unwrap();

        let path = td.path().join("test");
        File::create(&path).write(b"test").unwrap();

        ar.append("test2", &mut File::open(&path).unwrap()).unwrap();
        ar.finish().unwrap();

        let rd = MemReader::new(ar.unwrap().unwrap());
        let ar = Archive::new(rd);
        let mut files = ar.files().unwrap();
        let mut f = files.next().unwrap().unwrap();
        assert!(files.next().is_none());

        assert_eq!(f.filename(), Some("test2"));
        assert_eq!(f.size(), 4);
        assert_eq!(f.read_to_string().unwrap().as_slice(), "test");
    }

    #[test]
    fn large_filename() {
        let ar = Archive::new(MemWriter::new());
        let td = TempDir::new("tar-rs").unwrap();

        let path = td.path().join("test");
        File::create(&path).write(b"test").unwrap();

        let filename = "abcd/".repeat(50);
        ar.append(filename.as_slice(), &mut File::open(&path).unwrap()).unwrap();
        ar.finish().unwrap();

        let too_long = "abcd".repeat(200);
        ar.append(too_long.as_slice(), &mut File::open(&path).unwrap())
          .err().unwrap();

        let rd = MemReader::new(ar.unwrap().unwrap());
        let ar = Archive::new(rd);
        let mut files = ar.files().unwrap();
        let mut f = files.next().unwrap().unwrap();
        assert!(files.next().is_none());

        assert_eq!(f.filename(), Some(filename.as_slice()));
        assert_eq!(f.size(), 4);
        assert_eq!(f.read_to_string().unwrap().as_slice(), "test");
    }
}
