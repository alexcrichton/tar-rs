//! A library for reading and writing TAR archives
//!
//! This library provides utilities necessary to manage TAR archives [1]
//! abstracted over a reader or writer. Great strides are taken to ensure that
//! an archive is never required to be fully resident in memory, all objects
//! provide largely a streaming interface to read bytes from.
//!
//! [1]: http://en.wikipedia.org/wiki/Tar_%28computing%29

#![feature(core, collections, std_misc, io, path, fs, os)]
#![deny(missing_docs)]
#![cfg_attr(test, deny(warnings))]
#![cfg_attr(test, feature(env))]

use std::cell::{RefCell, Cell};
use std::cmp;
use std::ffi::CString;
use std::fmt;
use std::fs;
use std::io::prelude::*;
use std::io::{self, Error, ErrorKind, Seek, SeekFrom};
use std::iter::{AdditiveIterator, repeat};
use std::mem;
use std::num;
use std::path::{PathBuf, Path};
use std::slice::bytes;
use std::str;

macro_rules! try_iter{ ($me:expr, $e:expr) => (
    match $e {
        Ok(e) => e,
        Err(e) => { $me.done = true; return Some(Err(e)) }
    }
) }

/// A top-level representation of an archive file.
///
/// This archive can have a file added to it and it can be iterated over.
pub struct Archive<R> {
    obj: RefCell<R>,
    pos: Cell<u64>,
}

/// An iterator over the files of an archive.
///
/// Requires that `R` implement `Seek`.
pub struct Files<'a, R:'a> {
    archive: &'a Archive<R>,
    done: bool,
    offset: u64,
}

/// An iterator over the files of an archive.
///
/// Does not require that `R` implements `Seek`, but each file must be processed
/// before the next.
pub struct FilesMut<'a, R:'a> {
    archive: &'a Archive<R>,
    next: u64,
    done: bool,
}

/// A read-only view into a file of an archive.
///
/// This structure is a windows into a portion of a borrowed archive which can
/// be inspected. It acts as a file handle by implementing the Reader and Seek
/// traits. A file cannot be rewritten once inserted into an archive.
pub struct File<'a, R: 'a> {
    header: Header,
    archive: &'a Archive<R>,
    pos: u64,
    size: u64,
    filename: Vec<u8>,

    // Used in read() to make sure we're positioned at the next byte. For a
    // `Files` iterator these are meaningful while for a `FilesMut` iterator
    // these are both unused/noops.
    seek: fn(&File<R>) -> io::Result<()>,
    tar_offset: u64,
}

/// Representation of the header of a file in an archive
#[repr(C)]
#[allow(missing_docs)]
pub struct Header {
    pub name: [u8; 100],
    pub mode: [u8; 8],
    pub owner_id: [u8; 8],
    pub group_id: [u8; 8],
    pub size: [u8; 12],
    pub mtime: [u8; 12],
    pub cksum: [u8; 8],
    pub link: [u8; 1],
    pub linkname: [u8; 100],

    // UStar format
    pub ustar: [u8; 6],
    pub ustar_version: [u8; 2],
    pub owner_name: [u8; 32],
    pub group_name: [u8; 32],
    pub dev_major: [u8; 8],
    pub dev_minor: [u8; 8],
    pub prefix: [u8; 155],
    _rest: [u8; 12],
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
    pub fn into_inner(self) -> O {
        self.obj.into_inner()
    }
}

impl<R: Seek + Read> Archive<R> {
    /// Construct an iterator over the files of this archive.
    ///
    /// This function can return an error if any underlying I/O operation files
    /// while attempting to construct the iterator.
    ///
    /// Additionally, the iterator yields `io::Result<File>` instead of `File` to
    /// handle invalid tar archives as well as any intermittent I/O error that
    /// occurs.
    pub fn files(&self) -> io::Result<Files<R>> {
        try!(self.seek(0));
        Ok(Files { archive: self, done: false, offset: 0 })
    }

    fn seek(&self, pos: u64) -> io::Result<()> {
        if self.pos.get() == pos { return Ok(()) }
        try!(self.obj.borrow_mut().seek(SeekFrom::Start(pos)));
        self.pos.set(pos);
        Ok(())
    }
}

impl<R: Read> Archive<R> {
    /// Construct an iterator over the files in this archive.
    ///
    /// While similar to the `files` iterator, this iterator does not require
    /// that `R` implement `Seek` and restricts the iterator to processing only
    /// one file at a time in a streaming fashion.
    ///
    /// Note that care must be taken to consider each file within an archive in
    /// sequence. If files are processed out of sequence (from what the iterator
    /// returns), then the contents read for each file may be corrupted.
    pub fn files_mut(&mut self) -> io::Result<FilesMut<R>> {
        Ok(FilesMut { archive: self, done: false, next: 0 })
    }

    /// Unpacks this tarball into the specified path
    pub fn unpack(&mut self, into: &Path) -> io::Result<()> {
        for file in try!(self.files_mut()) {
            let mut file = try!(file);
            let bytes = file.filename_bytes().iter().map(|&b| {
                if b == b'\\' {b'/'} else {b}
            }).collect::<Vec<_>>();
            let is_directory = bytes[bytes.len() - 1] == b'/';
            let mut dst = into.to_path_buf();
            for part in <[u8] as SliceExt>::split(&bytes, |x| *x == b'/') {
                push(&mut dst, part);
            }
            if is_directory {
                // TODO: this should not require `.parent().unwrap()`
                try!(fs::create_dir_all(dst.parent().unwrap()));
            } else {
                try!(fs::create_dir_all(dst.parent().unwrap()));
                {
                    let mut dst = try!(fs::File::create(&dst));
                    try!(io::copy(&mut file, &mut dst));
                }
                let mut perm = try!(dst.metadata()).permissions();
                set_perms(&mut perm, try!(file.mode()));
                try!(fs::set_permissions(&dst, perm));
            }
        }
        return Ok(());

        #[cfg(unix)]
        fn set_perms(perm: &mut fs::Permissions, mode: i32) {
            use std::os::unix::*;
            perm.set_mode(mode);
        }
        #[cfg(windows)]
        fn set_perms(perm: &mut fs::Permissions, mode: i32) {
            perm.set_readonly(mode & 0o200 != 0o200);
        }
        #[cfg(unix)]
        fn push(path: &mut PathBuf, bytes: &[u8]) {
            use std::os::unix::*;
            use std::ffi::OsStr;
            path.push(<OsStr as OsStrExt>::from_bytes(bytes));
        }
        #[cfg(windows)]
        fn push(path: &mut PathBuf, bytes: &[u8]) {
            use std::str;
            let s = str::from_utf8(bytes).expect("only unicode paths are \
                                                  supported on windows");
            path.push(s);
        }
    }

    fn skip(&self, mut amt: u64) -> io::Result<()> {
        let mut buf = [0u8; 4096 * 8];
        let mut me = self;
        while amt > 0 {
            let n = cmp::min(amt, buf.len() as u64);
            try!(Read::read(&mut me, &mut buf[..n as usize]));
            amt -= n;
        }
        Ok(())
    }

    // Assumes that the underlying reader is positioned at the start of a valid
    // header to parse.
    fn next_file(&self, offset: &mut u64, seek: fn(&File<R>) -> io::Result<()>)
                 -> io::Result<Option<File<R>>> {
        // If we have 2 or more sections of 0s, then we're done!
        let mut chunk = [0; 512];
        let mut cnt = 0;
        let mut me = self;
        loop {
            if try!(Read::read(&mut me, &mut chunk)) != 512 {
                return Err(bad_archive())
            }
            *offset += 512;
            if chunk.iter().any(|i| *i != 0) { break }
            cnt += 1;
            if cnt > 1 { return Ok(None) }
        }

        let sum = chunk[..148].iter().map(|i| *i as u32).sum() +
                  chunk[156..].iter().map(|i| *i as u32).sum() +
                  32 * 8;

        let mut ret = File {
            archive: self,
            header: unsafe { mem::transmute(chunk) },
            pos: 0,
            size: 0,
            tar_offset: *offset,
            filename: Vec::new(),
            seek: seek,
        };

        // Make sure the checksum is ok
        let cksum = try!(ret.header.cksum());
        if sum != cksum { return Err(bad_archive()) }

        // Figure out where the next file is
        let size = try!(ret.header.size());
        ret.size = size;
        let size = (size + 511) & !(512 - 1);
        *offset += size;

        if ret.header.is_ustar() && ret.header.prefix[0] != 0 {
            ret.filename.push_all(truncate(&ret.header.prefix));
            ret.filename.push(b'/');
        }
        ret.filename.push_all(truncate(&ret.header.name));

        return Ok(Some(ret));
    }
}

impl<W: Write> Archive<W> {
    /// Add the file at the specified path to this archive.
    ///
    /// This function will insert the file into the archive with the appropriate
    /// metadata set, returning any I/O error which occurs while writing.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    pub fn append(&self, path: &str, file: &mut fs::File) -> io::Result<()> {
        let stat = try!(file.metadata());

        // Prepare the header, flagging it as a UStar archive
        let mut header: Header = unsafe { mem::zeroed() };
        header.ustar = [b'u', b's', b't', b'a', b'r', 0];
        header.ustar_version = [b'0', b'0'];

        // Prepare the filename
        let cstr = try!(CString::new(path.replace(r"\", "/")));
        let path = cstr.as_bytes();
        let (namelen, prefixlen) = (header.name.len(), header.prefix.len());
        if path.len() < namelen {
            bytes::copy_memory(&mut header.name, path);
        } else if path.len() < namelen + prefixlen {
            let prefix = &path[..cmp::min(path.len(), prefixlen)];
            let pos = match prefix.iter().rposition(|&b| b == b'/' || b == b'\\') {
                Some(i) => i,
                None => return Err(Error::new(ErrorKind::Other,
                                              "path cannot be split to be \
                                               inserted into archive", None)),
            };
            bytes::copy_memory(&mut header.name, &path[pos + 1..]);
            bytes::copy_memory(&mut header.prefix, &path[..pos]);
        } else {
            return Err(Error::new(ErrorKind::Other,
                                  "path is too long to insert into archive",
                                  None))
        }

        // Prepare the metadata fields.
        octal(&mut header.mode, perm_bits(&stat)); // TODO: is this right?
        octal(&mut header.mtime, stat.modified() / 1000);
        // octal(&mut header.owner_id, stat.unstable.uid);
        // octal(&mut header.group_id, stat.unstable.gid);
        octal(&mut header.owner_id, 0); // TODO: fill this in
        octal(&mut header.group_id, 0); // TODO: fill this in
        octal(&mut header.size, stat.len());
        octal(&mut header.dev_minor, 0);
        octal(&mut header.dev_major, 0);

        // TODO: need to bind more file types
        header.link[0] = if stat.is_dir() {b'5'} else {b'0'};
        // header.link[0] = match stat.kind {
        //     old_io::FileType::RegularFile => b'0',
        //     old_io::FileType::Directory => b'5',
        //     old_io::FileType::NamedPipe => b'6',
        //     old_io::FileType::BlockSpecial => b'4',
        //     old_io::FileType::Symlink => b'2',
        //     old_io::FileType::Unknown => b' ',
        // };

        // Final step, calculate the checksum
        let cksum = {
            let bytes = header.as_bytes();
            bytes[..148].iter().map(|i| *i as u32).sum() +
                bytes[156..].iter().map(|i| *i as u32).sum() +
                32 * (header.cksum.len() as u32)
        };
        octal(&mut header.cksum, cksum);

        // Write out the header, the entire file, then pad with zeroes.
        let mut obj = self.obj.borrow_mut();
        try!(obj.write_all(header.as_bytes().as_slice()));
        try!(io::copy(file, &mut *obj));
        let buf = [0; 512];
        let remaining = 512 - (stat.len() % 512);
        if remaining < 512 {
            try!(obj.write_all(&buf[..remaining as usize]));
        }

        // And we're done!
        return Ok(());

        fn octal<T: fmt::Octal>(dst: &mut [u8], val: T) {
            let o = format!("{:o}", val);
            let value = o.as_slice().bytes().rev().chain(repeat(b'0'));
            for (slot, value) in dst.iter_mut().rev().skip(1).zip(value) {
                *slot = value;
            }
        }

        #[cfg(unix)]
        fn perm_bits(meta: &fs::Metadata) -> i32 {
            // TODO: there should be a platform-specific accessor in the stdlib
            // for this field
            use std::mem;
            unsafe { mem::transmute(meta.permissions()) }
        }
        #[cfg(windows)]
        fn perm_bits(meta: &fs::Metadata) -> i32 {
            if meta.is_dir() {0o755}
            else if perms.readonly() {0o444}
            else {0o644}
        }
    }

    /// Finish writing this archive, emitting the termination sections.
    ///
    /// This function is required to be called to complete the archive, it will
    /// be invalid if this is not called.
    pub fn finish(&self) -> io::Result<()> {
        let b = [0; 1024];
        self.obj.borrow_mut().write_all(&b)
    }
}

impl<'a, R: Seek + Read> Iterator for Files<'a, R> {
    type Item = io::Result<File<'a, R>>;

    fn next(&mut self) -> Option<io::Result<File<'a, R>>> {
        // If we hit a previous error, or we reached the end, we're done here
        if self.done { return None }

        // Seek to the start of the next header in the archive
        try_iter!(self, self.archive.seek(self.offset));

        fn doseek<R: Seek + Read>(file: &File<R>) -> io::Result<()> {
            file.archive.seek(file.tar_offset + file.pos)
        }

        // Parse the next file header
        match try_iter!(self, self.archive.next_file(&mut self.offset, doseek)) {
            None => { self.done = true; None }
            Some(f) => Some(Ok(f)),
        }
    }
}


impl<'a, R: Read> Iterator for FilesMut<'a, R> {
    type Item = io::Result<File<'a, R>>;

    fn next(&mut self) -> Option<io::Result<File<'a, R>>> {
        // If we hit a previous error, or we reached the end, we're done here
        if self.done { return None }

        // Seek to the start of the next header in the archive
        let delta = self.next - self.archive.pos.get();
        try_iter!(self, self.archive.skip(delta));

        // no-op because this reader can't seek
        fn doseek<R>(_: &File<R>) -> io::Result<()> { Ok(()) }

        // Parse the next file header
        match try_iter!(self, self.archive.next_file(&mut self.next, doseek)) {
            None => { self.done = true; None }
            Some(f) => Some(Ok(f)),
        }
    }
}

impl Header {
    fn size(&self) -> io::Result<u64> { octal(&self.size) }
    fn cksum(&self) -> io::Result<u32> { octal(&self.cksum) }
    fn is_ustar(&self) -> bool {
        &self.ustar[..5] == b"ustar"
    }
    fn as_bytes(&self) -> &[u8; 512] {
        unsafe { &*(self as *const _ as *const [u8; 512]) }
    }
}

impl<'a, R> File<'a, R> {
    /// Returns the filename of this archive as a byte array
    pub fn filename_bytes(&self) -> &[u8] {
        self.filename.as_slice()
    }

    /// Returns the filename of this archive as a utf8 string.
    ///
    /// If `None` is returned, then the filename is not valid utf8
    pub fn filename(&self) -> Option<&str> {
        str::from_utf8(self.filename_bytes()).ok()
    }

    /// Returns the value of the owner's user ID field
    pub fn uid(&self) -> io::Result<u32> { octal(&self.header.owner_id) }
    /// Returns the value of the group's user ID field
    pub fn gid(&self) -> io::Result<u32> { octal(&self.header.group_id) }
    /// Returns the last modification time in Unix time format
    pub fn mtime(&self) -> io::Result<u64> { octal(&self.header.mtime) }
    /// Returns the mode bits for this file
    pub fn mode(&self) -> io::Result<i32> {
        octal(&self.header.mode)
    }

    // /// Classify the type of file that this entry represents
    // pub fn classify(&self) -> old_io::FileType {
    //     match (self.header.is_ustar(), self.header.link[0]) {
    //         (_, b'0') => old_io::FileType::RegularFile,
    //         (_, b'1') => old_io::FileType::Unknown, // need a hard link enum?
    //         (_, b'2') => old_io::FileType::Symlink,
    //         (false, _) => old_io::FileType::Unknown, // not technically valid...
    //
    //         (_, b'3') => old_io::FileType::Unknown, // character special...
    //         (_, b'4') => old_io::FileType::BlockSpecial,
    //         (_, b'5') => old_io::FileType::Directory,
    //         (_, b'6') => old_io::FileType::NamedPipe,
    //         (_, _) => old_io::FileType::Unknown, // not technically valid...
    //     }
    // }

    /// Returns the username of the owner of this file, if present
    pub fn username_bytes(&self) -> Option<&[u8]> {
        if self.header.is_ustar() {
            Some(truncate(&self.header.owner_name))
        } else {
            None
        }
    }
    /// Returns the group name of the owner of this file, if present
    pub fn groupname_bytes(&self) -> Option<&[u8]> {
        if self.header.is_ustar() {
            Some(truncate(&self.header.group_name))
        } else {
            None
        }
    }
    /// Return the username of the owner of this file, if present and if valid
    /// utf8
    pub fn username(&self) -> Option<&str> {
        self.username_bytes().and_then(|s| str::from_utf8(s).ok())
    }
    /// Return the group name of the owner of this file, if present and if valid
    /// utf8
    pub fn groupname(&self) -> Option<&str> {
        self.groupname_bytes().and_then(|s| str::from_utf8(s).ok())
    }

    /// Returns the device major number, if present.
    ///
    /// This field is only present in UStar archives. A value of `None` means
    /// that this archive is not a UStar archive, while a value of `Some`
    /// represents the attempt to decode the field in the header.
    pub fn device_major(&self) -> Option<io::Result<u32>> {
        if self.header.is_ustar() {
            Some(octal(&self.header.dev_major))
        } else {
            None
        }
    }
    /// Returns the device minor number, if present.
    ///
    /// This field is only present in UStar archives. A value of `None` means
    /// that this archive is not a UStar archive, while a value of `Some`
    /// represents the attempt to decode the field in the header.
    pub fn device_minor(&self) -> Option<io::Result<u32>> {
        if self.header.is_ustar() {
            Some(octal(&self.header.dev_minor))
        } else {
            None
        }
    }

    /// Returns raw access to the header of this file in the archive.
    pub fn raw_header(&self) -> &Header { &self.header }

    /// Returns the size of the file in the archive.
    pub fn size(&self) -> u64 { self.size }
}

impl<'a, R: Read> Read for &'a Archive<R> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        self.obj.borrow_mut().read(into).map(|i| {
            self.pos.set(self.pos.get() + i as u64);
            i
        })
    }
}

impl<'a, R: Read> Read for File<'a, R> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        if self.size == self.pos { return Ok(0) }

        try!((self.seek)(self));
        let amt = cmp::min((self.size - self.pos) as usize, into.len());
        let amt = try!(Read::read(&mut self.archive, &mut into[..amt]));
        self.pos += amt as u64;
        Ok(amt)
    }
}

impl<'a, R: Read + Seek> Seek for File<'a, R> {
    fn seek(&mut self, how: SeekFrom) -> io::Result<u64> {
        let next = match how {
            SeekFrom::Start(pos) => pos as i64,
            SeekFrom::Current(pos) => self.pos as i64 + pos,
            SeekFrom::End(pos) => self.size as i64 + pos,
        };
        if next < 0 {
            Err(Error::new(ErrorKind::Other, "cannot seek before position 0",
                           None))
        } else if next as u64 > self.size {
            Err(Error::new(ErrorKind::Other, "cannot seek past end of file",
                           None))
        } else {
            self.pos = next as u64;
            Ok(self.pos)
        }
    }
}

fn bad_archive() -> Error {
    Error::new(ErrorKind::Other, "invalid tar archive", None)
}

fn octal<T: num::FromStrRadix>(slice: &[u8]) -> io::Result<T> {
    let num = match str::from_utf8(truncate(slice)) {
        Ok(n) => n,
        Err(_) => return Err(bad_archive()),
    };
    match num::from_str_radix(num.trim(), 8) {
        Ok(n) => Ok(n),
        Err(_) => Err(bad_archive())
    }
}

fn truncate<'a>(slice: &'a [u8]) -> &'a [u8] {
    match slice.iter().position(|i| *i == 0) {
        Some(i) => &slice[..i],
        None => slice,
    }
}

#[cfg(test)] mod tempdir;

#[cfg(test)]
mod tests {
    use std::io::prelude::*;
    use std::io::{Cursor, SeekFrom};
    use std::iter::repeat;
    use std::fs::File;

    use tempdir::TempDir;
    use super::Archive;

    #[test]
    fn simple() {
        let ar = Archive::new(Cursor::new(include_bytes!("tests/simple.tar")));
        for file in ar.files().unwrap() {
            file.unwrap();
        }
    }

    #[test]
    fn reading_files() {
        let rdr = Cursor::new(include_bytes!("tests/reading_files.tar"));
        let ar = Archive::new(rdr);
        let mut files = ar.files().unwrap();
        let mut a = files.next().unwrap().unwrap();
        let mut b = files.next().unwrap().unwrap();
        assert!(files.next().is_none());

        assert_eq!(a.filename(), Some("a"));
        assert_eq!(b.filename(), Some("b"));
        let mut s = String::new();
        a.read_to_string(&mut s).unwrap();
        assert_eq!(s, "a\na\na\na\na\na\na\na\na\na\na\n");
        s.truncate(0);
        b.read_to_string(&mut s).unwrap();
        assert_eq!(s, "b\nb\nb\nb\nb\nb\nb\nb\nb\nb\nb\n");
        a.seek(SeekFrom::Start(0)).unwrap();
        s.truncate(0);
        a.read_to_string(&mut s).unwrap();
        assert_eq!(s, "a\na\na\na\na\na\na\na\na\na\na\n");
    }

    #[test]
    fn writing_files() {
        let wr = Cursor::new(Vec::new());
        let ar = Archive::new(wr);
        let td = TempDir::new("tar-rs").unwrap();

        let path = td.path().join("test");
        File::create(&path).unwrap().write_all(b"test").unwrap();

        ar.append("test2", &mut File::open(&path).unwrap()).unwrap();
        ar.finish().unwrap();

        let rd = Cursor::new(ar.into_inner().into_inner());
        let ar = Archive::new(rd);
        let mut files = ar.files().unwrap();
        let mut f = files.next().unwrap().unwrap();
        assert!(files.next().is_none());

        assert_eq!(f.filename(), Some("test2"));
        assert_eq!(f.size(), 4);
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        assert_eq!(s, "test");
    }

    #[test]
    fn large_filename() {
        let ar = Archive::new(Cursor::new(Vec::new()));
        let td = TempDir::new("tar-rs").unwrap();

        let path = td.path().join("test");
        File::create(&path).unwrap().write_all(b"test").unwrap();

        let filename = repeat("abcd/").take(50).collect::<String>();
        ar.append(filename.as_slice(), &mut File::open(&path).unwrap()).unwrap();
        ar.finish().unwrap();

        let too_long = repeat("abcd").take(200).collect::<String>();
        ar.append(too_long.as_slice(), &mut File::open(&path).unwrap())
          .err().unwrap();

        let rd = Cursor::new(ar.into_inner().into_inner());
        let ar = Archive::new(rd);
        let mut files = ar.files().unwrap();
        let mut f = files.next().unwrap().unwrap();
        assert!(files.next().is_none());

        assert_eq!(f.filename(), Some(filename.as_slice()));
        assert_eq!(f.size(), 4);
        let mut s = String::new();
        f.read_to_string(&mut s).unwrap();
        assert_eq!(s, "test");
    }

    #[test]
    fn reading_files_mut() {
        let rdr = Cursor::new(include_bytes!("tests/reading_files.tar"));
        let mut ar = Archive::new(rdr);
        let mut files = ar.files_mut().unwrap();
        let mut a = files.next().unwrap().unwrap();
        assert_eq!(a.filename(), Some("a"));
        let mut s = String::new();
        a.read_to_string(&mut s).unwrap();
        assert_eq!(s, "a\na\na\na\na\na\na\na\na\na\na\n");
        s.truncate(0);
        a.read_to_string(&mut s).unwrap();
        assert_eq!(s, "");
        let mut b = files.next().unwrap().unwrap();

        assert_eq!(b.filename(), Some("b"));
        s.truncate(0);
        b.read_to_string(&mut s).unwrap();
        assert_eq!(s, "b\nb\nb\nb\nb\nb\nb\nb\nb\nb\nb\n");
        assert!(files.next().is_none());
    }

    #[test]
    fn extracting_directories() {
        let td = TempDir::new("tar-rs").unwrap();
        let rdr = Cursor::new(include_bytes!("tests/directory.tar"));
        let mut ar = Archive::new(rdr);
        ar.unpack(td.path()).unwrap();

        let dir_a = td.path().join("a");
        let dir_b = td.path().join("a/b");
        let file_c = td.path().join("a/c");
        assert!(dir_a.is_dir());
        assert!(dir_b.is_dir());
        assert!(file_c.is_file());
    }

    #[test]
    fn octal_spaces()
    {
        let rdr = Cursor::new(include_bytes!("tests/spaces.tar"));
        let ar = Archive::new(rdr);

        let file = ar.files().unwrap().next().unwrap().unwrap();
        assert_eq!(file.mode().unwrap() & 0o777, 0o777);
        assert_eq!(file.uid().unwrap(), 0);
        assert_eq!(file.gid().unwrap(), 0);
        assert_eq!(file.size, 2);
        assert_eq!(file.mtime().unwrap(), 0o12440016664);
        assert_eq!(file.header.cksum().unwrap(), 0o4253);
    }
}
