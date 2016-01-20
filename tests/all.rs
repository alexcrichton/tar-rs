extern crate filetime;
extern crate tar;
extern crate tempdir;

use std::io::prelude::*;
use std::io::{self, Cursor, SeekFrom};
use std::iter::repeat;
use std::fs::{self, File};
use std::path::Path;

use filetime::FileTime;
use self::tempdir::TempDir;
use tar::{Archive, Builder, Header};

macro_rules! t {
    ($e:expr) => (match $e {
        Ok(v) => v,
        Err(e) => panic!("{} returned {}", stringify!($e), e),
    })
}

macro_rules! tar {
    ($e:expr) => (&include_bytes!(concat!("archives/", $e))[..])
}

mod header;

#[test]
fn simple() {
    let mut ar = Archive::new(Cursor::new(tar!("simple.tar")));
    for entry in t!(ar.entries()) {
        t!(entry);
    }
    let mut ar = Archive::new(Cursor::new(tar!("simple.tar")));
    for entry in t!(ar.entries()) {
        t!(entry);
    }
}

#[test]
fn header_impls() {
    let mut ar = Archive::new(Cursor::new(tar!("simple.tar")));
    let hn = Header::new_old();
    let hnb = hn.as_bytes();
    for file in t!(ar.entries()) {
        let file = t!(file);
        let h1 = file.header();
        let h1b = h1.as_bytes();
        let h2 = h1.clone();
        let h2b = h2.as_bytes();
        assert!(h1b[..] == h2b[..] && h2b[..] != hnb[..])
    }
}

#[test]
fn reading_files() {
    let rdr = Cursor::new(tar!("reading_files.tar"));
    let mut ar = Archive::new(rdr);
    let mut entries = t!(ar.entries());

    let mut a = t!(entries.next().unwrap());
    assert_eq!(&*a.header().path_bytes(), b"a");
    let mut s = String::new();
    t!(a.read_to_string(&mut s));
    assert_eq!(s, "a\na\na\na\na\na\na\na\na\na\na\n");

    let mut b = t!(entries.next().unwrap());
    assert_eq!(&*b.header().path_bytes(), b"b");
    s.truncate(0);
    t!(b.read_to_string(&mut s));
    assert_eq!(s, "b\nb\nb\nb\nb\nb\nb\nb\nb\nb\nb\n");

    assert!(entries.next().is_none());
}

#[test]
fn writing_files() {
    let mut ar = Builder::new(Vec::new());
    let td = t!(TempDir::new("tar-rs"));

    let path = td.path().join("test");
    t!(t!(File::create(&path)).write_all(b"test"));

    t!(ar.append_file("test2", &mut t!(File::open(&path))));

    let data = t!(ar.into_inner());
    let mut ar = Archive::new(Cursor::new(data));
    let mut entries = t!(ar.entries());
    let mut f = t!(entries.next().unwrap());

    assert_eq!(&*f.header().path_bytes(), b"test2");
    assert_eq!(f.header().size().unwrap(), 4);
    let mut s = String::new();
    t!(f.read_to_string(&mut s));
    assert_eq!(s, "test");

    assert!(entries.next().is_none());
}

#[test]
fn large_filename() {
    let mut ar = Builder::new(Vec::new());
    let td = t!(TempDir::new("tar-rs"));

    let path = td.path().join("test");
    t!(t!(File::create(&path)).write_all(b"test"));

    let filename = repeat("abcd/").take(50).collect::<String>();
    let mut header = Header::new_ustar();
    header.set_path(&filename).unwrap();
    header.set_metadata(&t!(fs::metadata(&path)));
    header.set_cksum();
    t!(ar.append(&header, &b"test"[..]));
    let too_long = repeat("abcd").take(200).collect::<String>();
    t!(ar.append_file(&too_long, &mut t!(File::open(&path))));

    let rd = Cursor::new(t!(ar.into_inner()));
    let mut ar = Archive::new(rd);
    let mut entries = t!(ar.entries());

    let mut f = entries.next().unwrap().unwrap();
    assert_eq!(&*f.header().path_bytes(), filename.as_bytes());
    assert_eq!(f.header().size().unwrap(), 4);
    let mut s = String::new();
    t!(f.read_to_string(&mut s));
    assert_eq!(s, "test");

    let mut f = entries.next().unwrap().unwrap();
    assert_eq!(&*f.path_bytes(), too_long.as_bytes());
    assert_eq!(f.header().size().unwrap(), 4);
    let mut s = String::new();
    t!(f.read_to_string(&mut s));
    assert_eq!(s, "test");

    assert!(entries.next().is_none());
}

#[test]
fn reading_entries() {
    let rdr = Cursor::new(tar!("reading_files.tar"));
    let mut ar = Archive::new(rdr);
    let mut entries = t!(ar.entries());
    let mut a = t!(entries.next().unwrap());
    assert_eq!(&*a.header().path_bytes(), b"a");
    let mut s = String::new();
    t!(a.read_to_string(&mut s));
    assert_eq!(s, "a\na\na\na\na\na\na\na\na\na\na\n");
    s.truncate(0);
    t!(a.read_to_string(&mut s));
    assert_eq!(s, "");
    let mut b = t!(entries.next().unwrap());

    assert_eq!(&*b.header().path_bytes(), b"b");
    s.truncate(0);
    t!(b.read_to_string(&mut s));
    assert_eq!(s, "b\nb\nb\nb\nb\nb\nb\nb\nb\nb\nb\n");
    assert!(entries.next().is_none());
}

fn check_dirtree(td: &TempDir) {
    let dir_a = td.path().join("a");
    let dir_b = td.path().join("a/b");
    let file_c = td.path().join("a/c");
    assert!(fs::metadata(&dir_a).map(|m| m.is_dir()).unwrap_or(false));
    assert!(fs::metadata(&dir_b).map(|m| m.is_dir()).unwrap_or(false));
    assert!(fs::metadata(&file_c).map(|m| m.is_file()).unwrap_or(false));
}

#[test]
fn extracting_directories() {
    let td = t!(TempDir::new("tar-rs"));
    let rdr = Cursor::new(tar!("directory.tar"));
    let mut ar = Archive::new(rdr);
    t!(ar.unpack(td.path()));
    check_dirtree(&td);
}

#[test]
fn writing_and_extracting_directories() {
    let td = t!(TempDir::new("tar-rs"));

    let mut ar = Builder::new(Vec::new());
    let tmppath = td.path().join("tmpfile");
    t!(t!(File::create(&tmppath)).write_all(b"c"));
    t!(ar.append_dir("a", "."));
    t!(ar.append_dir("a/b", "."));
    t!(ar.append_file("a/c", &mut t!(File::open(&tmppath))));
    t!(ar.finish());

    let rdr = Cursor::new(t!(ar.into_inner()));
    let mut ar = Archive::new(rdr);
    t!(ar.unpack(td.path()));
    check_dirtree(&td);
}

#[test]
fn extracting_duplicate_dirs() {
    let td = t!(TempDir::new("tar-rs"));
    let rdr = Cursor::new(tar!("duplicate_dirs.tar"));
    let mut ar = Archive::new(rdr);
    t!(ar.unpack(td.path()));

    let some_dir = td.path().join("some_dir");
    assert!(fs::metadata(&some_dir).map(|m| m.is_dir()).unwrap_or(false));
}

#[test]
fn handling_incorrect_file_size() {
    let td = t!(TempDir::new("tar-rs"));

    let mut ar = Builder::new(Vec::new());

    let path = td.path().join("tmpfile");
    t!(File::create(&path));
    let mut file = t!(File::open(&path));
    let mut header = Header::new_old();
    t!(header.set_path("somepath"));
    header.set_metadata(&t!(file.metadata()));
    header.set_size(2048); // past the end of file null blocks
    header.set_cksum();
    t!(ar.append(&header, &mut file));

    // Extracting
    let rdr = Cursor::new(t!(ar.into_inner()));
    let mut ar = Archive::new(rdr);
    assert!(ar.unpack(td.path()).is_err());

    // Iterating
    let rdr = Cursor::new(ar.into_inner().into_inner());
    let mut ar = Archive::new(rdr);
    assert!(t!(ar.entries()).any(|fr| fr.is_err()));
}

#[test]
fn extracting_malicious_tarball() {
    use std::fs;
    use std::fs::OpenOptions;
    use std::io::{Seek, Write};

    let td = t!(TempDir::new("tar-rs"));

    let mut evil_tar = Cursor::new(Vec::new());

    {
        let mut a = Builder::new(&mut evil_tar);
        let mut evil_txt_f = t!(OpenOptions::new().read(true).write(true)
                                            .create(true)
                                            .open(td.path().join("evil.txt")));
        t!(writeln!(evil_txt_f, "This is an evil file."));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file("/tmp/abs_evil.txt", &mut evil_txt_f));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file("//tmp/abs_evil2.txt", &mut evil_txt_f));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file("///tmp/abs_evil3.txt", &mut evil_txt_f));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file("/./tmp/abs_evil4.txt", &mut evil_txt_f));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file("//./tmp/abs_evil5.txt", &mut evil_txt_f));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file("///./tmp/abs_evil6.txt", &mut evil_txt_f));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file("/../tmp/rel_evil.txt", &mut evil_txt_f));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file("../rel_evil2.txt", &mut evil_txt_f));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file("./../rel_evil3.txt", &mut evil_txt_f));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file("some/../../rel_evil4.txt", &mut evil_txt_f));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file("", &mut evil_txt_f));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file("././//./", &mut evil_txt_f));
        t!(evil_txt_f.seek(SeekFrom::Start(0)));
        t!(a.append_file(".", &mut evil_txt_f));
        t!(a.finish());
    }

    t!(evil_tar.seek(SeekFrom::Start(0)));
    let mut ar = Archive::new(&mut evil_tar);
    t!(ar.unpack(td.path()));

    assert!(fs::metadata("/tmp/abs_evil.txt").is_err());
    assert!(fs::metadata("/tmp/abs_evil.txt2").is_err());
    assert!(fs::metadata("/tmp/abs_evil.txt3").is_err());
    assert!(fs::metadata("/tmp/abs_evil.txt4").is_err());
    assert!(fs::metadata("/tmp/abs_evil.txt5").is_err());
    assert!(fs::metadata("/tmp/abs_evil.txt6").is_err());
    assert!(fs::metadata("/tmp/rel_evil.txt").is_err());
    assert!(fs::metadata("/tmp/rel_evil.txt").is_err());
    assert!(fs::metadata(td.path().join("../tmp/rel_evil.txt")).is_err());
    assert!(fs::metadata(td.path().join("../rel_evil2.txt")).is_err());
    assert!(fs::metadata(td.path().join("../rel_evil3.txt")).is_err());
    assert!(fs::metadata(td.path().join("../rel_evil4.txt")).is_err());

    // The `some` subdirectory should not be created because the only
    // filename that references this has '..'.
    assert!(fs::metadata(td.path().join("some")).is_err());

    // The `tmp` subdirectory should be created and within this
    // subdirectory, there should be files named `abs_evil.txt` through
    // `abs_evil6.txt`.
    assert!(fs::metadata(td.path().join("tmp")).map(|m| m.is_dir())
               .unwrap_or(false));
    assert!(fs::metadata(td.path().join("tmp/abs_evil.txt"))
               .map(|m| m.is_file()).unwrap_or(false));
    assert!(fs::metadata(td.path().join("tmp/abs_evil2.txt"))
               .map(|m| m.is_file()).unwrap_or(false));
    assert!(fs::metadata(td.path().join("tmp/abs_evil3.txt"))
               .map(|m| m.is_file()).unwrap_or(false));
    assert!(fs::metadata(td.path().join("tmp/abs_evil4.txt"))
               .map(|m| m.is_file()).unwrap_or(false));
    assert!(fs::metadata(td.path().join("tmp/abs_evil5.txt"))
               .map(|m| m.is_file()).unwrap_or(false));
    assert!(fs::metadata(td.path().join("tmp/abs_evil6.txt"))
               .map(|m| m.is_file()).unwrap_or(false));
}

#[test]
fn octal_spaces() {
    let rdr = Cursor::new(tar!("spaces.tar"));
    let mut ar = Archive::new(rdr);

    let entry = ar.entries().unwrap().next().unwrap().unwrap();
    assert_eq!(entry.header().mode().unwrap() & 0o777, 0o777);
    assert_eq!(entry.header().uid().unwrap(), 0);
    assert_eq!(entry.header().gid().unwrap(), 0);
    assert_eq!(entry.header().size().unwrap(), 2);
    assert_eq!(entry.header().mtime().unwrap(), 0o12440016664);
    assert_eq!(entry.header().cksum().unwrap(), 0o4253);
}

#[test]
fn extracting_malformed_tar_null_blocks() {
    let td = t!(TempDir::new("tar-rs"));

    let mut ar = Builder::new(Vec::new());

    let path1 = td.path().join("tmpfile1");
    let path2 = td.path().join("tmpfile2");
    t!(File::create(&path1));
    t!(File::create(&path2));
    t!(ar.append_path(&path1));
    let mut data = t!(ar.into_inner());
    let amt = data.len();
    data.truncate(amt - 512);
    let mut ar = Builder::new(data);
    t!(ar.append_path(&path2));
    t!(ar.finish());

    let data = t!(ar.into_inner());
    let mut ar = Archive::new(&data[..]);
    assert!(ar.unpack(td.path()).is_err());
}

#[test]
fn empty_filename()
{
    let td = t!(TempDir::new("tar-rs"));
    let rdr = Cursor::new(tar!("empty_filename.tar"));
    let mut ar = Archive::new(rdr);
    assert!(ar.unpack(td.path()).is_err());
}

#[test]
fn file_times() {
    let td = t!(TempDir::new("tar-rs"));
    let rdr = Cursor::new(tar!("file_times.tar"));
    let mut ar = Archive::new(rdr);
    t!(ar.unpack(td.path()));

    let meta = fs::metadata(td.path().join("a")).unwrap();
    let mtime = FileTime::from_last_modification_time(&meta);
    let atime = FileTime::from_last_access_time(&meta);
    assert_eq!(mtime.seconds_relative_to_1970(), 1000000000);
    assert_eq!(mtime.nanoseconds(), 0);
    assert_eq!(atime.seconds_relative_to_1970(), 1000000000);
    assert_eq!(atime.nanoseconds(), 0);
}

#[test]
fn backslash_same_as_slash() {
    // Insert a file into an archive with a backslash
    let td = t!(TempDir::new("tar-rs"));
    let mut ar = Builder::new(Vec::<u8>::new());
    t!(ar.append_dir("foo\\bar", td.path()));
    let mut ar = Archive::new(Cursor::new(t!(ar.into_inner())));
    let f = t!(t!(ar.entries()).next().unwrap());
    assert_eq!(&*f.header().path().unwrap(), Path::new("foo/bar"));

    // Unpack an archive with a backslash in the name
    let mut ar = Builder::new(Vec::<u8>::new());
    let mut header = Header::new_gnu();
    header.set_metadata(&t!(fs::metadata(td.path())));
    header.set_size(0);
    for (a, b) in header.as_old_mut().name.iter_mut().zip(b"foo\\bar\x00") {
        *a = *b;
    }
    header.set_cksum();
    t!(ar.append(&header, &mut io::empty()));
    let data = t!(ar.into_inner());
    let mut ar = Archive::new(&data[..]);
    let f = t!(t!(ar.entries()).next().unwrap());
    assert_eq!(&*f.header().path().unwrap(), Path::new("foo/bar"));

    let mut ar = Archive::new(&data[..]);
    t!(ar.unpack(td.path()));
    assert!(fs::metadata(td.path().join("foo/bar")).is_ok());
}

#[cfg(unix)]
#[test]
fn nul_bytes_in_path() {
    use std::os::unix::prelude::*;
    use std::ffi::OsStr;

    let nul_path = OsStr::from_bytes(b"foo\0");
    let td = t!(TempDir::new("tar-rs"));
    let mut ar = Builder::new(Vec::<u8>::new());
    let err = ar.append_dir(nul_path, td.path()).unwrap_err();
    assert!(err.to_string().contains("contains a nul byte"));
}

#[test]
fn links() {
    let mut ar = Archive::new(Cursor::new(tar!("link.tar")));
    let mut entries = t!(ar.entries());
    let link = t!(entries.next().unwrap());
    assert_eq!(t!(link.header().link_name()).as_ref().map(|p| &**p),
               Some(Path::new("file")));
    let other = t!(entries.next().unwrap());
    assert!(t!(other.header().link_name()).is_none());
}

#[test]
#[cfg(unix)] // making symlinks on windows is hard
fn unpack_links() {
    let td = t!(TempDir::new("tar-rs"));
    let mut ar = Archive::new(Cursor::new(tar!("link.tar")));
    t!(ar.unpack(td.path()));

    let md = t!(fs::symlink_metadata(td.path().join("lnk")));
    assert!(md.file_type().is_symlink());
    assert_eq!(&*t!(fs::read_link(td.path().join("lnk"))),
               Path::new("file"));
    t!(File::open(td.path().join("lnk")));
}

#[test]
fn pax_simple() {
    let mut ar = Archive::new(tar!("pax.tar"));
    let mut entries = t!(ar.entries());

    let mut first = t!(entries.next().unwrap());
    let mut attributes = t!(first.pax_extensions()).unwrap();
    let first = t!(attributes.next().unwrap());
    let second = t!(attributes.next().unwrap());
    let third = t!(attributes.next().unwrap());
    assert!(attributes.next().is_none());

    assert_eq!(first.key(), Ok("mtime"));
    assert_eq!(first.value(), Ok("1453146164.953123768"));
    assert_eq!(second.key(), Ok("atime"));
    assert_eq!(second.value(), Ok("1453251915.24892486"));
    assert_eq!(third.key(), Ok("ctime"));
    assert_eq!(third.value(), Ok("1453146164.953123768"));
}
