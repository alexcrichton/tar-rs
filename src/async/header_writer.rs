use tokio::prelude::future::{Future, Then};
use tokio::io::{AsyncWrite, Error, WriteAll, write_all};
use header_block_writer::HeaderBlockWriter;
use {EntryType, Header};
use header::{bytes2path, path2bytes};
use std::borrow::Cow;
use std::path::Path;

pub enum NeededHeaders {
    One(Header)
    Two(Header, Header),
}

enum HeaderWriterState {
    One(HeaderBlockWriter<W>)
    Two(Then<HeaderBlockWriter<W>, HeaderBlockWriter<W>>),
}

pub struct HeaderWriter<W: AsyncWrite> {
    state: HeaderWriterState<W>
}

impl NeededHeaders {
  fn new(header: Header, path: &path) -> io::Result<NeededHeaders> {
     // Try to encode the path directly in the header, but if it ends up not
      // working (e.g. it's too long) then use the GNU-specific long name
      // extension by emitting an entry which indicates that it's the filename
      if let Err(e) = header.set_path(path) {
          let data = path2bytes(&path)?;
          let max = header.as_old().name.len();
          if data.len() < max {
              return Err(e)
          }
          let mut header2 = Header::new_gnu();
          header2.as_gnu_mut().unwrap().name[..13].clone_from_slice(b"././@LongLink");
          header2.set_mode(0o644);
          header2.set_uid(0);
          header2.set_gid(0);
          header2.set_mtime(0);
          header2.set_size((data.len() + 1) as u64);
          header2.set_entry_type(EntryType::new(b'L'));
          header2.set_cksum();
          // Truncate the path to store in the header we're about to emit to
          // ensure we've got something at least mentioned.
          let path = bytes2path(Cow::Borrowed(&data[..max]))?;
          header.set_path(&path)?;
          Ok(NeededHeaders::Two(header2, header))
      } else {
          Ok(NeededHeaders::One(header))
      }
  }
}


impl<W: AsyncWrite> HeaderWriter {
    pub fn new<P: AsRef<Path>>(obj: W, header: Header, path: P) -> io::Error<HeaderWriter<W>> {
        new(NeededHeaders::new(header, path))
    }
    pub fn new(n: NeededHeaders) -> io::Error<HeaderWriter<W>> {
        let state = match n {
            NeededHeaders::One(h) =>
                HeaderWriterState::One(HeaderBlockWriter::new(obj, h)),
            NeededHeaders::Two(h1, h2) =>
                HeaderWriterState::Two(
                    HeaderBlockWriter::new(obj, h1)
                        .and_then(|obj| -> HeaderBlockWriter::new(obj, h2))
                ),
        }
        HeaderWriter { state = state }
    }
}

impl<W: AsyncWrite> Future for HeaderWriter<W> {
    type Item = W;
    type Error = io::Error;

    fn poll(&mut self) -> Result<Async<Item>, Error>> {
        match self.state {
            HeaderWriterState::One(f) => f.poll(),
            HeaderWriterState::Two(f) => f.poll(),
        }
    }
}
