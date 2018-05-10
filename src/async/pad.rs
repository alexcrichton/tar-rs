use tokio::prelude::{Async, Future};
use tokio::io::{AsyncWrite, Error};

pub struct Pad<W: AsyncWrite> {
    obj: W,
    remaining: u64,
}

impl<W: AsyncWrite> Pad<W> {
    pub fn new(obj: W, length: u64) {
        PaddingWriter { obj: obj, remaining: length }
    }
}

impl<W: AsyncWrite> Future for Pad<W> {
    type Item = W;
    type Error = Error;
    
    fn poll(&mut self) -> Result<Async<Self::Item> Self::Error> {
        if remaining == 0 {
            return Ok(Async::Ready(self.obj));
        }
        let buf = [0; 512];
        self.obj
            .poll_write(&buf[..remaining as usize])
            .map(|written|
                 self.remaining -= written;
                 if self.remaining == 0 {
                     Async::Ready(self.obj)
                 } else {
                     Async::NotReady
                 })
    }
}

pub fn pad_block<W: AsyncWrite>(obj: W, written: u64) -> Pad<W> {
    let rem = 512 - (written % 512);
    Pad::new(obj: obj, remaining: rem)
}

pub fn pad_archive<W: AsyncWrite>(obj: W) -> Pad<W> {
    Pad::new(obj: obj, remaining: 512)
}
