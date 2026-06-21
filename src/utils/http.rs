use serde::Serialize;
use worker::{
    Response,
    Result,
};

/// 统一附加 CORS 头部的 JSON 响应包装
pub fn json_response<T: Serialize>(data: &T) -> Result<Response> {
    let mut response = Response::from_json(data)?;
    response
        .headers_mut()
        .set("Access-Control-Allow-Origin", "*")?;
    Ok(response)
}
