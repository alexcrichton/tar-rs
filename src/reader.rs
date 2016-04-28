use std::io::{self, Read};
use std::cmp::min;

use EntryType;
use archive::ArchiveInner;


pub enum Reader<'a> {
    Normal(io::Take<&'a ArchiveInner<io::Read + 'a>>),
    Sparse {
        data: &'a ArchiveInner<io::Read + 'a>,
        blocks: Vec<(u64, u64)>,
        block: usize,
        position: u64,
        size: u64,
    },
}

impl<'a> Reader<'a> {
    pub fn new<'x>(typ: EntryType, reader: &'x ArchiveInner<io::Read + 'x>,
        sparse_chunks: Vec<(u64, u64)>, file_size: u64)
        -> Reader<'x>
    {
        match typ {
            EntryType::GNUSparse => {
                Reader::Sparse {
                    data: reader,
                    blocks: sparse_chunks,
                    block: 0,
                    position: 0,
                    size: file_size,
                }
            }
            _ => Reader::Normal(reader.take(file_size)),
        }
    }
}

impl<'a> io::Read for Reader<'a> {
    /// The reader implementation emits sparse file as fully alocated file
    /// that has absent blocks zeroed
    ///
    /// This is non-optimal if you unpack to filesystem but correct and useful
    /// if you feed the file to some streaming parser or whatever.
    fn read(&mut self, mut into: &mut [u8]) -> io::Result<usize> {
        match *self {
            Reader::Normal(ref mut reader) => reader.read(into),
            Reader::Sparse {
                ref mut data, ref blocks, ref mut block,
                ref mut position, size,
            } => {
                let mut bytes_read: usize = 0;
                while into.len() > bytes_read {
                    let dest = &mut into[bytes_read..];
                    if *block >= blocks.len() {
                        // after last block
                        if *position + dest.len() as u64 > size {
                            let bytes = (size - *position) as usize;
                            for i in &mut dest[..bytes] { *i = 0; }
                            *position += bytes as u64;
                            return Ok(bytes_read + bytes);
                        }
                        for i in &mut dest[..] { *i = 0; }
                        *position += dest.len() as u64;
                        return Ok(bytes_read + dest.len());
                    } else if blocks[*block].0 > *position {
                        // before the next block
                        let bytes = min(
                            (blocks[*block].0 - *position) as usize,
                            dest.len());
                        for i in &mut dest[..bytes] { *i = 0; }
                        *position += bytes as u64;
                        bytes_read += bytes;
                    } else {
                        let block_off = *position - blocks[*block].0;
                        debug_assert!(block_off < blocks[*block].1);
                        if block_off + (dest.len() as u64) < blocks[*block].1 {
                            // partially read block
                            let read = try!(data.read(dest));
                            *position += read as u64;
                            return Ok(bytes_read + read);
                        } else {
                            // fully read block
                            debug_assert!(blocks[*block].1 <=
                                block_off + dest.len() as u64);
                            let bytes = (blocks[*block].1 - block_off)
                                as usize;
                            let real = try!(data.read(&mut dest[..bytes]));
                            *position += real as u64;
                            bytes_read += real;
                            if real < bytes {
                                // Partial read is returned as partial read
                                return Ok(bytes_read);
                            } else {
                                *block += 1;
                            }
                        }
                    }
                }
                return Ok(bytes_read);
            }
        }
    }
}
