use std::io;
use std::path::Path;
use std::io::prelude::*;
use std::fs;
use std::borrow::Cow;

use {EntryType, Header, other};
use header::{bytes2path, HeaderMode, path2bytes};

/// A structure for building archives
///
/// This structure has methods for building up an archive from scratch into any
/// arbitrary writer.
pub struct Builder<W: Write> {
    mode: HeaderMode,
    finished: bool,
    obj: Option<W>,
}

impl<W: Write> Builder<W> {
    /// Create a new archive builder with the underlying object as the
    /// destination of all data written. The builder will use
    /// `HeaderMode::Complete` by default.
    pub fn new(obj: W) -> Builder<W> {
        Builder {
            mode: HeaderMode::Complete,
            finished: false,
            obj: Some(obj),
        }
    }

    fn inner(&mut self) -> &mut W {
        self.obj.as_mut().unwrap()
    }

    /// Changes the HeaderMode that will be used when reading fs Metadata for
    /// methods that implicitly read metadata for an input Path. Notably, this
    /// does _not_ apply to `append(Header)`.
    pub fn mode(&mut self, mode: HeaderMode) {
        self.mode = mode;
    }

    /// Unwrap this archive, returning the underlying object.
    ///
    /// This function will finish writing the archive if the `finish` function
    /// hasn't yet been called, returning any I/O error which happens during
    /// that operation.
    pub fn into_inner(mut self) -> io::Result<W> {
        if !self.finished {
            try!(self.finish());
        }
        Ok(self.obj.take().unwrap())
    }

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
    /// Also note that after all entries have been written to an archive the
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
    /// use tar::{Builder, Header};
    ///
    /// let mut header = Header::new_gnu();
    /// header.set_path("foo");
    /// header.set_size(4);
    /// header.set_cksum();
    ///
    /// let mut data: &[u8] = &[1, 2, 3, 4];
    ///
    /// let mut ar = Builder::new(Vec::new());
    /// ar.append(&header, data).unwrap();
    /// let data = ar.into_inner().unwrap();
    /// ```
    pub fn append<R: Read>(&mut self, header: &Header, mut data: R)
                           -> io::Result<()> {
        append(self.inner(), header, &mut data)
    }

    /// Adds a new entry to this archive with the specified path.
    ///
    /// This function will set the specified path in the given header, which may
    /// require appending a GNU long-name extension entry to the archive first.
    /// The checksum for the header will be automatically updated via the
    /// `set_cksum` method after setting the path. No other metadata in the
    /// header will be modified.
    ///
    /// Then it will append the header, followed by contents of the stream
    /// specified by `data`. To produce a valid archive the `size` field of
    /// `header` must be the same as the length of the stream that's being
    /// written.
    ///
    /// Note that this will not attempt to seek the archive to a valid position,
    /// so if the archive is in the middle of a read or some other similar
    /// operation then this may corrupt the archive.
    ///
    /// Also note that after all entries have been written to an archive the
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
    /// use tar::{Builder, Header};
    ///
    /// let mut header = Header::new_gnu();
    /// header.set_size(4);
    /// header.set_cksum();
    ///
    /// let mut data: &[u8] = &[1, 2, 3, 4];
    ///
    /// let mut ar = Builder::new(Vec::new());
    /// ar.append_data(&mut header, "really/long/path/to/foo", data).unwrap();
    /// let data = ar.into_inner().unwrap();
    /// ```
    pub fn append_data<P: AsRef<Path>, R: Read>(&mut self, header: &mut Header, path: P, data: R)
                                                -> io::Result<()> {
        try!(prepare_header(self.inner(), header, path.as_ref()));
        header.set_cksum();
        self.append(&header, data)
    }

    /// Adds a file on the local filesystem to this archive.
    ///
    /// This function will open the file specified by `path` and insert the file
    /// into the archive with the appropriate metadata set, returning any I/O
    /// error which occurs while writing. The path name for the file inside of
    /// this archive will be the same as `path`, and it is required that the
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
    /// use tar::Builder;
    ///
    /// let mut ar = Builder::new(Vec::new());
    ///
    /// ar.append_path("foo/bar.txt").unwrap();
    /// ```
    pub fn append_path<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        let mode = self.mode.clone();
        append_path(self.inner(), path.as_ref(), mode)
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
    /// use tar::Builder;
    ///
    /// let mut ar = Builder::new(Vec::new());
    ///
    /// // Open the file at one location, but insert it into the archive with a
    /// // different name.
    /// let mut f = File::open("foo/bar/baz.txt").unwrap();
    /// ar.append_file("bar/baz.txt", &mut f).unwrap();
    /// ```
    pub fn append_file<P: AsRef<Path>>(&mut self, path: P, file: &mut fs::File)
                                       -> io::Result<()> {
        let mode = self.mode.clone();
        append_file(self.inner(), path.as_ref(), file, mode)
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
    /// use tar::Builder;
    ///
    /// let mut ar = Builder::new(Vec::new());
    ///
    /// // Use the directory at one location, but insert it into the archive
    /// // with a different name.
    /// ar.append_dir("bardir", ".").unwrap();
    /// ```
    pub fn append_dir<P, Q>(&mut self, path: P, src_path: Q) -> io::Result<()>
        where P: AsRef<Path>, Q: AsRef<Path>
    {
        let mode = self.mode.clone();
        append_dir(self.inner(), path.as_ref(), src_path.as_ref(), mode)
    }

    /// Adds a directory and all of its contents (recursively) to this archive
    /// with the given path as the name of the directory in the archive.
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
    /// use tar::Builder;
    ///
    /// let mut ar = Builder::new(Vec::new());
    ///
    /// // Use the directory at one location, but insert it into the archive
    /// // with a different name.
    /// ar.append_dir_all("bardir", ".").unwrap();
    /// ```
    pub fn append_dir_all<P, Q>(&mut self, path: P, src_path: Q) -> io::Result<()>
        where P: AsRef<Path>, Q: AsRef<Path>
    {
        let mode = self.mode.clone();
        append_dir_all(self.inner(), path.as_ref(), src_path.as_ref(), mode)
    }

    /// Finish writing this archive, emitting the termination sections.
    ///
    /// This function should only be called when the archive has been written
    /// entirely and if an I/O error happens the underlying object still needs
    /// to be acquired.
    ///
    /// In most situations the `into_inner` method should be preferred.
    pub fn finish(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(())
        }
        self.finished = true;
        self.inner().write_all(&[0; 1024])
    }
}

fn append(mut dst: &mut Write,
          header: &Header,
          mut data: &mut Read) -> io::Result<()> {
    try!(dst.write_all(header.as_bytes()));
    let len = try!(io::copy(&mut data, &mut dst));

    // Pad with zeros if necessary.
    let buf = [0; 512];
    let remaining = 512 - (len % 512);
    if remaining < 512 {
        try!(dst.write_all(&buf[..remaining as usize]));
    }

    Ok(())
}

fn append_path(dst: &mut Write, path: &Path, mode: HeaderMode) -> io::Result<()> {
    let stat = try!(fs::metadata(path));
    if stat.is_file() {
        append_fs(dst, path, &stat, &mut try!(fs::File::open(path)), mode)
    } else if stat.is_dir() {
        append_fs(dst, path, &stat, &mut io::empty(), mode)
    } else {
        Err(other("path has unknown file type"))
    }
}

fn append_file(dst: &mut Write, path: &Path, file: &mut fs::File, mode: HeaderMode)
                -> io::Result<()> {
    let stat = try!(file.metadata());
    append_fs(dst, path, &stat, file, mode)
}

fn append_dir(dst: &mut Write, path: &Path, src_path: &Path, mode: HeaderMode) -> io::Result<()> {
    let stat = try!(fs::metadata(src_path));
    append_fs(dst, path, &stat, &mut io::empty(), mode)
}

fn prepare_header(dst: &mut Write, header: &mut Header, path: &Path) -> io::Result<()> {
    // Try to encode the path directly in the header, but if it ends up not
    // working (e.g. it's too long) then use the GNU-specific long name
    // extension by emitting an entry which indicates that it's the filename
    if let Err(e) = header.set_path(path) {
        let data = try!(path2bytes(&path));
        let max = header.as_old().name.len();
        if data.len() < max {
            return Err(e)
        }
        let mut header2 = Header::new_gnu();
        header2.as_gnu_mut().unwrap().name[..13].clone_from_slice(b"././@LongLink");
        header2.set_mode(0o644);
        header2.set_uid(0);
        header2.set_gid(0);
        header2.set_mtime(0);
        header2.set_size((data.len() + 1) as u64);
        header2.set_entry_type(EntryType::new(b'L'));
        header2.set_cksum();
        let mut data2 = data.chain(io::repeat(0).take(0));
        try!(append(dst, &header2, &mut data2));
        // Truncate the path to store in the header we're about to emit to
        // ensure we've got something at least mentioned.
        let path = try!(bytes2path(Cow::Borrowed(&data[..max])));
        try!(header.set_path(&path));
    }
    Ok(())
}

fn append_fs(dst: &mut Write,
             path: &Path,
             meta: &fs::Metadata,
             read: &mut Read,
             mode: HeaderMode) -> io::Result<()> {
    let mut header = Header::new_gnu();

    try!(prepare_header(dst, &mut header, path));
    header.set_metadata_in_mode(meta, mode);
    header.set_cksum();
    append(dst, &header, read)
}

fn append_dir_all(dst: &mut Write, path: &Path, src_path: &Path, mode: HeaderMode) -> io::Result<()> {
    let mut stack = vec![(src_path.to_path_buf(), true)];
    while let Some((src, is_dir)) = stack.pop() {
        let dest = path.join(src.strip_prefix(&src_path).unwrap());
        if is_dir {
            for entry in try!(fs::read_dir(&src)) {
                let entry = try!(entry);
                stack.push((entry.path(), try!(entry.file_type()).is_dir()));
            }
            if dest != Path::new("") {
                try!(append_dir(dst, &dest, &src, mode));
            }
        } else {
            try!(append_file(dst, &dest, &mut try!(fs::File::open(src)), mode));
        }
    }
    Ok(())
}

impl<W: Write> Drop for Builder<W> {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}
