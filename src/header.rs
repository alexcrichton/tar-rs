#[cfg(unix)] use std::os::unix::prelude::*;
#[cfg(windows)] use std::os::windows::prelude::*;

use std::borrow::Cow;
use std::cmp;
use std::fmt;
use std::fs;
use std::io;
use std::iter::repeat;
use std::mem;
use std::path::Path;
use std::str;

use libc;

use EntryType;
use {bad_archive, other, path2bytes, bytes2path};

/// Representation of the header of an entry in an archive
#[repr(C)]
#[allow(missing_docs)]
pub struct Header {
    pub name: [u8; 100],
    pub mode: [u8; 8],
    pub owner_id: [u8; 8],
    pub group_id: [u8; 8],
    pub size: [u8; 12],
    pub mtime: [u8; 12],
    pub cksum: [u8; 8],
    pub link: [u8; 1],
    pub linkname: [u8; 100],

    // UStar format
    pub ustar: [u8; 6],
    pub ustar_version: [u8; 2],
    pub owner_name: [u8; 32],
    pub group_name: [u8; 32],
    pub dev_major: [u8; 8],
    pub dev_minor: [u8; 8],
    pub prefix: [u8; 155],
    _rest: [u8; 12],
}

impl Header {
    /// Creates a new blank ustar header ready to be filled in
    pub fn new() -> Header {
        let mut header: Header = unsafe { mem::zeroed() };
        // Flag this header as a UStar archive
        header.ustar = *b"ustar\0";
        header.ustar_version = *b"00";
        return header
    }

    fn is_ustar(&self) -> bool {
        &self.ustar[..5] == b"ustar"
    }

    /// Returns a view into this header as a byte array.
    pub fn as_bytes(&self) -> &[u8; 512] {
        debug_assert_eq!(512, mem::size_of_val(self));
        unsafe { &*(self as *const _ as *const [u8; 512]) }
    }

    /// Blanket sets the metadata in this header from the metadata argument
    /// provided.
    ///
    /// This is useful for initializing a `Header` from the OS's metadata from a
    /// file.
    pub fn set_metadata(&mut self, meta: &fs::Metadata) {
        // Platform-specific fill
        self.fill_from(meta);
        // Platform-agnostic fill
        // Set size of directories to zero
        self.set_size(if meta.is_dir() { 0 } else { meta.len() });
        self.set_device_major(0);
        self.set_device_minor(0);
    }

    /// Returns the file size this header represents.
    ///
    /// May return an error if the field is corrupted.
    pub fn size(&self) -> io::Result<u64> {
        octal_from(&self.size)
    }

    /// Encodes the `size` argument into the size field of this header.
    pub fn set_size(&mut self, size: u64) {
        octal_into(&mut self.size, size)
    }

    /// Returns the raw path name stored in this header.
    ///
    /// This method may fail if the pathname is not valid unicode and this is
    /// called on a Windows platform.
    ///
    /// Note that this function will convert any `\` characters to directory
    /// separators.
    pub fn path(&self) -> io::Result<Cow<Path>> {
        bytes2path(self.path_bytes())
    }

    /// Returns the pathname stored in this header as a byte array.
    ///
    /// This function is guaranteed to succeed, but you may wish to call the
    /// `path` method to convert to a `Path`.
    ///
    /// Note that this function will convert any `\` characters to directory
    /// separators.
    pub fn path_bytes(&self) -> Cow<[u8]> {
        if (!self.is_ustar() || self.prefix[0] == 0) &&
           !self.name.contains(&b'\\') {
            Cow::Borrowed(truncate(&self.name))
        } else {
            fn noslash(b: &u8) -> u8 {
                if *b == b'\\' {b'/'} else {*b}
            }
            let mut bytes = Vec::new();
            let prefix = truncate(&self.prefix);
            if prefix.len() > 0 {
                bytes.extend(prefix.iter().map(noslash));
                bytes.push(b'/');
            }
            bytes.extend(truncate(&self.name).iter().map(noslash));
            Cow::Owned(bytes)
        }
    }

    /// Sets the path name for this header.
    ///
    /// This function will set the pathname listed in this header, encoding it
    /// in the appropriate format. May fail if the path is too long or if the
    /// path specified is not unicode and this is a Windows platform.
    pub fn set_path<P: AsRef<Path>>(&mut self, p: P) -> io::Result<()> {
        self._set_path(p.as_ref())
    }

    fn _set_path(&mut self, path: &Path) -> io::Result<()> {
        let bytes = try!(path2bytes(path));
        let (namelen, prefixlen) = (self.name.len(), self.prefix.len());
        if bytes.len() <= namelen {
            try!(copy_into(&mut self.name, bytes, true));
        } else {
            let prefix = &bytes[..cmp::min(bytes.len(), prefixlen)];
            let pos = match prefix.iter().rposition(|&b| b == b'/' || b == b'\\') {
                Some(i) => i,
                None => return Err(other("path cannot be split to be inserted \
                                          into archive")),
            };
            try!(copy_into(&mut self.name, &bytes[pos + 1..], true));
            try!(copy_into(&mut self.prefix, &bytes[..pos], true));
        }
        Ok(())
    }

    /// Returns the link name stored in this header, if any is found.
    ///
    /// This method may fail if the pathname is not valid unicode and this is
    /// called on a Windows platform. `Ok(None)` being returned, however,
    /// indicates that the link name was not present.
    ///
    /// Note that this function will convert any `\` characters to directory
    /// separators.
    pub fn link_name(&self) -> io::Result<Option<Cow<Path>>> {
        match self.link_name_bytes() {
            Some(bytes) => bytes2path(bytes).map(Some),
            None => Ok(None),
        }
    }

    /// Returns the link name stored in this header as a byte array, if any.
    ///
    /// This function is guaranteed to succeed, but you may wish to call the
    /// `link_name` method to convert to a `Path`.
    ///
    /// Note that this function will convert any `\` characters to directory
    /// separators.
    pub fn link_name_bytes(&self) -> Option<Cow<[u8]>> {
        if self.linkname[0] == 0 {
            None
        } else {
            Some(deslash(&self.linkname))
        }
    }

    /// Sets the path name for this header.
    ///
    /// This function will set the pathname listed in this header, encoding it
    /// in the appropriate format. May fail if the path is too long or if the
    /// path specified is not unicode and this is a Windows platform.
    pub fn set_link_name<P: AsRef<Path>>(&mut self, p: P) -> io::Result<()> {
        self._set_link_name(p.as_ref())
    }

    fn _set_link_name(&mut self, path: &Path) -> io::Result<()> {
        let bytes = try!(path2bytes(path));
        try!(copy_into(&mut self.linkname, bytes, true));
        Ok(())
    }

    /// Returns the mode bits for this file
    ///
    /// May return an error if the field is corrupted.
    pub fn mode(&self) -> io::Result<u32> {
        octal_from(&self.mode).map(|u| u as u32)
    }

    /// Encodes the `mode` provided into this header.
    pub fn set_mode(&mut self, mode: u32) {
        octal_into(&mut self.mode, mode & 0o3777);
    }

    /// Returns the value of the owner's user ID field
    ///
    /// May return an error if the field is corrupted.
    pub fn uid(&self) -> io::Result<u32> {
        octal_from(&self.owner_id).map(|u| u as u32)
    }

    /// Encodes the `uid` provided into this header.
    pub fn set_uid(&mut self, uid: u32) {
        octal_into(&mut self.owner_id, uid);
    }

    /// Returns the value of the group's user ID field
    pub fn gid(&self) -> io::Result<u32> {
        octal_from(&self.group_id).map(|u| u as u32)
    }

    /// Encodes the `gid` provided into this header.
    pub fn set_gid(&mut self, gid: u32) {
        octal_into(&mut self.group_id, gid);
    }

    /// Returns the last modification time in Unix time format
    pub fn mtime(&self) -> io::Result<u64> {
        octal_from(&self.mtime)
    }

    /// Encodes the `mtime` provided into this header.
    ///
    /// Note that this time is typically a number of seconds passed since
    /// January 1, 1970.
    pub fn set_mtime(&mut self, mtime: u64) {
        octal_into(&mut self.mtime, mtime);
    }

    /// Return the username of the owner of this file, if present and if valid
    /// utf8
    pub fn username(&self) -> Option<&str> {
        self.username_bytes().and_then(|s| str::from_utf8(s).ok())
    }

    /// Returns the username of the owner of this file, if present
    pub fn username_bytes(&self) -> Option<&[u8]> {
        if self.is_ustar() {
            Some(truncate(&self.owner_name))
        } else {
            None
        }
    }

    /// Sets the username inside this header.
    ///
    /// May return an error if the name provided is too long.
    pub fn set_username(&mut self, name: &str) -> io::Result<()> {
        copy_into(&mut self.owner_name, name.as_bytes(), false)
    }

    /// Return the group name of the owner of this file, if present and if valid
    /// utf8
    pub fn groupname(&self) -> Option<&str> {
        self.groupname_bytes().and_then(|s| str::from_utf8(s).ok())
    }

    /// Returns the group name of the owner of this file, if present
    pub fn groupname_bytes(&self) -> Option<&[u8]> {
        if self.is_ustar() {
            Some(truncate(&self.group_name))
        } else {
            None
        }
    }

    /// Sets the group name inside this header.
    ///
    /// May return an error if the name provided is too long.
    pub fn set_groupname(&mut self, name: &str) -> io::Result<()> {
        copy_into(&mut self.group_name, name.as_bytes(), false)
    }

    /// Returns the device major number, if present.
    ///
    /// This field is only present in UStar archives. A value of `None` means
    /// that this archive is not a UStar archive, while a value of `Some`
    /// represents the attempt to decode the field in the header.
    pub fn device_major(&self) -> Option<io::Result<u32>> {
        if self.is_ustar() {
            Some(octal_from(&self.dev_major).map(|u| u as u32))
        } else {
            None
        }
    }

    /// Encodes the value `major` into the dev_major field of this header.
    pub fn set_device_major(&mut self, major: u32) {
        octal_into(&mut self.dev_major, major);
    }

    /// Returns the device minor number, if present.
    ///
    /// This field is only present in UStar archives. A value of `None` means
    /// that this archive is not a UStar archive, while a value of `Some`
    /// represents the attempt to decode the field in the header.
    pub fn device_minor(&self) -> Option<io::Result<u32>> {
        if self.is_ustar() {
            Some(octal_from(&self.dev_minor).map(|u| u as u32))
        } else {
            None
        }
    }

    /// Encodes the value `minor` into the dev_major field of this header.
    pub fn set_device_minor(&mut self, minor: u32) {
        octal_into(&mut self.dev_minor, minor);
    }

    /// Returns the type of file described by this header.
    pub fn entry_type(&self) -> EntryType {
        EntryType::new(self.link[0])
    }

    /// Sets the type of file that will be described by this header.
    pub fn set_entry_type(&mut self, ty: EntryType) {
        self.link = [ty.as_byte()];
    }

    /// Returns the checksum field of this header.
    ///
    /// May return an error if the field is corrupted.
    pub fn cksum(&self) -> io::Result<u32> {
        octal_from(&self.cksum).map(|u| u as u32)
    }

    /// Sets the checksum field of this header based on the current fields in
    /// this header.
    pub fn set_cksum(&mut self) {
        let cksum = {
            let bytes = self.as_bytes();
            bytes[..148].iter().map(|i| *i as u32).fold(0, |a, b| a + b) +
                bytes[156..].iter().map(|i| *i as u32).fold(0, |a, b| a + b) +
                32 * (self.cksum.len() as u32)
        };
        octal_into(&mut self.cksum, cksum);
    }

    #[cfg(unix)]
    fn fill_from(&mut self, meta: &fs::Metadata) {
        self.set_mode((meta.mode() & 0o3777) as u32);
        self.set_mtime(meta.mtime() as u64);
        self.set_uid(meta.uid() as u32);
        self.set_gid(meta.gid() as u32);

        // TODO: need to bind more file types
        self.set_entry_type(match meta.mode() & libc::S_IFMT {
            libc::S_IFREG => EntryType::file(),
            libc::S_IFLNK => EntryType::symlink(),
            libc::S_IFCHR => EntryType::character_special(),
            libc::S_IFBLK => EntryType::block_special(),
            libc::S_IFDIR => EntryType::dir(),
            libc::S_IFIFO => EntryType::fifo(),
            _ => EntryType::new(b' '),
        });
    }

    #[cfg(windows)]
    fn fill_from(&mut self, meta: &fs::Metadata) {
        let readonly = meta.file_attributes() & winapi::FILE_ATTRIBUTE_READONLY;

        // There's no concept of a mode on windows, so do a best approximation
        // here.
        let mode = match (meta.is_dir(), readonly != 0) {
            (true, false) => 0o755,
            (true, true) => 0o555,
            (false, false) => 0o644,
            (false, true) => 0o444,
        };
        self.set_mode(mode);
        self.set_uid(0);
        self.set_gid(0);

        let ft = meta.file_type();
        self.set_entry_type(if ft.is_dir() {
            EntryType::dir()
        } else if ft.is_file() {
            EntryType::file()
        } else if ft.is_symlink() {
            EntryType::symlink()
        } else {
            EntryType::new(b' ')
        });

        // The dates listed in tarballs are always seconds relative to
        // January 1, 1970. On Windows, however, the timestamps are returned as
        // dates relative to January 1, 1601 (in 100ns intervals), so we need to
        // add in some offset for those dates.
        let mtime = (meta.last_write_time() / (1_000_000_000 / 100)) - 11644473600;
        self.set_mtime(mtime);
    }
}

impl Clone for Header {
    fn clone(&self) -> Header {
        Header { ..*self }
    }
}

fn deslash(bytes: &[u8]) -> Cow<[u8]> {
    if !bytes.contains(&b'\\') {
        Cow::Borrowed(truncate(bytes))
    } else {
        fn noslash(b: &u8) -> u8 {
            if *b == b'\\' {b'/'} else {*b}
        }
        Cow::Owned(truncate(bytes).iter().map(noslash).collect())
    }
}

fn octal_from(slice: &[u8]) -> io::Result<u64> {
    let num = match str::from_utf8(truncate(slice)) {
        Ok(n) => n,
        Err(_) => return Err(bad_archive()),
    };
    match u64::from_str_radix(num.trim(), 8) {
        Ok(n) => Ok(n),
        Err(_) => Err(bad_archive())
    }
}

fn octal_into<T: fmt::Octal>(dst: &mut [u8], val: T) {
    let o = format!("{:o}", val);
    let value = o.bytes().rev().chain(repeat(b'0'));
    for (slot, value) in dst.iter_mut().rev().skip(1).zip(value) {
        *slot = value;
    }
}

fn truncate<'a>(slice: &'a [u8]) -> &'a [u8] {
    match slice.iter().position(|i| *i == 0) {
        Some(i) => &slice[..i],
        None => slice,
    }
}

/// Copies `bytes` into the `slot` provided, returning an error if the `bytes`
/// array is too long or if it contains any nul bytes.
///
/// Also provides the option to map '\' characters to '/' characters for the
/// names of paths in archives. The `tar` utility doesn't seem to like windows
/// backslashes when unpacking on Unix.
fn copy_into(slot: &mut [u8], bytes: &[u8], map_slashes: bool) -> io::Result<()> {
    if bytes.len() > slot.len() {
        Err(other("provided value is too long"))
    } else if bytes.iter().any(|b| *b == 0) {
        Err(other("provided value contains a nul byte"))
    } else {
        for (slot, val) in slot.iter_mut().zip(bytes) {
            if map_slashes && *val == b'\\' {
                *slot = b'/';
            } else {
                *slot = *val;
            }
        }
        Ok(())
    }
}
