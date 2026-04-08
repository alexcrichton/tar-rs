use std::cell::{Cell, RefCell};
use std::cmp;
use std::convert::TryFrom;
use std::fs;
use std::io::prelude::*;
use std::io::{self, SeekFrom};
use std::marker;
use std::path::Path;

use tar_core::parse::{Limits, ParseError, ParseEvent, Parser};
use tar_core::SparseEntry as CoreSparseEntry;

use crate::entry::{EntryFields, EntryIo};
use crate::error::TarError;
use crate::header::BLOCK_SIZE;
use crate::other;
use crate::{Entry, Header};

/// A top-level representation of an archive file.
///
/// This archive can have an entry added to it and it can be iterated over.
pub struct Archive<R: ?Sized + Read> {
    inner: ArchiveInner<R>,
}

pub struct ArchiveInner<R: ?Sized> {
    pos: Cell<u64>,
    mask: u32,
    unpack_xattrs: bool,
    preserve_permissions: bool,
    preserve_ownerships: bool,
    preserve_mtime: bool,
    overwrite: bool,
    ignore_zeros: bool,
    obj: RefCell<R>,
}

/// An iterator over the entries of an archive.
pub struct Entries<'a, R: 'a + Read> {
    fields: EntriesFields<'a>,
    _ignored: marker::PhantomData<&'a Archive<R>>,
}

trait SeekRead: Read + Seek {}
impl<R: Read + Seek> SeekRead for R {}

struct EntriesFields<'a> {
    archive: &'a Archive<dyn Read + 'a>,
    seekable_archive: Option<&'a Archive<dyn SeekRead + 'a>>,
    next: u64,
    done: bool,
    raw: bool,
    parser: Parser,
    buf: Vec<u8>,
}

impl<R: Read> Archive<R> {
    /// Create a new archive with the underlying object as the reader.
    pub fn new(obj: R) -> Archive<R> {
        Archive {
            inner: ArchiveInner {
                mask: u32::MIN,
                unpack_xattrs: false,
                preserve_permissions: false,
                preserve_ownerships: false,
                preserve_mtime: true,
                overwrite: true,
                ignore_zeros: false,
                obj: RefCell::new(obj),
                pos: Cell::new(0),
            },
        }
    }

    /// Unwrap this archive, returning the underlying object.
    pub fn into_inner(self) -> R {
        self.inner.obj.into_inner()
    }

    /// Construct an iterator over the entries in this archive.
    ///
    /// Note that care must be taken to consider each entry within an archive in
    /// sequence. If entries are processed out of sequence (from what the
    /// iterator returns), then the contents read for each entry may be
    /// corrupted.
    pub fn entries(&mut self) -> io::Result<Entries<'_, R>> {
        let me: &mut Archive<dyn Read> = self;
        me._entries(None).map(|fields| Entries {
            fields,
            _ignored: marker::PhantomData,
        })
    }

    /// Unpacks the contents tarball into the specified `dst`.
    ///
    /// This function will iterate over the entire contents of this tarball,
    /// extracting each file in turn to the location specified by the entry's
    /// path name.
    ///
    /// # Security
    ///
    /// A best-effort is made to prevent writing files outside `dst` (paths
    /// containing `..` are skipped, symlinks are validated). However, there
    /// have been historical bugs in this area, and more may exist. For this
    /// reason, when processing untrusted archives, stronger sandboxing is
    /// encouraged: e.g. the [`cap-std`] crate and/or OS-level
    /// containerization/virtualization.
    ///
    /// If `dst` does not exist, it is created. Unpacking into an existing
    /// directory merges content. This function assumes `dst` is not
    /// concurrently modified by untrusted processes. Protecting against
    /// TOCTOU races is out of scope for this crate.
    ///
    /// [`cap-std`]: https://docs.rs/cap-std/
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
        let me: &mut Archive<dyn Read> = self;
        me._unpack(dst.as_ref())
    }

    /// Set the mask of the permission bits when unpacking this entry.
    ///
    /// The mask will be inverted when applying against a mode, similar to how
    /// `umask` works on Unix. In logical notation it looks like:
    ///
    /// ```text
    /// new_mode = old_mode & (~mask)
    /// ```
    ///
    /// The mask is 0 by default and is currently only implemented on Unix.
    pub fn set_mask(&mut self, mask: u32) {
        self.inner.mask = mask;
    }

    /// Indicate whether extended file attributes (xattrs on Unix) are preserved
    /// when unpacking this archive.
    ///
    /// This flag is disabled by default and is currently only implemented on
    /// Unix using xattr support. This may eventually be implemented for
    /// Windows, however, if other archive implementations are found which do
    /// this as well.
    pub fn set_unpack_xattrs(&mut self, unpack_xattrs: bool) {
        self.inner.unpack_xattrs = unpack_xattrs;
    }

    /// Indicate whether extended permissions (like suid on Unix) are preserved
    /// when unpacking this entry.
    ///
    /// This flag is disabled by default and is currently only implemented on
    /// Unix.
    pub fn set_preserve_permissions(&mut self, preserve: bool) {
        self.inner.preserve_permissions = preserve;
    }

    /// Indicate whether numeric ownership ids (like uid and gid on Unix)
    /// are preserved when unpacking this entry.
    ///
    /// This flag is disabled by default and is currently only implemented on
    /// Unix.
    pub fn set_preserve_ownerships(&mut self, preserve: bool) {
        self.inner.preserve_ownerships = preserve;
    }

    /// Indicate whether files and symlinks should be overwritten on extraction.
    pub fn set_overwrite(&mut self, overwrite: bool) {
        self.inner.overwrite = overwrite;
    }

    /// Indicate whether access time information is preserved when unpacking
    /// this entry.
    ///
    /// This flag is enabled by default.
    pub fn set_preserve_mtime(&mut self, preserve: bool) {
        self.inner.preserve_mtime = preserve;
    }

    /// Ignore zeroed headers, which would otherwise indicate to the archive that it has no more
    /// entries.
    ///
    /// This can be used in case multiple tar archives have been concatenated together.
    pub fn set_ignore_zeros(&mut self, ignore_zeros: bool) {
        self.inner.ignore_zeros = ignore_zeros;
    }
}

impl<R: Seek + Read> Archive<R> {
    /// Construct an iterator over the entries in this archive for a seekable
    /// reader. Seek will be used to efficiently skip over file contents.
    ///
    /// Note that care must be taken to consider each entry within an archive in
    /// sequence. If entries are processed out of sequence (from what the
    /// iterator returns), then the contents read for each entry may be
    /// corrupted.
    pub fn entries_with_seek(&mut self) -> io::Result<Entries<'_, R>> {
        let me: &Archive<dyn Read> = self;
        let me_seekable: &Archive<dyn SeekRead> = self;
        me._entries(Some(me_seekable)).map(|fields| Entries {
            fields,
            _ignored: marker::PhantomData,
        })
    }
}

impl Archive<dyn Read + '_> {
    fn _entries<'a>(
        &'a self,
        seekable_archive: Option<&'a Archive<dyn SeekRead + 'a>>,
    ) -> io::Result<EntriesFields<'a>> {
        if self.inner.pos.get() != 0 {
            return Err(other(
                "cannot call entries unless archive is at \
                 position 0",
            ));
        }
        Ok(EntriesFields {
            archive: self,
            seekable_archive,
            done: false,
            next: 0,
            raw: false,
            parser: new_parser(),
            buf: Vec::new(),
        })
    }

    fn _unpack(&mut self, dst: &Path) -> io::Result<()> {
        if dst.symlink_metadata().is_err() {
            fs::create_dir_all(dst)
                .map_err(|e| TarError::new(format!("failed to create `{}`", dst.display()), e))?;
        }

        // Canonicalizing the dst directory will prepend the path with '\\?\'
        // on windows which will allow windows APIs to treat the path as an
        // extended-length path with a 32,767 character limit. Otherwise all
        // unpacked paths over 260 characters will fail on creation with a
        // NotFound exception.
        let dst = &dst.canonicalize().unwrap_or(dst.to_path_buf());

        // Delay any directory entries until the end (they will be created if needed by
        // descendants), to ensure that directory permissions do not interfere with descendant
        // extraction.
        let mut directories = Vec::new();
        for entry in self._entries(None)? {
            let mut file = entry.map_err(|e| TarError::new("failed to iterate over archive", e))?;
            if file.header().entry_type() == crate::EntryType::Directory {
                directories.push(file);
            } else {
                file.unpack_in(dst)?;
            }
        }

        // Apply the directories.
        //
        // Note: the order of application is important to permissions. That is, we must traverse
        // the filesystem graph in topological ordering or else we risk not being able to create
        // child directories within those of more restrictive permissions. See [0] for details.
        //
        // [0]: <https://github.com/alexcrichton/tar-rs/issues/242>
        directories.sort_by(|a, b| b.path_bytes().cmp(&a.path_bytes()));
        for mut dir in directories {
            dir.unpack_in(dst)?;
        }

        Ok(())
    }
}

impl<'a, R: Read> Entries<'a, R> {
    /// Indicates whether this iterator will return raw entries or not.
    ///
    /// If the raw list of entries is returned, then no preprocessing happens
    /// on account of this library, for example taking into account GNU long name
    /// or long link archive members. Raw iteration is disabled by default.
    pub fn raw(self, raw: bool) -> Entries<'a, R> {
        Entries {
            fields: EntriesFields { raw, ..self.fields },
            _ignored: marker::PhantomData,
        }
    }
}
impl<'a, R: Read> Iterator for Entries<'a, R> {
    type Item = io::Result<Entry<'a, R>>;

    fn next(&mut self) -> Option<io::Result<Entry<'a, R>>> {
        self.fields
            .next()
            .map(|result| result.map(|e| EntryFields::from(e).into_entry()))
    }
}

impl<'a> EntriesFields<'a> {
    /// Read a single raw entry from the archive without processing
    /// extension headers (GNU long name/link, PAX).
    fn next_entry_raw(&mut self) -> io::Result<Option<Entry<'a, io::Empty>>> {
        let mut header = Header::new_old();
        let mut header_pos = self.next;
        loop {
            // Seek to the start of the next header in the archive
            let delta = self
                .next
                .checked_sub(self.archive.inner.pos.get())
                .ok_or_else(|| other("archive position overflow"))?;
            self.skip(delta)?;

            // EOF is an indicator that we are at the end of the archive.
            if !try_read_all(&mut &self.archive.inner, header.as_mut_bytes())? {
                return Ok(None);
            }

            // If a header is not all zeros, we have another valid header.
            // Otherwise, check if we are ignoring zeros and continue, or break as if this is the
            // end of the archive.
            if !header.as_bytes().iter().all(|i| *i == 0) {
                self.next += BLOCK_SIZE;
                break;
            }

            if !self.archive.inner.ignore_zeros {
                return Ok(None);
            }
            self.next += BLOCK_SIZE;
            header_pos = self.next;
        }

        // Make sure the checksum is ok
        let sum = header.as_bytes()[..148]
            .iter()
            .chain(&header.as_bytes()[156..])
            .fold(0, |a, b| a + (*b as u32))
            + 8 * 32;
        let cksum = header.cksum()?;
        if sum != cksum {
            return Err(other("archive header checksum mismatch"));
        }

        let file_pos = self.next;
        let size = header.entry_size()?;
        let ret = EntryFields {
            size,
            header_pos,
            file_pos,
            data: vec![EntryIo::Data((&self.archive.inner).take(size))],
            header,
            long_pathname: None,
            long_linkname: None,
            pax_extensions: None,
            mask: self.archive.inner.mask,
            unpack_xattrs: self.archive.inner.unpack_xattrs,
            preserve_permissions: self.archive.inner.preserve_permissions,
            preserve_mtime: self.archive.inner.preserve_mtime,
            overwrite: self.archive.inner.overwrite,
            preserve_ownerships: self.archive.inner.preserve_ownerships,
        };

        // Store where the next entry is, rounding up by 512 bytes (the size of
        // a header);
        let size = size
            .checked_add(BLOCK_SIZE - 1)
            .ok_or_else(|| other("size overflow"))?;
        self.next = self
            .next
            .checked_add(size & !(BLOCK_SIZE - 1))
            .ok_or_else(|| other("size overflow"))?;

        Ok(Some(ret.into_entry()))
    }

    /// Read header bytes into the buffer and feed them to the tar-core parser
    /// until it emits an Entry or End event.
    fn next_entry(&mut self) -> io::Result<Option<Entry<'a, io::Empty>>> {
        // Skip past any content from the previous entry that hasn't been
        // consumed yet.
        let delta = self
            .next
            .checked_sub(self.archive.inner.pos.get())
            .ok_or_else(|| other("archive position overflow"))?;
        self.skip(delta)?;

        // Clear the header buffer for this round.
        self.buf.clear();

        loop {
            let event = self.parser.parse(&self.buf).map_err(parse_error_to_io)?;

            match event {
                ParseEvent::NeedData { min_bytes } => {
                    let cur_len = self.buf.len();
                    let new_bytes = min_bytes.checked_sub(cur_len).ok_or_else(|| {
                        other("parser requested fewer bytes than already buffered")
                    })?;
                    self.buf.resize(min_bytes, 0);
                    match try_read_all(&mut &self.archive.inner, &mut self.buf[cur_len..]) {
                        Ok(true) => {
                            self.next += new_bytes as u64;
                        }
                        Ok(false) => {
                            if cur_len == 0 || self.archive.inner.ignore_zeros {
                                return Ok(None);
                            }
                            return Err(other("unexpected EOF in archive"));
                        }
                        Err(e) => return Err(e),
                    }
                }

                ParseEvent::End { consumed } => {
                    if self.archive.inner.ignore_zeros {
                        // Drain consumed zero blocks and reset the parser so
                        // it can parse the next concatenated archive (if any).
                        self.buf.drain(..consumed);
                        self.parser = new_parser();
                        continue;
                    }
                    return Ok(None);
                }

                ParseEvent::Entry { consumed, entry } => {
                    let meta = EntryMeta::from_parsed(consumed, entry, None);
                    return self.finish_entry(meta);
                }

                ParseEvent::SparseEntry {
                    consumed,
                    entry,
                    sparse_map,
                    real_size,
                } => {
                    let meta =
                        EntryMeta::from_parsed(consumed, entry, Some((sparse_map, real_size)));
                    return self.finish_entry(meta);
                }

                ParseEvent::GlobalExtensions { consumed, .. } => {
                    // Global PAX headers set defaults for subsequent entries.
                    // tar-rs historically ignores them; consume and continue.
                    self.buf.drain(..consumed);
                    continue;
                }
            }
        }
    }

    /// Finish constructing an entry from its owned metadata.
    ///
    /// `EntryMeta::from_parsed` already consumed all borrowed data from
    /// the `ParsedEntry`, so this method can freely borrow `&mut self`.
    fn finish_entry(&mut self, meta: EntryMeta) -> io::Result<Option<Entry<'a, io::Empty>>> {
        let header_pos = self
            .next
            .checked_sub(meta.consumed as u64)
            .ok_or_else(|| other("archive position overflow"))?;
        let file_pos = self.next;

        // Build the I/O chain.
        let (data, size) = if let Some((sparse_map, real_size)) = meta.sparse {
            let data = Self::build_sparse_io(
                &self.archive.inner,
                &sparse_map,
                real_size,
                meta.content_size,
            )?;
            (data, real_size)
        } else {
            (
                vec![EntryIo::Data((&self.archive.inner).take(meta.content_size))],
                meta.content_size,
            )
        };

        self.next = file_pos
            .checked_add(meta.padded_content_size)
            .ok_or_else(|| other("size overflow"))?;

        let fields = EntryFields {
            size,
            header_pos,
            file_pos,
            data,
            header: meta.header,
            long_pathname: meta.long_pathname,
            long_linkname: meta.long_linkname,
            pax_extensions: meta.pax_extensions,
            mask: self.archive.inner.mask,
            unpack_xattrs: self.archive.inner.unpack_xattrs,
            preserve_permissions: self.archive.inner.preserve_permissions,
            preserve_mtime: self.archive.inner.preserve_mtime,
            overwrite: self.archive.inner.overwrite,
            preserve_ownerships: self.archive.inner.preserve_ownerships,
        };

        Ok(Some(fields.into_entry()))
    }

    /// Build the sparse I/O chain from a tar-core sparse map.
    ///
    /// Interleaves zero-fill padding (`EntryIo::Pad`) for gaps and data
    /// reads (`EntryIo::Data`) for sparse chunks, producing a reader that
    /// yields the logical file content.
    fn build_sparse_io(
        reader: &'a ArchiveInner<dyn Read + 'a>,
        sparse_map: &[CoreSparseEntry],
        real_size: u64,
        on_disk_size: u64,
    ) -> io::Result<Vec<EntryIo<'a>>> {
        let mut data = Vec::new();
        let mut cur = 0u64;
        let mut remaining = on_disk_size;

        for block in sparse_map {
            let off = block.offset;
            let len = block.length;

            if len != 0 && (on_disk_size - remaining) % BLOCK_SIZE != 0 {
                return Err(other(
                    "previous block in sparse file was not \
                     aligned to 512-byte boundary",
                ));
            }
            if off < cur {
                return Err(other(
                    "out of order or overlapping sparse \
                     blocks",
                ));
            }
            if cur < off {
                data.push(EntryIo::Pad(io::repeat(0).take(off - cur)));
            }
            cur = off
                .checked_add(len)
                .ok_or_else(|| other("more bytes listed in sparse file than u64 can hold"))?;
            remaining = remaining.checked_sub(len).ok_or_else(|| {
                other(
                    "sparse file consumed more data than the header \
                     listed",
                )
            })?;
            data.push(EntryIo::Data(reader.take(len)));
        }

        if cur != real_size {
            return Err(other(
                "mismatch in sparse file chunks and \
                 size in header",
            ));
        }
        if remaining > 0 {
            return Err(other(
                "mismatch in sparse file chunks and \
                 entry size in header",
            ));
        }

        Ok(data)
    }

    fn skip(&mut self, mut amt: u64) -> io::Result<()> {
        if let Some(seekable_archive) = self.seekable_archive {
            let pos = io::SeekFrom::Current(
                i64::try_from(amt).map_err(|_| other("seek position out of bounds"))?,
            );
            (&seekable_archive.inner).seek(pos)?;
        } else {
            let mut buf = [0u8; 4096 * 8];
            while amt > 0 {
                let n = cmp::min(amt, buf.len() as u64);
                let n = (&self.archive.inner).read(&mut buf[..n as usize])?;
                if n == 0 {
                    return Err(other("unexpected EOF during skip"));
                }
                amt -= n as u64;
            }
        }
        Ok(())
    }
}

impl<'a> Iterator for EntriesFields<'a> {
    type Item = io::Result<Entry<'a, io::Empty>>;

    fn next(&mut self) -> Option<io::Result<Entry<'a, io::Empty>>> {
        if self.done {
            return None;
        }
        let result = if self.raw {
            self.next_entry_raw()
        } else {
            self.next_entry()
        };
        match result {
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

impl<R: ?Sized + Read> Read for &ArchiveInner<R> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        let i = self.obj.borrow_mut().read(into)?;
        self.pos.set(self.pos.get() + i as u64);
        Ok(i)
    }
}

impl<R: ?Sized + Seek> Seek for &ArchiveInner<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let pos = self.obj.borrow_mut().seek(pos)?;
        self.pos.set(pos);
        Ok(pos)
    }
}

/// Owned entry metadata extracted from a borrowed `ParsedEntry`.
///
/// Consuming the `ParsedEntry` fields into owned data releases the borrow
/// on the parser's input buffer, letting `finish_entry` take `&mut self`
/// to build the I/O chain and update stream positions.
struct EntryMeta {
    consumed: usize,
    header: Header,
    content_size: u64,
    padded_content_size: u64,
    long_pathname: Option<Vec<u8>>,
    long_linkname: Option<Vec<u8>>,
    pax_extensions: Option<Vec<u8>>,
    sparse: Option<(Vec<CoreSparseEntry>, u64)>,
}

impl EntryMeta {
    fn from_parsed(
        consumed: usize,
        entry: tar_core::parse::ParsedEntry<'_>,
        sparse: Option<(Vec<CoreSparseEntry>, u64)>,
    ) -> Self {
        let mut header = Header::new_old();
        header
            .as_mut_bytes()
            .copy_from_slice(entry.header.as_bytes());
        header.set_uid(entry.uid);
        header.set_gid(entry.gid);

        // Extract sizes before moving fields out of entry.
        let content_size = entry.size;
        let padded_content_size = entry.padded_size();

        let long_pathname = if entry.path.as_ref() != entry.header.path_bytes() {
            Some(entry.path.into_owned())
        } else {
            None
        };

        let long_linkname = entry.link_target.and_then(|lt| {
            let header_link = entry.header.link_name_bytes();
            if lt.as_ref() != header_link {
                Some(lt.into_owned())
            } else {
                None
            }
        });

        Self {
            consumed,
            header,
            content_size,
            padded_content_size,
            long_pathname,
            long_linkname,
            pax_extensions: entry.pax.map(|b| b.to_vec()),
            sparse,
        }
    }
}

/// Create a new tar-core parser with default limits.
fn new_parser() -> Parser {
    let mut parser = Parser::new(Limits::default());
    parser.set_allow_empty_path(true);
    parser
}

/// Map tar-core parse errors to io::Error with messages compatible with
/// existing tar-rs error strings.
fn parse_error_to_io(e: ParseError) -> io::Error {
    let msg = match e {
        ParseError::InvalidSize(_) => "size overflow".to_string(),
        err => err.to_string(),
    };
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Try to fill the buffer from the reader.
///
/// If the reader reaches its end before filling the buffer at all, returns `false`.
/// Otherwise returns `true`.
fn try_read_all<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<bool> {
    let mut read = 0;
    while read < buf.len() {
        match r.read(&mut buf[read..])? {
            0 => {
                if read == 0 {
                    return Ok(false);
                }

                return Err(other("failed to read entire block"));
            }
            n => read += n,
        }
    }
    Ok(true)
}
