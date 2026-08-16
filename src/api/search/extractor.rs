use crate::{
    api::shared::query::{
        NATIVE_SEARCH_DIALECT,
        parse_search_query,
    },
    core::{
        error::AppError,
        models::SearchQuery,
        pagination::Pagination,
    },
};

pub fn extract_search_query(query_str: &str) -> Result<(SearchQuery, Pagination), AppError> {
    parse_search_query(query_str, &NATIVE_SEARCH_DIALECT)
}
