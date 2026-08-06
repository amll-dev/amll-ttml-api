use crate::{
    api::shared::{
        pagination::Pagination,
        query::{
            NATIVE_SEARCH_DIALECT,
            parse_search_query,
        },
    },
    core::{
        error::AppError,
        models::SearchQuery,
    },
};

pub fn extract_search_query(query_str: &str) -> Result<(SearchQuery, Pagination), AppError> {
    parse_search_query(query_str, &NATIVE_SEARCH_DIALECT)
}
