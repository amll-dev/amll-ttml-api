use crate::{
    api::shared::query::{
        LRCLIB_GET_DIALECT,
        LRCLIB_SEARCH_DIALECT,
        parse_search_query,
        parse_search_query_exact,
    },
    core::{
        error::AppError,
        models::SearchQuery,
        pagination::Pagination,
    },
};

pub fn extract_lrclib_search_query(query_str: &str) -> Result<(SearchQuery, Pagination), AppError> {
    parse_search_query(query_str, &LRCLIB_SEARCH_DIALECT)
}

pub fn extract_lrclib_get_query(query_str: &str) -> Result<SearchQuery, AppError> {
    parse_search_query_exact(query_str, &LRCLIB_GET_DIALECT)
}
