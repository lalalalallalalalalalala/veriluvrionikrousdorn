//! Единственная учётная запись. Пароль, если он задан, хранится как
//! SHA-256 от соли и пароля. Второй пользователь не предусмотрен.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub const ACCOUNT_PATH: &str = "/System/account";
pub const PBKDF2_ITERS: u32 = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub name: String,
    pub salt: String,
    pub hash: String,
}

impl Account {
    pub fn password_required(&self) -> bool {
        self.hash != "-" && !self.hash.is_empty()
    }

    pub fn verify(&self, password: &str) -> bool {
        if !self.password_required() {
            return password.is_empty();
        }
        let salt = decode_hex(&self.salt);
        if let Some(rest) = self.hash.strip_prefix("pbkdf2:") {
            let mut parts = rest.split(':');
            let iters: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
            let want = parts.next().unwrap_or("");
            return iters > 0 && want == hex(&pbkdf2(password.as_bytes(), &salt, iters));
        }
        self.hash == hex(&digest(&salt, password.as_bytes()))
    }

    pub fn needs_upgrade(&self) -> bool {
        self.password_required() && !self.hash.starts_with("pbkdf2:")
    }
}

pub fn seal(name: &str, password: &str, salt: [u8; 8]) -> Account {
    let hash = if password.is_empty() {
        String::from("-")
    } else {
        alloc::format!(
            "pbkdf2:{}:{}",
            PBKDF2_ITERS,
            hex(&pbkdf2(password.as_bytes(), &salt, PBKDF2_ITERS))
        )
    };
    Account {
        name: String::from(name),
        salt: hex(&salt),
        hash,
    }
}

pub fn upgrade(account: &Account, password: &str) -> Account {
    let salt = decode_hex(&account.salt);
    let mut salt_bytes = [0u8; 8];
    for (i, byte) in salt.iter().take(8).enumerate() {
        salt_bytes[i] = *byte;
    }
    seal(&account.name, password, salt_bytes)
}

pub fn encode(account: &Account) -> String {
    alloc::format!(
        "name={}\nsalt={}\nhash={}\n",
        account.name, account.salt, account.hash
    )
}

pub fn parse(text: &str) -> Option<Account> {
    let mut name = None;
    let mut salt = None;
    let mut hash = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("name=") {
            name = Some(String::from(value.trim()));
        } else if let Some(value) = line.strip_prefix("salt=") {
            salt = Some(String::from(value.trim()));
        } else if let Some(value) = line.strip_prefix("hash=") {
            hash = Some(String::from(value.trim()));
        }
    }
    let name = name?;
    if !valid_name(&name) {
        return None;
    }
    Some(Account {
        name,
        salt: salt.unwrap_or_else(|| String::from("00")),
        hash: hash.unwrap_or_else(|| String::from("-")),
    })
}

pub fn valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    (1..=16).contains(&bytes.len())
        && bytes.iter().all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

pub fn home_of(name: &str) -> String {
    alloc::format!("/Users/{name}")
}

fn pbkdf2(password: &[u8], salt: &[u8], iters: u32) -> [u8; 32] {
    let mut block = Vec::with_capacity(salt.len() + 4);
    block.extend_from_slice(salt);
    block.extend_from_slice(&1u32.to_be_bytes());
    let mut u = hmac_sha256(password, &block);
    let mut t = u;
    for _ in 1..iters.max(1) {
        u = hmac_sha256(password, &u);
        for i in 0..32 {
            t[i] ^= u[i];
        }
    }
    t
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&sha256(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0u8; 64];
    let mut opad = [0u8; 64];
    for i in 0..64 {
        ipad[i] = k[i] ^ 0x36;
        opad[i] = k[i] ^ 0x5c;
    }
    let mut inner = Vec::with_capacity(64 + message.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(message);
    let inner_hash = sha256(&inner);
    let mut outer = Vec::with_capacity(96);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner_hash);
    sha256(&outer)
}

fn digest(salt: &[u8], password: &[u8]) -> [u8; 32] {
    let mut data = Vec::with_capacity(salt.len() + password.len());
    data.extend_from_slice(salt);
    data.extend_from_slice(password);
    sha256(&data)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::new();
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0F) as usize] as char);
    }
    out
}

fn decode_hex(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let hi = hex_val(bytes[i]);
        let lo = hex_val(bytes[i + 1]);
        if let (Some(hi), Some(lo)) = (hi, lo) {
            out.push((hi << 4) | lo);
        }
        i += 2;
    }
    out
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn sha256(message: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (message.len() as u64) * 8;
    let mut data = message.to_vec();
    data.push(0x80);
    while (data.len() % 64) != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in data.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut a = state[0];
        let mut b = state[1];
        let mut c = state[2];
        let mut d = state[3];
        let mut e = state[4];
        let mut f = state[5];
        let mut g = state[6];
        let mut h = state[7];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
        state[4] = state[4].wrapping_add(e);
        state[5] = state[5].wrapping_add(f);
        state[6] = state[6].wrapping_add(g);
        state[7] = state[7].wrapping_add(h);
    }
    let mut out = [0u8; 32];
    for (i, word) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_of_abc() {
        let dig = sha256(b"abc");
        assert_eq!(
            hex(&dig),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn empty_password_is_not_a_secret() {
        let account = seal("nia", "", [1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(!account.password_required());
        assert!(account.verify(""));
        assert!(!account.verify("x"));
        let stored = encode(&account);
        let parsed = parse(&stored).unwrap();
        assert_eq!(parsed.name, "nia");
        assert!(parsed.verify(""));
    }

    #[test]
    fn salted_password_roundtrip() {
        let account = seal("nia", "pitaya", [9, 8, 7, 6, 5, 4, 3, 2]);
        assert!(account.verify("pitaya"));
        assert!(!account.verify("pitaya "));
        assert!(parse(&encode(&account)).unwrap().verify("pitaya"));
        assert!(parse("name=bad name").is_none());
    }

    #[test]
    fn legacy_sha256_still_verifies_and_upgrades() {
        let salt = [9u8, 8, 7, 6, 5, 4, 3, 2];
        let legacy = Account {
            name: String::from("nia"),
            salt: hex(&salt),
            hash: hex(&digest(&salt, b"pitaya")),
        };
        assert!(legacy.needs_upgrade());
        assert!(legacy.verify("pitaya"));
        let next = upgrade(&legacy, "pitaya");
        assert!(!next.needs_upgrade());
        assert!(next.hash.starts_with("pbkdf2:"));
        assert!(next.verify("pitaya"));
        assert!(!next.verify("nope"));
    }
}
