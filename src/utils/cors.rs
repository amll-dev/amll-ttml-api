use worker::{
    Response,
    Result,
};

/// 统一为所有响应追加跨域头
pub fn add_cors_headers(res: &mut Response) -> Result<()> {
    res.headers_mut().set("Access-Control-Allow-Origin", "*")?;
    Ok(())
}

/// 专门处理预检请求
pub fn handle_preflight() -> Result<Response> {
    let mut res = Response::empty()?.with_status(204);

    add_cors_headers(&mut res)?;

    let headers = res.headers_mut();
    headers.set("Access-Control-Allow-Methods", "GET, POST, OPTIONS")?;
    headers.set("Access-Control-Allow-Headers", "*")?;
    headers.set("Access-Control-Max-Age", "86400")?;

    Ok(res)
}
