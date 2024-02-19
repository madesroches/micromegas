use bytes::Buf;

pub struct BytesChunkReader {
    pub bytes: bytes::Bytes,
}

impl parquet::file::reader::Length for BytesChunkReader {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
}

impl parquet::file::reader::ChunkReader for BytesChunkReader {
    type T = bytes::buf::Reader<bytes::Bytes>;
    fn get_read(
        &self,
        start: u64,
        length: usize,
    ) -> Result<Self::T, parquet::errors::ParquetError> {
        Ok(self
            .bytes
            .slice(start as usize..start as usize + length)
            .reader())
    }
}
