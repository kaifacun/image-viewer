//! 판 번호 — `MAJOR.MINOR.PATCH` 만, 각 자리는 선행 0 없는 10진수 (모호하면 다운그레이드·무한 제안의 씨앗)

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.split('.');
        let mut part = || -> Option<u32> {
            let p = it.next()?;
            let ok = !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit()) && !(p.len() > 1 && p.starts_with('0'));
            ok.then(|| p.parse().ok()).flatten()
        };
        let v = Version(part()?, part()?, part()?);
        it.next().is_none().then_some(v)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_and_numeric() {
        assert_eq!(Version::parse("0.4.23"), Some(Version(0, 4, 23)));
        assert!(Version::parse("0.4.10").unwrap() > Version::parse("0.4.9").unwrap());
        for bad in ["", "0.4", "0.4.23.1", "0.04.1", "v0.4.1", "0.4.-1", "0.4.1 ", "1.2.x", "9999999.0.0"] {
            assert_eq!(Version::parse(bad), None, "{bad}");
        }
        assert_eq!(Version(1, 0, 0).to_string(), "1.0.0");
    }
}
