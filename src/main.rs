use anyhow::Error;
use askama_hyper::Template;
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use image::imageops::FilterType::Lanczos3;
use image::{ImageFormat, ImageReader};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tokio::net::UnixListener;

#[derive(Template)]
#[template(path = "dirent.html")]
struct DirentTemplate {
    entries: Vec<fs::DirEntry>,
}

impl DirentTemplate {
    fn file_name(&self, entry: &fs::DirEntry) -> Result<String, Error> {
        entry
            .file_name()
            .to_str()
            .map(|x| x.to_owned())
            .ok_or_else(|| Error::msg("non utf-8 path"))
    }
    fn is_image(&self, entry: &fs::DirEntry) -> Result<bool, Error> {
        entry
            .file_name()
            .to_str()
            .ok_or_else(|| Error::msg("non utf-8 path"))
            .map(|f| f.ends_with(".png"))
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
            let handlers = Handlers;
            // Finally, we bind the incoming connection to our `listing` service
            if let Err(err) = http1::Builder::new()
                // `service_fn` converts our function in a `Service`
                .serve_connection(io, service_fn(|req| handlers.handle(req)))
                .await
            {
                eprintln!("Error serving connection: {:?}", err);
            }
        });
    }
}

struct Handlers;

impl Handlers {
    async fn handle(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Error> {
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
                self.thumbnail(&req, &path, q).await
            } else {
                self.listing(&path).await
            }
        } else {
            self.listing(&path).await
        }
    }

    async fn thumbnail(
        &self,
        _req: &Request<Incoming>,
        path: &Path,
        q: &str,
    ) -> Result<Response<Full<Bytes>>, Error> {
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
        Ok(Response::new(Full::from(buf)))
    }

    async fn listing(&self, path: &PathBuf) -> Result<Response<Full<Bytes>>, Error> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let file_name_os = entry.file_name();
            let filename = file_name_os.to_str();
            match filename {
                Some(s) => {
                    if s.starts_with(".") {
                        continue;
                    }
                }
                None => {
                    eprintln!("Non-utf-8 path found: {:?}", entry);
                    continue;
                }
            }
            entries.push(entry);
        }

        let ctx = DirentTemplate { entries };

        Ok(Response::builder()
            .header(CONTENT_TYPE, HeaderValue::from_static("text/html"))
            .body(Full::new(Bytes::from(ctx.render().unwrap())))
            .unwrap())
    }
}
