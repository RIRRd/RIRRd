//! Verification for existing BSD MD5-crypt credentials (`MD5-PW`).

use md5::{Digest, Md5};
use subtle::ConstantTimeEq;

const MAGIC: &str = "$1$";
const MAX_PASSWORD_BYTES: usize = 72;
const MAX_SALT_BYTES: usize = 8;
const DIGEST_CHARS: usize = 22;
const ROUNDS: usize = 1_000;
const PERMUTATION: [[usize; 3]; 5] = [[0, 6, 12], [1, 7, 13], [2, 8, 14], [3, 9, 15], [4, 10, 5]];

pub fn valid_hash(hash: &str) -> bool {
    parse(hash).is_some()
}

pub fn verify(password: &str, hash: &str) -> bool {
    let password = password.as_bytes();
    if password.len() > MAX_PASSWORD_BYTES || password.contains(&0) {
        return false;
    }
    let Some((salt, expected)) = parse(hash) else {
        return false;
    };
    let actual = digest(password, salt);
    bool::from(actual.ct_eq(&expected))
}

fn crypt_digit(byte: u8) -> Option<u8> {
    match byte {
        b'.' => Some(0),
        b'/' => Some(1),
        b'0'..=b'9' => Some(byte - b'0' + 2),
        b'A'..=b'Z' => Some(byte - b'A' + 12),
        b'a'..=b'z' => Some(byte - b'a' + 38),
        _ => None,
    }
}

fn parse(hash: &str) -> Option<(&[u8], [u8; 16])> {
    let fixed = MAGIC.len() + 1 + DIGEST_CHARS;
    if !(fixed + 1..=fixed + MAX_SALT_BYTES).contains(&hash.len()) {
        return None;
    }
    let (salt, encoded) = hash.strip_prefix(MAGIC)?.split_once('$')?;
    let salt = salt.as_bytes();
    if !(1..=MAX_SALT_BYTES).contains(&salt.len())
        || !salt.iter().all(|byte| crypt_digit(*byte).is_some())
    {
        return None;
    }
    let encoded: &[u8; DIGEST_CHARS] = encoded.as_bytes().try_into().ok()?;
    let mut digest = [0; 16];
    for (group, [high, middle, low]) in encoded[..20].as_chunks::<4>().0.iter().zip(PERMUTATION) {
        let digits = group.iter().rev().try_fold(0_u32, |bits, byte| {
            Some((bits << 6) | u32::from(crypt_digit(*byte)?))
        })?;
        let [_, a, b, c] = digits.to_be_bytes();
        digest[high] = a;
        digest[middle] = b;
        digest[low] = c;
    }
    let high = crypt_digit(encoded[21])?;
    if high > 3 {
        return None; // Unused high bits must be zero in the final character.
    }
    digest[11] = (high << 6) | crypt_digit(encoded[20])?;
    Some((salt, digest))
}

fn digest(password: &[u8], salt: &[u8]) -> [u8; 16] {
    let alternate = Md5::new()
        .chain_update(password)
        .chain_update(salt)
        .chain_update(password)
        .finalize();

    let mut seed = Md5::new()
        .chain_update(password)
        .chain_update(MAGIC)
        .chain_update(salt);
    let copies = password.len() / alternate.len();
    let remainder = password.len() % alternate.len();
    for _ in 0..copies {
        seed.update(alternate);
    }
    seed.update(&alternate[..remainder]);

    // The BSD construction mixes one byte for every bit of the byte count.
    let mut count = password.len();
    while count != 0 {
        seed.update([if count & 1 == 0 { password[0] } else { 0 }]);
        count >>= 1;
    }
    let mut result = seed.finalize();

    for round in 0..ROUNDS {
        let mut state = Md5::new();
        let (first, last) = if round & 1 == 0 {
            (result.as_slice(), password)
        } else {
            (password, result.as_slice())
        };
        state.update(first);
        if round % 3 != 0 {
            state.update(salt);
        }
        if round % 7 != 0 {
            state.update(password);
        }
        state.update(last);
        result = state.finalize();
    }
    result.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifies_documented_and_independently_generated_openssl_vectors() {
        // First vector is documented in openssl-passwd(1). The rest were
        // generated using OpenSSL 3.5.9 passwd -1 with explicit salts.
        for (password, hash) in [
            ("password", "$1$xxxxxxxx$UYCIxa628.9qXjpQCjM4a."),
            ("password", "$1$salt$qJH7.N4xYta3aEG/dfqo/0"),
            ("Hello world!", "$1$hfT7jp2q$18SNspT8ZylzBoP0v.ry9."),
            ("p\u{e4}ssw\u{f6}rd", "$1$T/a.0$a5u2.m7HNVS.br8qZZei2."),
            ("", "$1$s$iXWZU0LDEJ7Jjmms7KFsH0"),
        ] {
            assert!(valid_hash(hash));
            assert!(verify(password, hash));
            assert!(!verify("a different synthetic password", hash));
        }
    }

    #[test]
    fn enforces_byte_limits_without_truncating_passwords() {
        let hash = "$1$12345678$Lwb4t.KTdgiau9SL7eti60";
        let password = "A".repeat(72);
        assert!(verify(&password, hash));
        assert!(!verify(&(password + "A"), hash));
        assert!(!verify(
            "password\0suffix",
            "$1$salt$qJH7.N4xYta3aEG/dfqo/0"
        ));
    }

    #[test]
    fn rejects_noncanonical_and_malformed_hashes() {
        for hash in [
            "$apr1$salt$qJH7.N4xYta3aEG/dfqo/0",
            "$1$$qJH7.N4xYta3aEG/dfqo/0",
            "$1$123456789$qJH7.N4xYta3aEG/dfqo/0",
            "$1$sa:lt$qJH7.N4xYta3aEG/dfqo/0",
            "$1$sa\u{e4}lt$qJH7.N4xYta3aEG/dfqo/0",
            "$1$salt$qJH7.N4xYta3aEG/dfqo/2",
            "$1$salt$qJH7.N4xYta3aEG/dfqo+0",
            "$1$salt$qJH7.N4xYta3aEG/dfqo/",
            "$1$salt$qJH7.N4xYta3aEG/dfqo/0\n",
        ] {
            assert!(!valid_hash(hash), "{hash}");
            assert!(!verify("password", hash));
        }
    }
}
