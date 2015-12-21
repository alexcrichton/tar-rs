use std::cmp;
use std::fs;
use std::io::prelude::*;
use std::io::{self, SeekFrom};
use std::marker;
use std::path::Path;

use filetime::{self, FileTime};

use error::TarError;
use {Header, Archive};
use {bad_archive, other};

/// Backwards compatible alias for `Entry`.
#[doc(hidden)]
pub type File<'a, T> = Entry<'a, T>;

/// A read-only view into an entry of an archive.
///
/// This structure is a window into a portion of a borrowed archive which can
/// be inspected. It acts as a file handle by implementing the Reader and Seek
/// traits. An entry cannot be rewritten once inserted into an archive.
pub struct Entry<'a, R: 'a> {
    fields: EntryFields<'a>,
    _ignored: marker::PhantomData<&'a Archive<R>>,
}

// private implementation detail of `Entry`, but concrete (no type parameters)
// and also all-public to be constructed from other modules.
pub struct EntryFields<'a> {
    pub header: Header,
    pub archive: &'a Archive<Read + 'a>,
    pub pos: u64,
    pub size: u64,

    // Used in read() to make sure we're positioned at the next byte. For a
    // `Entries` iterator these are meaningful while for a `EntriesMut` iterator
    // these are both unused/noops.
    pub seek: Box<Fn(&EntryFields) -> io::Result<()> + 'a>,
    pub tar_offset: u64,
}

impl<'a, R: Read> Entry<'a, R> {
    /// Returns access to the header of this entry in the archive.
    ///
    /// This provides access to the the metadata for this entry in the archive.
    pub fn header(&self) -> &Header { &self.fields.header }

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
        self.fields._unpack(dst.as_ref())
    }
}

impl<'a, R: Read> Read for Entry<'a, R> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        self.fields.read(into)
    }
}

impl<'a, R: Read + Seek> Seek for Entry<'a, R> {
    fn seek(&mut self, how: SeekFrom) -> io::Result<u64> {
        self.fields._seek(how)
    }
}

impl<'a> EntryFields<'a> {
    pub fn into_entry<R>(self) -> Entry<'a, R> {
        Entry {
            fields: self,
            _ignored: marker::PhantomData,
        }
    }

    fn _unpack(&mut self, dst: &Path) -> io::Result<()> {
        let kind = self.header.entry_type();
        if kind.is_dir() {
            // If the directory already exists just let it slide
            let prev = fs::metadata(&dst);
            if prev.map(|m| m.is_dir()).unwrap_or(false) {
                return Ok(())
            }
            return fs::create_dir(&dst)
        } else if kind.is_hard_link() || kind.is_symlink() {
            let src = match try!(self.header.link_name()) {
                Some(name) => name,
                None => return Err(other("hard link listed but no link \
                                          name found"))
            };

            return if kind.is_hard_link() {
                fs::hard_link(&src, dst)
            } else {
                symlink(&src, dst)
            };

            #[cfg(windows)]
            fn symlink(src: &Path, dst: &Path) -> io::Result<()> {
                ::std::os::windows::fs::symlink_file(src, dst)
            }
            #[cfg(unix)]
            fn symlink(src: &Path, dst: &Path) -> io::Result<()> {
                ::std::os::unix::fs::symlink(src, dst)
            }
        } else if !kind.is_file() {
            // Right now we can only otherwise handle regular files
            return Err(other(&format!("unknown file type 0x{:x}",
                                      kind.as_byte())))
        };

        try!(fs::File::create(dst).and_then(|mut f| {
            if try!(io::copy(self, &mut f)) != self.size {
                return Err(bad_archive());
            }
            Ok(())
        }).map_err(|e| {
            let header = self.header.path_bytes();
            TarError::new(&format!("failed to unpack `{}` into `{}`",
                                   String::from_utf8_lossy(&header),
                                   dst.display()), e)
        }));

        if let Ok(mtime) = self.header.mtime() {
            let mtime = FileTime::from_seconds_since_1970(mtime, 0);
            try!(filetime::set_file_times(dst, mtime, mtime).map_err(|e| {
                TarError::new(&format!("failed to set mtime for `{}`",
                                       dst.display()), e)
            }));
        }
        if let Ok(mode) = self.header.mode() {
            try!(set_perms(dst, mode).map_err(|e| {
                TarError::new(&format!("failed to set permissions to {:o} \
                                        for `{}`", mode, dst.display()), e)
            }));
        }
        return Ok(());

        #[cfg(unix)]
        fn set_perms(dst: &Path, mode: u32) -> io::Result<()> {
            use std::os::unix::raw;
            use std::os::unix::prelude::*;

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

    fn _seek(&mut self, how: SeekFrom) -> io::Result<u64> {
        let next = match how {
            SeekFrom::Start(pos) => pos as i64,
            SeekFrom::Current(pos) => self.pos as i64 + pos,
            SeekFrom::End(pos) => self.size as i64 + pos,
        };
        if next < 0 {
            Err(other("cannot seek before position 0"))
        } else if next as u64 > self.size {
            Err(other("cannot seek past end of file"))
        } else {
            self.pos = next as u64;
            Ok(self.pos)
        }
    }
}

impl<'a> Read for EntryFields<'a> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        if self.size == self.pos { return Ok(0) }

        try!((self.seek)(self));
        let amt = cmp::min((self.size - self.pos) as usize, into.len());
        let amt = try!(Read::read(&mut self.archive, &mut into[..amt]));
        self.pos += amt as u64;
        Ok(amt)
    }
}
