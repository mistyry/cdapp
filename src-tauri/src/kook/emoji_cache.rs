use std::fs;

use super::api::Api;

const HOST: &str = "https://img.kookapp.cn/emojis/";

fn id_of(path: &str) -> Option<String> {
    let id = path.trim_start_matches('/').trim_end_matches(".png");
    super::emoji::valid_id(id).then(|| id.to_owned())
}

fn file_of(id: &str) -> Option<std::path::PathBuf> {
    crate::storage::cache_file(&format!("emojis/{}.png", id.replace('/', "_")))
}

pub async fn get(path: &str) -> Option<Vec<u8>> {
    let id = id_of(path)?;
    let file = file_of(&id);
    if let Some(b) = file
        .as_ref()
        .and_then(|p| fs::read(p).ok())
        .filter(|b| !b.is_empty())
    {
        return Some(b);
    }
    let bytes = Api::new(None)
        .ok()?
        .get_bytes(&format!("{HOST}{id}.png"))
        .await;
    let bytes = match bytes {
        Ok(b) => b.to_vec(),
        Err(e) => {
            log::debug!("表情下载失败：{e}");
            return None;
        }
    };
    if let Some(Err(e)) = file.map(|p| crate::storage::write(&p, &bytes)) {
        log::debug!("表情缓存写入失败：{e}");
    }
    Some(bytes)
}

pub fn protocol<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol("emoji", |_ctx, request, responder| {
        let path = request.uri().path().to_owned();
        tauri::async_runtime::spawn(async move {
            let res = match get(&path).await {
                Some(body) => tauri::http::Response::builder()
                    .header("Cache-Control", "max-age=31536000, immutable")
                    .header("Access-Control-Allow-Origin", "*")
                    .body(body),
                None => tauri::http::Response::builder()
                    .status(404)
                    .body(Vec::new()),
            };
            if let Ok(r) = res {
                responder.respond(r);
            }
        });
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_from_path() {
        assert_eq!(id_of("/100/aB_1.png").as_deref(), Some("100/aB_1"));
        assert_eq!(id_of("/1/22/abc").as_deref(), Some("1/22/abc"));
        assert_eq!(id_of("/../etc/passwd"), None);
        assert_eq!(id_of("/x"), None);
    }
}
