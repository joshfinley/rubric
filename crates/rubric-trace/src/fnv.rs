//! FNV-1a 64-bit, the hash behind every seal.

const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

pub fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h = OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_offset() {
        assert_eq!(fnv1a_64(b""), OFFSET);
    }

    #[test]
    fn known_vector_a() {
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn deterministic_across_calls() {
        assert_eq!(fnv1a_64(b"parser::header_magic"), fnv1a_64(b"parser::header_magic"));
        assert_ne!(fnv1a_64(b"parser::header_magic"), fnv1a_64(b"parser::header_magix"));
    }
}
