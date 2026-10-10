//! Little-endian reader over a file's bytes.

use super::token::Token;
use anyhow::{bail, Result};

pub struct Reader<'a> {
    data: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Reader<'a> {
        Reader { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.remaining() {
            bail!(
                "unexpected end of file at {} (need {n} bytes, {} left)",
                self.pos,
                self.remaining()
            );
        }
        let b = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(b)
    }

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }

    fn arr<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.bytes(N)?.try_into().unwrap())
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.arr()?))
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.arr()?))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.arr()?))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.arr()?))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.arr()?))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.arr()?))
    }
    pub fn token(&mut self) -> Result<Token> {
        Ok(Token(self.u64()?))
    }

    /// A count that has to fit in what is left of the file, `size` bytes per element: a
    /// misread count fails here instead of allocating gigabytes.
    pub fn count(&mut self, size: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(size) > self.remaining() {
            bail!("count {n} at {} does not fit in the file", self.pos - 4);
        }
        Ok(n)
    }

    pub fn u64s(&mut self) -> Result<Vec<u64>> {
        let n = self.count(8)?;
        (0..n).map(|_| self.u64()).collect()
    }

    pub fn tokens(&mut self) -> Result<Vec<Token>> {
        let n = self.count(8)?;
        (0..n).map(|_| self.token()).collect()
    }

    /// UTF-8 with a u64 length.
    pub fn string(&mut self) -> Result<String> {
        let n = self.u64()?;
        if n > self.remaining() as u64 {
            bail!(
                "string length {n} at {} does not fit in the file",
                self.pos - 8
            );
        }
        Ok(String::from_utf8_lossy(self.bytes(n as usize)?).into_owned())
    }
}
