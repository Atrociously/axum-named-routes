#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! AxumNamedRouter
//! This is a router that wraps the [`Router`](axum::Router) from [`axum`]
//! and automatically adds an [`Extension<Routes>`](axum::extract::Extension) layer.
//!
//! Check out [`NamedRouter`] and [`Routes`] for more information on how this works

use std::{collections::HashMap, convert::Infallible, path::PathBuf, sync::Arc, task::Poll};

use axum::{
    body::{BoxBody, Bytes, HttpBody},
    extract::{
        connect_info::IntoMakeServiceWithConnectInfo, rejection::ExtensionRejection, FromRequest,
    },
    http::Request,
    response::Response,
    routing::{future::RouteFuture, IntoMakeService, Route},
    BoxError, Extension,
};
use futures::{future::BoxFuture, FutureExt, TryFutureExt};
use tower_layer::Layer;
use tower_service::Service;

type RouterRef<B> = axum::Router<B>;
type ServiceResp = Response<BoxBody>;
type ServiceErr = Infallible;

/// A mapping of all route names to their paths.
/// This can be used in requests as it implements [`FromRequest`](axum::extract::FromRequest)
/// It is also based on an [`Arc`](std::sync::Arc) internally so it can be cloned across requests
/// efficiently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Routes(Arc<HashMap<String, PathBuf>>);

impl Routes {
    /// Returns the route for the given name
    /// # Panics
    /// Panics if the name does not exist in routes
    pub fn has(&self, name: &str) -> &PathBuf {
        match self.0.get(name) {
            Some(path) => path,
            None => panic!("called `Routes::has` for a route that does not exist"),
        }
    }

    /// Tries to get the route for the given name
    /// if the route does not exist returns `None`
    pub fn get(&self, name: &str) -> Option<&PathBuf> {
        self.0.get(name)
    }

    /// Tries to get the route for the given name and takes an error
    /// to return if it does not exist
    pub fn get_or<E>(&self, name: &str, err: E) -> Result<&PathBuf, E> {
        self.0.get(name).ok_or(err)
    }

    /// Tries to get the route for the given name and takes an `FnOnce`
    /// to create an error if it does not exist
    pub fn get_or_else<F, E>(&self, name: &str, f: F) -> Result<&PathBuf, E>
    where
        F: FnOnce() -> E,
    {
        self.0.get(name).ok_or_else(f)
    }
}

impl Routes {
    fn new(map: HashMap<String, PathBuf>) -> Self {
        Self(Arc::new(map))
    }
}

impl<B> FromRequest<B> for Routes
where
    B: Send,
{
    type Rejection = ExtensionRejection;
    fn from_request<'life0, 'async_trait>(
        req: &'life0 mut axum::extract::RequestParts<B>,
    ) -> BoxFuture<'async_trait, Result<Self, Self::Rejection>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Extension::<Self>::from_request(req)
            .map_ok(|ext| ext.0)
            .boxed()
    }
}

/// Wraps the axum [`Router`](axum::Router) with an implementation
/// that builds a mapping of route names to paths.
///
/// Adds an [`Routes`] to the inner router as an [`Extension`](axum::extract::Extension) layer
/// when either [`into_make_service`](NamedRouter::into_make_service) or [`into_make_service_with_connect_info`](NamedRouter::into_make_service_with_connect_info)
/// are called.
#[derive(Clone, Debug)]
pub struct NamedRouter<B = axum::body::Body> {
    inner: RouterRef<B>,
    routes: HashMap<String, PathBuf>,
    nest_sep: String,
}

impl NamedRouter<axum::body::Body> {
    /// Create a new NamedRouter with default values.
    /// The default name separator is `.`
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new NamedRouter with a different name separator than the default
    pub fn with_separator<T: Into<String>>(sep: T) -> Self {
        Self {
            nest_sep: sep.into(),
            ..Default::default()
        }
    }
}

impl<B> NamedRouter<B>
where
    B: HttpBody + Send + 'static,
{
    /// Set the separator for the router to use when nesting
    pub fn set_separator<T: Into<String>>(mut self, sep: T) -> Self {
        self.nest_sep = sep.into();
        self
    }

    /// The same as [`Router::fallback`](axum::Router::fallback)
    #[inline]
    pub fn fallback<S>(mut self, service: S) -> Self
    where
        S: Service<Request<B>, Response = ServiceResp, Error = ServiceErr> + Clone + Send + 'static,
        S::Future: Send + 'static,
    {
        self.inner = self.inner.fallback(service);
        self
    }

    /// Uses [`Router::into_make_service`](axum::Router::into_make_service) after
    /// adding an [`Extension<Routes>`](axum::extract::Extension) layer to the inner router
    pub fn into_make_service(self) -> IntoMakeService<RouterRef<B>> {
        let inner = self.inner.layer(Extension(Routes::new(self.routes)));
        inner.into_make_service()
    }

    /// Uses [`Router::into_make_service_with_connect_info`](axum::Router::into_make_service_with_connect_info) after
    /// adding an [`Extension<Routes>`](axum::extract::Extension) layer to the inner router
    pub fn into_make_service_with_connect_info<C>(
        self,
    ) -> IntoMakeServiceWithConnectInfo<RouterRef<B>, C> {
        let inner = self.inner.layer(Extension(Routes::new(self.routes)));
        inner.into_make_service_with_connect_info()
    }

    /// The same as [`Router::layer`](axum::Router::layer)
    #[inline]
    pub fn layer<L, NewReqBody, NewResBody>(self, layer: L) -> NamedRouter<NewReqBody>
    where
        L: Layer<Route<B>>,
        L::Service: Service<Request<NewReqBody>, Response = Response<NewResBody>, Error = ServiceErr>
            + Clone
            + Send
            + 'static,
        <L::Service as Service<Request<NewReqBody>>>::Future: Send + 'static,
        NewReqBody: HttpBody + Send + 'static,
        NewResBody: HttpBody<Data = Bytes> + Send + 'static,
        NewResBody::Error: Into<BoxError>,
    {
        let routes = self.routes;
        let nest_sep = self.nest_sep;
        let inner = self.inner.layer(layer);
        NamedRouter {
            inner,
            routes,
            nest_sep,
        }
    }

    /// The merges the inner axum [`Router`](axum::Router) and the route map
    pub fn merge<R>(mut self, other: R) -> Self
    where
        R: Into<Self>,
    {
        let other = other.into();
        self.inner = self.inner.merge(other.inner);
        self.routes.extend(other.routes);
        self
    }

    /// Nests the inner axum [`Router`](axum::Router).
    ///
    /// When nesting the router adds `name` as a prefix to all route names in `router`.
    /// The the name nesting process looks essentially like `name + separator + route_name`
    /// for example:
    /// ```
    /// use std::path::PathBuf;
    /// use axum::routing::get;
    /// use axum_named_routes::{NamedRouter, Routes};
    ///
    /// async fn index(routes: Routes) -> &'static str {
    ///     "Hello, World!"
    /// }
    ///
    /// let ui_router = NamedRouter::new()
    ///     .route("index", "/", get(index));
    /// let base = NamedRouter::new()
    ///     .nest("ui", "/", ui_router);
    ///
    /// let routes = base.routes();
    /// assert!(routes.get("ui.index").is_some());
    /// assert_eq!(routes.get("ui.index").unwrap(), &PathBuf::from("/"));
    /// ```
    ///
    /// Also ensures all paths in `router` are joined to `path` uses
    /// [`Path::join`](std::path::Path::join) like `path.join(route_path)`
    pub fn nest<N, P, R>(mut self, name: N, path: P, router: R) -> Self
    where
        N: Into<String>,
        P: AsRef<str>,
        R: Into<Self>,
    {
        let name = name.into();
        let router = router.into();
        self.inner = self.inner.nest(path.as_ref(), router.inner);
        let path = PathBuf::from(path.as_ref());

        let prefixed_routes = router.routes.into_iter().map(|(inner_name, inner_path)| {
            // This is correct because axum routers panic when trying to insert a path that does
            // not start with a "/" meaning inner_path is guaranteed to start with a "/" but that
            // also means if we don't remove it then the path.join will fail to properly join the
            // paths as it will think inner_path is an absolute path
            #[allow(clippy::unwrap_used)]
            let inner_path = inner_path.strip_prefix("/").unwrap();
            (
                name.clone() + &self.nest_sep + &inner_name,
                path.join(inner_path),
            )
        });
        self.routes.extend(prefixed_routes);

        self
    }

    /// Add a service the the router with a name and a path
    /// the name can then later be used to get a reference to the path
    pub fn route<N, P, S>(mut self, name: N, path: P, service: S) -> Self
    where
        N: Into<String>,
        P: AsRef<str>,
        S: Service<Request<B>, Response = ServiceResp, Error = ServiceErr> + Clone + Send + 'static,
        S::Future: Send,
    {
        self.inner = self.inner.route(path.as_ref(), service);
        self.routes
            .insert(name.into(), PathBuf::from(path.as_ref()));
        self
    }

    /// The same as [`Router::route_layer`](axum::Router::route_layer)
    #[inline]
    pub fn route_layer<L, NewResBody>(mut self, layer: L) -> Self
    where
        L: Layer<Route<B>>,
        L::Service: Service<Request<B>, Response = Response<NewResBody>, Error = ServiceErr>
            + Clone
            + Send
            + 'static,
        <L::Service as Service<Request<B>>>::Future: Send + 'static,
        NewResBody: HttpBody<Data = Bytes> + Send + 'static,
        NewResBody::Error: Into<BoxError>,
    {
        self.inner = self.inner.route_layer(layer);
        self
    }

    /// Get a reference to the routes mapping before turning it into a [`Routes`]
    pub fn routes(&self) -> &HashMap<String, PathBuf> {
        &self.routes
    }
}

impl<B> Service<Request<B>> for NamedRouter<B>
where
    B: HttpBody + Send + 'static,
{
    type Response = ServiceResp;
    type Error = ServiceErr;
    type Future = RouteFuture<B, ServiceErr>;

    #[inline]
    fn poll_ready(
        &mut self,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    #[inline]
    fn call(&mut self, req: Request<B>) -> Self::Future {
        self.inner.call(req)
    }
}

impl<B> Default for NamedRouter<B>
where
    B: HttpBody + Send + 'static,
{
    fn default() -> Self {
        Self {
            inner: RouterRef::new(),
            routes: HashMap::default(),
            nest_sep: ".".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::path::PathBuf;

    use crate::NamedRouter;
    use axum::routing::get;

    async fn dummy() {}

    #[test]
    fn test_nesting() {
        let a = NamedRouter::new().route("route_a", "/a", get(dummy));
        let b = NamedRouter::new().route("route_a", "/a", get(dummy)).route(
            "route_b",
            "/b",
            get(dummy),
        );
        let c = NamedRouter::new().route("route_c", "/c", get(dummy));

        let app = NamedRouter::new()
            .nest("a", "/", a)
            .nest("b", "/b", b)
            .nest("c", "/b", c);
        let routes = app.routes();

        assert!(routes.get("a.route_a").unwrap() == &PathBuf::from("/a"));
        assert!(routes.get("b.route_a").unwrap() == &PathBuf::from("/b/a"));
        assert!(routes.get("b.route_b").unwrap() == &PathBuf::from("/b/b"));
        assert!(routes.get("c.route_c").unwrap() == &PathBuf::from("/b/c"));
    }

    #[test]
    #[should_panic]
    fn test_route_overlap() {
        let a = NamedRouter::new().route("route_a", "/a", get(dummy));
        let b = NamedRouter::new().route("route_a", "/a", get(dummy));
        NamedRouter::new().nest("a", "/", a).nest("b", "/", b);
    }
}
