//! A library for reading and writing TAR archives
//!
//! This library provides utilities necessary to manage TAR archives [1]
//! abstracted over a reader or writer. Great strides are taken to ensure that
//! an archive is never required to be fully resident in memory, all objects
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
extern crate winapi;
extern crate filetime;

use std::borrow::Cow;
use std::io::{self, Error, ErrorKind};
use std::path::{Path, PathBuf};

#[cfg(unix)] use std::os::unix::prelude::*;
#[cfg(unix)] use std::ffi::{OsStr, OsString};
#[cfg(windows)] use std::os::windows::prelude::*;

macro_rules! try_iter {
    ($me:expr, $e:expr) => (match $e {
        Ok(e) => e,
        Err(e) => { $me.done = true; return Some(Err(e)) }
    })
}

pub use header::Header;
pub use entry_type::EntryType;
pub use entry::{File, Entry};
pub use archive::{Archive, Files, Entries, FilesMut, EntriesMut};

mod archive;
mod entry;
mod entry_type;
mod error;
mod header;

fn bad_archive() -> Error {
    other("invalid tar archive")
}

fn other(msg: &str) -> Error {
    Error::new(ErrorKind::Other, msg)
}

#[cfg(windows)]
fn path2bytes(p: &Path) -> io::Result<&[u8]> {
    p.as_os_str().to_str().map(|s| s.as_bytes()).ok_or_else(|| {
        other("path was not valid unicode")
    })
}

#[cfg(unix)]
fn path2bytes(p: &Path) -> io::Result<&[u8]> {
    Ok(p.as_os_str().as_bytes())
}

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

#[cfg(windows)]
fn not_unicode() -> Error {
    other("only unicode paths are supported on windows")
}
