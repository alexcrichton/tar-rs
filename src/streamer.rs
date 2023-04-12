#[cfg(unix)]
use std::os::unix::prelude::*;
use crate::other;

#[cfg(windows)]
use std::os::windows::prelude::*;

use std::io::{self, Read, Result, SeekFrom, Seek};
use std::path::{PathBuf, Path};
use std::str;
use std::collections::HashMap;
use std::fs::{self};

#[cfg(windows)]
use crate::other;
use crate::header::{HeaderMode, Header};
use crate::{EntryType};

pub struct StreamFile {
    encoded_header: Vec<u8>,
	follow: bool,
	path: PathBuf,
    read_bytes: usize, //needed to calculate padding;
    padding_bytes: Option<Vec<u8>>,
}

impl StreamFile {
	pub fn new_with_encoded_header(path: PathBuf, encoded_header: Vec<u8>, follow: bool) -> StreamFile {
		Self {
			path,
			encoded_header,
			follow,
            read_bytes: 0,
            padding_bytes: None
		}
	}
}

pub struct StreamData {
	encoded_header: Vec<u8>,
	data: Box<dyn Read>,
    padding_bytes: Option<Vec<u8>>,
    read_bytes: usize, //needed to calculate padding;
}

impl StreamData {
    fn new<R: Read + 'static>(header: Header, data: R) -> StreamData {
        Self {
            encoded_header: header.as_bytes().to_vec(),
            data: Box::new(data),
            padding_bytes: None,
            read_bytes: 0,
        }
    }

    // This method may be used with long name extension entries.
    fn new_with_encoded_header<R: Read + 'static>(encoded_header: Vec<u8>, data: R) -> StreamData {
        Self {
            encoded_header,
            data: Box::new(data),
            padding_bytes: None,
            read_bytes: 0,
        }
    }
}

#[cfg(unix)]
pub struct StreamSpecialFile {
    encoded_header: Vec<u8>,
}

#[cfg(unix)]
impl StreamSpecialFile {
    fn new_with_encoded_header(encoded_header: Vec<u8>) -> Self {
        Self {
            encoded_header,
        }
    }
}

pub struct StreamLink {
    encoded_header: Vec<u8>,
}

impl StreamLink {
    fn new_with_encoded_header(encoded_header: Vec<u8>) -> Self {
        Self {
            encoded_header,
        }
    }
}

pub struct StreamerReadMetadata {
    read_bytes: usize,
    current_index: usize,
    finish_bytes_remaining: usize,
}

impl Default for StreamerReadMetadata {
    fn default() -> Self {
        Self {
            read_bytes: 0,
            current_index: 0,
            finish_bytes_remaining: 1024,
        }
    }
}

/// A structure for building and streaming archives
///
/// This structure has methods for building up an archive and implements [std::io::Read] for this archive.
pub struct Streamer {
	mode: HeaderMode,
	follow: bool,
	streamer_metadata: StreamerReadMetadata,
    index_counter: usize,
	stream_files: HashMap<usize, StreamFile>, // <index_counter, StreamFile>
	stream_data: HashMap<usize, StreamData>, // <index_counter, StreamData>
    stream_special_file: HashMap<usize, StreamSpecialFile>, //<index_counter, StreamSpecialFile>
    stream_link: HashMap<usize, StreamLink>, // <index_counter, StreamLink>
}

impl Default for Streamer {
    fn default() -> Self {
        Self::new()
    }
}

impl Read for Streamer {
    fn read(&mut self, buffer: &mut [u8]) -> std::result::Result<usize, std::io::Error> {
        let mut read_bytes = 0;
        'outer: loop {
          // end of archive reached, if there are remaining finish bytes, we should read them :)
            if self.streamer_metadata.current_index > self.index_counter {
                if self.streamer_metadata.finish_bytes_remaining > 0 {
                    if buffer[read_bytes..].len() > self.streamer_metadata.finish_bytes_remaining {
                        let finishing_bytes = vec![0u8; self.streamer_metadata.finish_bytes_remaining];
                        self.streamer_metadata.finish_bytes_remaining -= finishing_bytes.len();
                        buffer[read_bytes..read_bytes+finishing_bytes.len()].copy_from_slice(&finishing_bytes);
                        read_bytes += finishing_bytes.len();
                    } else {
                        self.streamer_metadata.finish_bytes_remaining -= buffer[read_bytes..].len();
                        let finishing_bytes = vec![0u8; buffer[read_bytes..].len()];
                        buffer[read_bytes..read_bytes+finishing_bytes.len()].copy_from_slice(&finishing_bytes);
                        read_bytes += finishing_bytes.len();
                    }
                }
                break;
            }
            
            if let Some(stream_file) = self.stream_files.get_mut(&self.streamer_metadata.current_index) {
                //read the header first...
                if stream_file.encoded_header.len() > buffer[read_bytes..].len() {
                    let drained_bytes: Vec<u8> = stream_file.encoded_header.drain(..buffer[read_bytes..].len()).collect();
                    buffer[read_bytes..read_bytes+drained_bytes.len()].copy_from_slice(&drained_bytes);
                    read_bytes += drained_bytes.len();
                    break;
                } else {
                    let drained_bytes: Vec<u8> = stream_file.encoded_header.drain(..).collect();
                    buffer[read_bytes..read_bytes+drained_bytes.len()].copy_from_slice(&drained_bytes);
                    read_bytes += drained_bytes.len();
                }

                //...then read the appropriate data
                loop {
                    if read_bytes == buffer.len() {
                        // breaks the outer-loop to skip the update of current_index attribute, as EOF of data is not reached yet.
                        break 'outer;
                    }
                    if let Some(ref mut padding_bytes) = stream_file.padding_bytes {
                        if padding_bytes.len() > buffer[read_bytes..].len() {
                            let drained_bytes: Vec<u8> = padding_bytes.drain(..buffer[read_bytes..].len()).collect();
                            buffer[read_bytes..read_bytes+drained_bytes.len()].copy_from_slice(&drained_bytes);
                            read_bytes += drained_bytes.len();
                            break 'outer;
                        } else {
                            let drained_bytes: Vec<u8> = padding_bytes.drain(..).collect();
                            buffer[read_bytes..read_bytes+drained_bytes.len()].copy_from_slice(&drained_bytes);
                            read_bytes += drained_bytes.len();
                            break;
                        }
                    } else {
                        let stat = get_stat(&stream_file.path, stream_file.follow)?;
                        if !stat.is_file() {
                            break;
                        }
                        let mut file = fs::File::open(&stream_file.path)?;
                        file.seek(SeekFrom::Start(stream_file.read_bytes as u64))?;
                        let r = file.read(&mut buffer[read_bytes..])?;
                        stream_file.read_bytes += r;
                        if r == 0 {
                            // EOF of inner data is reached, so we continue the outer-loop to skip the update of current_index as we have to
                            // read the padding bytes first, if necessary.
                            let remaining = 512 - (stream_file.read_bytes % 512);
                            if remaining < 512 {
                                stream_file.padding_bytes = Some(vec![0u8; remaining]);
                                continue 'outer;
                            } else {
                                break;
                            }
                        }
                        read_bytes += r;
                    }     
                }
            }

            if let Some(stream_data) = self.stream_data.get_mut(&self.streamer_metadata.current_index) {
                //read the header first...
                if stream_data.encoded_header.len() > buffer[read_bytes..].len() {
                    let drained_bytes: Vec<u8> = stream_data.encoded_header.drain(..buffer[read_bytes..].len()).collect();
                    buffer[read_bytes..read_bytes+drained_bytes.len()].copy_from_slice(&drained_bytes);
                    read_bytes += drained_bytes.len();
                    break;
                } else {
                    let drained_bytes: Vec<u8> = stream_data.encoded_header.drain(..).collect();
                    buffer[read_bytes..read_bytes+drained_bytes.len()].copy_from_slice(&drained_bytes);
                    read_bytes += drained_bytes.len();
                }

                //...then read the appropriate data
                loop {
                    if read_bytes == buffer.len() {
                        // breaks the outer-loop to skip the update of current_index attribute, as EOF of data is not reached yet.
                        break 'outer;
                    }
                    if let Some(ref mut padding_bytes) = stream_data.padding_bytes {
                        if padding_bytes.len() > buffer[read_bytes..].len() {
                            let drained_bytes: Vec<u8> = padding_bytes.drain(..buffer[read_bytes..].len()).collect();
                            buffer[read_bytes..read_bytes+drained_bytes.len()].copy_from_slice(&drained_bytes);
                            read_bytes += drained_bytes.len();
                            break 'outer;
                        } else {
                            let drained_bytes: Vec<u8> = padding_bytes.drain(..).collect();
                            buffer[read_bytes..read_bytes+drained_bytes.len()].copy_from_slice(&drained_bytes);
                            read_bytes += drained_bytes.len();
                            break;
                        }
                    } else {
                        let r = stream_data.data.read(&mut buffer[read_bytes..])?;
                        stream_data.read_bytes += r;
                        if r == 0 {
                            // EOF of inner data is reached, so we continue the outer-loop to skip the update of current_index as we have to
                            // read the padding bytes first, if necessary.
                            let remaining = 512 - (stream_data.read_bytes % 512);
                            if remaining < 512 {
                                stream_data.padding_bytes = Some(vec![0u8; remaining]);
                                continue 'outer;
                            } else {
                                break;
                            }
                        }
                        read_bytes += r;
                    }     
                }
            }

            if let Some(stream_special_file) = self.stream_special_file.get_mut(&self.streamer_metadata.current_index) {
                //Zero padding should not necessary here.
                if stream_special_file.encoded_header.len() > buffer[read_bytes..].len() {
                    let drained_bytes: Vec<u8> = stream_special_file.encoded_header.drain(..buffer[read_bytes..].len()).collect();
                    buffer[read_bytes..read_bytes+drained_bytes.len()].copy_from_slice(&drained_bytes);
                    read_bytes += drained_bytes.len();
                    break;
                } else {
                    let drained_bytes: Vec<u8> = stream_special_file.encoded_header.drain(..).collect();
                    buffer[read_bytes..read_bytes+drained_bytes.len()].copy_from_slice(&drained_bytes);
                    read_bytes += drained_bytes.len();
                }
            }

            if let Some(stream_link) = self.stream_link.get_mut(&self.streamer_metadata.current_index) {
                //Zero padding should not necessary here.
                if stream_link.encoded_header.len() > buffer[read_bytes..].len() {
                    let drained_bytes: Vec<u8> = stream_link.encoded_header.drain(..buffer[read_bytes..].len()).collect();
                    buffer[read_bytes..read_bytes+drained_bytes.len()].copy_from_slice(&drained_bytes);
                    read_bytes += drained_bytes.len();
                    break;
                } else {
                    let drained_bytes: Vec<u8> = stream_link.encoded_header.drain(..).collect();
                    buffer[read_bytes..read_bytes+drained_bytes.len()].copy_from_slice(&drained_bytes);
                    read_bytes += drained_bytes.len();
                }
            }
            self.streamer_metadata.current_index += 1;
        }
        self.streamer_metadata.read_bytes += read_bytes;
        Ok(read_bytes)
    }
}



impl Streamer {
	/// Create a new empty archive streamer.The streamer will use
    /// `HeaderMode::Complete` by default.
	pub fn new() -> Streamer {
		Self {
			mode: HeaderMode::Complete,
			follow: true,
			streamer_metadata: StreamerReadMetadata::default(),
            index_counter: 0,
			stream_files: HashMap::new(),
			stream_data: HashMap::new(),
            stream_special_file: HashMap::new(),
            stream_link: HashMap::new(),
		}
	}

	/// Changes the HeaderMode that will be used when reading fs Metadata for
    /// methods that implicitly read metadata for an input Path. Notably, this
    /// does _not_ apply to `append(Header)`.
    pub fn mode(&mut self, mode: HeaderMode) {
        self.mode = mode;
    }

    /// Follow symlinks, archiving the contents of the file they point to rather
    /// than adding a symlink to the archive. Defaults to true.
    pub fn follow_symlinks(&mut self, follow: bool) {
        self.follow = follow;
    }

    pub fn append_link<P: AsRef<Path>, T: AsRef<Path>>(
        &mut self,
        header: &mut Header,
        path: P,
        target: T,
    ) -> io::Result<()> {
        let mut encoded_header = Vec::new();
        if let Some(mut long_name_extension_entry) = prepare_header_path(header, path.as_ref())? {
            encoded_header.append(&mut long_name_extension_entry);
        }
        if let Some(mut long_name_extension_entry) = prepare_header_link(header, target.as_ref())? {
            encoded_header.append(&mut long_name_extension_entry)
        };
        header.set_cksum();
        encoded_header.append(&mut header.as_bytes().to_vec());
        self.stream_link.insert(self.index_counter, StreamLink::new_with_encoded_header(encoded_header));
        self.index_counter += 1;
        Ok(())
    }

    pub fn append<R: Read + 'static>(&mut self, header: Header, data: R) {
        let stream_data = StreamData::new(header, data);
        self.stream_data.insert(self.index_counter, stream_data);
        self.index_counter += 1;
    }

    pub fn append_data<P: AsRef<Path>, R: Read + 'static>(&mut self, header: &mut Header, path: P, data: R) -> Result<()> {
        let mut encoded_header = Vec::new();
        if let Some(mut long_name_extension_entry) = prepare_header_path(header, path.as_ref())? {
            encoded_header.append(&mut long_name_extension_entry);
            //self.long_name_extension_entries.insert(self.index_counter, long_name_extension_entry);
        }
        header.set_cksum();
        encoded_header.append(&mut header.as_bytes().to_vec());
        self.stream_data.insert(self.index_counter, StreamData::new_with_encoded_header(encoded_header, data));
        self.index_counter += 1;
        Ok(())
    }

    pub fn append_stream_data(&mut self, stream_data: StreamData) {
    	self.stream_data.insert(self.index_counter, stream_data);
        self.index_counter += 1;
    }

    pub fn append_path<P: AsRef<Path>>(&mut self, path: P) -> io::Result<()> {
        self.append_stream_file(path.as_ref(), None)
    }

    pub fn append_path_with_name<P: AsRef<Path>, N: AsRef<Path>>(&mut self, path: P, name: N) -> Result<()> {
        self.append_stream_file(path.as_ref(), Some(name.as_ref()))
    }

    pub fn append_dir<P, Q>(&mut self, path: P, src_path: Q) -> io::Result<()>
    where
        P: AsRef<Path>,
        Q: AsRef<Path>,
    {
        self.append_stream_file(src_path.as_ref(), Some(path.as_ref()))
    }

    pub fn append_dir_all(&mut self, path: &Path, src_path: &Path) -> io::Result<()> {
        let mut stack = vec![(src_path.to_path_buf(), true, false)];
        while let Some((src, is_dir, is_symlink)) = stack.pop() {
            let dest = path.join(src.strip_prefix(src_path).unwrap());
            // In case of a symlink pointing to a directory, is_dir is false, but src.is_dir() will return true
            if is_dir || (is_symlink && self.follow && src.is_dir()) {
                for entry in fs::read_dir(&src)? {
                    let entry = entry?;
                    let file_type = entry.file_type()?;
                    stack.push((entry.path(), file_type.is_dir(), file_type.is_symlink()));
                }
                if dest != Path::new("") {
                    self.append_dir(&src, &dest)?;
                }
            } else {
                #[cfg(unix)]
                {
                    let stat = fs::metadata(&src)?;
                    if !stat.is_file() {
                        self.append_special(&dest)?;
                        continue;
                    }
                }
                self.append_stream_file(&dest, Some(&src))?;
            }
        }
        Ok(())
    }

    #[cfg(unix)]
    fn append_special(&mut self, path: &Path) -> io::Result<()> {
        let stat = get_stat(path, self.follow)?;
        
        let file_type = stat.file_type();
        let entry_type;
        if file_type.is_socket() {
            // sockets can't be archived
            return Err(other(&format!(
                "{}: socket can not be archived",
                path.display()
            )));
        } else if file_type.is_fifo() {
            entry_type = EntryType::Fifo;
        } else if file_type.is_char_device() {
            entry_type = EntryType::Char;
        } else if file_type.is_block_device() {
            entry_type = EntryType::Block;
        } else {
            return Err(other(&format!("{} has unknown file type", path.display())));
        }

        let mut encoded_header = Vec::new();
        let mut header = Header::new_gnu();
        header.set_metadata_in_mode(&stat, self.mode);
        if let Some(mut long_name_extension_entry) = prepare_header_path(&mut header, path)? {
            encoded_header.append(&mut long_name_extension_entry);
        }
        header.set_entry_type(entry_type);
        let dev_id = stat.rdev();
        let dev_major = ((dev_id >> 32) & 0xffff_f000) | ((dev_id >> 8) & 0x0000_0fff);
        let dev_minor = ((dev_id >> 12) & 0xffff_ff00) | ((dev_id) & 0x0000_00ff);
        header.set_device_major(dev_major as u32)?;
        header.set_device_minor(dev_minor as u32)?;

        header.set_cksum();
        encoded_header.append(&mut header.as_bytes().to_vec());
        self.stream_special_file.insert(self.index_counter, StreamSpecialFile::new_with_encoded_header(encoded_header));
        self.index_counter +=1;

        Ok(())
    }

    fn append_stream_file(&mut self, path: &Path, name: Option<&Path>) -> Result<()> {
        let stat = get_stat(path, self.follow)?;
        let ar_name = name.unwrap_or(path);

        //generate and prepare appropriate header
        let mut encoded_header = Vec::new();
        let mut header = Header::new_gnu();

        if let Some(mut long_name_extension_entry) = prepare_header_path(&mut header, ar_name)? {
            encoded_header.append(&mut long_name_extension_entry);
        }
        header.set_metadata_in_mode(&stat, self.mode);
        if stat.file_type().is_symlink() {
            let link_name = fs::read_link(path)?;
            if let Some(mut long_name_extension_entry) = prepare_header_link(&mut header, &link_name)? {
                encoded_header.append(&mut long_name_extension_entry);
            }
        }
        header.set_cksum();
        encoded_header.append(&mut header.as_bytes().to_vec());
        let stream_file = StreamFile::new_with_encoded_header(path.to_path_buf(), encoded_header, self.follow);
        self.stream_files.insert(self.index_counter, stream_file);
        self.index_counter += 1;
        Ok(())
    }
}


fn get_stat<P: AsRef<Path>>(path: P, follow: bool) -> io::Result<fs::Metadata> {
    if follow {
        fs::metadata(path.as_ref()).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("{} when getting metadata for {}", err, path.as_ref().display()),
            )
        })
    } else {
        fs::symlink_metadata(path.as_ref()).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!("{} when getting metadata for {}", err, path.as_ref().display()),
            )
        })
    }
}

// function tries to encode the path directly in header.
// Returns an Ok(None) if everything is fine.
// Returns an Ok(Some(StreamData)) as an extra entry to emit the "long file name".
fn prepare_header_path(header: &mut Header, path: &Path) -> Result<Option<Vec<u8>>> {
    // Try to encode the path directly in the header, but if it ends up not
    // working (probably because it's too long) then try to use the GNU-specific
    // long name extension by emitting an entry which indicates that it's the
    // filename.
    let mut extra_entry = None;
    if let Err(e) = header.set_path(path) {
        let data = path2bytes(path)?;
        let max = header.as_old().name.len();
        // Since `e` isn't specific enough to let us know the path is indeed too
        // long, verify it first before using the extension.
        if data.len() < max {
            return Err(e);
        }
        let header2 = prepare_header(data.len() as u64, b'K');
        // null-terminated string
        let mut data2 = data.to_vec();
        data2.push(0);
        let mut entry_data = header2.as_bytes().to_vec();
        entry_data.append(&mut data2);
        extra_entry = Some(entry_data);
        
        // Truncate the path to store in the header we're about to emit to
        // ensure we've got something at least mentioned. Note that we use
        // `str`-encoding to be compatible with Windows, but in general the
        // entry in the header itself shouldn't matter too much since extraction
        // doesn't look at it.
        let truncated = match str::from_utf8(&data[..max]) {
            Ok(s) => s,
            Err(e) => str::from_utf8(&data[..e.valid_up_to()]).unwrap(),
        };
        header.set_path(truncated)?;
    }
    Ok(extra_entry)
}

fn prepare_header_link(header: &mut Header, link_name: &Path) -> Result<Option<Vec<u8>>> {
    // Same as previous function but for linkname
    let mut extra_entry = None;
    if let Err(e) = header.set_link_name(link_name) {
        let data = path2bytes(link_name)?;
        if data.len() < header.as_old().linkname.len() {
            return Err(e);
        }
        let header2 = prepare_header(data.len() as u64, b'L');
        // null-terminated string
        let mut data2 = data.to_vec();
        data2.push(0);
        let mut entry_data = header2.as_bytes().to_vec();
        entry_data.append(&mut data2);
        extra_entry = Some(entry_data);
    }
    Ok(extra_entry)
}

fn prepare_header(size: u64, entry_type: u8) -> Header {
    let mut header = Header::new_gnu();
    let name = b"././@LongLink";
    header.as_gnu_mut().unwrap().name[..name.len()].clone_from_slice(&name[..]);
    header.set_mode(0o644);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    // + 1 to be compliant with GNU tar
    header.set_size(size + 1);
    header.set_entry_type(EntryType::new(entry_type));
    header.set_cksum();
    header
}

//TODO
#[cfg(any(windows, target_arch = "wasm32"))]
pub fn path2bytes(p: &Path) -> std::io::Result<&[u8]> {
    p.as_os_str()
        .to_str()
        .map(|s| s.as_bytes())
        .ok_or_else(|| other(&format!("path {} was not valid Unicode", p.display())))
        .map(|bytes| {
            if bytes.contains(&b'\\') {
                // Normalize to Unix-style path separators
                let mut bytes = bytes.to_owned();
                for b in &mut bytes {
                    if *b == b'\\' {
                        *b = b'/';
                    }
                }
                bytes
            } else {
                bytes.to_vec()
            }
        })
}


#[cfg(unix)]
/// On unix this will never fail
pub fn path2bytes(p: &Path) -> std::io::Result<&[u8]> {
    Ok(p.as_os_str().as_bytes())
}