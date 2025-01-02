#[macro_use] extern crate rocket;
mod as_byte_filter;
mod entry;
mod etag_rejectable;
mod listing;
mod thumbnail;

use crate::listing::listing;
use crate::thumbnail::thumbnail;
use anyhow::{bail, Context as AnyhowContext, Error, Result as AnyhowResult};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::header::CONTENT_TYPE;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use std::convert::Infallible;
use std::fs;
#[cfg(target_os = "linux")]
use std::os::fd::FromRawFd;
use std::path::PathBuf;
use std::str::FromStr;
use tokio::net::UnixListener;
use urlencoding::decode as urldecode;

#[get("/")]
fn index() -> &'static str {
    "Hello, world!"
}

#[launch]
fn rocket() -> _ {
    rocket::build().mount("/", routes![index])
}

#[tokio::main]
async fn xmain() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
    let root = req
        .headers()
        .get("X-Index-Root")
        .ok_or_else(|| Error::msg("missing X-Index-Root header"))?
        .to_str()
        .with_context(|| "cannot convert root to string")
        .and_then(|s| urldecode(s).with_context(|| "could not decode root as UTF-8"))
        .map(|s| PathBuf::from_str(&s).unwrap())?
        .canonicalize()
        .with_context(|| "could not canonicalize root")?;

    let base = req
        .headers()
        .get("X-Index-URL-Base")
        .ok_or_else(|| Error::msg("missing X-Index-URL-Base header"))?
        .to_str()?;

    let uripath = req.uri().path().to_string();
    if !uripath.starts_with(base) {
        bail!("path is outside of base");
    }

    let uripath =
        urldecode(&uripath[base.len() + 1..]).with_context(|| "could not decode path as UTF-8")?;

    let path = root
        .join(PathBuf::from(&uripath.to_string()))
        .canonicalize()
        .with_context(|| {
            format!(
                "could not canonicalize path {:?} within {:?}",
                uripath, root
            )
        })?;

    if !path.starts_with(&root) {
        bail!("path is outside of root");
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
