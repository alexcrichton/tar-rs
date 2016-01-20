use std::fs;
use std::io::prelude::*;
use std::io;
use std::marker;
use std::path::Path;

use filetime::{self, FileTime};

use {Header, Archive};
use archive::ArchiveInner;
use error::TarError;
use other;

/// A read-only view into an entry of an archive.
///
/// This structure is a window into a portion of a borrowed archive which can
/// be inspected. It acts as a file handle by implementing the Reader trait. An
/// entry cannot be rewritten once inserted into an archive.
pub struct Entry<'a, R: 'a + Read> {
    fields: EntryFields<'a>,
    _ignored: marker::PhantomData<&'a Archive<R>>,
}

// private implementation detail of `Entry`, but concrete (no type parameters)
// and also all-public to be constructed from other modules.
pub struct EntryFields<'a> {
    pub header: Header,
    pub size: u64,
    pub data: io::Take<&'a ArchiveInner<Read + 'a>>,
}

impl<'a, R: Read> Entry<'a, R> {
    /// Returns access to the header of this entry in the archive.
    ///
    /// This provides access to the the metadata for this entry in the archive.
    pub fn header(&self) -> &Header {
        &self.fields.header
    }

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
    /// let mut ar = Archive::new(File::open("foo.tar").unwrap());
    ///
    /// for (i, file) in ar.entries().unwrap().enumerate() {
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

impl<'a> EntryFields<'a> {
    pub fn into_entry<R: Read>(self) -> Entry<'a, R> {
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
                return Err(other("failed to write entire file"));
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
}

impl<'a> Read for EntryFields<'a> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        self.data.read(into)
    }
}
