use tokio::io::{AsyncWrite, Error, WriteAll, write_all};
use tokio::prelude::future::{Future, Async};
use Header;

pub struct HeaderBlockWriter<W: AsyncWrite> {
    inner: WriteAll<W, AsRef<[u8]>>
}

impl<W: AsyncWrite> HeaderBlockWriter<W> {
   pub fn new<P: AsRef<Path>>(obj: W, header: Header) -> HeaderWriter<W> {
       HeaderWriter { inner: write_all(obj, h.as_bytes()) }
   }
}

impl<W: AsyncWrite> Future for HeaderBlockWriter<W> {
    type Item = W;
    type Error = io::Error;

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error>> {
        match self.inner.poll() {
            Async::Ready((inner,_)) => Async::Ready(inner),
            _ => Async::NotReady
        }
    }
}
