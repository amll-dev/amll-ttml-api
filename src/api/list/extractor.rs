use crate::{
    api::shared::query::parse_list_query,
    core::{
        error::AppError,
        list_query::ListQuery,
    },
};

pub fn extract_list_query(query_str: &str) -> Result<ListQuery, AppError> {
    parse_list_query(query_str)
}
