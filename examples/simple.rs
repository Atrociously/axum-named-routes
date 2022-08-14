use axum::routing::get;
use axum_named_routes::{NamedRouter, Routes};
use std::{net::SocketAddr, path::PathBuf};

async fn index() -> &'static str {
    "Hello, World!"
}

// gets the routes from axum extensions
async fn nested_other(routes: Routes) {
    // this could panic if the name is not in the Routes map
    // but we know that it is because we got here
    let this_route = routes.has("ui.other");
    assert_eq!(this_route, &PathBuf::from("/ui/other"));
}

async fn other(routes: Routes) {
    // the get function does not panic rather it returns an Option
    let route = routes.get("ui.other");
    let this_route = routes.get("other");
    assert_ne!(route, this_route);
}

#[tokio::main]
async fn main() {
    let ui = NamedRouter::new().route("index", "/", get(index)).route(
        "other",
        "/other",
        get(nested_other),
    );
    let app = NamedRouter::new()
        .nest("ui", "/ui/", ui)
        .route("other", "/other", get(other));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
