//! Tokens: up to 12 characters of `0-9a-z_` packed into a u64, base 38, the first
//! character in the lowest digit.

const CHARS: &[u8; 38] = b"\x000123456789abcdefghijklmnopqrstuvwxyz_";

#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Token(pub u64);

impl Token {
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut v = self.0;
        while v > 0 {
            let c = CHARS[(v % 38) as usize];
            if c != 0 {
                write!(f, "{}", c as char)?;
            }
            v /= 38;
        }
        Ok(())
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

impl std::str::FromStr for Token {
    type Err = String;
    fn from_str(s: &str) -> Result<Token, String> {
        if s.len() > 12 {
            return Err(format!("token too long: {s}"));
        }
        let mut v = 0u64;
        for c in s.bytes().rev() {
            let i = CHARS
                .iter()
                .position(|&x| x == c && c != 0)
                .ok_or_else(|| format!("bad token character in {s}"))?;
            v = v * 38 + i as u64;
        }
        Ok(Token(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for s in ["", "euro2", "ger1", "road_city_02", "a"] {
            let t: Token = s.parse().unwrap();
            assert_eq!(t.to_string(), s);
        }
    }
}
