use etag::EntityTag;
use hyper::body::Incoming;
use hyper::Request;

pub trait EtagRejectable {
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
