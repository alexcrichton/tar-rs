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

#![doc(html_root_url = "https://docs.rs/tar/0.4")]
#![deny(missing_docs)]
#![cfg_attr(test, deny(warnings))]

use std::io::{Error, ErrorKind};

pub use crate::archive::{Archive, Entries};
pub use crate::builder::{Builder, EntryWriter};
pub use crate::entry::{Entry, Unpacked};
pub use crate::entry_type::EntryType;
pub use crate::header::GnuExtSparseHeader;
pub use crate::header::{GnuHeader, GnuSparseHeader, Header, HeaderMode, OldHeader, UstarHeader};
pub use crate::pax::{PaxExtension, PaxExtensions};

mod archive;
mod builder;
mod entry;
mod entry_type;
mod error;
mod header;
mod pax;

fn other(msg: &str) -> Error {
    Error::new(ErrorKind::Other, msg)
}
