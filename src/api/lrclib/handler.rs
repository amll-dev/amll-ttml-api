use worker::{
    Request,
    Response,
    RouteContext,
};

use crate::{
    api::lrclib::extractor::{
        extract_lrclib_get_query,
        extract_lrclib_search_query,
    },
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
    let query = extract_lrclib_search_query(&url)?;

    let max_results = 50;
    let items = LyricService::lrclib_search(&ctx, query, max_results).await?;

    Ok(Response::from_json(&items)?)
}

pub async fn handle_get(
    req: Request,
    ctx: RouteContext<worker::Context>,
) -> Result<Response, worker::Error> {
    match handle_get_inner(req, ctx).await {
        Ok(res) => Ok(res),
        Err(err) => err.to_response(),
    }
}

async fn handle_get_inner(
    req: Request,
    ctx: RouteContext<worker::Context>,
) -> Result<Response, AppError> {
    let url = req.url()?;
    let query = extract_lrclib_get_query(&url)?;

    let item = LyricService::lrclib_get_by_fields(&ctx, query).await?;

    Ok(Response::from_json(&item)?)
}

pub async fn handle_get_by_id(
    req: Request,
    ctx: RouteContext<worker::Context>,
) -> Result<Response, worker::Error> {
    match handle_get_by_id_inner(req, ctx).await {
        Ok(res) => Ok(res),
        Err(err) => err.to_response(),
    }
}

async fn handle_get_by_id_inner(
    _req: Request,
    ctx: RouteContext<worker::Context>,
) -> Result<Response, AppError> {
    let id_str = ctx
        .param("id")
        .ok_or(AppError::BadRequest("Missing ID parameter".into()))?;

    let id = id_str
        .parse::<u64>()
        .map_err(|_| AppError::BadRequest("Invalid ID format. Must be an integer.".into()))?;

    let item = LyricService::lrclib_get_by_id(&ctx, id).await?;

    Ok(Response::from_json(&item)?)
}
