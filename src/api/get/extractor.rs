pub use crate::api::shared::query::GetQuery;
use crate::{
    api::shared::query::parse_get_query,
    core::error::AppError,
};

pub fn extract_get_query(query_str: &str) -> Result<GetQuery, AppError> {
    parse_get_query(query_str)
}
