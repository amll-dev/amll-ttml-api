use worker::{
    Env,
    Request,
    Response,
    Router,
    event,
};

mod api;
mod core;
mod services;
mod utils;

#[event(fetch)]
pub async fn main(req: Request, env: Env, ctx: worker::Context) -> worker::Result<Response> {
    let mut router = Router::with_data(ctx);

    router = router.get_async("/api/get", api::get::handler::handle_get);
    router = router.get_async("/api/search", api::search::handler::handle_search);

    router = router.or_else_any_method_async("/*catchall", |_req, _ctx| async move {
        let mut res = Response::error("Not Found", 404)?;
        res.headers_mut().set("Access-Control-Allow-Origin", "*")?;
        Ok(res)
    });

    router.run(req, env).await
}
