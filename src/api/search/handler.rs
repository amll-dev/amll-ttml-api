use worker::{
    Request,
    Response,
    RouteContext,
};

use crate::{
    api::search::extractor::extract_search_query,
    core::error::AppError,
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

    let query = extract_search_query(&url)?;

    let max_results = 50;
    let result = LyricService::search_lyric(&ctx, query, max_results).await?;

    Ok(Response::from_json(&result)?)
}
