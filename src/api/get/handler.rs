use worker::{
    Request,
    Response,
    RouteContext,
};

use crate::{
    api::get::extractor::extract_id_query,
    core::error::AppError,
    services::lyric_service::LyricService,
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

    let query = extract_id_query(&url)?;

    let result = LyricService::get_lyric(&ctx, query).await?;
    Ok({ Response::from_json(&result) }?)
}
