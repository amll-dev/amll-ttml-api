use worker::{
    Request,
    Response,
    RouteContext,
};

use crate::{
    core::{
        error::AppError,
        models::IdQuery,
    },
    services::lyric_service::LyricService,
    utils::http::json_response,
};

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
    let query_pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();

    let query = IdQuery::from_http_query(&query_pairs)
        .ok_or_else(|| AppError::BadRequest("Only valid query parameters are allowed.".into()))?;

    let result = LyricService::get_lyric(&ctx, query).await?;
    Ok(json_response(&result)?)
}
