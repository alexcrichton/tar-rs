use std::borrow::Cow;
use std::cmp;
use std::io::prelude::*;
use std::io::{self, SeekFrom};
use std::path::Path;

use header::bytes2path;
use other;
use {Entry, Header};

macro_rules! try_iter {
    ($e:expr) => (match $e {
        Ok(e) => e,
        Err(e) => return Some(Err(e)),
    })
}

/// dox
pub struct GnuEntries<'a, R: 'a> {
    inner: Box<Iterator<Item=io::Result<Entry<'a, R>>> + 'a>,
}

/// dox
pub struct GnuEntry<'a, R: 'a> {
    inner: Entry<'a, R>,
    name: Option<Vec<u8>>,
}

impl<'a, R: 'a + Read> GnuEntries<'a, R> {
    /// dox
    pub fn new<I>(i: I) -> GnuEntries<'a, R>
        where I: IntoIterator<Item=io::Result<Entry<'a, R>>> + 'a,
              I::IntoIter: 'a,
    {
        GnuEntries { inner: Box::new(i.into_iter()) }
    }
}

impl<'a, R: 'a + Read> Iterator for GnuEntries<'a, R> {
    type Item = io::Result<GnuEntry<'a, R>>;

    fn next(&mut self) -> Option<io::Result<GnuEntry<'a, R>>> {
        let mut entry = match self.inner.next() {
            Some(Ok(e)) => e,
            Some(Err(e)) => return Some(Err(e)),
            None => return None,
        };

        if !entry.header().entry_type().is_gnu_longname() {
            return Some(Ok(GnuEntry { inner: entry, name: None }))
        }

        // Don't allow too too crazy allocation sizes up front
        let cap = cmp::min(entry.header().size().unwrap_or(0), 128 * 1024);
        let mut filename = Vec::with_capacity(cap as usize);
        try_iter!(entry.read_to_end(&mut filename));

        match self.inner.next() {
            Some(Ok(e)) => Some(Ok(GnuEntry { inner: e, name: Some(filename) })),
            Some(Err(e)) => Some(Err(e)),
            None => Some(Err(other("longname entry not followed by another"))),
        }
    }
}

impl<'a, R: 'a + Read> GnuEntry<'a, R> {
    /// Returns access to the header of this entry in the archive.
    ///
    /// For more information see `Entry::header`
    pub fn header(&self) -> &Header {
        self.inner.header()
    }

    /// Writes this file to the specified location.
    ///
    /// For more information see `Entry::unpack`.
    pub fn unpack<P: AsRef<Path>>(&mut self, dst: P) -> io::Result<()> {
        self.inner.unpack(dst.as_ref())
    }

    /// dox
    pub fn path(&self) -> io::Result<Cow<Path>> {
        match self.name {
            Some(ref bytes) => bytes2path(Cow::Borrowed(bytes)),
            None => self.header().path(),
        }
    }

    /// dox
    pub fn path_bytes(&self) -> Cow<[u8]> {
        match self.name {
            Some(ref bytes) => Cow::Borrowed(bytes),
            None => self.header().path_bytes(),
        }
    }
}

impl<'a, R: Read> Read for GnuEntry<'a, R> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        self.inner.read(into)
    }
}

impl<'a, R: Read + Seek> Seek for GnuEntry<'a, R> {
    fn seek(&mut self, how: SeekFrom) -> io::Result<u64> {
        self.inner.seek(how)
    }
}

