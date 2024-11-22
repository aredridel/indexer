use crate::as_byte_filter::AsBytesFilter;
use crate::entry::Entry;
use anyhow::{bail, Context as AnyhowContext, Result};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::{Request, Response};
use std::fs;
use std::path::Path;
use tera::{Context, Tera};

pub async fn listing(
    req: &Request<Incoming>,
    root: &Path,
    path: &Path,
) -> Result<Response<Full<Bytes>>> {
    let mut tmpl_dir = path.to_path_buf();
    let mut tera: Option<Tera> = None;
    while tmpl_dir.starts_with(root) {
        let mut tmpl_path = tmpl_dir.clone();
        tmpl_path.push(".templates");
        tmpl_path.push("*.html");
        let tera_path_str = tmpl_path.to_str();
        if let Some(path) = tera_path_str {
            let tmpl = Tera::new(path).with_context(|| format!("template directory {:?}", path))?;
            if tmpl.get_template_names().any(|e| e == "index.html") {
                tera = Some(tmpl);
                break;
            }
        }
        tmpl_dir.pop();
    }

    let mut tera = match tera {
        None => bail!("no template directory found"),
        Some(tera) => tera,
    };

    tera.register_filter("as_bytes", AsBytesFilter);
    let mut context = Context::new();
    let mut entries =
        Entry::entries(path, true).with_context(|| format!("getting entries for {:?}", path))?;
    entries.sort_by_key(|f| f.file_name.clone());
    context.insert("entries", &entries);
    context.insert("path", req.uri().path());

    let mut desc_path = path.to_path_buf();
    desc_path.push("README.md");
    if let Ok(desc) = fs::read(desc_path) {
        let desc = String::from_utf8_lossy(&desc);
        let html = markdown::to_html(&desc);
        context.insert("description", &html);
    } else {
        context.insert("description", "");
    }
    let body = tera.render("index.html", &context)?;

    Ok(Response::builder()
        .header(CONTENT_TYPE, HeaderValue::from_static("text/html"))
        .body(Full::new(Bytes::from(body)))
        .unwrap())
}
