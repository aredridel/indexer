use anyhow::Error;
use askama_hyper::Template;
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use std::fs::DirEntry;
use std::path::PathBuf;
use std::str::FromStr;
use std::{fs, process};
use tokio::net::UnixListener;

#[derive(Template)]
#[template(path = "dirent.html")]
struct DirentTemplate {
    entries: Vec<DirEntry>,
}

impl DirentTemplate {
    fn file_name(&self, entry: &DirEntry) -> Result<String, Error> {
        entry
            .file_name()
            .to_str()
            .map(|x| x.to_owned())
            .ok_or_else(|| Error::msg("non utf-8 path"))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    ctrlc::set_handler(move || {
        fs::remove_file("warp.sock").unwrap();
        process::exit(0);
    })
    .unwrap();
    let listener = UnixListener::bind("warp.sock").unwrap();

    // We start a loop to continuously accept incoming connections
    loop {
        let (stream, _) = listener.accept().await?;

        // Use an adapter to access something implementing `tokio::io` traits as if they implement
        // `hyper::rt` IO traits.
        let io = TokioIo::new(stream);

        // Spawn a tokio task to serve multiple connections concurrently
        tokio::task::spawn(async move {
            // Finally, we bind the incoming connection to our `listing` service
            if let Err(err) = http1::Builder::new()
                // `service_fn` converts our function in a `Service`
                .serve_connection(io, service_fn(handle))
                .await
            {
                eprintln!("Error serving connection: {:?}", err);
            }
        });
    }
}

async fn handle(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Error> {
    if let Some(q) = req.uri().query() {
        if q.contains("image") {
            image_handler(req).await
        } else {
            listing(req).await
        }
    } else {
        listing(req).await
    }
}

async fn image_handler(_req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Error> {
    Ok(Response::new(Full::from("")))
}

async fn listing(req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Error> {
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
