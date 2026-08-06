//! 元数据匹配打分。
//!
//! - `normalize`：`OpenCC` 繁简转换与名称归一化，入库和查询共用
//! - `types`：匹配档位枚举
//! - `compare`：单个字段（歌名 / 歌手）的比对
//! - `score`：加权汇总成 [`MatchType`]
//! - `query`：把 [`SearchQuery`](crate::core::models::SearchQuery) 预处理成 [`PreparedQuery`]
//! - `rough`：打分前的粗筛

mod compare;
mod normalize;
mod query;
mod rough;
mod score;
mod types;

#[cfg(test)]
mod test_support;

pub use normalize::{
    convert_tw2s,
    normalize_name_for_comparison,
};
pub use query::PreparedQuery;
pub use rough::rough_match;
pub use score::score_entry;
pub use types::MatchType;
