//! A library for reading and writing TAR archives
//!
//! This library provides utilities necessary to manage [TAR archives][1]
//! abstracted over a reader or writer. Great strides are taken to ensure that
//! an archive is never required to be fully resident in memory, and all objects
//! provide largely a streaming interface to read bytes from.
//!
//! [1]: http://en.wikipedia.org/wiki/Tar_%28computing%29

// More docs about the detailed tar format can also be found here:
// http://www.freebsd.org/cgi/man.cgi?query=tar&sektion=5&manpath=FreeBSD+8-current

// NB: some of the coding patterns and idioms here may seem a little strange.
//     This is currently attempting to expose a super generic interface while
//     also not forcing clients to codegen the entire crate each time they use
//     it. To that end lots of work is done to ensure that concrete
//     implementations are all found in this crate and the generic functions are
//     all just super thin wrappers (e.g. easy to codegen).

#![doc(html_root_url = "http://alexcrichton.com/tar-rs")]
#![deny(missing_docs)]
#![cfg_attr(test, deny(warnings))]

extern crate libc;
extern crate filetime;
#[cfg(all(unix, feature = "xattr"))]
extern crate xattr;

use std::io::{Error, ErrorKind};
use std::ops::{Deref, DerefMut};

pub use header::{Header, OldHeader, UstarHeader, GnuHeader, GnuSparseHeader};
pub use header::{GnuExtSparseHeader};
pub use entry_type::EntryType;
pub use entry::Entry;
pub use archive::{Archive, Entries};
pub use builder::Builder;
pub use pax::{PaxExtensions, PaxExtension};

mod archive;
mod builder;
mod entry;
mod entry_type;
mod error;
mod header;
mod pax;

// FIXME(rust-lang/rust#26403):
//      Right now there's a bug when a DST struct's last field has more
//      alignment than the rest of a structure, causing invalid pointers to be
//      created when it's casted around at runtime. To work around this we force
//      our DST struct to instead have a forcibly higher alignment via a
//      synthesized u64 (hopefully the largest alignment we'll run into in
//      practice), and this should hopefully ensure that the pointers all work
//      out.
struct AlignHigher<R: ?Sized>(u64, R);

impl<R: ?Sized> Deref for AlignHigher<R> {
    type Target = R;
    fn deref(&self) -> &R { &self.1 }
}
impl<R: ?Sized> DerefMut for AlignHigher<R> {
    fn deref_mut(&mut self) -> &mut R { &mut self.1 }
}

fn other(msg: &str) -> Error {
    Error::new(ErrorKind::Other, msg)
}
