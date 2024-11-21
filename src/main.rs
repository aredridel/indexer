use anyhow::Error;
use etag::EntityTag;
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderValue, CONTENT_TYPE, ETAG};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use image::imageops::FilterType::Lanczos3;
use image::{ImageFormat, ImageReader};
use serde::Serialize;
use std::convert::Infallible;
use std::error::Error as StdError;
use std::fs;
use std::io::Cursor;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tera::{Context, Tera};
use tokio::net::UnixListener;

mod as_byte_filter;
use as_byte_filter::AsBytesFilter;

struct DirentTemplate;

#[derive(Serialize, Debug)]
struct Entry {
    file_name: String,
    size: u64,
    is_dir: bool,
    is_image: bool,
    children: Option<Vec<Entry>>,
}

impl DirentTemplate {
    fn entries(&self, path: &Path, include_children: bool) -> Result<Vec<Entry>, impl StdError> {
        fs::read_dir(path).map(|r| {
            r.filter_map(|e| {
                e.map_or(None, |de| {
                    if self.is_hidden(&de) {
                        None
                    } else {
                        let e = self.entry_for_dirent(&de, include_children);
                        match e {
                            Ok(ent) => Some(ent),
                            Err(_) => None,
                        }
                    }
                })
            })
            .collect()
        })
    }

    fn entry_for_dirent(&self, de: &fs::DirEntry, include_children: bool) -> Result<Entry, Error> {
        let is_dir = de.file_type()?.is_dir();
        let metadata = de.metadata()?;
        Ok(Entry {
            file_name: de
                .file_name()
                .into_string()
                .map_err(|_e| Error::msg("non-utf-8 filename"))?,
            is_image: self.is_image(de)?,
            is_dir,
            size: metadata.size(),
            children: if is_dir && include_children {
                Some(self.entries(&de.path(), false)?)
            } else {
                None
            },
        })
    }

    fn is_hidden(&self, entry: &fs::DirEntry) -> bool {
        entry
            .file_name()
            .to_str()
            .map(|s| s.starts_with("."))
            .unwrap_or(true)
    }

    fn is_image(&self, entry: &fs::DirEntry) -> Result<bool, Error> {
        entry
            .file_name()
            .to_str()
            .ok_or_else(|| Error::msg("non utf-8 path"))
            .map(|f| f.ends_with(".png") || f.ends_with(".jpg") || f.ends_with(".jpeg"))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if fs::exists("warp.sock")? {
        fs::remove_file("warp.sock")?;
    }
    let listener = UnixListener::bind("warp.sock").unwrap();

    // We start a loop to continuously accept incoming connections
    loop {
        let (stream, _) = listener.accept().await?;

        // Use an adapter to access something implementing `tokio::io` traits as if they implement
        // `hyper::rt` IO traits.
        let io = TokioIo::new(stream);

        // Spawn a tokio task to serve multiple connections concurrently
        tokio::task::spawn(async {
            // Finally, we bind the incoming connection to our `listing` service
            if let Err(err) = http1::Builder::new()
                // `service_fn` converts our function in a `Service`
                .serve_connection(io, service_fn(handle_and_map_err))
                .await
            {
                eprintln!("Error serving connection: {:?}", err);
            }
        });
    }
}

async fn handle_and_map_err(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let result = handle(req).await;
    match result {
        Err(err) => {
            eprintln!("{:?}", err.to_string());
            Ok(Response::builder()
                .status(500)
                .body(Full::from(err.to_string()))
                .unwrap())
        }
        Ok(response) => Ok(response),
    }
}

async fn handle(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Error> {
    let root = PathBuf::from_str(
        req.headers()
            .get("X-Index-Root")
            .expect("X-Index-Root header is absent")
            .to_str()?,
    )?
    .canonicalize()?;

    let path = root
        .join(PathBuf::from(req.uri().path().to_string().split_off(1)))
        .canonicalize()?;

    if !path.starts_with(&root) {
        return Err(Error::msg("path is outside of root"));
    }

    if let Some(q) = req.uri().query() {
        if q.contains("thumbnail") {
            thumbnail(&req, &path, q).await
        } else {
            listing(&root, &path).await
        }
    } else {
        listing(&root, &path).await
    }
}

trait EtagRejectable {
    fn satisfies_request(&self, req: &Request<Incoming>) -> bool;
}

impl EtagRejectable for EntityTag {
    fn satisfies_request(&self, req: &Request<Incoming>) -> bool {
        if let Some(h) = req.headers().get("if-none-match") {
            if let Ok(check_etag) = h.to_str().unwrap_or("").parse::<EntityTag>() {
                return check_etag.weak_eq(self);
            }
        }
        false
    }
}

async fn thumbnail(
    req: &Request<Incoming>,
    path: &Path,
    q: &str,
) -> Result<Response<Full<Bytes>>, Error> {
    let et = EntityTag::from_file_meta(&fs::metadata(path)?);
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
        .body(Full::from(buf))?;
    Ok(res)
}

async fn listing(root: &Path, path: &Path) -> Result<Response<Full<Bytes>>, Error> {
    let ctx = DirentTemplate {};

    let mut tmpl_dir = path.to_path_buf();
    let mut tmpl: Option<String> = None;
    while tmpl_dir.starts_with(root) {
        let mut tmpl_path = tmpl_dir.clone();
        tmpl_path.push(".index.tmpl.html");
        let contents = fs::read(tmpl_path);
        if contents.is_ok() {
            let t = String::from_utf8(contents.unwrap())?;
            tmpl = Some(t);
            break;
        }
        tmpl_dir.pop();
    }

    if tmpl.is_none() {
        return Err(Error::msg("no template found"));
    }

    let mut tera = Tera::default();
    tera.register_filter("as_bytes", AsBytesFilter);
    tera.add_raw_template("index", &tmpl.unwrap())?;
    let mut context = Context::new();
    let mut entries = ctx.entries(path, true)?;
    entries.sort_by_key(|f| f.file_name.clone());
    context.insert("entries", &entries);
    let body = tera.render("index", &context)?;

    Ok(Response::builder()
        .header(CONTENT_TYPE, HeaderValue::from_static("text/html"))
        .body(Full::new(Bytes::from(body)))
        .unwrap())
}
