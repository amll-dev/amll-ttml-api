use serde::Serialize;

use crate::core::error::AppError;

pub const DEFAULT_PAGE: u64 = 1;
pub const DEFAULT_PAGE_SIZE: u64 = 50;
pub const MAX_PAGE_SIZE: u64 = 100;

/// 搜索接口的分页参数。
///
/// `page` 从 1 起算，`page_size` 上限为 [`MAX_PAGE_SIZE`]
///
/// 由 extractor 层从搜索查询参数构造，服务层只消费
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pagination {
    pub page: u64,
    pub page_size: u64,
}

impl Default for Pagination {
    fn default() -> Self {
        Self {
            page: DEFAULT_PAGE,
            page_size: DEFAULT_PAGE_SIZE,
        }
    }
}

impl Pagination {
    /// 从搜索查询参数的原始值构造，`None` 表示用户未传相关分页参数
    ///
    /// # Errors
    ///
    /// 非正整数、或 `page_size` 超过 [`MAX_PAGE_SIZE`] 时返回 [`AppError::BadRequest`]
    pub fn from_raw(page: Option<&str>, page_size: Option<&str>) -> Result<Self, AppError> {
        let page = match page {
            Some(raw) => parse_positive(raw, "page")?,
            None => DEFAULT_PAGE,
        };

        let page_size = match page_size {
            Some(raw) => {
                let value = parse_positive(raw, "pageSize")?;
                if value > MAX_PAGE_SIZE {
                    return Err(AppError::BadRequest(format!(
                        "'pageSize' must not exceed {MAX_PAGE_SIZE}."
                    )));
                }
                value
            }
            None => DEFAULT_PAGE_SIZE,
        };

        Ok(Self { page, page_size })
    }

    /// 要跳过的条目数，用于 [`Iterator::skip`]
    #[must_use]
    pub fn offset(&self) -> usize {
        let offset = self.page.saturating_sub(1).saturating_mul(self.page_size);
        usize::try_from(offset).unwrap_or(usize::MAX)
    }

    /// 本页最多取几条，用于 [`Iterator::take`]
    #[must_use]
    pub fn take(&self) -> usize {
        usize::try_from(self.page_size).unwrap_or(usize::MAX)
    }

    /// 给定总数时的总页数，总数为 0 时是 0 页
    #[must_use]
    pub const fn total_pages(&self, total: u64) -> u64 {
        total.div_ceil(self.page_size)
    }

    /// 当前页之后是否还有内容
    #[must_use]
    pub const fn has_more(&self, total: u64) -> bool {
        self.page < self.total_pages(total)
    }
}

fn parse_positive(raw: &str, field: &str) -> Result<u64, AppError> {
    let value: u64 = raw
        .trim()
        .parse()
        .map_err(|_| AppError::BadRequest(format!("'{field}' must be a positive integer.")))?;

    if value == 0 {
        return Err(AppError::BadRequest(format!(
            "'{field}' must be at least 1."
        )));
    }

    Ok(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginationInfo {
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
    pub total_pages: u64,
    pub has_more: bool,
}

/// 分页组合器输出结果，包含当前页的切片数据与完整的分页元数据信息
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paginated<T> {
    pub items: Vec<T>,
    pub pagination: PaginationInfo,
}

/// 泛型组合器：对全量 slice 或 Iterator 进行安全的 offset/take 内存分页，并自动构建完整的
/// [`PaginationInfo`] 元数据
pub fn paginate<T, I, F, R>(items: I, pagination: Pagination, mut transform: F) -> Paginated<R>
where
    I: IntoIterator<Item = T>,
    I::IntoIter: ExactSizeIterator,
    F: FnMut(T) -> R,
{
    let iter = items.into_iter();
    let total = u64::try_from(iter.len()).unwrap_or(u64::MAX);

    let paged_items: Vec<R> = iter
        .skip(pagination.offset())
        .take(pagination.take())
        .map(&mut transform)
        .collect();

    Paginated {
        items: paged_items,
        pagination: PaginationInfo {
            page: pagination.page,
            page_size: pagination.page_size,
            total,
            total_pages: pagination.total_pages(total),
            has_more: pagination.has_more(total),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_absent() {
        let p = Pagination::from_raw(None, None).unwrap();
        assert_eq!(p.page, 1);
        assert_eq!(p.page_size, 50);
        assert_eq!(p.offset(), 0);
        assert_eq!(p.take(), 50);
    }

    #[test]
    fn parses_explicit_values() {
        let p = Pagination::from_raw(Some("3"), Some("20")).unwrap();
        assert_eq!(p.page, 3);
        assert_eq!(p.page_size, 20);
        assert_eq!(p.offset(), 40);
        assert_eq!(p.take(), 20);
    }

    #[test]
    fn accepts_max_page_size() {
        let p = Pagination::from_raw(None, Some("100")).unwrap();
        assert_eq!(p.page_size, 100);
    }

    #[test]
    fn rejects_page_size_above_max() {
        let err = Pagination::from_raw(None, Some("101"));
        assert!(err.is_err());
    }

    #[test]
    fn rejects_zero_page() {
        assert!(Pagination::from_raw(Some("0"), None).is_err());
    }

    #[test]
    fn rejects_zero_page_size() {
        assert!(Pagination::from_raw(None, Some("0")).is_err());
    }

    #[test]
    fn rejects_non_numeric() {
        assert!(Pagination::from_raw(Some("abc"), None).is_err());
        assert!(Pagination::from_raw(None, Some("1.5")).is_err());
        assert!(Pagination::from_raw(Some("-1"), None).is_err());
    }

    #[test]
    fn total_pages_rounds_up() {
        let p = Pagination::from_raw(None, Some("20")).unwrap();
        assert_eq!(p.total_pages(0), 0);
        assert_eq!(p.total_pages(1), 1);
        assert_eq!(p.total_pages(20), 1);
        assert_eq!(p.total_pages(21), 2);
    }

    #[test]
    fn has_more_reflects_remaining_pages() {
        let first = Pagination::from_raw(Some("1"), Some("10")).unwrap();
        assert!(first.has_more(25));
        assert!(!first.has_more(10));
        assert!(!first.has_more(0));

        let last = Pagination::from_raw(Some("3"), Some("10")).unwrap();
        assert!(!last.has_more(25));
    }

    #[test]
    fn huge_page_saturates_instead_of_overflowing() {
        let p = Pagination::from_raw(Some(&u64::MAX.to_string()), Some("100")).unwrap();
        assert_eq!(p.offset(), usize::MAX);
        assert!(!p.has_more(1000));
    }

    #[test]
    fn test_paginate_combinator() {
        let data: Vec<i32> = (1..=25).collect();
        let pagination = Pagination::from_raw(Some("2"), Some("10")).unwrap();

        let paginated = paginate(data, pagination, |x| x * 2);

        assert_eq!(
            paginated.items,
            vec![22, 24, 26, 28, 30, 32, 34, 36, 38, 40]
        );
        assert_eq!(paginated.pagination.page, 2);
        assert_eq!(paginated.pagination.page_size, 10);
        assert_eq!(paginated.pagination.total, 25);
        assert_eq!(paginated.pagination.total_pages, 3);
        assert!(paginated.pagination.has_more);
    }
}
