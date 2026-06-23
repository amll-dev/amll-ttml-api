use worker::{
    Request,
    Response,
    RouteContext,
};

use crate::{
    core::{
        error::AppError,
        models::SearchQuery,
    },
    services::lyric_service::LyricService,
};

pub async fn handle_search(
    req: Request,
    ctx: RouteContext<worker::Context>,
) -> Result<Response, worker::Error> {
    match handle_search_inner(req, ctx).await {
        Ok(res) => Ok(res),
        Err(err) => err.to_response(),
    }
}

async fn handle_search_inner(
    req: Request,
    ctx: RouteContext<worker::Context>,
) -> Result<Response, AppError> {
    let url = req.url()?;
    let query_pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let query = SearchQuery::from_http_query(&query_pairs)
        .ok_or_else(|| AppError::BadRequest("Missing valid search parameters.".into()))?;

    let max_results = 50;
    let result = LyricService::search_lyric(&ctx, query, max_results).await?;

    Ok({ Response::from_json(&result) }?)
}
