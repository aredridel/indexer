use anyhow::{Context as AnyhowContext, Error, Result as AnyhowResult};
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
use std::convert::Infallible;
use std::fs;
use std::io::Cursor;
#[cfg(target_os = "linux")]
use std::os::fd::FromRawFd;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tera::{Context, Tera};
use tokio::net::UnixListener;

mod as_byte_filter;
use as_byte_filter::AsBytesFilter;

mod entry;
use entry::Entry;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(target_os = "linux")]
    let listenfds = systemd::daemon::listen_fds(false)?;
    #[cfg(target_os = "linux")]
    let listener = if listenfds.is_empty() {
        if fs::exists("warp.sock")? {
            fs::remove_file("warp.sock")?;
        }
        UnixListener::bind("warp.sock").unwrap()
    } else {
        let std_listener = unsafe {
            std::os::unix::net::UnixListener::from_raw_fd(listenfds.iter().next().unwrap())
        };
        std_listener.set_nonblocking(true)?;
        UnixListener::from_std(std_listener).unwrap()
    };
    #[cfg(not(target_os = "linux"))]
    if fs::exists("warp.sock")? {
        fs::remove_file("warp.sock")?;
    }
    #[cfg(not(target_os = "linux"))]
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
            eprintln!("{:?}", err);
            Ok(Response::builder()
                .status(500)
                .header(CONTENT_TYPE, "text/plain")
                .body(Full::from(format!("{:?}", err)))
                .unwrap())
        }
        Ok(response) => Ok(response),
    }
}

async fn handle(req: Request<Incoming>) -> AnyhowResult<Response<Full<Bytes>>> {
    let root = PathBuf::from_str(
        req.headers()
            .get("X-Index-Root")
            .ok_or_else(|| Error::msg("missing X-Index-Root header"))?
            .to_str()?,
    )?
    .canonicalize()
    .with_context(|| "could not canonicalize root")?;

    let base = req
        .headers()
        .get("X-Index-URL-Base")
        .ok_or_else(|| Error::msg("missing X-Index-URL-Base header"))?
        .to_str()?;

    let uripath = req.uri().path().to_string();
    if !uripath.starts_with(base) {
        return Err(Error::msg("path is outside of base"));
    }

    let uripath = uripath[base.len() + 1..].to_string();

    let path = root
        .join(PathBuf::from(&uripath))
        .canonicalize()
        .with_context(|| format!("could not canonicalize path {:?}", uripath))?;

    if !path.starts_with(&root) {
        return Err(Error::msg("path is outside of root"));
    }

    if let Some(q) = req.uri().query() {
        if q.contains("thumbnail") {
            thumbnail(&req, &path, q).await
        } else {
            listing(&req, &root, &path).await
        }
    } else {
        listing(&req, &root, &path).await
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
) -> AnyhowResult<Response<Full<Bytes>>> {
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

async fn listing(
    req: &Request<Incoming>,
    root: &Path,
    path: &Path,
) -> AnyhowResult<Response<Full<Bytes>>> {
    let mut tmpl_dir = path.to_path_buf();
    let mut tera: Option<Tera> = None;
    while tmpl_dir.starts_with(root) {
        let mut tmpl_path = tmpl_dir.clone();
        tmpl_path.push(".templates");
        tmpl_path.push("*.html");
        let tera_path_str = tmpl_path.to_str();
        if let Some(path) = tera_path_str {
            let tmpl = Tera::new(path)?;
            if tmpl.get_template_names().any(|e| e == "index.html") {
                tera = Some(tmpl);
                break;
            }
        }
        tmpl_dir.pop();
    }

    let mut tera = match tera {
        None => return Err(Error::msg("no template directory found")),
        Some(tera) => tera,
    };

    tera.register_filter("as_bytes", AsBytesFilter);
    let mut context = Context::new();
    let mut entries = Entry::entries(path, true)?;
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
