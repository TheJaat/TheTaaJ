//! The HTTP server's request reader (net/src/http.rs).
#![no_main]
fuzz::fuzz_target!(|data: &[u8]| {
    let _ = net::http::head_length(data);
    if let Ok(request) = net::http::parse(data) {
        let _ = request.header("host");
        let _ = request.header("content-length");
    }
});
