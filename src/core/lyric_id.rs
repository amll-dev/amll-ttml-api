use std::{
    fmt,
    str::FromStr,
};

use serde::{
    Deserialize,
    Serialize,
};

use crate::core::error::AppError;

/// 歌词 ID 类型，封装 53 位 JavaScript Safe Integer 安全区间 `(0 ..= 9,007,199,254,740,991)`
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LyricId(u64);

impl LyricId {
    /// 53 位安全整数的最大值: 2^53 - 1 = `9_007_199_254_740_991` (`0x001F_FFFF_FFFF_FFFF`)
    pub const MAX: u64 = 0x001F_FFFF_FFFF_FFFF;

    const MASK: u64 = Self::MAX;
    const MIN: u64 = 0;

    /// 从裸 u64 构造 [`LyricId`]
    ///
    /// # Errors
    /// 如果 u64 不在 `0` 和 `9_007_199_254_740_991` 范围内，将会返回错误
    pub fn from_u64(val: u64) -> Result<Self, AppError> {
        if val <= Self::MAX {
            Ok(Self(val))
        } else {
            Err(AppError::BadRequest(format!(
                "Invalid ID format. Must be an unsigned integer between {} and {}.",
                Self::MIN,
                Self::MAX
            )))
        }
    }

    /// 从裸 u64 按 53 位掩码截断构造
    #[must_use]
    pub const fn from_u64_masked(val: u64) -> Self {
        Self(val & Self::MASK)
    }

    /// 从文件名生成 53 位唯一的 [`LyricId`]
    ///
    /// 采用 FNV-1a 64-bit 哈希并按 53 位掩码截断
    #[allow(clippy::unreadable_literal)]
    pub fn from_filename(filename: &str) -> Self {
        const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
        const FNV_PRIME: u64 = 0x100000001b3;

        let mut hash = FNV_OFFSET_BASIS;
        for byte in filename.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }

        Self::from_u64_masked(hash)
    }

    /// 从字符串解析并校验 53 位边界
    pub fn parse(s: &str) -> Result<Self, AppError> {
        s.parse::<u64>().map_or_else(
            |_| {
                Err(AppError::BadRequest(format!(
                    "Invalid ID format. Must be an unsigned integer between {} and {}.",
                    Self::MIN,
                    Self::MAX
                )))
            },
            Self::from_u64,
        )
    }

    /// 获取底层的 u64 原始数值
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<LyricId> for u64 {
    fn from(id: LyricId) -> Self {
        id.0
    }
}

impl fmt::Display for LyricId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for LyricId {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_lyric_id_creation_and_get() {
        let id = LyricId::from_u64(12345).unwrap();
        assert_eq!(id.get(), 12345);
        assert_eq!(u64::from(id), 12345);
    }

    #[test]
    fn test_max_safe_integer_boundary() {
        let max_id = LyricId::from_u64(LyricId::MAX).unwrap();
        assert_eq!(max_id.get(), 9_007_199_254_740_991);

        let over_boundary = LyricId::from_u64(LyricId::MAX + 1);
        assert!(over_boundary.is_err());
    }

    #[test]
    fn test_from_u64_masked() {
        let masked = LyricId::from_u64_masked(u64::MAX);
        assert_eq!(masked.get(), LyricId::MAX);
    }

    #[test]
    fn test_parse_valid_and_invalid() {
        let parsed = LyricId::parse("269710089745311").unwrap();
        assert_eq!(parsed.get(), 269_710_089_745_311);

        let invalid_str = LyricId::parse("abc");
        assert!(invalid_str.is_err());

        let overflow = LyricId::parse("9007199254740992");
        assert!(overflow.is_err());
    }

    #[test]
    fn test_from_filename_deterministic() {
        let id1 = LyricId::from_filename("test.ttml");
        let id2 = LyricId::from_filename("test.ttml");
        assert_eq!(id1, id2);
        assert!(id1.get() <= LyricId::MAX);
    }
}
