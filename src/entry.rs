use std::borrow::Cow;
use std::cmp;
use std::fs;
use std::io::prelude::*;
use std::io::{self, SeekFrom};
use std::marker;
use std::path::{Component, Path, PathBuf};

use filetime::{self, FileTime};

use {Header, Archive, PaxExtensions};
use archive::ArchiveInner;
use error::TarError;
use header::bytes2path;
use other;
use pax::pax_extensions;

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
    pub long_pathname: Option<Vec<u8>>,
    pub long_linkname: Option<Vec<u8>>,
    pub pax_extensions: Option<Vec<u8>>,
    pub header: Header,
    pub size: u64,
    pub data: Vec<EntryIo<'a>>,
    pub unpack_xattrs: bool,
    pub preserve_permissions: bool,
}

pub enum EntryIo<'a> {
    Pad(io::Take<io::Repeat>),
    Data(io::Take<&'a ArchiveInner<Read + 'a>>),
}

impl<'a, R: Read> Entry<'a, R> {
    /// Returns the path name for this entry.
    ///
    /// This method may fail if the pathname is not valid unicode and this is
    /// called on a Windows platform.
    ///
    /// Note that this function will convert any `\` characters to directory
    /// separators, and it will not always return the same value as
    /// `self.header().path()` as some archive formats have support for longer
    /// path names described in separate entries.
    ///
    /// It is recommended to use this method instead of inspecting the `header`
    /// directly to ensure that various archive formats are handled correctly.
    pub fn path(&self) -> io::Result<Cow<Path>> {
        self.fields.path()
    }

    /// Returns the raw bytes listed for this entry.
    ///
    /// Note that this function will convert any `\` characters to directory
    /// separators, and it will not always return the same value as
    /// `self.header().path_bytes()` as some archive formats have support for
    /// longer path names described in separate entries.
    pub fn path_bytes(&self) -> Cow<[u8]> {
        self.fields.path_bytes()
    }

    /// Returns the link name for this entry, if any is found.
    ///
    /// This method may fail if the pathname is not valid unicode and this is
    /// called on a Windows platform. `Ok(None)` being returned, however,
    /// indicates that the link name was not present.
    ///
    /// Note that this function will convert any `\` characters to directory
    /// separators, and it will not always return the same value as
    /// `self.header().link_name()` as some archive formats have support for
    /// longer path names described in separate entries.
    ///
    /// It is recommended to use this method instead of inspecting the `header`
    /// directly to ensure that various archive formats are handled correctly.
    pub fn link_name(&self) -> io::Result<Option<Cow<Path>>> {
        self.fields.link_name()
    }

    /// Returns the link name for this entry, in bytes, if listed.
    ///
    /// Note that this will not always return the same value as
    /// `self.header().link_name_bytes()` as some archive formats have support for
    /// longer path names described in separate entries.
    pub fn link_name_bytes(&self) -> Option<Cow<[u8]>> {
        self.fields.link_name_bytes()
    }

    /// Returns an iterator over the pax extensions contained in this entry.
    ///
    /// Pax extensions are a form of archive where extra metadata is stored in
    /// key/value pairs in entries before the entry they're intended to
    /// describe. For example this can be used to describe long file name or
    /// other metadata like atime/ctime/mtime in more precision.
    ///
    /// The returned iterator will yield key/value pairs for each extension.
    ///
    /// `None` will be returned if this entry does not indicate that it itself
    /// contains extensions, or if there were no previous extensions describing
    /// it.
    ///
    /// Note that global pax extensions are intended to be applied to all
    /// archive entries.
    ///
    /// Also note that this function will read the entire entry if the entry
    /// itself is a list of extensions.
    pub fn pax_extensions(&mut self) -> io::Result<Option<PaxExtensions>> {
        self.fields.pax_extensions()
    }

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
    /// > **Note**: This function does not have as many sanity checks as
    /// > `Archive::unpack` or `Entry::unpack_in`. As a result if you're
    /// > thinking of unpacking untrusted tarballs you may want to review the
    /// > implementations of the previous two functions and perhaps implement
    /// > similar logic yourself.
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
        self.fields.unpack(dst.as_ref(), None)
    }

    /// Extracts this file under the specified path, avoiding security issues.
    ///
    /// This function will write the entire contents of this file into the
    /// location obtained by appending the path of this file in the archive to
    /// `dst`, creating any intermediate directories if needed. Metadata will
    /// also be propagated to the path `dst`. Any existing file at the location
    /// `dst` will be overwritten.
    ///
    /// This function carefully avoids writing outside of `dst`. If the file has
    /// a '..' in its path, this function will skip it and return false.
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
    ///     file.unpack_in("target").unwrap();
    /// }
    /// ```
    pub fn unpack_in<P: AsRef<Path>>(&mut self, dst: P) -> io::Result<bool> {
        self.fields.unpack_in(dst.as_ref())
    }

    /// Indicate whether extended file attributes (xattrs on Unix) are preserved
    /// when unpacking this entry.
    ///
    /// This flag is disabled by default and is currently only implemented on
    /// Unix using xattr support. This may eventually be implemented for
    /// Windows, however, if other archive implementations are found which do
    /// this as well.
    pub fn set_unpack_xattrs(&mut self, unpack_xattrs: bool) {
        self.fields.unpack_xattrs = unpack_xattrs;
    }

    /// Indicate whether extended permissions (like suid on Unix) are preserved
    /// when unpacking this entry.
    ///
    /// This flag is disabled by default and is currently only implemented on
    /// Unix.
    pub fn set_preserve_permissions(&mut self, preserve: bool) {
        self.fields.preserve_permissions = preserve;
    }
}

impl<'a, R: Read> Read for Entry<'a, R> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        self.fields.read(into)
    }
}

impl<'a> EntryFields<'a> {
    pub fn from<R: Read>(entry: Entry<R>) -> EntryFields {
        entry.fields
    }

    pub fn into_entry<R: Read>(self) -> Entry<'a, R> {
        Entry {
            fields: self,
            _ignored: marker::PhantomData,
        }
    }

    pub fn read_all(&mut self) -> io::Result<Vec<u8>> {
        // Preallocate some data but don't let ourselves get too crazy now.
        let cap = cmp::min(self.size, 128 * 1024);
        let mut v = Vec::with_capacity(cap as usize);
        self.read_to_end(&mut v).map(|_| v)
    }

    fn path(&self) -> io::Result<Cow<Path>> {
        bytes2path(self.path_bytes())
    }

    fn path_bytes(&self) -> Cow<[u8]> {
        match self.long_pathname {
            Some(ref bytes) => {
                if let Some(&0) = bytes.last() {
                    Cow::Borrowed(&bytes[..bytes.len() - 1])
                } else {
                    Cow::Borrowed(bytes)
                }
            }
            None => self.header.path_bytes(),
        }
    }

    fn link_name(&self) -> io::Result<Option<Cow<Path>>> {
        match self.link_name_bytes() {
            Some(bytes) => bytes2path(bytes).map(Some),
            None => Ok(None),
        }
    }

    fn link_name_bytes(&self) -> Option<Cow<[u8]>> {
        match self.long_linkname {
            Some(ref bytes) => {
                if let Some(&0) = bytes.last() {
                    Some(Cow::Borrowed(&bytes[..bytes.len() - 1]))
                } else {
                    Some(Cow::Borrowed(bytes))
                }
            }
            None => self.header.link_name_bytes(),
        }
    }

    fn pax_extensions(&mut self) -> io::Result<Option<PaxExtensions>> {
        if self.pax_extensions.is_none() {
            if !self.header.entry_type().is_pax_global_extensions() &&
               !self.header.entry_type().is_pax_local_extensions() {
                return Ok(None)
            }
            self.pax_extensions = Some(try!(self.read_all()));
        }
        Ok(Some(pax_extensions(self.pax_extensions.as_ref().unwrap())))
    }

    fn unpack_in(&mut self, dst: &Path) -> io::Result<bool> {
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
            let path = try!(self.path().map_err(|e| {
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
                    Component::ParentDir => return Ok(false),

                    Component::Normal(part) => file_dst.push(part),
                }
            }
        }

        // Skip cases where only slashes or '.' parts were seen, because
        // this is effectively an empty filename.
        if *dst == *file_dst {
            return Ok(true);
        }

        if let Some(parent) = file_dst.parent() {
            try!(fs::create_dir_all(&parent).map_err(|e| {
                TarError::new(&format!("failed to create `{}`",
                                       parent.display()), e)
            }));
        }
        try!(self.unpack(&file_dst, Some(&dst)).map_err(|e| {
            TarError::new(&format!("failed to unpack `{}`",
                                   file_dst.display()), e)
        }));

        Ok(true)
    }

    /// Returns access to the header of this entry in the archive.
    fn unpack(&mut self,
              dst: &Path,
              root: Option<&Path>) -> io::Result<()> {
        let kind = self.header.entry_type();
        if kind.is_dir() {
            // If the directory already exists just let it slide
            let prev = fs::metadata(&dst);
            if prev.map(|m| m.is_dir()).unwrap_or(false) {
                return Ok(())
            }
            return fs::create_dir(&dst)
        } else if kind.is_hard_link() || kind.is_symlink() {
            let src = match try!(self.link_name()) {
                Some(name) => name,
                None => return Err(other("hard link listed but no link \
                                          name found"))
            };

            // Ok, we're going to try to create a symlink. We need to protect
            // against symlinks which point outside the destination root
            // directory, however. Otherwise it could be possible to
            // accidentally write files outside there with malformed tarballs.
            //
            // To do that we take a look at the link name for this target, `src`
            // above. We then recreate the target that we're actually going to
            // link to, `actual_src`. Root directories and the current directory
            // are skipped (like `unpack_in` above) and `..` is allowed, but
            // only if it doesn't escape the root directory. This should allow
            // for relative symlinks within the destination but disallow
            // symlinks that point outside.
            let mut target = dst.to_path_buf();
            target.pop();
            let mut actual_src = PathBuf::new();
            for part in src.components() {
                match part {
                    Component::Prefix(..) |
                    Component::RootDir |
                    Component::CurDir => continue,
                    Component::ParentDir => {
                        actual_src.push("..");
                        if !target.pop() {
                            return Err(other("symlink destination points \
                                              outside unpack destination"))
                        }
                        if let Some(root) = root {
                            if !target.starts_with(root) {
                                return Err(other("symlink destination points \
                                                  outside unpack destination"))
                            }
                        }
                    }
                    Component::Normal(part) => {
                        target.push(part);
                        actual_src.push(part);
                    }
                }
            }
            if actual_src.iter().count() == 0 {
                return Err(other("symlink destination is empty"))
            }

            println!("{:?} {:?}", actual_src, dst);
            return if kind.is_hard_link() {
                fs::hard_link(&actual_src, dst)
            } else {
                symlink(&actual_src, dst)
            };

            #[cfg(windows)]
            fn symlink(src: &Path, dst: &Path) -> io::Result<()> {
                ::std::os::windows::fs::symlink_file(src, dst)
            }
            #[cfg(unix)]
            fn symlink(src: &Path, dst: &Path) -> io::Result<()> {
                ::std::os::unix::fs::symlink(src, dst)
            }
        } else if kind.is_pax_global_extensions() ||
                  kind.is_pax_local_extensions() ||
                  kind.is_gnu_longname() ||
                  kind.is_gnu_longlink() {
            return Ok(())
        };

        // Note the lack of `else` clause above. According to the FreeBSD
        // documentation:
        //
        // > A POSIX-compliant implementation must treat any unrecognized
        // > typeflag value as a regular file.
        //
        // As a result if we don't recognize the kind we just write out the file
        // as we would normally.

        try!(fs::File::create(dst).and_then(|mut f| {
            for io in self.data.drain(..) {
                match io {
                    EntryIo::Data(mut d) => {
                        let expected = d.limit();
                        if try!(io::copy(&mut d, &mut f)) != expected {
                            return Err(other("failed to write entire file"));
                        }
                    }
                    EntryIo::Pad(d) => {
                        // TODO: checked cast to i64
                        let to = SeekFrom::Current(d.limit() as i64);
                        let size = try!(f.seek(to));
                        try!(f.set_len(size));
                    }
                }
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
            try!(set_perms(dst, mode, self.preserve_permissions).map_err(|e| {
                TarError::new(&format!("failed to set permissions to {:o} \
                                        for `{}`", mode, dst.display()), e)
            }));
        }
        if self.unpack_xattrs {
            try!(set_xattrs(self, dst));
        }
        return Ok(());

        #[cfg(unix)]
        #[allow(deprecated)] // raw deprecated in 1.8
        fn set_perms(dst: &Path, mode: u32, preserve: bool) -> io::Result<()> {
            use std::os::unix::raw;
            use std::os::unix::prelude::*;

            let mode = if preserve {
                mode
            } else {
                mode & 0o777
            };

            let perm = fs::Permissions::from_mode(mode as raw::mode_t);
            fs::set_permissions(dst, perm)
        }
        #[cfg(windows)]
        fn set_perms(dst: &Path, mode: u32, _preserve: bool) -> io::Result<()> {
            let mut perm = try!(fs::metadata(dst)).permissions();
            perm.set_readonly(mode & 0o200 != 0o200);
            fs::set_permissions(dst, perm)
        }

        #[cfg(all(unix, feature = "xattr"))]
        fn set_xattrs(me: &mut EntryFields, dst: &Path) -> io::Result<()> {
            use std::os::unix::prelude::*;
            use std::ffi::OsStr;
            use xattr;

            let exts = match me.pax_extensions() {
                Ok(Some(e)) => e,
                _ => return Ok(()),
            };
            let exts = exts.filter_map(|e| e.ok()).filter_map(|e| {
                let key = e.key_bytes();
                let prefix = b"SCHILY.xattr.";
                if key.starts_with(prefix) {
                    Some((&key[prefix.len()..], e))
                } else {
                    None
                }
            }).map(|(key, e)| {
                (OsStr::from_bytes(key), e.value_bytes())
            });

            for (key, value) in exts {
                try!(xattr::set(dst, key, value).map_err(|e| {
                    TarError::new(&format!("failed to set extended \
                                            attributes to {}. \
                                            Xattrs: key={:?}, value={:?}.",
                                           dst.display(),
                                           key,
                                           String::from_utf8_lossy(value)),
                                  e)
                }));
            }

            Ok(())
        }
        // Windows does not completely support posix xattrs
        // https://en.wikipedia.org/wiki/Extended_file_attributes#Windows_NT
        #[cfg(any(windows, not(feature = "xattr")))]
        fn set_xattrs(_: &mut EntryFields, _: &Path) -> io::Result<()> {
            Ok(())
        }
    }
}

impl<'a> Read for EntryFields<'a> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        loop {
            match self.data.get_mut(0).map(|io| io.read(into)) {
                Some(Ok(0)) => { self.data.remove(0); }
                Some(r) => return r,
                None => return Ok(0),
            }
        }
    }
}

impl<'a> Read for EntryIo<'a> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        match *self {
            EntryIo::Pad(ref mut io) => io.read(into),
            EntryIo::Data(ref mut io) => io.read(into),
        }
    }
}
