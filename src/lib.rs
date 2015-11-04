//! A library for reading and writing TAR archives
//!
//! This library provides utilities necessary to manage TAR archives [1]
//! abstracted over a reader or writer. Great strides are taken to ensure that
//! an archive is never required to be fully resident in memory, all objects
//! provide largely a streaming interface to read bytes from.
//!
//! [1]: http://en.wikipedia.org/wiki/Tar_%28computing%29

#![doc(html_root_url = "http://alexcrichton.com/tar-rs")]
#![deny(missing_docs)]
#![cfg_attr(test, deny(warnings))]

extern crate libc;
extern crate winapi;
extern crate filetime;

use std::borrow::Cow;
use std::cell::{RefCell, Cell};
use std::cmp;
use std::error;
use std::fmt;
use std::fs;
use std::io::prelude::*;
use std::io::{self, Error, ErrorKind, SeekFrom};
use std::iter::repeat;
use std::mem;
use std::path::{Path, PathBuf, Component};
use std::str;

use filetime::FileTime;

#[cfg(unix)] use std::os::unix::prelude::*;
#[cfg(unix)] use std::ffi::{OsStr, OsString};
#[cfg(windows)] use std::os::windows::prelude::*;

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
/// This structure is a window into a portion of a borrowed archive which can
/// be inspected. It acts as a file handle by implementing the Reader and Seek
/// traits. A file cannot be rewritten once inserted into an archive.
pub struct File<'a, R: 'a> {
    header: Header,
    archive: &'a Archive<R>,
    pos: u64,
    size: u64,

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

#[doc(hidden)]
#[derive(Debug)]
pub struct TarError {
    desc: String,
    io: io::Error,
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
    /// This function can return an error if any underlying I/O operation fails
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
        if self.pos.get() != 0 {
            return Err(Error::new(ErrorKind::Other, "cannot call files_mut \
                                                     unless archive is at \
                                                     position 0"))
        }
        Ok(FilesMut { archive: self, done: false, next: 0 })
    }

    /// Unpacks the contents tarball into the specified `dst`.
    ///
    /// This function will iterate over the entire contents of this tarball,
    /// extracting each file in turn to the location specified by the entry's
    /// path name.
    ///
    /// This operation is relatively sensitive in that it will not write files
    /// outside of the path specified by `into`. Files in the archive which have
    /// a '..' in their path are skipped during the unpacking process.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs::File;
    /// use tar::Archive;
    ///
    /// let mut ar = Archive::new(File::open("foo.tar").unwrap());
    /// ar.unpack("foo").unwrap();
    /// ```
    pub fn unpack<P: AsRef<Path>>(&mut self, dst: P) -> io::Result<()> {
        self.unpack2(dst.as_ref())
    }

    fn unpack2(&mut self, dst: &Path) -> io::Result<()> {
        'outer: for file in try!(self.files_mut()) {
            let mut file = try!(file.map_err(|e| {
                TarError::new("failed to iterate over archive", e)
            }));

            // Notes regarding bsdtar 2.8.3 / libarchive 2.8.3:
            // * Leading '/'s are trimmed. For example, `///test` is treated as
            //   `test`.
            // * If the filename contains '..', then the file is skipped when
            //   extracting the tarball.
            // * '//' within a filename is effectively skipped. An error is
            //   logged, but otherwise the effect is as if any two or more
            //   adjacent '/'s within the filename were consolidated into one
            //   '/'.
            //
            // Most of this is handled by the `path` module of the standard
            // library, but we specially handle a few cases here as well.

            let mut file_dst = dst.to_path_buf();
            {
                let path = try!(file.header().path().map_err(|e| {
                    TarError::new("invalid path in entry header", e)
                }));
                for part in path.components() {
                    match part {
                        // Leading '/' characters, root paths, and '.'
                        // components are just ignored and treated as "empty
                        // components"
                        Component::Prefix(..) |
                        Component::RootDir |
                        Component::CurDir => continue,

                        // If any part of the filename is '..', then skip over
                        // unpacking the file to prevent directory traversal
                        // security issues.  See, e.g.: CVE-2001-1267,
                        // CVE-2002-0399, CVE-2005-1918, CVE-2007-4131
                        Component::ParentDir => continue 'outer,

                        Component::Normal(part) => file_dst.push(part),
                    }
                }
            }

            // Skip cases where only slashes or '.' parts were seen, because
            // this is effectively an empty filename.
            if *dst == *file_dst {
                continue
            }

            if file.header().link[0] == b'5' {
                try!(fs::create_dir_all(&file_dst).map_err(|e| {
                    TarError::new(&format!("failed to create `{}`",
                                           file_dst.display()), e)
                }));
            } else {
                let dir = file_dst.parent().unwrap();
                try!(fs::create_dir_all(&dir).map_err(|e| {
                    TarError::new(&format!("failed to create `{}`",
                                           dir.display()), e)
                }));
                try!(file.unpack(&file_dst));
            }
        }
        Ok(())
    }

    fn skip(&self, mut amt: u64) -> io::Result<()> {
        let mut buf = [0u8; 4096 * 8];
        let mut me = self;
        while amt > 0 {
            let n = cmp::min(amt, buf.len() as u64);
            let n = try!(Read::read(&mut me, &mut buf[..n as usize]));
            if n == 0 {
                let errstr = "unexpected EOF during skip";
                return Err(Error::new(ErrorKind::Other, errstr));
            }
            amt -= n as u64;
        }
        Ok(())
    }

    // Assumes that the underlying reader is positioned at the start of a valid
    // header to parse.
    fn next_file(&self, offset: &mut u64, seek: fn(&File<R>) -> io::Result<()>)
                 -> io::Result<Option<File<R>>> {
        // If we have 2 or more sections of 0s, then we're done!
        let mut chunk = [0; 512];
        let mut me = self;
        try!(read_all(&mut me, &mut chunk));
        *offset += 512;
        // A block of 0s is never valid as a header (because of the checksum),
        // so if it's all zero it must be the first of the two end blocks
        if chunk.iter().all(|i| *i == 0) {
            try!(read_all(&mut me, &mut chunk));
            *offset += 512;
            return if chunk.iter().all(|i| *i == 0) {
                Ok(None)
            } else {
                Err(bad_archive())
            }
        }

        let sum = chunk[..148].iter().map(|i| *i as u32).fold(0, |a, b| a + b) +
                  chunk[156..].iter().map(|i| *i as u32).fold(0, |a, b| a + b) +
                  32 * 8;

        let header: Header = unsafe { mem::transmute(chunk) };
        let ret = File {
            archive: self,
            pos: 0,
            size: try!(header.size()),
            header: header,
            tar_offset: *offset,
            seek: seek,
        };

        // Make sure the checksum is ok
        let cksum = try!(ret.header.cksum());
        if sum != cksum { return Err(bad_archive()) }

        // Figure out where the next file is
        let size = (ret.size + 511) & !(512 - 1);
        *offset += size;

        return Ok(Some(ret));
    }
}

impl<W: Write> Archive<W> {
    /// Adds a new entry to this archive.
    ///
    /// This function will append the header specified, followed by contents of
    /// the stream specified by `data`. To produce a valid archive the `size`
    /// field of `header` must be the same as the length of the stream that's
    /// being written. Additionally the checksum for the header should have been
    /// set via the `set_cksum` method.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    ///
    /// Also note that after all files have been written to an archive the
    /// `finish` function needs to be called to finish writing the archive.
    ///
    /// # Errors
    ///
    /// This function will return an error for any intermittent I/O error which
    /// occurs when either reading or writing.
    ///
    /// # Examples
    ///
    /// ```
    /// use tar::{Archive, Header};
    ///
    /// let mut header = Header::new();
    /// header.set_path("foo");
    /// header.set_size(4);
    /// header.set_cksum();
    ///
    /// let mut data: &[u8] = &[1, 2, 3, 4];
    ///
    /// let mut ar = Archive::new(Vec::new());
    /// ar.append(&header, &mut data).unwrap();
    /// let archive = ar.into_inner();
    /// ```
    pub fn append(&self, header: &Header, mut data: &mut Read) -> io::Result<()> {
        let mut obj = self.obj.borrow_mut();
        try!(obj.write_all(header.as_bytes()));
        let len = try!(io::copy(&mut data, &mut *obj));

        // Pad with zeros if necessary.
        let buf = [0; 512];
        let remaining = 512 - (len % 512);
        if remaining < 512 {
            try!(obj.write_all(&buf[..remaining as usize]));
        }

        Ok(())
    }

    /// Adds a file on the local filesystem to this archive.
    ///
    /// This function will open the file specified by `path` and insert the file
    /// into the archive with the appropriate metadata set, returning any I/O
    /// error which occurs while writing. The path name for the file inside of
    /// this archive will be the same as `path`, and it is recommended that the
    /// path is a relative path.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    ///
    /// Also note that after all files have been written to an archive the
    /// `finish` function needs to be called to finish writing the archive.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use tar::Archive;
    ///
    /// let mut ar = Archive::new(Vec::new());
    ///
    /// ar.append_path("foo/bar.txt").unwrap();
    /// ```
    pub fn append_path<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        self.append_path2(path.as_ref())
    }

    fn append_path2(&self, path: &Path) -> io::Result<()> {
        let stat = try!(fs::metadata(path));
        let header = try!(Header::from_path_and_metadata(path, &stat));
        if stat.is_file() {
            let mut file = try!(fs::File::open(path));
            self.append(&header, &mut file)
        } else if stat.is_dir() {
            self.append(&header, &mut io::empty())
        } else {
            Err(Error::new(ErrorKind::Other, "path has unknown file type"))
        }
    }

    /// Adds a file to this archive with the given path as the name of the file
    /// in the archive.
    ///
    /// This will use the metadata of `file` to populate a `Header`, and it will
    /// then append the file to the archive with the name `path`.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    ///
    /// Also note that after all files have been written to an archive the
    /// `finish` function needs to be called to finish writing the archive.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs::File;
    /// use tar::Archive;
    ///
    /// let mut ar = Archive::new(Vec::new());
    ///
    /// // Open the file at one location, but insert it into the archive with a
    /// // different name.
    /// let mut f = File::open("foo/bar/baz.txt").unwrap();
    /// ar.append_file("bar/baz.txt", &mut f).unwrap();
    /// ```
    pub fn append_file<P: AsRef<Path>>(&self, path: P, file: &mut fs::File)
                                       -> io::Result<()> {
        self.append_file2(path.as_ref(), file)
    }

    fn append_file2(&self, path: &Path, file: &mut fs::File) -> io::Result<()> {
        let stat = try!(file.metadata());
        let header = try!(Header::from_path_and_metadata(path, &stat));
        self.append(&header, file)
    }

    /// Adds a directory to this archive with the given path as the name of the
    /// directory in the archive.
    ///
    /// This will use `stat` to populate a `Header`, and it will then append the
    /// directory to the archive with the name `path`.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    ///
    /// Also note that after all files have been written to an archive the
    /// `finish` function needs to be called to finish writing the archive.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::fs;
    /// use tar::Archive;
    ///
    /// let mut ar = Archive::new(Vec::new());
    ///
    /// // Use the directory at one location, but insert it into the archive
    /// // with a different name.
    /// ar.append_dir("bardir", ".").unwrap();
    /// ```
    pub fn append_dir<P: AsRef<Path>, P2: AsRef<Path>>(
                      &self, path: P, src_path: P2) -> io::Result<()> {
        self.append_dir2(path.as_ref(), src_path.as_ref())
    }

    fn append_dir2(&self, path: &Path, src_path: &Path) -> io::Result<()> {
        let stat = try!(fs::metadata(src_path));
        let header = try!(Header::from_path_and_metadata(path, &stat));
        self.append(&header, &mut io::empty())
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

impl Clone for Header {
    fn clone(&self) -> Header {
        Header { ..*self }
    }
}

impl Header {
    /// Creates a new blank ustar header ready to be filled in
    pub fn new() -> Header {
        let mut header: Header = unsafe { mem::zeroed() };
        // Flag this header as a UStar archive
        header.ustar = *b"ustar\0";
        header.ustar_version = *b"00";
        return header
    }

    fn from_path_and_metadata(path: &Path, stat: &fs::Metadata)
                              -> io::Result<Header> {
        let mut header = Header::new();
        // TODO: add trailing path::MAIN_SEPARATOR onto directories for
        // compatibility. Requires either the std::path to allow it or OsStr
        // to permit character checks
        // https://github.com/rust-lang/rust/issues/29008
        try!(header.set_path(path));
        header.set_metadata(&stat);
        header.set_cksum();
        Ok(header)
    }

    fn is_ustar(&self) -> bool {
        &self.ustar[..5] == b"ustar"
    }

    /// Returns a view into this header as a byte array.
    pub fn as_bytes(&self) -> &[u8; 512] {
        debug_assert_eq!(512, mem::size_of_val(self));
        unsafe { &*(self as *const _ as *const [u8; 512]) }
    }

    /// Blanket sets the metadata in this header from the metadata argument
    /// provided.
    ///
    /// This is useful for initializing a `Header` from the OS's metadata from a
    /// file.
    pub fn set_metadata(&mut self, meta: &fs::Metadata) {
        // Platform-specific fill
        self.fill_from(meta);
        // Platform-agnostic fill
        // Set size of directories to zero
        self.set_size(if meta.is_dir() { 0 } else { meta.len() });
        self.set_device_major(0);
        self.set_device_minor(0);
    }

    /// Returns the file size this header represents.
    ///
    /// May return an error if the field is corrupted.
    pub fn size(&self) -> io::Result<u64> {
        octal_from(&self.size)
    }

    /// Encodes the `size` argument into the size field of this header.
    pub fn set_size(&mut self, size: u64) {
        octal_into(&mut self.size, size)
    }

    /// Returns the pathname stored in this header.
    ///
    /// This method may fail if the pathname is not valid unicode and this is
    /// called on a Windows platform.
    ///
    /// Note that this function will convert any `\` characters to directory
    /// separators.
    pub fn path(&self) -> io::Result<Cow<Path>> {
        return bytes2path(self.path_bytes());

        #[cfg(windows)]
        fn bytes2path(bytes: Cow<[u8]>) -> io::Result<Cow<Path>> {
            match bytes {
                Cow::Borrowed(bytes) => {
                    let s = try!(str::from_utf8(bytes).map_err(|_| {
                        not_unicode()
                    }));
                    Ok(Cow::Borrowed(Path::new(s)))
                }
                Cow::Owned(bytes) => {
                    let s = try!(String::from_utf8(bytes).map_err(|_| {
                        not_unicode()
                    }));
                    Ok(Cow::Owned(PathBuf::from(s)))
                }
            }
        }
        #[cfg(unix)]
        fn bytes2path(bytes: Cow<[u8]>) -> io::Result<Cow<Path>> {
            Ok(match bytes {
                Cow::Borrowed(bytes) => Cow::Borrowed({
                    Path::new(OsStr::from_bytes(bytes))
                }),
                Cow::Owned(bytes) => Cow::Owned({
                    PathBuf::from(OsString::from_vec(bytes))
                })
            })
        }
    }

    /// Returns the pathname stored in this header as a byte array.
    ///
    /// This function is guaranteed to succeed, but you may wish to call the
    /// `path` method to convert to a `Path`.
    ///
    /// Note that this function will convert any `\` characters to directory
    /// separators.
    pub fn path_bytes(&self) -> Cow<[u8]> {
        if (!self.is_ustar() || self.prefix[0] == 0) &&
           !self.name.contains(&b'\\') {
            Cow::Borrowed(truncate(&self.name))
        } else {
            fn noslash(b: &u8) -> u8 {
                if *b == b'\\' {b'/'} else {*b}
            }
            let mut bytes = Vec::new();
            let prefix = truncate(&self.prefix);
            if prefix.len() > 0 {
                bytes.extend(prefix.iter().map(noslash));
                bytes.push(b'/');
            }
            bytes.extend(truncate(&self.name).iter().map(noslash));
            Cow::Owned(bytes)
        }
    }

    /// Sets the path name for this header.
    ///
    /// This function will set the pathname listed in this header, encoding it
    /// in the appropriate format. May fail if the path is too long or if the
    /// path specified is not unicode and this is a Windows platform.
    pub fn set_path<P: AsRef<Path>>(&mut self, p: P) -> io::Result<()> {
        self.set_path2(p.as_ref())
    }

    fn set_path2(&mut self, path: &Path) -> io::Result<()> {
        let bytes = match bytes(path) {
            Some(b) => b,
            None => return Err(Error::new(ErrorKind::Other, "path was not \
                                                             valid unicode")),
        };
        if bytes.iter().any(|b| *b == 0) {
            return Err(Error::new(ErrorKind::Other, "path contained a nul byte"))
        }

        let (namelen, prefixlen) = (self.name.len(), self.prefix.len());
        if bytes.len() < namelen {
            try!(copy_into(&mut self.name, bytes, true));
        } else {
            let prefix = &bytes[..cmp::min(bytes.len(), prefixlen)];
            let pos = match prefix.iter().rposition(|&b| b == b'/' || b == b'\\') {
                Some(i) => i,
                None => return Err(Error::new(ErrorKind::Other,
                                              "path cannot be split to be \
                                               inserted into archive")),
            };
            try!(copy_into(&mut self.name, &bytes[pos + 1..], true));
            try!(copy_into(&mut self.prefix, &bytes[..pos], true));
        }
        return Ok(());

        #[cfg(windows)]
        fn bytes(p: &Path) -> Option<&[u8]> {
            p.as_os_str().to_str().map(|s| s.as_bytes())
        }
        #[cfg(unix)]
        fn bytes(p: &Path) -> Option<&[u8]> {
            Some(p.as_os_str().as_bytes())
        }
    }

    /// Returns the mode bits for this file
    ///
    /// May return an error if the field is corrupted.
    pub fn mode(&self) -> io::Result<u32> {
        octal_from(&self.mode).map(|u| u as u32)
    }

    /// Encodes the `mode` provided into this header.
    pub fn set_mode(&mut self, mode: u32) {
        octal_into(&mut self.mode, mode & 0o3777);
    }

    /// Returns the value of the owner's user ID field
    ///
    /// May return an error if the field is corrupted.
    pub fn uid(&self) -> io::Result<u32> {
        octal_from(&self.owner_id).map(|u| u as u32)
    }

    /// Encodes the `uid` provided into this header.
    pub fn set_uid(&mut self, uid: u32) {
        octal_into(&mut self.owner_id, uid);
    }

    /// Returns the value of the group's user ID field
    pub fn gid(&self) -> io::Result<u32> {
        octal_from(&self.group_id).map(|u| u as u32)
    }

    /// Encodes the `gid` provided into this header.
    pub fn set_gid(&mut self, gid: u32) {
        octal_into(&mut self.group_id, gid);
    }

    /// Returns the last modification time in Unix time format
    pub fn mtime(&self) -> io::Result<u64> {
        octal_from(&self.mtime)
    }

    /// Encodes the `mtime` provided into this header.
    ///
    /// Note that this time is typically a number of seconds passed since
    /// January 1, 1970.
    pub fn set_mtime(&mut self, mtime: u64) {
        octal_into(&mut self.mtime, mtime);
    }

    /// Return the username of the owner of this file, if present and if valid
    /// utf8
    pub fn username(&self) -> Option<&str> {
        self.username_bytes().and_then(|s| str::from_utf8(s).ok())
    }

    /// Returns the username of the owner of this file, if present
    pub fn username_bytes(&self) -> Option<&[u8]> {
        if self.is_ustar() {
            Some(truncate(&self.owner_name))
        } else {
            None
        }
    }

    /// Sets the username inside this header.
    ///
    /// May return an error if the name provided is too long.
    pub fn set_username(&mut self, name: &str) -> io::Result<()> {
        copy_into(&mut self.owner_name, name.as_bytes(), false)
    }

    /// Return the group name of the owner of this file, if present and if valid
    /// utf8
    pub fn groupname(&self) -> Option<&str> {
        self.groupname_bytes().and_then(|s| str::from_utf8(s).ok())
    }

    /// Returns the group name of the owner of this file, if present
    pub fn groupname_bytes(&self) -> Option<&[u8]> {
        if self.is_ustar() {
            Some(truncate(&self.group_name))
        } else {
            None
        }
    }

    /// Sets the group name inside this header.
    ///
    /// May return an error if the name provided is too long.
    pub fn set_groupname(&mut self, name: &str) -> io::Result<()> {
        copy_into(&mut self.group_name, name.as_bytes(), false)
    }

    /// Returns the device major number, if present.
    ///
    /// This field is only present in UStar archives. A value of `None` means
    /// that this archive is not a UStar archive, while a value of `Some`
    /// represents the attempt to decode the field in the header.
    pub fn device_major(&self) -> Option<io::Result<u32>> {
        if self.is_ustar() {
            Some(octal_from(&self.dev_major).map(|u| u as u32))
        } else {
            None
        }
    }

    /// Encodes the value `major` into the dev_major field of this header.
    pub fn set_device_major(&mut self, major: u32) {
        octal_into(&mut self.dev_major, major);
    }

    /// Returns the device minor number, if present.
    ///
    /// This field is only present in UStar archives. A value of `None` means
    /// that this archive is not a UStar archive, while a value of `Some`
    /// represents the attempt to decode the field in the header.
    pub fn device_minor(&self) -> Option<io::Result<u32>> {
        if self.is_ustar() {
            Some(octal_from(&self.dev_minor).map(|u| u as u32))
        } else {
            None
        }
    }

    /// Encodes the value `minor` into the dev_major field of this header.
    pub fn set_device_minor(&mut self, minor: u32) {
        octal_into(&mut self.dev_minor, minor);
    }

    /// Returns the checksum field of this header.
    ///
    /// May return an error if the field is corrupted.
    pub fn cksum(&self) -> io::Result<u32> {
        octal_from(&self.cksum).map(|u| u as u32)
    }

    /// Sets the checksum field of this header based on the current fields in
    /// this header.
    pub fn set_cksum(&mut self) {
        let cksum = {
            let bytes = self.as_bytes();
            bytes[..148].iter().map(|i| *i as u32).fold(0, |a, b| a + b) +
                bytes[156..].iter().map(|i| *i as u32).fold(0, |a, b| a + b) +
                32 * (self.cksum.len() as u32)
        };
        octal_into(&mut self.cksum, cksum);
    }

    #[cfg(unix)]
    fn fill_from(&mut self, meta: &fs::Metadata) {
        self.set_mode((meta.mode() & 0o3777) as u32);
        self.set_mtime(meta.mtime() as u64);
        self.set_uid(meta.uid() as u32);
        self.set_gid(meta.gid() as u32);

        // TODO: need to bind more file types
        self.link[0] = match meta.mode() & libc::S_IFMT {
            libc::S_IFREG => b'0',
            libc::S_IFLNK => b'2',
            libc::S_IFCHR => b'3',
            libc::S_IFBLK => b'4',
            libc::S_IFDIR => b'5',
            libc::S_IFIFO => b'6',
            _ => b' ',
        };
    }

    #[cfg(windows)]
    fn fill_from(&mut self, meta: &fs::Metadata) {
        let readonly = meta.file_attributes() & winapi::FILE_ATTRIBUTE_READONLY;

        // There's no concept of a mode on windows, so do a best approximation
        // here.
        let mode = match (meta.is_dir(), readonly != 0) {
            (true, false) => 0o755,
            (true, true) => 0o555,
            (false, false) => 0o644,
            (false, true) => 0o444,
        };
        self.set_mode(mode);
        self.set_uid(0);
        self.set_gid(0);

        let ft = meta.file_type();
        self.link[0] = if ft.is_dir() {
            b'5'
        } else if ft.is_file() {
            b'0'
        } else if ft.is_symlink() {
            b'2'
        } else {
            b' '
        };

        // The dates listed in tarballs are always seconds relative to
        // January 1, 1970. On Windows, however, the timestamps are returned as
        // dates relative to January 1, 1601 (in 100ns intervals), so we need to
        // add in some offset for those dates.
        let mtime = (meta.last_write_time() / (1_000_000_000 / 100)) - 11644473600;
        self.set_mtime(mtime);
    }
}

impl<'a, R: Read> File<'a, R> {
    /// Returns access to the header of this file in the archive.
    ///
    /// This provides access to the the metadata for this file in the archive.
    pub fn header(&self) -> &Header { &self.header }

    /// Writes this file to the specified location.
    ///
    /// This function will write the entire contents of this file into the
    /// location specified by `dst`. Metadata will also be propagated to the
    /// path `dst`.
    ///
    /// This function will create a file at the path `dst`, and it is required
    /// that the intermediate directories are created. Any existing file at the
    /// location `dst` will be overwritten.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::fs::File;
    /// use tar::Archive;
    ///
    /// let ar = Archive::new(File::open("foo.tar").unwrap());
    ///
    /// for (i, file) in ar.files().unwrap().enumerate() {
    ///     let mut file = file.unwrap();
    ///     file.unpack(format!("file-{}", i)).unwrap();
    /// }
    /// ```
    pub fn unpack<P: AsRef<Path>>(&mut self, dst: P) -> io::Result<()> {
        self.unpack2(dst.as_ref())
    }

    fn unpack2(&mut self, dst: &Path) -> io::Result<()> {
        try!(fs::File::create(dst).and_then(|mut f| {
            if try!(io::copy(self, &mut f)) != self.size {
                return Err(bad_archive());
            }
            Ok(())
        }).map_err(|e| {
            let header = self.header().path_bytes();
            TarError::new(&format!("failed to unpack `{}` into `{}`",
                                   String::from_utf8_lossy(&header),
                                   dst.display()), e)
        }));

        if let Ok(mtime) = self.header().mtime() {
            let mtime = FileTime::from_seconds_since_1970(mtime, 0);
            try!(filetime::set_file_times(dst, mtime, mtime).map_err(|e| {
                TarError::new(&format!("failed to set mtime for `{}`",
                                       dst.display()), e)
            }));
        }
        if let Ok(mode) = self.header().mode() {
            try!(set_perms(dst, mode).map_err(|e| {
                TarError::new(&format!("failed to set permissions to {:o} \
                                        for `{}`", mode, dst.display()), e)
            }));
        }
        return Ok(());

        #[cfg(unix)]
        fn set_perms(dst: &Path, mode: u32) -> io::Result<()> {
            use std::os::unix::raw;
            let perm = fs::Permissions::from_mode(mode as raw::mode_t);
            fs::set_permissions(dst, perm)
        }
        #[cfg(windows)]
        fn set_perms(dst: &Path, mode: u32) -> io::Result<()> {
            let mut perm = try!(fs::metadata(dst)).permissions();
            perm.set_readonly(mode & 0o200 != 0o200);
            fs::set_permissions(dst, perm)
        }
    }
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
            Err(Error::new(ErrorKind::Other, "cannot seek before position 0"))
        } else if next as u64 > self.size {
            Err(Error::new(ErrorKind::Other, "cannot seek past end of file"))
        } else {
            self.pos = next as u64;
            Ok(self.pos)
        }
    }
}

fn bad_archive() -> Error {
    Error::new(ErrorKind::Other, "invalid tar archive")
}

fn octal_from(slice: &[u8]) -> io::Result<u64> {
    let num = match str::from_utf8(truncate(slice)) {
        Ok(n) => n,
        Err(_) => return Err(bad_archive()),
    };
    match u64::from_str_radix(num.trim(), 8) {
        Ok(n) => Ok(n),
        Err(_) => Err(bad_archive())
    }
}

fn octal_into<T: fmt::Octal>(dst: &mut [u8], val: T) {
    let o = format!("{:o}", val);
    let value = o.bytes().rev().chain(repeat(b'0'));
    for (slot, value) in dst.iter_mut().rev().skip(1).zip(value) {
        *slot = value;
    }
}

fn truncate<'a>(slice: &'a [u8]) -> &'a [u8] {
    match slice.iter().position(|i| *i == 0) {
        Some(i) => &slice[..i],
        None => slice,
    }
}

fn read_all<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<()> {
    let mut read = 0;
    while read < buf.len() {
        match try!(r.read(&mut buf[read..])) {
            0 => return Err(bad_archive()),
            n => read += n,
        }
    }
    Ok(())
}

/// Copies `bytes` into the `slot` provided, returning an error if the `bytes`
/// array is too long or if it contains any nul bytes.
///
/// Also provides the option to map '\' characters to '/' characters for the
/// names of paths in archives. The `tar` utility doesn't seem to like windows
/// backslashes when unpacking on Unix.
fn copy_into(slot: &mut [u8], bytes: &[u8], map_slashes: bool) -> io::Result<()> {
    if bytes.len() > slot.len() {
        Err(Error::new(ErrorKind::Other, "provided value is too long"))
    } else if bytes.iter().any(|b| *b == 0) {
        Err(Error::new(ErrorKind::Other, "provided value contains a nul byte"))
    } else {
        for (slot, val) in slot.iter_mut().zip(bytes) {
            if map_slashes && *val == b'\\' {
                *slot = b'/';
            } else {
                *slot = *val;
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
fn not_unicode() -> Error {
    Error::new(ErrorKind::Other, "only unicode paths are supported on windows")
}

impl TarError {
    fn new(desc: &str, err: Error) -> TarError {
        TarError {
            desc: desc.to_string(),
            io: err,
        }
    }
}

impl error::Error for TarError {
    fn description(&self) -> &str {
        &self.desc
    }

    fn cause(&self) -> Option<&error::Error> {
        Some(&self.io)
    }
}

impl fmt::Display for TarError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.desc.fmt(f)
    }
}

impl From<TarError> for Error {
    fn from(t: TarError) -> Error {
        Error::new(t.io.kind(), t)
    }
}

#[cfg(test)]
mod tests {
    extern crate tempdir;

    use std::io::prelude::*;
    use std::io::{self, Cursor, SeekFrom};
    use std::iter::repeat;
    use std::fs::{self, File};
    use std::path::Path;

    use filetime::FileTime;
    use self::tempdir::TempDir;
    use super::{Archive, Header};

    macro_rules! t {
        ($e:expr) => (match $e {
            Ok(v) => v,
            Err(e) => panic!("{} returned {}", stringify!($e), e),
        })
    }

    #[test]
    fn simple() {
        let ar = Archive::new(Cursor::new(&include_bytes!("tests/simple.tar")[..]));
        for file in t!(ar.files()) {
            t!(file);
        }
    }

    #[test]
    fn header_impls() {
        let ar = Archive::new(Cursor::new(&include_bytes!("tests/simple.tar")[..]));
        let hn = Header::new();
        let hnb = hn.as_bytes();
        for file in t!(ar.files()) {
            let file = t!(file);
            let h1 = file.header();
            let h1b = h1.as_bytes();
            let h2 = h1.clone();
            let h2b = h2.as_bytes();
            assert!(h1b[..] == h2b[..] && h2b[..] != hnb[..])
        }
    }

    #[test]
    fn reading_files() {
        let rdr = Cursor::new(&include_bytes!("tests/reading_files.tar")[..]);
        let ar = Archive::new(rdr);
        let mut files = t!(ar.files());
        let mut a = t!(files.next().unwrap());
        let mut b = t!(files.next().unwrap());
        assert!(files.next().is_none());

        assert_eq!(&*a.header().path_bytes(), b"a");
        assert_eq!(&*b.header().path_bytes(), b"b");
        let mut s = String::new();
        t!(a.read_to_string(&mut s));
        assert_eq!(s, "a\na\na\na\na\na\na\na\na\na\na\n");
        s.truncate(0);
        t!(b.read_to_string(&mut s));
        assert_eq!(s, "b\nb\nb\nb\nb\nb\nb\nb\nb\nb\nb\n");
        t!(a.seek(SeekFrom::Start(0)));
        s.truncate(0);
        t!(a.read_to_string(&mut s));
        assert_eq!(s, "a\na\na\na\na\na\na\na\na\na\na\n");
    }

    #[test]
    fn writing_files() {
        let wr = Cursor::new(Vec::new());
        let ar = Archive::new(wr);
        let td = t!(TempDir::new("tar-rs"));

        let path = td.path().join("test");
        t!(t!(File::create(&path)).write_all(b"test"));

        t!(ar.append_file("test2", &mut t!(File::open(&path))));
        t!(ar.finish());

        let rd = Cursor::new(ar.into_inner().into_inner());
        let ar = Archive::new(rd);
        let mut files = t!(ar.files());
        let mut f = t!(files.next().unwrap());
        assert!(files.next().is_none());

        assert_eq!(&*f.header().path_bytes(), b"test2");
        assert_eq!(f.header().size().unwrap(), 4);
        let mut s = String::new();
        t!(f.read_to_string(&mut s));
        assert_eq!(s, "test");
    }

    #[test]
    fn large_filename() {
        let ar = Archive::new(Cursor::new(Vec::new()));
        let td = t!(TempDir::new("tar-rs"));

        let path = td.path().join("test");
        t!(t!(File::create(&path)).write_all(b"test"));

        let filename = repeat("abcd/").take(50).collect::<String>();
        t!(ar.append_file(&filename, &mut t!(File::open(&path))));
        t!(ar.finish());

        let too_long = repeat("abcd").take(200).collect::<String>();
        assert!(ar.append_file(&too_long, &mut t!(File::open(&path))).is_err());

        let rd = Cursor::new(ar.into_inner().into_inner());
        let ar = Archive::new(rd);
        let mut files = t!(ar.files());
        let mut f = files.next().unwrap().unwrap();
        assert!(files.next().is_none());

        assert_eq!(&*f.header().path_bytes(), filename.as_bytes());
        assert_eq!(f.header().size().unwrap(), 4);
        let mut s = String::new();
        t!(f.read_to_string(&mut s));
        assert_eq!(s, "test");
    }

    #[test]
    fn reading_files_mut() {
        let rdr = Cursor::new(&include_bytes!("tests/reading_files.tar")[..]);
        let mut ar = Archive::new(rdr);
        let mut files = t!(ar.files_mut());
        let mut a = t!(files.next().unwrap());
        assert_eq!(&*a.header().path_bytes(), b"a");
        let mut s = String::new();
        t!(a.read_to_string(&mut s));
        assert_eq!(s, "a\na\na\na\na\na\na\na\na\na\na\n");
        s.truncate(0);
        t!(a.read_to_string(&mut s));
        assert_eq!(s, "");
        let mut b = t!(files.next().unwrap());

        assert_eq!(&*b.header().path_bytes(), b"b");
        s.truncate(0);
        t!(b.read_to_string(&mut s));
        assert_eq!(s, "b\nb\nb\nb\nb\nb\nb\nb\nb\nb\nb\n");
        assert!(files.next().is_none());
    }

    fn check_dirtree(td: &TempDir) {
        let dir_a = td.path().join("a");
        let dir_b = td.path().join("a/b");
        let file_c = td.path().join("a/c");
        assert!(fs::metadata(&dir_a).map(|m| m.is_dir()).unwrap_or(false));
        assert!(fs::metadata(&dir_b).map(|m| m.is_dir()).unwrap_or(false));
        assert!(fs::metadata(&file_c).map(|m| m.is_file()).unwrap_or(false));
    }

    #[test]
    fn extracting_directories() {
        let td = t!(TempDir::new("tar-rs"));
        let rdr = Cursor::new(&include_bytes!("tests/directory.tar")[..]);
        let mut ar = Archive::new(rdr);
        t!(ar.unpack(td.path()));
        check_dirtree(&td);
    }

    #[test]
    fn writing_and_extracting_directories() {
        let td = t!(TempDir::new("tar-rs"));

        let cur = Cursor::new(Vec::new());
        let ar = Archive::new(cur);
        let tmppath = td.path().join("tmpfile");
        t!(t!(File::create(&tmppath)).write_all(b"c"));
        t!(ar.append_dir("a", "."));
        t!(ar.append_dir("a/b", "."));
        t!(ar.append_file("a/c", &mut t!(File::open(&tmppath))));
        t!(ar.finish());

        let rdr = Cursor::new(ar.into_inner().into_inner());
        let mut ar = Archive::new(rdr);
        t!(ar.unpack(td.path()));
        check_dirtree(&td);
    }

    #[test]
    fn extracting_duplicate_dirs() {
        let td = t!(TempDir::new("tar-rs"));
        let rdr = Cursor::new(&include_bytes!("tests/duplicate_dirs.tar")[..]);
        let mut ar = Archive::new(rdr);
        t!(ar.unpack(td.path()));

        let some_dir = td.path().join("some_dir");
        assert!(fs::metadata(&some_dir).map(|m| m.is_dir()).unwrap_or(false));
    }

    #[test]
    fn handling_incorrect_file_size() {
        let td = t!(TempDir::new("tar-rs"));

        let cur = Cursor::new(Vec::new());
        let ar = Archive::new(cur);

        let path = td.path().join("tmpfile");
        t!(File::create(&path));
        let mut file = t!(File::open(&path));
        let mut header = Header::new();
        t!(header.set_path("somepath"));
        header.set_metadata(&t!(file.metadata()));
        header.set_size(2048); // past the end of file null blocks
        header.set_cksum();
        t!(ar.append(&header, &mut file));
        t!(ar.finish());

        // Extracting
        let rdr = Cursor::new(ar.into_inner().into_inner());
        let mut ar = Archive::new(rdr);
        assert!(ar.unpack(td.path()).is_err());

        // Iterating
        let rdr = Cursor::new(ar.into_inner().into_inner());
        let mut ar = Archive::new(rdr);
        assert!(t!(ar.files_mut()).any(|fr| fr.is_err()));
    }

    #[test]
    fn extracting_malicious_tarball() {
        use std::fs;
        use std::fs::OpenOptions;
        use std::io::{Seek, Write};

        let td = t!(TempDir::new("tar-rs"));

        let mut evil_tar = Cursor::new(Vec::new());

        {
            let a = Archive::new(&mut evil_tar);
            let mut evil_txt_f = t!(OpenOptions::new().read(true).write(true)
                                                .create(true)
                                                .open(td.path().join("evil.txt")));
            t!(writeln!(evil_txt_f, "This is an evil file."));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file("/tmp/abs_evil.txt", &mut evil_txt_f));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file("//tmp/abs_evil2.txt", &mut evil_txt_f));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file("///tmp/abs_evil3.txt", &mut evil_txt_f));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file("/./tmp/abs_evil4.txt", &mut evil_txt_f));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file("//./tmp/abs_evil5.txt", &mut evil_txt_f));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file("///./tmp/abs_evil6.txt", &mut evil_txt_f));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file("/../tmp/rel_evil.txt", &mut evil_txt_f));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file("../rel_evil2.txt", &mut evil_txt_f));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file("./../rel_evil3.txt", &mut evil_txt_f));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file("some/../../rel_evil4.txt", &mut evil_txt_f));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file("", &mut evil_txt_f));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file("././//./", &mut evil_txt_f));
            t!(evil_txt_f.seek(SeekFrom::Start(0)));
            t!(a.append_file(".", &mut evil_txt_f));
            t!(a.finish());
        }

        t!(evil_tar.seek(SeekFrom::Start(0)));
        let mut ar = Archive::new(&mut evil_tar);
        t!(ar.unpack(td.path()));

        assert!(fs::metadata("/tmp/abs_evil.txt").is_err());
        assert!(fs::metadata("/tmp/abs_evil.txt2").is_err());
        assert!(fs::metadata("/tmp/abs_evil.txt3").is_err());
        assert!(fs::metadata("/tmp/abs_evil.txt4").is_err());
        assert!(fs::metadata("/tmp/abs_evil.txt5").is_err());
        assert!(fs::metadata("/tmp/abs_evil.txt6").is_err());
        assert!(fs::metadata("/tmp/rel_evil.txt").is_err());
        assert!(fs::metadata("/tmp/rel_evil.txt").is_err());
        assert!(fs::metadata(td.path().join("../tmp/rel_evil.txt")).is_err());
        assert!(fs::metadata(td.path().join("../rel_evil2.txt")).is_err());
        assert!(fs::metadata(td.path().join("../rel_evil3.txt")).is_err());
        assert!(fs::metadata(td.path().join("../rel_evil4.txt")).is_err());

        // The `some` subdirectory should not be created because the only
        // filename that references this has '..'.
        assert!(fs::metadata(td.path().join("some")).is_err());

        // The `tmp` subdirectory should be created and within this
        // subdirectory, there should be files named `abs_evil.txt` through
        // `abs_evil6.txt`.
        assert!(fs::metadata(td.path().join("tmp")).map(|m| m.is_dir())
                   .unwrap_or(false));
        assert!(fs::metadata(td.path().join("tmp/abs_evil.txt"))
                   .map(|m| m.is_file()).unwrap_or(false));
        assert!(fs::metadata(td.path().join("tmp/abs_evil2.txt"))
                   .map(|m| m.is_file()).unwrap_or(false));
        assert!(fs::metadata(td.path().join("tmp/abs_evil3.txt"))
                   .map(|m| m.is_file()).unwrap_or(false));
        assert!(fs::metadata(td.path().join("tmp/abs_evil4.txt"))
                   .map(|m| m.is_file()).unwrap_or(false));
        assert!(fs::metadata(td.path().join("tmp/abs_evil5.txt"))
                   .map(|m| m.is_file()).unwrap_or(false));
        assert!(fs::metadata(td.path().join("tmp/abs_evil6.txt"))
                   .map(|m| m.is_file()).unwrap_or(false));
    }

    #[test]
    fn octal_spaces() {
        let rdr = Cursor::new(&include_bytes!("tests/spaces.tar")[..]);
        let ar = Archive::new(rdr);

        let file = ar.files().unwrap().next().unwrap().unwrap();
        assert_eq!(file.header().mode().unwrap() & 0o777, 0o777);
        assert_eq!(file.header().uid().unwrap(), 0);
        assert_eq!(file.header().gid().unwrap(), 0);
        assert_eq!(file.header().size().unwrap(), 2);
        assert_eq!(file.header().mtime().unwrap(), 0o12440016664);
        assert_eq!(file.header().cksum().unwrap(), 0o4253);
    }

    #[test]
    fn extracting_malformed_tar_null_blocks() {
        let td = t!(TempDir::new("tar-rs"));

        let cur = Cursor::new(Vec::new());
        let ar = Archive::new(cur);

        let path1 = td.path().join("tmpfile1");
        let path2 = td.path().join("tmpfile2");
        t!(File::create(&path1));
        t!(File::create(&path2));
        t!(ar.append_path(&path1));
        let mut wrtr = ar.into_inner();
        t!(wrtr.write_all(&[0; 512]));
        let ar = Archive::new(wrtr);
        t!(ar.append_path(&path2));
        t!(ar.finish());

        let rdr = Cursor::new(ar.into_inner().into_inner());
        let mut ar = Archive::new(rdr);
        assert!(ar.unpack(td.path()).is_err());
    }

    #[test]
    fn empty_filename()
    {
        let td = t!(TempDir::new("tar-rs"));
        let rdr = Cursor::new(&include_bytes!("tests/empty_filename.tar")[..]);
        let mut ar = Archive::new(rdr);
        assert!(ar.unpack(td.path()).is_err());
    }

    #[test]
    fn file_times() {
        let td = t!(TempDir::new("tar-rs"));
        let rdr = Cursor::new(&include_bytes!("tests/file_times.tar")[..]);
        let mut ar = Archive::new(rdr);
        t!(ar.unpack(td.path()));

        let meta = fs::metadata(td.path().join("a")).unwrap();
        let mtime = FileTime::from_last_modification_time(&meta);
        let atime = FileTime::from_last_access_time(&meta);
        assert_eq!(mtime.seconds_relative_to_1970(), 1000000000);
        assert_eq!(mtime.nanoseconds(), 0);
        assert_eq!(atime.seconds_relative_to_1970(), 1000000000);
        assert_eq!(atime.nanoseconds(), 0);
    }

    #[test]
    fn backslash_same_as_slash() {
        // Insert a file into an archive with a backslash
        let td = t!(TempDir::new("tar-rs"));
        let ar = Archive::new(Vec::<u8>::new());
        t!(ar.append_dir("foo\\bar", td.path()));
        ar.finish().unwrap();
        let ar = Archive::new(Cursor::new(ar.into_inner()));
        let f = t!(t!(ar.files()).next().unwrap());
        assert_eq!(&*f.header().path().unwrap(), Path::new("foo/bar"));

        // Unpack an archive with a backslash in the name
        let ar = Archive::new(Vec::<u8>::new());
        let mut header = Header::new();
        header.set_metadata(&t!(fs::metadata(td.path())));
        header.set_size(0);
        for (a, b) in header.name.iter_mut().zip(b"foo\\bar\x00") {
            *a = *b;
        }
        header.set_cksum();
        t!(ar.append(&header, &mut io::empty()));
        ar.finish().unwrap();
        let mut ar = Archive::new(Cursor::new(ar.into_inner()));
        {
            let f = t!(t!(ar.files()).next().unwrap());
            assert_eq!(&*f.header().path().unwrap(), Path::new("foo/bar"));
        }
        t!(ar.files()); // seek to 0
        t!(ar.unpack(td.path()));
        assert!(fs::metadata(td.path().join("foo/bar")).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn nul_bytes_in_path() {
        use std::os::unix::prelude::*;
        use std::ffi::OsStr;

        let nul_path = OsStr::from_bytes(b"foo\0");
        let td = t!(TempDir::new("tar-rs"));
        let ar = Archive::new(Vec::<u8>::new());
        let err = ar.append_dir(nul_path, td.path()).unwrap_err();
        assert!(err.to_string().contains("contained a nul byte"));
    }
}
