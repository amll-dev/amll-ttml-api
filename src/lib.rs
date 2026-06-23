use worker::{
    Env,
    Method,
    Request,
    Response,
    Router,
    event,
};

use crate::core::error::AppError;

mod api;
mod core;
mod services;
mod utils;

#[event(fetch)]
pub async fn main(req: Request, env: Env, ctx: worker::Context) -> worker::Result<Response> {
    if req.method() == Method::Options {
        return utils::cors::handle_preflight();
    }

    let mut router = Router::with_data(ctx);

    router = router.get_async("/api/get", api::get::handler::handle_get);
    router = router.get_async("/api/search", api::search::handler::handle_search);

    router = router.or_else_any_method_async("/*catchall", |_req, _ctx| async move {
        AppError::NotFound.to_response()
    });

    let mut res = router.run(req, env).await?;

    utils::cors::add_cors_headers(&mut res)?;

    Ok(res)
}
