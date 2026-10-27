//! Bit reader for Bink bitstreams.
//!
//! Bink packs its fields into little-endian 32-bit words and consumes every
//! word from the least significant bit upwards, so bit `n` of the stream is
//! bit `n % 8` of byte `n / 8`. Reads past the end yield zero bits.

#[derive(Clone)]
pub(crate) struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// Current position in bits.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Up to 57 upcoming bits, least significant first.
    #[inline]
    fn window(&self) -> u64 {
        let byte = self.pos >> 3;
        let word = match self.data.get(byte..byte + 8) {
            Some(b) => u64::from_le_bytes(b.try_into().unwrap()),
            None => {
                let mut b = [0u8; 8];
                if byte < self.data.len() {
                    let tail = &self.data[byte..];
                    b[..tail.len()].copy_from_slice(tail);
                }
                u64::from_le_bytes(b)
            }
        };
        word >> (self.pos & 7)
    }

    /// The next `n` (at most 32) bits without consuming them.
    #[inline]
    pub fn peek(&self, n: u32) -> u32 {
        (self.window() & ((1u64 << n) - 1)) as u32
    }

    #[inline]
    pub fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    #[inline]
    pub fn read(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.pos += n as usize;
        v
    }

    #[inline]
    pub fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    /// Skips to the next multiple of 32 bits.
    pub fn align32(&mut self) {
        self.pos = (self.pos + 31) & !31;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsb_first() {
        // 0b1011_0110, 0b0000_0001
        let data = [0xB6, 0x01];
        let mut b = Bits::new(&data);
        assert!(!b.bit());
        assert_eq!(b.read(3), 0b011);
        assert_eq!(b.read(6), 0b01_1011);
        assert_eq!(b.position(), 10);
        // Past the end: zero bits.
        assert_eq!(b.read(8), 0);
        assert_eq!(b.position(), 18);
    }

    #[test]
    fn align() {
        let data = [0xFF; 8];
        let mut b = Bits::new(&data);
        b.read(5);
        b.align32();
        assert_eq!(b.position(), 32);
        b.align32();
        assert_eq!(b.position(), 32);
    }
}
