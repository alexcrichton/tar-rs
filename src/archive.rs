use std::cell::{RefCell, Cell};
use std::cmp;
use std::fs;
use std::io::prelude::*;
use std::io;
use std::marker;
use std::path::{Path, Component};

use entry::{EntryFields, EntryIo};
use error::TarError;
use other;
use {Entry, Header, GnuSparseHeader, GnuExtSparseHeader};

/// A top-level representation of an archive file.
///
/// This archive can have an entry added to it and it can be iterated over.
pub struct Archive<R: ?Sized + Read> {
    unpack_xattrs: bool,
    inner: ArchiveInner<R>,
}

pub struct ArchiveInner<R: ?Sized> {
    pos: Cell<u64>,
    obj: RefCell<::AlignHigher<R>>,
}

/// An iterator over the entries of an archive.
pub struct Entries<'a, R: 'a + Read> {
    fields: EntriesFields<'a>,
    _ignored: marker::PhantomData<&'a Archive<R>>,
}

struct EntriesFields<'a> {
    archive: &'a Archive<Read + 'a>,
    next: u64,
    done: bool,
    raw: bool,
}

impl<R: Read> Archive<R> {
    /// Create a new archive with the underlying object as the reader.
    pub fn new(obj: R) -> Archive<R> {
        Archive {
            unpack_xattrs: false,
            inner: ArchiveInner {
                obj: RefCell::new(::AlignHigher(0, obj)),
                pos: Cell::new(0),
            },
        }
    }

    /// Unwrap this archive, returning the underlying object.
    pub fn into_inner(self) -> R {
        self.inner.obj.into_inner().1
    }

    /// Construct an iterator over the entries in this archive.
    ///
    /// Note that care must be taken to consider each entry within an archive in
    /// sequence. If entries are processed out of sequence (from what the
    /// iterator returns), then the contents read for each entry may be
    /// corrupted.
    pub fn entries(&mut self) -> io::Result<Entries<R>> {
        let me: &mut Archive<Read> = self;
        me._entries().map(|fields| {
            Entries { fields: fields, _ignored: marker::PhantomData }
        })
    }

    /// Unpacks the contents tarball into the specified `dst`.
    ///
    /// This function will iterate over the entire contents of this tarball,
    /// extracting each file in turn to the location specified by the entry's
    /// path name.
    ///
    /// This operation is relatively sensitive in that it will not write files
    /// outside of the path specified by `dst`. Files in the archive which have
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
        let me: &mut Archive<Read> = self;
        me._unpack(dst.as_ref())
    }

    /// Sets the flag determining the behavior of unpack(). If flag is set to
    /// true, unpack() will preserve extended file attributes (xattrs).
    #[cfg(unix)]
    pub fn set_unpack_xattrs(&mut self, unpack_xattrs: bool) {
        self.unpack_xattrs = unpack_xattrs;
    }

}

impl<'a> Archive<Read + 'a> {
    fn _entries(&mut self) -> io::Result<EntriesFields> {
        if self.inner.pos.get() != 0 {
            return Err(other("cannot call entries unless archive is at \
                              position 0"))
        }
        Ok(EntriesFields {
            archive: self,
            done: false,
            next: 0,
            raw: false,
        })
    }

    fn _unpack(&mut self, dst: &Path) -> io::Result<()> {
        let unpack_xattrs = self.unpack_xattrs;
        'outer: for entry in try!(self._entries()) {
            let mut file = try!(entry.map_err(|e| {
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
                let path = try!(file.path().map_err(|e| {
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

            if let Some(parent) = file_dst.parent() {
                try!(fs::create_dir_all(&parent).map_err(|e| {
                    TarError::new(&format!("failed to create `{}`",
                                           parent.display()), e)
                }));
            }
            file.set_unpack_xattrs(unpack_xattrs);
            try!(file.unpack(&file_dst).map_err(|e| {
                TarError::new(&format!("failed to unpack `{}`",
                                       file_dst.display()), e)
            }));
        }
        Ok(())
    }

    fn skip(&self, mut amt: u64) -> io::Result<()> {
        let mut buf = [0u8; 4096 * 8];
        while amt > 0 {
            let n = cmp::min(amt, buf.len() as u64);
            let n = try!((&self.inner).read(&mut buf[..n as usize]));
            if n == 0 {
                return Err(other("unexpected EOF during skip"))
            }
            amt -= n as u64;
        }
        Ok(())
    }
}

impl<'a, R: Read> Entries<'a, R> {
    /// Indicates whether this iterator will return raw entries or not.
    ///
    /// If the raw list of entries are returned, then no preprocessing happens
    /// on account of this library, for example taking into accout GNU long name
    /// or long link archive members. Raw iteration is disabled by default.
    pub fn raw(self, raw: bool) -> Entries<'a, R> {
        Entries {
            fields: EntriesFields {
                raw: raw,
                ..self.fields
            },
            _ignored: marker::PhantomData,
        }
    }
}
impl<'a, R: Read> Iterator for Entries<'a, R> {
    type Item = io::Result<Entry<'a, R>>;

    fn next(&mut self) -> Option<io::Result<Entry<'a, R>>> {
        self.fields.next().map(|result| {
            result.map(|e| EntryFields::from(e).into_entry())
        })
    }
}

impl<'a> EntriesFields<'a> {
    fn next_entry_raw(&mut self) -> io::Result<Option<Entry<'a, io::Empty>>> {
        // Seek to the start of the next header in the archive
        let delta = self.next - self.archive.inner.pos.get();
        try!(self.archive.skip(delta));

        let mut header = Header::new_old();
        try!(read_all(&mut &self.archive.inner, header.as_mut_bytes()));
        self.next += 512;

        // If we have an all 0 block, then this should be the start of the end
        // of the archive. A block of 0s is never valid as a header (because of
        // the checksum), so if it's all zero it must be the first of the two
        // end blocks
        if header.as_bytes().iter().all(|i| *i == 0) {
            try!(read_all(&mut &self.archive.inner,
                                     header.as_mut_bytes()));
            self.next += 512;
            return if header.as_bytes().iter().all(|i| *i == 0) {
                Ok(None)
            } else {
                Err(other("found block of 0s not followed by a second \
                           block of 0s"))
            }
        }

        // Make sure the checksum is ok
        let sum = header.as_bytes()[..148].iter()
                        .chain(&header.as_bytes()[156..])
                        .fold(0, |a, b| a + (*b as u32)) + 8 * 32;
        let cksum = try!(header.cksum());
        if sum != cksum {
            return Err(other("archive header checksum mismatch"))
        }

        let size = try!(header.entry_size());

        let ret = EntryFields {
            size: size,
            data: vec![EntryIo::Data((&self.archive.inner).take(size))],
            header: header,
            long_pathname: None,
            long_linkname: None,
            pax_extensions: None,
        };

        // Store where the next entry is, rounding up by 512 bytes (the size of
        // a header);
        let size = (size + 511) & !(512 - 1);
        self.next += size;

        Ok(Some(ret.into_entry()))
    }

    fn next_entry(&mut self) -> io::Result<Option<Entry<'a, io::Empty>>> {
        if self.raw {
            return self.next_entry_raw()
        }

        let mut gnu_longname = None;
        let mut gnu_longlink = None;
        let mut pax_extensions = None;
        let mut processed = 0;
        loop {
            processed += 1;
            let entry = match try!(self.next_entry_raw()) {
                Some(entry) => entry,
                None if processed > 1 => {
                    return Err(other("members found describing a future member \
                                      but no future member found"))
                }
                None => return Ok(None),
            };

            if entry.header().as_gnu().is_some() &&
               entry.header().entry_type().is_gnu_longname() {
                if gnu_longname.is_some() {
                    return Err(other("two long name entries describing \
                                      the same member"))
                }
                gnu_longname = Some(try!(EntryFields::from(entry).read_all()));
                continue
            }

            if entry.header().as_gnu().is_some() &&
               entry.header().entry_type().is_gnu_longlink() {
                if gnu_longlink.is_some() {
                    return Err(other("two long name entries describing \
                                      the same member"))
                }
                gnu_longlink = Some(try!(EntryFields::from(entry).read_all()));
                continue
            }

            if entry.header().as_ustar().is_some() &&
               entry.header().entry_type().is_pax_local_extensions() {
                if pax_extensions.is_some() {
                    return Err(other("two pax extensions entries describing \
                                      the same member"))
                }
                pax_extensions = Some(try!(EntryFields::from(entry).read_all()));
                continue
            }

            let mut fields = EntryFields::from(entry);
            fields.long_pathname = gnu_longname;
            fields.long_linkname = gnu_longlink;
            fields.pax_extensions = pax_extensions;
            try!(self.parse_sparse_header(&mut fields));
            return Ok(Some(fields.into_entry()))
        }
    }

    fn parse_sparse_header(&mut self,
                           entry: &mut EntryFields<'a>) -> io::Result<()> {
        if !entry.header.entry_type().is_gnu_sparse() {
            return Ok(())
        }
        let gnu = match entry.header.as_gnu() {
            Some(gnu) => gnu,
            None => return Err(other("sparse entry type listed but not GNU header")),
        };

        // Sparse files are represented internally as a list of blocks that are
        // read. Blocks are either a bunch of 0's or they're data from the
        // underlying archive.
        //
        // Blocks of a sparse file are described by the `GnuSparseHeader`
        // structure, some of which are contained in `GnuHeader` but some of
        // which may also be contained after the first header in further
        // headers.
        //
        // We read off all the blocks here and use the `add_block` function to
        // incrementally add them to the list of I/O block (in `entry.data`).
        // The `add_block` function also validates that each chunk comes after
        // the previous, we don't overrun the end of the file, and each block is
        // aligned to a 512-byte boundary in the archive itself.
        //
        // At the end we verify that the sparse file size (`Header::size`) is
        // the same as the current offset (described by the list of blocks) as
        // well as the amount of data read equals the size of the entry
        // (`Header::entry_size`).
        entry.data.truncate(0);

        let mut cur = 0;
        let mut remaining = entry.size;
        {
            let data = &mut entry.data;
            let reader = &self.archive.inner;
            let size = entry.size;
            let mut add_block = |block: &GnuSparseHeader| -> io::Result<_> {
                if block.is_empty() {
                    return Ok(())
                }
                let off = try!(block.offset());
                let len = try!(block.length());

                if (size - remaining) % 512 != 0 {
                    return Err(other("previous block in sparse file was not \
                                      aligned to 512-byte boundary"))
                } else if off < cur {
                    return Err(other("out of order or overlapping sparse \
                                      blocks"))
                } else if cur < off {
                    let block = io::repeat(0).take(off - cur);
                    data.push(EntryIo::Pad(block));
                }
                cur = try!(off.checked_add(len).ok_or_else(|| {
                    other("more bytes listed in sparse file than u64 can hold")
                }));
                remaining = try!(remaining.checked_sub(len).ok_or_else(|| {
                    other("sparse file consumed more data than the header \
                           listed")
                }));
                data.push(EntryIo::Data(reader.take(len)));
                Ok(())
            };
            for block in gnu.sparse.iter() {
                try!(add_block(block))
            }
            if gnu.is_extended() {
                let mut ext = GnuExtSparseHeader::new();
                ext.isextended[0] = 1;
                while ext.is_extended() {
                    try!(read_all(&mut &self.archive.inner, ext.as_mut_bytes()));
                    self.next += 512;
                    for block in ext.sparse.iter() {
                        try!(add_block(block));
                    }
                }
            }
        }
        if cur != try!(gnu.real_size()) {
            return Err(other("mismatch in sparse file chunks and \
                              size in header"))
        }
        entry.size = cur;
        if remaining > 0 {
            return Err(other("mismatch in sparse file chunks and \
                              entry size in header"))
        }
        Ok(())
    }
}

impl<'a> Iterator for EntriesFields<'a> {
    type Item = io::Result<Entry<'a, io::Empty>>;

    fn next(&mut self) -> Option<io::Result<Entry<'a, io::Empty>>> {
        if self.done {
            None
        } else {
            match self.next_entry() {
                Ok(Some(e)) => Some(Ok(e)),
                Ok(None) => {
                    self.done = true;
                    None
                }
                Err(e) => {
                    self.done = true;
                    Some(Err(e))
                }
            }
        }
    }
}

impl<'a, R: ?Sized + Read> Read for &'a ArchiveInner<R> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        self.obj.borrow_mut().read(into).map(|i| {
            self.pos.set(self.pos.get() + i as u64);
            i
        })
    }
}

fn read_all<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<()> {
    let mut read = 0;
    while read < buf.len() {
        match try!(r.read(&mut buf[read..])) {
            0 => return Err(other("failed to read entire block")),
            n => read += n,
        }
    }
    Ok(())
}
