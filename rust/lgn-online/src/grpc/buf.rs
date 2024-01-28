use bytes::Buf;

pub struct BoxBuf {
    inner: Box<dyn Buf + Send>,
}

impl BoxBuf {
    pub fn new<T: Buf + Send + 'static>(inner: T) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }
}

impl Buf for BoxBuf {
    fn remaining(&self) -> usize {
        self.inner.remaining()
    }

    fn chunk(&self) -> &[u8] {
        self.inner.chunk()
    }

    fn advance(&mut self, cnt: usize) {
        self.inner.advance(cnt);
    }
}
