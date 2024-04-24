/*
    Relay req to upstream server

    - find host from request
    - use host to copy upstream URI from address map
    - replace the path_and_query of the upstream URI with the path_and_query of request URI
    - request URI is replaced by the the destinataion URI
    - updated request is relayed to the upstream server

    Errors can stem from both the current server and the upstream server.
    This server returns HTTP 502 for all failed request originating from this server.
    Response body is a semi-informative error.
*/

use hyper::body::Incoming;
use hyper::service::Service;
use hyper::{Request, StatusCode};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::requests;

const HTTP: &str = "http";
const HTTPS: &str = "https";
const HOST: &str = "host";
const URI_FROM_REQUEST_ERROR: &str = "failed to parse URI from request";
const UPSTREAM_URI_ERROR: &str = "falied to create an upstream URI from request";

pub struct Svc {
    pub addresses: Arc<HashMap<String, http::Uri>>,
}

impl Service<Request<Incoming>> for Svc {
    type Response = requests::BoxedResponse;
    type Error = http::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<Incoming>) -> Self::Future {
        // http1 and http2 headers
        let arrival_uri = match get_host_from_request(&req) {
            Some(uri) => uri,
            _ => {
                return Box::pin(async {
                    // bad request
                    requests::create_error_response(
                        &StatusCode::BAD_GATEWAY,
                        &URI_FROM_REQUEST_ERROR,
                    )
                });
            }
        };

        // updated req
        let updated_req = match update_request_with_dest_uri(req, &self.addresses, &arrival_uri) {
            Some(uri) => uri,
            _ => {
                return Box::pin(async {
                    requests::create_error_response(&StatusCode::BAD_GATEWAY, &UPSTREAM_URI_ERROR)
                })
            }
        };

        return Box::pin(async {
            let version = updated_req.version();
            let scheme = match updated_req.uri().scheme() {
                Some(a) => a.as_str(),
                _ => HTTP,
            };

            match (version, scheme) {
                (hyper::Version::HTTP_2, HTTPS) => {
                    requests::send_http2_tls_request(updated_req).await
                }
                (hyper::Version::HTTP_2, HTTP) => requests::send_http2_request(updated_req).await,
                (_, HTTPS) => requests::send_http1_tls_request(updated_req).await,
                _ => requests::send_http1_request(updated_req).await,
            }
        });
    }
}

fn get_host_from_request(req: &Request<Incoming>) -> Option<String> {
    // http2
    if req.version() == hyper::Version::HTTP_2 {
        return match req.uri().host() {
            Some(s) => Some(s.to_string()),
            _ => None,
        };
    }

    // http1.1
    let host_str = match req.headers().get(HOST) {
        Some(h) => match h.to_str() {
            Ok(hst) => hst,
            _ => return None,
        },
        _ => return None,
    };

    // verify host header is a URI
    let uri = match http::Uri::try_from(host_str) {
        Ok(u) => u,
        _ => return None,
    };

    match uri.host() {
        Some(host) => Some(host.to_string()),
        _ => None,
    }
}

// possibly more efficient to manipulate strings
fn update_request_with_dest_uri(
    mut req: Request<Incoming>,
    addresses: &HashMap<String, http::Uri>,
    uri: &str,
) -> Option<Request<Incoming>> {
    let mut dest_parts = match addresses.get(uri) {
        Some(dest_uri) => dest_uri.clone().into_parts(),
        _ => return None,
    };
    dest_parts.path_and_query = req.uri().path_and_query().cloned();

    *req.uri_mut() = match http::Uri::from_parts(dest_parts) {
        Ok(u) => u,
        _ => return None,
    };

    Some(req)
}
