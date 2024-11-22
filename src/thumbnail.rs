use crate::etag_rejectable::EtagRejectable;
use anyhow::{Context, Result};
use etag::EntityTag;
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::{CONTENT_TYPE, ETAG};
use hyper::{Request, Response};
use image::imageops::FilterType::Lanczos3;
use image::{ImageFormat, ImageReader};
use std::fs;
use std::io::Cursor;
use std::path::Path;

pub async fn thumbnail(
    req: &Request<Incoming>,
    path: &Path,
    q: &str,
) -> Result<Response<Full<Bytes>>> {
    let md = fs::metadata(path).with_context(|| format!("file {:?}", path))?;
    let et = EntityTag::from_file_meta(&md);
    if et.satisfies_request(req) {
        return Ok(Response::builder().status(304).body(Full::from(""))?);
    }
    let mut parsed = form_urlencoded::parse(q.as_bytes());
    let mut thumb = path.to_path_buf();
    thumb.push(
        parsed
            .find_map(|(k, v)| if k == "thumbnail" { Some(v) } else { None })
            .unwrap()
            .to_string(),
    );

    let image = ImageReader::open(thumb)?.decode()?;
    let resized = image.resize(50, 50, Lanczos3);

    let mut buf: Vec<u8> = Vec::new();
    let mut cursor = Cursor::new(&mut buf);

    resized.write_to(&mut cursor, ImageFormat::Png)?;
    let res = Response::builder()
        .header(ETAG, et.to_string())
        .header(CONTENT_TYPE, "image/png")
        .body(Full::from(buf))?;
    Ok(res)
}
